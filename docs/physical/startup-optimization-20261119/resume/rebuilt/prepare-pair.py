"""Prepare independently rebuilt A/B inputs on test-env; this program has no device transport."""

import argparse
import io
import json
import os
from pathlib import Path
import re
import secrets
import socket
import tarfile
import zipfile


ARMS = ("A", "B")
SCHEDULE = [(arm, pair) for pair in range(7) for arm in (ARMS if pair == 0 or pair % 2 else ARMS[::-1])]
LEAVES = ("launch.sh", "app/bilicomics", "app/kindle-jpeg-worker")
COMMITS = {"A": "d81b529fd8f84790d91dfe3a8396853c296a8280", "B": "57265de60553ea97b01894592427d60c9008f850"}
SOURCE_BASE = "ab52019569ec8431e145a5cd0bc7cbf58c340ce0"
SHELF_VARIANT = "shelf-private-rgb-v1"
ARCHIVE = Path("app/artifacts/verification/bilicomics-kindle-0.1.25-dev.zip")
MAIN_BINARY = "bilicomics/app/bilicomics"
SELECTED = {"bilicomics/" + leaf for leaf in LEAVES} | {"documents/bilicomics.sh"}


def require(condition, reason):
    if not condition:
        raise ValueError(reason)


def replace_once(text, before, after):
    require(text.count(before) == 1, "The selected source does not have the expected instrumentation boundary")
    return text.replace(before, after)


def adapt_launcher(data, run, arm):
    stage = "/mnt/us/bilicomics-" + run
    runtime = "/var/tmp/bilicomics-" + run
    prelude = f'''BENCH_STAGE={stage}
BENCH_PRIVATE={runtime}
test "$(cat "$BENCH_STAGE/owner")" = {run} && test "$(cat "$BENCH_PRIVATE/owner")" = {run} || exit 1
IFS= read -r BENCH_CASE <"$BENCH_PRIVATE/active" || exit 1
case "$BENCH_CASE" in {arm.lower()}0[0-6]) :;; *) exit 1;; esac
BENCH_OUT=$BENCH_PRIVATE/$BENCH_CASE
DATA_DIR=$BENCH_STAGE/cases/$BENCH_CASE/data
test -d "$BENCH_OUT" && test -d "$DATA_DIR" && test "$(cat "$BILICOMICS_HOME/.bench-arm")" = {arm} || exit 1
export BILICOMICS_BENCH_RUN={run} BILICOMICS_BENCH_CASE=$BENCH_CASE BILICOMICS_LAUNCH_TRACE=1'''
    text = replace_once(data.decode("ascii"), "DATA_DIR=$BILICOMICS_HOME/data", prelude)
    boundary = 'case "$BILICOMICS_HOME" in'
    text = replace_once(text, boundary, '''case "${1:-}" in
    --library) bench_role=entry;; --detached) bench_role=worker;; *) exit 1;;
esac
read_process "$$" || exit 1
(set -C; printf '%s %s\\n' "$$" "$PROCESS_BIRTH" >"$BENCH_OUT/$bench_role.pid") || exit 1
''' + boundary)
    command = '"$APP_DIR/bilicomics" --kindle --device "$PROFILE" --data-dir "$DATA_DIR" >&6'
    extra = (' --demo --offline --screen shelf --performance-trace "$BENCH_OUT/performance.jsonl"'
             ' --lifecycle-trace "$BENCH_OUT/lifecycle.jsonl" --capture "$BENCH_OUT/final.png" --exit-after-ms 4500')
    text = replace_once(text, command, command.replace(" >&6", extra + " >&6"))
    text = replace_once(text, '        exit "$LIBRARY_STATUS"', '''        printf '%s\\n' "$LIBRARY_STATUS" >"$BENCH_OUT/entry.status"
        exit "$LIBRARY_STATUS"''')
    return text.encode("ascii")


def canonical_entry(run):
    return f'''#!/bin/sh
set -eu
test "$#" = 1 && test "$1" = --library || exit 1
stage=/mnt/us/bilicomics-{run}
private=/var/tmp/bilicomics-{run}
test "$(cat /mnt/us/bilicomics/.native-library-owner)" = {run} &&
    test "$(cat "$stage/owner")" = {run} && test "$(cat "$private/owner")" = {run} || exit 1
IFS= read -r name <"$private/active" || exit 1
case "$name" in a0[0-6]) arm=A;; b0[0-6]) arm=B;; *) exit 1;; esac
test -d "$private/$name/armed" && test ! -L "$private/$name/armed" || exit 1
mkdir "$private/$name/claimed" || exit 1
BILICOMICS_HOME=$stage/park-$arm
export BILICOMICS_HOME
exec sh "$BILICOMICS_HOME/launch.sh" --library
'''.encode("ascii")


