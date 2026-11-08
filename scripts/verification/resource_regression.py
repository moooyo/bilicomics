#!/usr/bin/env python3
"""Measure bounded real offline guest reader workloads on the designated Linux worker.

This runner never builds an application. It needs the final desktop binary and
its matching source manifest. Only the evidence/ subdirectory is safe to export;
private/ contains the disposable guest database and original page fixtures.
"""

import argparse
from contextlib import contextmanager
import datetime
import json
import os
from pathlib import Path
import platform
import re
import select
import shutil
import signal
import statistics
import struct
import subprocess
import sys
import time
import traceback

sys.dont_write_bytecode = True
from resource_fixture import (complete_page_proof, file_identity, load_guest_fixture,
                              require, seed_case, select_cover_asset, sha256)


MIB = 1024 * 1024
SOURCE_TREES = ("assets", "src", "ui", "vendor", "tests")
ROOT_INPUTS = ("Cargo.toml", "Cargo.lock", "build.rs", ".cargo/config.toml")
CACHE_FIELDS = ("source_cache_bytes", "source_cache_budget", "source_cache_entries")
IMAGE_FIELDS = ("page_size", "second_page_size", "overview_image_size")
EXPORTED_SUFFIXES = {".json", ".jsonl", ".sha256", ".py", ".txt", ".log"}


class ObservationGap(AssertionError):
    """The selected binary did not expose a required production fact."""


def utc_now():
    return datetime.datetime.now(datetime.timezone.utc).isoformat()


def write_json(path, value, exclusive=False):
    with path.open("x" if exclusive else "w", encoding="utf-8") as stream:
        json.dump(value, stream, indent=2)
        stream.write("\n")


def atomic_json(path, value):
    temporary = path.with_name(path.name + ".new")
    with temporary.open("w", encoding="utf-8") as stream:
        json.dump(value, stream)
        stream.write("\n")
    temporary.replace(path)


def append_json(path, value):
    with path.open("a", encoding="utf-8") as stream:
        stream.write(json.dumps(value) + "\n")


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


def source_manifest(app):
    files = [app / name for name in ROOT_INPUTS if (app / name).is_file()]
    for tree in SOURCE_TREES:
        for path in (app / tree).rglob("*"):
            relative = path.relative_to(app)
            if any(part in ("target", ".git", "__pycache__") for part in relative.parts):
                continue
            if path.is_file() and not path.is_symlink() and path.suffix not in (".pyc", ".log", ".tmp"):
                files.append(path)
    return "".join(f"{sha256(path)}  {path.relative_to(app).as_posix()}\n"
                   for path in sorted(set(files), key=lambda path: path.relative_to(app).as_posix()))


def proc_record(pid):
    text = Path(f"/proc/{pid}/stat").read_text()
    closing = text.rfind(")")
    require(closing > 0, "The owned process stat record is malformed")
    fields = text[closing + 2:].split()
    status_text = Path(f"/proc/{pid}/status").read_text()
    status = {}
    for line in status_text.splitlines():
        if ":" in line:
            key, value = line.split(":", 1)
            status[key] = value.strip()
    integer_kib = lambda name: int(status.get(name, "0 kB").split()[0]) * 1024
    return {
        "pid": int(text[:text.index(" ")]), "state": fields[0],
        "parent_pid": int(fields[1]), "process_group": int(fields[2]), "session": int(fields[3]),
        "user_cpu_ticks": int(fields[11]), "system_cpu_ticks": int(fields[12]),
        "threads": int(fields[17]), "birth_start_ticks": int(fields[19]),
        "rss_bytes": integer_kib("VmRSS"), "rss_high_water_bytes": integer_kib("VmHWM"),
        "fd_count": len(list(Path(f"/proc/{pid}/fd").iterdir())),
        "raw_stat": text.rstrip(),
        "raw_status": {key: status.get(key) for key in ("VmRSS", "VmHWM", "Threads")},
    }


def same_birth(identity):
    try:
        current = proc_record(identity["pid"])
        return (current["birth_start_ticks"] == identity["birth_start_ticks"]
                and current["process_group"] == identity["process_group"]
                and current["session"] == identity["session"])
    except (FileNotFoundError, ProcessLookupError, PermissionError):
        return False


def owned_signal(identity, signum):
    require(identity["process_group"] == identity["pid"] and identity["session"] == identity["pid"],
            "Refusing to signal a process that did not start its own session")
    descriptor = None
    try:
        # A pidfd pins the process before the birth check, so PID reuse between
        # that check and delivery can never redirect this signal to another PID.
        descriptor = os.pidfd_open(identity["pid"], 0)
        if not same_birth(identity):
            return {"sent": False, "reason": "The original PID/birth/session is no longer present"}
        signal.pidfd_send_signal(descriptor, signum, None, 0)
        return {"sent": True, "signal": signal.Signals(signum).name}
    except ProcessLookupError:
        return {"sent": False, "reason": "The original process exited before the signal"}
    finally:
        if descriptor is not None:
            os.close(descriptor)


class OwnedProcess:
    def __init__(self, role, command, log_path, environment=None, pass_fds=()):
        self.role, self.command, self.log_path = role, command, log_path
        self.log = log_path.open("x")
        try:
            self.process = subprocess.Popen(command, stdout=self.log, stderr=subprocess.STDOUT,
                                            stdin=subprocess.DEVNULL, env=environment,
                                            start_new_session=True, pass_fds=pass_fds)
        except BaseException:
            self.log.close()
            raise
        try:
            self.identity = proc_record(self.process.pid)
        except BaseException:
            # The Popen child is still owned even if its first /proc read fails.
            # Open a pidfd before inspecting/reaping it to avoid a stale PID kill.
            descriptor = None
            try:
                descriptor = os.pidfd_open(self.process.pid, 0)
                signal.pidfd_send_signal(descriptor, signal.SIGKILL, None, 0)
            except ProcessLookupError:
                pass
            finally:
                if descriptor is not None:
                    os.close(descriptor)
                self.process.wait(timeout=2)
                self.log.close()
            raise
        self.identity = {key: self.identity[key] for key in
                         ("pid", "birth_start_ticks", "process_group", "session")}
        require(self.identity["pid"] == self.identity["process_group"] == self.identity["session"],
                "A resource workload did not receive a private process session")

    def stop(self):
        result = {"role": self.role, "identity": self.identity, "signals": []}
        if self.process.poll() is None:
            result["signals"].append(owned_signal(self.identity, signal.SIGTERM))
            try:
                self.process.wait(timeout=3)
            except subprocess.TimeoutExpired:
                result["signals"].append(owned_signal(self.identity, signal.SIGKILL))
                try:
                    self.process.wait(timeout=2)
                except subprocess.TimeoutExpired:
                    result["reap_timed_out"] = True
        result["exit_code"] = self.process.poll()
        result["original_process_still_present"] = same_birth(self.identity)
        self.log.close()
        return result


