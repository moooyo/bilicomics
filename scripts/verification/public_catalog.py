#!/usr/bin/env python3
"""Run the explicit, anonymous public-metadata check on the remote Linux worker."""

import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source-root", type=Path, required=True)
    parser.add_argument("--target-dir", type=Path, required=True)
    parser.add_argument("--asset-dir", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    args = parser.parse_args()
    if sys.platform != "linux":
        parser.error("Run this check only on the designated remote Linux worker")
    source = args.source_root.resolve(strict=True)
    output = args.output_dir.resolve()
    output.mkdir(parents=True, exist_ok=False)
    sys.path.insert(0, str(source / "scripts/verification"))
    from native_protocol import build_input_manifest, run, sha256

    inputs = build_input_manifest(source)
    (output / "source-inputs.sha256").write_text(inputs)
    report = {"passed": False, "physical_device": False, "live_account_verified": False,
              "scope": "At most three allowlisted anonymous public-metadata requests",
              "source_manifest_sha256": sha256(output / "source-inputs.sha256"),
              "runner_sha256": sha256(Path(__file__)),
              "shared_process_helper_sha256": sha256(source / "scripts/verification/native_protocol.py")}
    environment = dict(os.environ, CARGO_TARGET_DIR=str(args.target_dir.resolve()),
                       CARGO_BUILD_JOBS=os.environ.get("CARGO_BUILD_JOBS") or "12",
                       BILICOMICS_SIGNING_ASSET_DIR=str(args.asset_dir.resolve(strict=True)),
                       BILICOMICS_PUBLIC_CATALOG_OUTPUT=str(output / "service-result.json"))
    try:
        command = ["cargo", "test", "--locked", "--offline", "--test", "public_catalog",
                   "--no-run", "--message-format=json"]
        report["build_command"] = command
        run(command, source, environment, output / "build.jsonl", 600)
        artifacts = []
        for line in (output / "build.jsonl").read_text().splitlines():
            try:
                event = json.loads(line)
            except json.JSONDecodeError:
                continue
            if event.get("reason") == "compiler-artifact" and event.get("target", {}).get("name") == "public_catalog":
                if event.get("executable"):
                    artifacts.append(Path(event["executable"]))
        if len(artifacts) != 1:
            raise RuntimeError("The build did not identify one public_catalog test executable")
        built = artifacts[0]
        binary = output / ("public-catalog-" + sha256(built)[:12])
        shutil.copyfile(built, binary)
        binary.chmod(0o500)
        report["test_binary"] = {"path": str(binary), "sha256": sha256(binary), "bytes": binary.stat().st_size}
        command = [str(binary), "--ignored", "--nocapture", "--test-threads=1"]
        report["test_command"] = command
        run(command, source, environment, output / "tests.log", 150)
        service = json.loads((output / "service-result.json").read_text())
        if not service.get("passed") or service.get("session_absent") is not True \
                or service.get("account_mutations") != 0 or len(service.get("requests", [])) != 3:
            raise RuntimeError("The anonymous public service contract did not pass")
        report["passed"] = True
    except Exception as error:
        report["failure"] = str(error)
    finally:
        if (output / "service-result.json").is_file():
            report["service_result"] = json.loads((output / "service-result.json").read_text())
        report["source_inputs_unchanged"] = inputs == build_input_manifest(source)
        if not report["source_inputs_unchanged"]:
            report["passed"] = False
            report["failure"] = "Build inputs changed during verification"
        (output / "summary.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({"passed": report["passed"], "summary": str(output / "summary.json"),
                      "failure": report.get("failure")}), flush=True)
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    sys.exit(main())
