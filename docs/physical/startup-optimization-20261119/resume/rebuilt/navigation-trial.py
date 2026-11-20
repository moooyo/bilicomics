"""Exercise the display reader only; never install a document, change tabs or issue input."""

import argparse
import getpass
import json
import os
from pathlib import Path
import re
import secrets
import shlex
import socket
import subprocess
import tarfile
import time

import device_lease


def require(condition, reason):
    if not condition:
        raise ValueError(reason)


def write(path, data):
    with path.open("x") as output:
        json.dump(data, output, indent=2)
        output.write("\n")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    modes = parser.add_mutually_exclusive_group(required=True)
    modes.add_argument("--kit", type=Path)
    modes.add_argument("--filesystem", action="store_true")
    parser.add_argument("--selector", type=Path)
    parser.add_argument("--expected-host", required=True)
    parser.add_argument("--wait-seconds", type=int, default=0, choices=range(0, 1801), metavar="0..1800")
    args = parser.parse_args()
    os.umask(0o077)
    owner = os.environ.get("CODEX_THREAD_ID", "")
    require(re.fullmatch(r"[a-f0-9]{8}(?:-[a-f0-9]{4}){3}-[a-f0-9]{12}", owner), "Use the actual receiving conversation")
    root = Path(__file__).resolve().parent
    require(os.geteuid() == 0 and socket.gethostname().split(".")[0] == "test-env" and Path.cwd() == root
            and root.stat().st_mode & 0o777 == 0o700 and Path("/var/tmp/kindle-build-coordination").is_dir(), "Use the private host workspace")
    config = Path("/root/.config/kindle-physical/ssh_config")
    trust = config.with_name("known_hosts")
    for path in (config, trust, config.parent):
        require(path.resolve(strict=True) == path and path.stat().st_uid == 0 and not path.stat().st_mode & 0o022, "Unsafe pinned transport")
    ssh = ["ssh", "-T", "-F", str(config), "-o", "StrictHostKeyChecking=yes", "-o", "UpdateHostKeys=no",
           "-o", f"UserKnownHostsFile={trust}", "-o", "GlobalKnownHostsFile=/dev/null", "-o", "ControlMaster=no",
           "-o", "ControlPath=none", "-o", "ControlPersist=no", "-o", "ClearAllForwardings=yes", "-o", "ForwardAgent=no",
           "-o", "SendEnv=-*", "-o", "ConnectTimeout=5", "-o", "NumberOfPasswordPrompts=1"]
    options = dict(line.split(" ", 1) for line in subprocess.check_output([*ssh, "-G", "kindle-physical"], text=True).splitlines())
    require(options.get("hostname") == args.expected_host and options.get("user") == "root" and options.get("port") == "22",
            "The pinned alias is not the selected Kindle")
    names, payload = [], b""
    if args.kit:
        with tarfile.open(args.kit, "r:gz") as archive:
            names = archive.getnames()
            require(len(names) == len(set(names)) and {"native-target", "lib/ld-linux-armhf.so.3", "models/eng.traineddata",
                    "models/chi_sim.traineddata"} <= set(names), "Incomplete private display reader")
            require(all(entry.isfile() and re.fullmatch(r"native-target|lib/[A-Za-z0-9._+-]+|models/(?:eng|chi_sim)\.traineddata", entry.name)
                        for entry in archive.getmembers()) and sum(entry.size for entry in archive.getmembers()) < 128 * 1024**2,
                    "Unexpected navigation kit member")
        payload = args.kit.read_bytes()
    if args.filesystem:
        require(args.selector is not None, "Select the qualified ARM pair-fs helper")
        payload = args.selector.read_bytes()
        require(len(payload) < 4 * 1024**2 and payload[:7] == b"\x7fELF\x01\x01\x01" and payload[18:20] == b"(\0", "Not the ARM32 selector")
    run = ("lib-p-" if args.filesystem else "bili-nav-") + secrets.token_hex(4)
    batch = root / run
    batch.mkdir(mode=0o700)
    leaf = ("bilicomics-" if args.filesystem else "") + run
    stage = "/mnt/us/" + leaf
    private = "/var/tmp/" + leaf
    request = device_lease.LeaseRequest(Path("/tmp/codex-kindle-physical-test.lock"), batch / "private-token", owner, run)
    askpass = batch / "askpass"
    askpass.write_text('#!/bin/sh\ncase "$1" in *assword:*) printf "%s\\n" "$BILI_ASKPASS";; *) exit 1;; esac\n')
    askpass.chmod(0o700)
    password = getpass.getpass("Kindle SSH password (not saved): ")
    require(bool(password), "A password is required")
    stage_binding = f'''test -d {stage} && test ! -L {stage} && test "$(readlink -f {stage})" = {stage} &&
test -f {stage}/owner && test ! -L {stage}/owner && test "$(cat {stage}/owner)" = {run}'''
    native = '''test "$(lipc-get-prop com.lab126.appmgrd activeApp)" = com.lab126.KPPMainApp
test "$(lipc-get-prop -s com.lab126.powerd state)" = active
case "$(status statusbar)" in *start/running*) :;; *) exit 1;; esac
for project in bilicomics aidoku weread; do path=/tmp/$project-launcher.lock; test ! -e "$path" && test ! -L "$path" || exit 1; done'''

    def remote(script, data=None, deadline=25):
        device_lease.status(request)
        result = subprocess.run([*ssh, "kindle-physical", "sh -c " + shlex.quote("set -eu\n" + script)], input=data,
                                capture_output=True, timeout=deadline, env={"PATH": os.defpath, "HOME": "/root", "LC_ALL": "C",
                                "DISPLAY": ":askpass", "SSH_ASKPASS": str(askpass), "SSH_ASKPASS_REQUIRE": "force", "BILI_ASKPASS": password})
        require(result.returncode == 0, f"Scoped SSH command returned {result.returncode}: {result.stderr.decode(errors='replace')[:260]}")
        return result.stdout

    deadline = time.monotonic() + args.wait_seconds
    while True:
        try:
            acquired = device_lease.acquire(request)
            break
        except device_lease.LeaseError as error:
            if error.code != "lease_exists" or any(error.details.values()) or time.monotonic() >= deadline:
                askpass.unlink()
                raise
            while os.path.lexists(request.lease_dir) and time.monotonic() < deadline:
                time.sleep(0.5)
    prepared = False
    try:
        write(batch / "acquired.json", acquired)
        remote(f'''{native}
test ! -e {stage} && test ! -L {stage}
space=$(df -Pk /mnt/us | awk 'END {{print $(NF-2)}}')
case "$space" in ''|*[!0-9]*) exit 1;; esac
test "$space" -ge 180000''')
        prepared = True
        if args.filesystem:
            raw = remote(f'''umask 077
for root in {private} {stage}; do test ! -e "$root" && test ! -L "$root" || exit 1; done
for root in {private} {stage}; do
    mkdir "$root" && printf '%s\\n' {run} >"$root/owner" || exit 1
done
mkdir {stage}/tools
for site in media private; do
    if test "$site" = media; then root={stage}; else root={private}; fi
    printf '%s_dir=%s\\n%s_file=%s\\n' "$site" "$(stat -c '%u:%a:%h' "$root")" "$site" "$(stat -c '%u:%h:%s' "$root/owner")"
    if test "$(readlink -f "$root/owner")" = "$root/owner"; then result=yes; else result=no; fi
    printf '%s_canonical=%s\\n' "$site" "$result"
    if printf '%s\\n' {run} | cmp -s - "$root/owner"; then result=yes; else result=no; fi
    printf '%s_stdin_cmp=%s\\n' "$site" "$result"
    if cmp -s "$root/owner" "$root/owner"; then result=yes; else result=no; fi
    printf '%s_file_cmp=%s\\n' "$site" "$result"
    if IFS= read -r value <"$root/owner" && test "$value" = {run} &&
        test "$(wc -c <"$root/owner")" -eq {len(run)+1}; then result=yes; else result=no; fi
    printf '%s_exact_line=%s\\n' "$site" "$result"
done
if test "$(readlink -f {stage}/tools)" = {stage}/tools; then echo tools_canonical=yes; else echo tools_canonical=no; fi
chmod 700 {private}
printf 'private_explicit_mode=%s\\n' "$(stat -c '%u:%a' {private})"
if printf '%s\\n' {run} | diff - {stage}/owner >/dev/null 2>&1; then echo stdin_diff=yes; else echo stdin_diff=no; fi
if cmp -s {private}/owner {stage}/owner; then echo across_cmp=yes; else echo across_cmp=no; fi''')
            remote(f'''{stage_binding} || exit 1
(set -C; cat >{stage}/tools/pair-fs); chmod 755 {stage}/tools/pair-fs''', payload)
            script = (f'#!/bin/sh\nset -eu\n[ "$*" = "locked A probe" ] || exit 2\n'
                      f'exec {stage}/tools/pair-fs probe {private}/owner {stage}\n')
            remote(f'''{stage_binding} || exit 1
(set -C; cat >{stage}/tools/select-arms.sh); chmod 755 {stage}/tools/select-arms.sh''', script.encode())
            kernel = remote(f'''{stage_binding} || exit 1
rc=0; output=$({stage}/tools/pair-fs lock {private}/owner {stage}/tools/select-arms.sh A probe 2>&1) || rc=$?
printf 'exit=%s\\n%s\\n' "$rc" "$output"''').decode().splitlines()
            result = dict(line.split("=", 1) for line in raw.decode("ascii").splitlines())
            result["kernel_directory_probe"] = kernel
            write(batch / "filesystem.json", result)
            print(batch / "filesystem.json", flush=True)
            return
        remote(f'''umask 077; mkdir {stage} && printf '%s\\n' {run} >{stage}/owner
{stage_binding} || exit 1
cd {stage}; mkdir lib models; tar -xzf -''', payload, 100)
        raw = remote(f'''{stage_binding} || exit 1
{native}
cd {stage}
DISPLAY=:0 OMP_THREAD_LIMIT=1 LC_ALL=C ./lib/ld-linux-armhf.so.3 --inhibit-cache --library-path {stage}/lib ./native-target navigation {stage}/models''', deadline=65)
        record = json.loads(raw)
        require(set(record) == {"screen", "pointer", "buttons_clear", "home", "library", "title"} and record["title"] == {"count": 0},
                "Unexpected read-only navigation receipt")
        write(batch / "navigation.json", record)
    finally:
        if prepared:
            if args.filesystem:
                remote(f'''if test -e {private} || test -L {private}; then
test -d {private} && test ! -L {private} && test "$(readlink -f {private})" = {private} &&
    test -f {private}/owner && test ! -L {private}/owner && test "$(cat {private}/owner)" = {run} || exit 1
rm {private}/owner; rmdir {private}; fi
if test -e {stage}/tools || test -L {stage}/tools; then
    {stage_binding} && test ! -L {stage}/tools || exit 1
    for leaf in pair-fs select-arms.sh; do path={stage}/tools/$leaf
        if test -e "$path" || test -L "$path"; then test -f "$path" && test ! -L "$path" && rm "$path" || exit 1; fi
    done
    rmdir {stage}/tools
fi''')
            cleanup = f'''if test -e {stage} || test -L {stage}; then
{stage_binding} || exit 1
cd {stage}
for directory in lib models; do if test -e "$directory" || test -L "$directory"; then test -d "$directory" && test ! -L "$directory" || exit 1; fi; done
for leaf in {" ".join(names)}; do
    if test -e "$leaf" || test -L "$leaf"; then test -f "$leaf" && test ! -L "$leaf" || exit 1; rm "$leaf"; fi
done
for directory in lib models; do if test -e "$directory" || test -L "$directory"; then test -d "$directory" && test ! -L "$directory" && rmdir "$directory" || exit 1; fi; done
test "$(ls -A)" = owner; rm owner; cd /mnt/us; rmdir {stage}
fi
test ! -e {stage} && test ! -L {stage}'''
            remote(cleanup)
        released = device_lease.release(request, native_restored_and_connections_closed=True)
        write(batch / "released.json", released)
        askpass.unlink()
    print(batch)


if __name__ == "__main__":
    main()
