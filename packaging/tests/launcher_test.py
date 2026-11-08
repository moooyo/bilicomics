"""Exercise same-process Kindle lifecycle supervision with isolated Linux fixtures."""

import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time
import unittest


SOURCE = Path(__file__).resolve().parents[1] / "kindle" / "launch.sh"

APPLICATION = r'''#!/usr/bin/env python3
import json
import os
from pathlib import Path
import signal
import sys
import time

env = os.environ
if sys.argv[1:2] == ["--probe-native-portrait-framebuffer"]:
    expected = ["--probe-native-portrait-framebuffer", "--kindle", "--device", env["BILICOMICS_APPROVED_PROFILE"]]
    if sys.argv[1:] != expected or env["BILICOMICS_APPROVED_FIRMWARE"] != "5.20.1":
        sys.exit(93)
    probes = Path(env["TEST_PORTRAIT_PROBES"])
    previous = probes.read_text().splitlines() if probes.exists() else []
    identity = Path("/proc/self/stat").read_text().rsplit(") ", 1)[1].split()
    wm = env.get("TEST_PORTRAIT_WM_PID", "")
    wm_state = Path(f"/proc/{wm}/status").read_text().split("State:", 1)[1].lstrip().splitlines()[0] if wm else "unavailable"
    with probes.open("a") as output:
        output.write(f"{os.getpid()}:{identity[2]}:{identity[3]}:{wm_state}\n")
    time.sleep(float(env["TEST_PORTRAIT_PROBE_DELAY"]))
    status = int(env["TEST_PORTRAIT_PROBE_EXIT"])
    if status:
        sys.exit(status)
    sequence = Path(env["TEST_PORTRAIT_SEQUENCE"])
    if sequence.exists():
        values = json.loads(sequence.read_text())
        value = values[min(len(previous), len(values) - 1)].encode()
    else:
        value = Path(env["TEST_PORTRAIT_METADATA"]).read_bytes()
    os.write(1, value)
    sys.exit(0)
if sys.argv[1:2] == ["--publish-native-portrait-orientation"]:
    if (len(sys.argv[1:]) != 5 or sys.argv[3:5] != ["--kindle", "--device"]
            or sys.argv[5] != env["BILICOMICS_APPROVED_PROFILE"]):
        sys.exit(93)
    publication_directory = Path(env["BILICOMICS_LIFECYCLE_DIR"])
    publication_header = f"{env['BILICOMICS_LAUNCH_GENERATION']}:{env['BILICOMICS_LAUNCH_TOKEN']}"
    if ((publication_directory / "generation").read_text() != publication_header + "\n"
            or (publication_directory / "nonce").read_text() != env["BILICOMICS_LAUNCH_TOKEN"] + "\n"):
        sys.exit(93)
    payload = sys.argv[2].split(":")
    if len(payload) != 2:
        sys.exit(93)
    if payload[0] == "CALIBRATED":
        if not payload[1].isdigit() or payload[1].startswith("0") or int(payload[1]) <= 1:
            sys.exit(93)
        previous = f"{publication_header}:{int(payload[1]) - 1}:suspend"
        if ((publication_directory / "lifecycle-request").read_text() != previous + "\n"
                or (publication_directory / "lifecycle-ack").read_text() not in (previous + "\n", previous + ":ok\n", previous + ":checkpoint-error\n")
                or (publication_directory / "lifecycle-ready").read_text() != publication_header + "\n"
                or not (publication_directory / "application-pid").is_file()):
            sys.exit(93)
        publication_name = "resume-native-portrait-orientation"
        publication_record = f"{publication_header}:{payload[1]}:CALIBRATED\n"
    elif payload[0] in ("U", "D", "FIXED") and payload[1] in ("0", "1", "2", "3"):
        if ((publication_directory / "application-pid").exists()
                or (publication_directory / "lifecycle-request").exists()):
            sys.exit(93)
        publication_name = "native-portrait-orientation"
        publication_record = f"{publication_header}:{payload[0]}:{payload[1]}\n"
    else:
        sys.exit(93)
    directory_fd = os.open(publication_directory, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC)
    publication_pending = publication_name + ".pending"
    pending_identity = None
    try:
        descriptor = os.open(publication_pending, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW | os.O_CLOEXEC,
                             0o600, dir_fd=directory_fd)
        with os.fdopen(descriptor, "w") as output:
            pending_identity = os.fstat(output.fileno())
            output.write(publication_record)
            output.flush()
            os.fsync(output.fileno())
        current = os.stat(publication_pending, dir_fd=directory_fd, follow_symlinks=False)
        if (current.st_dev, current.st_ino, current.st_nlink) != (pending_identity.st_dev, pending_identity.st_ino, 1):
            raise OSError("The fixture publication leaf changed")
        # An absent-only link models the fixture publication contract without
        # shell redirection or destination-directory move semantics. Production
        # uses the application's reviewed descriptor-relative no-replace rename.
        os.link(publication_pending, publication_name, src_dir_fd=directory_fd, dst_dir_fd=directory_fd,
                follow_symlinks=False)
        os.unlink(publication_pending, dir_fd=directory_fd)
        with Path(env["TEST_PORTRAIT_PUBLISHERS"]).open("a") as output:
            output.write(sys.argv[2] + "\n")
    except OSError:
        # Preserve a failed exclusive writer's pending evidence. The owning
        # launcher performs cleanup only after its bounded reader group stops.
        sys.exit(93)
    finally:
        os.close(directory_fd)
    print("PUBLISHED")
    sys.exit(0)
if sys.argv[1:] == ["--relay-power-events"]:
    import select
    with Path(env["TEST_OBSERVER_RELAY_PIDS"]).open("a") as output:
        output.write(str(os.getpid()) + "\n")
    generation = env["BILICOMICS_POWER_RELAY_GENERATION"].encode()
    token = env["BILICOMICS_POWER_RELAY_TOKEN"].encode()
    for descriptor in (0, 1, 3):
        os.set_blocking(descriptor, False)
    raw = bytearray()
    controls = bytearray()
    packet = notice = deadline = None
    latest = 0
    startup_seen = False
    allowed = {b"goingToScreenSaver", b"outOfScreenSaver", b"exitingScreenSaver", b"wakeupFromSuspend", b"usbConfigured"}

    def receive(descriptor):
        try:
            return os.read(descriptor, 256)
        except BlockingIOError:
            return None

    def frame(buffer):
        if b"\n" not in buffer:
            if len(buffer) > 255:
                sys.exit(97)
            return None
        end = buffer.index(10) + 1
        value = bytes(buffer[:end])
        del buffer[:end]
        if len(value) > 256 or any(byte != 10 and not 32 <= byte <= 126 for byte in value):
            sys.exit(97)
        return value

    def forward(value):
        try:
            if os.write(1, value) != len(value):
                sys.exit(97)
            return True
        except BlockingIOError:
            return False

    try:
        while True:
            if deadline is not None and time.monotonic() >= deadline:
                sys.exit(97)
            while True:
                control = frame(controls)
                if control is None:
                    block = receive(3)
                    if block == b"":
                        sys.exit(97)
                    if block is None:
                        if controls and deadline is None:
                            deadline = time.monotonic() + 2
                        break
                    controls.extend(block)
                    continue
                fields = control[:-1].split(b" ")
                if len(fields) < 3 or fields[1:3] != [generation, token]:
                    sys.exit(97)
                if fields[0] == b"_bilicomics_start_ready" and len(fields) == 3:
                    if startup_seen or latest:
                        continue
                    startup_seen = True
                elif fields[0] == b"_bilicomics_native_mode_ready" and len(fields) == 5:
                    if not fields[3].isdigit() or int(fields[3]) == 0 or not fields[4].isdigit() or int(fields[4]) == 0:
                        sys.exit(97)
                    if int(fields[4]) <= latest:
                        continue
                    latest = int(fields[4])
                else:
                    sys.exit(97)
                notice = control
                if deadline is None:
                    deadline = time.monotonic() + 2
            drained = False
            if packet is None:
                packet = frame(raw)
                if packet is None:
                    block = receive(0)
                    if block == b"":
                        sys.exit(0 if not raw and notice is None and not controls else 97)
                    if block is None:
                        drained = True
                    else:
                        raw.extend(block)
                        continue
                elif packet[:-1].split(b" ", 1)[0] not in allowed:
                    sys.exit(97)
            if packet is not None:
                if forward(packet):
                    packet = None
                    continue
            elif drained and not raw and notice is not None:
                if forward(notice):
                    notice = None
            if notice is None and not controls:
                deadline = None
            timeout = None if deadline is None else max(0, deadline - time.monotonic())
            select.select([3] + ([0] if packet is None else []),
                          [1] if packet is not None or (notice is not None and drained and not raw) else [], [], timeout)
    except OSError:
        sys.exit(97)

directory = Path(env["BILICOMICS_LIFECYCLE_DIR"])
generation = env["BILICOMICS_LAUNCH_GENERATION"]
token = env["BILICOMICS_LAUNCH_TOKEN"]
context = f"{generation}:{token}"
trace = Path(env["TEST_APP_TRACE"])
state_path = Path(env["TEST_APP_STATE"])
action_path = Path(env["TEST_APP_ACTION"])
pid = os.getpid()
pending = []
state = {
    "pid": pid, "comic": 28602, "episode": 2860203,
    "revision": "fixture-revision", "page": 8, "route": "reader",
    "startup_preference": "shelf", "suspended": False,
    "booting": env["TEST_BOOTING"] == "1",
}
startup_mode = (directory / "native-epdc-mode").read_text().strip().split(":")
if len(startup_mode) != 3 or startup_mode[:2] != [generation, token] or startup_mode[2] not in ("Y8", "Y8INV"):
    sys.exit(94)
state["native_mode"] = startup_mode[2]
startup_portrait_raw = (directory / "native-portrait-orientation").read_text()
startup_portrait = startup_portrait_raw.rstrip("\n").split(":")
fresh_rotation = Path(env["TEST_FRAMEBUFFER_ROTATION"]).read_text().strip()
if (len(startup_portrait) != 4 or startup_portrait[:2] != [generation, token]
        or startup_portrait[2] not in ("U", "D", "FIXED") or startup_portrait[3] not in ("0", "1", "2", "3")
        or startup_portrait_raw != ":".join(startup_portrait) + "\n" or fresh_rotation != startup_portrait[3]):
    sys.exit(94)
state["portrait_orientation"] = startup_portrait[2]
state["native_rotation"] = int(fresh_rotation)

def log(message):
    with trace.open("a") as output:
        output.write(f"{message}:{pid}\n")

def atomic_write(path, value):
    pending_path = path.with_name(path.name + ".pending")
    pending_path.write_text(value + "\n")
    pending_path.replace(path)

def snapshot():
    atomic_write(state_path, json.dumps(state))

def stop(signum, frame):
    log("terminated")
    sys.exit(int(env["TEST_TERM_EXIT"]))

def receive(signum, frame):
    pending.append("suspend" if signum == signal.SIGUSR1 else "resume")

if env["TEST_OUTPUT_CAPABILITY"] != "missing":
    atomic_write(directory / "native-output-capability", f"{context}:{pid}:{env['TEST_OUTPUT_CAPABILITY']}")

Path(env["TEST_ARGS"]).write_text("\n".join(sys.argv[1:]) + "\n")
Path(env["TEST_APPROVAL"]).write_text(
    env["BILICOMICS_APPROVED_PROFILE"] + "|" + env["BILICOMICS_APPROVED_FIRMWARE"]
)
with Path(env["TEST_STARTS"]).open("a") as output:
    output.write(str(pid) + "\n")
signal.signal(signal.SIGTERM, stop)
signal.signal(signal.SIGHUP, stop)
signal.signal(signal.SIGINT, stop)
snapshot()
log("started")
time.sleep(float(env["TEST_READY_DELAY"]))
signal.signal(signal.SIGUSR1, receive)
signal.signal(signal.SIGUSR2, receive)
atomic_write(directory / "lifecycle-ready", context)
log("ready")
if env["TEST_HOLD"] != "1":
    sys.exit(int(env["TEST_APP_EXIT"]))

while True:
    if pending:
        action = pending.pop(0)
        request = (directory / "lifecycle-request").read_text().strip()
        fields = request.split(":")
        if len(fields) != 4 or fields[:2] != [generation, token] or fields[3] != action:
            log("ignored-signal")
            continue
        if action == "resume":
            calibration = (directory / "resume-native-portrait-orientation").read_text()
            if calibration != ":".join(fields[:3]) + ":CALIBRATED\n":
                log("ignored-portrait-calibration")
                continue
            fresh_rotation = Path(env["TEST_FRAMEBUFFER_ROTATION"]).read_text().strip()
            if fresh_rotation not in ("0", "1", "2", "3"):
                log("portrait-probe-failed")
                continue
            rotation_delta = (int(fresh_rotation) - int(startup_portrait[3])) % 4
            if rotation_delta not in (0, 2) or (startup_portrait[2] == "FIXED" and rotation_delta != 0):
                log("portrait-probe-failed")
                continue
            state["portrait_orientation"] = startup_portrait[2] if rotation_delta == 0 else {"U": "D", "D": "U"}[startup_portrait[2]]
            state["native_rotation"] = int(fresh_rotation)
            mode = (directory / "resume-native-epdc-mode").read_text().strip().split(":")
            if len(mode) != 4 or mode[:3] != fields[:3] or mode[3] not in ("Y8", "Y8INV", "FRAMEBUFFER"):
                log("ignored-native-mode")
                continue
            state["resume_presentation"] = mode[3]
            if mode[3] == "FRAMEBUFFER":
                capability = (directory / "native-output-capability").read_text().strip()
                fresh = Path(env["TEST_FRAMEBUFFER_MODE"]).read_text().strip()
                if capability != f"{context}:{pid}:framebuffer8" or fresh not in ("Y8", "Y8INV"):
                    log("framebuffer-probe-failed")
                    continue
                state["native_mode"] = fresh
            else:
                state["native_mode"] = mode[3]
        state["suspended"] = action == "suspend"
        snapshot()
        log(action)
        time.sleep(float(env["TEST_ACK_DELAY"]))
        mode = env["TEST_ACK_MODE"]
        if mode not in ("manual", "none"):
            suffix = "checkpoint-error" if mode == "checkpoint-error" and action == "suspend" else "ok"
            atomic_write(directory / "lifecycle-ack", request if mode == "legacy" and action == "suspend" else request + ":" + suffix)
    if action_path.exists():
        action = action_path.read_text().strip()
        action_path.unlink()
        if action == "quit":
            log("ordinary-exit")
            sys.exit(int(env["TEST_APP_EXIT"]))
        if action == "next":
            if state["suspended"]:
                log("ignored-page")
            else:
                state["page"] += 1
                snapshot()
                log("next-page")
    if state["booting"] and Path(env["TEST_BOOT_RELEASE"]).exists():
        state["booting"] = False
        snapshot()
        log("boot-complete")
    time.sleep(0.01)
'''


class LauncherLifecycleTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="bilicomics-launcher-test-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.home = self.root / "bilicomics"
        (self.home / "app").mkdir(parents=True)
        (self.home / "data").mkdir()
        self.bin = self.root / "bin"
        self.bin.mkdir()
        (self.home / "data" / "device-profile").write_text("pw6\n")
        (self.home / "data" / "device-allowlist").write_text("pw6 5.20.1\n")
        (self.root / "firmware").write_text("Kindle 5.20.1 (fixture)\n")
        fixture_source = (
            SOURCE.read_text()
            .replace("/etc/prettyversion.txt", str(self.root / "firmware"))
            .replace("/dev/fb0", "/dev/null")
            .replace("/tmp/bilicomics-launcher.lock", str(self.root / "lock"))
            .replace("\nsleep 2\n", "\n")
            .replace("                launch_trace wake_barrier_dropped\n            fi\n            ;;\n        '_bilicomics_app_exit '*)",
                     "                launch_trace wake_barrier_dropped\n            fi\n"
                     '            printf "processed\\n" >>"$TEST_SUPERVISOR_BARRIERS"\n'
                     "            ;;\n        '_bilicomics_app_exit '*)")
        )
        self.launcher = self.root / "launch.sh"
        self.launcher.write_text(fixture_source)
        self.env = os.environ.copy()
        self.env.update(
            BILICOMICS_HOME=str(self.home),
            PATH=f"{self.bin}:{self.env['PATH']}",
            TEST_TRACE=str(self.root / "trace"),
            TEST_ARGS=str(self.root / "args"),
            TEST_APPROVAL=str(self.root / "approval"),
            TEST_STARTS=str(self.root / "starts"),
            TEST_APP_TRACE=str(self.root / "application-trace"),
            TEST_APP_STATE=str(self.root / "application-state"),
            TEST_APP_ACTION=str(self.root / "application-action"),
            TEST_STATUSBAR_STATE=str(self.root / "statusbar-state"),
            TEST_POWER_STATE=str(self.root / "power-state"),
            TEST_NATIVE_MODE=str(self.root / "native-mode"),
            TEST_FRAMEBUFFER_MODE=str(self.root / "framebuffer-mode"),
            TEST_FRAMEBUFFER_ROTATION=str(self.root / "framebuffer-rotation"),
            TEST_PORTRAIT_METADATA=str(self.root / "portrait-metadata"),
            TEST_PORTRAIT_SEQUENCE=str(self.root / "portrait-sequence.json"),
            TEST_PORTRAIT_PROBES=str(self.root / "portrait-probes"),
            TEST_PORTRAIT_PUBLISHERS=str(self.root / "portrait-publishers"),
            TEST_PORTRAIT_PROBE_DELAY="0",
            TEST_PORTRAIT_PROBE_EXIT="0",
            TEST_NATIVE_ORIENTATION=str(self.root / "native-orientation"),
            TEST_OUTPUT_CAPABILITY="legacy",
            TEST_NATIVE_QUERIES=str(self.root / "native-queries"),
            TEST_CAPTURE_GROUPS="",
            BILICOMICS_LAUNCH_TRACE="0",
            TEST_OBSERVER_RELAY_PIDS=str(self.root / "observer-relay-pids"),
            TEST_OBSERVER_CLIENT_PIDS=str(self.root / "observer-client-pids"),
            TEST_SUPERVISOR_BARRIERS=str(self.root / "supervisor-barriers"),
            TEST_BOOT_RELEASE=str(self.root / "boot-release"),
            TEST_BOOTING="0",
            TEST_HOLD="0",
            TEST_APP_EXIT="0",
            TEST_TERM_EXIT="0",
            TEST_READY_DELAY="0",
            TEST_ACK_DELAY="0",
            TEST_ACK_MODE="normal",
        )
        (self.root / "statusbar-state").write_text("start/running, process 123\n")
        (self.root / "power-state").write_text("active\n")
        (self.root / "native-mode").write_text("Y8\n")
        (self.root / "framebuffer-mode").write_text("Y8\n")
        (self.root / "framebuffer-rotation").write_text("0\n")
        (self.root / "portrait-metadata").write_text("FIXED:0\n")
        (self.root / "native-orientation").write_text("U\n")
        self.script(self.bin / "status", 'printf "statusbar %s\\n" "$(cat "$TEST_STATUSBAR_STATE")"')
        self.script(
            self.bin / "stop",
            'printf "stop:%s\\n" "$*" >>"$TEST_TRACE"\n'
            'printf "stop/waiting\\n" >"$TEST_STATUSBAR_STATE"',
        )
        self.script(
            self.bin / "start",
            'printf "start:%s\\n" "$*" >>"$TEST_TRACE"\n'
            'printf "start/running, process 123\\n" >"$TEST_STATUSBAR_STATE"',
        )
        self.script(self.bin / "xrefresh", 'printf "xrefresh:%s\\n" "$*" >>"$TEST_TRACE"')
        self.script(self.bin / "pidof", "exit 1")
        self.observer_source("exec sleep 60")
        self.native_getter()
        self.script(self.bin / "lipc-set-prop", 'printf "unexpected-global-write:%s\\n" "$*" >>"$TEST_TRACE"\nexit 95')
        self.script(self.bin / "get-dynconf-value", 'printf "unexpected-persisted-read:%s\\n" "$*" >>"$TEST_TRACE"\nexit 95')
        usleep = self.bin / "usleep"
        usleep.write_text("#!/usr/bin/env python3\nimport sys, time\ntime.sleep(int(sys.argv[1]) / 1_000_000)\n")
        usleep.chmod(0o755)
        application = self.home / "app" / "bilicomics"
        application.write_text(APPLICATION)
        application.chmod(0o755)

    @staticmethod
    def script(path, body):
        path.write_text("#!/bin/sh\nset -eu\n" + body + "\n")
        path.chmod(0o755)

    def native_getter(self, mode_body='cat "$TEST_NATIVE_MODE"', power_body='cat "$TEST_POWER_STATE"',
                      orientation_body='cat "$TEST_NATIVE_ORIENTATION"'):
        self.script(
            self.bin / "lipc-get-prop",
            'printf "%s\\n" "$*" >>"$TEST_NATIVE_QUERIES"\n'
            'if [ -n "$TEST_CAPTURE_GROUPS" ]; then\n'
            '  ( IFS= read -r entry </proc/self/stat; tail=${entry##*) }; set -- $tail; '
            'printf "%s:%s\\n" "$3" "$4" >>"$TEST_CAPTURE_GROUPS" )\n'
            'fi\n'
            'case "$*" in\n'
            '  "-s com.lab126.winmgr epdcMode")\n' + mode_body + '\n    ;;\n'
            '  "-s com.lab126.powerd state")\n' + power_body + '\n    ;;\n'
            '  "com.lab126.winmgr accelerometer")\n' + orientation_body + '\n    ;;\n'
            '  *) exit 96 ;;\n'
            'esac',
        )

    def observer_source(self, body):
        self.script(self.bin / "lipc-wait-event",
                    'printf "%s\\n" "$$" >>"$TEST_OBSERVER_CLIENT_PIDS"\n' + body)

    def python_observer(self, body):
        driver = self.root / "observer-driver.py"
        driver.write_text("import os, signal, sys, time\nfrom pathlib import Path\nenv = os.environ\n" + body)
        self.env["TEST_OBSERVER_DRIVER"] = str(driver)
        self.observer_source('exec python3 "$TEST_OBSERVER_DRIVER"')

    def gate_observer(self, payload, hold=True):
        self.env.update(TEST_SOURCE_GATE=str(self.root / "source-gate"),
                        TEST_SOURCE_DONE=str(self.root / "source-done"),
                        TEST_SOURCE_PAYLOAD=str(self.root / "source-payload"))
        (self.root / "source-payload").write_bytes(payload)
        self.python_observer(
            'while not Path(env["TEST_SOURCE_GATE"]).exists(): time.sleep(0.01)\n'
            'data = Path(env["TEST_SOURCE_PAYLOAD"]).read_bytes()\n'
            'while data:\n'
            '    data = data[os.write(1, data):]\n'
            'Path(env["TEST_SOURCE_DONE"]).touch()\n'
            + ('while True: time.sleep(60)\n' if hold else '')
        )

    def fill_event_fifo(self):
        import fcntl
        descriptor = os.open(self.root / "lock" / "power-events", os.O_RDWR | os.O_NONBLOCK)
        record = b"wakeupFromSuspend 0\n"
        count = 0
        try:
            fcntl.fcntl(descriptor, fcntl.F_SETPIPE_SZ, 4096)
            while True:
                try:
                    self.assertEqual(os.write(descriptor, record), len(record))
                    count += 1
                except BlockingIOError:
                    break
        finally:
            os.close(descriptor)
        self.assertGreater(count, 0)
        return count

    @staticmethod
    def process_identity(pid):
        value = Path(f"/proc/{pid}/stat").read_text()
        fields = value.rsplit(") ", 1)[1].split()
        return {"pid": int(pid), "parent": int(fields[1]), "group": int(fields[2]),
                "session": int(fields[3]), "birth": fields[19], "state": fields[0]}

    def stop_launcher_after_failure(self, child):
        if child.poll() is not None:
            return child.communicate(timeout=3)
        child.send_signal(signal.SIGTERM)
        try:
            return child.communicate(timeout=5)
        except subprocess.TimeoutExpired:
            # The private record is not proof by itself. Recheck the live
            # original child, its birth, and its complete session identity.
            lock = self.root / "lock"
            try:
                generation, nonce, pid, birth = (lock / "observer-owned").read_text().strip().split(":")
                identity = self.process_identity(pid)
                if ((lock / "generation").read_text().strip() == f"{generation}:{nonce}"
                        and (lock / "nonce").read_text().strip() == nonce
                        and identity["parent"] == child.pid and identity["birth"] == birth
                        and identity["group"] == identity["session"] == identity["pid"]):
                    os.killpg(identity["pid"], signal.SIGKILL)
            except (OSError, ValueError, IndexError):
                pass
            os.killpg(child.pid, signal.SIGKILL)
            return child.communicate(timeout=3)

    def run_launcher(self, drive=None):
        child = subprocess.Popen(
            ["sh", str(self.launcher), "--detached"],
            env=self.env,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            start_new_session=True,
        )
        try:
            if drive is not None:
                drive(child)
            output, error = child.communicate(timeout=18)
        except subprocess.TimeoutExpired:
            self.stop_launcher_after_failure(child)
            self.fail("The fixture launcher did not finish in time.")
        except BaseException:
            self.stop_launcher_after_failure(child)
            raise
        try:
            os.killpg(child.pid, 0)
        except ProcessLookupError:
            pass
        else:
            os.killpg(child.pid, signal.SIGKILL)
            self.fail("The fixture launcher left a child process running.")
        for name in ("observer-relay-pids", "observer-client-pids"):
            for pid in self.lines(name):
                try:
                    self.assertIn(self.process_identity(pid)["state"], ("Z", "X"),
                                  "The fixture launcher left an observer or relay process running.")
                except FileNotFoundError:
                    pass
        return child.returncode, output, error

    def lines(self, name):
        path = self.root / name
        return path.read_text().splitlines() if path.exists() else []

    def timing(self):
        path = self.home / "data" / "logs" / "launcher-timing.tsv"
        return [dict(uptime=float(fields[0]), event=fields[1],
                     **dict(field.split("=", 1) for field in fields[2:]))
                for fields in (line.rstrip("\n").split("\t") for line in path.read_text().splitlines(keepends=True)
                               if line.endswith("\n"))] if path.exists() else []

    def trace(self):
        return self.lines("trace")

    def starts(self):
        return self.lines("starts")

    def app_events(self):
        return [line.rsplit(":", 1)[0] for line in self.lines("application-trace")]

    def state(self):
        path = self.root / "application-state"
        return json.loads(path.read_text()) if path.exists() else {}

    def wait_until(self, predicate, timeout=9):
        deadline = time.monotonic() + timeout
        while not predicate():
            if time.monotonic() >= deadline:
                self.fail("The fixture did not reach the expected lifecycle state.")
            time.sleep(0.01)

    def emit(self, *events):
        with (self.root / "lock" / "power-events").open("w") as channel:
            channel.write("\n".join(events) + "\n")

    def app_action(self, action):
        path = self.root / "application-action"
        temporary = path.with_name(path.name + ".pending")
        temporary.write_text(action)
        temporary.replace(path)

    def ready(self):
        self.wait_until(lambda: "ready" in self.app_events())

    def suspend(self):
        count = self.trace().count("xrefresh:")
        self.emit("goingToScreenSaver 2")
        self.wait_until(lambda: self.trace().count("xrefresh:") == count + 1)

    def resume(self):
        count = self.app_events().count("resume")
        self.emit("outOfScreenSaver 1", "exitingScreenSaver 1")
        self.wait_until(lambda: self.app_events().count("resume") == count + 1)

    def request(self):
        return (self.root / "lock" / "lifecycle-request").read_text().strip()

    def acknowledge(self, value=None):
        ack = self.root / "lock" / "lifecycle-ack"
        pending = ack.with_name(ack.name + ".pending")
        pending.write_text((value or self.request() + ":ok") + "\n")
        pending.replace(ack)

    def configure_portrait(self, orientation, rotation, kind="AUTO"):
        (self.root / "native-orientation").write_text(orientation + "\n")
        (self.root / "portrait-metadata").write_text(f"{kind}:{rotation}\n")
        (self.root / "framebuffer-rotation").write_text(f"{rotation}\n")

    def configure_native_profile(self, profile):
        (self.home / "data" / "device-profile").write_text(profile + "\n")
        (self.home / "data" / "device-allowlist").write_text(f"{profile} 5.20.1\n")

    def test_unaccepted_firmware_changes_no_services(self):
        (self.home / "data" / "device-allowlist").write_text("# Empty until acceptance.\n")
        status, _, error = self.run_launcher()
        self.assertEqual(status, 1)
        self.assertIn("has not been accepted", error)
        self.assertEqual(self.trace(), [])
        self.assertEqual(self.starts(), [])
        self.assertFalse((self.root / "lock").exists())

    def test_ordinary_exit_restores_services_and_passes_config(self):
        status, _, error = self.run_launcher()
        self.assertEqual(status, 0, error)
        self.assertEqual(self.trace(), ["stop:statusbar", "start:statusbar", "xrefresh:"])
        self.assertEqual((self.root / "approval").read_text(), "pw6|5.20.1")
        self.assertEqual(len(self.starts()), 1)
        self.assertEqual(
            self.lines("args"),
            ["--kindle", "--device", "pw6", "--data-dir", str(self.home / "data")],
        )
        self.assertFalse((self.root / "lock").exists())
        self.assertFalse((self.home / "data" / "logs" / "launcher-timing.tsv").exists())

    def test_application_failure_preserves_exit_code(self):
        self.env["TEST_APP_EXIT"] = "7"
        status, _, _ = self.run_launcher()
        self.assertEqual(status, 7)
        self.assertEqual(self.trace(), ["stop:statusbar", "start:statusbar", "xrefresh:"])

    def test_windows_configuration_line_endings_are_accepted(self):
        (self.home / "data" / "device-profile").write_bytes(b"pw6\r\n")
        (self.home / "data" / "device-allowlist").write_bytes(b"pw6 5.20.1\r\n")
        status, _, error = self.run_launcher()
        self.assertEqual(status, 0, error)
        self.assertEqual((self.root / "approval").read_text(), "pw6|5.20.1")

    def test_inverted_native_startup_publishes_compensation_without_a_global_write(self):
        self.env["TEST_HOLD"] = "1"
        (self.root / "native-mode").write_text("Y8INV\n")

        def drive(_):
            self.ready()
            self.assertEqual(self.state()["native_mode"], "Y8INV")
            self.assertEqual((self.root / "native-mode").read_text(), "Y8INV\n")
            self.app_action("quit")

        status, _, error = self.run_launcher(drive)
        self.assertEqual(status, 0, error)
        self.assertEqual(self.trace(), ["stop:statusbar", "start:statusbar", "xrefresh:"])
        self.assertEqual(self.lines("native-queries"), [
            "-s com.lab126.powerd state", "-s com.lab126.winmgr epdcMode", "-s com.lab126.powerd state",
        ])

    def test_startup_portrait_calibration_is_sampled_before_the_window_manager_pause(self):
        self.env.update(TEST_HOLD="1", TEST_CAPTURE_GROUPS=str(self.root / "capture-groups"))
        self.configure_portrait("D", 1)
        manager = subprocess.Popen(["sleep", "60"])
        self.addCleanup(self.stop_fixture_process, manager)
        self.env["TEST_PORTRAIT_WM_PID"] = str(manager.pid)
        self.script(self.bin / "pidof", f"printf '%s\\n' '{manager.pid}'")
        self.native_getter(orientation_body=
                           f'case "$(sed -n \'s/^State:[[:space:]]*//p\' /proc/{manager.pid}/status)" in\n'
                           '  T*|t*) exit 97 ;;\n'
                           'esac\ncat "$TEST_NATIVE_ORIENTATION"')

        def drive(_):
            self.ready()
            self.wait_until(lambda: self.process_state(manager.pid).startswith("T"))
            generation, token = (self.root / "lock" / "generation").read_text().strip().split(":")
            self.assertEqual((self.root / "lock" / "native-portrait-orientation").read_text(),
                             f"{generation}:{token}:D:1\n")
            self.assertEqual(self.state()["portrait_orientation"], "D")
            self.assertEqual(self.state()["native_rotation"], 1)
            self.app_action("quit")

        status, _, error = self.run_launcher(drive)
        self.assertEqual(status, 0, error)
        probes = [entry.split(":", 3) for entry in self.lines("portrait-probes")]
        self.assertEqual(len(probes), 2)
        self.assertTrue(all(not probe[3].startswith(("T", "t")) for probe in probes))
        capture_groups = self.lines("capture-groups")
        self.assertEqual(len(capture_groups), 5)
        self.assertEqual(len(set(capture_groups)), 1)
        self.assertTrue(all(":".join(probe[1:3]) == capture_groups[0] for probe in probes))
        self.assertEqual(self.lines("native-queries"), [
            "-s com.lab126.powerd state", "-s com.lab126.winmgr epdcMode",
            "com.lab126.winmgr accelerometer", "com.lab126.winmgr accelerometer",
            "-s com.lab126.powerd state",
        ])
        self.assertFalse(self.process_state(manager.pid).startswith("T"))

    def test_startup_portrait_direction_is_not_assumed_from_an_absolute_rotation_index(self):
        self.env["TEST_HOLD"] = "1"
        for count, (orientation, rotation) in enumerate((("U", 0), ("D", 3), ("U", 2), ("D", 1)), 1):
            with self.subTest(orientation=orientation, rotation=rotation):
                self.configure_portrait(orientation, rotation)
                expected_starts = count

                def drive(_):
                    self.wait_until(lambda: len(self.starts()) == expected_starts and self.state().get("portrait_orientation") == orientation)
                    self.assertEqual(self.state()["native_rotation"], rotation)
                    self.app_action("quit")

                status, _, error = self.run_launcher(drive)
                self.assertEqual(status, 0, error)
                self.assertFalse((self.root / "lock").exists())

    def test_scribe_native_l_r_labels_publish_normalized_portrait_directions(self):
        self.env["TEST_HOLD"] = "1"
        self.configure_native_profile("scribe")
        for count, (native, rotation, normalized) in enumerate((("L", 3, "U"), ("R", 1, "D")), 1):
            with self.subTest(native=native):
                self.configure_portrait(native, rotation)
                expected_starts = count

                def drive(_):
                    self.wait_until(lambda: len(self.starts()) == expected_starts
                                    and self.state().get("portrait_orientation") == normalized)
                    header = (self.root / "lock" / "generation").read_text().strip()
                    self.assertEqual((self.root / "lock" / "native-portrait-orientation").read_text(),
                                     f"{header}:{normalized}:{rotation}\n")
                    self.app_action("quit")

                status, _, error = self.run_launcher(drive)
                self.assertEqual(status, 0, error)
                self.assertFalse((self.root / "lock").exists())

    def test_scribe_mixed_labels_must_agree_after_portrait_normalization(self):
        self.env.update(TEST_HOLD="1", TEST_ORIENTATION_FIRST=str(self.root / "orientation-first"))
        self.configure_native_profile("scribe")
        self.native_getter(orientation_body=
                           'if [ -e "$TEST_ORIENTATION_FIRST" ]; then printf "%s\\n" "$TEST_ORIENTATION_SECOND_LABEL"; else\n'
                           '  : >"$TEST_ORIENTATION_FIRST"; printf "%s\\n" "$TEST_ORIENTATION_FIRST_LABEL"\n'
                           'fi')
        for count, (first, second, rotation, normalized) in enumerate((("U", "L", 3, "U"), ("D", "R", 1, "D")), 1):
            with self.subTest(first=first, second=second):
                self.configure_portrait(first, rotation)
                self.env.update(TEST_ORIENTATION_FIRST_LABEL=first, TEST_ORIENTATION_SECOND_LABEL=second)
                (self.root / "orientation-first").unlink(missing_ok=True)
                expected_starts = count

                def drive(_):
                    self.wait_until(lambda: len(self.starts()) == expected_starts
                                    and self.state().get("portrait_orientation") == normalized)
                    self.app_action("quit")

                status, _, error = self.run_launcher(drive)
                self.assertEqual(status, 0, error)
                self.assertFalse((self.root / "lock").exists())
        self.configure_portrait("U", 3)
        self.env.update(TEST_ORIENTATION_FIRST_LABEL="U", TEST_ORIENTATION_SECOND_LABEL="R")
        (self.root / "orientation-first").unlink()
        previous_starts, previous_trace = self.starts(), self.trace()
        status, _, error = self.run_launcher()
        self.assertEqual(status, 1, error)
        self.assertEqual(self.starts(), previous_starts)
        self.assertEqual(self.trace(), previous_trace)
        self.assertFalse((self.root / "lock").exists())

    def test_unreviewed_profiles_reject_scribe_l_r_orientation_labels(self):
        for profile in ("pw5", "pw6", "scribe2025"):
            for label in ("L", "R"):
                with self.subTest(profile=profile, label=label):
                    self.configure_native_profile(profile)
                    self.configure_portrait(label, 3)
                    status, _, error = self.run_launcher()
                    self.assertEqual(status, 1, error)
                    self.assertEqual(self.starts(), [])
                    self.assertEqual(self.trace(), [])
                    self.assertFalse((self.root / "lock").exists())

    def test_fixed_portrait_inventory_publishes_a_static_calibration_without_lipc_direction_queries(self):
        self.env["TEST_HOLD"] = "1"
        self.configure_portrait("unavailable", 2, kind="FIXED")

        def drive(_):
            self.ready()
            prefix = (self.root / "lock" / "generation").read_text().strip()
            self.assertEqual((self.root / "lock" / "native-portrait-orientation").read_text(), f"{prefix}:FIXED:2\n")
            self.assertEqual(self.state()["portrait_orientation"], "FIXED")
            self.app_action("quit")

        status, _, error = self.run_launcher(drive)
        self.assertEqual(status, 0, error)
        self.assertEqual(len(self.lines("portrait-probes")), 2)
        self.assertNotIn("com.lab126.winmgr accelerometer", self.lines("native-queries"))

    def test_malformed_or_overflowed_portrait_metadata_never_takes_display_control(self):
        for value in ("AUTO:4\n", "AUTO:03\n", "FIXED:-1\n", "AUTO:3", "AUTO:3\r\n",
                      "AUTO:3\n\n", " AUTO:3\n", "AUTO:3\x00", "AUTO:3\n" + "x" * 129):
            with self.subTest(value=repr(value)):
                (self.root / "portrait-metadata").write_text(value)
                status, _, error = self.run_launcher()
                self.assertEqual(status, 1, error)
                self.assertIn("could not be read before startup", error)
                self.assertEqual(self.trace(), [])
                self.assertEqual(self.starts(), [])
                self.assertFalse((self.root / "lock").exists())

    def test_unknown_or_ambiguous_lipc_portrait_direction_never_takes_display_control(self):
        self.configure_portrait("U", 3)
        for value in ("R\n", "U\n\n", "D extra\n", "U\nD\n", "D\x00", " D\n", "D\r\n", ""):
            with self.subTest(value=repr(value)):
                (self.root / "native-orientation").write_text(value)
                status, _, error = self.run_launcher()
                self.assertEqual(status, 1, error)
                self.assertEqual(self.trace(), [])
                self.assertEqual(self.starts(), [])
                self.assertFalse((self.root / "lock").exists())

    def test_changed_portrait_metadata_or_direction_is_rejected_before_takeover(self):
        self.configure_portrait("U", 3)
        for second in ("AUTO:1\n", "FIXED:3\n"):
            with self.subTest(second=second):
                (self.root / "portrait-sequence.json").write_text(json.dumps(["AUTO:3\n", second]))
                (self.root / "portrait-probes").unlink(missing_ok=True)
                status, _, error = self.run_launcher()
                self.assertEqual(status, 1, error)
                self.assertEqual(self.trace(), [])
                self.assertEqual(self.starts(), [])
                self.assertFalse((self.root / "lock").exists())
        (self.root / "portrait-sequence.json").unlink()
        self.env["TEST_ORIENTATION_FIRST"] = str(self.root / "orientation-first")
        self.native_getter(orientation_body=
                           'if [ -e "$TEST_ORIENTATION_FIRST" ]; then printf "D\\n"; else\n'
                           '  : >"$TEST_ORIENTATION_FIRST"; printf "U\\n"\n'
                           'fi')
        status, _, error = self.run_launcher()
        self.assertEqual(status, 1, error)
        self.assertEqual(self.trace(), [])
        self.assertEqual(self.starts(), [])
        self.assertFalse((self.root / "lock").exists())

    def test_portrait_helper_and_lipc_reads_share_the_existing_transaction_deadline(self):
        self.configure_portrait("U", 3)
        self.env["TEST_PORTRAIT_PROBE_DELAY"] = "0.75"
        self.native_getter(orientation_body='sleep 0.6\ncat "$TEST_NATIVE_ORIENTATION"')
        started = time.monotonic()
        status, _, error = self.run_launcher()
        self.assertEqual(status, 1, error)
        self.assertIn("could not be read before startup", error)
        self.assertLess(time.monotonic() - started, 6)
        self.assertEqual(self.trace(), [])
        self.assertEqual(self.starts(), [])
        for record in self.lines("portrait-probes"):
            path = Path(f"/proc/{record.split(':', 1)[0]}/status")
            if path.exists():
                self.assertTrue(self.process_state(record.split(":", 1)[0]).startswith(("Z", "X")))
        self.assertFalse((self.root / "lock").exists())

    def test_older_application_without_portrait_helper_fails_before_native_takeover(self):
        self.env["TEST_PORTRAIT_PROBE_EXIT"] = "93"
        status, _, error = self.run_launcher()
        self.assertEqual(status, 1, error)
        self.assertEqual(self.trace(), [])
        self.assertEqual(self.starts(), [])
        self.assertFalse((self.root / "lock").exists())

    def test_sleep_or_usb_queued_during_portrait_sampling_precedes_the_startup_barrier(self):
        self.env["TEST_PORTRAIT_PROBE_DELAY"] = "0.2"
        for event, expected_status in (("goingToScreenSaver 2", 1), ("usbConfigured 1", 143)):
            with self.subTest(event=event):
                (self.root / "portrait-probes").unlink(missing_ok=True)

                def drive(_):
                    self.wait_until(lambda: bool(self.lines("portrait-probes")))
                    self.emit(event)

                status, _, error = self.run_launcher(drive)
                self.assertEqual(status, expected_status, error)
                self.assertEqual(self.trace(), [])
                self.assertEqual(self.starts(), [])
                self.assertFalse((self.root / "lock").exists())

    def test_portrait_publication_rejects_hostile_leaves_without_external_writes(self):
        self.env["TEST_PORTRAIT_PROBE_DELAY"] = "0.2"
        external_file = self.root / "external-publication-target"
        external_directory = self.root / "external-publication-directory"
        external_missing = self.root / "external-publication-missing"
        external_file.write_text("unchanged\n")
        external_directory.mkdir()
        for kind in ("pending-fifo", "pending-symlink", "pending-dangling-symlink",
                     "final-directory", "final-directory-symlink", "final-file-symlink"):
            with self.subTest(kind=kind):
                (self.root / "portrait-probes").unlink(missing_ok=True)
                injected = []

                def drive(_):
                    self.wait_until(lambda: bool(self.lines("portrait-probes")))
                    name = "native-portrait-orientation.pending" if kind.startswith("pending-") else "native-portrait-orientation"
                    leaf = self.root / "lock" / name
                    if kind == "pending-fifo":
                        os.mkfifo(leaf, 0o600)
                    elif kind == "pending-dangling-symlink":
                        leaf.symlink_to(external_missing)
                    elif kind == "final-directory":
                        leaf.mkdir()
                    elif kind == "final-directory-symlink":
                        leaf.symlink_to(external_directory, target_is_directory=True)
                    else:
                        leaf.symlink_to(external_file)
                    injected.append(leaf)

                started = time.monotonic()
                status, _, error = self.run_launcher(drive)
                self.assertEqual(status, 1, error)
                self.assertLess(time.monotonic() - started, 6)
                self.assertEqual(self.trace(), [])
                self.assertEqual(self.starts(), [])
                self.assertEqual(external_file.read_text(), "unchanged\n")
                self.assertEqual(list(external_directory.iterdir()), [])
                self.assertFalse(external_missing.exists())
                for leaf in injected:
                    if leaf.is_dir() and not leaf.is_symlink():
                        self.assertEqual(list(leaf.iterdir()), [])
                        leaf.rmdir()
                if (self.root / "lock").exists():
                    self.assertEqual(list((self.root / "lock").iterdir()), [])
                    (self.root / "lock").rmdir()

    def test_resume_samples_the_new_native_mode_for_each_exact_request(self):
        self.env["TEST_HOLD"] = "1"

        def drive(_):
            self.ready()
            original = self.state()["pid"]
            for mode in ("Y8INV", "Y8"):
                self.suspend()
                queries = self.lines("native-queries")
                time.sleep(0.15)
                self.assertEqual(self.lines("native-queries"), queries)
                (self.root / "native-mode").write_text(mode + "\n")
                self.resume()
                self.assertEqual(self.state()["native_mode"], mode)
                self.assertEqual(self.state()["pid"], original)
                generation, token, sequence, action = self.request().split(":")
                self.assertEqual(action, "resume")
                self.assertEqual((self.root / "lock" / "resume-native-epdc-mode").read_text().strip(),
                                 f"{generation}:{token}:{sequence}:{mode}")
                self.assertEqual((self.root / "lock" / "resume-native-portrait-orientation").read_text(),
                                 f"{generation}:{token}:{sequence}:CALIBRATED\n")
            self.app_action("quit")

        status, _, error = self.run_launcher(drive)
        self.assertEqual(status, 0, error)
        self.assertEqual(self.lines("native-queries").count("-s com.lab126.winmgr epdcMode"), 3)
        self.assertNotIn("ignored-native-mode", self.app_events())
        self.assertEqual(self.trace(), ["stop:statusbar", "start:statusbar", "xrefresh:"] * 3)

    def test_unknown_or_ambiguous_startup_mode_never_takes_native_control(self):
        for value in ("RGB32\n", "Y8INV\n\n", "Y8INV extra\n", "Y8\nY8INV\n", "Y8\x00", ""):
            with self.subTest(value=repr(value)):
                (self.root / "native-mode").write_text(value)
                status, _, error = self.run_launcher()
                self.assertEqual(status, 1, error)
                self.assertIn("could not be read before startup", error)
                self.assertEqual(self.trace(), [])
                self.assertEqual(self.starts(), [])
                self.assertFalse((self.root / "lock").exists())

    def test_getter_failure_and_output_overflow_fail_before_takeover(self):
        for body in ('exit 7', 'printf "Y8INV\\n"\ndd if=/dev/zero bs=1024 count=100 2>/dev/null'):
            with self.subTest(body=body):
                self.native_getter(body)
                status, _, error = self.run_launcher()
                self.assertEqual(status, 1, error)
                self.assertIn("could not be read before startup", error)
                self.assertEqual(self.trace(), [])
                self.assertEqual(self.starts(), [])
                self.assertFalse((self.root / "lock").exists())

    def test_hung_getter_is_bounded_and_its_private_descendants_are_stopped(self):
        pids = self.root / "getter-pids"
        self.env["TEST_GETTER_PIDS"] = str(pids)
        self.native_getter('printf "%s\\n" "$$" >"$TEST_GETTER_PIDS"\n'
                           'trap "" INT TERM\n'
                           'sleep 60 &\n'
                           'printf "%s\\n" "$!" >>"$TEST_GETTER_PIDS"\n'
                           'wait')
        started = time.monotonic()
        status, _, error = self.run_launcher()
        self.assertEqual(status, 1, error)
        self.assertLess(time.monotonic() - started, 8)
        self.assertIn("could not be read before startup", error)
        self.assertEqual(self.trace(), [])
        self.assertEqual(self.starts(), [])
        for pid in pids.read_text().splitlines():
            path = Path(f"/proc/{pid}/status")
            if path.exists():
                self.assertTrue(self.process_state(pid).startswith(("Z", "X")))
        self.assertFalse((self.root / "lock").exists())

    def test_unknown_resume_mode_leaves_the_suspended_application_without_input(self):
        self.env["TEST_HOLD"] = "1"

        def drive(_):
            self.ready()
            self.suspend()
            (self.root / "native-mode").write_text("unknown\n")
            self.emit("outOfScreenSaver 1", "exitingScreenSaver 1")

        status, _, error = self.run_launcher(drive)
        self.assertEqual(status, 1, error)
        self.assertIn("could not be read before resume", error)
        self.assertNotIn("resume", self.app_events())
        self.assertTrue(self.state()["suspended"])
        self.assertEqual(self.trace(), ["stop:statusbar", "start:statusbar", "xrefresh:"])
        self.assertFalse((self.root / "lock").exists())

    def block_next_mode_read(self):
        self.env.update(TEST_MODE_BLOCK=str(self.root / "mode-block"),
                        TEST_MODE_ENTERED=str(self.root / "mode-entered"),
                        TEST_MODE_RELEASE=str(self.root / "mode-release"))
        self.native_getter('if [ -e "$TEST_MODE_BLOCK" ]; then\n'
                           '  : >"$TEST_MODE_ENTERED"\n'
                           '  while [ ! -e "$TEST_MODE_RELEASE" ]; do sleep 0.01; done\n'
                           'fi\n'
                           'cat "$TEST_NATIVE_MODE"')

    def test_sleep_queued_during_mode_read_invalidates_that_result(self):
        self.env["TEST_HOLD"] = "1"
        self.block_next_mode_read()

        def drive(_):
            self.ready()
            self.suspend()
            (self.root / "mode-block").touch()
            self.emit("outOfScreenSaver 1", "exitingScreenSaver 1")
            self.wait_until(lambda: (self.root / "mode-entered").exists())
            self.emit("goingToScreenSaver 2")
            (self.root / "mode-release").touch()
            self.wait_until(lambda: len(self.lines("native-queries")) >= 6)
            time.sleep(0.2)
            self.assertNotIn("resume", self.app_events())
            self.assertTrue(self.state()["suspended"])
            self.assertEqual(self.trace().count("stop:statusbar"), 1)
            self.assertFalse((self.root / "lock" / "resume-native-portrait-orientation").exists())
            (self.root / "mode-block").unlink()
            self.resume()
            self.app_action("quit")

        status, _, error = self.run_launcher(drive)
        self.assertEqual(status, 0, error)
        self.assertEqual(self.app_events().count("resume"), 1)
        self.assertEqual(self.trace(), ["stop:statusbar", "start:statusbar", "xrefresh:"] * 2)

    def test_usb_queued_during_mode_read_exits_without_reclaiming_native_control(self):
        self.env["TEST_HOLD"] = "1"
        self.block_next_mode_read()

        def drive(_):
            self.ready()
            self.suspend()
            (self.root / "mode-block").touch()
            self.emit("outOfScreenSaver 1", "exitingScreenSaver 1")
            self.wait_until(lambda: (self.root / "mode-entered").exists())
            self.emit("usbConfigured 1")
            (self.root / "mode-release").touch()

        status, _, error = self.run_launcher(drive)
        self.assertEqual(status, 143, error)
        self.assertNotIn("resume", self.app_events())
        self.assertEqual(self.trace(), ["stop:statusbar", "start:statusbar", "xrefresh:"])

    def test_a_full_event_fifo_does_not_deadlock_the_native_mode_ready_notice(self):
        import fcntl
        self.env["TEST_HOLD"] = "1"
        self.block_next_mode_read()

        def drive(_):
            self.ready()
            self.suspend()
            (self.root / "mode-block").touch()
            self.emit("outOfScreenSaver 1", "exitingScreenSaver 1")
            self.wait_until(lambda: (self.root / "mode-entered").exists())
            fifo = self.root / "lock" / "power-events"
            descriptor = os.open(fifo, os.O_RDWR | os.O_NONBLOCK)
            try:
                fcntl.fcntl(descriptor, fcntl.F_SETPIPE_SZ, 4096)
            finally:
                os.close(descriptor)
            flood = subprocess.Popen([
                sys.executable, "-c",
                # Each record must remain below PIPE_BUF. One giant write can
                # interleave the notice into the middle of a synthetic line.
                "import os, sys\nfd = os.open(sys.argv[1], os.O_WRONLY)\n"
                "record = b'wakeupFromSuspend 0\\n'\n"
                "for _ in range(6000):\n    assert os.write(fd, record) == len(record)\n"
                "os.close(fd)\n",
                str(fifo),
            ], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            self.addCleanup(self.stop_fixture_process, flood)
            time.sleep(0.05)
            self.assertIsNone(flood.poll())
            (self.root / "mode-release").touch()
            self.wait_until(lambda: "resume" in self.app_events())
            self.assertEqual(flood.wait(timeout=3), 0)
            self.app_action("quit")

        status, _, error = self.run_launcher(drive)
        self.assertEqual(status, 0, error)
        self.assertEqual(self.app_events().count("resume"), 1)

    def test_native_source_split_and_bulk_wake_records_keep_the_reader_session(self):
        self.env.update(TEST_HOLD="1", TEST_SOURCE_SLEEP=str(self.root / "source-sleep"),
                        TEST_SOURCE_WAKE=str(self.root / "source-wake"))
        self.python_observer(
            'while not Path(env["TEST_SOURCE_SLEEP"]).exists(): time.sleep(0.01)\n'
            'for byte in b"goingToScreenSaver 2\\n":\n'
            '    os.write(1, bytes([byte]))\n'
            '    time.sleep(0.001)\n'
            'while not Path(env["TEST_SOURCE_WAKE"]).exists(): time.sleep(0.01)\n'
            'os.write(1, b"outOfScreenSaver 1\\nexitingScreenSaver 1\\n")\n'
            'while True: time.sleep(60)\n'
        )

        def drive(_):
            self.ready()
            original = self.state()["pid"]
            (self.root / "source-sleep").touch()
            self.wait_until(lambda: self.trace().count("xrefresh:") == 1)
            self.assertTrue(self.state()["suspended"])
            (self.root / "source-wake").touch()
            self.wait_until(lambda: "resume" in self.app_events())
            self.assertEqual(self.state()["pid"], original)
            self.app_action("quit")

        status, _, error = self.run_launcher(drive)
        self.assertEqual(status, 0, error)
        self.assertEqual(self.app_events().count("suspend"), 1)
        self.assertEqual(self.app_events().count("resume"), 1)

    def test_native_source_bulk_output_does_not_splice_an_application_exit(self):
        self.env["TEST_HOLD"] = "1"
        self.gate_observer(b"wakeupFromSuspend 0\n" * 6000)

        def drive(_):
            self.ready()
            (self.root / "source-gate").touch()
            self.wait_until(lambda: (self.root / "source-done").exists())
            self.app_action("quit")

        status, _, error = self.run_launcher(drive)
        self.assertEqual(status, 0, error)
        self.assertNotIn("suspend", self.app_events())
        self.assertNotIn("resume", self.app_events())
        self.assertFalse((self.root / "lock").exists())

    def assert_invalid_native_source(self, payload):
        self.env["TEST_HOLD"] = "1"
        self.gate_observer(payload, hold=False)

        def drive(_):
            self.ready()
            (self.root / "source-gate").touch()

        status, _, error = self.run_launcher(drive)
        self.assertEqual(status, 1, error)
        self.assertIn("observation ended unexpectedly", error)
        self.assertNotIn("suspend", self.app_events())
        self.assertNotIn("resume", self.app_events())
        self.assertFalse((self.root / "lock").exists())

    def test_native_source_partial_eof_is_not_inferred_as_a_sleep(self):
        self.assert_invalid_native_source(b"goingToScreenSaver 2")

    def test_native_source_unknown_event_stops_without_a_guessed_power_state(self):
        self.assert_invalid_native_source(b"goingToScreenSaverSuffix 2\n")

    def test_native_source_oversized_record_stops_before_forwarding_a_prefix(self):
        self.assert_invalid_native_source(b"goingToScreenSaver " + b"1" * 256 + b"\n")

    def prepare_source_tail_during_snapshot(self, tail):
        self.env["TEST_HOLD"] = "1"
        self.block_next_mode_read()
        # This complete bulk write fits in the private source FIFO while the
        # shared FIFO is full. Its tail is accepted before the getter resumes.
        self.gate_observer(b"wakeupFromSuspend 0\n" * 50 + tail)

    def queue_source_tail_during_snapshot(self):
        self.ready()
        self.suspend()
        (self.root / "mode-block").touch()
        self.emit("outOfScreenSaver 1", "exitingScreenSaver 1")
        self.wait_until(lambda: (self.root / "mode-entered").exists())
        self.fill_event_fifo()
        (self.root / "source-gate").touch()
        self.wait_until(lambda: (self.root / "source-done").exists())

    def test_full_fifo_native_sleep_tail_precedes_the_old_snapshot_barrier(self):
        self.prepare_source_tail_during_snapshot(b"goingToScreenSaver 2\n")

        def drive(_):
            self.queue_source_tail_during_snapshot()
            (self.root / "mode-release").touch()
            self.wait_until(lambda: self.lines("supervisor-barriers") == ["processed"])
            self.assertNotIn("resume", self.app_events())
            self.assertTrue(self.state()["suspended"])
            self.assertEqual(self.trace().count("stop:statusbar"), 1)
            self.assertFalse((self.root / "lock" / "resume-native-portrait-orientation").exists())
            self.app_action("quit")

        status, _, error = self.run_launcher(drive)
        self.assertEqual(status, 0, error)
        self.assertNotIn("resume", self.app_events())

    def test_full_fifo_native_usb_tail_prevents_the_old_snapshot_takeover(self):
        self.prepare_source_tail_during_snapshot(b"usbConfigured 1\n")

        def drive(_):
            self.queue_source_tail_during_snapshot()
            (self.root / "mode-release").touch()

        status, _, error = self.run_launcher(drive)
        self.assertEqual(status, 143, error)
        self.assertNotIn("resume", self.app_events())
        self.assertEqual(self.trace().count("stop:statusbar"), 1)

    def test_full_fifo_shutdown_reaps_the_blocked_source_and_relay_group(self):
        self.prepare_source_tail_during_snapshot(b"wakeupFromSuspend 0\n")
        stopped_at = []

        def drive(child):
            self.queue_source_tail_during_snapshot()
            stopped_at.append(time.monotonic())
            child.send_signal(signal.SIGTERM)

        status, _, error = self.run_launcher(drive)
        self.assertEqual(status, 143, error)
        self.assertLess(time.monotonic() - stopped_at[0], 12)
        self.assertNotIn("resume", self.app_events())
        self.assertFalse((self.root / "lock").exists())

    def test_display_mode_is_read_before_each_awesome_pause(self):
        self.env["TEST_HOLD"] = "1"
        window_manager = subprocess.Popen(["sleep", "60"])
        self.addCleanup(self.stop_fixture_process, window_manager)
        self.script(self.bin / "pidof", f"printf '%s\\n' '{window_manager.pid}'")
        self.native_getter(f'case "$(sed -n \'s/^State:[[:space:]]*//p\' /proc/{window_manager.pid}/status)" in\n'
                           '  T*|t*) exit 97 ;;\n'
                           'esac\ncat "$TEST_NATIVE_MODE"')

        def drive(_):
            self.ready()
            self.wait_until(lambda: self.process_state(window_manager.pid).startswith("T"))
            self.suspend()
            self.resume()
            self.wait_until(lambda: self.process_state(window_manager.pid).startswith("T"))
            self.app_action("quit")

        status, _, error = self.run_launcher(drive)
        self.assertEqual(status, 0, error)
        self.assertEqual(self.lines("native-queries").count("-s com.lab126.winmgr epdcMode"), 2)
        self.assertFalse(self.process_state(window_manager.pid).startswith("T"))

    def test_sleep_wake_preserves_same_process_and_reader_ram(self):
        self.env["TEST_HOLD"] = "1"

        def drive(_):
            self.ready()
            original = self.state()
            self.suspend()
            self.assertEqual(self.state()["pid"], original["pid"])
            self.assertTrue(self.state()["suspended"])
            self.app_action("next")
            self.wait_until(lambda: "ignored-page" in self.app_events())
            self.assertEqual(self.state()["page"], 8)
            self.resume()
            resumed = self.state()
            for key in ("pid", "comic", "episode", "revision", "page", "route", "startup_preference"):
                self.assertEqual(resumed[key], original[key])
            self.app_action("next")
            self.wait_until(lambda: self.state().get("page") == 9)
            self.app_action("quit")

        status, _, error = self.run_launcher(drive)
        self.assertEqual(status, 0, error)
        self.assertEqual(len(self.starts()), 1)
        self.assertEqual(self.trace(), ["stop:statusbar", "start:statusbar", "xrefresh:"] * 2)
        self.assertFalse((self.root / "lock").exists())

    def test_wake_arriving_while_suspend_ack_is_pending_is_not_lost(self):
        self.env.update(TEST_HOLD="1", TEST_ACK_DELAY="0.3")

        def drive(_):
            self.ready()
            self.emit("goingToScreenSaver 2", "outOfScreenSaver 1", "exitingScreenSaver 1")
            self.wait_until(lambda: "resume" in self.app_events())
            self.app_action("quit")

        status, _, error = self.run_launcher(drive)
        self.assertEqual(status, 0, error)
        self.assertEqual(len(self.starts()), 1)
        self.assertEqual(self.app_events().count("suspend"), 1)
        self.assertEqual(self.app_events().count("resume"), 1)

    def test_early_boot_waits_for_signal_handlers_and_keeps_same_process(self):
        self.env.update(TEST_HOLD="1", TEST_READY_DELAY="0.5", TEST_BOOTING="1")

        def drive(_):
            self.wait_until(lambda: len(self.starts()) == 1 and "pid" in self.state())
            pid = self.state()["pid"]
            self.emit("goingToScreenSaver 2")
            self.wait_until(lambda: "xrefresh:" in self.trace())
            self.assertEqual(self.state()["pid"], pid)
            self.assertTrue(self.state()["booting"])
            self.resume()
            self.assertEqual(self.state()["pid"], pid)
            (self.root / "boot-release").touch()
            self.wait_until(lambda: not self.state().get("booting", True))
            self.app_action("quit")

        status, _, error = self.run_launcher(drive)
        self.assertEqual(status, 0, error)
        self.assertEqual(len(self.starts()), 1)

    def test_duplicate_sleep_wake_events_keep_one_process(self):
        self.env["TEST_HOLD"] = "1"

        def drive(_):
            self.ready()
            for count in (1, 2):
                self.emit("goingToScreenSaver 2", "goingToScreenSaver 2")
                self.wait_until(lambda: self.trace().count("xrefresh:") == count)
                self.emit("outOfScreenSaver 1", "outOfScreenSaver 1", "exitingScreenSaver 1", "exitingScreenSaver 1")
                self.wait_until(lambda: self.app_events().count("resume") == count)
            self.app_action("quit")

        status, _, error = self.run_launcher(drive)
        self.assertEqual(status, 0, error)
        self.assertEqual(len(self.starts()), 1)
        self.assertEqual(self.app_events().count("suspend"), 2)
        self.assertEqual(self.trace(), ["stop:statusbar", "start:statusbar", "xrefresh:"] * 3)

    def test_sleep_during_resume_waits_for_the_in_flight_request(self):
        self.env.update(TEST_HOLD="1", TEST_ACK_DELAY="0.3")

        def drive(_):
            self.ready()
            self.suspend()
            self.resume()
            self.emit("goingToScreenSaver 2")
            self.wait_until(lambda: self.app_events().count("suspend") == 2)
            self.wait_until(lambda: self.trace().count("xrefresh:") == 2)
            self.resume()
            self.app_action("quit")

        status, _, error = self.run_launcher(drive)
        self.assertEqual(status, 0, error)
        self.assertEqual(len(self.starts()), 1)
        self.assertEqual(self.app_events().count("suspend"), 2)
        self.assertEqual(self.app_events().count("resume"), 2)

    def test_second_sleep_and_wake_during_resume_ack_are_both_preserved(self):
        self.env.update(TEST_HOLD="1", TEST_ACK_DELAY="0.3")

        def drive(_):
            self.ready()
            self.suspend()
            self.resume()
            self.emit("goingToScreenSaver 2", "outOfScreenSaver 1", "exitingScreenSaver 1")
            self.wait_until(lambda: self.app_events().count("resume") == 2)
            self.assertEqual(self.app_events().count("suspend"), 2)
            self.assertFalse(self.state()["suspended"])
            self.app_action("quit")

        status, _, error = self.run_launcher(drive)
        self.assertEqual(status, 0, error)
        self.assertEqual(len(self.starts()), 1)

    def test_timer_wake_and_unlock_in_progress_do_not_reclaim_input(self):
        self.env["TEST_HOLD"] = "1"

        def drive(_):
            self.ready()
            (self.root / "power-state").write_text("screenSaver\n")
            self.suspend()
            self.emit("wakeupFromSuspend 0", "exitingScreenSaver 1")
            time.sleep(0.15)
            self.assertNotIn("resume", self.app_events())
            self.emit("outOfScreenSaver 1")
            time.sleep(2.2)
            self.assertNotIn("resume", self.app_events())
            self.assertEqual(self.trace().count("stop:statusbar"), 1)
            (self.root / "power-state").write_text("active\n")
            self.wait_until(lambda: "resume" in self.app_events())
            self.app_action("quit")

        status, _, error = self.run_launcher(drive)
        self.assertEqual(status, 0, error)
        self.assertEqual(len(self.starts()), 1)

    def test_out_event_can_use_explicit_active_state_when_exit_event_is_missing(self):
        self.env.update(TEST_HOLD="1", BILICOMICS_LAUNCH_TRACE="1")

        def drive(_):
            self.ready()
            self.suspend()
            self.emit("outOfScreenSaver 1")
            self.wait_until(lambda: "resume" in self.app_events())
            timeline = self.timing()
            wake = next(record for record in timeline if record["event"] == "wake_out")
            sample = next(record for record in timeline if record["event"] == "wake_sample_begin")
            self.assertGreaterEqual(sample["uptime"] - wake["uptime"], 2)
            self.app_action("quit")

        status, _, error = self.run_launcher(drive)
        self.assertEqual(status, 0, error)
        self.assertEqual(len(self.starts()), 1)

    def test_out_only_has_no_synchronous_precheck_or_take_before_actual_dismissal(self):
        self.env.update(TEST_HOLD="1", BILICOMICS_LAUNCH_TRACE="1")

        def drive(_):
            self.ready()
            self.suspend()
            self.emit("outOfScreenSaver 1")
            self.wait_until(lambda: any(row["event"] == "wake_probe_normal" for row in self.timing()))
            self.assertTrue(self.state()["suspended"])
            self.assertNotIn("resume", self.app_events())
            self.assertEqual(self.lines("native-queries").count("-s com.lab126.winmgr epdcMode"), 1)
            self.assertEqual(self.trace().count("stop:statusbar"), 1)
            self.assertEqual(len(self.lines("native-queries")), 3)
            self.emit("exitingScreenSaver 1")
            self.wait_until(lambda: "resume" in self.app_events())
            timeline = self.timing()
            wake = next(record for record in timeline if record["event"] == "wake_exit")
            sample = next(record for record in timeline if record["event"] == "wake_sample_begin")
            self.assertLess(sample["uptime"] - wake["uptime"], 1.5)
            self.assertNotIn("wake_probe_immediate", [record["event"] for record in timeline])
            self.app_action("quit")

        status, _, error = self.run_launcher(drive)
        self.assertEqual(status, 0, error)
        self.assertEqual(self.lines("native-queries").count("-s com.lab126.winmgr epdcMode"), 2)

    def test_exit_before_out_is_retained_once_without_a_wake_intent_from_rtc(self):
        self.env.update(TEST_HOLD="1", BILICOMICS_LAUNCH_TRACE="1")

        def drive(_):
            self.ready()
            self.suspend()
            self.emit("exitingScreenSaver 1", "exitingScreenSaver 1", "wakeupFromSuspend 0")
            self.wait_until(lambda: len([row for row in self.timing() if row["event"] == "wake_exit"]) == 2)
            self.assertNotIn("resume", self.app_events())
            self.assertEqual(len(self.lines("native-queries")), 3)
            self.emit("outOfScreenSaver 1", "outOfScreenSaver 1", "exitingScreenSaver 1")
            self.wait_until(lambda: "resume" in self.app_events())
            self.app_action("quit")

        status, _, error = self.run_launcher(drive)
        self.assertEqual(status, 0, error)
        self.assertEqual(self.app_events().count("resume"), 1)
        self.assertEqual(len(self.lines("native-queries")), 6)

    def test_snapshot_three_reads_share_one_proven_group_per_capture(self):
        self.env.update(TEST_HOLD="1", TEST_CAPTURE_GROUPS=str(self.root / "capture-groups"))

        def drive(_):
            self.ready()
            self.suspend()
            self.emit("exitingScreenSaver 1", "outOfScreenSaver 1")
            self.wait_until(lambda: "resume" in self.app_events())
            self.app_action("quit")

        status, _, error = self.run_launcher(drive)
        self.assertEqual(status, 0, error)
        groups = self.lines("capture-groups")
        self.assertEqual(len(groups), 6)
        self.assertTrue(all(group.split(":")[0] == group.split(":")[1] for group in groups))
        self.assertEqual(len(set(groups[:3])), 1)
        self.assertEqual(len(set(groups[3:])), 1)
        self.assertNotEqual(groups[0], groups[3])
        self.assertEqual(self.lines("native-queries"), [
            "-s com.lab126.powerd state", "-s com.lab126.winmgr epdcMode", "-s com.lab126.powerd state",
        ] * 2)

    def test_framebuffer_capability_uses_fresh_resume_probe_without_mode_queries(self):
        self.env.update(TEST_HOLD="1", TEST_OUTPUT_CAPABILITY="framebuffer8")

        def drive(_):
            self.ready()
            original = self.state()["pid"]
            self.suspend()
            baseline = self.lines("native-queries")
            (self.root / "native-mode").write_text("unsupported\n")
            (self.root / "framebuffer-mode").write_text("Y8INV\n")
            self.emit("exitingScreenSaver 1", "outOfScreenSaver 1")
            self.wait_until(lambda: "resume" in self.app_events())
            self.assertEqual(self.state()["pid"], original)
            self.assertEqual(self.state()["resume_presentation"], "FRAMEBUFFER")
            self.assertEqual(self.state()["native_mode"], "Y8INV")
            self.assertEqual(self.lines("native-queries"), baseline)
            request = self.request().split(":")
            self.assertEqual((self.root / "lock" / "resume-native-epdc-mode").read_text().strip(),
                             ":".join(request[:3] + ["FRAMEBUFFER"]))
            self.assertEqual((self.root / "lock" / "resume-native-portrait-orientation").read_text(),
                             ":".join(request[:3] + ["CALIBRATED"]) + "\n")
            self.app_action("quit")

        status, _, error = self.run_launcher(drive)
        self.assertEqual(status, 0, error)
        self.assertFalse((self.root / "lock").exists())

    def test_fast_wake_calibrates_each_fresh_half_turn_without_lipc_or_observation_helpers(self):
        self.env.update(TEST_HOLD="1", TEST_OUTPUT_CAPABILITY="framebuffer8")
        self.configure_portrait("U", 3)

        def drive(_):
            self.ready()
            original = self.state()
            calibration = (self.root / "lock" / "native-portrait-orientation").read_text()
            baseline_queries = self.lines("native-queries")
            baseline_helpers = self.lines("portrait-probes")
            sequences = []
            for rotation, orientation in ((1, "D"), (3, "U")):
                self.suspend()
                self.assertFalse((self.root / "lock" / "resume-native-portrait-orientation").exists())
                self.configure_portrait(orientation, rotation)
                self.emit("exitingScreenSaver 1", "outOfScreenSaver 1")
                self.wait_until(lambda: self.app_events().count("resume") == len(sequences) + 1)
                request = self.request().split(":")
                sequences.append(int(request[2]))
                self.assertEqual((self.root / "lock" / "resume-native-portrait-orientation").read_text(),
                                 ":".join(request[:3] + ["CALIBRATED"]) + "\n")
                self.assertEqual((self.root / "lock" / "native-portrait-orientation").read_text(), calibration)
                self.assertEqual(self.state()["portrait_orientation"], orientation)
                self.assertEqual(self.state()["native_rotation"], rotation)
                for key in ("pid", "comic", "episode", "revision", "page", "route"):
                    self.assertEqual(self.state()[key], original[key])
                self.assertEqual(self.lines("native-queries"), baseline_queries)
                self.assertEqual(self.lines("portrait-probes"), baseline_helpers)
            self.assertEqual(sequences, sorted(set(sequences)))
            self.app_action("quit")

        status, _, error = self.run_launcher(drive)
        self.assertEqual(status, 0, error)
        self.assertEqual(len(self.starts()), 1)
        self.assertEqual(self.trace(), ["stop:statusbar", "start:statusbar", "xrefresh:"] * 3)
        self.assertFalse((self.root / "lock").exists())

    def test_fast_wake_rejects_a_quarter_turn_instead_of_reusing_the_startup_direction(self):
        self.env.update(TEST_HOLD="1", TEST_OUTPUT_CAPABILITY="framebuffer8")
        self.configure_portrait("U", 3)

        def drive(_):
            self.ready()
            self.suspend()
            (self.root / "framebuffer-rotation").write_text("0\n")
            self.emit("exitingScreenSaver 1", "outOfScreenSaver 1")

        status, _, error = self.run_launcher(drive)
        self.assertEqual(status, 1, error)
        self.assertIn("did not acknowledge resume", error)
        self.assertIn("portrait-probe-failed", self.app_events())
        self.assertNotIn("resume", self.app_events())
        self.assertTrue(self.state()["suspended"])
        self.assertEqual(self.state()["portrait_orientation"], "U")
        self.assertFalse((self.root / "lock").exists())

    def test_inherited_sgid_private_launcher_directory_keeps_framebuffer_fast_path(self):
        self.env.update(TEST_HOLD="1", TEST_OUTPUT_CAPABILITY="framebuffer8")
        self.root.chmod(0o2700)

        def drive(_):
            self.ready()
            directory = self.root / "lock"
            self.assertEqual(directory.stat().st_mode & 0o7777, 0o2700)
            self.assertEqual(directory.stat().st_gid, self.root.stat().st_gid)
            self.assertEqual((directory / "native-output-capability").stat().st_mode & 0o7777, 0o600)
            self.suspend()
            queries = self.lines("native-queries")
            (self.root / "native-mode").write_text("unsupported\n")
            (self.root / "framebuffer-mode").write_text("Y8INV\n")
            self.emit("exitingScreenSaver 1", "outOfScreenSaver 1")
            self.wait_until(lambda: "resume" in self.app_events())
            self.assertEqual(self.state()["resume_presentation"], "FRAMEBUFFER")
            self.assertEqual(self.state()["native_mode"], "Y8INV")
            self.assertEqual(self.lines("native-queries"), queries)
            self.app_action("quit")

        status, _, error = self.run_launcher(drive)
        self.assertEqual(status, 0, error)
        self.assertFalse((self.root / "lock").exists())

    def test_sgid_launcher_directory_with_group_access_cannot_enable_fast_path(self):
        self.env.update(TEST_HOLD="1", TEST_OUTPUT_CAPABILITY="framebuffer8")
        self.root.chmod(0o2700)

        def drive(_):
            self.ready()
            directory = self.root / "lock"
            self.assertEqual(directory.stat().st_mode & 0o7777, 0o2700)
            directory.chmod(0o2750)
            self.assertEqual(directory.stat().st_mode & 0o7777, 0o2750)
            self.suspend()
            queries = self.lines("native-queries")
            self.emit("exitingScreenSaver 1", "outOfScreenSaver 1")
            self.wait_until(lambda: "resume" in self.app_events())
            self.assertIn(self.state()["resume_presentation"], ("Y8", "Y8INV"))
            self.assertEqual(self.lines("native-queries")[len(queries):], [
                "-s com.lab126.powerd state", "-s com.lab126.winmgr epdcMode", "-s com.lab126.powerd state",
            ])
            self.app_action("quit")

        status, _, error = self.run_launcher(drive)
        self.assertEqual(status, 0, error)
        self.assertFalse((self.root / "lock").exists())

    def test_framebuffer_takeover_pauses_awesome_before_stopping_statusbar(self):
        self.env.update(TEST_HOLD="1", TEST_OUTPUT_CAPABILITY="framebuffer8",
                        TEST_WM_FIRST=str(self.root / "wm-first"))
        manager = subprocess.Popen(["sleep", "60"])
        self.addCleanup(self.stop_fixture_process, manager)
        self.script(self.bin / "pidof", f"printf '%s\\n' '{manager.pid}'")
        self.script(self.bin / "stop",
                    'if [ -e "$TEST_WM_FIRST" ]; then\n'
                    f'  case "$(sed -n \'s/^State:[[:space:]]*//p\' /proc/{manager.pid}/status)" in T*|t*) ;; *) exit 72 ;; esac\n'
                    'fi\n'
                    'printf "stop:%s\\n" "$*" >>"$TEST_TRACE"\n'
                    'printf "stop/waiting\\n" >"$TEST_STATUSBAR_STATE"')

        def drive(_):
            self.ready()
            self.suspend()
            (self.root / "wm-first").touch()
            self.resume()
            self.assertEqual(self.state()["resume_presentation"], "FRAMEBUFFER")
            self.assertEqual(self.trace().count("stop:statusbar"), 2)
            self.app_action("quit")

        status, _, error = self.run_launcher(drive)
        self.assertEqual(status, 0, error)
        self.assertFalse(self.process_state(manager.pid).startswith("T"))

    def assert_capability_falls_back(self, capability, mutate=None):
        self.env.update(TEST_HOLD="1", TEST_OUTPUT_CAPABILITY=capability)

        def drive(_):
            self.ready()
            if mutate is not None:
                mutate()
            self.suspend()
            self.resume()
            self.assertIn(self.state()["resume_presentation"], ("Y8", "Y8INV"))
            self.assertEqual(self.lines("native-queries").count("-s com.lab126.winmgr epdcMode"), 2)
            self.app_action("quit")

        status, _, error = self.run_launcher(drive)
        self.assertEqual(status, 0, error)

    def test_missing_capability_preserves_classic_resume(self):
        self.assert_capability_falls_back("missing")

    def test_unsupported_capability_preserves_classic_resume(self):
        self.assert_capability_falls_back("unsupported")

    def test_capability_with_another_application_pid_cannot_enable_fast_resume(self):
        def mutate():
            directory = self.root / "lock"
            header = (directory / "generation").read_text().strip()
            (directory / "native-output-capability").write_text(header + ":99999:framebuffer8\n")
        self.assert_capability_falls_back("framebuffer8", mutate)

    def test_public_capability_cannot_enable_fast_resume(self):
        self.assert_capability_falls_back("framebuffer8", lambda: (self.root / "lock" / "native-output-capability").chmod(0o644))

    def test_changed_capability_after_suspend_falls_back_without_using_cached_permission(self):
        self.env.update(TEST_HOLD="1", TEST_OUTPUT_CAPABILITY="framebuffer8")

        def drive(_):
            self.ready()
            self.suspend()
            directory = self.root / "lock"
            header = (directory / "generation").read_text().strip()
            (directory / "native-output-capability").write_text(f"{header}:{self.state()['pid']}:legacy\n")
            self.resume()
            self.assertIn(self.state()["resume_presentation"], ("Y8", "Y8INV"))
            self.app_action("quit")

        status, _, error = self.run_launcher(drive)
        self.assertEqual(status, 0, error)

    def test_capability_changed_while_barrier_is_pending_restarts_classic_sampling(self):
        self.env.update(TEST_HOLD="1", TEST_OUTPUT_CAPABILITY="framebuffer8", BILICOMICS_LAUNCH_TRACE="1",
                        TEST_SOURCE_START=str(self.root / "source-start"),
                        TEST_SOURCE_RELEASE=str(self.root / "source-release"))
        self.python_observer(
            'while not Path(env["TEST_SOURCE_START"]).exists(): time.sleep(0.01)\n'
            'os.write(1, b"outOfScreenSaver 1\\nexitingScreenSaver 1\\nwakeupFromSuspend")\n'
            'while not Path(env["TEST_SOURCE_RELEASE"]).exists(): time.sleep(0.01)\n'
            'os.write(1, b" 0\\n")\n'
            'while True: time.sleep(60)\n'
        )

        def drive(_):
            self.ready()
            self.suspend()
            (self.root / "source-start").touch()
            self.wait_until(lambda: any(row["event"] == "wake_framebuffer_barrier" for row in self.timing()))
            self.assertNotIn("resume", self.app_events())
            self.assertEqual(self.trace().count("stop:statusbar"), 1)
            directory = self.root / "lock"
            header = (directory / "generation").read_text().strip()
            (directory / "native-output-capability").write_text(f"{header}:{self.state()['pid']}:legacy\n")
            (self.root / "source-release").touch()
            self.wait_until(lambda: "resume" in self.app_events())
            self.assertIn(self.state()["resume_presentation"], ("Y8", "Y8INV"))
            self.assertIn("wake_capability_fallback", {row["event"] for row in self.timing()})
            self.assertEqual(self.lines("native-queries").count("-s com.lab126.winmgr epdcMode"), 2)
            self.app_action("quit")

        status, _, error = self.run_launcher(drive)
        self.assertEqual(status, 0, error)

    def test_same_length_root_capability_edit_never_reaches_an_unvalidated_frame(self):
        self.env.update(TEST_HOLD="1", TEST_OUTPUT_CAPABILITY="framebuffer8")
        self.launcher.write_text(self.launcher.read_text().replace("waiter_remaining=8", "waiter_remaining=3"))

        def drive(_):
            self.ready()
            self.suspend()
            capability = self.root / "lock" / "native-output-capability"
            before = capability.stat()
            value = capability.read_text()
            capability.write_text(value.replace("framebuffer8", "framebuffer9"))
            os.utime(capability, ns=(before.st_atime_ns, before.st_mtime_ns))
            self.emit("exitingScreenSaver 1", "outOfScreenSaver 1")
            self.wait_until(lambda: "resume" in self.app_events() or "framebuffer-probe-failed" in self.app_events())
            if "resume" in self.app_events():
                self.assertIn(self.state()["resume_presentation"], ("Y8", "Y8INV"))
                self.app_action("quit")

        status, _, error = self.run_launcher(drive)
        if status == 0:
            self.assertIn(self.state()["resume_presentation"], ("Y8", "Y8INV"))
        else:
            self.assertEqual(status, 1, error)
            self.assertIn("framebuffer-probe-failed", self.app_events())
            self.assertNotIn("resume", self.app_events())
            self.assertTrue(self.state()["suspended"])
        self.assertFalse((self.root / "lock").exists())

    def assert_framebuffer_tail_cancels(self, tail, expected_status):
        self.env.update(TEST_HOLD="1", TEST_OUTPUT_CAPABILITY="framebuffer8", BILICOMICS_LAUNCH_TRACE="1")
        self.gate_observer(b"outOfScreenSaver 1\nexitingScreenSaver 1\n" + b"wakeupFromSuspend 0\n" * 50 + tail)

        def drive(_):
            self.ready()
            self.suspend()
            (self.root / "source-gate").touch()
            if expected_status == 0:
                self.wait_until(lambda: any(row["event"] == "wake_barrier_dropped" for row in self.timing()))
                self.assertNotIn("resume", self.app_events())
                self.assertTrue(self.state()["suspended"])
                self.app_action("quit")

        status, _, error = self.run_launcher(drive)
        self.assertEqual(status, expected_status, error)
        self.assertNotIn("resume", self.app_events())
        self.assertEqual(self.lines("native-queries").count("-s com.lab126.winmgr epdcMode"), 1)
        self.assertEqual(self.trace().count("stop:statusbar"), 1)

    def test_framebuffer_barrier_discards_a_new_sleep_before_takeover(self):
        self.assert_framebuffer_tail_cancels(b"goingToScreenSaver 2\n", 0)

    def test_framebuffer_barrier_discards_usb_before_takeover(self):
        self.assert_framebuffer_tail_cancels(b"usbConfigured 1\n", 143)

    def test_framebuffer_probe_failure_is_not_acknowledged_and_restores_native(self):
        self.env.update(TEST_HOLD="1", TEST_OUTPUT_CAPABILITY="framebuffer8")
        self.launcher.write_text(self.launcher.read_text().replace("waiter_remaining=8", "waiter_remaining=3"))

        def drive(_):
            self.ready()
            self.suspend()
            (self.root / "framebuffer-mode").write_text("unsupported\n")
            self.emit("exitingScreenSaver 1", "outOfScreenSaver 1")

        status, _, error = self.run_launcher(drive)
        self.assertEqual(status, 1, error)
        self.assertIn("did not acknowledge resume", error)
        self.assertIn("framebuffer-probe-failed", self.app_events())
        self.assertNotIn("resume", self.app_events())
        self.assertTrue(self.state()["suspended"])
        self.assertFalse((self.root / "lock").exists())

    def test_wake_before_suspend_ack_does_not_query_the_stopped_window_manager(self):
        self.env.update(TEST_HOLD="1", TEST_ACK_DELAY="0.4", TEST_ASSERT_RELEASE=str(self.root / "assert-release"))
        guard = ('if [ -e "$TEST_ASSERT_RELEASE" ] && '
                 '[ "$(cat "$TEST_STATUSBAR_STATE")" = "stop/waiting" ]; then exit 71; fi\n')
        self.native_getter(guard + 'cat "$TEST_NATIVE_MODE"', guard + 'cat "$TEST_POWER_STATE"')

        def drive(_):
            self.ready()
            (self.root / "assert-release").touch()
            self.emit("goingToScreenSaver 2", "exitingScreenSaver 1", "outOfScreenSaver 1")
            self.wait_until(lambda: "resume" in self.app_events())
            self.app_action("quit")

        status, _, error = self.run_launcher(drive)
        self.assertEqual(status, 0, error)
        self.assertEqual(len(self.lines("native-queries")), 6)

    def test_suspend_ack_release_does_not_bypass_the_missing_dismissal_delay(self):
        self.env.update(TEST_HOLD="1", TEST_ACK_DELAY="0.4", BILICOMICS_LAUNCH_TRACE="1")

        def drive(_):
            self.ready()
            self.emit("goingToScreenSaver 2", "outOfScreenSaver 1")
            self.wait_until(lambda: any(row["event"] == "wake_probe_normal" for row in self.timing()))
            self.assertIn("lifecycle_ack_suspend", {row["event"] for row in self.timing()})
            self.assertTrue(self.state()["suspended"])
            self.assertNotIn("resume", self.app_events())
            self.assertEqual(self.lines("native-queries").count("-s com.lab126.winmgr epdcMode"), 1)
            self.assertEqual(self.trace().count("stop:statusbar"), 1)
            self.emit("usbConfigured 1")

        status, _, error = self.run_launcher(drive)
        self.assertEqual(status, 143, error)
        self.assertNotIn("resume", self.app_events())

    def test_not_active_before_snapshot_keeps_native_unlock_and_skips_epdc(self):
        self.env.update(TEST_HOLD="1", BILICOMICS_LAUNCH_TRACE="1")

        def drive(_):
            self.ready()
            original = self.state()["pid"]
            self.suspend()
            (self.root / "power-state").write_text("screenSaver\n")
            self.emit("outOfScreenSaver 1", "exitingScreenSaver 1")
            self.wait_until(lambda: any(row["event"] == "wake_sample_inactive" for row in self.timing()))
            self.assertTrue(self.state()["suspended"])
            self.assertEqual(self.state()["pid"], original)
            self.assertNotIn("resume", self.app_events())
            self.assertEqual(self.lines("native-queries").count("-s com.lab126.winmgr epdcMode"), 1)
            self.assertEqual(self.trace().count("stop:statusbar"), 1)
            (self.root / "power-state").write_text("active\n")
            self.emit("exitingScreenSaver 1")
            self.wait_until(lambda: "resume" in self.app_events())
            self.app_action("quit")

        status, _, error = self.run_launcher(drive)
        self.assertEqual(status, 0, error)
        self.assertEqual(self.app_events().count("resume"), 1)

    def test_not_active_after_mode_discards_snapshot_and_retries_without_exiting(self):
        self.env.update(TEST_HOLD="1", BILICOMICS_LAUNCH_TRACE="1", TEST_CHANGE_POWER=str(self.root / "change-power"))
        self.native_getter('cat "$TEST_NATIVE_MODE"\n'
                           'if [ -e "$TEST_CHANGE_POWER" ]; then printf "screenSaver\\n" >"$TEST_POWER_STATE"; fi')

        def drive(_):
            self.ready()
            original = self.state()["pid"]
            self.suspend()
            (self.root / "change-power").touch()
            self.emit("outOfScreenSaver 1", "exitingScreenSaver 1")
            self.wait_until(lambda: any(row["event"] == "wake_sample_inactive" for row in self.timing()))
            self.assertEqual(self.state()["pid"], original)
            self.assertTrue(self.state()["suspended"])
            self.assertNotIn("resume", self.app_events())
            self.assertFalse((self.root / "lock" / "resume-native-epdc-mode").exists())
            self.assertEqual(self.trace().count("stop:statusbar"), 1)
            (self.root / "change-power").unlink()
            (self.root / "power-state").write_text("active\n")
            self.wait_until(lambda: "resume" in self.app_events())
            self.app_action("quit")

        status, _, error = self.run_launcher(drive)
        self.assertEqual(status, 0, error)

    def test_old_probe_attempt_does_not_wait_for_the_new_backoff_timer(self):
        self.env.update(TEST_HOLD="1", BILICOMICS_LAUNCH_TRACE="1")
        stopped_at = []

        def drive(_):
            self.ready()
            self.suspend()
            (self.root / "power-state").write_text("screenSaver\n")
            self.emit("outOfScreenSaver 1")
            self.wait_until(lambda: any(row["event"] == "wake_probe_normal" for row in self.timing()))
            current = next(row for row in reversed(self.timing()) if row["event"] == "wake_probe_normal")
            token = (self.root / "lock" / "nonce").read_text().strip()
            self.emit(f"_bilicomics_wake_check {token} {current['sleep']} {int(current['probe']) - 1}")
            self.wait_until(lambda: any(row["event"] == "wake_probe_stale" for row in self.timing()))
            stopped_at.append(time.monotonic())
            self.emit("usbConfigured 1")

        status, _, error = self.run_launcher(drive)
        self.assertEqual(status, 143, error)
        self.assertLess(time.monotonic() - stopped_at[0], 2)
        self.assertNotIn("resume", self.app_events())

    def test_probe_sleep_ignoring_term_is_killed_and_reaped_before_its_wrapper(self):
        self.env.update(TEST_HOLD="1", BILICOMICS_LAUNCH_TRACE="1",
                        TEST_PROBE_IGNORE=str(self.root / "probe-ignore"),
                        TEST_PROBE_SLEEP_PID=str(self.root / "probe-sleep-pid"),
                        TEST_LAUNCH_LOCK=str(self.root / "lock"))
        self.script(self.bin / "sleep",
                    'duration=$1\n'
                    'if [ "$duration" = 2 ] && [ -e "$TEST_PROBE_IGNORE" ]; then\n'
                    '  IFS= read -r entry </proc/self/stat\n'
                    '  tail=${entry##*) }; set -- $tail\n'
                    '  if [ "$3" = "$(cat "$TEST_LAUNCH_LOCK/pid")" ]; then\n'
                    '    printf "%s\\n" "$$" >"$TEST_PROBE_SLEEP_PID"\n'
                    '    trap "" INT TERM\n'
                    '    exec /bin/sleep 60\n'
                    '  fi\n'
                    'fi\n'
                    'exec /bin/sleep "$duration"')
        captured = []

        def cleanup_sleep():
            for identity, executable in captured:
                try:
                    current = self.process_identity(identity["pid"])
                    if (current["birth"] == identity["birth"]
                            and os.readlink(f"/proc/{identity['pid']}/exe") == executable):
                        os.kill(identity["pid"], signal.SIGKILL)
                except (FileNotFoundError, ProcessLookupError):
                    pass

        self.addCleanup(cleanup_sleep)

        def drive(child):
            self.ready()
            self.suspend()
            (self.root / "probe-ignore").touch()
            self.emit("outOfScreenSaver 1")
            self.wait_until(lambda: (self.root / "probe-sleep-pid").exists())
            pid = int((self.root / "probe-sleep-pid").read_text())
            expected_executable = str(Path("/bin/sleep").resolve())
            self.wait_until(lambda: os.readlink(f"/proc/{pid}/exe") == expected_executable)
            identity = self.process_identity(pid)
            self.assertEqual(self.process_identity(identity["parent"])["parent"], child.pid)
            captured.append((identity, os.readlink(f"/proc/{pid}/exe")))
            self.emit("usbConfigured 1")

        status, _, error = self.run_launcher(drive)
        self.assertEqual(status, 143, error)
        for identity, _ in captured:
            try:
                self.assertIn(self.process_identity(identity["pid"])["state"], ("Z", "X"))
            except FileNotFoundError:
                pass
        self.assertFalse((self.root / "lock").exists())

    def test_snapshot_deadline_covers_all_three_getters_as_one_transaction(self):
        self.native_getter('sleep 0.9\ncat "$TEST_NATIVE_MODE"', 'sleep 0.9\ncat "$TEST_POWER_STATE"')
        started = time.monotonic()
        status, _, error = self.run_launcher()
        self.assertEqual(status, 1, error)
        self.assertIn("could not be read before startup", error)
        self.assertLess(time.monotonic() - started, 5)
        self.assertEqual(self.starts(), [])
        self.assertEqual(self.trace(), [])

    def test_malformed_power_output_does_not_become_a_successful_active_probe(self):
        self.native_getter(power_body='printf "active\\nextra\\n"')
        status, _, error = self.run_launcher()
        self.assertEqual(status, 1, error)
        self.assertIn("could not be read before startup", error)
        self.assertEqual(self.lines("native-queries"), ["-s com.lab126.powerd state"])
        self.assertEqual(self.trace(), [])

    def test_default_trace_is_absent_when_the_option_is_unset(self):
        self.env.pop("BILICOMICS_LAUNCH_TRACE")
        status, _, error = self.run_launcher()
        self.assertEqual(status, 0, error)
        self.assertFalse((self.home / "data" / "logs" / "launcher-timing.tsv").exists())

    def test_enabled_trace_contains_only_fixed_stages_and_monotonic_metadata(self):
        self.env.update(TEST_HOLD="1", BILICOMICS_LAUNCH_TRACE="1")
        tokens = []

        def drive(_):
            self.ready()
            tokens.append((self.root / "lock" / "nonce").read_text().strip())
            self.suspend()
            self.resume()
            self.wait_until(lambda: any(row["event"] == "signal_resume" for row in self.timing()))
            self.app_action("quit")

        status, _, error = self.run_launcher(drive)
        self.assertEqual(status, 0, error)
        text = (self.home / "data" / "logs" / "launcher-timing.tsv").read_text()
        self.assertNotIn(tokens[0], text)
        self.assertNotIn("goingToScreenSaver", text)
        self.assertNotIn("_bilicomics_", text)
        self.assertNotIn("comic", text)
        timeline = self.timing()
        events = {row["event"] for row in timeline}
        self.assertTrue({"startup_sample_begin", "startup_sample_ready", "sleep_event", "wake_out",
                         "wake_sample_begin", "wake_sample_ready", "wake_barrier_accepted", "native_take_done",
                         "signal_resume", "lifecycle_ack_suspend"}.issubset(events))
        for row in timeline:
            self.assertEqual(set(row), {"uptime", "event", "pid", "phase", "sleep", "capture", "probe", "lifecycle", "mode"})
            self.assertGreaterEqual(row["uptime"], 0)
            self.assertIn(row["mode"], ("Y8", "Y8INV", "unknown"))
            self.assertTrue(row["event"].replace("_", "").isalpha())

    def test_optional_trace_write_failure_does_not_break_native_restoration(self):
        self.env["BILICOMICS_LAUNCH_TRACE"] = "1"
        (self.home / "data" / "logs" / "launcher-timing.tsv").mkdir(parents=True)
        status, _, error = self.run_launcher()
        self.assertEqual(status, 0, error)
        self.assertEqual(self.trace(), ["stop:statusbar", "start:statusbar", "xrefresh:"])
        self.assertFalse((self.root / "lock").exists())

    def test_new_sleep_cancels_the_previous_wake_intent_and_probe(self):
        self.env["TEST_HOLD"] = "1"

        def drive(_):
            self.ready()
            self.suspend()
            token = (self.root / "lock" / "nonce").read_text().strip()
            self.emit("outOfScreenSaver 1", "goingToScreenSaver 2", "exitingScreenSaver 1")
            self.emit(f"_bilicomics_wake_check {token} 1 1")
            time.sleep(0.15)
            self.assertNotIn("resume", self.app_events())
            self.resume()
            self.app_action("quit")

        status, _, error = self.run_launcher(drive)
        self.assertEqual(status, 0, error)
        self.assertEqual(len(self.starts()), 1)

    def test_stale_acknowledgements_are_ignored_until_every_field_matches(self):
        self.env.update(TEST_HOLD="1", TEST_ACK_MODE="manual")

        def drive(_):
            self.ready()
            self.emit("goingToScreenSaver 2")
            self.wait_until(lambda: "suspend" in self.app_events())
            fields = self.request().split(":")
            for index, value in ((0, "9"), (1, "0" * 32), (2, "0"), (3, "resume")):
                stale = fields.copy()
                stale[index] = value
                self.acknowledge(":".join(stale) + ":ok")
                time.sleep(0.15)
                self.assertNotIn("xrefresh:", self.trace())
            self.acknowledge()
            self.wait_until(lambda: "xrefresh:" in self.trace())
            self.resume()
            self.acknowledge()
            self.app_action("quit")

        status, _, error = self.run_launcher(drive)
        self.assertEqual(status, 0, error)
        self.assertEqual(len(self.starts()), 1)

    def test_checkpoint_error_keeps_ram_session_and_releases_display(self):
        self.env.update(TEST_HOLD="1", TEST_ACK_MODE="checkpoint-error")

        def drive(_):
            self.ready()
            pid = self.state()["pid"]
            self.suspend()
            self.resume()
            self.assertEqual(self.state()["pid"], pid)
            self.assertEqual(self.state()["page"], 8)
            self.app_action("quit")

        status, _, error = self.run_launcher(drive)
        self.assertEqual(status, 0, error)
        self.assertIn("checkpoint could not be saved", error)
        self.assertIn("remains in memory", error)
        self.assertEqual(len(self.starts()), 1)

    def test_legacy_bare_suspend_acknowledgement_keeps_portrait_resume_permission_current(self):
        self.env.update(TEST_HOLD="1", TEST_ACK_MODE="legacy")

        def drive(_):
            self.ready()
            original = self.state()
            self.suspend()
            prior = self.request()
            self.assertEqual((self.root / "lock" / "lifecycle-ack").read_text(), prior + "\n")
            self.resume()
            current = self.request().split(":")
            self.assertEqual((self.root / "lock" / "resume-native-portrait-orientation").read_text(),
                             ":".join(current[:3] + ["CALIBRATED"]) + "\n")
            for key in ("pid", "comic", "episode", "revision", "page", "route", "portrait_orientation"):
                self.assertEqual(self.state()[key], original[key])
            self.app_action("quit")

        status, _, error = self.run_launcher(drive)
        self.assertEqual(status, 0, error)
        self.assertEqual(len(self.starts()), 1)
        self.assertFalse((self.root / "lock").exists())

    def test_missing_ack_has_bounded_cleanup_and_restores_services(self):
        self.env.update(TEST_HOLD="1", TEST_ACK_MODE="none")
        self.launcher.write_text(self.launcher.read_text().replace("waiter_remaining=8", "waiter_remaining=3"))

        def drive(_):
            self.ready()
            self.emit("goingToScreenSaver 2")

        started = time.monotonic()
        status, _, error = self.run_launcher(drive)
        self.assertEqual(status, 1, error)
        self.assertIn("did not acknowledge suspend", error)
        self.assertLess(time.monotonic() - started, 9)
        self.assertEqual(self.trace(), ["stop:statusbar", "start:statusbar", "xrefresh:"])
        self.assertFalse((self.root / "lock").exists())

    def test_ack_in_the_last_deadline_interval_is_accepted(self):
        self.env.update(TEST_HOLD="1", TEST_ACK_DELAY="2.2")
        self.launcher.write_text(self.launcher.read_text().replace("waiter_remaining=8", "waiter_remaining=3"))

        def drive(_):
            self.ready()
            self.suspend()
            self.assertTrue(self.state()["suspended"])
            self.app_action("quit")

        status, _, error = self.run_launcher(drive)
        self.assertEqual(status, 0, error)
        self.assertNotIn("deadline", error)

    def test_usb_during_ack_wait_is_not_blocked_by_the_deadline(self):
        self.env.update(TEST_HOLD="1", TEST_ACK_MODE="none")

        def drive(_):
            self.ready()
            self.emit("goingToScreenSaver 2")
            self.wait_until(lambda: "suspend" in self.app_events())
            self.assertNotIn("xrefresh:", self.trace())
            self.emit("usbConfigured 1")

        started = time.monotonic()
        status, _, error = self.run_launcher(drive)
        self.assertEqual(status, 143, error)
        self.assertLess(time.monotonic() - started, 6)
        self.assertNotIn("resume", self.app_events())
        self.assertEqual(self.trace(), ["stop:statusbar", "start:statusbar", "xrefresh:"])

    def test_usb_while_sleeping_does_not_resume_or_restart(self):
        self.env["TEST_HOLD"] = "1"

        def drive(_):
            self.ready()
            self.suspend()
            self.emit("outOfScreenSaver 1", "usbConfigured 1", "exitingScreenSaver 1")

        status, _, error = self.run_launcher(drive)
        self.assertEqual(status, 143, error)
        self.assertNotIn("resume", self.app_events())
        self.assertEqual(len(self.starts()), 1)
        self.assertEqual(self.trace(), ["stop:statusbar", "start:statusbar", "xrefresh:"])

    def test_ordinary_exit_while_suspending_preserves_application_status(self):
        self.env.update(TEST_HOLD="1", TEST_ACK_MODE="none", TEST_APP_EXIT="7")

        def drive(_):
            self.ready()
            self.emit("goingToScreenSaver 2")
            self.wait_until(lambda: "suspend" in self.app_events())
            self.app_action("quit")

        status, _, error = self.run_launcher(drive)
        self.assertEqual(status, 7, error)
        self.assertNotIn("deadline", error)

    def test_recorded_application_exit_wins_a_queued_timeout_race(self):
        self.env.update(TEST_HOLD="1", TEST_ACK_MODE="none", TEST_APP_EXIT="7")
        self.launcher.write_text(
            self.launcher.read_text().replace(
                "    printf '_bilicomics_app_exit ",
                "    sleep 0.3\n    printf '_bilicomics_app_exit ",
            )
        )

        def drive(_):
            self.ready()
            self.emit("goingToScreenSaver 2")
            self.wait_until(lambda: "suspend" in self.app_events())
            generation, token, sequence, action = self.request().split(":")
            self.app_action("quit")
            self.wait_until(lambda: (self.root / "lock" / "application-exit").exists())
            self.emit(f"_bilicomics_lifecycle_timeout {generation} {token} {sequence} {action}")

        status, _, error = self.run_launcher(drive)
        self.assertEqual(status, 7, error)
        self.assertNotIn("deadline", error)

    def test_ordinary_exit_while_sleeping_ends_the_supervisor(self):
        self.env["TEST_HOLD"] = "1"

        def drive(_):
            self.ready()
            self.suspend()
            self.app_action("quit")

        status, _, error = self.run_launcher(drive)
        self.assertEqual(status, 0, error)
        self.assertEqual(self.trace(), ["stop:statusbar", "start:statusbar", "xrefresh:"])
        self.assertFalse((self.root / "lock").exists())

    def test_previously_stopped_services_remain_stopped_after_sleep_and_exit(self):
        self.env["TEST_HOLD"] = "1"
        (self.root / "statusbar-state").write_text("stop/waiting\n")
        running = subprocess.Popen(["sleep", "60"])
        stopped = subprocess.Popen(["sleep", "60"])
        self.addCleanup(self.stop_fixture_process, running)
        self.addCleanup(self.stop_fixture_process, stopped)
        os.kill(stopped.pid, signal.SIGSTOP)
        self.wait_until(lambda: self.process_state(stopped.pid).startswith("T"))
        self.script(self.bin / "pidof", f"printf '%s\\n' '{running.pid} {stopped.pid}'")

        def drive(_):
            self.ready()
            self.wait_until(lambda: self.process_state(running.pid).startswith("T"))
            self.suspend()
            self.assertFalse(self.process_state(running.pid).startswith("T"))
            self.assertTrue(self.process_state(stopped.pid).startswith("T"))
            self.resume()
            self.wait_until(lambda: self.process_state(running.pid).startswith("T"))
            self.app_action("quit")

        status, _, error = self.run_launcher(drive)
        self.assertEqual(status, 0, error)
        self.assertFalse(self.process_state(running.pid).startswith("T"))
        self.assertTrue(self.process_state(stopped.pid).startswith("T"))
        self.assertEqual(self.trace(), ["xrefresh:"] * 2)

    @staticmethod
    def process_state(pid):
        for line in Path(f"/proc/{pid}/status").read_text().splitlines():
            if line.startswith("State:"):
                return line.split(":", 1)[1].strip()
        raise AssertionError("The fixture process has no state.")

    @staticmethod
    def stop_fixture_process(process):
        if process.poll() is None:
            process.kill()
            process.wait()

    def test_external_signal_while_sleeping_exits_and_cleans_up(self):
        self.env["TEST_HOLD"] = "1"

        def drive(child):
            self.ready()
            self.suspend()
            child.send_signal(signal.SIGTERM)

        status, _, error = self.run_launcher(drive)
        self.assertEqual(status, 143, error)
        self.assertEqual(self.trace(), ["stop:statusbar", "start:statusbar", "xrefresh:"])

    def test_repeated_cleanup_signals_preserve_the_first_status(self):
        self.env.update(
            TEST_HOLD="1",
            TEST_CLEANUP_ENTERED=str(self.root / "cleanup-entered"),
            TEST_CLEANUP_RELEASE=str(self.root / "cleanup-release"),
        )
        self.script(
            self.bin / "start",
            ': >"$TEST_CLEANUP_ENTERED"\n'
            'while [ ! -e "$TEST_CLEANUP_RELEASE" ]; do sleep 0.01; done\n'
            'printf "start:%s\\n" "$*" >>"$TEST_TRACE"',
        )

        def drive(child):
            self.ready()
            child.send_signal(signal.SIGINT)
            self.wait_until(lambda: (self.root / "cleanup-entered").exists())
            for repeated in (signal.SIGTERM, signal.SIGHUP, signal.SIGINT, signal.SIGTERM):
                child.send_signal(repeated)
            (self.root / "cleanup-release").touch()

        status, _, error = self.run_launcher(drive)
        self.assertEqual(status, 130, error)
        self.assertEqual(self.trace(), ["stop:statusbar", "start:statusbar", "xrefresh:"])

    def test_observer_failure_exits_and_restores_services(self):
        self.env["TEST_HOLD"] = "1"
        self.observer_source("exit 7")
        status, _, error = self.run_launcher()
        self.assertEqual(status, 1, error)
        self.assertIn("observation ended unexpectedly", error)
        self.assertEqual(self.trace(), [])
        self.assertEqual(self.starts(), [])

    def test_observer_ignoring_signals_is_reaped_on_exit(self):
        self.observer_source('trap "" INT TERM\nexec sleep 60')
        status, _, error = self.run_launcher()
        self.assertEqual(status, 0, error)
        self.assertFalse((self.root / "lock").exists())

    def test_repeated_fast_exit_does_not_strand_children(self):
        for attempt in range(8):
            with self.subTest(attempt=attempt):
                status, _, error = self.run_launcher()
                self.assertEqual(status, 0, error)
                self.assertFalse((self.root / "lock").exists())


if __name__ == "__main__":
    unittest.main()
