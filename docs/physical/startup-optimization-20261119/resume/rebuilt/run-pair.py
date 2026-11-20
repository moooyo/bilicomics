"""Run or recover a freshly prepared native Library click comparison on the test-env host."""

import argparse
from datetime import datetime, timezone
import getpass
import io
import json
import os
from pathlib import Path
import re
import shlex
import signal
import socket
import subprocess
import tarfile
import time

import device_lease


def require(condition, reason):
    if not condition:
        raise ValueError(reason)


def save(path, data):
    with path.open("x") as output:
        json.dump(data, output, indent=2)
        output.write("\n")


def selected(record):
    require(record["screen"] == [1860, 2480] and record["buttons_clear"] is True, "Wrong geometry or busy pointer")
    home, library = record["home"], record["library"]
    require(home["count"] == library["count"] == 1 and home["box"][2] < library["box"][0], "Ambiguous public navigation controls")
    if home["underline_pct"] >= 60 and library["underline_pct"] <= 20:
        return "home"
    if library["underline_pct"] >= 60 and home["underline_pct"] <= 20:
        return "library"
    raise ValueError("Selected native tab not positively identified")


def protocol(info):
    expected = {
        "rebuilt-library-click-2": {"A": "d81b529fd8f84790d91dfe3a8396853c296a8280", "B": "57265de60553ea97b01894592427d60c9008f850"},
        "shelf-render-click-3": {"A": "57265de60553ea97b01894592427d60c9008f850", "B": {
            "source_base_commit": "ab52019569ec8431e145a5cd0bc7cbf58c340ce0", "source_variant": "shelf-private-rgb-v1"}},
    }
    key = info.get("protocol")
    require(key in expected and info.get("source") == expected[key] and info.get("dry_only") is False,
            "Not an admitted real paired source comparison")
    return key


