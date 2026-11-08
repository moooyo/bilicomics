#!/usr/bin/env python3
"""Verify native signing on Linux using one pinned public asset and synthetic requests."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import sys
import time
import urllib.request

ASSET_FILE = "efae82c96a7eef44bee5.wasm"
ASSET_URL = "https://s1.hdslb.com/bfs/manga-static/manga-pc/" + ASSET_FILE
ASSET_SHA256 = "39bc0676953752c461197df592e1f5894f1a7492a29400c946e560fc109a8e2e"
GO_SHIM_URL = "https://raw.githubusercontent.com/golang/go/go1.25.0/lib/wasm/wasm_exec.js"
GO_SHIM_SHA256 = "0c949f4996f9a89698e4b5c586de32249c3b69b7baadb64d220073cc04acba14"
MAX_ASSET_BYTES = 16 * 1024 * 1024


def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def build_input_manifest(source):
    paths = [path for group in ("src", "ui", "assets", "vendor", "tests")
             for path in (source / group).rglob("*") if path.is_file()]
    paths += [source / name for name in ("Cargo.toml", "Cargo.lock", "build.rs")]
    configuration = source / ".cargo/config.toml"
    if configuration.is_file():
        paths.append(configuration)
    if any(path.is_symlink() for path in paths):
        raise RuntimeError("Build input files must not be symbolic links")
    return "".join(f"{sha256(path)}  {path.relative_to(source).as_posix()}\n" for path in sorted(paths))


def run(command, cwd, environment, log, timeout):
    with log.open("w", encoding="utf-8") as output:
        result = subprocess.run(command, cwd=cwd, env=environment, stdout=output,
                                stderr=subprocess.STDOUT, timeout=timeout, check=False)
    if result.returncode:
        raise RuntimeError(f"Command failed with exit {result.returncode}; inspect {log.name}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source-root", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument("--target-dir", type=Path)
    parser.add_argument("--jobs", type=int, default=os.environ.get("CARGO_BUILD_JOBS") or "12",
                        help="Maximum concurrent Cargo build jobs (CARGO_BUILD_JOBS, otherwise 12)")
    parser.add_argument("--asset-file", type=Path, help="Use an already acquired public module")
    parser.add_argument("--go-shim", type=Path, help="Use the digest-pinned official Go 1.25.0 shim")
    arguments = parser.parse_args()
    if platform.system() != "Linux":
        parser.error("Run this verifier on the designated Linux remote environment")
    if arguments.jobs < 1:
        parser.error("Cargo jobs must be a positive integer")
    source = arguments.source_root.resolve(strict=True)
    output = arguments.output_dir.resolve()
    output.mkdir(parents=True, exist_ok=False)
    assets = output / "public-assets"
    assets.mkdir()
    report = {"schema":1, "verification":"native-protocol", "passed":False,
              "live_account":False, "service_api_requests":0, "physical_device":False,
              "source_root":str(source), "started_unix":int(time.time()),
              "asset":{"url":ASSET_URL, "expected_sha256":ASSET_SHA256}}
    try:
        asset = assets / ASSET_FILE
        if arguments.asset_file:
            shutil.copyfile(arguments.asset_file, asset)
            report["asset"]["acquisition"] = "supplied_public_asset"
        else:
            request = urllib.request.Request(ASSET_URL, headers={"User-Agent":"BiliComics-Protocol-Verification/0.1"})
            with urllib.request.urlopen(request, timeout=60) as response:
                data = response.read(MAX_ASSET_BYTES + 1)
            if len(data) > MAX_ASSET_BYTES:
                raise RuntimeError("The public module exceeds the pinned asset size bound")
            asset.write_bytes(data)
            report["asset"]["acquisition"] = "public_https_download"
        report["asset"].update({"bytes":asset.stat().st_size, "sha256":sha256(asset)})
        if report["asset"]["sha256"] != ASSET_SHA256:
            raise RuntimeError("The public signing module does not match the reviewed digest")
        source_inputs = build_input_manifest(source)
        manifest = output / "source-inputs.sha256"
        manifest.write_text(source_inputs, encoding="utf-8")
        report["source_manifest_sha256"] = sha256(manifest)
        report["source_inputs_scope"] = ["Cargo.toml", "Cargo.lock", "build.rs", ".cargo/config.toml", "src/**", "ui/**",
                                         "assets/**", "vendor/**", "tests/**"]
        runner_manifest = output / "runner-inputs.sha256"
        runner_manifest.write_text("".join(f"{sha256(path)}  {path.relative_to(source).as_posix()}\n"
                                            for path in sorted((source / "scripts/verification").glob("native_protocol*"))),
                                   encoding="utf-8")
        report["runner_manifest_sha256"] = sha256(runner_manifest)
        environment = os.environ.copy()
        environment["CARGO_BUILD_JOBS"] = str(arguments.jobs)
        if arguments.target_dir:
            environment["CARGO_TARGET_DIR"] = str(arguments.target_dir.resolve())
        environment["BILICOMICS_SIGNING_ASSET_DIR"] = str(assets)
        report["build_environment"] = {"CARGO_BUILD_JOBS":environment["CARGO_BUILD_JOBS"],
                                       "CARGO_TARGET_DIR":environment.get("CARGO_TARGET_DIR"),
                                       "RUSTFLAGS":environment.get("RUSTFLAGS", "")}
        build_command = ["cargo","test","--locked","--offline","--test","native_protocol",
                         "--no-run","--message-format=json"]
        report["build_command"] = build_command
        run(build_command, source, environment, output / "build.jsonl", 1200)
        artifacts = []
        for line in (output / "build.jsonl").read_text(encoding="utf-8").splitlines():
            try:
                item = json.loads(line)
            except json.JSONDecodeError:
                continue
            if item.get("reason") == "compiler-artifact" and item.get("target",{}).get("name") == "native_protocol":
                if item.get("executable"):
                    artifacts.append(Path(item["executable"]))
        if len(artifacts) != 1:
            raise RuntimeError("Cargo did not identify exactly one native protocol test executable")
        built_executable = artifacts[0]
        executable = output / ("native-protocol-" + sha256(built_executable)[:12])
        shutil.copyfile(built_executable, executable)
        executable.chmod(0o755)
        report["test_binary"] = {"path":str(executable),"sha256":sha256(executable),"bytes":executable.stat().st_size}
        test_command = [str(executable),"--ignored","--nocapture","--test-threads=1"]
        report["test_command"] = test_command
        run(test_command, source, environment, output / "tests.log", 240)
        cases = []
        for line in (output / "tests.log").read_text(encoding="utf-8").splitlines():
            marker = "NATIVE_PROTOCOL_CASE "
            if marker in line:
                cases.append(json.loads(line.split(marker,1)[1]))
        expected = {"official_callback_inputs","module_failures_and_recovery","input_validation",
                    "production_wire_contract","concurrent_runtime_isolation"}
        if {case["case"] for case in cases} != expected or len(cases) != len(expected):
            raise RuntimeError("The native suite did not produce all required case records")
        report["cases"] = cases
        synthetic = next(case["detail"]["reference_cases"] for case in cases if case["case"] == "official_callback_inputs")
        synthetic_path = output / "synthetic-signing-inputs.json"
        synthetic_path.write_text(json.dumps(synthetic, indent=2) + "\n", encoding="utf-8")
        shim = output / "go1.25.0-wasm_exec.js"
        if arguments.go_shim:
            shutil.copyfile(arguments.go_shim, shim)
        else:
            with urllib.request.urlopen(GO_SHIM_URL, timeout=60) as response:
                shim_bytes = response.read(1024 * 1024 + 1)
            if len(shim_bytes) > 1024 * 1024:
                raise RuntimeError("The official Go shim exceeds the reference size bound")
            shim.write_bytes(shim_bytes)
        if sha256(shim) != GO_SHIM_SHA256:
            raise RuntimeError("The official Go shim does not match the reviewed Go 1.25.0 digest")
        reference_command = ["node",str(source / "scripts/verification/native_protocol_reference.cjs"),
                             str(asset),str(shim),str(synthetic_path)]
        report["reference_command"] = reference_command
        report["go_shim"] = {"path":str(shim),"sha256":sha256(shim),"url":GO_SHIM_URL}
        run(reference_command, source, environment, output / "reference.log", 120)
        reference = json.loads((output / "reference.log").read_text(encoding="utf-8"))
        by_name = {case["name"]:case for case in reference["cases"]}
        comparisons = [{"name":case["name"],
                        "matched":by_name[case["name"]]["signature_sha256"] == case["signature_sha256"]}
                       for case in synthetic]
        report["independent_reference"] = {"host":"official-go-js-host","comparisons":comparisons}
        if not all(case["matched"] for case in comparisons):
            raise RuntimeError("The native signatures differ from the official Go/JavaScript host")
        native_by_name = {case["name"]:case["signature_sha256"] for case in synthetic}
        report["observations"] = {"same_input_is_deterministic":native_by_name["baseline"] == native_by_name["same_input"],
                                  "body_changes_signature":native_by_name["baseline"] != native_by_name["changed_body"],
                                  "timestamp_changes_signature":native_by_name["baseline"] != native_by_name["changed_timestamp"],
                                  "eot_only_change_changes_signature":native_by_name["baseline"] != native_by_name["changed_query"],
                                  "whitespace_changes_signature":native_by_name["baseline"] != native_by_name["changed_whitespace"]}
        report["source_inputs_unchanged"] = build_input_manifest(source) == source_inputs
        if not report["source_inputs_unchanged"]:
            raise RuntimeError("Build inputs changed during native protocol verification")
        report["passed"] = True
    except Exception as error:
        report["failure"] = str(error)
    finally:
        report["completed_unix"] = int(time.time())
        (output / "summary.json").write_text(json.dumps(report,indent=2) + "\n", encoding="utf-8")
    print(json.dumps({"passed":report["passed"],"summary":str(output / "summary.json"),
                      "failure":report.get("failure")}, indent=2))
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    sys.exit(main())
