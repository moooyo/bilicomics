#!/usr/bin/env python3
"""Capture two same-point evdev contacts in the isolated ARM firmware guest."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import shlex
import subprocess
import sys
import time


def sha256(path: Path) -> str:
    value = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            value.update(chunk)
    return value.hexdigest()


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--workspace", type=Path, required=True)
    parser.add_argument("--probe", type=Path, required=True)
    parser.add_argument("--expected-probe-sha256", required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if sys.platform != "linux":
        raise SystemExit("Run this command only on the designated Linux verification host.")
    workspace = args.workspace.resolve(strict=True)
    if not str(workspace).startswith("/root/bilicomics-emulator-predevice-"):
        raise SystemExit("Only the isolated BiliComics firmware guest may be used.")
    if args.output.exists():
        raise SystemExit("Each diagnostic requires a new output directory.")
    args.output.mkdir(parents=True)
    digest = sha256(args.probe)
    assert digest == args.expected_probe_sha256, digest
    environment = json.loads((workspace / "environment.json").read_text())
    emulator = workspace / "emulator"
    helper = workspace / "guest-ssh"
    guest_probe = "/mnt/us/bilicomics-predevice/firmware-touch-probe-" + digest[:12]

    def guest(*command: str, check=True):
        return subprocess.run([str(helper), shlex.join(command)], capture_output=True,
                              check=check, timeout=20)

    def records(path: Path) -> list[dict]:
        return [json.loads(line) for line in path.read_text().splitlines(keepends=True)
                if line.endswith("\n") and line.strip()]

    guest("test", "!", "-d", "/tmp/bilicomics-launcher.lock")
    inputs = guest("cat", "/proc/bus/input/devices").stdout.decode()
    (args.output / "proc-input-devices.txt").write_text(inputs)
    block = next(value for value in inputs.split("\n\n") if 'Name="goodix-ts"' in value)
    assert "event3" in next(line for line in block.splitlines() if line.startswith("H:")), block
    device = "/dev/input/event3"
    scp = ["scp", "-O", "-i", str(emulator / "build/ssh/id_ed25519"),
           "-P", str(environment["ports"]["ssh"]), "-o", "BatchMode=yes",
           "-o", "UserKnownHostsFile=" + str(workspace / "known-hosts"), "-o", "HostKeyAlias=bili-s1"]
    subprocess.run([*scp, str(args.probe), "root@127.0.0.1:" + guest_probe],
                   capture_output=True, check=True, timeout=90)
    guest("chmod", "755", guest_probe)
    assert guest("sha256sum", guest_probe).stdout.decode().split()[0] == digest
    report = {"schema_version": 1, "physical_device_accessed": False,
              "application_acceptance": False, "harness_sha256": sha256(Path(__file__)),
              "probe_sha256": digest, "device": device, "coordinates": [1580, 1240],
              "scope": "Kernel MT persistence and same-point raw contact delivery; no application is running.",
              "host_boot_id": Path("/proc/sys/kernel/random/boot_id").read_text().strip(),
              "qemu_pid": int((workspace / "qemu.pid").read_text()), "runs": [], "result": "running"}
    raw_abs = next(line.split("=", 1)[1] for line in block.splitlines() if line.startswith("B: ABS="))
    raw_key = next(line.split("=", 1)[1] for line in block.splitlines() if line.startswith("B: KEY="))

    def bitmap(raw: str) -> int:
        value = 0
        for word in raw.split():
            value = (value << 32) | int(word, 16)
        return value

    abs_bits, key_bits = bitmap(raw_abs), bitmap(raw_key)
    report["capabilities"] = {"raw_abs_bitmap": raw_abs, "raw_key_bitmap": raw_key,
                              "guest_word_bits": 32, "abs_mt_slot": bool(abs_bits & (1 << 0x2f)),
                              "abs_mt_position_x": bool(abs_bits & (1 << 0x35)),
                              "abs_mt_position_y": bool(abs_bits & (1 << 0x36)),
                              "abs_mt_tracking_id": bool(abs_bits & (1 << 0x39)),
                              "btn_touch": bool(key_bits & (1 << 0x14a))}

    def save():
        (args.output / "summary.json").write_text(json.dumps(report, indent=2) + "\n")

    save()
    try:
        for number in (1, 2):
            query = guest(guest_probe, device, "1", "query")
            (args.output / f"before-open-{number}.jsonl").write_bytes(query.stdout)
            path = args.output / f"contact-{number}.jsonl"
            stderr = args.output / f"contact-{number}.stderr"
            with path.open("wb") as output, stderr.open("wb") as errors:
                process = subprocess.Popen([str(helper), shlex.join((guest_probe, device, "6", "read"))],
                                           stdout=output, stderr=errors)
                ready = None
                deadline = time.monotonic() + 10
                while time.monotonic() < deadline:
                    ready = next((value for value in records(path) if value.get("kind") == "ready"), None)
                    if ready is not None:
                        break
                    if process.poll() is not None:
                        raise RuntimeError(f"Probe exited before the exclusive grab: {process.returncode}")
                    time.sleep(0.1)
                assert ready is not None and ready["query_only"] is False, ready
                identity = guest("sh", "-c", f"cat /proc/{ready['pid']}/stat; readlink /proc/{ready['pid']}/exe").stdout.decode()
                subprocess.run([sys.executable, str(emulator / "scripts/eink-qmp.py"), "--machine", "bili-s1",
                                "tap", "1580", "1240", "--hold", "0.1"], capture_output=True, check=True, timeout=20)
                status = process.wait(timeout=15)
            assert status == 0, (status, stderr.read_text())
            data = records(path)
            assert not any(value["kind"].endswith("error") for value in data), data
            events = [value for value in data if value["kind"] == "event"]
            assert events, "The exclusively grabbed probe received no raw touch events."
            report["runs"].append({"contact": number, "probe_pid": ready["pid"], "process_identity": identity,
                                   "event_count": len(events), "position_event_codes": [value["code"] for value in events
                                   if value["type"] == 3 and value["code"] in (0x35, 0x36)],
                                   "raw_log": path.name, "exit_code": status})
            save()
        assert report["runs"][0]["probe_pid"] != report["runs"][1]["probe_pid"]
        report["second_contact_position_events_absent"] = not report["runs"][1]["position_event_codes"]
        report["result"] = "completed"
    except Exception as error:
        report["result"] = "failed"
        report["error"] = repr(error)
        raise
    finally:
        save()
        print(json.dumps(report), flush=True)


if __name__ == "__main__":
    main()
