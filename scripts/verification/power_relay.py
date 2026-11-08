#!/usr/bin/env python3
"""Exercise the sealed production power relay without an App or native services.

Run only on the designated Linux worker. This runner does not build anything,
connect to a Kindle, contact a network service, or use account data. It starts
the selected desktop ELF directly with --relay-power-events, supplies private
native/control pipes, and checks exact output framing and bounded failures.
Only fixed, fictional protocol capabilities are used. Reports omit their text.
"""

import argparse
from contextlib import contextmanager
import datetime
import errno
import fcntl
import hashlib
import json
import os
from pathlib import Path
import platform
import select
import signal
import stat
import struct
import subprocess
import sys
import time


sys.dont_write_bytecode = True

GENERATION = "9"
NONCE = "0123456789abcdef0123456789abcdef"
MAX_FRAME = 256
MAX_STDOUT = 512 * 1024
MAX_STDERR = 64 * 1024
PIPE_SIZE = 4096
NATIVE = (
    b"goingToScreenSaver 2\n",
    b"outOfScreenSaver 1\n",
    b"exitingScreenSaver 1\n",
    b"wakeupFromSuspend 0\n",
    b"usbConfigured 1\n",
)


class HarnessInterrupted(BaseException):
    """Stop dispatching cases while preserving the owned-child cleanup path."""


def interrupt(signum, frame):
    raise HarnessInterrupted(f"Verification interrupted by {signal.Signals(signum).name}")


@contextmanager
def cleanup_signals():
    events = (signal.SIGTERM, signal.SIGHUP, signal.SIGINT)
    previous = {event: signal.getsignal(event) for event in events}
    try:
        for event in events:
            signal.signal(event, signal.SIG_IGN)
        yield
    finally:
        for event, handler in previous.items():
            signal.signal(event, handler)


def require(condition, message):
    if not condition:
        raise AssertionError(message)


def utc_now():
    return datetime.datetime.now(datetime.timezone.utc).isoformat()


def digest(value):
    return hashlib.sha256(value).hexdigest()


def safe_error(error):
    return str(error).replace(NONCE, "<fixture-capability>")[:2000]


def write_json(path, value):
    temporary = path.with_suffix(path.suffix + ".pending")
    with temporary.open("w", encoding="utf-8") as stream:
        json.dump(value, stream, indent=2)
        stream.write("\n")
        stream.flush()
        os.fsync(stream.fileno())
    temporary.replace(path)


def proc_record(pid):
    value = Path(f"/proc/{pid}/stat").read_text()
    closing = value.rfind(")")
    require(closing > 0, "The owned process stat record is malformed")
    fields = value[closing + 2:].split()
    require(len(fields) >= 20, "The owned process stat record is incomplete")
    return {
        "pid": int(value[:value.index(" ")]),
        "state": fields[0],
        "parent_pid": int(fields[1]),
        "process_group": int(fields[2]),
        "session": int(fields[3]),
        "birth_start_ticks": int(fields[19]),
    }


def duplicate_high(descriptor):
    return fcntl.fcntl(descriptor, fcntl.F_DUPFD_CLOEXEC, 64)


def private_pipe():
    original = os.pipe2(os.O_CLOEXEC)
    copies = []
    try:
        for descriptor in original:
            copies.append(duplicate_high(descriptor))
        return tuple(copies)
    except BaseException:
        for descriptor in copies:
            os.close(descriptor)
        raise
    finally:
        for descriptor in original:
            os.close(descriptor)


class SealedBinary:
    def __init__(self, path, expected):
        require(path.is_absolute(), "--binary must be an absolute path")
        require(not path.is_symlink(), "The selected binary must not be a symlink")
        require(len(expected) == 64 and all(char in "0123456789abcdef" for char in expected),
                "The expected binary SHA-256 must contain 64 lowercase hexadecimal characters")
        self.path = path.resolve(strict=True)
        opened = os.open(self.path, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC)
        try:
            self.descriptor = duplicate_high(opened)
        finally:
            os.close(opened)
        self.expected = expected
        try:
            self.identity = os.fstat(self.descriptor)
            require(stat.S_ISREG(self.identity.st_mode) and self.identity.st_mode & 0o111,
                    "The selected binary must be an executable regular file")
            header = os.pread(self.descriptor, 64, 0)
            require(header[:7] == b"\x7fELF\x02\x01\x01" and len(header) >= 20
                    and struct.unpack_from("<H", header, 18)[0] == 62,
                    "Only a Linux x86-64 ELF may execute in this worker check")
            self.assert_current(check_hash=True)
        except BaseException:
            os.close(self.descriptor)
            self.descriptor = None
            raise

    @staticmethod
    def signature(metadata):
        return (metadata.st_dev, metadata.st_ino, metadata.st_size,
                metadata.st_mtime_ns, metadata.st_ctime_ns, metadata.st_mode)

    def assert_current(self, check_hash=False):
        current = os.fstat(self.descriptor)
        named = self.path.stat(follow_symlinks=False)
        require(self.signature(current) == self.signature(self.identity)
                and self.signature(named) == self.signature(self.identity),
                "The sealed binary identity changed")
        if check_hash:
            hasher = hashlib.sha256()
            offset = 0
            while True:
                data = os.pread(self.descriptor, 1024 * 1024, offset)
                if not data:
                    break
                hasher.update(data)
                offset += len(data)
            require(hasher.hexdigest() == self.expected,
                    "The actual ELF does not match the selected binary SHA-256")
            require(self.signature(os.fstat(self.descriptor)) == self.signature(self.identity),
                    "The sealed binary changed while hashing")

    def close(self):
        if self.descriptor is not None:
            os.close(self.descriptor)
            self.descriptor = None