class Guard:
    def __init__(self, private, evidence, deadline, rss_limit):
        self.path = private / "watchdog-control.json"
        self.events = evidence / "watchdog.jsonl"
        own = proc_record(os.getpid())
        self.config = {"supervisor": {key: own[key] for key in
                                     ("pid", "birth_start_ticks", "process_group", "session")},
                       "deadline_monotonic_ns": int(deadline * 1_000_000_000),
                       "rss_limit_bytes": rss_limit, "targets": [], "stop": False,
                       "events": str(self.events)}
        self.targets = []
        self.safety_failure = None
        atomic_json(self.path, self.config)
        self.worker = OwnedProcess("watchdog", [sys.executable, str(Path(__file__).resolve()),
                                                "--internal-watchdog", str(self.path)],
                                   evidence / "watchdog.log")

    def add(self, process):
        self.targets.append(process)
        self.config["targets"] = [{"role": item.role, "identity": item.identity} for item in self.targets]
        atomic_json(self.path, self.config)

    def remove(self, process):
        self.targets.remove(process)
        self.config["targets"] = [{"role": item.role, "identity": item.identity} for item in self.targets]
        atomic_json(self.path, self.config)

    def check(self):
        self.read_safety_events()
        require(self.safety_failure is None, "The independent resource guard stopped the workload: " + str(self.safety_failure))
        require(self.worker.process.poll() is None, "The independent resource watchdog exited unexpectedly")

    def read_safety_events(self):
        if self.events.exists():
            for line in self.events.read_text().splitlines(keepends=True):
                if not line.endswith("\n"):
                    continue
                if json.loads(line).get("event") == "safety-stop":
                    self.safety_failure = json.loads(line).get("reason", "safety-stop")

    def stop(self):
        if time.monotonic_ns() >= self.config["deadline_monotonic_ns"]:
            self.safety_failure = self.safety_failure or "absolute-deadline-before-guard-stop"
        if self.targets:
            # Do not retire the independent guard while an owned workload is
            # still registered after a failed cleanup path.
            self.config["deadline_monotonic_ns"] = time.monotonic_ns() - 1
            atomic_json(self.path, self.config)
            try:
                self.worker.process.wait(timeout=2)
            except subprocess.TimeoutExpired:
                pass
            for target in list(reversed(self.targets)):
                target.stop()
                if not same_birth(target.identity):
                    self.remove(target)
        self.config["stop"] = True
        atomic_json(self.path, self.config)
        try:
            self.worker.process.wait(timeout=2)
        except subprocess.TimeoutExpired:
            pass
        result = self.worker.stop()
        self.read_safety_events()
        result["safety_failure"] = self.safety_failure
        result["registered_workloads_remaining"] = [item.identity for item in self.targets
                                                    if same_birth(item.identity)]
        return result


def watchdog_main(path):
    last_heartbeat, peak = 0.0, 0
    while True:
        config = json.loads(path.read_text())
        if config.get("stop"):
            return 0
        now = time.monotonic()
        reason = None
        if now * 1_000_000_000 >= config["deadline_monotonic_ns"]:
            reason = "absolute-deadline"
        elif not same_birth(config["supervisor"]):
            reason = "supervisor-birth-ended"
        total, processes = 0, []
        for target in config["targets"]:
            identity = target["identity"]
            if not same_birth(identity):
                continue
            try:
                current = proc_record(identity["pid"])
                total += current["rss_bytes"]
                processes.append({"role": target["role"], "pid": identity["pid"],
                                  "rss_bytes": current["rss_bytes"],
                                  "rss_high_water_bytes": current["rss_high_water_bytes"]})
                if current["rss_high_water_bytes"] > config["rss_limit_bytes"]:
                    reason = "owned-process-observed-rss-high-water-limit"
            except (FileNotFoundError, ProcessLookupError):
                pass
        peak = max(peak, total)
        if total > config["rss_limit_bytes"]:
            reason = "combined-owned-workload-rss-limit"
        if reason:
            result = {"event": "safety-stop", "reason": reason, "monotonic_ns": time.monotonic_ns(),
                      "rss_bytes": total, "peak_combined_rss_bytes": peak, "processes": processes,
                      "signals": []}
            for target in reversed(config["targets"]):
                result["signals"].append({"role": target["role"],
                                          **owned_signal(target["identity"], signal.SIGTERM)})
            append_json(Path(config["events"]), result)
            time.sleep(0.5)
            for target in reversed(config["targets"]):
                owned_signal(target["identity"], signal.SIGKILL)
            return 1
        if now - last_heartbeat >= 1:
            append_json(Path(config["events"]), {"event": "heartbeat", "monotonic_ns": time.monotonic_ns(),
                                                "combined_rss_bytes": total, "peak_combined_rss_bytes": peak,
                                                "processes": processes})
            last_heartbeat = now
        time.sleep(0.05)


def remaining(deadline, maximum=None):
    seconds = deadline - time.monotonic()
    require(seconds > 0, "The absolute resource-regression deadline expired")
    return seconds if maximum is None else min(seconds, maximum)


