"""Prepare or operate one fresh, leased, normal Library install on the verification host."""

import argparse
import getpass
import importlib.util
import io
import json
import os
from pathlib import Path
import re
import secrets
import shlex
import signal
import socket
import stat
import subprocess
import tarfile
import time
import zipfile

import decision
import device_lease


ROOT = Path(__file__).resolve().parent
HOST_LEASE = Path("/tmp/codex-kindle-physical-test.lock")
PROTOCOL = "production-library-rollout-1"
RUN = re.compile(r"bili-prod-[a-f0-9]{8}")
PEER = "kindle-physical"


def module(filename):
    spec = importlib.util.spec_from_file_location(filename, ROOT / (filename + ".py"))
    result = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(result)
    return result


pair = module("prepare-pair")
paired = module("run-pair")
require, save, selected = paired.require, paired.save, paired.selected


def load(path):
    return json.loads(path.read_text())


def private(path):
    return path.is_absolute() and path.resolve(strict=True) == path and path.is_dir() and \
        path.stat().st_uid == 0 and stat.S_IMODE(path.stat().st_mode) == 0o700


def host():
    require(os.geteuid() == 0 and socket.gethostname().split(".")[0] == "test-env" and
            Path("/var/tmp/kindle-build-coordination").is_dir() and private(ROOT) and Path.cwd() == ROOT,
            "Use the private rollout workspace on the test-env host")
    owner = os.environ.get("CODEX_THREAD_ID", "")
    require(re.fullmatch(r"[a-f0-9]{8}(?:-[a-f0-9]{4}){3}-[a-f0-9]{12}", owner), "Use the actual receiving conversation")
    return owner, Path("/proc/sys/kernel/random/boot_id").read_text().strip()


def published(batch, control, candidate, published_dir, selected_zip):
    admission, released, result, verdict = (load(batch / name) for name in
                                            ("admission.json", "released.json", "paired-result.json", "decision.json"))
    require(paired.protocol(admission) == "shelf-render-click-3" and batch.name == verdict["run"] and
            verdict["recommend_candidate_for_measured_profile"] is True and released["state"] == "released" and
            released["owner_and_token_checked"] is True and released["native_restored_and_connections_closed"] is True and
            not (batch / "failure.json").exists(), "Missing successful closed device comparison")
    require(verdict == load(published_dir / "decision.json") == decision.evaluate(batch, result)
            and result == load(published_dir / "paired-result.json"), "Published decision no longer matches the complete comparison")
    receipts = ["build-receipt.json", "package-static.json", "software.json"]
    for leaf in receipts:
        require(load(candidate / "evidence" / leaf) == load(published_dir / leaf), "Selected receipt differs from publication")
    pair.qualified_control(control)
    members = pair.shelf_candidate(control, candidate)
    require(selected_zip.read_bytes() == (candidate / pair.ARCHIVE).read_bytes() and
            all(len(members["bilicomics/" + leaf]) == admission["expected_sizes"]["B"][leaf]
                for leaf in ("app/bilicomics", "app/kindle-jpeg-worker")), "Wrong selected candidate ZIP or measured binary sizes")
    require(members["documents/bilicomics.sh"].endswith(b"exec sh /mnt/us/bilicomics/launch.sh --library\n"),
            "Not the genuine canonical document")
    return load(candidate / "evidence/build-receipt.json")["source_id"]


def safe_member(path):
    return all(part not in ("", ".", "..") and re.fullmatch(r"[A-Za-z0-9._+-]+", part) for part in path.split("/"))


def add(archive, name, data, mode):
    require(safe_member(name), "Unsafe rollout archive path")
    item = tarfile.TarInfo(name)
    item.mode, item.size = mode, len(data)
    archive.addfile(item, io.BytesIO(data))


