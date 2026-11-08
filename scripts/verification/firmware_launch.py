#!/usr/bin/env python3
"""Start a stopped isolated BiliComics VM with a bounded TCG cache."""

from __future__ import annotations

import argparse
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import shlex
import subprocess
import sys


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--workspace", type=Path, required=True)
    parser.add_argument("--tcg-cache-mib", type=int, choices=(128, 256, 512), default=256)
    args = parser.parse_args()
    if sys.platform != "linux":
        raise SystemExit("Run this command only on the designated Linux verification host.")
    workspace = args.workspace.resolve(strict=True)
    if not str(workspace).startswith("/root/bilicomics-emulator-predevice-"):
        raise SystemExit("The workspace is outside the isolated BiliComics test directory.")
    environment = json.loads((workspace / "environment.json").read_text())
    emulator = workspace / "emulator"
    machine = emulator / "machines/bili-s1"
    if not machine.is_dir():
        raise SystemExit("The independent bili-s1 machine is absent.")
    for process in Path("/proc").iterdir():
        if not process.name.isdecimal():
            continue
        try:
            command = (process / "cmdline").read_bytes().split(b"\0")
        except (FileNotFoundError, ProcessLookupError, PermissionError):
            continue
        if command and b"qemu-system" in command[0] and any(str(machine).encode() in item for item in command):
            raise SystemExit(f"The independent QEMU process is still alive: {process.name}")
    for suffix in ("qmp.sock", "serial.sock"):
        socket = machine / ("bili-s1." + suffix)
        if socket.exists():
            socket.unlink()
    serial_log = machine / "bili-s1.serial.log"
    if serial_log.exists():
        serial_log.rename(machine / ("bili-s1.serial.previous-" + datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%S%fZ") + ".log"))
    ports = environment["ports"]
    command = [sys.executable, str(emulator / "eink"), "run", "bili-s1", "--dry-run",
               "--vnc", f"127.0.0.1:{ports['vnc'] - 5900}", "--serial-socket", "--qmp-socket",
               "--ssh-port", str(ports["ssh"]), "--wifi-ssh-port", str(ports["wifi_ssh"]), "--",
               "-L", str((emulator / "qemu/pc-bios").resolve()), "-daemonize",
               "-pidfile", str(workspace / "qemu.pid")]
    planned = subprocess.run(command, check=True, capture_output=True, text=True,
                             env={**os.environ, "PYTHONDONTWRITEBYTECODE": "1"})
    actual = shlex.split(planned.stdout.strip())
    accelerator = actual.index("-accel") + 1
    assert actual[accelerator] == "tcg,thread=multi,tb-size=2048", actual[accelerator]
    actual[accelerator] = f"tcg,thread=multi,tb-size={args.tcg_cache_mib}"
    record = {"started_at_utc": datetime.now(timezone.utc).isoformat(),
              "command": actual, "tcg_cache_mib": args.tcg_cache_mib,
              "host_boot_id": Path("/proc/sys/kernel/random/boot_id").read_text().strip(),
              "guest_environment_modified": False, "physical_device_accessed": False}
    result = subprocess.run(actual, capture_output=True, text=True)
    record.update({"exit_code": result.returncode, "stdout": result.stdout, "stderr": result.stderr})
    if result.returncode == 0:
        record["qemu_pid"] = int((workspace / "qemu.pid").read_text())
    with (workspace / "host-launches.jsonl").open("a") as output:
        output.write(json.dumps(record) + "\n")
    print(json.dumps(record), flush=True)
    if result.returncode:
        raise SystemExit(result.returncode)


if __name__ == "__main__":
    main()
