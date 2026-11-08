#!/usr/bin/env python3
"""Test the Kindle dependency without changing the application's lockfile or sources."""

import argparse
import json
import os
from pathlib import Path
import re
import shutil
import signal
import subprocess
import sys
import tomllib


def interrupt(signum, frame):
    raise KeyboardInterrupt("Backend verification was interrupted")


def completed_test_results(output):
    """Read libtest terminal results even when stderr interrupts a status line."""
    pattern = re.compile(
        r"(?:^|\n)test\s+([\w:]+)\s+\.\.\.(.*?)(?=\ntest\s|\Z)",
        re.DOTALL,
    )
    results = {}
    for match in pattern.finditer(output):
        lines = match.group(2).strip().splitlines()
        terminal = lines[-1].strip() if lines else ""
        if terminal in {"ok", "FAILED", "ignored"}:
            results[match.group(1)] = terminal
    return results


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source-root", type=Path, required=True)
    parser.add_argument("--target-dir", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument("--baseline-touch", type=Path)
    parser.add_argument("--minimum-tests", type=int, default=0)
    args = parser.parse_args()
    if sys.platform != "linux":
        parser.error("Run this verifier only on the designated remote Linux worker")
    signal.signal(signal.SIGTERM, interrupt)
    signal.signal(signal.SIGHUP, interrupt)
    source = args.source_root.resolve(strict=True)
    output = args.output_dir.resolve()
    output.mkdir(parents=True, exist_ok=False)
    sys.path.insert(0, str(source / "scripts/verification"))
    from native_protocol import build_input_manifest, sha256
    from run_suite import stop_owned_group
    inputs = build_input_manifest(source)
    (output / "source-inputs.sha256").write_text(inputs)
    crate = output / "backend-crate"
    shutil.copytree(source / "vendor/slint-backend-kindle", crate,
                    ignore=shutil.ignore_patterns("target", "Cargo.lock", "__pycache__"))
    logs = output / "logs"
    logs.mkdir()
    environment = dict(os.environ, CARGO_TARGET_DIR=str(args.target_dir.resolve()),
                       CARGO_BUILD_JOBS=os.environ.get("CARGO_BUILD_JOBS") or "12",
                       CARGO_PROFILE_DEV_DEBUG="0", CARGO_PROFILE_TEST_DEBUG="0")
    environment.setdefault("CARGO_PROFILE_DEV_INCREMENTAL", "true")
    report = {"passed": False, "physical_device": False, "native_device_opened": False,
              "source_manifest_sha256": sha256(output / "source-inputs.sha256"),
              "runner_sha256": sha256(Path(__file__)),
              "process_cleanup_helper_sha256": sha256(source / "scripts/verification/run_suite.py"),
              "production_lock_sha256": sha256(source / "Cargo.lock"),
              "touch_sha256": sha256(crate / "src/touch.rs"), "commands": []}

    def invoke(label, command, timeout):
        log = logs / (label + ".log")
        report["commands"].append({"name": label, "arguments": [str(value) for value in command]})
        with log.open("w") as stream:
            process = subprocess.Popen([str(value) for value in command], cwd=crate,
                                       env=environment, stdout=stream, stderr=subprocess.STDOUT,
                                       start_new_session=True)
            try:
                result = process.wait(timeout=timeout)
            except BaseException:
                (logs / (label + ".termination.json")).write_text(json.dumps(stop_owned_group(process), indent=2) + "\n")
                raise
            if result:
                (logs / (label + ".termination.json")).write_text(json.dumps(stop_owned_group(process), indent=2) + "\n")
                raise RuntimeError(f"{label} exited with {result}; inspect {log.name}")
        return log

    def packages(path):
        return {(row["name"], row["version"], row.get("source")): row.get("checksum")
                for row in tomllib.loads(path.read_text())["package"]}

    def inherit_production_patches():
        production = tomllib.loads((source / "Cargo.toml").read_text())
        sections = []
        inputs = []
        for registry, entries in production.get("patch", {}).items():
            if registry != "crates-io":
                raise RuntimeError("The isolated verifier does not support a non-crates.io production patch")
            sections.append("\n[patch.crates-io]\n")
            for name, descriptor in entries.items():
                if not isinstance(descriptor, dict) or set(descriptor) - {"path", "version", "package"}:
                    raise RuntimeError("Production patches must use checked local paths in this verifier")
                path = (source / descriptor["path"]).resolve(strict=True)
                if not path.is_relative_to(source) or not (path / "Cargo.toml").is_file():
                    raise RuntimeError("A production patch escapes the frozen source")
                inherited = dict(descriptor, path=str(path))
                fields = ", ".join(f"{key} = {json.dumps(value)}" for key, value in inherited.items())
                sections.append(f"{json.dumps(name)} = {{ {fields} }}\n")
                inputs.append({"package": name, "path": str(path),
                               "manifest_sha256": sha256(path / "Cargo.toml")})
        with (crate / "Cargo.toml").open("a") as stream:
            stream.write("".join(sections))
        report["production_patch_inputs"] = inputs
        report["standalone_manifest_sha256"] = sha256(crate / "Cargo.toml")

    try:
        shutil.copyfile(source / "Cargo.lock", output / "production-Cargo.lock")
        shutil.copyfile(source / "Cargo.lock", crate / "Cargo.lock")
        inherit_production_patches()
        # Restrict lock resolution to the copied workspace package. Dependencies
        # retain their production versions; metadata would also attempt to fetch
        # unused Android and Apple sources on this offline Linux worker.
        invoke("lock-resolution", ["cargo", "update", "--workspace", "--offline"], 120)
        production, standalone = packages(source / "Cargo.lock"), packages(crate / "Cargo.lock")
        mismatches = [list(key) for key, checksum in standalone.items()
                      if key not in production or production[key] != checksum]
        report["standalone_lock_sha256"] = sha256(crate / "Cargo.lock")
        report["dependency_alignment"] = {"all_versions_and_checksums_match_production": not mismatches,
                                          "standalone_packages": len(standalone), "mismatches": mismatches}
        if mismatches:
            raise RuntimeError("The isolated dependency graph differs from the production lockfile")
        command = ["cargo", "test", "--locked", "--offline", "--lib", "--no-run", "--message-format=json"]
        build = invoke("build", command, 900)
        binaries = []
        for line in build.read_text().splitlines():
            try:
                event = json.loads(line)
            except json.JSONDecodeError:
                continue
            if event.get("reason") == "compiler-artifact" and event.get("target", {}).get("name") == "slint_backend_kindle":
                if event.get("executable"):
                    binaries.append(Path(event["executable"]))
        if len(binaries) != 1:
            raise RuntimeError("The backend build did not identify one test executable")
        binary = output / ("backend-tests-" + sha256(binaries[0])[:12])
        shutil.copyfile(binaries[0], binary)
        binary.chmod(0o500)
        report["test_binary"] = {"path": str(binary), "sha256": sha256(binary), "bytes": binary.stat().st_size}
        tests = invoke("tests", [binary, "--nocapture", "--test-threads=1"], 180).read_text()
        count = re.search(r"test result: ok\.\s+(\d+) passed; (\d+) failed; (\d+) ignored;", tests)
        if not count:
            raise RuntimeError("The backend did not report a passing executed test suite")
        report["tests"] = {name: int(value) for name, value in zip(("passed", "failed", "ignored"), count.groups())}
        if report["tests"]["passed"] < args.minimum_tests or report["tests"]["failed"] or report["tests"]["ignored"]:
            raise RuntimeError("The complete selected backend suite did not pass")
        names = lambda text: set(re.findall(r"#\[test\]\s*fn\s+(\w+)", text))
        selected = names((crate / "src/touch.rs").read_text())
        if args.baseline_touch:
            selected -= names(args.baseline_touch.read_text())
        report["checked_touch_tests"] = sorted(selected)
        completed = completed_test_results(tests)
        report["checked_touch_test_results"] = {
            name: completed.get(f"touch::tests::{name}") for name in sorted(selected)
        }
        if not selected or not all(result == "ok" for result in report["checked_touch_test_results"].values()):
            raise RuntimeError("One or more selected touch regressions were not executed")
        report["passed"] = True
    except Exception as error:
        report["error"] = str(error)
    finally:
        report["inputs_unchanged"] = inputs == build_input_manifest(source)
        if not report["inputs_unchanged"]:
            report["passed"] = False
        (output / "summary.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({"passed": report["passed"], "summary": str(output / "summary.json"),
                      "error": report.get("error")}), flush=True)
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    sys.exit(main())
