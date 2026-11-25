#!/usr/bin/env python3
"""Drive the redesigned reader in two disposable, network-isolated Kindle VMs."""

import argparse
import json
import os
from pathlib import Path
import re
import shlex
import shutil
import socket
import subprocess
import sys
import time

from PIL import Image


# Leave the stock guest's noexec userstore mount unchanged.
GUEST = "/var/local/bilicomics-handoff"
LOCK = "/tmp/bilicomics-launcher.lock"
FIRMWARE = "5.20.1"
SSH_PORT = 4122
# Native logical viewports at 2x, not the desktop design canvases.
PROFILES = {
    "scribe": ("kindle-scribe-1", "dvt", 930, 1240),
    "pw6": ("kindle-paperwhite-6", "production", 636, 848),
}


def wait_for(test, label, timeout):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        value = test()
        if value:
            return value
        time.sleep(0.45)
    raise TimeoutError(label)


def prepare(reference, work, profile):
    model, variant, _, _ = PROFILES[profile]
    vm = work / "vm"
    machine = vm / "machines" / profile
    socket_path = machine / (profile + ".qmp.sock")
    if len(os.fsencode(socket_path)) >= 108:
        raise RuntimeError("Use a shorter output directory for the Linux QMP socket")
    machine.mkdir(parents=True)
    for path in ("eink", "models.json", "scripts/eink-qmp.py"):
        (vm / path).parent.mkdir(exist_ok=True)
        shutil.copy2(reference / path, vm / path)
    shutil.copytree(reference / "eink_emulator", vm / "eink_emulator",
                    ignore=shutil.ignore_patterns("__pycache__", "*.pyc"))
    for path in ("firmware", "qemu", "build/ssh"):
        (vm / path).parent.mkdir(exist_ok=True)
        (vm / path).symlink_to(reference / path, target_is_directory=True)
    base, = (reference / "build/images").glob(model + "-*.qcow2")
    if base.stat().st_mode & 0o222:
        raise RuntimeError("The prepared base must not be writable")
    (vm / "build/images").mkdir()
    (vm / "build/images" / base.name).symlink_to(base)
    qemu = reference / "qemu/build"
    subprocess.run([qemu / "qemu-img", "create", "-f", "qcow2", "-F", "qcow2",
                    "-b", base, machine / "disk.qcow2"], check=True, capture_output=True)
    metadata = json.loads(base.with_suffix(".json").read_text())
    (machine / "machine.json").write_text(json.dumps({
        "format": 1, "name": profile, "model": model, "profile": variant,
        "firmware_version": FIRMWARE, "firmware_fingerprint": metadata["fingerprint"],
        "base": "build/images/" + base.name,
    }))
    planned = subprocess.check_output([
        sys.executable, "-B", vm / "eink", "run", profile, "--dry-run", "--vnc", "127.0.0.1:29",
        "--qmp-socket", "--ssh-port", str(SSH_PORT), "--wifi-ssh-port", str(SSH_PORT + 1),
        "--", "-L", reference / "qemu/pc-bios",
    ], text=True)
    command = shlex.split(planned)
    if Path(command[0]).resolve() != (qemu / "qemu-system-aarch64").resolve():
        raise RuntimeError("Unexpected emulator executable")
    command[command.index("-accel") + 1] = "tcg,thread=multi,tb-size=128"
    if profile == "scribe":
        # The prepared guest has no production Scribe ID. This is emulator-only.
        command[command.index("-machine") + 1] += ",idme-serial=G0023L0000000000"
    return vm, command