def start_xvfb(evidence, guard, deadline):
    executable = shutil.which("Xvfb")
    require(executable, "Xvfb is required for the private real application display")
    read_fd, write_fd = os.pipe()
    os.set_blocking(read_fd, False)
    process = None
    try:
        process = OwnedProcess("xvfb", [executable, "-displayfd", str(write_fd), "-screen", "0",
                                        "1600x2200x24", "-nolisten", "tcp", "-noreset"],
                               evidence / "xvfb.log", pass_fds=(write_fd,))
        guard.add(process)
        os.close(write_fd)
        write_fd = None
        value = b""
        while b"\n" not in value:
            guard.check()
            require(process.process.poll() is None, "The private Xvfb exited before publishing its display")
            selected, _, _ = select.select([read_fd], [], [], remaining(deadline, 0.1))
            if selected:
                chunk = os.read(read_fd, 64)
                require(chunk, "Xvfb closed its display descriptor without a number")
                value += chunk
                require(len(value) < 64, "Xvfb returned an invalid display number")
        number = value.split(b"\n", 1)[0].decode("ascii")
        require(number.isdigit(), "Xvfb did not return a local numeric display")
        return process, ":" + number
    except BaseException:
        if process is not None:
            process.stop()
            if process in guard.targets:
                guard.remove(process)
        raise
    finally:
        os.close(read_fd)
        if write_fd is not None:
            os.close(write_fd)


def create_actions(args):
    actions = [{"at_ms": args.catalog_warmup_ms, "section": "catalog", "action": "comic.read", "value": ""}]
    first_close = args.catalog_warmup_ms + args.initial_ready_ms + args.settle_ms + round(args.idle_seconds * 1000)
    cycle_length = args.cycle_closed_ms + args.cycle_ready_ms
    pairs = []
    for index in range(args.cycles):
        close_at = first_close + index * cycle_length
        open_at = close_at + args.cycle_closed_ms
        actions.extend([{"at_ms": close_at, "section": "reader", "action": "shelf", "value": ""},
                        {"at_ms": open_at, "section": "catalog", "action": "comic.read", "value": ""}])
        pairs.append({"cycle": index + 1, "close_at_ms": close_at, "reopen_at_ms": open_at})
    exit_at = pairs[-1]["reopen_at_ms"] + args.initial_ready_ms + args.settle_ms + round(args.idle_seconds * 1000) + 1000
    actions.append({"at_ms": exit_at, "section": "shell", "action": "exit", "value": ""})
    return actions, {"pairs": pairs, "explicit_exit_at_ms": exit_at,
                     "clock_origin": "The real application ready event; actions are loaded once at startup"}


def cache_state(state):
    if not isinstance(state, dict) or any(type(state.get(name)) is not int for name in CACHE_FIELDS):
        raise ObservationGap("The production snapshot lacks integer source-cache bytes/budget/entries")
    usage = {name: state[name] for name in CACHE_FIELDS}
    require(usage["source_cache_budget"] > 0, "The selected workload has no positive source-cache budget")
    require(0 <= usage["source_cache_bytes"] <= usage["source_cache_budget"], "The production source cache exceeded its byte budget")
    require(usage["source_cache_entries"] >= 0, "The source-cache entry count is negative")
    require((usage["source_cache_entries"] == 0) == (usage["source_cache_bytes"] == 0),
            "The source-cache byte and entry counters disagree")
    return usage


def image_size(value):
    if isinstance(value, dict):
        value = [value.get("width"), value.get("height")]
    if not isinstance(value, list) or len(value) != 2 or any(type(item) is not int for item in value):
        raise ObservationGap("The production ReaderModel image size is missing or unreadable")
    return value


class TraceObserver:
    def __init__(self, path, observations, process, guest, seed):
        self.path, self.observations, self.process = path, observations, process
        self.guest, self.seed = guest, seed
        self.offset, self.pending, self.index = 0, b"", 0
        self.ready_events, self.close_events = [], []
        self.startup_ready = None
        self.active_generation = None
        self.active_ready_at = None
        self.last_resources, self.last_resources_at = None, None

    def read(self, final=False):
        if not self.path.exists():
            return
        with self.path.open("rb") as stream:
            stream.seek(self.offset)
            chunk = stream.read(512 * 1024)
            self.offset += len(chunk)
        self.pending += chunk
        require(len(self.pending) <= 1024 * 1024, "A lifecycle record exceeded the bounded trace parser")
        while b"\n" in self.pending:
            line, self.pending = self.pending.split(b"\n", 1)
            if not line.strip():
                continue
            record = json.loads(line)
            self.index += 1
            require(record.get("process_id") == self.process.process.pid, "The trace contains another process identity")
            live = not final and same_birth(self.process.identity)
            if not final and not live:
                require(self.process.process.poll() is not None, "The lifecycle observation lost its owned PID birth")
            observed = {"trace_index": self.index, "observed_monotonic_ns": time.monotonic_ns(),
                        "process_identity": self.process.identity, "record": record,
                        "observed_while_owned_process_alive": live}
            append_json(self.observations, observed)
            if not live:
                # Final file drainage is preserved, but never substitutes for
                # any of the required live ready/close facts.
                continue
            if record.get("event") == "ready":
                require(self.startup_ready is None, "The same process emitted more than one startup ready event")
                self.startup_ready = observed
            elif record.get("event") == "reader-ready":
                self.reader_ready(observed)
            elif record.get("event") == "reader-closed":
                self.reader_closed(observed)

    def reader_ready(self, observed):
        record, state = observed["record"], observed["record"].get("reader_state")
        if not isinstance(state, dict):
            raise ObservationGap("A reader-ready record has no real reader snapshot")
        required = ("reader_generation", "rendering", "transition", "page_id", "page_index", "frame_width", "frame_height")
        if any(name not in state for name in required):
            raise ObservationGap("The reader snapshot omits a required completed-page observable")
        require(record.get("route") == "reader" and record.get("reader_loading") is False,
                "The reader-ready record is loading or outside the real reader")
        frame = record.get("frame_size")
        require(isinstance(frame, list) and len(frame) == 2 and all(type(value) is int and value > 0 for value in frame),
                "The global ReaderModel has no actual primary page")
        require(state["rendering"] is False and state["transition"] is False,
                "The accepted resource window is still rendering or transitioning")
        require(state["frame_width"] > 0 and state["frame_height"] > 0,
                "The reader state has no completed prepared frame")
        require(state.get("comic_id") == self.guest.COMIC and state.get("episode_id") == self.guest.EPISODE
                and state.get("revision") == self.guest.REVISION and state["page_index"] == 0
                and state["page_id"] == self.seed["origin_sha256"],
                "The completed page does not match the valid isolated guest fixture")
        generation = state["reader_generation"]
        require(type(generation) is int and generation >= 0, "The reader generation is invalid")
        usage = cache_state(state)
        self.last_resources, self.last_resources_at = usage, observed["observed_monotonic_ns"]
        if self.active_generation == generation:
            return
        require(self.active_generation is None, "A reader was replaced without an observed explicit close")
        require(not any(item["record"]["reader_state"]["reader_generation"] == generation for item in self.ready_events),
                "A previously closed reader generation was reused")
        self.active_generation = generation
        self.active_ready_at = observed["observed_monotonic_ns"]
        self.ready_events.append(observed)

    def reader_closed(self, observed):
        record = observed["record"]
        require(self.active_generation is not None, "A close occurred before a completed reader-ready page")
        require(record.get("reader_generation") == self.active_generation,
                "The close event does not identify the last completed reader generation")
        resources = record.get("reader_resources")
        if not isinstance(resources, dict):
            raise ObservationGap("The explicit close lacks the production reader_resources snapshot")
        needed = ("reader_active", "reader_generation", *IMAGE_FIELDS, "thumbnails_count", "chapters_count")
        if ("reader_generation" not in resources
                or any(name not in resources or resources[name] is None for name in needed if name != "reader_generation")):
            raise ObservationGap("The close event cannot read the actual global ReaderModel resources")
        require(record.get("route") != "reader" and resources["reader_active"] is False
                and resources.get("reader_generation") is None,
                "The explicitly closed reader remains active")
        for name in IMAGE_FIELDS:
            require(image_size(resources[name]) == [0, 0], "The global Slint image was retained after close: " + name)
        require(resources["thumbnails_count"] == 0 and resources["chapters_count"] == 0,
                "The global Slint reader models were retained after close")
        usage = cache_state(resources)
        self.last_resources, self.last_resources_at = usage, observed["observed_monotonic_ns"]
        self.close_events.append(observed)
        self.active_generation = None
        self.active_ready_at = None


