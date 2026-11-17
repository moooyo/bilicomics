"""Exec one noninteractive device SSH command only while this host owns its lease.

This source contains no authentication material. Its private configuration,
password file, askpass helper and known-hosts file stay outside the repository.
The coordinator must close connections and restore native control before release.
"""

import argparse
import ipaddress
import json
import os
from pathlib import Path
import stat
import sys


def private_file(path, maximum):
    path = Path(path)
    if not path.is_absolute():
        raise RuntimeError("Private transport paths must be absolute")
    descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK | os.O_CLOEXEC)
    with os.fdopen(descriptor, "rb") as stream:
        metadata = os.fstat(stream.fileno())
        if not stat.S_ISREG(metadata.st_mode) or metadata.st_uid != os.geteuid() \
                or metadata.st_nlink != 1 or metadata.st_mode & 0o077:
            raise RuntimeError("Private transport input ownership or permissions are unsafe")
        value = stream.read(maximum + 1)
        if not value or len(value) > maximum:
            raise RuntimeError("Private transport input size is invalid")
        return value


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--config", type=Path, required=True)
    parser.add_argument("command")
    args = parser.parse_args()
    if sys.platform != "linux" or "\0" in args.command or len(args.command.encode()) > 1024 * 1024:
        raise RuntimeError("A bounded Linux device command is required")
    configuration = json.loads(private_file(args.config, 8192))
    tool_directory = Path(configuration["lease_tool_directory"])
    sys.path.insert(0, str(tool_directory))
    from device_lease import LeaseRequest, status
    request = LeaseRequest(Path(configuration["lease_dir"]), Path(configuration["lease_token_file"]),
                           configuration["owner_thread"], configuration["lease_run_id"])
    proof = status(request)
    if proof.get("ownership_verified") is not True:
        raise RuntimeError("The shared device lease is not owned by this coordinator")
    host = str(ipaddress.IPv4Address(configuration["host"]))
    password = Path(configuration["password_file"])
    known_hosts = Path(configuration["known_hosts"])
    askpass = Path(configuration["askpass"])
    private_file(password, 1024)
    private_file(known_hosts, 65536)
    helper_metadata = askpass.lstat()
    if not stat.S_ISREG(helper_metadata.st_mode) or helper_metadata.st_uid != os.geteuid() \
            or helper_metadata.st_nlink != 1 or helper_metadata.st_mode & 0o077 \
            or not helper_metadata.st_mode & 0o100:
        raise RuntimeError("The private askpass helper is unsafe")
    environment = dict(os.environ, SSH_ASKPASS=str(askpass), SSH_ASKPASS_REQUIRE="force",
                       DISPLAY="bilicomics-private-askpass", BILICOMICS_SSH_PASSWORD_FILE=str(password))
    command = ["/usr/bin/ssh", "-F", "/dev/null", "-T", "-o", "BatchMode=no", "-o", "StrictHostKeyChecking=yes",
               "-o", "UserKnownHostsFile=" + str(known_hosts), "-o", "GlobalKnownHostsFile=/dev/null",
               "-o", "ConnectTimeout=10", "-o", "ServerAliveInterval=5", "-o", "ServerAliveCountMax=2",
               "-o", "ControlMaster=no", "-o", "ControlPersist=no", "-o", "ControlPath=none",
               "-o", "NumberOfPasswordPrompts=1", "root@" + host, args.command]
    # Exec preserves the process that physical_device.py owns and times out.
    # There is no detached SSH worker or multiplexed connection after it exits.
    if os.getpgrp() != os.getpid():
        os.setsid()
    os.execve(command[0], command, environment)


if __name__ == "__main__":
    try:
        main()
    except Exception:
        print("The guarded device transport refused this command.", file=sys.stderr)
        raise SystemExit(2)
