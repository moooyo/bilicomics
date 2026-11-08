#!/usr/bin/env python3
"""Prepare an isolated Kindle VM from a copied, immutable prepared base."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shlex
import shutil
import socket
import subprocess
import sys
from datetime import datetime, timezone


EMULATOR_REVISION = "c96062d51a1a54dd445d1668e740786d160a80bb"
QEMU_REVISION = "b75b93cdc82d60d8c3f844dd114fd38dfb481df5"
BASE_NAME = "kindle-scribe-1-b9198a0ce42bb2db.qcow2"
MODEL = "kindle-scribe-1"
FIRMWARE = "5.20.1"


def run(*arguments: str, **kwargs):
    return subprocess.run(arguments, check=True, capture_output=True, text=True, **kwargs)


def digest(path: Path) -> str:
    value = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(8 * 1024 * 1024), b""):
            value.update(chunk)
    return value.hexdigest()


def identity(path: Path) -> dict:
    info = path.stat()
    return {"sha256": digest(path), "size": info.st_size,
            "mode": oct(info.st_mode & 0o777), "mtime_ns": info.st_mtime_ns}


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--reference-root", type=Path, required=True)
    parser.add_argument("--workspace", type=Path, required=True)
    parser.add_argument("--machine", default="bili-s1")
    parser.add_argument("--ssh-port", type=int, default=4132)
    parser.add_argument("--wifi-ssh-port", type=int, default=4133)
    parser.add_argument("--vnc-display", type=int, default=17)
    parser.add_argument("--tcg-cache-mib", type=int, choices=(128, 256, 512), default=256)
    args = parser.parse_args()
    if sys.platform != "linux":
        raise SystemExit("Run this preparation only on the designated Linux verification host.")
    reference = args.reference_root.resolve(strict=True)
    workspace = args.workspace.resolve()
    if workspace.exists() or not str(workspace).startswith("/root/bilicomics-emulator-predevice-"):
        raise SystemExit("Use a new, isolated /root/bilicomics-emulator-predevice-* directory.")
    if not re.fullmatch(r"[A-Za-z0-9._-]+", workspace.name) or not re.fullmatch(r"[a-z0-9_-]+", args.machine):
        raise SystemExit("Workspace and machine names must contain simple path-safe characters.")
    socket_path = workspace / "emulator/machines" / args.machine / f"{args.machine}.qmp.sock"
    if len(os.fsencode(socket_path)) >= 104:
        raise SystemExit("Choose a shorter machine name so the Unix socket path fits.")
    revision = run("git", "-C", str(reference), "rev-parse", "HEAD").stdout.strip()
    qemu_revision = run("git", "-C", str(reference / "qemu"), "rev-parse", "HEAD").stdout.strip()
    if revision != EMULATOR_REVISION or qemu_revision != QEMU_REVISION:
        raise SystemExit("The emulator or QEMU revision differs from the reviewed reference.")
    source_base = reference / "build/images" / BASE_NAME
    original = identity(source_base)
    if source_base.stat().st_mode & 0o222:
        raise SystemExit("The prepared base must be immutable before copying it.")
    qemu_img = reference / "qemu/build/qemu-img"
    source_info = json.loads(run(str(qemu_img), "info", "--output=json", str(source_base)).stdout)
    if source_info.get("backing-filename"):
        raise SystemExit("Only a standalone immutable prepared base may be copied.")
    for port in (args.ssh_port, args.wifi_ssh_port, 5900 + args.vnc_display):
        with socket.socket() as connection:
            connection.bind(("127.0.0.1", port))
    memory = Path("/proc/meminfo").read_text()
    available = int(next(line.split()[1] for line in memory.splitlines() if line.startswith("MemAvailable:")))
    if available < 2 * 1024 * 1024 or shutil.disk_usage(workspace.parent).free < 3 * 1024**3:
        raise SystemExit("At least 2 GiB available memory and 3 GiB disk are required.")
    source_processes = run("ps", "-eo", "pid,args").stdout
    observed_reference = [line.strip() for line in source_processes.splitlines()
                          if "qemu-system" in line and "-name weread-" in line]
    workspace.mkdir(mode=0o700)
    emulator = workspace / "emulator"
    emulator.mkdir()
    for name in ("eink", "models.json"):
        shutil.copy2(reference / name, emulator / name)
    shutil.copytree(reference / "eink_emulator", emulator / "eink_emulator",
                    ignore=shutil.ignore_patterns("__pycache__", "*.pyc"))
    (emulator / "scripts").mkdir()
    shutil.copy2(reference / "scripts/eink-qmp.py", emulator / "scripts/eink-qmp.py")
    for name in ("firmware", "qemu"):
        (emulator / name).symlink_to(reference / name, target_is_directory=True)
    images = emulator / "build/images"
    images.mkdir(parents=True)
    copied_base = images / BASE_NAME
    run("cp", "--reflink=auto", "--sparse=always", str(source_base), str(copied_base))
    copied_base.chmod(0o444)
    shutil.copy2(source_base.with_suffix(".json"), copied_base.with_suffix(".json"))
    if digest(copied_base) != original["sha256"]:
        raise SystemExit("The independent base copy failed its digest check.")
    key_root = emulator / "build/ssh"
    key_root.mkdir(mode=0o700)
    for name in ("id_ed25519", "id_ed25519.pub"):
        shutil.copy2(reference / "build/ssh" / name, key_root / name)
    (key_root / "id_ed25519").chmod(0o600)
    machine = emulator / "machines" / args.machine
    machine.mkdir(parents=True)
    disk = machine / "disk.qcow2"
    run(str(qemu_img), "create", "-f", "qcow2", "-F", "qcow2", "-b",
        os.path.relpath(copied_base, machine), str(disk))
    base_metadata = json.loads(source_base.with_suffix(".json").read_text())
    manifest = {"format": 1, "model": MODEL, "firmware_version": FIRMWARE,
                "profile": "dvt", "name": args.machine,
                "base": str(copied_base.relative_to(emulator)),
                "firmware_fingerprint": base_metadata["fingerprint"],
                "created": datetime.now(timezone.utc).isoformat()}
    (machine / "machine.json").write_text(json.dumps(manifest, indent=2) + "\n")
    key = key_root / "id_ed25519"
    helper = workspace / "guest-ssh"
    helper.write_text("#!/bin/sh\nset -eu\nexec ssh -i '" + str(key) + "' -p " + str(args.ssh_port)
                      + " -o BatchMode=yes -o ConnectTimeout=8 -o StrictHostKeyChecking=accept-new"
                      + " -o UserKnownHostsFile='" + str(workspace / "known-hosts") + "'"
                      + " -o HostKeyAlias=" + args.machine + " root@127.0.0.1 \"$@\"\n")
    helper.chmod(0o700)
    report = {"schema_version": 1, "physical_device_accessed": False,
              "setup_harness_sha256": digest(Path(__file__)),
              "emulator_revision": revision, "qemu_revision": qemu_revision,
              "qemu_version": run(str(reference / "qemu/build/qemu-system-aarch64"), "--version").stdout.splitlines()[0],
              "source_prepared_base": str(source_base), "source_base_before": original,
              "copied_base": str(copied_base), "copied_base_identity": identity(copied_base),
              "source_guest_disk_copied": False,
              "reference_processes_before": observed_reference,
              "reference_processes_after": None,
              "model": MODEL, "firmware": FIRMWARE, "profile": "dvt",
              "ram_bytes": 1024**3, "cpus": 2,
              "ports": {"ssh": args.ssh_port, "wifi_ssh": args.wifi_ssh_port,
                        "vnc": 5900 + args.vnc_display},
              "guest_environment_deltas": ["Fresh overlay on a separately copied prepared base; no WeRead runtime data."],
              "host_environment_deltas": [f"Independent QEMU TCG translation cache limited to {args.tcg_cache_mib} MiB."]}
    (workspace / "environment.json").write_text(json.dumps(report, indent=2) + "\n")
    command = [sys.executable, str(emulator / "eink"), "run", args.machine,
               "--vnc", f"127.0.0.1:{args.vnc_display}", "--serial-socket", "--qmp-socket",
               "--ssh-port", str(args.ssh_port), "--wifi-ssh-port", str(args.wifi_ssh_port),
               "--", "-L", str(reference / "qemu/pc-bios"), "-daemonize", "-pidfile", str(workspace / "qemu.pid")]
    separator = command.index("--")
    planned = run(*command[:separator], "--dry-run", *command[separator:],
                  env={**os.environ, "PYTHONDONTWRITEBYTECODE": "1"})
    actual_command = shlex.split(planned.stdout.strip())
    accelerator = actual_command.index("-accel") + 1
    original_accelerator = actual_command[accelerator]
    assert original_accelerator == "tcg,thread=multi,tb-size=2048", original_accelerator
    actual_command[accelerator] = f"tcg,thread=multi,tb-size={args.tcg_cache_mib}"
    report["emulator_launch_command"] = command
    report["launch_command"] = actual_command
    report["tcg_cache_mib"] = args.tcg_cache_mib
    try:
        result = run(*actual_command, env={**os.environ, "PYTHONDONTWRITEBYTECODE": "1"})
    except subprocess.CalledProcessError as error:
        (workspace / "launch.log").write_text((error.stdout or "") + (error.stderr or ""))
        report["launch_error"] = {"exit_code": error.returncode, "command": actual_command}
        (workspace / "environment.json").write_text(json.dumps(report, indent=2) + "\n")
        raise
    (workspace / "launch.log").write_text(result.stdout + result.stderr)
    report["source_base_after"] = identity(source_base)
    observed_after = run("ps", "-eo", "pid,args").stdout
    report["reference_processes_after"] = [line.strip() for line in observed_after.splitlines()
                                           if "qemu-system" in line and "-name weread-" in line]
    report["qemu_pid"] = int((workspace / "qemu.pid").read_text())
    if report["source_base_after"] != report["source_base_before"]:
        raise RuntimeError("The source prepared-base identity changed during setup.")
    if report["reference_processes_after"] != observed_reference:
        raise RuntimeError("Reference VM process identity changed during setup.")
    (workspace / "environment.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({"workspace": str(workspace), "machine": args.machine,
                      "qemu_pid": report["qemu_pid"], "ports": report["ports"]}), flush=True)


if __name__ == "__main__":
    main()
