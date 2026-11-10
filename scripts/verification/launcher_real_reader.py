#!/usr/bin/env python3
"""Exercise the production launcher with the real Rust desktop lifecycle."""

import argparse
import ctypes
import ctypes.util
import datetime
import hashlib
import json
import os
from pathlib import Path
import re
import signal
import sqlite3
import subprocess
import sys
import time
import traceback

SERVICE_CYCLE = ["stop:statusbar", "start:statusbar", "xrefresh:"]


def require(value, message):
    if not value:
        raise AssertionError(message)


def write_json(path, value):
    path.write_text(json.dumps(value, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")


def lines(path):
    if not path.exists():
        return []
    return [line.rstrip("\n") for line in path.read_text().splitlines(keepends=True)
            if line.endswith("\n")]


def records(path):
    return [json.loads(line) for line in lines(path) if line.strip()]


def identity(pid):
    fields = Path(f"/proc/{pid}/stat").read_text().rsplit(") ", 1)[1].split()
    require(fields[0] != "Z", "The real Rust process is a zombie")
    return {"pid": pid, "start_ticks": fields[19],
            "executable": str(Path(f"/proc/{pid}/exe").resolve(strict=True))}


def parent(pid):
    return int(next(line.split(":", 1)[1] for line in Path(f"/proc/{pid}/status").read_text().splitlines()
                    if line.startswith("PPid:")))


def group_identity(pid):
    fields = Path(f"/proc/{pid}/stat").read_text().rsplit(") ", 1)[1].split()
    return {"pid": pid, "state": fields[0], "parent": int(fields[1]),
            "group": int(fields[2]), "session": int(fields[3]), "start_ticks": fields[19]}


def observer_proof(launcher, lock):
    require(launcher.poll() is None, "Observer ownership requires a live launcher")
    master = identity(launcher.pid)
    record = (lock / "observer-owned").read_text().strip()
    fields = record.split(":")
    require(len(fields) == 4 and re.fullmatch(r"[1-9][0-9]*:[0-9a-f]{32}:[1-9][0-9]*:[0-9]+", record),
            "The observer ownership record is invalid")
    generation, nonce, process, birth = fields
    require((lock / "generation").read_text().strip() == f"{generation}:{nonce}"
            and (lock / "nonce").read_text().strip() == nonce, "The observer ownership context does not match")
    observer = group_identity(int(process))
    observer.update(identity(int(process)))
    require(observer["state"] not in ("Z", "X") and observer["parent"] == launcher.pid
            and observer["group"] == observer["session"] == observer["pid"]
            and observer["start_ticks"] == birth, "The observer does not match its live ownership record")
    after = group_identity(observer["pid"])
    require(all(after[key] == observer[key] for key in ("pid", "parent", "group", "session", "start_ticks"))
            and identity(observer["pid"])["executable"] == observer["executable"]
            and (lock / "observer-owned").read_text().strip() == record
            and (lock / "generation").read_text().strip() == f"{generation}:{nonce}"
            and (lock / "nonce").read_text().strip() == nonce
            and identity(launcher.pid) == master and launcher.poll() is None,
            "Observer ownership changed while it was captured")
    return {"generation": int(generation), "nonce_sha256": hashlib.sha256(nonce.encode()).hexdigest(),
            "master": master, "observer": observer}


def observer_group_members(proof):
    members = []
    group = proof["observer"]["pid"]
    for path in Path("/proc").iterdir():
        if not path.name.isdigit():
            continue
        try:
            value = group_identity(int(path.name))
        except (FileNotFoundError, ProcessLookupError):
            continue
        if value["group"] == value["session"] == group and value["state"] not in ("Z", "X"):
            members.append(value)
    return members


def cleanup_observer_group(proof, result):
    evidence = result.setdefault("observer_process_group", {})
    if proof is None:
        evidence.update({"cleaned": False, "proof_gap": "No live master-bound observer identity was captured; no observer group signal was sent"})
        result["passed"] = False
        return
    evidence["proof"] = proof
    deadline = time.monotonic() + 1
    members = observer_group_members(proof)
    while members and time.monotonic() < deadline:
        time.sleep(0.04)
        members = observer_group_members(proof)
    if members:
        result["passed"] = False
        result.setdefault("error", "The launcher left its independently owned observer group running")
        evidence["members_before_cleanup"] = members
        try:
            expected = proof["observer"]
            current = group_identity(expected["pid"])
            current.update(identity(expected["pid"]))
            require(all(current[key] == expected[key] for key in ("pid", "group", "session", "start_ticks", "executable")),
                    "The captured observer leader identity no longer matches")
            os.killpg(expected["pid"], signal.SIGKILL)
            evidence["verified_group_kill_sent"] = True
        except (OSError, AssertionError) as error:
            evidence["cleanup_gap"] = str(error)
        deadline = time.monotonic() + 5
        members = observer_group_members(proof)
        while members and time.monotonic() < deadline:
            time.sleep(0.04)
            members = observer_group_members(proof)
    evidence.update({"cleaned": not members, "remaining_live_members": members})


def stopped(pid):
    return next(line.split(":", 1)[1].strip() for line in Path(f"/proc/{pid}/status").read_text().splitlines()
                if line.startswith("State:")).startswith("T")


def wait(description, predicate, launcher=None, timeout=60):
    deadline = time.monotonic() + timeout
    last_error = None
    while time.monotonic() < deadline:
        if launcher is not None:
            require(launcher.poll() is None, f"Launcher exited while waiting for {description}: {launcher.returncode}")
        try:
            value = predicate()
            if value:
                return value
        except (FileNotFoundError, ProcessLookupError, json.JSONDecodeError, sqlite3.Error) as error:
            last_error = str(error)
        time.sleep(0.04)
    raise AssertionError(f"Timed out waiting for {description}; last read error: {last_error}")


def stored(database, comic):
    with sqlite3.connect(database, timeout=5) as connection:
        anchor = connection.execute(
            "SELECT payload FROM reading_anchors WHERE account_key='design-demo' AND comic_id=?", (comic,)).fetchone()
        preferences = connection.execute(
            "SELECT payload FROM preferences WHERE account_key='design-demo' AND key='preferences'").fetchone()
    require(anchor and preferences, "The real App has not persisted its fixture state")
    return {"anchor": json.loads(anchor[0]), "preferences": json.loads(preferences[0])}


class Pointer:
    def __init__(self):
        self.x11 = ctypes.CDLL(ctypes.util.find_library("X11"))
        self.xtst = ctypes.CDLL(ctypes.util.find_library("Xtst"))
        self.x11.XOpenDisplay.argtypes = [ctypes.c_char_p]
        self.x11.XOpenDisplay.restype = ctypes.c_void_p
        self.x11.XDefaultScreen.argtypes = [ctypes.c_void_p]
        self.x11.XDefaultScreen.restype = ctypes.c_int
        self.x11.XFlush.argtypes = [ctypes.c_void_p]
        self.x11.XSync.argtypes = [ctypes.c_void_p, ctypes.c_int]
        self.x11.XCloseDisplay.argtypes = [ctypes.c_void_p]
        self.xtst.XTestFakeMotionEvent.argtypes = [ctypes.c_void_p, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_ulong]
        self.xtst.XTestFakeButtonEvent.argtypes = [ctypes.c_void_p, ctypes.c_uint, ctypes.c_int, ctypes.c_ulong]
        self.display = self.x11.XOpenDisplay(None)
        require(self.display, "An isolated Xvfb display is required")

    def window(self):
        tree = subprocess.check_output(["xwininfo", "-root", "-tree"], text=True)
        matches = re.findall(r'(0x[0-9a-fA-F]+) "BiliComics - Design Preview"', tree)
        require(len(matches) <= 1, "Multiple application windows appeared")
        if not matches:
            return None
        details = subprocess.check_output(["xwininfo", "-id", matches[0]], text=True)
        if "Map State: IsViewable" not in details:
            return None
        expressions = {"x": r"Absolute upper-left X:\s*(-?\d+)", "y": r"Absolute upper-left Y:\s*(-?\d+)",
                       "width": r"Width:\s*(\d+)", "height": r"Height:\s*(\d+)"}
        return {"id": matches[0], **{key: int(re.search(expression, details).group(1))
                                    for key, expression in expressions.items()}}

    def next(self, window):
        x = window["x"] + round(window["width"] * 0.86)
        y = window["y"] + round(window["height"] * 0.48)
        self.xtst.XTestFakeMotionEvent(self.display, self.x11.XDefaultScreen(self.display), x, y, 0)
        self.xtst.XTestFakeButtonEvent(self.display, 1, 1, 0)
        self.x11.XFlush(self.display)
        time.sleep(0.08)
        self.xtst.XTestFakeButtonEvent(self.display, 1, 0, 0)
        self.x11.XSync(self.display, 0)
        return {"method": "XTest", "button": 1, "x": x, "y": y, "zone": "reader-right-next"}

    def close(self):
        self.x11.XCloseDisplay(self.display)


def script(path, body):
    path.write_text("#!/bin/sh\nset -eu\n" + body + "\n", encoding="utf-8")
    path.chmod(0o700)


def install_stubs(directory):
    script(directory / "status", '[ "$*" = statusbar ] || exit 92\nprintf "statusbar %s\\n" "$(cat "$HARNESS_STATUSBAR")"')
    for command, state in (("stop", "stop/waiting"), ("start", "start/running, process 123")):
        script(directory / command, '[ "$*" = statusbar ] || exit 92\n'
               f'printf "{command}:%s\\n" "$*" >>"$HARNESS_SERVICES"\n'
               f'printf "{state}\\n" >"$HARNESS_STATUSBAR"')
    script(directory / "xrefresh", '[ "$#" -eq 0 ] || exit 92\nprintf "xrefresh:\\n" >>"$HARNESS_SERVICES"')
    script(directory / "pidof", '[ "$*" = awesome ] || exit 92\nprintf "%s %s\\n" "$HARNESS_WM_RUNNING" "$HARNESS_WM_STOPPED"')
    script(directory / "awesome", "exit 92")
    script(directory / "lipc-get-prop", 'case "$*" in\n'
           '  "-s com.lab126.powerd state") printf "%s\\n" "$*" >>"$HARNESS_POWER_QUERIES"; exec /bin/cat "$HARNESS_POWER_STATE" ;;\n'
           '  "-s com.lab126.winmgr epdcMode") printf "Y8\\n" ;;\n'
           '  *) exit 92 ;;\nesac')
    script(directory / "usleep", '[ "$#" -eq 1 ] || exit 92\n'
           'exec /usr/bin/python3 -c \'import sys,time; time.sleep(int(sys.argv[1])/1000000)\' "$1"')
    script(directory / "lipc-wait-event",
           '[ "$*" = "-m -s 0 com.lab126.powerd goingToScreenSaver,outOfScreenSaver,exitingScreenSaver,wakeupFromSuspend,usbConfigured" ] || exit 92\n'
           'printf "%s\\n" "$*" >"$HARNESS_OBSERVER_ARGS"\nexec /bin/cat "$HARNESS_POWER_FIFO"')


def install_wrapper(path):
    script(path, '''if [ "$#" -eq 1 ] && [ "$1" = --relay-power-events ]; then
    exec "$HARNESS_BINARY" "$@"
fi
if [ "$#" -eq 4 ] && [ "$1" = --probe-native-portrait-framebuffer ]; then
    [ "$2" = --kindle ] && [ "$3" = --device ] && [ "$4" = pw5 ] || exit 93
    [ "$BILICOMICS_APPROVED_PROFILE" = pw5 ] && [ "$BILICOMICS_APPROVED_FIRMWARE" = 5.20.1 ] || exit 93
    # This private fixture supplies metadata; it never touches native devices
    # or enters the UI identity/demo branch for a read-only helper invocation.
    printf "FIXED:0\\n" >>"$HARNESS_PORTRAIT_HELPERS"
    printf "FIXED:0\\n"
    exit 0
fi
if [ "$#" -eq 4 ] && [ "$1" = --capture-native-display ]; then
    [ "$2" = --kindle ] && [ "$3" = --device ] && [ "$4" = pw5 ] || exit 93
    [ "$BILICOMICS_APPROVED_PROFILE" = pw5 ] && [ "$BILICOMICS_APPROVED_FIRMWARE" = 5.20.1 ] || exit 93
    [ -n "$BILICOMICS_LIFECYCLE_DIR" ] && [ -n "$BILICOMICS_LAUNCH_GENERATION" ] && [ -n "$BILICOMICS_LAUNCH_TOKEN" ] || exit 93
    # This OS-service fixture supplies fixed metadata. Real native sampling and
    # its before-I/O ownership checks have separate Rust/Kindle-feature tests.
    power=$(lipc-get-prop -s com.lab126.powerd state) || exit 93
    if [ "$power" != active ]; then printf "not-active\\n"; exit 0; fi
    mode=$(lipc-get-prop -s com.lab126.winmgr epdcMode) || exit 93
    case "$mode" in Y8|Y8INV) ;; *) exit 93 ;; esac
    printf "FIXED:0\\nFIXED:0\\n" >>"$HARNESS_PORTRAIT_HELPERS"
    power=$(lipc-get-prop -s com.lab126.powerd state) || exit 93
    if [ "$power" != active ]; then printf "not-active\\n"; exit 0; fi
    printf "ready:%s:FIXED:0\\n" "$mode"
    exit 0
fi
if [ "$#" -eq 5 ] && [ "$1" = --publish-native-portrait-orientation ]; then
    [ "$3" = --kindle ] && [ "$4" = --device ] && [ "$5" = pw5 ] || exit 93
    [ "$BILICOMICS_APPROVED_PROFILE" = pw5 ] && [ "$BILICOMICS_APPROVED_FIRMWARE" = 5.20.1 ] || exit 93
    case "$2" in FIXED:0|CALIBRATED:*) ;; *) exit 93 ;; esac
    printf "%s\\n" "$2" >>"$HARNESS_PORTRAIT_PUBLISHERS"
    # Exercise the real Rust descriptor-relative writer inside the launcher
    # lease without routing its internal invocation through the UI branch.
    exec "$HARNESS_BINARY" "$@"
fi
[ "$#" -eq 5 ] || exit 93
[ "$1" = --kindle ] && [ "$2" = --device ] && [ "$3" = pw5 ] && [ "$4" = --data-dir ] || exit 93
[ "$5" = "$HARNESS_DATA" ] && [ "$BILICOMICS_LIFECYCLE_DIR" = "$HARNESS_LOCK" ] || exit 93
[ "$BILICOMICS_APPROVED_PROFILE" = pw5 ] && [ "$BILICOMICS_APPROVED_FIRMWARE" = 5.20.1 ] || exit 93
printf "%s\\n" "$$" >>"$HARNESS_STARTS"
printf "%s|%s|%s|%s\\n" "$$" "$BILICOMICS_LAUNCH_GENERATION" "$BILICOMICS_LAUNCH_TOKEN" "$BILICOMICS_LIFECYCLE_DIR" >>"$HARNESS_CONTEXTS"
printf "%s\\n" "$@" >"$HARNESS_LAUNCH_ARGS"
exec "$HARNESS_BINARY" --demo --offline --device "$3" --data-dir "$5" --screen shelf --actions "$HARNESS_ACTIONS" --capture "$HARNESS_CAPTURE" --capture-startup "$HARNESS_STARTUP" --startup-delay-ms 900 --lifecycle-trace "$HARNESS_TRACE" --exit-after-ms 120000''')


def run(arguments, result):
    workspace = arguments.workspace.resolve(strict=True)
    source = (workspace / "app/packaging/kindle/launch.sh").resolve(strict=True)
    binary = (arguments.binary or workspace / "target/debug/bilicomics").resolve(strict=True)
    require(binary.is_file() and os.access(binary, os.X_OK), "The new Rust binary is unavailable")
    require("baseline" not in binary.parts, "The preserved candidate3 binary must not be used")
    require(os.environ.get("DISPLAY"), "Run this case under isolated xvfb-run")
    output = arguments.output.resolve()
    require(output.is_relative_to(workspace) and output != workspace, "Output must be a new directory under the isolated workspace")
    digest = hashlib.sha256(binary.read_bytes()).hexdigest()
    if arguments.expected_binary_sha256:
        require(digest == arguments.expected_binary_sha256, "The binary hash does not match the selected new build")
    output.mkdir(parents=True, mode=0o700, exist_ok=False)
    result["output_created"] = True
    result.update({"case": "production_launcher_real_reader_retains_ram", "binary": str(binary), "binary_sha256": digest,
                   "output": str(output), "launcher_source": str(source),
                   "launcher_source_sha256": hashlib.sha256(source.read_bytes()).hexdigest(),
                   "coverage": "Isolated Linux production launcher, real Rust SignalBridge and App lifecycle, real XTest input",
                   "limitations": ["No physical Kindle or firmware-emulator acceptance is established",
                                   "Desktop display lifecycle does not establish native framebuffer or evdev ownership behavior"],
                   "real_system_services_used": False, "live_account_used": False})
    fixture = json.loads((workspace / "app/assets/demo/catalog.json").read_text())
    deep_index, deep = next((index, comic) for index, comic in enumerate(fixture["comics"]) if comic["key"] == "deep")
    require(deep_index > 0, "The selected fixture comic must not be the first comic")
    comic, episode = str(deep["id"]), str(int(deep["id"]) * 100 + int(deep["read_episode"]))
    result["fixture_identity"] = {"key": "deep", "catalog_index": deep_index, "comic_id": comic, "episode_id": episode}
    home, bin_dir, lock = output / "bilicomics", output / "bin", output / "lock"
    (home / "app").mkdir(parents=True)
    data = home / "data"
    data.mkdir()
    bin_dir.mkdir()
    (data / "device-profile").write_text("pw5\n")
    (data / "device-allowlist").write_text("pw5 5.20.1\n")
    firmware = output / "firmware"
    firmware.write_text("Kindle 5.20.1 (isolated launcher fixture)\n")
    (output / "statusbar-state").write_text("start/running, process 123\n")
    (output / "power-state").write_text("active\n")
    input_fifo = output / "power-input.fifo"
    os.mkfifo(input_fifo, 0o600)
    launcher_text = source.read_text()
    replacements = {"/etc/prettyversion.txt": str(firmware), "/dev/fb0": "/dev/null",
                    "/tmp/bilicomics-launcher.lock": str(lock)}
    for original, replacement in replacements.items():
        require(original in launcher_text, f"Missing explicit launcher fixture boundary: {original}")
        launcher_text = launcher_text.replace(original, replacement)
    require("\nsleep 2\n" in launcher_text, "The production launcher startup delay must remain intact")
    launcher_file = output / "launch.sh"
    launcher_file.write_text(launcher_text)
    launcher_file.chmod(0o700)
    result["launcher_transform"] = {"replacements": replacements, "line_endings": "LF",
                                  "production_startup_delay_retained": True,
                                  "copied_sha256": hashlib.sha256(launcher_file.read_bytes()).hexdigest()}
    install_stubs(bin_dir)
    install_wrapper(home / "app/bilicomics")
    actions, capture, startup = output / "actions.json", output / "reader-after-next.png", output / "startup.png"
    write_json(actions, [{"at_ms": 100, "section": "account", "action": "launch", "value": "shelf"},
                         {"at_ms": 250, "section": "reader", "action": "open-comic", "value": comic},
                         {"at_ms": 600, "section": "reader", "action": "setting", "value": "mode:page"},
                         {"at_ms": 750, "section": "reader", "action": "setting", "value": "direction:ltr"},
                         {"at_ms": 1000, "section": "reader", "action": "source-page", "value": "8"}])
    services, trace = output / "services.log", output / "lifecycle.jsonl"
    starts, contexts = output / "application-starts", output / "application-contexts"
    database = data / "demo/state.sqlite3"
    environment = os.environ.copy()
    for key in ("BILICOMICS_LIFECYCLE_DIR", "BILICOMICS_LAUNCH_GENERATION", "BILICOMICS_LAUNCH_TOKEN"):
        environment.pop(key, None)
    environment.update({"BILICOMICS_HOME": str(home), "PATH": f"{bin_dir}:/usr/bin:/bin",
                        "HARNESS_BINARY": str(binary), "HARNESS_DATA": str(data), "HARNESS_LOCK": str(lock),
                        "HARNESS_ACTIONS": str(actions), "HARNESS_CAPTURE": str(capture), "HARNESS_STARTUP": str(startup),
                        "HARNESS_TRACE": str(trace), "HARNESS_STARTS": str(starts), "HARNESS_CONTEXTS": str(contexts),
                        "HARNESS_LAUNCH_ARGS": str(output / "launch-args"), "HARNESS_SERVICES": str(services),
                        "HARNESS_STATUSBAR": str(output / "statusbar-state"), "HARNESS_POWER_FIFO": str(input_fifo),
                        "HARNESS_POWER_STATE": str(output / "power-state"), "HARNESS_POWER_QUERIES": str(output / "power-queries"),
                        "HARNESS_PORTRAIT_HELPERS": str(output / "portrait-helpers"),
                        "HARNESS_PORTRAIT_PUBLISHERS": str(output / "portrait-publishers"),
                        "HARNESS_OBSERVER_ARGS": str(output / "observer-args")})
    launcher = pointer = fifo_fd = original_identity = captured_observer = None
    wm_running = wm_stopped = None
    result["transitions"], result["power_event_injections"] = [], []

    def remember_observer():
        nonlocal captured_observer
        candidate = observer_proof(launcher, lock)
        if captured_observer is not None:
            require(candidate["master"] == captured_observer["master"]
                    and candidate["generation"] == captured_observer["generation"]
                    and candidate["nonce_sha256"] == captured_observer["nonce_sha256"]
                    and all(candidate["observer"][key] == captured_observer["observer"][key]
                            for key in ("pid", "parent", "group", "session", "start_ticks", "executable")),
                    "The captured observer ownership identity changed")
        captured_observer = candidate
        return candidate

    def unchanged():
        require(identity(pid) == original_identity, "The real Rust PID, executable, or start ticks changed")
        require(lines(starts) == [str(pid)], "The launcher started a replacement or extra application")
        require((lock / "application-pid").read_text().strip() == str(pid), "The production launcher child record changed")

    def emit(*events):
        payload = ("\n".join(events) + "\n").encode()
        require(len(payload) < 4096 and os.write(fifo_fd, payload) == len(payload), "The power event FIFO write failed")
        result["power_event_injections"].append({"events": list(events), "monotonic_seconds": time.monotonic()})

    def loaded(page):
        for record in reversed(records(trace)):
            state = record.get("reader_state")
            if (record.get("event") == "reader-ready" and record.get("process_id") == pid
                and record.get("route") == "reader" and record.get("reader_loading") is False
                and len(record.get("frame_size", [])) == 2 and all(value > 0 for value in record["frame_size"])
                and state and state.get("comic_id") == comic and state.get("episode_id") == episode
                and state.get("page_number") == page and state.get("mode") == "page" and state.get("core_mode") == "page"
                and state.get("direction") == "ltr" and state.get("launch_preference") == "shelf"):
                return record
        return None

    def transition(action, sequence, prior):
        header = (lock / "generation").read_text().strip()
        request = f"{header}:{sequence}:{action}"
        wait("the production lifecycle request", lambda: (lock / "lifecycle-request").read_text().strip() == request, launcher)
        wait("the real Rust lifecycle acknowledgement", lambda: (lock / "lifecycle-ack").read_text().strip() == request + ":ok", launcher)
        record = wait("the real Rust power trace", lambda: next((value for value in records(trace)[prior:]
                      if value.get("event") == action and value.get("process_id") == pid), None), launcher)
        require(record["checkpoint_saved"] is True and record["suspended"] is (action == "suspend"), "The lifecycle state is wrong")
        require(record["booting"] is False and record["route"] == "reader", "The lifecycle transition left the loaded reader")
        require((lock / "native-portrait-orientation").read_text() == f"{header}:FIXED:0\n",
                "The startup portrait calibration changed across a retained reader transition")
        if action == "resume":
            require((lock / "resume-native-portrait-orientation").read_text() == f"{header}:{sequence}:CALIBRATED\n",
                    "The portrait observation permission is not bound to the exact resume request")
        else:
            require(not (lock / "resume-native-portrait-orientation").exists(),
                    "Suspend retained permission from an older resume transition")
        result["transitions"].append({"request": request, "acknowledgement": request + ":ok", "trace": record})
        unchanged()
        return record

    try:
        wm_running = subprocess.Popen(["/bin/sleep", "180"], start_new_session=True)
        wm_stopped = subprocess.Popen(["/bin/sleep", "180"], start_new_session=True)
        os.kill(wm_stopped.pid, signal.SIGSTOP)
        wait("the initially stopped fixture window manager", lambda: stopped(wm_stopped.pid))
        environment.update({"HARNESS_WM_RUNNING": str(wm_running.pid), "HARNESS_WM_STOPPED": str(wm_stopped.pid)})
        result["owned_window_managers"] = {"running_pid": wm_running.pid, "previously_stopped_pid": wm_stopped.pid}
        fifo_fd = os.open(input_fifo, os.O_RDWR | os.O_NONBLOCK)
        with (output / "launcher.log").open("w") as log:
            launcher = subprocess.Popen(["/bin/sh", str(launcher_file), "--detached"], env=environment,
                                        stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
            result["launcher_pid"] = launcher.pid
            pid = wait("the recorded application PID", lambda: int((lock / "application-pid").read_text().strip()), launcher)
            wait("the proven independent observer group", remember_observer, launcher)
            original_identity = wait("exec into the real Rust binary", lambda:
                                     value if (value := identity(pid))["executable"] == str(binary) else None, launcher)
            result["process_identity"] = original_identity
            require(parent(parent(pid)) == launcher.pid, "The actual Rust process is not supervised by this launcher")
            header = (lock / "generation").read_text().strip()
            require(re.fullmatch(r"1:[0-9a-f]{32}", header), "The launcher did not create its own valid context")
            wait("real SignalBridge readiness", lambda: (lock / "lifecycle-ready").read_text().strip() == header, launcher)
            result["lifecycle_context"] = {"header": header, "directory": str(lock)}
            require((lock / "native-portrait-orientation").read_text() == f"{header}:FIXED:0\n",
                    "The isolated launcher did not publish its strict portrait fixture calibration")
            ready = wait("the real non-first comic with a loaded page eight", lambda: loaded(8), launcher, timeout=75)
            baseline = ready["reader_state"]
            result["reader_ready_observation"], result["reader_baseline"] = ready, baseline
            require(baseline["frame_width"] > 0 and baseline["frame_height"] > 0, "The retained reader has no rendered frame")
            unchanged()
            require(lines(contexts) == [f"{pid}|1|{header.split(':')[1]}|{lock}"], "The exec wrapper did not preserve the context")
            require(lines(services) == ["stop:statusbar"], "Unexpected initial fixture service takeover")
            wait("the initial fixture WM pause", lambda: stopped(wm_running.pid), launcher)
            require(stopped(wm_stopped.pid), "The launcher resumed an initially stopped fixture WM")
            pointer = Pointer()
            window = wait("the mapped real Rust window after reader readiness", pointer.window, launcher)
            result["window"] = window
            result["interaction_gate"] = "reader-ready, matching PID, reader_loading=false, positive frame dimensions"
            prior = len(records(trace))
            (output / "power-state").write_text("screenSaver\n")
            emit("goingToScreenSaver 2", "goingToScreenSaver 2")
            suspended = transition("suspend", 1, prior)
            require(suspended["reader_state"] == baseline, "Suspend changed the complete retained reader state")
            wait("launcher display release after the real suspend acknowledgement", lambda: lines(services) == SERVICE_CYCLE, launcher)
            wait("the running fixture WM restored for sleep", lambda: not stopped(wm_running.pid), launcher)
            require(stopped(wm_stopped.pid), "Sleep resumed the initially stopped fixture WM")
            result["sleep_input"] = pointer.next(window)
            time.sleep(0.4)
            unchanged()
            sleeping = stored(database, comic)
            require(sleeping["anchor"]["page_index"] == 7 and sleeping["preferences"]["launch"] == "shelf",
                    "Sleeping input changed the persisted page or startup preference")
            result["sleeping_durable_state"] = sleeping
            prior = len(records(trace))
            (output / "power-state").write_text("active\n")
            emit("outOfScreenSaver 1", "outOfScreenSaver 1", "exitingScreenSaver 1", "exitingScreenSaver 1")
            resumed = transition("resume", 2, prior)
            require(resumed["reader_state"] == baseline, "Wake lost the retained reader state or admitted the sleeping click")
            wait("fixture display reacquisition", lambda: lines(services) == SERVICE_CYCLE + ["stop:statusbar"], launcher)
            wait("fixture WM pause after wake", lambda: stopped(wm_running.pid), launcher)
            require(stopped(wm_stopped.pid) and pointer.window() == window, "Wake changed the WM baseline or application window")
            result["awake_input"] = pointer.next(window)
            next_ready = wait("the awake pointer input reaching page nine", lambda: loaded(9), launcher)
            require(next_ready["reader_state"]["reader_generation"] == baseline["reader_generation"]
                    and next_ready["reader_state"]["revision"] == baseline["revision"], "The next page used a replacement reader")
            result["next_reader_observation"] = next_ready
            unchanged()
            observed = [value["event"] for value in records(trace) if value.get("event") in ("suspend", "resume")]
            require(observed == ["suspend", "resume"], "Duplicate events caused extra lifecycle transitions")
            result["duplicate_event_deduplication"] = {"sleep_injected": 2, "out_injected": 2, "exit_injected": 2,
                                                     "observed_transitions": observed, "application_starts": 1}
            os.kill(pid, signal.SIGTERM)
            require(launcher.wait(timeout=25) == 0, "The production launcher lost the real Rust graceful exit status")
        metadata = json.loads(capture.with_suffix(".json").read_text())
        require(metadata["process_id"] == pid and metadata["demo"] is True and metadata["offline"] is True
                and metadata["backend"] == "desktop-software", "The capture is not from the original real isolated Rust process")
        require(metadata["route"] == "reader" and metadata["reader"]["page"] == 9
                and metadata["reader"]["comic_title"] == deep["title"] and metadata["reader"]["mode"] == "page"
                and metadata["reader"]["direction"] == "ltr" and metadata["reader"]["loading"] is False
                and metadata["suspended"] is False and metadata["startup"]["booting"] is False
                and metadata["startup"]["error"] is False, "The final captured reader did not retain the resumed session")
        require(lines(services) == SERVICE_CYCLE * 2 and not lock.exists(), "The production launcher did not restore services or clean its lock")
        require(not stopped(wm_running.pid) and stopped(wm_stopped.pid), "The production launcher did not restore the owned WM baseline")
        require(lines(starts) == [str(pid)], "More than one real Rust application was started")
        require(lines(output / "portrait-helpers") == ["FIXED:0"] * 4,
                "Portrait metadata helpers entered the UI branch or duplicate wakes repeated a capture")
        require(lines(output / "portrait-publishers") == ["FIXED:0", "CALIBRATED:2"],
                "The real Rust portrait writer was bypassed or duplicated across the reader lifecycle")
        final_store = stored(database, comic)
        require(final_store["anchor"]["page_index"] == 8 and final_store["anchor"]["episode_id"] == episode
                and final_store["anchor"]["revision"] == baseline["revision"] and final_store["preferences"]["launch"] == "shelf",
                "The final durable reader anchor or startup preference changed")
        startup_frames = []
        for index in range(2):
            path = output / f"startup-{index}.png"
            frame = json.loads(path.with_suffix(".json").read_text())
            require(frame["process_id"] == pid and frame["booting"] is True and frame["error"] is False,
                    "The startup frame is not from the healthy real Rust loading window")
            startup_frames.append({"path": str(path), "metadata": frame, "sha256": hashlib.sha256(path.read_bytes()).hexdigest()})
        require(startup_frames[0]["metadata"]["animation_frame"] != startup_frames[1]["metadata"]["animation_frame"]
                and startup_frames[0]["sha256"] != startup_frames[1]["sha256"], "The real loading activity did not visibly change")
        result.update({"passed": True, "launcher_exit_code": launcher.returncode, "final_capture": str(capture),
                       "final_metadata": metadata, "final_durable_state": final_store, "service_trace": lines(services),
                       "startup_frames": startup_frames, "observer_arguments": lines(output / "observer-args"),
                       "portrait_calibration_fixture": {"kind": "fixed", "rotation": 0,
                           "helper_records": lines(output / "portrait-helpers"),
                           "publisher_records": lines(output / "portrait-publishers"),
                           "coverage": "Real Rust portrait writer with private fixed metadata; native sensor and touch transforms are not exercised"},
                       "power_state_queries": lines(output / "power-queries"), "lifecycle_markers_cleaned": True})
    finally:
        if launcher is not None and launcher.poll() is None:
            try:
                remember_observer()
            except (OSError, AssertionError, ValueError) as error:
                result.setdefault("observer_process_group", {})["proof_refresh_gap"] = str(error)
            launcher.terminate()
            try:
                launcher.wait(timeout=20)
            except subprocess.TimeoutExpired:
                os.killpg(launcher.pid, signal.SIGKILL)
                launcher.wait(timeout=5)
        if launcher is not None:
            try:
                os.killpg(launcher.pid, 0)
            except ProcessLookupError:
                result["launcher_process_group_cleaned"] = True
            else:
                result["launcher_process_group_cleaned"] = False
                os.killpg(launcher.pid, signal.SIGKILL)
                result["passed"] = False
                result.setdefault("error", "The launcher left an owned child process behind")
        try:
            cleanup_observer_group(captured_observer, result)
        except (OSError, AssertionError, ValueError, IndexError) as error:
            result["passed"] = False
            result.setdefault("observer_process_group", {}).update({"cleaned": False, "cleanup_gap": str(error)})
        if fifo_fd is not None:
            os.close(fifo_fd)
        input_fifo.unlink(missing_ok=True)
        result["fixture_fifo_removed"] = True
        for process in (wm_running, wm_stopped):
            if process is not None and process.poll() is None:
                process.kill()
                process.wait(timeout=5)
        result["owned_window_manager_fixtures_cleaned"] = True
        result["service_trace"] = lines(services)
        result["observed_trace_records"] = records(trace)
        result["observed_application_starts"] = lines(starts)
        result["launcher_log_tail"] = (output / "launcher.log").read_text()[-8000:] if (output / "launcher.log").exists() else ""
        if pointer is not None:
            pointer.close()


def main():
    os.umask(0o077)
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--workspace", type=Path, required=True)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--expected-binary-sha256", required=True)
    arguments = parser.parse_args()
    result = {"passed": False, "started_at": datetime.datetime.now(datetime.timezone.utc).isoformat()}
    try:
        run(arguments, result)
    except Exception as error:
        result.update({"passed": False, "error": str(error), "traceback": traceback.format_exc()})
    result["finished_at"] = datetime.datetime.now(datetime.timezone.utc).isoformat()
    if result.get("output_created"):
        write_json(arguments.output / "launcher-real-reader-result.json", result)
    print(json.dumps(result, indent=2, ensure_ascii=False), flush=True)
    return 0 if result["passed"] else 1


if __name__ == "__main__":
    from harness_signals import install_interruption_handlers
    install_interruption_handlers()
    sys.exit(main())