class RelayChild:
    """Own one real ELF process, its pidfd, and four separate pipe OFDs."""

    def __init__(self, binary, work, record, raw=b"", control=b"", filled_output=False,
                 environment=None, source_closed=False):
        self.binary, self.work, self.record = binary, work, record
        self.process = None
        self.pidfd = None
        self.identity = None
        self.descriptors = set()
        self.raw_write = self.control_write = self.output_read = self.error_read = None
        self.output = bytearray()
        self.errors = bytearray()
        self.output_eof = self.error_eof = False
        self.started = time.monotonic()
        self.cleanup_actions = []
        self.peak_threads = 0
        self.exe_observed = False
        self.output_prefix = b"X" * PIPE_SIZE if filled_output else b""
        self.work.mkdir(mode=0o700)
        try:
            raw_read, self.raw_write = self.new_pipe()
            control_read, self.control_write = self.new_pipe()
            self.output_read, output_write = self.new_pipe()
            self.error_read, error_write = self.new_pipe()
            for descriptor in (self.raw_write, self.control_write, self.output_read, self.error_read):
                os.set_blocking(descriptor, False)
            require(len(raw) <= PIPE_SIZE and len(control) <= PIPE_SIZE,
                    "Preloaded fixture input exceeded its pipe capacity")
            for descriptor in (raw_read, control_read, self.output_read):
                fcntl.fcntl(descriptor, fcntl.F_SETPIPE_SZ, PIPE_SIZE)
            if raw:
                require(os.write(self.raw_write, raw) == len(raw), "Could not preload native input")
            if control:
                require(os.write(self.control_write, control) == len(control), "Could not preload control input")
            if filled_output:
                os.set_blocking(output_write, False)
                require(os.write(output_write, self.output_prefix) == len(self.output_prefix),
                        "Could not fill the independent output pipe")
            if source_closed:
                self.close_raw()
            child_environment = {
                "PATH": "/usr/bin:/bin", "LANG": "C", "LC_ALL": "C",
                "HOME": str(work / "forbidden-home"),
                "TMPDIR": str(work / "forbidden-tmp"),
                "XDG_CONFIG_HOME": str(work / "forbidden-config"),
                "XDG_DATA_HOME": str(work / "forbidden-data"),
                "XDG_CACHE_HOME": str(work / "forbidden-cache"),
                "SLINT_BACKEND": "relay-must-not-install-a-platform",
                "BILICOMICS_LIFECYCLE_DIR": str(work / "forbidden-lifecycle"),
                "BILICOMICS_LAUNCH_GENERATION": GENERATION,
                "BILICOMICS_LAUNCH_TOKEN": NONCE,
                "BILICOMICS_POWER_RELAY_GENERATION": GENERATION,
                "BILICOMICS_POWER_RELAY_TOKEN": NONCE,
            }
            for name, value in (environment or {}).items():
                if value is None:
                    child_environment.pop(name, None)
                else:
                    child_environment[name] = value
            binary.assert_current()
            # All owned endpoints and the sealed ELF fd are above 63. Preserve
            # any caller fd 3, alias only the control reader during this spawn,
            # and pass the exact required descriptor through exec without a
            # preexec_fn or a shell wrapper.
            original_three = None
            original_inheritable = False
            try:
                try:
                    original_inheritable = os.get_inheritable(3)
                    original_three = duplicate_high(3)
                except OSError as error:
                    if error.errno != errno.EBADF:
                        raise
                os.dup2(control_read, 3, inheritable=False)
                try:
                    self.process = subprocess.Popen(
                        [str(binary.path), "--relay-power-events"],
                        executable=f"/proc/self/fd/{binary.descriptor}",
                        stdin=raw_read, stdout=output_write, stderr=error_write,
                        cwd=work, env=child_environment, start_new_session=True,
                        close_fds=True, pass_fds=(3, binary.descriptor),
                    )
                finally:
                    if original_three is None:
                        os.close(3)
                    else:
                        os.dup2(original_three, 3, inheritable=original_inheritable)
            finally:
                if original_three is not None:
                    os.close(original_three)
            for descriptor in (raw_read, control_read, output_write, error_write):
                self.close_descriptor(descriptor)
            self.pidfd = os.pidfd_open(self.process.pid, 0)
            self.identity = proc_record(self.process.pid)
            require(self.identity["parent_pid"] == os.getpid()
                    and self.identity["process_group"] == self.process.pid
                    and self.identity["session"] == self.process.pid,
                    "The relay did not enter its owned process/session identity")
            self.observe_process()
            record.update({
                "identity": {key: value for key, value in self.identity.items() if key != "state"},
                "execution": "Pinned SHA-256-checked ELF fd; direct exec; inherited fd 3 control reader",
                "input_preloaded_bytes": len(raw), "control_preloaded_bytes": len(control),
                "output_prefilled_bytes": len(self.output_prefix),
            })
        except BaseException:
            with cleanup_signals():
                self.cleanup()
            raise

    def new_pipe(self):
        pair = private_pipe()
        self.descriptors.update(pair)
        return pair

    def close_descriptor(self, descriptor):
        if descriptor in self.descriptors:
            os.close(descriptor)
            self.descriptors.remove(descriptor)

    def close_raw(self):
        self.close_descriptor(self.raw_write)
        self.raw_write = None

    def close_control(self):
        self.close_descriptor(self.control_write)
        self.control_write = None

    def observe_process(self):
        if self.process is None or self.process.returncode is not None:
            return
        try:
            current = proc_record(self.process.pid)
        except (FileNotFoundError, ProcessLookupError):
            return
        if self.identity is not None:
            require(all(current[key] == self.identity[key] for key in
                        ("pid", "parent_pid", "birth_start_ticks", "process_group", "session")),
                    "The tracked relay process identity changed")
        if current["state"] in ("Z", "X"):
            return
        try:
            actual = os.stat(f"/proc/{self.process.pid}/exe")
            require((actual.st_dev, actual.st_ino) == (self.binary.identity.st_dev, self.binary.identity.st_ino),
                    "The running relay executable is not the selected sealed ELF")
            self.exe_observed = True
            tasks = list(Path(f"/proc/{self.process.pid}/task").iterdir())
            self.peak_threads = max(self.peak_threads, len(tasks))
            require(len(tasks) == 1, "The relay started an App, signal watcher, or background thread")
            children = Path(f"/proc/{self.process.pid}/task/{self.process.pid}/children").read_text().strip()
            require(not children, "The relay started an unowned subprocess")
        except (FileNotFoundError, ProcessLookupError):
            # Invalid-input cases may finish between the stat and exe/task
            # reads. The pinned exec fd remains authoritative for those fast
            # exits; never turn that normal race into a fixture failure.
            if self.process.poll() is not None:
                return
            latest = proc_record(self.process.pid)
            require(latest["state"] in ("Z", "X"),
                    "The running relay lost its executable or task identity")

    def collect(self, capture_stdout=True):
        for descriptor, target, limit, eof_name in (
            (self.output_read if capture_stdout else None, self.output, MAX_STDOUT, "output_eof"),
            (self.error_read, self.errors, MAX_STDERR, "error_eof"),
        ):
            if descriptor is None or descriptor not in self.descriptors:
                continue
            while True:
                try:
                    data = os.read(descriptor, min(65536, limit - len(target) + 1))
                except BlockingIOError:
                    break
                if not data:
                    self.close_descriptor(descriptor)
                    setattr(self, eof_name, True)
                    break
                target.extend(data)
                require(len(target) <= limit, "The actual relay exceeded its bounded capture budget")

    def step(self, delay=0.01, capture_stdout=True):
        self.collect(capture_stdout)
        descriptors = [descriptor for descriptor in
                       (self.output_read if capture_stdout else None, self.error_read)
                       if descriptor is not None and descriptor in self.descriptors]
        if descriptors:
            select.select(descriptors, [], [], delay)
        else:
            time.sleep(delay)
        self.collect(capture_stdout)
        return self.process.poll()

    def wait_duration(self, duration, capture_stdout=True):
        deadline = time.monotonic() + duration
        while time.monotonic() < deadline:
            self.step(min(0.01, max(0, deadline - time.monotonic())), capture_stdout)

    def write(self, descriptor, value, timeout=3):
        require(descriptor is not None and descriptor in self.descriptors,
                "The fixture input endpoint is closed")
        deadline = time.monotonic() + timeout
        offset = 0
        while offset < len(value):
            try:
                count = os.write(descriptor, value[offset:])
                require(count > 0, "A fixture input write made no progress")
                offset += count
            except BlockingIOError:
                require(time.monotonic() < deadline, "The real relay stopped consuming fixture input")
                require(self.step() is None, "The real relay exited while fixture input remained")
            except BrokenPipeError as error:
                raise AssertionError("The real relay rejected fixture input before it was sent") from error
        self.collect()

    def write_raw(self, value):
        self.write(self.raw_write, value)

    def write_control(self, value):
        self.write(self.control_write, value)

    def expect_output(self, expected, timeout=3):
        deadline = time.monotonic() + timeout
        while len(self.output) < len(expected):
            require(time.monotonic() < deadline, "The real relay did not emit the expected complete records")
            code = self.step()
            require(code is None or len(self.output) >= len(expected),
                    "The real relay exited before emitting the expected records")
        self.collect()
        require(bytes(self.output) == expected, "The actual relay output lost, interleaved, or reordered a record")

    def wait_exit(self, timeout=4, capture_stdout=True):
        deadline = time.monotonic() + timeout
        while self.process.poll() is None:
            require(time.monotonic() < deadline, "The real relay did not terminate within the bounded case deadline")
            self.step(capture_stdout=capture_stdout)
        self.collect(capture_stdout)
        return self.process.returncode

    def expect_success(self, expected):
        self.close_raw()
        require(self.wait_exit() == 0, "The real relay did not cleanly finish after native EOF")
        require(bytes(self.output) == expected, "The final relay output differs from the exact complete fixture records")

    def expect_failure(self, output=b"", timeout=4, capture_stdout=True):
        require(self.wait_exit(timeout, capture_stdout) != 0, "The real relay accepted an invalid or stalled stream")
        if not capture_stdout:
            self.collect()
        require(bytes(self.output) == output, "A failed relay emitted an unexpected native or ready record")
        require(b"_bilicomics_" not in bytes(self.output), "A failed relay published a ready barrier")

    def signal_owned(self, event):
        if self.process is None or self.process.poll() is not None:
            return False
        require(self.pidfd is not None and self.identity is not None,
                "The relay has no pinned process identity for signal delivery")
        try:
            current = proc_record(self.process.pid)
        except (FileNotFoundError, ProcessLookupError):
            require(self.process.poll() is not None, "The owned relay identity disappeared before cleanup")
            return False
        require(all(current[key] == self.identity[key] for key in
                    ("pid", "parent_pid", "birth_start_ticks", "process_group", "session")),
                "Refusing to signal a replaced relay process")
        try:
            signal.pidfd_send_signal(self.pidfd, event, None, 0)
        except ProcessLookupError:
            return False
        self.cleanup_actions.append(signal.Signals(event).name)
        return True

    def terminate(self):
        if self.process is None or self.process.poll() is not None:
            return
        self.signal_owned(signal.SIGTERM)
        until = time.monotonic() + 0.35
        while self.process.poll() is None and time.monotonic() < until:
            # Cleanup must not be interrupted by a capture-budget assertion
            # from an already-failing child. Signals do not depend on stdout
            # being consumed; reap first, then inspect the bounded captures.
            time.sleep(0.01)
        if self.process.poll() is None:
            self.signal_owned(signal.SIGKILL)
            self.process.wait(timeout=2)
        require(self.process.poll() is not None, "The owned relay survived TERM/KILL convergence")

    def check_no_app_artifacts(self):
        require(not list(self.work.iterdir()),
                "The relay created App state, a Slint resource, or a lifecycle directory")

    def cleanup(self):
        failure = None
        try:
            if self.process is not None and self.process.poll() is None:
                # If constructor observation failed, Popen and a fresh pidfd
                # still establish ownership before any cleanup signal.
                if self.pidfd is None:
                    self.pidfd = os.pidfd_open(self.process.pid, 0)
                if self.identity is None:
                    self.identity = proc_record(self.process.pid)
                    require(self.identity["parent_pid"] == os.getpid()
                            and self.identity["process_group"] == self.process.pid
                            and self.identity["session"] == self.process.pid,
                            "Could not prove ownership of the partially started relay")
                self.terminate()
            if self.process is not None:
                self.process.wait(timeout=2)
            self.collect()
            self.check_no_app_artifacts()
        except BaseException as error:
            failure = error
        finally:
            for descriptor in tuple(self.descriptors):
                self.close_descriptor(descriptor)
            if self.pidfd is not None:
                os.close(self.pidfd)
                self.pidfd = None
            self.record.update({
                "elapsed_seconds": round(time.monotonic() - self.started, 6),
                "exit_code": self.process.returncode if self.process is not None else None,
                "stdout_bytes": len(self.output), "stdout_sha256": digest(self.output),
                "stderr_bytes": len(self.errors), "stderr_sha256": digest(self.errors),
                "stderr_excerpt": bytes(self.errors[-1600:]).decode("utf-8", errors="replace").replace(NONCE, "<fixture-capability>"),
                "running_executable_observed": self.exe_observed,
                "peak_observed_threads": self.peak_threads,
                "cleanup_signals": self.cleanup_actions,
                "owned_process_reaped": self.process is None or self.process.returncode is not None,
                "owned_pipe_descriptors_closed": not self.descriptors,
                "app_artifacts_created": bool(list(self.work.iterdir())),
            })
        if failure is not None:
            raise failure