def wait_for_real_executable(process, binary, guard, deadline):
    expected = binary.stat()
    while True:
        guard.check()
        remaining(deadline)
        require(process.process.poll() is None, "The application launcher exited before exec")
        require(same_birth(process.identity), "The application PID birth changed before attachment")
        try:
            actual = Path(f"/proc/{process.process.pid}/exe").stat()
            if (actual.st_dev, actual.st_ino) == (expected.st_dev, expected.st_ino):
                return {"path": os.readlink(f"/proc/{process.process.pid}/exe"),
                        "device": actual.st_dev, "inode": actual.st_ino}
        except (FileNotFoundError, ProcessLookupError):
            pass
        time.sleep(min(0.02, remaining(deadline)))


def numeric_summary(values):
    return {"first": values[0], "last": values[-1], "minimum": min(values), "maximum": max(values),
            "median": statistics.median(values), "mean": statistics.fmean(values)}


def summarize_window(samples, start_ns, end_ns, clock_ticks, sample_interval_ms):
    selected = [value for value in samples if start_ns <= value["monotonic_ns"] <= end_ns]
    require(len(selected) >= 3, "A stable resource window has fewer than three raw samples")
    require(all(value["phase"] == "reader-ready" and value["reader_generation"] == selected[0]["reader_generation"]
                for value in selected), "A stable window contains a loading or closed reader sample")
    duration = (selected[-1]["monotonic_ns"] - selected[0]["monotonic_ns"]) / 1e9
    require(duration > 0, "The resource window has no elapsed sample interval")
    tolerance_ns = max(2 * sample_interval_ms * 1_000_000, 150_000_000)
    gaps = [after["monotonic_ns"] - before["monotonic_ns"] for before, after in zip(selected, selected[1:])]
    maximum_gap_ns = max(gaps)
    if (selected[0]["monotonic_ns"] - start_ns > tolerance_ns
            or end_ns - selected[-1]["monotonic_ns"] > tolerance_ns
            or selected[-1]["monotonic_ns"] - selected[0]["monotonic_ns"] < end_ns - start_ns - 2 * tolerance_ns
            or maximum_gap_ns > max(5 * sample_interval_ms * 1_000_000, 500_000_000)):
        raise ObservationGap("Raw samples do not cover the complete declared stable window at the bounded sampling cadence")
    cpu_ticks = lambda value: value["user_cpu_ticks"] + value["system_cpu_ticks"]
    return {
        "requested_start_monotonic_ns": start_ns, "requested_end_monotonic_ns": end_ns,
        "sample_count": len(selected), "sampled_seconds": duration,
        "requested_seconds": (end_ns - start_ns) / 1e9,
        "maximum_sample_gap_seconds": maximum_gap_ns / 1e9,
        "boundary_tolerance_seconds": tolerance_ns / 1e9,
        "cpu_percent_of_one_core": 100 * (cpu_ticks(selected[-1]) - cpu_ticks(selected[0])) / clock_ticks / duration,
        "rss_bytes": numeric_summary([value["rss_bytes"] for value in selected]),
        "fd_count": numeric_summary([value["fd_count"] for value in selected]),
        "threads": numeric_summary([value["threads"] for value in selected]),
        "generation": selected[0]["reader_generation"],
    }