def payload(target, selected_zip, kit, helper, kit_receipt):
    with tarfile.open(target, "x:gz", compresslevel=2) as output, zipfile.ZipFile(selected_zip) as package:
        names = package.namelist()
        require(len(names) == len(set(names)) == 49, "Unexpected production package roster")
        for item in package.infolist():
            path = item.filename
            require(safe_member(path) and not item.is_dir() and (path.startswith(("notices/", "bilicomics/")) or
                    path in ("documents/bilicomics.sh", "README.md", "THIRD_PARTY_NOTICES.md")), "Unexpected release payload")
            add(output, "package/" + path, package.read(item), (item.external_attr >> 16) & 0o777)
        with tarfile.open(kit, "r:gz") as reader:
            names = reader.getnames()
            require(sorted(names) == sorted(kit_receipt["members"]) and
                    sum(item.size for item in reader.getmembers()) == kit_receipt["bytes"] and
                    len(names) == len(set(names)) and {"card-input", "native-target", "lib/ld-linux-armhf.so.3",
                    "models/eng.traineddata", "models/chi_sim.traineddata"} <= set(names) and
                    sum(item.size for item in reader.getmembers()) < 128 * 1024**2, "Unqualified input kit")
            for item in reader.getmembers():
                require(item.isfile() and re.fullmatch(r"card-input|native-target|lib/[A-Za-z0-9._+-]+|models/(?:eng|chi_sim)\.traineddata",
                        item.name), "Unexpected input kit member")
                add(output, "probe/" + item.name, reader.extractfile(item).read(), 0o700 if not item.name.startswith("models/") else 0o600)
        add(output, "tools/control.sh", helper.read_bytes(), 0o700)


def roster(archive):
    with tarfile.open(archive, "r:gz") as records:
        names = records.getnames()
        require(len(names) == len(set(names)) and all(item.isfile() and safe_member(item.name)
                and item.name.partition("/")[0] in ("package", "probe", "tools") for item in records.getmembers()),
                "Unexpected rollout staging members")
        return sorted(names)


def prepare(args):
    owner, boot = host()
    source_id = published(args.comparison, args.control, args.candidate, args.publication, args.zip)
    run = "bili-prod-" + secrets.token_hex(4)
    batch = ROOT / run
    batch.mkdir(mode=0o700)
    receipt = load(args.kit_receipt)
    paired_roster = load(args.comparison / "admission.json")["members"]
    require(sorted("probe/" + name for name in receipt["members"]) == sorted(name for name in paired_roster if name.startswith("probe/")),
            "Recognizer roster differs from the measured input kit")
    payload(batch / "payload.tar.gz", args.zip, args.kit, ROOT / "rollout-device.sh", receipt)
    with zipfile.ZipFile(args.zip) as package:
        expected_document = package.read("documents/bilicomics.sh").decode("ascii")
    require(len(expected_document) <= 1024 and "\0" not in expected_document and
            expected_document.endswith("exec sh /mnt/us/bilicomics/launch.sh --library\n"), "Unexpected production Library document")
    save(batch / "admission.json", {"protocol": PROTOCOL, "run": run, "owner": owner, "boot": boot,
         "measured_run": args.comparison.name, "source_id": source_id, "profile": "scribe", "firmware": "5.19.5",
         "payload_bytes": (batch / "payload.tar.gz").stat().st_size, "stage_files": roster(batch / "payload.tar.gz"),
         "expected_document": expected_document})
    print(batch, flush=True)


def pinned(expected_host):
    config = Path("/root/.config/kindle-physical/ssh_config")
    trust = config.with_name("known_hosts")
    for item in (config, trust, config.parent):
        require(item.resolve(strict=True) == item and item.stat().st_uid == 0 and not item.stat().st_mode & 0o022,
                "Unsafe pinned transport configuration")
    command = ["ssh", "-T", "-F", str(config), "-o", "StrictHostKeyChecking=yes", "-o", "UpdateHostKeys=no",
               "-o", f"UserKnownHostsFile={trust}", "-o", "GlobalKnownHostsFile=/dev/null", "-o", "ControlMaster=no",
               "-o", "ControlPath=none", "-o", "ControlPersist=no", "-o", "ClearAllForwardings=yes", "-o", "ForwardAgent=no",
               "-o", "SendEnv=-*", "-o", "ConnectTimeout=6", "-o", "ServerAliveInterval=10",
               "-o", "ServerAliveCountMax=2", "-o", "NumberOfPasswordPrompts=1"]
    options = dict(line.split(" ", 1) for line in subprocess.check_output([*command, "-G", PEER], text=True).splitlines())
    require(options.get("hostname") == expected_host and options.get("user") == "root" and options.get("port") == "22",
            "Wrong pinned destination")
    return command


