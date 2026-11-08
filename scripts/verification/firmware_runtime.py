#!/usr/bin/env python3
"""Verify a BiliComics development bundle in an isolated Scribe firmware guest."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path, PurePosixPath
import shlex
import shutil
import subprocess
import sys
import threading
import time
import zipfile

from PIL import Image, ImageChops


GUEST_ROOT = "/mnt/us/bilicomics-predevice"
LOGS = GUEST_ROOT + "/data/logs"
LOCK = "/tmp/bilicomics-launcher.lock"
CASES = ("startup-1", "interactive", "reader", "sleep", "recovery", "startup-2", "startup-3")
READER_IDENTITY_FIELDS = ("comic_id", "episode_id", "revision", "reader_generation", "route",
                          "launch_preference", "mode", "core_mode", "fit", "direction", "rotation",
                          "landscape_layout", "cover_alone", "dither", "footer", "demo_strip")
BODY_ROI = (0, 160, 1860, 2290)
ARTWORK_REGIONS = {"disc": (920, 290, 980, 350),
                   "upper_right": (1500, 780, 1600, 820),
                   "lower_right": (1500, 1900, 1600, 2000)}


def sha256(path: Path) -> str:
    value = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            value.update(chunk)
    return value.hexdigest()


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--workspace", type=Path, required=True)
    parser.add_argument("--package", type=Path, required=True)
    parser.add_argument("--wrapper", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--case", choices=("all", *CASES), default="all")
    parser.add_argument("--deploy", action="store_true")
    args = parser.parse_args()
    if sys.platform != "linux":
        raise SystemExit("Run firmware verification only on the Linux verification host.")
    workspace = args.workspace.resolve(strict=True)
    if not str(workspace).startswith("/root/bilicomics-emulator-predevice-"):
        raise SystemExit("Only the explicitly isolated BiliComics workspace may be used.")
    environment = json.loads((workspace / "environment.json").read_text())
    machine = workspace / "emulator/machines/bili-s1"
    if not machine.is_dir():
        raise SystemExit("The expected independent bili-s1 guest is missing.")
    qemu_pid = int((workspace / "qemu.pid").read_text())
    qemu_command = [value.decode() for value in (Path("/proc") / str(qemu_pid) / "cmdline").read_bytes().split(b"\0") if value]
    assert any(str(machine) in value for value in qemu_command), qemu_command
    current_accelerator = qemu_command[qemu_command.index("-accel") + 1]
    launches = workspace / "host-launches.jsonl"
    host_launch = json.loads(launches.read_text().splitlines()[-1]) if launches.exists() else None
    if host_launch is not None:
        assert host_launch["qemu_pid"] == qemu_pid, host_launch
    output = args.output.resolve()
    if output.exists():
        raise SystemExit("Each evidence run requires a new output directory.")
    output.mkdir(parents=True)
    helper = workspace / "guest-ssh"
    emulator = workspace / "emulator"
    report = {"schema_version": 1, "environment": environment,
              "active_qemu_pid": qemu_pid, "active_qemu_command": qemu_command,
              "active_qemu_accelerator": current_accelerator,
              "active_host_launch": host_launch,
              "physical_device_accessed": False, "live_account_used": False,
              "runtime_harness_sha256": sha256(Path(__file__)),
              "package_sha256": sha256(args.package), "cases": [],
              "sleep_contract": "Preserve the same process, in-memory reader and source anchor."}
    active_pid = None
    qmp_lock = threading.Lock()

    def guest(*command: str, data=None, check=True, timeout=25):
        return subprocess.run([str(helper), shlex.join(command)], input=data,
                              capture_output=True, check=check, timeout=timeout)

    def guest_shell(command: str, **kwargs):
        return guest("sh", "-c", command, **kwargs)

    def qmp(*command):
        with qmp_lock:
            return subprocess.run([sys.executable, str(emulator / "scripts/eink-qmp.py"),
                                   "--machine", "bili-s1", *map(str, command)],
                                  capture_output=True, check=True, timeout=25)

    def capture(directory: Path, name: str) -> Path:
        path = (directory / (name + ".png")).resolve()
        qmp("screendump", path)
        with Image.open(path) as image:
            assert image.size == (1860, 2480), image.size
        return path

    def capture_artwork(directory: Path, name: str, page_number: int, record: dict) -> Path:
        deadline = time.monotonic() + 15
        previous_hash = None
        samples = []
        expected_template = 19 if page_number == 19 else 18
        while True:
            path = capture(directory, f"{name}-candidate-{len(samples):03d}")
            with Image.open(path) as image:
                gray = image.convert("L")
                ratios = {}
                for region, box in ARTWORK_REGIONS.items():
                    pixels = gray.crop(box).tobytes()
                    ratios[region] = sum(value < 96 for value in pixels) / len(pixels)
                pixels = gray.crop(BODY_ROI).tobytes()
                content_fraction = sum(value < 128 for value in pixels) / len(pixels)
                body_hash = hashlib.sha256(pixels).hexdigest()
            expected_distribution = (
                ratios["disc"] > 0.9 and ratios["upper_right"] > 0.9 and ratios["lower_right"] < 0.35
                if expected_template == 18 else
                ratios["disc"] < 0.1 and ratios["upper_right"] < 0.1 and ratios["lower_right"] > 0.9)
            qualified = content_fraction > 0.08 and expected_distribution
            samples.append({"path": path.name, "body_sha256": body_hash,
                            "content_ink_fraction": content_fraction, "region_black_ratios": ratios,
                            "expected_template_matches": expected_distribution, "qualified": qualified})
            record.setdefault("native_artwork", {})[name] = {
                "page_number": page_number, "bundled_template_number": expected_template,
                "fixture_substitution": page_number != expected_template,
                "body_roi": BODY_ROI, "samples": samples,
                "consecutive_stable_frames_required": 2, "accepted": False}
            if qualified and previous_hash == body_hash:
                destination = directory / (name + ".png")
                shutil.copy2(path, destination)
                record["native_artwork"][name].update({"accepted": True, "body_sha256": body_hash,
                                                       "stable_frames": 2})
                return destination
            previous_hash = body_hash if qualified else None
            if time.monotonic() >= deadline:
                raise AssertionError(f"The native page {page_number} artwork did not reach two stable body frames.")
            time.sleep(0.5)

    def body_hash(path: Path) -> str:
        with Image.open(path) as image:
            return hashlib.sha256(image.convert("L").crop(BODY_ROI).tobytes()).hexdigest()

    def trace() -> list[dict]:
        response = guest("cat", LOGS + "/lifecycle.jsonl", check=False)
        return [json.loads(line) for line in response.stdout.decode().splitlines(keepends=True)
                if line.endswith("\n") and line.strip()]

    def wait_for(predicate, description: str, process=None, timeout=80):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            values = trace()
            match = predicate(values)
            if match:
                return match
            if process is not None and process.poll() is not None:
                raise RuntimeError(f"Application exited before {description}: {process.returncode}")
            time.sleep(0.35)
        raise TimeoutError(description)

    def read_pid(path: str) -> str:
        value = guest("cat", path).stdout.decode().strip()
        assert value.isdecimal() and int(value) > 1, value
        return value

    def process_identity(pid: str) -> dict:
        raw = guest_shell(f"cat /proc/{pid}/stat; readlink /proc/{pid}/exe; sha256sum /proc/{pid}/exe").stdout.decode().splitlines()
        assert len(raw) == 3, raw
        fields = raw[0].rsplit(")", 1)[1].split()
        assert raw[0].split(" ", 1)[0] == pid, raw
        value = {"pid": int(pid), "start_ticks": int(fields[19]),
                 "exe_path": raw[1], "exe_sha256": raw[2].split()[0]}
        assert value["exe_path"] == GUEST_ROOT + "/app/" + binary_name, value
        assert value["exe_sha256"] == binary_digest, value
        return value

    def restored() -> dict:
        guest("test", "!", "-d", LOCK)
        status = guest("status", "statusbar").stdout.decode().strip()
        assert "start/running" in status, status
        process_states = {}
        for pid in guest("pidof", "awesome").stdout.decode().split():
            state = guest("cat", f"/proc/{pid}/status").stdout.decode()
            line = next(line for line in state.splitlines() if line.startswith("State:"))
            assert line.split()[1] not in ("T", "t"), line
            process_states[pid] = line
        assert process_states, "The native window manager is absent."
        return {"statusbar": status, "awesome": process_states, "launcher_lock": "absent"}

    def tap(x: int, y: int):
        qmp("tap", x, y, "--hold", 0.1)

    def reader_ready(values):
        return next((value for value in reversed(values)
                     if value.get("event") == "reader-ready" and value.get("process_id") == active_pid
                     and value.get("reader_loading") is False
                     and len(value.get("frame_size", [])) == 2 and min(value["frame_size"]) > 0
                     and isinstance(value.get("reader_state"), dict)
                     and all(value["reader_state"].get(field) for field in ("comic_id", "episode_id", "revision"))
                     and value["reader_state"].get("frame_width", 0) > 0
                     and value["reader_state"].get("frame_height", 0) > 0), None)

    def current_ui(values):
        return next((value.get("state") for value in reversed(values)
                     if value.get("event") == "ui-state" and value.get("process_id") == active_pid), {})

    def next_page(before, process):
        state = before["reader_state"]
        direction = state.get("direction", "ltr")
        new_records_start = len(trace())
        tap(280 if direction == "rtl" else 1580, 1240)
        after = wait_for(lambda values: (value if (value := reader_ready(values[new_records_start:]))
                         and value["reader_state"]["page_index"] == state["page_index"] + 1 else None),
                         "a real native page turn to the next source page", process)
        assert after["reader_state"]["page_id"] != state["page_id"], (before, after)
        for field in READER_IDENTITY_FIELDS:
            assert after["reader_state"][field] == state[field], (field, before, after)
        return after

    def select_artwork_page(before, process):
        # Pages 18 and 19 have distinct bundled artwork; later pages substitute 18.
        state = before["reader_state"]
        for _ in range(10):
            if state["page_index"] == 17:
                return before
            previous_index = state["page_index"]
            intent_next = previous_index < 17
            direction = state.get("direction", "ltr")
            on_right = intent_next == (direction != "rtl")
            new_records_start = len(trace())
            tap(1580 if on_right else 280, 1240)
            target = previous_index + (1 if intent_next else -1)
            moved = wait_for(lambda values: (value if (value := reader_ready(values[new_records_start:]))
                            and value["reader_state"]["page_index"] == target else None),
                            "the known-artwork source page", process)
            for field in READER_IDENTITY_FIELDS:
                assert moved["reader_state"][field] == state[field], (field, before, moved)
            before, state = moved, moved["reader_state"]
        raise AssertionError("The known artwork page was not reached within ten native taps.")

    def save():
        (output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
        for record in report["cases"]:
            (output / record["case"] / "result.json").write_text(json.dumps(record, indent=2) + "\n")

    firmware = guest("cat", "/etc/prettyversion.txt").stdout.decode().strip()
    assert firmware.startswith("Kindle 5.20.1 "), firmware
    report["firmware_identity"] = firmware
    report["guest_preflight"] = guest_shell(
        "uname -a; cat /sys/class/graphics/fb0/virtual_size /sys/class/graphics/fb0/stride "
        "/sys/class/graphics/fb0/bits_per_pixel /sys/class/graphics/fb0/rotate; cat /proc/meminfo"
    ).stdout.decode()

    stage = output / "package"
    with zipfile.ZipFile(args.package) as archive:
        for info in archive.infolist():
            path = PurePosixPath(info.filename)
            if path.is_absolute() or ".." in path.parts or info.is_dir():
                raise RuntimeError(f"Unexpected package path: {info.filename}")
            if (info.external_attr >> 16) & 0o170000 not in (0, 0o100000):
                raise RuntimeError(f"Nonregular package entry: {info.filename}")
            target = stage.joinpath(*path.parts)
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(archive.read(info))
            target.chmod((info.external_attr >> 16) & 0o777 or 0o644)
    binary = stage / "bilicomics/app/bilicomics"
    binary_digest = sha256(binary)
    binary_name = "bilicomics-" + binary_digest[:12]
    launcher = stage / "bilicomics/launch.sh"
    report.update({"binary_sha256": binary_digest, "binary_name": binary_name,
                   "launcher_sha256": sha256(launcher), "wrapper_sha256": sha256(args.wrapper)})
    if args.deploy:
        guest("test", "!", "-d", LOCK)
        binary.rename(binary.with_name(binary_name))
        shutil.copy2(args.wrapper, binary)
        binary.chmod(0o755)
        guest("mkdir", "-p", GUEST_ROOT + "/app", LOGS)
        scp = ["scp", "-O", "-i", str(emulator / "build/ssh/id_ed25519"),
               "-P", str(environment["ports"]["ssh"]), "-o", "BatchMode=yes",
               "-o", "UserKnownHostsFile=" + str(workspace / "known-hosts"),
               "-o", "HostKeyAlias=bili-s1"]
        subprocess.run([*scp, "-r", str(stage / "bilicomics/app"),
                        "root@127.0.0.1:" + GUEST_ROOT + "/"], check=True, capture_output=True)
        subprocess.run([*scp, str(launcher), "root@127.0.0.1:" + GUEST_ROOT + "/launch.sh"],
                       check=True, capture_output=True)
        for filename, text in (("device-profile", "scribe\n"), ("device-allowlist", "scribe 5.20.1\n"),
                               ("logs/test-binary", binary_name + "\n")):
            guest("tee", GUEST_ROOT + "/data/" + filename, data=text.encode())
        guest("chmod", "755", GUEST_ROOT + "/app/bilicomics", GUEST_ROOT + "/app/" + binary_name)
    expected = guest("sha256sum", GUEST_ROOT + "/app/" + binary_name).stdout.decode().split()[0]
    assert expected == binary_digest, expected
    assert guest("sha256sum", GUEST_ROOT + "/launch.sh").stdout.decode().split()[0] == report["launcher_sha256"]
    assert guest("sha256sum", GUEST_ROOT + "/app/bilicomics").stdout.decode().split()[0] == report["wrapper_sha256"]
    save()

    for case in CASES if args.case == "all" else (args.case,):
        directory = output / case
        directory.mkdir()
        record = {"case": case, "result": "running"}
        report["cases"].append(record)
        save()
        process = None
        started = time.monotonic()
        stop_sampling = threading.Event()
        samples = []
        sampling_errors = []
        sampler = None
        startup_observer = None
        stop_startup_observer = threading.Event()
        native_startup = []
        native_startup_errors = []
        failure = None
        try:
            record["native_before"] = restored()
            guest("tee", LOGS + "/test-case", data=(case + "\n").encode())
            guest("rm", "-f", *(LOGS + "/" + name for name in (
                "lifecycle.jsonl", "native.pid", "application.log", "final-native.png", "final-native.json",
                "startup-0.png", "startup-0.json", "startup-1.png", "startup-1.json")))
            with (directory / "launcher.log").open("wb") as stream:
                process = subprocess.Popen([str(helper), f"BILICOMICS_HOME={GUEST_ROOT} DISPLAY=:0.0 sh {GUEST_ROOT}/launch.sh --detached"],
                                           stdout=stream, stderr=subprocess.STDOUT)
            if case.startswith("startup-"):
                def observe_native_startup():
                    for sequence in range(180):
                        if stop_startup_observer.is_set():
                            break
                        try:
                            path = capture(directory, f"qmp-startup-{sequence:03d}")
                            with Image.open(path) as image:
                                gray = image.convert("L")
                                activity = []
                                for index in range(4):
                                    box = (818 + index * 64, 1370, 848 + index * 64, 1380)
                                    values = gray.crop(box).tobytes()
                                    activity.append(sum(value < 64 for value in values) / len(values))
                                active = [index for index, ratio in enumerate(activity) if ratio > 0.9]
                                title_values = gray.crop((500, 940, 1360, 1060)).tobytes()
                                title_present = sum(value < 64 for value in title_values) > 100
                                if title_present and len(active) == 1 and all(ratio < 0.1 for index, ratio in enumerate(activity) if index != active[0]):
                                    native_startup.append({"path": path.name, "active_segment": active[0],
                                                           "activity_black_ratios": activity,
                                                           "elapsed_seconds": round(time.monotonic() - started, 3)})
                        except Exception as error:
                            native_startup_errors.append(repr(error))
                            break
                        stop_startup_observer.wait(0.5)
                startup_observer = threading.Thread(target=observe_native_startup, daemon=True)
                startup_observer.start()
            deadline = time.monotonic() + 40
            pid = ""
            while time.monotonic() < deadline:
                result = guest("cat", LOGS + "/native.pid", check=False)
                pid = result.stdout.decode().strip()
                if result.returncode == 0 and pid.isdecimal():
                    break
                if process.poll() is not None:
                    raise RuntimeError(f"Exited before native PID: {process.returncode}")
                time.sleep(0.25)
            assert pid.isdecimal(), "Native PID unavailable"
            record["native_pid"] = int(pid)
            active_pid = int(pid)
            identity_deadline = time.monotonic() + 20
            while True:
                try:
                    native_identity = process_identity(pid)
                    break
                except AssertionError:
                    if time.monotonic() >= identity_deadline:
                        raise
                    time.sleep(0.2)
            record["native_process_identity"] = native_identity

            def sample_memory():
                with (directory / "memory.jsonl").open("w") as stream:
                    while not stop_sampling.is_set():
                        try:
                            result = guest("cat", f"/proc/{pid}/status", "/proc/meminfo", check=False)
                        except Exception as error:
                            sampling_errors.append(repr(error))
                            stream.write(json.dumps({"sampling_error": repr(error)}) + "\n")
                            break
                        if result.returncode:
                            break
                        fields = {"elapsed_seconds": round(time.monotonic() - started, 3)}
                        for line in result.stdout.decode().splitlines():
                            key, _, value = line.partition(":")
                            if key in {"VmRSS", "VmHWM", "MemAvailable"}:
                                fields[key] = int(value.split()[0])
                        samples.append(fields)
                        stream.write(json.dumps(fields) + "\n")
                        stream.flush()
                        stop_sampling.wait(0.4)

            sampler = threading.Thread(target=sample_memory, daemon=True)
            sampler.start()
            if case.startswith("startup-"):
                wait_for(lambda values: next((value for value in values if value.get("event") == "ready" and value.get("process_id") == active_pid), None),
                         "startup completion", process)
                capture(directory, "native-shelf")
            else:
                wait_for(lambda values: next((value for value in values if value.get("event") == "ready" and value.get("process_id") == active_pid), None),
                         "native startup", process)
                capture(directory, "native-shelf")
                if case == "interactive":
                    tap(1628, 2400)
                    wait_for(lambda values: current_ui(values).get("route") == "account", "native Account navigation", process)
                    capture(directory, "native-account")
                    tap(930, 2176)
                elif case in ("reader", "sleep"):
                    if case == "sleep":
                        tap(1628, 2400)
                        wait_for(lambda values: current_ui(values).get("route") == "account", "Account before the launch preference", process)
                        tap(1346, 318)
                        time.sleep(0.8)
                        capture(directory, "native-system-settings")
                        tap(1346, 592)
                        time.sleep(0.8)
                        capture(directory, "native-launch-shelf-selection")
                        tap(232, 2400)
                        wait_for(lambda values: current_ui(values).get("route") == "shelf", "Shelf after the launch preference", process)
                    tap(828, 762)
                    before = wait_for(reader_ready, "native Reader navigation", process)
                    before = select_artwork_page(before, process)
                    initial_reader_frame = capture_artwork(directory, "native-reader", 18, record)
                    before = next_page(before, process)
                    record["identity_after_reader_turn"] = process_identity(pid)
                    assert record["identity_after_reader_turn"] == native_identity
                    retained_reader_frame = capture_artwork(directory, "native-reader-next", 19, record)
                    assert body_hash(retained_reader_frame) != body_hash(initial_reader_frame), "The distinct source-page artwork did not change."
                    record["reader_before"] = before
                    if case == "sleep":
                        assert before["reader_state"]["page_index"] > 0, before
                        assert before["reader_state"]["launch_preference"] == "shelf", before
                        record["identity_before_suspend"] = process_identity(pid)
                        assert record["identity_before_suspend"] == native_identity
                        guest("lipc-set-prop", "com.lab126.powerd", "powerButton", "1")
                        suspended = wait_for(lambda values: next((value for value in reversed(values)
                            if value.get("event") == "suspend" and value.get("process_id") == active_pid), None), "native screensaver suspend", process)
                        assert suspended["process_id"] == int(pid) and suspended["suspended"] is True, suspended
                        guest("kill", "-0", pid)
                        record["identity_during_suspend"] = process_identity(pid)
                        assert record["identity_during_suspend"] == native_identity
                        record["suspend"] = suspended
                        record["powerd_during_sleep"] = guest("lipc-get-prop", "com.lab126.powerd", "state").stdout.decode().strip()
                        time.sleep(1)
                        capture(directory, "native-screensaver")
                        guest("lipc-set-prop", "com.lab126.powerd", "wakeUp", "1")
                        resumed = wait_for(lambda values: next((value for value in reversed(values)
                            if value.get("event") == "resume" and value.get("process_id") == active_pid), None), "native unlock and retained-reader resume", process)
                        assert resumed["process_id"] == int(pid) and resumed["suspended"] is False, resumed
                        assert resumed["route"] == "reader", resumed
                        assert resumed["reader_state"] == suspended["reader_state"] == before["reader_state"], (before, suspended, resumed)
                        record["resume"] = resumed
                        record["identity_after_resume"] = process_identity(pid)
                        assert record["identity_after_resume"] == native_identity
                        resumed_frame = capture_artwork(directory, "native-reader-resumed", 19, record)
                        assert body_hash(resumed_frame) == body_hash(retained_reader_frame), "The retained Reader body differs after wake."
                        record["retained_body_roi_pixel_difference"] = 0
                        after = next_page(before, process)
                        assert after["process_id"] == int(pid), after
                        record["reader_after_wake_turn"] = after
                        record["identity_after_wake_turn"] = process_identity(pid)
                        assert record["identity_after_wake_turn"] == native_identity
                        next_frame = capture_artwork(directory, "native-reader-wake-next", 20, record)
                        assert body_hash(next_frame) != body_hash(retained_reader_frame), "The native artwork did not change after the wake page turn."
                    guest("kill", "-TERM", pid)
                else:
                    launcher_pid = read_pid(LOCK + "/pid")
                    guest("kill", "-TERM", launcher_pid)
            status = process.wait(timeout=55)
            record["exit_code"] = status
            assert status == (143 if case == "recovery" else 0), status
            ended = guest("kill", "-0", pid, check=False)
            assert ended.returncode != 0, "The application process survived the completed launcher."
            record["native_process_ended"] = True
            record["native_after"] = restored()
            capture(directory, "native-restored-ui")
            if case != "recovery":
                metadata = json.loads(guest("cat", LOGS + "/final-native.json").stdout)
                assert metadata["process_id"] == int(pid), metadata
                assert metadata["backend"] == "kindle" and metadata["profile"] == "scribe", metadata
                assert metadata["demo"] is True and metadata["offline"] is True, metadata
                assert metadata["logical_size"] == [930, 1240], metadata
                assert metadata["window_size"] == metadata["snapshot_size"] == [1860, 2480], metadata
                expected_route = {"interactive": "account", "reader": "reader", "sleep": "reader"}.get(case, "shelf")
                assert metadata["route"] == expected_route, metadata
                record["metadata"] = metadata
            if case.startswith("startup-"):
                frames = []
                for index in (0, 1):
                    meta = json.loads(guest("cat", LOGS + f"/startup-{index}.json").stdout)
                    assert meta["booting"] is True and meta["error"] is False, meta
                    assert meta["process_id"] == int(pid), meta
                    frames.append(meta)
                    (directory / f"startup-{index}.json").write_text(json.dumps(meta, indent=2) + "\n")
                    (directory / f"startup-{index}.png").write_bytes(guest("cat", LOGS + f"/startup-{index}.png").stdout)
                assert frames[0]["animation_frame"] != frames[1]["animation_frame"], frames
                stop_startup_observer.set()
                if startup_observer is not None:
                    startup_observer.join(timeout=30)
                record["native_startup_observations"] = native_startup
                record["native_startup_errors"] = native_startup_errors
                assert not native_startup_errors, native_startup_errors
                assert len({value["active_segment"] for value in native_startup}) >= 2, native_startup
                with Image.open(directory / "startup-0.png") as first, Image.open(directory / "startup-1.png") as second:
                    difference = ImageChops.difference(first.convert("RGB"), second.convert("RGB"))
                    assert difference.getbbox() is not None, "Startup frames have identical pixels"
                    record["startup_changed_bbox"] = difference.getbbox()
                record["startup_delay_fixture_ms"] = 1800
            record["result"] = "passed"
        except Exception as error:
            failure = error
            record["result"] = "failed"
            record["error"] = repr(error)
            try:
                capture(directory, "native-failure")
            except Exception as capture_error:
                record["failure_capture_error"] = repr(capture_error)
            if process is not None and process.poll() is None:
                try:
                    guest("kill", "-TERM", read_pid(LOCK + "/pid"), check=False)
                    process.wait(timeout=20)
                except Exception as cleanup_error:
                    record["cleanup_error"] = repr(cleanup_error)
        finally:
            stop_sampling.set()
            stop_startup_observer.set()
            if startup_observer is not None:
                startup_observer.join(timeout=30)
                record["native_startup_observations"] = native_startup
                record["native_startup_errors"] = native_startup_errors
            if sampler is not None:
                sampler.join(timeout=30)
            unfinished = [name for name, worker in (("memory sampler", sampler), ("startup observer", startup_observer))
                          if worker is not None and worker.is_alive()]
            if unfinished:
                record["thread_cleanup_error"] = unfinished
                if failure is None:
                    failure = RuntimeError(f"Background verification workers did not stop: {unfinished}")
                    record["result"] = "failed"
                    record["error"] = repr(failure)
            record["memory_observation"] = {
                "samples": len(samples),
                "observed_peak_rss_kb": max((value.get("VmRSS", 0) for value in samples), default=0),
                "observed_high_water_kb": max((value.get("VmHWM", 0) for value in samples), default=0),
                "minimum_available_kb": min((value["MemAvailable"] for value in samples if "MemAvailable" in value), default=None),
                "sampling_errors": sampling_errors,
                "coverage": "Periodic /proc sampling; not an exhaustive peak measurement or physical performance."}
            record["elapsed_seconds"] = round(time.monotonic() - started, 3)
            for name in ("application.log", "lifecycle.jsonl", "final-native.json", "final-native.png"):
                result = guest("cat", LOGS + "/" + name, check=False)
                if result.returncode == 0:
                    (directory / name).write_bytes(result.stdout)
            save()
            print(json.dumps({"case": case, "result": record["result"], "error": record.get("error")}), flush=True)
        if failure is not None:
            raise failure
    report["result"] = "passed"
    save()


if __name__ == "__main__":
    main()