def package(directory, expected):
    build = json.loads((directory / "evidence/build-receipt.json").read_text())
    checks = json.loads((directory / "evidence/package-static.json").read_text())
    require(build["passed"] and checks["passed"] and build["source_commit"] == expected and len(checks["archive_entries"]) == 49,
            "Source or package qualification does not match the selected arm")
    selected = directory / ARCHIVE
    require(selected.stat().st_size == checks["inputs"]["archive_bytes"], "Selected archive size differs from the static report")
    with zipfile.ZipFile(selected) as archive:
        require(len(archive.namelist()) == len(set(archive.namelist())) == 49, "Unexpected archive roster")
        return {name: archive.read(name) for name in SELECTED}


def qualified_control(directory):
    software = json.loads((directory / "evidence/software.json").read_text())
    require(software["passed"] is True and software["physical_device_accessed"] is False
            and software["source_candidate_commit"] == COMMITS["B"], "Missing qualified November 20 control")
    return package(directory, COMMITS["B"])


def compare_archives(control, candidate, built):
    with zipfile.ZipFile(control) as original, zipfile.ZipFile(candidate) as changed:
        names = original.namelist()
        require(len(names) == len(set(names)) == 49 and sorted(names) == sorted(changed.namelist())
                and SELECTED <= set(names), "App-only candidate archive roster changed")
        for name in names:
            before, after = original.getinfo(name), changed.getinfo(name)
            require(before.external_attr == after.external_attr, "Package member mode changed: " + name)
            if name == MAIN_BINARY:
                data = changed.read(name)
                require(data != original.read(name) and data == built.read_bytes(), "Candidate is unchanged or not the built ARM app")
                require(data[:7] == b"\x7fELF\x01\x01\x01" and data[18:20] == b"(\0", "Candidate must be an ARM32 ELF")
            else:
                require(original.read(name) == changed.read(name), "Non-application member changed: " + name)
        return {name: changed.read(name) for name in SELECTED}