DOCUMENT_SCAN = rb'''
directory=/mnt/us/documents
test -d "$directory" && test ! -L "$directory" && test "$(readlink -f "$directory")" = "$directory" &&
    test -r "$directory" && test -x "$directory" || exit 1
doc_spelling=absent
doc_sidecar=absent
for leaf in "$directory"/[Bb][Ii][Ll][Ii][Cc][Oo][Mm][Ii][Cc][Ss].*; do
    test -e "$leaf" || test -L "$leaf" || continue
    name=${leaf##*/}
    case "$name" in
        [Bb][Ii][Ll][Ii][Cc][Oo][Mm][Ii][Cc][Ss].[Ss][Hh])
            if test "$name" = bilicomics.sh && test "$doc_spelling" = absent; then doc_spelling=bilicomics.sh
            else doc_spelling=unexpected; fi;;
        [Bb][Ii][Ll][Ii][Cc][Oo][Mm][Ii][Cc][Ss].[Ss][Dd][Rr]|[Bb][Ii][Ll][Ii][Cc][Oo][Mm][Ii][Cc][Ss].[Ss][Hh].[Ss][Dd][Rr])
            doc_sidecar=present;;
    esac
done
doc_id() {
    if test ! -e "$1" && test ! -L "$1"; then printf 'absent\n'; return; fi
    test -f "$1" && test ! -L "$1" || return 1
    stat -c '%d:%i:%h' "$1"
}
lower_id=$(doc_id "$directory/bilicomics.sh") && upper_id=$(doc_id "$directory/BiliComics.sh") || exit 1
printf 'doc_spelling=%s\ndoc_sidecar=%s\ndoc_lower_id=%s\ndoc_upper_id=%s\n' "$doc_spelling" "$doc_sidecar" "$lower_id" "$upper_id"
printf 'kindle_boot=%s\ndoc_volume=%s\n' "$(cat /proc/sys/kernel/random/boot_id)" "$(stat -c %d "$directory")"
'''


def document_comparisons(expected):
    require(isinstance(expected, str) and 0 < len(expected) <= 1024 and "\0" not in expected and expected.isascii() and
            expected.endswith("exec sh /mnt/us/bilicomics/launch.sh --library\n"), "No qualified package document to compare")
    return f'''
expected_document={shlex.quote(expected)}
compare_lookup() {{
    if test "$2" = absent; then test ! -e "$1" && test ! -L "$1" && printf 'absent\\n'; return; fi
    test -f "$1" && test ! -L "$1" && test "$(stat -c '%s' "$1")" = {len(expected)} &&
        printf '%s' "$expected_document" | cmp -s - "$1" || return 1
    test "$(doc_id "$1")" = "$2" && printf 'exact\\n'
}}
lower_bytes=$(compare_lookup "$directory/bilicomics.sh" "$lower_id") && upper_bytes=$(compare_lookup "$directory/BiliComics.sh" "$upper_id") || exit 1
rows=$(sqlite3 -readonly -cmd '.timeout 1500' /var/local/cc.db "SELECT count(*)||':'||coalesce(sum(p_location COLLATE BINARY='/mnt/us/documents/bilicomics.sh' AND p_mimeType='text/x-shellscript' AND p_titles_0_nominal='BiliComics for Kindle'),0) FROM Entries WHERE p_location COLLATE NOCASE='/mnt/us/documents/bilicomics.sh';") || exit 1
printf 'doc_lower_bytes=%s\\ndoc_upper_bytes=%s\\ndoc_index=%s\\n' "$lower_bytes" "$upper_bytes" "$rows"
'''.encode("ascii")