def analyze_case(args, observer, samples, exit_ns):
    require(observer.startup_ready is not None, "The real application never became ready")
    if len(observer.ready_events) != args.cycles + 1 or len(observer.close_events) != args.cycles:
        raise ObservationGap(f"Need {args.cycles + 1} completed reader generations and {args.cycles} explicit closes; "
                             f"observed {len(observer.ready_events)} ready and {len(observer.close_events)} close")
    cycles = []
    for index, closed in enumerate(observer.close_events):
        before, after = observer.ready_events[index:index + 2]
        require(before["trace_index"] < closed["trace_index"] < after["trace_index"],
                "A cycle did not complete ready, explicit close, and real reopen in order")
        ready_duration = (closed["observed_monotonic_ns"] - before["observed_monotonic_ns"]) / 1e9
        require(ready_duration >= 1, "A reader was cancelled before a meaningful completed-ready interval")
        cycles.append({"cycle": index + 1, "ready_trace_index": before["trace_index"],
                       "closed_trace_index": closed["trace_index"], "reopened_trace_index": after["trace_index"],
                       "ready_generation": before["record"]["reader_state"]["reader_generation"],
                       "reopened_generation": after["record"]["reader_state"]["reader_generation"],
                       "completed_ready_seconds_before_close": ready_duration,
                       "closed_global_slint_resources": closed["record"]["reader_resources"]})
    first_ready, final_ready = observer.ready_events[0], observer.ready_events[-1]
    duration_ns = round(args.idle_seconds * 1e9)
    initial_start = first_ready["observed_monotonic_ns"] + args.settle_ms * 1_000_000
    final_start = final_ready["observed_monotonic_ns"] + args.settle_ms * 1_000_000
    require(initial_start + duration_ns < observer.close_events[0]["observed_monotonic_ns"],
            "The initial reader did not provide the requested full stable idle window")
    require(final_start + duration_ns < exit_ns, "The final reader did not provide the requested full stable idle window")
    clock_ticks = os.sysconf("SC_CLK_TCK")
    initial = summarize_window(samples, initial_start, initial_start + duration_ns, clock_ticks, args.sample_interval_ms)
    final = summarize_window(samples, final_start, final_start + duration_ns, clock_ticks, args.sample_interval_ms)
    require(initial["generation"] == first_ready["record"]["reader_state"]["reader_generation"]
            and final["generation"] == final_ready["record"]["reader_state"]["reader_generation"],
            "A stable window is not attached to its completed reader generation")
    growth = {"rss_median_bytes": final["rss_bytes"]["median"] - initial["rss_bytes"]["median"],
              "fd_maximum": final["fd_count"]["maximum"] - initial["fd_count"]["maximum"],
              "threads_maximum": final["threads"]["maximum"] - initial["threads"]["maximum"]}
    require(growth["rss_median_bytes"] <= args.max_ready_rss_growth_mib * MIB,
            "The declared warm-reader RSS growth tolerance was exceeded")
    require(growth["fd_maximum"] <= args.max_fd_growth, "The declared stable FD growth tolerance was exceeded")
    require(growth["threads_maximum"] <= args.max_thread_growth, "The declared stable thread growth tolerance was exceeded")
    phases = {}
    for phase in sorted({value["phase"] for value in samples}):
        selected = [value for value in samples if value["phase"] == phase]
        paired = [(before, after) for before, after in zip(samples, samples[1:])
                  if before["phase"] == after["phase"] == phase]
        wall_seconds = sum((after["monotonic_ns"] - before["monotonic_ns"]) / 1e9 for before, after in paired)
        cpu_seconds = sum((after["user_cpu_ticks"] + after["system_cpu_ticks"]
                           - before["user_cpu_ticks"] - before["system_cpu_ticks"]) / clock_ticks
                          for before, after in paired)
        phases[phase] = {"sample_count": len(selected), "paired_sample_seconds": wall_seconds,
                         "cpu_percent_of_one_core": 100 * cpu_seconds / wall_seconds if wall_seconds else None,
                         "peak_rss_bytes": max(value["rss_bytes"] for value in selected),
                         "peak_fd_count": max(value["fd_count"] for value in selected),
                         "peak_threads": max(value["threads"] for value in selected)}
    resource_samples = [value["last_app_resource_snapshot"] for value in samples
                        if value["last_app_resource_snapshot"] is not None]
    require(resource_samples, "No actual App source-cache usage snapshot was sampled")
    return {"completed_cycles": len(cycles), "ready_generation_count": len(observer.ready_events), "cycles": cycles,
            "initial_stable_idle": initial, "post_cycles_stable_idle": final, "observed_growth": growth,
            "separate_phase_observations": phases,
            "observed_source_cache": {
                "peak_retained_bytes": max(value["source_cache_bytes"] for value in resource_samples),
                "peak_entries": max(value["source_cache_entries"] for value in resource_samples),
                "reported_budgets": sorted({value["source_cache_budget"] for value in resource_samples}),
                "scope": "Latest actual App resource snapshot at each raw sample; not an allocator or worker-Arc total"},
            "observed_all_phase_peak": {"rss_bytes": max(value["rss_bytes"] for value in samples),
                                        "rss_high_water_bytes": max(value["rss_high_water_bytes"] for value in samples),
                                        "fd_count": max(value["fd_count"] for value in samples),
                                        "threads": max(value["threads"] for value in samples)},
            "peak_scope": "All raw phases, including startup, cold page work, closing and reopening; not an idle statistic"}


