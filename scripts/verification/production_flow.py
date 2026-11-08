#!/usr/bin/env python3
"""Run controlled source/cache subprocess checks in an isolated Linux network namespace."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys


def sha256(path):
    value = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            value.update(chunk)
    return value.hexdigest()


def source_manifest(source):
    files = [source / name for name in ("Cargo.toml", "Cargo.lock", "build.rs")]
    configuration = source / ".cargo/config.toml"
    if configuration.is_file():
        files.append(configuration)
    for name in ("src", "ui", "assets", "vendor", "tests"):
        files.extend(path for path in (source / name).rglob("*") if path.is_file())
    return "".join(f"{sha256(path)}  {path.relative_to(source).as_posix()}\n"
                   for path in sorted(files))


def run_logged(command, source, environment, stdout_path, stderr_path, timeout):
    process = subprocess.Popen(command, cwd=source, env=environment,
                               stdout=subprocess.PIPE,
                               stderr=subprocess.PIPE if stderr_path else subprocess.STDOUT,
                               text=True, start_new_session=True)
    failure = None
    try:
        stdout, stderr = process.communicate(timeout=timeout)
    except BaseException as error:
        failure = error
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        stdout, stderr = process.communicate()
    stdout_path.write_text(stdout or "", encoding="utf-8")
    if stderr_path:
        stderr_path.write_text(stderr or "", encoding="utf-8")
    if failure:
        raise failure
    return subprocess.CompletedProcess(command, process.returncode, stdout, stderr)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source-root", type=Path, required=True)
    parser.add_argument("--target-dir", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    arguments = parser.parse_args()
    if sys.platform != "linux":
        parser.error("This verification runner requires the designated remote Linux environment.")
    source = arguments.source_root.resolve(strict=True)
    target = arguments.target_dir.resolve()
    output = arguments.output_dir.resolve()
    output.mkdir(parents=True, exist_ok=False)
    manifest = source_manifest(source)
    (output / "source-inputs.sha256").write_text(manifest, encoding="utf-8")
    environment = dict(os.environ, CARGO_TARGET_DIR=str(target))
    environment["CARGO_BUILD_JOBS"] = environment.get("CARGO_BUILD_JOBS") or "12"
    environment.setdefault("RUST_TEST_THREADS", "1")
    command = ["cargo", "test", "--locked", "--offline", "--test", "production_flow",
               "--no-run", "--message-format=json"]
    report = {"passed": False, "evidence_kind": "controlled_production_services_subprocess",
              "physical_device": False, "live_account": False, "live_service_verified": False,
              "production_app_click_flow": False, "signer": "injected_fixture",
              "image_payload": "synthetic_plain_png_with_encryption_flag",
              "encrypted_content_decode_verified": False,
              "source_manifest_sha256": hashlib.sha256(manifest.encode()).hexdigest(),
              "runner_sha256": sha256(Path(__file__).resolve()),
              "commands": {"build": command}}
    try:
        build = run_logged(command, source, environment, output / "build-events.jsonl",
                           output / "build.log", timeout=600)
        report["build_exit_code"] = build.returncode
        if build.returncode:
            raise RuntimeError("The integration test build failed; inspect build.log.")
        executables = []
        for line in build.stdout.splitlines():
            event = json.loads(line)
            if event.get("reason") == "compiler-artifact" and event.get("target", {}).get("name") == "production_flow":
                if event.get("executable"):
                    executables.append(Path(event["executable"]))
        if len(executables) != 1:
            raise RuntimeError("The build did not identify exactly one production_flow test executable.")
        binary = executables[0]
        report["test_binary_sha256"] = sha256(binary)
        if source_manifest(source) != manifest:
            raise RuntimeError("Source inputs changed while the test executable was being built.")
        if not shutil.which("unshare"):
            raise RuntimeError("unshare is required; these tests must run without a network interface.")
        environment["BILICOMICS_FLOW_OUTPUT"] = str(output / "fixtures")
        command = ["unshare", "--net", "--", str(binary), "--nocapture", "--test-threads=1"]
        report["commands"]["test"] = command
        run = run_logged(command, source, environment, output / "tests.log", None, timeout=180)
        report["test_exit_code"] = run.returncode
        report["network_namespace"] = "unshare --net; no host or service network interfaces"
        if run.returncode:
            raise RuntimeError("The subprocess checks failed; inspect tests.log and fixture phase logs.")
        if source_manifest(source) != manifest or sha256(binary) != report["test_binary_sha256"]:
            raise RuntimeError("Source inputs or the test executable changed during execution.")
        cases = {}
        for name in ("source-services", "transaction-startup"):
            result = json.loads((output / "fixtures" / name / "result.json").read_text())
            if not result.get("passed"):
                raise RuntimeError(f"The {name} case did not pass.")
            cases[name] = result
        report["cases"] = cases
        report["case_count"] = len(cases)
        report["passed"] = True
        report["synthetic_app_data"] = "fixtures/source-services/data"
        return 0
    except (Exception, KeyboardInterrupt) as error:
        report["error"] = str(error) or type(error).__name__
        print(report["error"], file=sys.stderr)
        return 1
    finally:
        (output / "result.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")


if __name__ == "__main__":
    sys.exit(main())