def verify(args, packed, profile):
    work = args.output / profile
    work.mkdir(mode=0o700)
    result = {"profile": profile, "physical_device": False, "live_account": False,
              "synthetic_model_identity": profile == "scribe", "passed": False,
              "binary_bytes": args.binary.stat().st_size, "compressed_bytes": packed.stat().st_size,
              "sleep_requested": args.check_sleep}
    _, _, width, height = PROFILES[profile]
    result["logical_viewport"] = [width, height]
    result["expected_framebuffer"] = [width * 2, height * 2]
    ssh = ["ssh", "-F", "/dev/null", "-i", str(args.emulator / "build/ssh/id_ed25519"),
           "-p", str(SSH_PORT), "-o", "IdentitiesOnly=yes", "-o", "BatchMode=yes",
           "-o", "ConnectTimeout=5", "-o", "StrictHostKeyChecking=accept-new",
           "-o", f"UserKnownHostsFile={work / 'known-hosts'}", "root@127.0.0.1"]
    vm_process = launcher = None

    def guest(command, *, data=None, stdin=None, timeout=25, check=True):
        return subprocess.run([*ssh, command], input=data, stdin=stdin, capture_output=True,
                              timeout=timeout, check=check)

    try:
        vm, command = prepare(args.emulator, work, profile)
        with (work / "qemu.log").open("wb") as log:
            vm_process = subprocess.Popen(command, stdin=subprocess.DEVNULL, stdout=log, stderr=log)
        print(f"{profile}: booting fresh firmware overlay", flush=True)

        def native_status():
            if vm_process.poll() is not None:
                raise RuntimeError(f"QEMU exited ({vm_process.returncode})")
            reply = guest(f"test ! -e {LOCK} && status statusbar && pidof awesome", check=False, timeout=12)
            if reply.returncode != 0 or b"start/running" not in reply.stdout:
                return None
            pids = reply.stdout.splitlines()[-1].split()
            if not pids or any(not pid.isdigit() for pid in pids):
                return None
            states = guest("; ".join(f"grep '^State:' /proc/{int(pid)}/status" for pid in pids),
                           check=False, timeout=12)
            lines = states.stdout.splitlines()
            if states.returncode or len(lines) != len(pids) or any(row.split()[1] in (b"T", b"t") for row in lines):
                return None
            return True

        def boot_ready():
            try:
                return native_status()
            except subprocess.TimeoutExpired:
                return None

        wait_for(boot_ready, "Native statusbar and window manager did not start", 320)
        version = guest("cat /etc/prettyversion.txt").stdout.decode().strip()
        observed_version = re.match(r"^Kindle\s+([0-9.]+)\b", version)
        if not observed_version or observed_version[1] != FIRMWARE:
            raise RuntimeError("Unexpected firmware: " + version)
        boot = guest("cat /proc/sys/kernel/random/boot_id").stdout
        result["firmware"] = version
        guest(f"umask 077; mkdir -p {GUEST}/app {GUEST}/data/logs")
        print(f"{profile}: native shell is running; deploying {packed.stat().st_size} compressed bytes", flush=True)

        with packed.open("rb") as source:
            guest(f"set -eu; umask 077; gzip -dc > {GUEST}/app/bilicomics-real; "
                  f"chmod 700 {GUEST}/app/bilicomics-real", stdin=source, timeout=360)
        size = int(guest(f"stat -c %s {GUEST}/app/bilicomics-real").stdout)
        if size != result["binary_bytes"]:
            raise RuntimeError(f"Unexpected guest executable size: {size}")
        result["deployed"] = True
        for path, data, mode in (
            ("launch.sh", args.launcher.read_bytes(), "700"),
            ("data/device-profile", (profile + "\n").encode(), "600"),
            ("data/device-allowlist", (profile + " " + FIRMWARE + "\n").encode(), "600"),
            ("app/bilicomics", f'''#!/bin/sh
for argument in "$@"; do
    if [ "$argument" = --data-dir ]; then
        exec {GUEST}/app/bilicomics-real "$@" --demo --offline --screen shelf \\
            --lifecycle-trace {GUEST}/data/logs/run.jsonl --exit-after-ms 210000
    fi
done
if [ "${{1:-}}" = --capture-native-display ]; then
    exec {GUEST}/app/bilicomics-real "$@" 2>>{GUEST}/data/logs/display-helper.log
fi
exec {GUEST}/app/bilicomics-real "$@"
'''.encode(), "700"),
        ):
            guest(f"umask 077; cat > {GUEST}/{path} && chmod {mode} {GUEST}/{path}", data=data)

        preflight = guest(f"grep -E ' /(mnt/us|tmp|var/local) ' /proc/mounts; {GUEST}/app/bilicomics --version",
                          check=False)
        (work / "executable-preflight.log").write_bytes(preflight.stdout + preflight.stderr)
        if preflight.returncode or not any(line.startswith(b"bilicomics ") for line in preflight.stdout.splitlines()):
            raise RuntimeError("The guest could not execute the deployed ARM candidate")
        with (work / "launcher.log").open("wb") as log:
            launcher = subprocess.Popen([
                *ssh, f"BILICOMICS_HOME={GUEST} BILICOMICS_LAUNCH_TRACE=1 DISPLAY=:0.0 sh {GUEST}/launch.sh --detached",
            ], stdout=log, stderr=subprocess.STDOUT, stdin=subprocess.DEVNULL)
        print(f"{profile}: native launcher started", flush=True)

        def trace():
            output = guest(f"cat {GUEST}/data/logs/run.jsonl", check=False).stdout
            return [json.loads(line) for line in output.splitlines(keepends=True) if line.endswith(b"\n")]

        def state(label, predicate, timeout=45):
            def observe():
                records = trace()
                latest = next((row for row in reversed(records) if row.get("event") == "ui-state"), None)
                if launcher.poll() is not None:
                    raise RuntimeError(f"Launcher exited before {label} ({launcher.returncode})")
                return latest if latest and predicate(latest["state"]) else None
            observed = wait_for(observe, label, timeout)
            result.setdefault("verified_states", []).append(label)
            return observed

        def qmp(*arguments):
            subprocess.run([sys.executable, "-B", vm / "scripts/eink-qmp.py", "--machine", profile, *map(str, arguments)],
                           check=True, capture_output=True, timeout=30)

        def tap(x, y):
            qmp("tap", round(x * 2), round(y * 2), "--hold", 0.12)

        previous = None

        def capture(name):
            nonlocal previous
            scratch = work / "sample.png"
            prior = None

            def stable():
                nonlocal prior
                qmp("screendump", scratch)
                with Image.open(scratch) as image:
                    if image.size != (width * 2, height * 2):
                        raise RuntimeError(f"Unexpected QMP geometry: {image.size}")
                    gray = image.convert("L")
                    pixels = gray.tobytes()
                    if gray.getextrema() == (255, 255) or gray.getextrema() == (0, 0) or pixels == previous:
                        prior = None
                        return None
                if pixels == prior:
                    return pixels
                prior = pixels
                return None

            previous = wait_for(stable, f"Stable changed QMP frame: {name}", 35)
            scratch.rename(work / (name + ".png"))
            result.setdefault("captures", []).append(name)

        wide = profile == "scribe"
        shelf = state("Shelf", lambda s: s["route"] == "shelf", 90)
        capture("shelf")
        tap(100 if wide else 36, 130 if wide else 64)
        reading = lambda s: s["route"] == "reader" and not s["reader"]["loading"] and s["reader"]["page"] == 18
        anchor = state("Reader page 18", reading)
        capture("reader")
        tap(width / 2, height / 2)
        state("Reader menu", lambda s: reading(s) and s["reader"]["menu"])
        tap(width / 8, height - 42)
        state("Full-screen directory", lambda s: reading(s) and s["reader"]["sheet"] == "toc")
        capture("directory")
        tap(45, 55)
        returned = state("Directory return to page 18", lambda s: reading(s) and not s["reader"]["menu"]
                         and not s["reader"]["sheet"])
        if not shelf["process_id"] == anchor["process_id"] == returned["process_id"]:
            raise RuntimeError("Directory return changed the application process")
        capture("reader-return")
        if args.check_sleep:
            def power_event(name):
                def observed():
                    if launcher.poll() is not None:
                        raise RuntimeError(f"Launcher exited during {name} ({launcher.returncode})")
                    return next((row for row in trace() if row.get("event") == name), None)
                return wait_for(observed, f"Native {name}", 45)

            guest("lipc-set-prop com.lab126.powerd powerButton 1")
            suspended = power_event("suspend")
            capture("sleep")
            guest("lipc-set-prop com.lab126.powerd wakeUp 1")
            resumed = power_event("resume")
            anchor_keys = ("comic_id", "episode_id", "reader_generation", "revision", "page_id", "page_index")
            if (suspended["process_id"] != shelf["process_id"] or resumed["process_id"] != shelf["process_id"]
                    or not suspended["suspended"] or resumed["suspended"]
                    or not suspended["checkpoint_saved"] or not resumed["checkpoint_saved"]
                    or resumed["route"] != "reader" or resumed["page"] != 18
                    or any(suspended["reader_state"][key] != resumed["reader_state"][key] for key in anchor_keys)):
                raise RuntimeError("Native wake did not preserve the live reader and source anchor")
            capture("reader-resumed")
            with Image.open(work / "reader-return.png") as before, Image.open(work / "reader-resumed.png") as after:
                if before.tobytes() != after.tobytes():
                    raise RuntimeError("Native wake did not restore the same reader pixels")
            result["sleep_wake_same_reader"] = True
        tap(width / 2, height / 2)
        state("Reader menu after return", lambda s: reading(s) and s["reader"]["menu"])
        tap(width * 3 / 8, height - 42)
        state("Reader settings", lambda s: reading(s) and s["reader"]["sheet"] == "display")
        capture("reader-settings")
        tap(width - (92 if wide else 46), height - (610 if wide else 376) + 29)
        state("Reader settings dismissed", lambda s: reading(s) and not s["reader"]["sheet"])
        tap(44, 52)
        state("Back to Shelf", lambda s: s["route"] == "shelf")
        tap(width * 7 / 8, height - 25)
        state("Account", lambda s: s["route"] == "account")
        tap(105 if wide else width - 66, height - 106 if wide else 70)
        launcher.wait(timeout=45)
        result["launcher_exit"] = launcher.returncode
        result["native_restored"] = bool(wait_for(boot_ready, "Native shell not restored", 40)) and (
            guest("cat /proc/sys/kernel/random/boot_id").stdout == boot)
        result["passed"] = result["native_restored"] and launcher.returncode == 0
        if not result["passed"]:
            result["error"] = "The guest did not exit cleanly and restore native control on the same boot"
    except (RuntimeError, subprocess.SubprocessError, OSError, ValueError, TypeError) as error:
        result["error"] = str(error)
        if getattr(error, "stderr", None):
            result["stderr"] = error.stderr.decode(errors="replace")[-2000:]
    finally:
        if vm_process is not None:
            if launcher is not None:
                for name in ("application.log", "launcher.log", "display-helper.log", "launcher-timing.tsv", "run.jsonl"):
                    try:
                        log = guest(f"cat {GUEST}/data/logs/{name}", check=False, timeout=8)
                        if log.returncode == 0:
                            (work / ("guest-" + name)).write_bytes(log.stdout)
                    except subprocess.SubprocessError:
                        pass
                stop(launcher)
            stop(vm_process)
            result["private_qemu_stopped"] = vm_process.poll() is not None
        (work / "result.json").write_text(json.dumps(result, indent=2) + "\n")
        print(json.dumps(result), flush=True)
    return result


