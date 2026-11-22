"""Compile the exact study overlay with mocked hardware and real Linux authorization."""

import importlib.util
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import time
import tomllib

from stage_overlay import exact, generate


HERE = Path(__file__).resolve().parent
RUN = "bili-shelf-0123456789abcdef0123456789abcdef"


def load(name):
    spec = importlib.util.spec_from_file_location(name, HERE / f"{name}.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def qualify_generator(base, work, overlay):
    for output in (overlay, base / "forbidden-display-overlay"):
        result = subprocess.run([sys.executable, "-B", HERE / "stage_overlay.py", base, output], capture_output=True)
        assert result.returncode == 1
    assert not (base / "forbidden-display-overlay").exists()
    second = work / "second-overlay"
    generate(base, second)
    leaves = sorted(path.relative_to(overlay) for path in overlay.rglob("*") if path.is_file())
    assert len(leaves) == 4 and all((overlay / leaf).read_bytes() == (second / leaf).read_bytes() for leaf in leaves)


def build(base, overlay, crate):
    source = crate / "src"
    for leaf in ("platform/native_capture.rs", "platform/capture_cost.rs", "native_presentation/portrait_writer.rs"):
        target = source / leaf
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(overlay / "src" / leaf, target)
    shutil.copyfile(base / "src/native_presentation.rs", source / "native_presentation.rs")
    fixture = (HERE / "host_fixture.rs").read_text()
    fixture = exact(fixture, "    pub mod native_capture;\n", "    pub mod native_capture;\n    pub(crate) mod capture_cost;\n")
    approval = (overlay / "src/platform/mod.rs").read_text().split("fn validate_native_launch_approval(", 1)[1].split("\n}\n", 1)[0]
    (source / "launch_approval.rs").write_text("fn validate_native_launch_approval(" + approval + "\n}\n")
    (source / "main.rs").write_text(fixture)
    locked = tomllib.loads((base / "Cargo.lock").read_text())
    dependencies = {entry["name"]: entry["version"] for entry in locked["package"] if entry["name"] in ("anyhow", "libc")}
    assert set(dependencies) == {"anyhow", "libc"}
    (crate / "Cargo.toml").write_text('[package]\nname="display-overlay-fixture"\nversion="0.0.0"\nedition="2024"\n'
                                     '[features]\ndefault=["kindle"]\nkindle=[]\nprofile=[]\n[dependencies]\n'
                                     + ''.join(f'{name}="={version}"\n' for name, version in dependencies.items()))
    subprocess.run(["cargo", "build", "--offline", "-j1"], cwd=crate, check=True, timeout=180,
                   env={**os.environ, "CARGO_TARGET_DIR": str(crate / "target"), "CARGO_INCREMENTAL": "0"})
    return crate / "target/debug/display-overlay-fixture"


def prepare_mount():
    assert os.getpid() > 1 and Path("/proc/1/cmdline").read_bytes().find(b"/opt/kindle-toolchain/ubuntu-run\0--mounted\0") >= 0, "Run inside the toolchain PID namespace"
    assert {line.split(":", 1)[0].strip() for line in Path("/proc/net/dev").read_text().splitlines()[2:]} == {"lo"}
    subprocess.run(["mount", "--make-rprivate", "/"], check=True)
    subprocess.run(["mount", "-t", "tmpfs", "-o", "mode=700,nosuid,nodev", "tmpfs", "/mnt"], check=True)
    Path("/mnt/us").mkdir(mode=0o700)


def exercise(base, work, binary):
    import ctypes
    bench = load("host_bench")
    collect = load("collect_display")
    assert ctypes.CDLL(None).prctl(36, 1, 0, 0, 0) == 0
    stage = Path("/mnt/us") / RUN
    tools = stage / "app"
    data = stage / "data"
    tools.mkdir(parents=True)
    data.mkdir()
    (stage / "run-id").write_text(RUN + "\n")
    shutil.copy2(binary, tools / "bilicomics")
    for alias in ("lipc-get-prop", "usleep"):
        os.link(tools / "bilicomics", tools / alias)
    sidecar = data / f"{RUN}.display-cost.json"
    startup = work / "capture.sh"
    bench.make_shell(base / "packaging/kindle/launch.sh", startup)
    resume = work / "resume.sh"
    resume.write_text(exact(startup.read_text(), "printf 'armed\\n'", r'''sh -c 'sleep 20 & printf "%s\n" "$!" > "$1/application-pid"; wait' sh "$LOCK_DIR" &
runner=$!
trap 'if [ -n "${application:-}" ]; then kill "$application" 2>/dev/null || :; fi; kill "$runner" 2>/dev/null || :; wait "$runner" 2>/dev/null || :' EXIT
while [ ! -s "$LOCK_DIR/application-pid" ]; do usleep 10000; done
IFS= read -r application < "$LOCK_DIR/application-pid"
printf '1:%s\n' "$LAUNCH_TOKEN" > "$LOCK_DIR/lifecycle-ready"
printf '1:%s:1:suspend\n' "$LAUNCH_TOKEN" > "$LOCK_DIR/lifecycle-request"
printf '1:%s:1:suspend:ok\n' "$LAUNCH_TOKEN" > "$LOCK_DIR/lifecycle-ack"
printf 'armed\n' '''.rstrip()))
    for profile, parsed, commands, script, mode, enabled in [
        ("scribe", "ready:Y8:U:0", 5, startup, "good", True),
        ("pw6", "ready:Y8:FIXED:0", 3, startup, "good", True),
        ("scribe", "ready:Y8:U:0", None, startup, "good", False),
        ("scribe", "ready:Y8:U:0", None, resume, "good", True),
        ("scribe", "not-active", None, startup, "inactive-first", True),
        ("scribe", "refused", None, startup, "bad-mode", True),
    ]:
        with tempfile.TemporaryDirectory(dir=work) as directory:
            env = {"PATH": str(tools) + ":" + os.defpath, "PROFILE": profile, "BILICOMICS_APPROVED_PROFILE": profile,
                   "BILICOMICS_APPROVED_FIRMWARE": "fixture", "BILICOMICS_HOME": str(stage),
                   "FIXTURE_MODE": mode, "FIXTURE_PROFILED": "0",
                   "FIXTURE_TOOLS": str(tools), "FIXTURE_GETTER": str(tools / "lipc-get-prop")}
            if enabled:
                env["BILICOMICS_DISPLAY_COST_RUN"] = RUN
            begin = time.clock_gettime_ns(time.CLOCK_MONOTONIC)
            result = subprocess.run(["/bin/sh", script], cwd=directory, env=env, input=b"read\n", capture_output=True,
                                    start_new_session=True, timeout=6, check=True)
            finish = time.clock_gettime_ns(time.CLOCK_MONOTONIC)
            bench.inspect_group()
            assert result.stdout == f"armed\nresult {parsed} 1\n".encode(), result.stdout
            if commands is None:
                assert not sidecar.exists()
                continue
            boot = Path("/proc/sys/kernel/random/boot_id").read_text().strip()
            filtered = subprocess.run(["/bin/sh", "-ec", collect.capture_command(RUN, str(stage), boot)],
                                      capture_output=True, timeout=3, check=True).stdout
            assert filtered == sidecar.read_bytes()
            row = collect.decode(filtered, RUN, boot, os.getpid())
            collect.components(row, begin, finish)
            assert row["buckets"]["commands_run"]["count"] == commands
            sidecar.unlink()
    print("Guarded automatic and fixed mock captures and strict Rust-to-Python decoding passed; disabled, resume, inactive, and rejected reads created no sidecar.")


def main():
    base = Path(sys.argv[1]).resolve(strict=True)
    with tempfile.TemporaryDirectory(dir=Path(sys.argv[2]).resolve(strict=True)) as directory:
        work = Path(directory)
        overlay = work / "overlay"
        generate(base, overlay)
        qualify_generator(base, work, overlay)
        prepare_mount()
        exercise(base, work, build(base, overlay, work / "crate"))


if __name__ == "__main__":
    main()
