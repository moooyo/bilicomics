"""Run one read-only inventory from the test-env host under its exact shared Kindle lease."""

import argparse
from datetime import datetime, timezone
import getpass
import json
import os
from pathlib import Path
import re
import secrets
import socket
import stat
import subprocess

import device_lease


def require(condition, reason):
    if not condition:
        raise ValueError(reason)


def save(path, data):
    with path.open("x", encoding="utf-8") as output:
        json.dump(data, output, indent=2)
        output.write("\n")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--expected-host", required=True)
    args = parser.parse_args()
    os.umask(0o077)
    owner = os.environ.get("CODEX_THREAD_ID", "")
    require(re.fullmatch(r"[a-f0-9]{8}(?:-[a-f0-9]{4}){3}-[a-f0-9]{12}", owner), "Forward the receiving conversation ID")
    root = Path(__file__).resolve().parent
    require(socket.gethostname().split(".")[0] == "test-env" and os.geteuid() == 0
            and Path("/var/tmp/kindle-build-coordination").is_dir() and root == Path.cwd()
            and root.stat().st_uid == 0 and stat.S_IMODE(root.stat().st_mode) == 0o700, "Use a private directory on the test-env host")
    config = Path("/root/.config/kindle-physical/ssh_config")
    trust = config.with_name("known_hosts")
    for path in (config, trust, config.parent):
        require(path.resolve(strict=True) == path and path.stat().st_uid == 0 and not path.stat().st_mode & 0o022,
                "Unsafe shared SSH configuration")
    destination = "kindle-physical"
    ssh = ["ssh", "-T", "-F", str(config), "-o", "StrictHostKeyChecking=yes", "-o", f"UserKnownHostsFile={trust}",
           "-o", "GlobalKnownHostsFile=/dev/null", "-o", "UpdateHostKeys=no", "-o", "ControlMaster=no",
           "-o", "ControlPath=none", "-o", "ControlPersist=no", "-o", "NumberOfPasswordPrompts=1",
           "-o", "ConnectTimeout=5", "-o", "ConnectionAttempts=1", "-o", "ClearAllForwardings=yes",
           "-o", "ForwardAgent=no", "-o", "SendEnv=-*"]
    options = dict(line.split(" ", 1) for line in subprocess.check_output(
        [*ssh, "-G", destination], text=True, stderr=subprocess.DEVNULL).splitlines())
    require(options.get("hostname") == args.expected_host and options.get("user") == "root" and options.get("port") == "22",
            "The pinned alias does not match the operator's endpoint")
    probe = (root / "native-inventory.sh").read_bytes()
    run = "bili-inv-" + secrets.token_hex(4)
    batch = root / run
    batch.mkdir(mode=0o700)
    askpass = batch / "askpass"
    askpass.write_text('#!/bin/sh\ncase "$1" in *assword:*) printf "%s\\n" "$BILI_ASKPASS";; *) exit 1;; esac\n')
    askpass.chmod(0o700)
    request = device_lease.LeaseRequest(Path("/tmp/codex-kindle-physical-test.lock"), batch / "private-token", owner, run)
    password = getpass.getpass("Kindle SSH password (not saved): ")
    require(password and "\n" not in password, "A single password is required")
    acquired = device_lease.acquire(request)
    try:
        save(batch / "acquired.json", acquired)
        outcome = subprocess.run([*ssh, destination, "sh -s"], input=probe, capture_output=True, timeout=30,
                                 env={"PATH": os.defpath, "HOME": "/root", "LC_ALL": "C", "DISPLAY": ":askpass",
                                      "SSH_ASKPASS": str(askpass), "SSH_ASKPASS_REQUIRE": "force", "BILI_ASKPASS": password})
        require(outcome.returncode == 0, f"Read-only SSH probe failed (exit {outcome.returncode}): {outcome.stderr.decode(errors='replace')[:240]}")
        values = {}
        for line in outcome.stdout.decode("ascii").splitlines():
            key, value = line.split("=", 1)
            require(key not in values and re.fullmatch(r"[-a-zA-Z0-9_:/\.]+", key) and re.fullmatch(r"[-a-z0-9.,]+", value),
                    "Invalid bounded inventory field")
            values[key] = value
        require("catalog_selected_rows" in values and "after_lock_weread" in values, "Incomplete device inventory")
        save(batch / "inventory.json", {"recorded_utc": datetime.now(timezone.utc).isoformat(), "run": run,
                                       "mode": "read-only", "values": values})
    finally:
        del password
        released = device_lease.release(request, native_restored_and_connections_closed=True)
        save(batch / "released.json", released)
        askpass.unlink()
    print(batch / "inventory.json")


if __name__ == "__main__":
    main()