class Batch:
    def __init__(self, path, expected_host):
        require(path.is_absolute() and path.resolve(strict=True) == path and path.parent == Path.cwd() and
                path.parent == Path(__file__).resolve().parent and path.stat().st_mode & 0o777 == 0o700 and
                socket.gethostname().split(".")[0] == "test-env" and os.geteuid() == 0 and
                Path("/var/tmp/kindle-build-coordination").is_dir(), "Use the private host workspace")
        self.path = path
        self.info = json.loads((path / "admission.json").read_text())
        self.run = self.info["run"]
        self.owner = os.environ.get("CODEX_THREAD_ID")
        require(re.fullmatch(r"lib-p-[0-9a-f]{8}", self.run) and self.run == path.name and self.owner == self.info["owner"]
                and re.fullmatch(r"[a-f0-9]{8}(?:-[a-f0-9]{4}){3}-[a-f0-9]{12}", self.owner or "")
                and self.info["host_boot"] == Path("/proc/sys/kernel/random/boot_id").read_text().strip(),
                "The run, owner, or worker boot differs from preparation")
        self.schedule = [arm + f"{pair:02}" for pair in range(7) for arm in ("ab" if pair == 0 or pair % 2 else "ba")]
        self.protocol = protocol(self.info)
        require(self.info["schedule"] == self.schedule
                and {"tools/frame-reader", "tools/control.sh", "tools/entry.sh", "probe/native-target", "probe/card-input", "document"} <= set(self.info["members"]),
                "Not an admitted real paired schedule")
        self.stage, self.priv = [prefix + self.run for prefix in ("/mnt/us/bilicomics-", "/var/tmp/bilicomics-")]
        self.request = device_lease.LeaseRequest(Path("/tmp/codex-kindle-physical-test.lock"), path / "private-token", self.owner, self.run)
        config = Path("/root/.config/kindle-physical/ssh_config")
        trust = config.with_name("known_hosts")
        for item in (config, trust, config.parent):
            require(item.resolve(strict=True) == item and item.stat().st_uid == 0 and not item.stat().st_mode & 0o022, "Unsafe pinned transport")
        self.ssh = ["ssh", "-T", "-F", str(config), "-o", "StrictHostKeyChecking=yes", "-o", "UpdateHostKeys=no",
                    "-o", f"UserKnownHostsFile={trust}", "-o", "GlobalKnownHostsFile=/dev/null", "-o", "ControlMaster=no",
                    "-o", "ControlPath=none", "-o", "ControlPersist=no", "-o", "ClearAllForwardings=yes", "-o", "ForwardAgent=no",
                    "-o", "SendEnv=-*", "-o", "ConnectTimeout=6", "-o", "ServerAliveInterval=10",
                    "-o", "ServerAliveCountMax=2", "-o", "NumberOfPasswordPrompts=1"]
        options = dict(line.split(" ", 1) for line in subprocess.check_output([*self.ssh, "-G", "kindle-physical"], text=True).splitlines())
        require(options.get("hostname") == expected_host and options.get("user") == "root" and options.get("port") == "22",
                "Wrong pinned destination")
        self.askpass = self.path / "askpass"
        self.password = ""

    def remote(self, script, data=None, deadline=25):
        device_lease.status(self.request)
        result = subprocess.run([*self.ssh, "kindle-physical", "sh -c " + shlex.quote("set -eu\n" + script)], input=data,
                                capture_output=True, timeout=deadline, env={"PATH": os.defpath, "HOME": "/root", "LC_ALL": "C",
                                "DISPLAY": ":askpass", "SSH_ASKPASS": str(self.askpass),
                                "SSH_ASKPASS_REQUIRE": "force", "BILI_ASKPASS": self.password})
        require(result.returncode == 0, f"Scoped SSH returned {result.returncode}: {result.stderr.decode(errors='replace')[:360]}")
        return result.stdout

    def control(self, *args, deadline=45):
        return self.remote(shlex.join(["sh", self.stage + "/tools/control.sh", self.run, *args]), deadline=deadline)

    def ensure_tab(self, wanted):
        record = json.loads(self.control("navigation"))
        if selected(record) != wanted:
            receipt = json.loads(self.control("nav-" + wanted))
            require(receipt["ok"] and receipt["buttons_clear_after"], "Public tab click not confirmed")
            for attempt in range(5):
                time.sleep(0.65)
                record = json.loads(self.control("navigation"))
                if selected(record) == wanted:
                    return record
            raise ValueError("Native tab transition not confirmed")
        return record

    def install(self):
        snapshot = self.remote("sh -s", Path(__file__).with_name("native-inventory.sh").read_bytes())
        fields = dict(line.split("=", 1) for line in snapshot.decode("ascii").splitlines())
        require(fields["firmware"] == "5.19.5" and fields["before_front"] == fields["after_front"] == "native" and
                fields["before_power"] == fields["after_power"] == "active" and fields["after_statusbar"] == fields["after_window_manager"] == "running"
                and fields["catalog_selected_rows"] == "0" and all(value == "absent" for key, value in fields.items()
                if key.startswith(("path:/mnt/us/", "before_lock_", "after_lock_"))), "Protected canonical inputs or native state changed")
        save(self.path / "preflight.json", fields)
        self.remote(f'''for root in {self.stage} {self.priv}; do test ! -e "$root" && test ! -L "$root" || exit 1; done
space=$(df -Pk /mnt/us | awk 'END {{print $(NF-2)}}')
case "$space" in ''|*[!0-9]*) exit 1;; esac
test "$space" -ge 900000''')
        (self.path / "staging-started").touch(exist_ok=False)
        self.remote(f'''umask 077; for root in {self.stage} {self.priv}; do
mkdir "$root" && printf '%s\\n' {self.run} >"$root/owner" || exit 1; done
chmod 700 {self.priv}; test "$(stat -c '%u:%a' {self.priv})" = 0:700''')
        self.remote(f'''test "$(cat {self.stage}/owner)" = {self.run} && test ! -L {self.stage} || exit 1
cd {self.stage}; tar -xzf -''', self.payload, deadline=340)
        (self.path / "unpacked").touch(exist_ok=False)
        original = json.loads(self.control("navigation"))
        selected(original)
        save(self.path / "original-navigation.json", original)
        (self.path / "bootstrap-started").touch(exist_ok=False)
        self.control("bootstrap", deadline=130)
        self.ensure_tab("library")

    def measure(self):
        for case in self.schedule:
            print(f"Starting {case} at {datetime.now(timezone.utc).isoformat()}", flush=True)
            directory = self.path / case
            directory.mkdir(mode=0o700)
            self.ensure_tab("library")
            target = json.loads(self.control("card", deadline=60))
            save(directory / "target.json", target)
            require(selected(target) == "library" and target["title"]["count"] == 1, "The exact visible Library card was not unique")
            self.control("arm", case, deadline=80)
            click = json.loads(self.control("click", case, deadline=65))
            save(directory / "click.json", click)
            require(click["target"] == "open-card" and click["ok"] and click["buttons_clear_after"], "Missing confirmed card release")
            self.control("wait", case, deadline=115)
            output = self.control("collect-stages" if self.protocol == "shelf-render-click-3" else "collect", case)
            expected = {"entry.pid", "worker.pid", "entry.status", "performance.jsonl", "lifecycle.jsonl", "final.json", "final.png", "guard-done"}
            if self.protocol == "shelf-render-click-3":
                expected.add("launcher-stages.tsv")
            if case in ("a06", "b06"):
                expected.update(("frame-info.json", "frame-pixels.txt"))
            with tarfile.open(fileobj=io.BytesIO(output)) as archive:
                require(len(archive.getmembers()) == len(expected) and set(archive.getnames()) == expected, "Unexpected case evidence roster")
                for item in archive.getmembers():
                    require(item.isfile() and item.size < 12 * 1024**2, "Unbounded case evidence")
                    with (directory / item.name).open("xb") as target_file:
                        target_file.write(archive.extractfile(item).read())
            final = json.loads((directory / "final.json").read_text())
            require(final["profile"] == "scribe" and final["backend"] == "kindle" and final["demo"] and final["offline"]
                    and final["route"] == "shelf" and not final["startup"]["booting"] and not final["startup"]["error"], "Wrong completed scene")

    def cleanup(self):
        original_file = self.path / "original-navigation.json"
        if (self.path / "bootstrap-started").exists() and not (self.path / "uninstalled").exists():
            self.control("quiesce", deadline=115)
            self.control("uninstall", deadline=125)
            (self.path / "uninstalled").touch(exist_ok=False)
        if original_file.exists() and not (self.path / "native-restored.json").exists():
            original = json.loads(original_file.read_text())
            self.ensure_tab(selected(original))
            pointer = original["pointer"]
            require(len(pointer) == 2 and all(type(value) is int for value in pointer), "Invalid original pointer")
            self.control("pointer", *map(str, pointer))
            final = json.loads(self.control("navigation"))
            require(selected(final) == selected(original) and final["pointer"] == pointer, "Native tab or pointer was not restored")
            save(self.path / "native-restored.json", final)
        if (self.path / "staging-started").exists():
            self.remote(f'''for root in {self.stage} {self.priv}; do
if test -e "$root" || test -L "$root"; then
    test -d "$root" && test ! -L "$root" && test "$(readlink -f "$root")" = "$root" &&
        test -f "$root/owner" && test ! -L "$root/owner" && test "$(cat "$root/owner")" = {self.run} || exit 1
    for leaf in "$root"/* "$root"/.[!.]* "$root"/..?*; do
        test -e "$leaf" || test -L "$leaf" || continue
        case "$root:${{leaf##*/}}" in
            {self.stage}:owner|{self.stage}:document|{self.stage}:tools|{self.stage}:probe|{self.stage}:seed|{self.stage}:cases|{self.stage}:park-[AB]|{self.stage}:reference-[AB]) :;;
            {self.priv}:owner|{self.priv}:active|{self.priv}:active.next|{self.priv}:[ab]0[0-6]|{self.priv}:created-document|{self.priv}:install-started|{self.priv}:protection.before|{self.priv}:protection.after) :;;
            *) exit 1;;
        esac
        test ! -L "$leaf" || exit 1
    done
    rm -rf "$root"
fi
test ! -e "$root" && test ! -L "$root" || exit 1; done''', deadline=65)

    def execute(self, recovering, wait_seconds):
        if not recovering:
            require(not (self.path / "started.json").exists() and not (self.path / "released.json").exists(), "Never reuse a run")
            self.payload = (self.path / "payload.tar.gz").read_bytes()
            require(len(self.payload) == self.info["payload_bytes"] and "tools/control.sh" in self.info["members"], "Incomplete prepared payload")
        else:
            require((self.path / "started.json").exists() and not (self.path / "released.json").exists(), "No interrupted owned operation")
            device_lease.status(self.request)
        self.askpass.write_text('#!/bin/sh\ncase "$1" in *assword:*) printf "%s\\n" "$BILI_ASKPASS";; *) exit 1;; esac\n')
        self.askpass.chmod(0o700)
        self.password = getpass.getpass("Kindle SSH password (not saved): ")
        require(bool(self.password), "Missing password")
        if not recovering:
            deadline = time.monotonic() + wait_seconds
            waiting = False
            while True:
                try:
                    acquired = device_lease.acquire(self.request)
                    break
                except device_lease.LeaseError as error:
                    if error.code != "lease_exists" or any(error.details.values()) or time.monotonic() >= deadline:
                        self.askpass.unlink()
                        raise
                    if not waiting:
                        print("Waiting on the shared host lease; no device access or reservation.", flush=True)
                        waiting = True
                    while os.path.lexists(self.request.lease_dir) and time.monotonic() < deadline:
                        time.sleep(0.5)
            save(self.path / "started.json", acquired)
        try:
            if not recovering:
                self.install()
                self.measure()
        except BaseException as error:
            save(self.path / "failure.json", {"type": type(error).__name__, "detail": str(error)})
            raise
        finally:
            self.cleanup()
            released = device_lease.release(self.request, native_restored_and_connections_closed=True)
            save(self.path / "released.json", released)
            self.password = ""
            self.askpass.unlink()
            print("Exact lease released; physical operation closed.", flush=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--batch", required=True, type=Path)
    parser.add_argument("--expected-host", required=True)
    parser.add_argument("--recover", action="store_true")
    parser.add_argument("--wait-seconds", type=int, default=0, choices=range(0, 1801), metavar="0..1800")
    args = parser.parse_args()
    def interrupt(signum, frame):
        raise InterruptedError(f"Signal {signum}")
    for sig in (signal.SIGTERM, signal.SIGHUP):
        signal.signal(sig, interrupt)
    Batch(args.batch, args.expected_host).execute(args.recover, args.wait_seconds)