def shelf_candidate(control, candidate):
    reports = [json.loads((candidate / "evidence" / filename).read_text())
               for filename in ("build-receipt.json", "package-static.json", "software.json")]
    build, checks, software = reports
    require(all(report.get("passed") is True and report.get("source_base_commit") == SOURCE_BASE and
                report.get("source_variant") == SHELF_VARIANT for report in reports), "Unqualified Shelf working-tree variant")
    require(build.get("target") == "armv7-unknown-linux-musleabihf" and build.get("cargo_jobs") == 2 and
            build.get("source_commit") is None and build.get("source_files") and
            {"lto=thin", "opt-level=3", "strip=symbols", "codegen-units=1"} <= set(build["compiler_invocations"]["bilicomics:bin"]),
            "Missing actual Shelf source or release build receipt")
    require(software.get("physical_device_accessed") is False and software.get("backend_tests_passed", 0) > 0 and
            software.get("targeted_tests_passed", 0) > 0, "Missing Shelf software gate")
    require(checks.get("changed_members") == [MAIN_BINARY] and len(checks["archive_entries"]) == 49 and
            (candidate / ARCHIVE).stat().st_size == checks["inputs"]["archive_bytes"], "Missing app-only package gate")
    for leaf in build["source_files"]:
        relative = Path(leaf)
        require(relative.parts and relative.parts[0] in ("src", "vendor") and ".." not in relative.parts and
                (candidate / "app" / relative).read_bytes() == (candidate / "evidence/source" / relative).read_bytes(),
                "Qualified source no longer matches the frozen build inputs")
    return compare_archives(control / ARCHIVE, candidate / ARCHIVE, candidate / "kindle-package" / MAIN_BINARY)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("work", "control", "candidate", "kit", "seed", "frame-reader"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--same-package", action="store_true")
    parser.add_argument("--shelf-render", action="store_true", help="Compare the November 20 candidate with the app-only private RGB experiment")
    args = parser.parse_args()
    os.umask(0o077)
    owner = os.environ.get("CODEX_THREAD_ID", "")
    require(socket.gethostname().split(".")[0] == "test-env" and os.geteuid() == 0 and
            re.fullmatch(r"[a-f0-9]{8}(?:-[a-f0-9]{4}){3}-[a-f0-9]{12}", owner), "Use the receiving owner on test-env")
    root = args.work.resolve(strict=True)
    require(root.stat().st_uid == 0 and root.stat().st_mode & 0o777 == 0o700, "Use a private work directory")
    run = "lib-p-" + secrets.token_hex(4)
    batch = root / run
    batch.mkdir(mode=0o700)
    cases = [arm.lower() + f"{pair:02}" for arm, pair in SCHEDULE]
    if args.shelf_render:
        arms = {"A": qualified_control(args.control)}
        arms["B"] = arms["A"] if args.same_package else shelf_candidate(args.control, args.candidate)
        source = {"A": COMMITS["B"], "B": COMMITS["B"] if args.same_package else
                  {"source_base_commit": SOURCE_BASE, "source_variant": SHELF_VARIANT}}
    else:
        software = json.loads((args.candidate / "evidence/software.json").read_text())
        require(software["passed"] and not software["physical_device_accessed"] and software["source_candidate_commit"] == COMMITS["B"],
                "Missing candidate software qualification")
        arms = {"A": package(args.control, COMMITS["A"])}
        arms["B"] = arms["A"] if args.same_package else package(args.candidate, COMMITS["B"])
        source = {"A": COMMITS["A"], "B": COMMITS["A" if args.same_package else "B"]}
    doc = arms["A"]["documents/bilicomics.sh"]
    require(doc == arms["B"]["documents/bilicomics.sh"] and doc.endswith(b'exec sh /mnt/us/bilicomics/launch.sh --library\n'),
            "Not the genuine common Library entry")
    members = {"document": doc, "tools/control.sh": Path(__file__).with_name("device-pair.sh").read_bytes(),
               "tools/entry.sh": canonical_entry(run)}
    members["tools/frame-reader"] = args.frame_reader.read_bytes()
    require(members["tools/frame-reader"][:7] == b"\x7fELF\x01\x01\x01" and members["tools/frame-reader"][18:20] == b"(\0",
            "Expected the project's ARM32 read-only framebuffer reader")
    sizes = {}
    for arm, files in arms.items():
        sizes[arm] = {}
        for leaf in LEAVES:
            data = files["bilicomics/" + leaf]
            if leaf == "launch.sh":
                data = adapt_launcher(data, run, arm)
            members[f"park-{arm}/{leaf}"] = data
            sizes[arm][leaf] = len(data)
        members[f"park-{arm}/.native-library-owner"] = (run + "\n").encode()
        members[f"park-{arm}/.bench-arm"] = (arm + "\n").encode()
    with tarfile.open(args.kit, "r:gz") as kit:
        for item in kit.getmembers():
            require(item.isfile() and re.fullmatch(r"(card-input|native-target|lib/[A-Za-z0-9.+_-]+|models/(?:eng|chi_sim)\.traineddata)", item.name),
                    "Unrecognized OCR kit member")
            members["probe/" + item.name] = kit.extractfile(item).read()
    require("probe/card-input" in members and "probe/lib/ld-linux-armhf.so.3" in members, "Missing input runtime")
    for path in sorted(args.seed.rglob("*")):
        require(not path.is_symlink(), "Demo seed must not contain links")
        if path.is_file():
            name = path.relative_to(args.seed).as_posix()
            require(name.startswith("demo/") and re.fullmatch(r"[-a-zA-Z0-9_/.\u0080-\uffff]+", name)
                    and not name.endswith(("-wal", "-shm", "-journal")), "Use only a closed synthetic demo seed")
            members["seed/" + name] = path.read_bytes()
    require("seed/demo/state.sqlite3" in members and sum(len(data) for name, data in members.items() if name.startswith("seed/")) < 8 * 1024**2,
            "Missing or oversized desktop demo fixture")
    members.update({"seed/device-profile": b"scribe\n", "seed/device-allowlist": b"scribe 5.19.5\n"})
    with tarfile.open(batch / "payload.tar.gz", "x:gz", compresslevel=2) as archive:
        for name, data in sorted(members.items()):
            info = tarfile.TarInfo(name)
            info.size = len(data)
            info.mode = 0o755 if name.startswith(("park-", "tools/", "probe/")) else 0o600
            archive.addfile(info, io.BytesIO(data))
    admission = {"protocol": "shelf-render-click-3" if args.shelf_render else "rebuilt-library-click-2",
                 "run": run, "owner": owner, "dry_only": args.same_package, "schedule": cases, "expected_sizes": sizes,
                 "source": source,
                 "kit": "device-local recognition; no native image or unrecognized title extraction",
                 "payload_bytes": (batch / "payload.tar.gz").stat().st_size, "members": sorted(members),
                 "host_boot": Path("/proc/sys/kernel/random/boot_id").read_text().strip()}
    (batch / "admission.json").write_text(json.dumps(admission, indent=2) + "\n")
    print(batch, flush=True)


if __name__ == "__main__":
    main()