def start_notice():
    return f"_bilicomics_start_ready {GENERATION} {NONCE}\n".encode("ascii")


def resume_notice(sequence=1, cycle=1):
    return f"_bilicomics_native_mode_ready {GENERATION} {NONCE} {cycle} {sequence}\n".encode("ascii")


class Suite:
    def __init__(self, binary, output, report):
        self.binary, self.output, self.report = binary, output, report

    def case(self, name, action, **arguments):
        record = {"name": name, "passed": False}
        self.report["cases"].append(record)
        child = None
        try:
            child = RelayChild(self.binary, self.output / "work" / name, record, **arguments)
            action(child, record)
            child.observe_process()
            child.check_no_app_artifacts()
            require(child.process.poll() is not None, "The case left the real relay running")
            record["passed"] = True
        except BaseException as error:
            record["error"] = safe_error(error)
            if isinstance(error, (HarnessInterrupted, KeyboardInterrupt, SystemExit)):
                raise
        finally:
            if child is not None:
                try:
                    with cleanup_signals():
                        child.cleanup()
                except BaseException as error:
                    record["passed"] = False
                    record["cleanup_error"] = safe_error(error)
            write_json(self.output / "report.json", self.report)
            print(json.dumps({"case": name, "passed": record["passed"]}), flush=True)
            require(record.get("owned_process_reaped") is not False,
                    "Verification stopped because an owned relay could not be reaped")