def run_case(args, guest, cover_asset, name, cached, private_root, evidence_root, guard, deadline):
    private, evidence = private_root / name, evidence_root / name
    private.mkdir(mode=0o700)
    evidence.mkdir(mode=0o700)
    data, database, seed, proof = seed_case(guest, private, args.assets_root, args.origin_page, cover_asset, cached)
    write_json(evidence / "fixture.json", proof, exclusive=True)
    actions, plan = create_actions(args)
    for action in actions:
        if action["action"] == "comic.read":
            action["value"] = guest.COMIC
    actions_path = evidence / "actions.json"
    write_json(actions_path, actions, exclusive=True)
    write_json(evidence / "action-plan.json", plan, exclusive=True)
    xvfb, process, observer = None, None, None
    samples, cleanup = [], []
    result = {"name": name, "passed": False, "status": "running", "fixture": proof,
              "observability_gap_is_failure": True, "named_intents_used_for": "Resource workload setup and explicit reader close/reopen",
              "visual_acceptance_verified": False, "cover_runtime_publication_observed": False,
              "cover_comparison_scope": "Same fixture URL and comic; only the managed local cover file presence differs"}
    try:
        xvfb, display = start_xvfb(evidence, guard, deadline)
        environment = os.environ.copy()
        for key in ("BILICOMICS_LIFECYCLE_DIR", "BILICOMICS_LAUNCH_TOKEN", "BILICOMICS_LAUNCH_GENERATION",
                    "WAYLAND_DISPLAY", "WAYLAND_SOCKET", "XAUTHORITY"):
            environment.pop(key, None)
        environment.update({"DISPLAY": display, "SLINT_BACKEND": "winit-software", "WINIT_UNIX_BACKEND": "x11",
                            "XDG_SESSION_TYPE": "x11", "CARGO_BUILD_JOBS": os.environ.get("CARGO_BUILD_JOBS") or "12",
                            "RUST_TEST_THREADS": "1"})
        trace = evidence / "lifecycle.jsonl"
        command = ["unshare", "-n", "--", str(args.binary), "--offline", "--device", args.profile,
                   "--screen", "shelf", "--data-dir", str(data), "--actions", str(actions_path),
                   "--lifecycle-trace", str(trace), "--exit-after-ms", str(plan["explicit_exit_at_ms"] + 5000)]
        process = OwnedProcess("application", command, evidence / "application.log", environment=environment)
        guard.add(process)
        result.update({"command": command, "process_identity": process.identity,
                       "xvfb_identity": xvfb.identity, "display": display,
                       "actual_executable": wait_for_real_executable(process, args.binary, guard, deadline)})
        namespace = os.readlink(f"/proc/{process.process.pid}/ns/net")
        parent_namespace = os.readlink("/proc/self/ns/net")
        require(namespace != parent_namespace, "The actual App did not enter its isolated network namespace")
        result["network_isolation"] = {"application_namespace": namespace, "harness_namespace": parent_namespace,
                                       "offline_requested": True, "network_namespace_separate": True}
        observer = TraceObserver(trace, evidence / "observations.jsonl", process, guest, seed)
        interval = args.sample_interval_ms / 1000
        next_sample = time.monotonic()
        last_sample = None
        page_proof = None
        while process.process.poll() is None:
            remaining(deadline)
            guard.check()
            if not same_birth(process.identity):
                if process.process.poll() is not None:
                    break
                raise AssertionError("The sampled application PID birth changed")
            observer.read()
            if process.process.poll() is not None:
                break
            try:
                current = proc_record(process.process.pid)
            except (FileNotFoundError, ProcessLookupError):
                if process.process.poll() is not None:
                    break
                raise
            current["monotonic_ns"] = time.monotonic_ns()
            current["reader_generation"] = observer.active_generation
            current["phase"] = ("startup" if observer.startup_ready is None else
                                "not-ready" if observer.active_generation is None else
                                "reader-warming" if current["monotonic_ns"] - observer.active_ready_at < args.settle_ms * 1_000_000
                                else "reader-ready")
            current["last_app_resource_snapshot"] = observer.last_resources
            current["app_resource_snapshot_age_ms"] = (None if observer.last_resources_at is None else
                                                       (current["monotonic_ns"] - observer.last_resources_at) / 1e6)
            ticks = current["user_cpu_ticks"] + current["system_cpu_ticks"]
            current["cpu_percent_of_one_core_since_prior_sample"] = (None if last_sample is None else
                100 * (ticks - last_sample["user_cpu_ticks"] - last_sample["system_cpu_ticks"])
                / os.sysconf("SC_CLK_TCK") / ((current["monotonic_ns"] - last_sample["monotonic_ns"]) / 1e9))
            require(current["rss_bytes"] <= args.rss_limit_mib * MIB
                    and current["rss_high_water_bytes"] <= args.rss_limit_mib * MIB,
                    "The actual App exceeded the observed RSS safety ceiling")
            samples.append(current)
            append_json(evidence / "resource-samples.jsonl", current)
            last_sample = current
            if observer.ready_events and page_proof is None:
                page_proof = complete_page_proof(guest, data, database, seed)
                write_json(evidence / "complete-page-proof.json", page_proof, exclusive=True)
                write_json(evidence / "reader_snapshot.json", {
                    "origin": "Actual App lifecycle reader-ready record; copied without changing production state",
                    "observation": observer.ready_events[0]}, exclusive=True)
            next_sample += interval
            if next_sample < time.monotonic():
                # Preserve real sampling gaps instead of emitting a burst of
                # catch-up samples that could disguise a blocked observation.
                next_sample = time.monotonic() + interval
            time.sleep(min(max(0, next_sample - time.monotonic()), remaining(deadline)))
        exit_ns = time.monotonic_ns()
        observer.read(final=True)
        remaining(deadline)
        guard.check()
        require(process.process.returncode == 0, "The real application exited unsuccessfully")
        require(page_proof is not None, "No fully validated real guest page was observed")
        guest.check_isolation(data, database)
        require(complete_page_proof(guest, data, database, seed) == page_proof,
                "Repeated real reader use changed the complete local page proof")
        result.update(analyze_case(args, observer, samples, exit_ns))
        remaining(deadline)
        guard.check()
        result.update({"passed": True, "status": "observed", "page_complete_proof": page_proof,
                       "application_exit_code": process.process.returncode})
    except BaseException as error:
        result.update({"passed": False, "status": "observability_gap" if isinstance(error, ObservationGap) else "failed",
                       "error": str(error), "traceback": traceback.format_exc(),
                       "ready_generation_count": len(observer.ready_events) if observer else 0,
                       "closed_count": len(observer.close_events) if observer else 0,
                       "raw_samples": len(samples)})
        if isinstance(error, (KeyboardInterrupt, SystemExit)):
            raise
    finally:
        with cleanup_signals():
            for owned in (process, xvfb):
                if owned is not None:
                    cleaned = owned.stop()
                    cleanup.append(cleaned)
                    if not cleaned["original_process_still_present"] and owned in guard.targets:
                        guard.remove(owned)
                    if cleaned["original_process_still_present"] or cleaned.get("reap_timed_out"):
                        result.update({"passed": False, "status": "failed", "cleanup_error": "An owned process survived bounded cleanup"})
        result["cleanup"] = cleanup
        write_json(evidence / "result.json", result)
    return result


