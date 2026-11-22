"""Compare the old and split recorders through the existing stock Linux read guard."""

import ctypes
import importlib.util
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import time

from stage_overlay import PARTITIONED_SITE, POLL_SITE, PREVIOUS, PREVIOUS_OVERLAY, exact, generate


HERE = Path(__file__).resolve().parent
RUN = "bili-shelf-0123456789abcdef0123456789abcdef"
OLD_SELECTOR = "BILICOMICS_DISPLAY_COST_RUN"
SPLIT_SELECTOR = "BILICOMICS_DISPLAY_POLL_RUN"
GETTER = r'''#!/bin/sh
set -eu
case "$*" in
    '-s com.lab126.powerd state')
        if [ "$FIXTURE_MODE" = inactive ]; then printf 'screenSaver\n'; else printf 'active\n'; fi ;;
    'com.lab126.winmgr accelerometer') printf 'L\n' ;;
    '-s com.lab126.winmgr epdcMode')
        case "$FIXTURE_MODE" in
            open) sleep 0.18; printf 'Y8\n' ;;
            eof|nonzero|timeout|revoke)
                printf 'Y8\n'
                exec 1>&-
                case "$FIXTURE_MODE" in
                    eof) sleep 0.18 ;;
                    nonzero) sleep 0.18; exit 7 ;;
                    timeout) exec sleep 6 ;;
                    revoke) rm "$BILICOMICS_LIFECYCLE_DIR/native-read-go"; exec sleep 6 ;;
                esac ;;
            *) printf 'Y8\n' ;;
        esac ;;
    *) exit 2 ;;
esac
'''


def module(path):
    spec = importlib.util.spec_from_file_location(path.stem, path)
    loaded = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(loaded)
    return loaded


def stage(base, work):
    old, split, duplicate = (work / leaf for leaf in ("old", "split", "duplicate"))
    PREVIOUS_OVERLAY.generate(base, old)
    generate(base, split)
    generate(base, duplicate)
    leaves = {path.relative_to(split) for path in split.rglob("*") if path.is_file()}
    assert len(leaves) == 4 and all((split / leaf).read_bytes() == (duplicate / leaf).read_bytes() for leaf in leaves)
    for leaf in ("src/platform/mod.rs", "src/native_presentation/portrait_writer.rs"):
        assert (old / leaf).read_bytes() == (split / leaf).read_bytes()
    native = "src/platform/native_capture.rs"
    assert exact((old / native).read_text(), POLL_SITE, PARTITIONED_SITE) == (split / native).read_text()
    for refused in (split, base / "invalid-poll-overlay"):
        result = subprocess.run([sys.executable, "-B", HERE / "stage_overlay.py", base, refused], capture_output=True)
        assert result.returncode == 1
    assert not (base / "invalid-poll-overlay").exists()
    return old, split


def exercise(base, work, binaries, prepare):
    prepare()
    bench = module(PREVIOUS / "host_bench.py")
    previous_collect = module(PREVIOUS / "collect_display.py")
    split_collect = module(HERE / "collect_display.py")
    assert ctypes.CDLL(None).prctl(36, 1, 0, 0, 0) == 0
    stage = Path("/mnt/us") / RUN
    tools, data = stage / "app", stage / "data"
    tools.mkdir(parents=True)
    data.mkdir()
    (stage / "run-id").write_text(RUN + "\n")
    boot = Path("/proc/sys/kernel/random/boot_id").read_text().strip()
    startup = work / "capture.sh"
    bench.make_shell(base / "packaging/kindle/launch.sh", startup)
    getter = tools / "lipc-get-prop"
    getter.write_text(GETTER)
    getter.chmod(0o700)
    cases = [
        ("scribe", "open", "ready:Y8:U:0", True),
        ("pw6", "eof", "ready:Y8:FIXED:0", True),
        ("scribe", "nonzero", "refused", False),
        ("scribe", "timeout", "refused", False),
        ("scribe", "revoke", "refused", False),
        ("scribe", "inactive", "not-active", False),
    ]
    for binary, selector, collector in zip(binaries, (OLD_SELECTOR, SPLIT_SELECTOR), (previous_collect, split_collect)):
        shutil.copy2(binary, tools / "bilicomics")
        if not (tools / "usleep").exists():
            os.link(tools / "bilicomics", tools / "usleep")
        other = SPLIT_SELECTOR if selector == OLD_SELECTOR else OLD_SELECTOR
        for profile, mode, reply, sidecar, selected in [
            *((*case, selector) for case in cases),
            ("scribe", "open", "ready:Y8:U:0", False, other),
        ]:
            env = {"PATH": str(tools) + ":" + os.defpath, "BILICOMICS_HOME": str(stage),
                   "PROFILE": profile, "BILICOMICS_APPROVED_PROFILE": profile, "BILICOMICS_APPROVED_FIRMWARE": "fixture",
                   "FIXTURE_MODE": mode, "FIXTURE_PROFILED": "0", "FIXTURE_TOOLS": str(tools),
                   "FIXTURE_GETTER": str(getter), selected: RUN}
            with tempfile.TemporaryDirectory(dir=work) as trial:
                begin = time.monotonic_ns()
                result = subprocess.run(["/bin/sh", startup], cwd=trial, env=env, input=b"read\n", capture_output=True,
                                        start_new_session=True, timeout=6, check=True)
                finish = time.monotonic_ns()
                bench.inspect_group()
            assert result.stdout == f"armed\nresult {reply} 1\n".encode(), (profile, mode, result.stdout)
            expected = data / collector.name_for(RUN)
            assert set(data.iterdir()) == ({expected} if sidecar else set())
            if not sidecar:
                continue
            filtered = subprocess.run(["/bin/sh", "-ec", collector.capture_command(RUN, str(stage), boot)],
                                      capture_output=True, timeout=3, check=True).stdout
            assert filtered == expected.read_bytes()
            wire = collector.decode(filtered, RUN, boot, os.getpid())
            projected = collector.components(wire, begin, finish)
            assert wire["buckets"]["commands_run"]["count"] == (5 if profile == "scribe" else 3)
            if selector == SPLIT_SELECTOR:
                parent = projected["poll_partition"]["parent"]
                opened, eof = (projected["poll_partition"][state] for state in ("stdout_open", "after_eof"))
                assert all(opened[field] + eof[field] == parent[field] for field in ("count", "elapsed_ns"))
                assert (opened["elapsed_ns"] > eof["elapsed_ns"]) == (mode == "open")
            expected.unlink()
    print("Old and split recorders preserved guarded outcomes and selector isolation; real split records passed the shell and Python partitions.")


def main():
    base, private = (Path(arg).resolve(strict=True) for arg in sys.argv[1:])
    harness = module(PREVIOUS / "test_overlay_fixture.py")
    with tempfile.TemporaryDirectory(dir=private) as directory:
        work = Path(directory)
        overlays = stage(base, work)
        binaries = [harness.build(base, overlay, work / f"crate-{number}") for number, overlay in enumerate(overlays)]
        exercise(base, work, binaries, harness.prepare_mount)


if __name__ == "__main__":
    main()
