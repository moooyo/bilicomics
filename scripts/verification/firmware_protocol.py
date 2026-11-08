#!/usr/bin/env python3
"""Run immutable native-protocol tests in the isolated ARM firmware guest."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import shlex
import subprocess
import sys
import time


ASSET_NAME = "efae82c96a7eef44bee5.wasm"
ASSET_SHA256 = "39bc0676953752c461197df592e1f5894f1a7492a29400c946e560fc109a8e2e"


def digest(path: Path) -> str:
    value = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            value.update(chunk)
    return value.hexdigest()


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--workspace", type=Path, required=True)
    parser.add_argument("--executable", type=Path, required=True)
    parser.add_argument("--asset", type=Path, required=True)
    parser.add_argument("--reference", type=Path, required=True)
    parser.add_argument("--synthetic-inputs", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--timeout-seconds", type=int, default=900)
    args = parser.parse_args()
    if sys.platform != "linux":
        raise SystemExit("Run this command only on the designated Linux verification host.")
    workspace = args.workspace.resolve(strict=True)
    if not str(workspace).startswith("/root/bilicomics-emulator-predevice-"):
        raise SystemExit("Only the explicitly isolated BiliComics firmware guest may be used.")
    if args.output.exists():
        raise SystemExit("Each protocol run requires a new output directory.")
    args.output.mkdir(parents=True)
    asset_hash = digest(args.asset)
    assert args.asset.name == ASSET_NAME and asset_hash == ASSET_SHA256, asset_hash
    executable_hash = digest(args.executable)
    environment = json.loads((workspace / "environment.json").read_text())
    qemu_pid = int((workspace / "qemu.pid").read_text())
    launches = workspace / "host-launches.jsonl"
    host_launch = json.loads(launches.read_text().splitlines()[-1]) if launches.exists() else None
    if host_launch is not None:
        assert host_launch["qemu_pid"] == qemu_pid, host_launch
    helper = workspace / "guest-ssh"
    key = workspace / "emulator/build/ssh/id_ed25519"
    guest_root = "/mnt/us/bilicomics-predevice/protocol/" + executable_hash[:12]
    guest_executable = guest_root + "/native-protocol-" + executable_hash[:12]

    def guest(*command: str, timeout=30, check=True):
        return subprocess.run([str(helper), shlex.join(command)], capture_output=True,
                              check=check, timeout=timeout)

    guest("test", "!", "-d", "/tmp/bilicomics-launcher.lock")
    guest("mkdir", "-p", guest_root)
    scp = ["scp", "-O", "-i", str(key), "-P", str(environment["ports"]["ssh"]),
           "-o", "BatchMode=yes", "-o", "UserKnownHostsFile=" + str(workspace / "known-hosts"),
           "-o", "HostKeyAlias=bili-s1"]
    subprocess.run([*scp, str(args.executable), "root@127.0.0.1:" + guest_executable],
                   check=True, capture_output=True, timeout=240)
    subprocess.run([*scp, str(args.asset), "root@127.0.0.1:" + guest_root + "/" + ASSET_NAME],
                   check=True, capture_output=True, timeout=90)
    guest("chmod", "755", guest_executable)
    assert guest("sha256sum", guest_executable).stdout.decode().split()[0] == executable_hash
    assert guest("sha256sum", guest_root + "/" + ASSET_NAME).stdout.decode().split()[0] == asset_hash
    report = {"schema_version": 1, "physical_device_accessed": False,
              "active_qemu_pid": qemu_pid, "active_host_launch": host_launch,
              "live_account_used": False, "harness_sha256": digest(Path(__file__)),
              "firmware_identity": guest("cat", "/etc/prettyversion.txt").stdout.decode().strip(),
              "architecture": guest("uname", "-m").stdout.decode().strip(),
              "executable_sha256": executable_hash, "asset_sha256": asset_hash,
              "test_executable_is_separate_from_application_binary": True,
              "official_reference_sha256": digest(args.reference),
              "synthetic_inputs_sha256": digest(args.synthetic_inputs),
              "result": "running"}
    (args.output / "summary.json").write_text(json.dumps(report, indent=2) + "\n")
    (args.output / "official-js-reference.json").write_bytes(args.reference.read_bytes())
    (args.output / "synthetic-signing-inputs.json").write_bytes(args.synthetic_inputs.read_bytes())
    command = "BILICOMICS_SIGNING_ASSET_DIR=" + shlex.quote(guest_root) + " " + shlex.join(
        (guest_executable, "--ignored", "--nocapture", "--test-threads=1"))
    started = time.monotonic()
    try:
        with (args.output / "native-protocol.log").open("wb") as output:
            result = subprocess.run([str(helper), command], stdout=output, stderr=subprocess.STDOUT,
                                    timeout=args.timeout_seconds)
    except Exception as error:
        report.update({"result": "failed", "error": repr(error),
                       "elapsed_seconds": round(time.monotonic() - started, 3)})
        (args.output / "summary.json").write_text(json.dumps(report, indent=2) + "\n")
        raise
    report["exit_code"] = result.returncode
    report["elapsed_seconds"] = round(time.monotonic() - started, 3)
    text = (args.output / "native-protocol.log").read_text()
    cases = []
    for line in text.splitlines():
        marker = line.find("NATIVE_PROTOCOL_CASE ")
        if marker >= 0:
            cases.append(json.loads(line[marker + len("NATIVE_PROTOCOL_CASE "):]))
    (args.output / "native-protocol-cases.json").write_text(json.dumps(cases, indent=2) + "\n")
    report["case_count"] = len(cases)
    try:
        assert result.returncode == 0, text
        assert "5 passed; 0 failed; 0 ignored" in text, text
        required = {"official_callback_inputs", "module_failures_and_recovery", "input_validation",
                    "production_wire_contract", "concurrent_runtime_isolation"}
        assert {case["case"] for case in cases} == required and len(cases) == 5, cases
        assert all(case.get("passed") is True and case.get("service_requests") == 0
                   and case.get("physical_device") is False and case.get("live_account") is False
                   for case in cases), cases
        callbacks = next(case for case in cases if case.get("case") == "official_callback_inputs")
        references = json.loads(args.reference.read_text())
        expected = {value["name"]: value["signature_sha256"] for value in references["cases"]}
        actual = {value["name"]: value["signature_sha256"] for value in callbacks["detail"]["reference_cases"]}
        inputs = json.loads(args.synthetic_inputs.read_text())
        assert {value["name"]: value for value in callbacks["detail"]["reference_cases"]} == {
            value["name"]: value for value in inputs}, "The ARM input contract differs from the host reference."
        assert len(expected) == len(actual) == 7, (expected, actual)
        assert actual == expected, (expected, actual)
        report["official_reference_cases"] = actual
        report["official_reference_matches"] = 7
        report["result"] = "passed"
    except Exception as error:
        report["result"] = "failed"
        report["error"] = repr(error)
        raise
    finally:
        (args.output / "summary.json").write_text(json.dumps(report, indent=2) + "\n")
        print(json.dumps(report), flush=True)


if __name__ == "__main__":
    main()