def compare_cases(cases):
    if len(cases) != 2 or not all(case.get("passed") for case in cases):
        return {"available": False, "reason": "Both complete real workloads must pass before comparison"}
    left, right = cases
    comparison = {"available": True, "order": [left["name"], right["name"]],
                  "scope": "Controlled local observations from two fresh processes; no statistical speedup or Kindle power claim",
                  "host_file_cache_controlled": False, "comparison_asserts_improvement": False, "windows": {}}
    for window in ("initial_stable_idle", "post_cycles_stable_idle"):
        comparison["windows"][window] = {
            "missing_cover": left[window], "cached_cover": right[window],
            "cached_minus_missing_cpu_percentage_points": right[window]["cpu_percent_of_one_core"] - left[window]["cpu_percent_of_one_core"],
            "cached_minus_missing_rss_median_bytes": right[window]["rss_bytes"]["median"] - left[window]["rss_bytes"]["median"],
        }
    return comparison


def export_manifest(evidence):
    files = []
    for path in sorted(evidence.rglob("*")):
        require(not path.is_symlink(), "Safe export rejects symlinks")
        if path.is_dir():
            require(path.name not in ("data", "private", "captures", "pages", "covers"), "Unsafe export directory")
            continue
        require(path.is_file() and path.suffix in EXPORTED_SUFFIXES, "Safe export rejects this file type: " + path.name)
        require(not any(value in path.name.lower() for value in ("sqlite", ".db", "session.json", "capture")),
                "Safe export rejects databases, sessions, and captures")
        files.append({"path": path.relative_to(evidence).as_posix(), "bytes": path.stat().st_size, "sha256": sha256(path)})
    write_json(evidence / "safe-export-manifest.json", {"schema_version": 1, "manifest_excludes_itself": True,
        "safe_export_root": str(evidence), "private_fixture_data_exported": False,
        "database_exported": False, "captures_exported": False, "files": files}, exclusive=True)


def arguments():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--workspace", type=Path, required=True)
    parser.add_argument("--app-root", type=Path, required=True)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--expected-binary-sha256", required=True)
    parser.add_argument("--binary-source-manifest", type=Path, required=True)
    parser.add_argument("--expected-source-manifest-sha256", required=True)
    parser.add_argument("--assets-root", type=Path, required=True)
    parser.add_argument("--cover-asset", type=Path)
    parser.add_argument("--output", type=Path, required=True, help="A new isolated directory; export only its evidence/ child")
    parser.add_argument("--profile", choices=("pw5", "pw6", "scribe", "scribe2025"), default="pw5")
    parser.add_argument("--origin-page", type=int, choices=(18, 19), default=18)
    parser.add_argument("--cycles", type=int, default=20)
    parser.add_argument("--catalog-warmup-ms", type=int, default=2000)
    parser.add_argument("--initial-ready-ms", type=int, default=5000)
    parser.add_argument("--cycle-ready-ms", type=int, default=3000)
    parser.add_argument("--cycle-closed-ms", type=int, default=1500)
    parser.add_argument("--settle-ms", type=int, default=1000)
    parser.add_argument("--idle-seconds", type=float, default=8)
    parser.add_argument("--sample-interval-ms", type=int, default=100)
    parser.add_argument("--deadline-seconds", type=float, default=360)
    parser.add_argument("--rss-limit-mib", type=int, default=768)
    parser.add_argument("--max-ready-rss-growth-mib", type=int, default=64)
    parser.add_argument("--max-fd-growth", type=int, default=4)
    parser.add_argument("--max-thread-growth", type=int, default=4)
    return parser.parse_args()