def stop(process):
    if process.poll() is None:
        process.terminate()
        try:
            process.wait(timeout=12)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--emulator", required=True, type=Path)
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--launcher", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--profiles", nargs="+", choices=tuple(PROFILES), default=list(PROFILES))
    parser.add_argument("--check-sleep", action="store_true",
                        help="Also drive the guest powerd sleep/wake round trip")
    args = parser.parse_args()
    if len(set(args.profiles)) != len(args.profiles):
        parser.error("Each profile may appear only once per output directory")
    if sys.platform != "linux" or {name for _, name in socket.if_nameindex()} != {"lo"}:
        parser.error("Run inside a private test-env network namespace with only loopback")
    for key in ("emulator", "binary", "launcher"):
        setattr(args, key, getattr(args, key).resolve(strict=True))
    args.output = args.output.resolve()
    args.output.mkdir(mode=0o700)
    packed = args.output / "app.gz"
    try:
        with packed.open("xb") as stream:
            subprocess.run(["gzip", "-1", "-c", args.binary], stdout=stream, check=True)
        results = [verify(args, packed, profile) for profile in args.profiles]
    finally:
        packed.unlink(missing_ok=True)
    (args.output / "results.json").write_text(json.dumps(results, indent=2) + "\n")
    return int(not all(row["passed"] for row in results))


if __name__ == "__main__":
    raise SystemExit(main())
