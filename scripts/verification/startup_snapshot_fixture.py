#!/usr/bin/env python3
"""Synthetic startup endpoint for launcher and desktop lifecycle fixtures.

This exercises the shell's real pipe handoff, wait status, and parser with a
controlled sampler. It does not implement the Rust owner or grant checks;
those run with the actual Kindle-feature executable in the product fixture.
"""

import os
from pathlib import Path
import re
import signal
import stat
import subprocess
import sys
import time


def require(condition):
    if not condition:
        raise ValueError("Invalid synthetic startup handoff")


def now():
    return time.clock_gettime(time.CLOCK_BOOTTIME)


def process(pid):
    return Path(f"/proc/{pid}/stat").read_text().rsplit(") ", 1)[1].split()


def read_pipe(fd, count):
    try:
        return os.read(fd, count)
    except BlockingIOError:
        return None


def publish_fd(source, target):
    require(source != target)
    os.dup2(source, target, inheritable=False)
    os.close(source)


def sample(command, until):
    with subprocess.Popen(command, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                          start_new_session=True) as child:
        fd = child.stdout.fileno()
        os.set_blocking(fd, False)
        body, eof = bytearray(), False
        try:
            while now() < until:
                if read_pipe(64, 1) is not None:
                    return 22, b"-"
                if not eof:
                    block = read_pipe(fd, 129 - len(body))
                    if block is not None:
                        body.extend(block)
                        eof = not block
                if len(body) > 128:
                    break
                if eof and child.poll() is not None:
                    if child.returncode == 0 and body and now() < until:
                        return 0, body.hex().encode("ascii")
                    break
                time.sleep(0.005)
        finally:
            if child.returncode is None:
                # The group's session leader is our exact, unreaped child.
                try:
                    os.killpg(child.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                child.wait(timeout=1)
    return 21, b"-"


def owner(master, control_frame, until, command):
    reader, writer = os.pipe2(os.O_NONBLOCK | os.O_CLOEXEC)
    os.close(writer)
    publish_fd(reader, 64)
    reader, writer = os.pipe2(os.O_NONBLOCK | os.O_CLOEXEC)
    os.close(reader)
    publish_fd(writer, 65)
    result_pipe = os.fstat(65)
    control = bytearray()
    while now() < until:
        try:
            peer = os.stat(f"/proc/{master}/fd/8")
        except FileNotFoundError:
            # EOF before the shell opens its endpoints is not cancellation.
            time.sleep(0.005)
            continue
        require(os.path.samestat(peer, result_pipe))
        block = read_pipe(64, len(control_frame) + 1 - len(control))
        if block == b"":
            status, body = 22, b"-"
            break
        if block is not None:
            control.extend(block)
            require(control_frame.startswith(control))
            if control == control_frame:
                status, body = sample(command, until)
                break
        time.sleep(0.005)
    else:
        status, body = 21, b"-"
    terminal = str(status).encode("ascii") + b" " + body + b"\n"
    require(os.write(65, terminal) == len(terminal))
    return status


def run(wrapper, flag, role, profile, master, birth, sequence, begin, *peer):
    require(flag == "--native-startup-snapshot" and profile == os.environ["BILICOMICS_APPROVED_PROFILE"]
            and os.environ["BILICOMICS_APPROVED_FIRMWARE"] == "5.20.1"
            and all(re.fullmatch(r"[1-9][0-9]{0,19}", value) for value in (master, birth, sequence, begin, *peer)))
    lock = Path(os.environ["BILICOMICS_LIFECYCLE_DIR"])
    generation = os.environ["BILICOMICS_LAUNCH_GENERATION"]
    nonce = os.environ["BILICOMICS_LAUNCH_TOKEN"]
    require(lock.is_absolute() and not lock.is_symlink() and re.fullmatch(r"[0-9a-f]{32}", nonce)
            and master == str(os.getppid()) and process(master)[19] == birth
            and (lock / "pid").read_text() == master + "\n"
            and (lock / "generation").read_text() == f"{generation}:{nonce}\n"
            and (lock / "nonce").read_text() == nonce + "\n"
            and not (lock / "application-pid").exists())
    until = int(begin) / 100 + 2
    if role == "owner" and not peer:
        control = f"startup-display-v1:{generation}:{nonce}:{sequence}:{master}:{birth}:{begin}\n".encode("ascii")
        return owner(master, control, until, [wrapper, "--fixture-startup-sample", "--kindle", "--device", profile])
    require(role == "receive" and len(peer) == 2 and now() < until + 1)
    require(os.path.samestat(os.fstat(0), os.stat(f"/proc/{master}/fd/8")) and stat.S_ISFIFO(os.fstat(0).st_mode))
    os.set_blocking(0, False)
    terminal = read_pipe(0, 262)
    require(terminal and re.fullmatch(rb"(?:0 [0-9a-f]{2,256}|2[12] -)\n", terminal)
            and read_pipe(0, 1) == b"")
    require(os.write(1, terminal) == len(terminal))
    return 0


if __name__ == "__main__":
    try:
        sys.exit(run(*sys.argv[1:]))
    except (OSError, ValueError, KeyError, TypeError, IndexError, subprocess.SubprocessError):
        sys.exit(23)
