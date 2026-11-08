#!/usr/bin/env python3
"""Build, identify, and freeze the native protocol test executable for a Linux ARM guest."""

import argparse
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import sys
import time

from native_protocol import build_input_manifest, run, sha256

TARGET = "armv7-unknown-linux-musleabihf"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source-root", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument("--target-dir", type=Path, required=True)
    parser.add_argument("--jobs", type=int, default=os.environ.get("CARGO_BUILD_JOBS") or "12",
                        help="Maximum concurrent Cargo build jobs (CARGO_BUILD_JOBS, otherwise 12)")
    arguments = parser.parse_args()
    if platform.system() != "Linux" or arguments.jobs < 1:
        parser.error("Run on the designated Linux remote host with a positive job limit")
    source = arguments.source_root.resolve(strict=True)
    output = arguments.output_dir.resolve()
    output.mkdir(parents=True, exist_ok=False)
    report = {"schema":1,"verification":"native-protocol-arm-build","passed":False,
              "execution_performed":False,"application_acceptance":False,"physical_device":False,
              "target":TARGET,"source_root":str(source),"started_unix":int(time.time())}
    try:
        source_inputs = build_input_manifest(source)
        manifest = output / "source-inputs.sha256"
        manifest.write_text(source_inputs, encoding="utf-8")
        report["source_manifest_sha256"] = sha256(manifest)
        runners = output / "runner-inputs.sha256"
        runners.write_text("".join(f"{sha256(path)}  {path.relative_to(source).as_posix()}\n"
                                   for path in sorted((source / "scripts/verification").glob("native_protocol*"))
                                   if path.is_file()), encoding="utf-8")
        report["runner_manifest_sha256"] = sha256(runners)
        environment = os.environ.copy()
        environment["CARGO_BUILD_JOBS"] = str(arguments.jobs)
        environment["CARGO_TARGET_DIR"] = str(arguments.target_dir.resolve())
        report["build_environment"] = {key:environment.get(key, "")
                                        for key in ("CARGO_BUILD_JOBS","CARGO_TARGET_DIR","RUSTFLAGS")}
        command = ["cargo","zigbuild","--release","--locked","--offline","--target",TARGET,
                   "--no-default-features","--features","kindle","--test","native_protocol",
                   "--message-format=json"]
        report["build_command"] = command
        run(command, source, environment, output / "build.jsonl", 1800)
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
            raise RuntimeError("Cargo did not identify one ARM protocol test executable")
        built = artifacts[0]
        executable = output / ("native-protocol-armv7-" + sha256(built)[:12])
        shutil.copyfile(built, executable)
        executable.chmod(0o755)
        report["test_binary"] = {"path":str(executable),"sha256":sha256(executable),"bytes":executable.stat().st_size}
        description = subprocess.check_output(["file",str(executable)], text=True, timeout=30)
        elf = subprocess.check_output(["readelf","-h","-l","-d",str(executable)], text=True, timeout=30)
        (output / "elf.txt").write_text(description + elf, encoding="utf-8")
        report["elf"] = {"arm":"Machine:                           ARM" in elf,
                          "hard_float":"hard-float ABI" in elf,
                          "no_interpreter":"INTERP" not in elf,
                          "no_dynamic_section":"There is no dynamic section" in elf}
        if not all(report["elf"].values()):
            raise RuntimeError("The ARM executable does not satisfy the intended static hard-float ABI")
        report["source_inputs_unchanged"] = build_input_manifest(source) == source_inputs
        if not report["source_inputs_unchanged"]:
            raise RuntimeError("Build inputs changed during ARM protocol test compilation")
        report["guest_invocation"] = {"environment":{"BILICOMICS_SIGNING_ASSET_DIR":"GUEST_PINNED_ASSET_DIRECTORY"},
                                      "arguments":["--ignored","--nocapture","--test-threads=1"]}
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