def inventory(output, installed, *, run=None, witness=None):
    rows = dict(line.split("=", 1) for line in output.decode("ascii").splitlines())
    paths = {"/mnt/us/bilicomics": "directory", "/mnt/us/bilicomics/data": "directory",
             "/mnt/us/bilicomics/app/bilicomics": "file", "/mnt/us/bilicomics/app/kindle-jpeg-worker": "file",
             "/mnt/us/bilicomics/launch.sh": "file", "/mnt/us/documents/bilicomics.sh": "file"}
    upper_path = "/mnt/us/documents/BiliComics.sh"
    upper_kind = rows.get("path:" + upper_path)
    device_boot = rows.get("kindle_boot", "")
    volume = rows.get("doc_volume", "")
    require(re.fullmatch(r"[a-f0-9]{8}(?:-[a-f0-9]{4}){3}-[a-f0-9]{12}", device_boot) and
            re.fullmatch(r"[0-9]+", volume), "Missing device boot or document volume")
    if installed:
        lower = rows.get("doc_lower_id", "")
        upper = rows.get("doc_upper_id", "")
        alternate = upper_kind == "file" and upper != lower
        mode = "casefold-proven" if alternate else "same-inode" if upper_kind == "file" else "absent"
        if witness is not None or alternate:
            require(run and RUN.fullmatch(run) and isinstance(witness, dict) and witness.get("run") == run and
                    witness.get("kindle_boot") == device_boot and witness.get("doc_volume") == volume and
                    witness.get("doc_lower_id") == lower and witness.get("doc_upper_id") == upper and witness.get("doc_alias_mode") == mode,
                    "Document lookups are not bound to this run, boot, volume and casefold experiment")
        require(rows.get("doc_spelling") == "bilicomics.sh" and rows.get("doc_sidecar") == "absent" and
                re.fullmatch(volume + r":[1-9][0-9]*:1", lower) and rows.get("doc_index") == "1:1" and
                rows.get("doc_lower_bytes") == "exact" and
                ((upper_kind == "absent" and upper == rows.get("doc_upper_bytes") == "absent") or
                 (upper_kind == "file" and re.fullmatch(volume + r":[1-9][0-9]*:1", upper) and
                  rows.get("doc_upper_bytes") == "exact")),
                "The installed lowercase document and its alternate lookup are not uniquely proven")
        paths[upper_path] = upper_kind
    else:
        require(all(rows.get(key) == "absent" for key in ("doc_spelling", "doc_sidecar", "doc_lower_id", "doc_upper_id")) and
                rows.get("doc_index") == "0:0",
                "BiliComics document or sidecar is not absent")
    require(rows.get("firmware") == "5.19.5" and rows.get("catalog_selected_rows") == ("1" if installed else "0") and
            rows.get("fb_bits_per_pixel") == "8" and rows.get("fb_virtual_size") == "1872,4960" and
            all(rows.get(f"{sample}_{key}") == value for sample in ("before", "after")
                for key, value in (("front", "native"), ("power", "active"), ("statusbar", "running"), ("window_manager", "running"))) and
            all(rows.get(f"{sample}_lock_{name}") == "absent" for sample in ("before", "after")
                for name in ("bilicomics", "aidoku", "weread")), "Device not idle on the qualified firmware")
    for path in {*paths, upper_path, "/mnt/us/documents/bilicomics.sdr",
                 "/mnt/us/documents/BiliComics.sdr", "/mnt/us/documents/bilicomics.sh.sdr", "/mnt/us/documents/BiliComics.sh.sdr"}:
        expected = paths.get(path, "absent") if installed else "absent"
        require(rows.get("path:" + path) == expected, "Unexpected BiliComics path at the deployment boundary: " + path)
    return {"firmware": rows["firmware"], "kindle_boot": device_boot, "doc_volume": volume,
            "native_idle": True, "catalog_rows": int(rows["catalog_selected_rows"]),
            "actual_document_basename": rows["doc_spelling"], "uppercase_lookup_is_alias": installed and upper_kind == "file"}