def run_cases(suite):
    def plain(child, record):
        child.expect_success(b"".join(NATIVE))

    suite.case("native_names_and_clean_eof", plain, raw=b"".join(NATIVE))
    suite.case("empty_source_eof", lambda child, _: child.expect_success(b""))

    def bulk(child, record):
        value = b"".join(NATIVE) * 1200
        child.write_raw(value)
        child.expect_success(value)
        record["native_record_count"] = 6000
        record["bulk_input_exceeded_pipe_buf"] = True

    suite.case("bulk_native_records_are_reframed", bulk)

    def bulk_with_barrier(child, record):
        value = b"".join(NATIVE) * 1200 + NATIVE[0] + NATIVE[4]
        child.write_raw(value)
        expected_native = NATIVE[3] * 200 + value
        notice = resume_notice()
        deadline = time.monotonic() + 3
        while len(child.output) < len(expected_native) + len(notice):
            require(time.monotonic() < deadline, "The bulk native stream or its control barrier was lost")
            require(child.step() is None, "The bulk relay exited before its stream was complete")
        observed = bytes(child.output)
        require(observed.count(notice) == 1 and observed.replace(notice, b"", 1) == expected_native,
                "A control barrier was spliced into, duplicated, or reordered the bulk native stream")
        require(notice in observed.splitlines(keepends=True),
                "The ready notice is not one complete standalone output frame")
        child.expect_success(observed)
        record["native_record_count"] = 6202
        record["ready_notice_count"] = 1
        record["bulk_input_exceeded_pipe_buf"] = True

    suite.case("bulk_native_bytes_and_ready_remain_separate_frames", bulk_with_barrier,
               raw=NATIVE[3] * 200, control=resume_notice())

    def split(child, record):
        value = b"".join(NATIVE)
        sizes = (1, 2, 7, 3, 11)
        offset, piece = 0, 0
        while offset < len(value):
            amount = sizes[piece % len(sizes)]
            chunk = value[offset:offset + amount]
            child.write_raw(chunk)
            offset += len(chunk)
            piece += 1
        child.expect_success(value)
        record["fragment_write_count"] = piece

    suite.case("split_native_records_are_reassembled", split)
    prefix = b"goingToScreenSaver "
    maximum = prefix + b"1" * (MAX_FRAME - len(prefix) - 1) + b"\n"
    require(len(maximum) == MAX_FRAME, "The maximum native fixture has the wrong length")
    suite.case("maximum_256_byte_native_frame", lambda child, _: child.expect_success(maximum), raw=maximum)

    def ready(child, record):
        expected = b"".join(NATIVE) + start_notice()
        child.expect_output(expected)
        child.expect_success(expected)

    suite.case("startup_barrier_follows_readable_native_tail", ready,
               raw=b"".join(NATIVE), control=start_notice())

    def split_control(child, record):
        notice = start_notice()
        for byte in notice[:-1]:
            child.write_control(bytes((byte,)))
        child.wait_duration(0.1)
        require(not child.output and child.process.poll() is None,
                "A partial control frame allowed an early ready notice")
        child.write_control(notice[-1:])
        child.expect_output(notice)
        child.expect_success(notice)
        record["control_fragment_write_count"] = len(notice)

    suite.case("split_control_is_not_a_barrier_before_its_lf", split_control)

    def sleep_usb(child, record):
        raw = NATIVE[3] * 100 + NATIVE[0] + NATIVE[4]
        expected = raw + resume_notice()
        child.expect_output(expected)
        child.expect_success(expected)
        record["tail_names_before_ready"] = ["goingToScreenSaver", "usbConfigured"]

    suite.case("resume_barrier_keeps_sleep_and_usb_tail_before_ready", sleep_usb,
               raw=NATIVE[3] * 100 + NATIVE[0] + NATIVE[4], control=resume_notice())

    def partial_then_complete(child, record):
        child.wait_duration(0.15)
        require(child.process.poll() is None and not child.output,
                "A pending partial native frame allowed an early ready barrier")
        child.write_raw(b"Saver 2\nusbConfigured 1\n")
        expected = NATIVE[0] + NATIVE[4] + resume_notice()
        child.expect_output(expected)
        child.expect_success(expected)

    suite.case("barrier_waits_for_a_split_native_tail", partial_then_complete,
               raw=b"goingToScreen", control=resume_notice())

    def replacement(child, record):
        child.wait_duration(0.1)
        require(not child.output, "A partial native frame released the first pending notice")
        child.write_control(resume_notice(2, 2))
        child.wait_duration(0.1)
        child.write_raw(b"Saver 2\n")
        expected = NATIVE[0] + resume_notice(2, 2)
        child.expect_output(expected)
        child.write_control(resume_notice(1, 1) + resume_notice(2, 2))
        child.wait_duration(0.15)
        require(bytes(child.output) == expected, "An old or duplicate capture notice was republished")
        child.expect_success(expected)

    suite.case("new_pending_capture_replaces_old_and_duplicates_stay_silent", replacement,
               raw=b"goingToScreen", control=resume_notice())

    def duplicates(child, record):
        expected = start_notice()
        child.expect_output(expected)
        child.write_control(start_notice())
        child.wait_duration(0.1)
        require(bytes(child.output) == expected, "A duplicate startup notice was republished")
        child.write_control(resume_notice(3, 2))
        expected += resume_notice(3, 2)
        child.expect_output(expected)
        child.write_control(resume_notice(2, 1) + resume_notice(3, 2))
        child.wait_duration(0.1)
        require(bytes(child.output) == expected, "An old or duplicate completed capture was republished")
        child.write_control(resume_notice(4, 3))
        expected += resume_notice(4, 3)
        child.expect_output(expected)
        child.expect_success(expected)

    suite.case("completed_start_and_capture_notices_are_deduplicated", duplicates, control=start_notice())

    invalid_native = {
        "unknown_native_name": b"unexpectedNativeEvent 0\n",
        "native_name_prefix_is_not_a_name": b"goingToScreenSaverExtra 2\n",
        "native_nul_is_rejected": b"goingToScreenSaver 2\x00\n",
        "native_non_ascii_is_rejected": b"goingToScreenSaver \xff\n",
        "native_tab_is_rejected": b"goingToScreenSaver\t2\n",
        "native_carriage_return_is_rejected": b"goingToScreenSaver 2\r\n",
        "native_overlong_frame_is_rejected": prefix + b"1" * (MAX_FRAME - len(prefix)) + b"\n",
        "native_overlong_unterminated_frame_is_rejected": b"w" * (MAX_FRAME + 1),
    }
    for name, value in invalid_native.items():
        suite.case(name, lambda child, _: child.expect_failure(), raw=value)

    def source_partial_eof(child, record):
        child.close_raw()
        child.expect_failure()

    suite.case("partial_native_eof_fails_closed", source_partial_eof, raw=b"goingToScreenSaver 2")

    invalid_control = {
        "unknown_control_command": f"_bilicomics_unknown {GENERATION} {NONCE}\n".encode(),
        "control_wrong_generation": f"_bilicomics_start_ready 10 {NONCE}\n".encode(),
        "control_wrong_capability": f"_bilicomics_start_ready {GENERATION} {'f' * 32}\n".encode(),
        "control_extra_field": start_notice().rstrip(b"\n") + b" extra\n",
        "control_missing_field": f"_bilicomics_native_mode_ready {GENERATION} {NONCE} 1\n".encode(),
        "control_non_numeric_capture": f"_bilicomics_native_mode_ready {GENERATION} {NONCE} 1 nope\n".encode(),
        "control_nul_is_rejected": start_notice().rstrip(b"\n") + b"\x00\n",
        "control_non_ascii_is_rejected": start_notice().rstrip(b"\n") + b"\xff\n",
        "control_overlong_frame_is_rejected": b"_" * MAX_FRAME + b"\n",
    }
    for name, value in invalid_control.items():
        suite.case(name, lambda child, _: child.expect_failure(), control=value)

    def control_eof(child, record):
        child.close_control()
        child.expect_failure()

    suite.case("unexpected_control_eof_fails_closed", control_eof)
    suite.case("partial_control_eof_fails_closed", control_eof, control=start_notice().rstrip(b"\n"))

    def source_eof_with_ready(child, record):
        child.expect_failure()

    suite.case("source_eof_cannot_release_an_already_queued_barrier", source_eof_with_ready,
               control=start_notice(), source_closed=True)

    invalid_environment = (
        ("missing_relay_generation", "BILICOMICS_POWER_RELAY_GENERATION", None),
        ("zero_relay_generation", "BILICOMICS_POWER_RELAY_GENERATION", "0"),
        ("non_numeric_relay_generation", "BILICOMICS_POWER_RELAY_GENERATION", "invalid"),
        ("missing_relay_capability", "BILICOMICS_POWER_RELAY_TOKEN", None),
        ("short_relay_capability", "BILICOMICS_POWER_RELAY_TOKEN", "abc"),
        ("non_hex_relay_capability", "BILICOMICS_POWER_RELAY_TOKEN", "g" * 32),
    )
    for name, key, value in invalid_environment:
        suite.case(name, lambda child, _: child.expect_failure(), environment={key: value})

    def timeout_partial(child, record):
        child.expect_failure(timeout=4)
        elapsed = time.monotonic() - child.started
        require(1.5 <= elapsed <= 4, "The pending partial-frame failure did not honor its two-second deadline")
        record["pending_deadline_elapsed_seconds"] = round(elapsed, 6)
        record["source_and_control_writers_remained_open"] = True

    suite.case("partial_native_tail_has_a_two_second_barrier_deadline", timeout_partial,
               raw=b"goingToScreenSaver 2", control=resume_notice())

    def timeout_replacement(child, record):
        child.wait_duration(0.9)
        require(child.process.poll() is None and not child.output,
                "The first pending barrier did not remain blocked on its native tail")
        replacement_started = time.monotonic()
        child.write_control(resume_notice(2, 2))
        child.expect_failure(timeout=2.3)
        remainder = time.monotonic() - replacement_started
        elapsed = time.monotonic() - child.started
        require(1.5 <= elapsed <= 4 and remainder < 1.7,
                "A replacement capture improperly extended the original pending deadline")
        record["pending_deadline_elapsed_seconds"] = round(elapsed, 6)
        record["elapsed_after_replacement_seconds"] = round(remainder, 6)

    suite.case("newer_capture_does_not_extend_the_pending_deadline", timeout_replacement,
               raw=b"goingToScreenSaver 2", control=resume_notice())

    def timeout_output(child, record):
        child.expect_failure(output=child.output_prefix, timeout=4, capture_stdout=False)
        elapsed = time.monotonic() - child.started
        require(1.5 <= elapsed <= 4, "Full output did not honor the pending two-second deadline")
        record["pending_deadline_elapsed_seconds"] = round(elapsed, 6)
        record["output_was_not_drained_before_process_exit"] = True
        record["child_output_bytes_after_prefill"] = len(child.output) - len(child.output_prefix)

    suite.case("full_output_pipe_fails_without_emitting_ready", timeout_output,
               raw=NATIVE[0] + NATIVE[4], control=resume_notice(), filled_output=True)

    def default_term(child, record):
        child.wait_duration(0.1)
        child.observe_process()
        require(child.signal_owned(signal.SIGTERM), "Could not deliver TERM to the real relay pidfd")
        require(child.wait_exit(timeout=2) == -signal.SIGTERM,
                "The relay installed an App lifecycle signal handler before the relay entry point")
        require(not child.output, "A terminated relay emitted an unexpected frame")
        record["default_term_behavior_preserved"] = True

    suite.case("relay_entry_precedes_signal_bridge_installation", default_term)

    def stopped_cleanup(child, record):
        child.wait_duration(0.1)
        child.observe_process()
        require(child.signal_owned(signal.SIGSTOP), "Could not stop the owned relay for cleanup convergence")
        deadline = time.monotonic() + 1
        while proc_record(child.process.pid)["state"] not in ("T", "t"):
            require(time.monotonic() < deadline, "The owned relay did not enter the controlled stopped state")
            time.sleep(0.01)
        child.terminate()
        require("SIGTERM" in child.cleanup_actions and "SIGKILL" in child.cleanup_actions
                and child.process.returncode == -signal.SIGKILL,
                "The bounded TERM/KILL fallback was not exercised on the original owned child")
        record["forced_cleanup_fallback_exercised"] = True

    suite.case("stopped_owned_relay_has_bounded_term_kill_cleanup", stopped_cleanup)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--expected-binary-sha256", required=True)
    parser.add_argument("--output", type=Path, required=True)
    arguments = parser.parse_args()
    require(sys.platform == "linux" and platform.machine() == "x86_64",
            "Run this verification only on the designated Linux x86-64 worker")
    require(hasattr(os, "pidfd_open") and hasattr(signal, "pidfd_send_signal"),
            "Pinned pidfd signal delivery is required for owned-process cleanup")
    require(arguments.output.is_absolute(), "--output must be an absolute path")
    require(not arguments.output.exists() and not arguments.output.is_symlink(),
            "The verification output must be a new directory")
    os.umask(0o077)
    for event in (signal.SIGTERM, signal.SIGHUP, signal.SIGINT):
        signal.signal(event, interrupt)
    binary = SealedBinary(arguments.binary, arguments.expected_binary_sha256)
    report = {
        "case": "production_power_relay_with_private_pipe_control",
        "started_at": utc_now(), "passed": False, "cases": [],
        "binary": str(binary.path), "binary_sha256": binary.expected,
        "command": [str(binary.path), "--relay-power-events"],
        "coverage": "Real sealed ELF subprocesses; exact native/control framing, ordering, deadlines, and owned cleanup",
        "environment": "Private fixed fictional relay capability; deliberately unusable App/lifecycle/Slint environment",
        "native_service_used": False, "network_used": False, "device_used": False,
        "live_account_used": False, "build_performed": False,
        "limits": {"frame_bytes_including_lf": MAX_FRAME, "pipe_capacity_bytes": PIPE_SIZE,
                   "case_stdout_capture_bytes": MAX_STDOUT, "case_stderr_capture_bytes": MAX_STDERR,
                   "pending_barrier_deadline_seconds": 2},
        "limitations": ["No Kindle, firmware-emulator, native service, or live-account acceptance is established"],
    }
    output_created = False
    try:
        arguments.output.mkdir(mode=0o700, parents=True, exist_ok=False)
        output_created = True
        (arguments.output / "work").mkdir(mode=0o700)
        run_cases(Suite(binary, arguments.output, report))
        binary.assert_current(check_hash=True)
        report["sealed_binary_still_matches"] = True
        report["passed"] = bool(report["cases"]) and all(case["passed"] for case in report["cases"])
    except BaseException as error:
        report["error"] = safe_error(error)
    finally:
        binary.close()
        report["finished_at"] = utc_now()
        report["passed_case_count"] = sum(case["passed"] for case in report["cases"])
        report["total_case_count"] = len(report["cases"])
        if output_created:
            write_json(arguments.output / "report.json", report)
            summary = [f"{'PASS' if case['passed'] else 'FAIL'} {case['name']}" for case in report["cases"]]
            summary.append(f"Passed {report['passed_case_count']}/{report['total_case_count']} cases")
            (arguments.output / "case-summary.txt").write_text("\n".join(summary) + "\n", encoding="utf-8")
    print(json.dumps({"passed": report["passed"], "cases": report["total_case_count"],
                      "passed_cases": report["passed_case_count"], "report": str(arguments.output / "report.json")}), flush=True)
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (AssertionError, OSError) as error:
        print(json.dumps({"passed": False, "error": safe_error(error)}), file=sys.stderr)
        raise SystemExit(1)