def main():
    args = arguments()
    require(sys.platform == "linux", "Run this resource verifier only on the designated remote Linux worker")
    require(hasattr(os, "pidfd_open") and hasattr(signal, "pidfd_send_signal"),
            "The Linux worker must provide pidfd-based owned-process signaling")
    require(20 <= args.cycles <= 100, "At least twenty complete reader close/reopen cycles are required")
    require(args.cycle_ready_ms >= 2000 and args.cycle_closed_ms >= 1000,
            "Precomputed actions need at least two seconds per ready slot and one second per closed slot")
    require(args.catalog_warmup_ms >= 1000 and args.initial_ready_ms >= 2000 and args.settle_ms >= 500,
            "The controlled shelf and reader windows need explicit settling time")
    require(args.idle_seconds >= 5 and 50 <= args.sample_interval_ms <= 500,
            "Use a stable idle window and a bounded sampler interval")
    require(1 <= args.rss_limit_mib <= 768, "The observed workload RSS ceiling cannot exceed 768 MiB")
    require(args.deadline_seconds > 0 and args.max_ready_rss_growth_mib >= 0
            and args.max_fd_growth >= 0 and args.max_thread_growth >= 0, "Invalid resource safety or growth bound")
    for digest in (args.expected_binary_sha256, args.expected_source_manifest_sha256):
        require(re.fullmatch(r"[0-9a-fA-F]{64}", digest), "Selected inputs require explicit SHA-256 digests")
    args.workspace = args.workspace.resolve(strict=True)
    args.app_root = args.app_root.resolve(strict=True)
    args.binary = args.binary.resolve(strict=True)
    args.binary_source_manifest = args.binary_source_manifest.resolve(strict=True)
    args.assets_root = args.assets_root.resolve(strict=True)
    args.output = args.output.resolve()
    require(args.output.is_relative_to(args.workspace) and args.output != args.workspace
            and not args.output.is_relative_to(args.app_root), "Output must be new workspace evidence outside application sources")
    require(args.binary.is_file() and not args.binary.is_symlink(), "The selected binary must be a regular sealed file")
    require(sha256(args.binary) == args.expected_binary_sha256.lower(), "The selected desktop binary digest differs")
    require(sha256(args.binary_source_manifest) == args.expected_source_manifest_sha256.lower(), "The binary source manifest digest differs")
    expected_source = args.binary_source_manifest.read_text()
    require(source_manifest(args.app_root) == expected_source, "The current application inputs differ from the selected binary build manifest")
    with args.binary.open("rb") as stream:
        header = stream.read(64)
    require(header[:7] == b"\x7fELF\x02\x01\x01" and struct.unpack_from("<H", header, 18)[0] == 62,
            "Only the selected Linux x86-64 desktop ELF may be executed; ARM artifacts are not accepted")
    plan_duration = create_actions(args)[1]["explicit_exit_at_ms"] / 1000
    require(args.deadline_seconds >= 2 * plan_duration + 30,
            "The absolute deadline is shorter than the two complete precomputed workloads")
    os.umask(0o077)
    args.output.mkdir(parents=True, exist_ok=False)
    private, evidence = args.output / "private", args.output / "evidence"
    private.mkdir(mode=0o700)
    evidence.mkdir(mode=0o700)
    deadline = time.monotonic() + args.deadline_seconds
    guest, guest_source = load_guest_fixture(args.app_root)
    cover_asset = select_cover_asset(args.app_root, args.cover_asset)
    source_directory = evidence / "source"
    source_directory.mkdir(mode=0o700)
    runner_inputs = []
    for source, name in ((Path(__file__).resolve(), "resource_regression.py"),
                         (Path(__file__).with_name("resource_fixture.py").resolve(), "resource_fixture.py"),
                         (guest_source, "reader_guest_fixtures.py")):
        shutil.copyfile(source, source_directory / name)
        runner_inputs.append(file_identity(source))
    shutil.copyfile(args.binary_source_manifest, evidence / "binary-build-source.sha256")
    report = {"schema_version": 1, "passed": False, "status": "running", "started_at": utc_now(),
              "class": "controlled_local_resource_observation", "binary": file_identity(args.binary),
              "source_manifest_sha256": args.expected_source_manifest_sha256.lower(),
              "sampler": file_identity(Path(__file__).resolve()), "guest_fixture": file_identity(guest_source),
              "runner_inputs": runner_inputs,
              "host": {"hostname": platform.node(), "system": platform.system(), "machine": platform.machine()},
              "policy": {"absolute_deadline_seconds": args.deadline_seconds, "sample_interval_ms": args.sample_interval_ms,
                         "rss_safety_ceiling_bytes": args.rss_limit_mib * MIB,
                         "rss_ceiling_scope": "Observed combined App and private Xvfb RSS; independent 50 ms guard, not a kernel cgroup limit",
                         "cleanup_budget_seconds_per_owned_process": 5,
                         "cpu_units": "Percent of one CPU core from raw /proc user+system ticks",
                         "cycles_per_case": args.cycles,
                         "growth_tolerances": {"ready_rss_bytes": args.max_ready_rss_growth_mib * MIB,
                                               "fd_count": args.max_fd_growth, "threads": args.max_thread_growth}},
              "cases": [], "physical_device_connected": False, "arm_binary_executed": False,
              "live_account_used": False, "remote_transactions_performed": False,
              "captures_created": False, "safe_export_root": str(evidence),
              "limits": ["These are controlled local observations, not a statistical speedup or Kindle power claim.",
                         "Stable idle windows are reported separately from startup and repeated-use peaks.",
                         "A retained global source cache or allocator RSS need not become zero after reader close.",
                         "A seeded cover file does not by itself prove runtime cover publication; no such evidence is invented.",
                         "Export only evidence/; private/ contains the disposable database and source page copies."]}
    write_json(evidence / "results.json", report)
    guard = None
    interrupted = False
    try:
        guard = Guard(private, evidence, deadline, args.rss_limit_mib * MIB)
        for name, cached in (("missing-cover", False), ("cached-cover", True)):
            remaining(deadline)
            result = run_case(args, guest, cover_asset, name, cached, private, evidence, guard, deadline)
            report["cases"].append(result)
            print(json.dumps({"case": name, "passed": result["passed"], "status": result["status"],
                              "completed_cycles": result.get("completed_cycles"), "error": result.get("error")}), flush=True)
            write_json(evidence / "results.json", report)
            if not result["passed"]:
                break
        report["comparison"] = compare_cases(report["cases"])
        require(sha256(args.binary) == args.expected_binary_sha256.lower(), "The selected binary changed during the resource run")
        require(source_manifest(args.app_root) == expected_source, "Application source inputs changed during the resource run")
        require(all(sha256(Path(item["path"])) == item["sha256"] for item in runner_inputs),
                "A resource sampler or fixture source changed during the run")
        remaining(deadline)
        guard.check()
        report["inputs_unchanged_during_run"] = True
        report["passed"] = len(report["cases"]) == 2 and all(value.get("passed") is True for value in report["cases"])
        report["status"] = "observed" if report["passed"] else "failed"
    except BaseException as error:
        report.update({"passed": False, "status": "failed", "error": str(error), "traceback": traceback.format_exc()})
        interrupted = isinstance(error, (KeyboardInterrupt, SystemExit))
    finally:
        if guard is not None:
            with cleanup_signals():
                report["watchdog_cleanup"] = guard.stop()
            if (report["watchdog_cleanup"]["original_process_still_present"]
                    or report["watchdog_cleanup"].get("registered_workloads_remaining")
                    or report["watchdog_cleanup"].get("safety_failure")
                    or report["watchdog_cleanup"].get("exit_code") != 0):
                report.update({"passed": False, "status": "failed", "cleanup_error": "The guard reported a safety failure, nonzero exit, or surviving owned process"})
        report["finished_at"] = utc_now()
        write_json(evidence / "results.json", report)
        export_manifest(evidence)
    print(json.dumps({"passed": report["passed"], "status": report["status"], "safe_export_root": str(evidence)}), flush=True)
    return 130 if interrupted else 0 if report["passed"] else 1


if __name__ == "__main__":
    if len(sys.argv) == 3 and sys.argv[1] == "--internal-watchdog":
        sys.exit(watchdog_main(Path(sys.argv[2])))
    def interrupt(signum, frame):
        raise KeyboardInterrupt("Resource verifier interrupted by signal " + str(signum))
    for event in (signal.SIGTERM, signal.SIGHUP, signal.SIGINT):
        signal.signal(event, interrupt)
    sys.exit(main())