class Rollout:
    def __init__(self, path, expected_host):
        owner, boot = host()
        require(path.parent == ROOT and private(path) and RUN.fullmatch(path.name), "Not a private rollout batch")
        self.path, self.run = path, path.name
        self.info = load(path / "admission.json")
        require(self.info["protocol"] == PROTOCOL and self.info["boot"] == boot and self.info["owner"] == owner and
                self.info["run"] == self.run and self.info["profile"] == "scribe" and self.info["firmware"] == "5.19.5",
                "Admission does not match the live rollout owner/boot")
        self.stage, self.priv = (prefix + self.run for prefix in ("/mnt/us/bilicomics-", "/var/tmp/bilicomics-"))
        self.request = device_lease.LeaseRequest(HOST_LEASE, path / "private-token", owner, self.run)
        self.ssh = pinned(expected_host)
        self.askpass = self.path / "askpass"
        self.password = ""

    def remote(self, script, data=None, deadline=25):
        device_lease.status(self.request)
        result = subprocess.run([*self.ssh, PEER, "sh -c " + shlex.quote("set -eu\n" + script)], input=data,
                                capture_output=True, timeout=deadline, env={"PATH": os.defpath, "HOME": "/root", "LC_ALL": "C",
                                "DISPLAY": ":askpass", "SSH_ASKPASS": str(self.askpass),
                                "SSH_ASKPASS_REQUIRE": "force", "BILI_ASKPASS": self.password})
        require(result.returncode == 0, f"Scoped rollout command failed (status {result.returncode}); output withheld")
        return result.stdout

    def control(self, *args, deadline=45):
        return self.remote(shlex.join(["sh", self.stage + "/tools/control.sh", self.run, *args]), deadline=deadline)

    def native_inventory(self, installed):
        script = ((ROOT / "native-inventory.sh").read_bytes() + DOCUMENT_SCAN + document_comparisons(self.info["expected_document"]))
        if not installed:
            return inventory(self.remote("sh -s", script), False)
        receipt = self.path / "document-view.json"
        initial = load(self.path / "preflight.json")
        if receipt.is_file():
            return inventory(self.remote("sh -s", script), True, run=self.run, witness=load(receipt))
        require(not (self.path / "recovery-verified").exists(), "Missing run-bound document proof before staging removal")
        view = dict(line.split("=", 1) for line in self.control("document-view").decode("ascii").splitlines())
        require(set(view) == {"doc_lower_id", "doc_upper_id", "doc_alias_mode"} and
                view["doc_alias_mode"] in ("absent", "same-inode", "casefold-proven"), "Unexpected helper document view")
        witness = {**view, "run": self.run, "kindle_boot": initial["kindle_boot"], "doc_volume": initial["doc_volume"]}
        output = self.remote("sh -s", script)
        fields = dict(line.split("=", 1) for line in output.decode("ascii").splitlines())
        require(all(witness[key] == fields.get(key) for key in ("doc_lower_id", "doc_upper_id", "kindle_boot", "doc_volume")),
                "The installed document or device changed across its casefold experiment")
        result = inventory(output, True, run=self.run, witness=witness)
        save(receipt, witness)
        return result

    def flag(self, name):
        (self.path / name).touch(exist_ok=False)

    def stage_payload(self, data):
        first = self.native_inventory(False)
        save(self.path / "preflight.json", first)
        self.remote(f'''for root in {self.stage} {self.priv}; do test ! -e "$root" && test ! -L "$root" || exit 1; done
free=$(df -Pk /mnt/us | awk 'END {{print $(NF-2)}}')
case "$free" in ''|*[!0-9]*) exit 1;; esac
test "$free" -ge 400000''')
        self.flag("staging-intended")
        self.remote(f'''umask 077; for root in {self.stage} {self.priv}; do
mkdir "$root" && (set -C; printf '%s\\n' {self.run} >"$root/owner") || exit 1; done
chmod 700 {self.priv}; test "$(stat -c '%u:%a' {self.priv})" = 0:700''')
        self.remote(f'''test ! -L {self.stage} && test "$(readlink -f {self.stage})" = {self.stage}
test ! -L {self.stage}/owner && test "$(cat {self.stage}/owner)" = {self.run}
cd {self.stage}; tar -xzf -''', data, deadline=330)
        self.flag("unpacked")

    def probe(self, mode, *args):
        require(mode in ("navigation", "card", "open-card", "nav-home", "nav-library", "restore-pointer"),
                "Only recognized native navigation can be requested")
        self.control("native")
        probe = self.stage + "/probe"
        reader = mode in ("card", "navigation")
        command = [probe + "/lib/ld-linux-armhf.so.3", "--inhibit-cache", "--library-path", probe + "/lib",
                   probe + ("/native-target" if reader else "/card-input"), mode]
        command.extend(list(map(str, args)) if mode == "restore-pointer" else [probe + "/models"])
        return json.loads(self.remote("DISPLAY=:0 OMP_THREAD_LIMIT=1 LC_ALL=C " + shlex.join(command), deadline=65))

    def ensure_tab(self, wanted):
        record = self.probe("navigation")
        if selected(record) != wanted:
            receipt = self.probe("nav-" + wanted)
            require(receipt["ok"] and receipt["buttons_clear_after"], "Public tab click was not confirmed")
            for attempt in range(5):
                time.sleep(.65)
                record = self.probe("navigation")
                if selected(record) == wanted:
                    return record
            raise ValueError("Selected public tab not confirmed")
        return record

    def restore_navigation(self):
        before = load(self.path / "original-navigation.json")
        self.ensure_tab(selected(before))
        pointer = before["pointer"]
        require(len(pointer) == 2 and all(type(value) is int for value in pointer), "Invalid original pointer")
        require(self.probe("restore-pointer", *pointer)["restored"] is True, "Pointer restoration not confirmed")
        after = self.probe("navigation")
        require(selected(before) == selected(after) and after["pointer"] == pointer, "Native state not restored")
        if not (self.path / "restored-navigation.json").exists():
            save(self.path / "restored-navigation.json", after)

    def install_and_smoke(self):
        original = self.probe("navigation")
        selected(original)
        save(self.path / "original-navigation.json", original)
        self.flag("install-intended")
        self.control("install", deadline=130)
        self.native_inventory(True)
        self.ensure_tab("library")
        card = self.probe("card")
        require(selected(card) == "library" and card["title"]["count"] == 1, "The exact production card was not unique")
        save(self.path / "card.json", card)
        self.flag("click-intended")
        self.control("arm")
        click = self.probe("open-card")
        require(click.get("ok") is True and click["target"] == "open-card" and click["buttons_clear_after"] is True,
                "Production Library click not confirmed")
        save(self.path / "click.json", click)
        self.control("observe", deadline=65)
        self.control("stop", deadline=100)
        self.control("verify", deadline=70)
        save(self.path / "smoke.json", {"ordinary_library_click": True, "production_runtime_verified": True,
             "exact_production_arguments": True, "matching_lifecycle_ready": True, "framebuffer_and_input_fds": True,
             "managed_exit": True, "sign_in_exercised": False, "purchases_exercised": False, "no_user_data_exported": True})
        self.restore_navigation()
        self.control("verify", deadline=70)
        self.native_inventory(True)
        self.flag("keep-intended")

    def remove_stage(self):
        private_files = ["state/" + name for name in
                         ("before.pending", "before", "after", "found", "journal", "notice-dirs", "profile", "allowlist",
                          "installed", "armed", "ready", "worker", "stopped", "signalled", "committed", "remove", "checked",
                          "guard.pid", "guard.done", "runtime", "runtime.pending", "transient", "document-volume")]
        scripts = []
        for root, files in ((self.stage, self.info["stage_files"]), (self.priv, private_files)):
            require(len(files) <= 256 and all(safe_member(name) for name in files), "Unsafe cleanup roster")
            directories = sorted({str(parent) for name in files for parent in Path(name).parents if str(parent) != "."},
                                 key=lambda name: (-name.count("/"), name))
            scripts.append(f'''root={root}
if test -e "$root" || test -L "$root"; then
    test -d "$root" && test ! -L "$root" && test "$(readlink -f "$root")" = "$root" &&
        test -f "$root/owner" && test ! -L "$root/owner" && test "$(cat "$root/owner")" = {self.run} || exit 1
    cd "$root"
    for leaf in {' '.join(directories)}; do
        if test -e "$leaf" || test -L "$leaf"; then
            test -d "$leaf" && test ! -L "$leaf" && test "$(readlink -f "$leaf")" = "$root/$leaf" || exit 1
        fi
    done
    for leaf in {' '.join(files)}; do
        if test -e "$leaf" || test -L "$leaf"; then test -f "$leaf" && test ! -L "$leaf" && rm "$leaf" || exit 1; fi
    done
    for leaf in {' '.join(directories)}; do
        if test -d "$leaf"; then rmdir "$leaf" || exit 1; fi
    done
    test "$(ls -A)" = owner && rm owner && cd / && rmdir "$root" || exit 1
fi
test ! -e "$root" && test ! -L "$root"''')
        self.remote("sh -s", ("set -eu\n" + "\n".join(scripts) + "\n").encode("ascii"), deadline=110)

    def cleanup(self):
        keep = (self.path / "keep-intended").is_file()
        prepared = (self.path / "staging-intended").is_file()
        if prepared and not (self.path / "recovery-verified").exists():
            if (self.path / "install-intended").exists():
                if keep:
                    self.control("commit", deadline=90)
                else:
                    self.control("rollback", deadline=140)
                if not keep and (self.path / "original-navigation.json").is_file():
                    self.restore_navigation()
            self.native_inventory(keep)
            self.flag("recovery-verified")
        if prepared:
            self.remove_stage()
            self.native_inventory(keep)
        return keep

    def execute(self, recovering):
        require(not (self.path / "released.json").exists(), "Do not reuse a completed rollout")
        if recovering:
            device_lease.status(self.request)
        else:
            require(not (self.path / "started.json").exists() and not (self.path / "private-token").exists(),
                    "Do not reacquire for an interrupted rollout")
            data = (self.path / "payload.tar.gz").read_bytes()
            require(len(data) == self.info["payload_bytes"] and roster(self.path / "payload.tar.gz") == self.info["stage_files"],
                    "Prepared rollout payload does not match admission")
        self.askpass.write_text('#!/bin/sh\ncase "$1" in *assword:*) printf "%s\\n" "$BILI_ASKPASS";; *) exit 1;; esac\n')
        self.askpass.chmod(0o700)
        try:
            self.password = getpass.getpass("Kindle SSH password (not saved): ")
            require(bool(self.password), "Missing password")
            if not recovering:
                acquired = device_lease.acquire(self.request)
                save(self.path / "started.json", acquired)
            try:
                if not recovering:
                    self.stage_payload(data)
                    self.install_and_smoke()
            except BaseException as error:
                if not (self.path / "failure.json").exists():
                    save(self.path / "failure.json", {"type": type(error).__name__,
                         "detail": str(error) if isinstance(error, ValueError) else "Scoped operation interrupted"})
                raise
            finally:
                kept = self.cleanup()
                released = device_lease.release(self.request, native_restored_and_connections_closed=True)
                save(self.path / "released.json", released)
                print("Rollout closed; exact lease released. Installation retained:", kept, flush=True)
        finally:
            self.password = ""
            self.askpass.unlink(missing_ok=True)


if __name__ == "__main__":
    os.umask(0o077)
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    preparation = commands.add_parser("prepare")
    for name in ("comparison", "control", "candidate", "publication", "zip", "kit", "kit-receipt"):
        preparation.add_argument("--" + name, type=Path, required=True)
    for name in ("run", "recover"):
        operation = commands.add_parser(name)
        operation.add_argument("--batch", type=Path, required=True)
        operation.add_argument("--expected-host", required=True)
    args = parser.parse_args()
    if args.command == "prepare":
        prepare(args)
    else:
        def interrupt(signum, frame):
            raise InterruptedError(f"Signal {signum}")
        for sig in (signal.SIGTERM, signal.SIGHUP):
            signal.signal(sig, interrupt)
        Rollout(args.batch, args.expected_host).execute(args.command == "recover")
