#!/usr/bin/env python3
"""Finish a stopped physical batch before returning control to its coordinator.

Run only on test-env. The private JSON config supplies root, run_output,
run_id, owner_thread, raw_harness, closure_helper, lease_utility, lease_dir,
and token_file. raw_harness is an executable accepting the original harness
subcommand. Invoke this wrapper through exec so no launcher remains active.
Only host evidence and existing host helpers are used for finalization.
"""

import argparse
import json
from pathlib import Path
import subprocess
import sys
import uuid


def require(condition, reason):
    if not condition:
        raise RuntimeError(reason)


def read_report(path):
    value = json.loads(path.read_text(encoding="utf-8"))
    require(isinstance(value, dict), "invalid_report")
    return value


def host_report(command, output):
    try:
        result = subprocess.run(command, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=False, timeout=20)
    except subprocess.TimeoutExpired as error:
        raise RuntimeError("host_helper_timeout") from error
    require(result.returncode == 0, "host_helper_failed")
    return read_report(output)


def execute(config, command):
    require(sys.platform == "linux", "use_test_env")
    root = Path(config["root"])
    evidence, run_output = root / "evidence", Path(config["run_output"])
    require(root.is_absolute() and root.resolve() == root and evidence.is_dir()
            and run_output.resolve() == evidence / "run", "invalid_private_root")
    require(command and not command[0].startswith("-"), "missing_harness_subcommand")
    identity = {"run_id": config["run_id"], "owner_thread": config["owner_thread"]}
    stem = "device-batch-" + uuid.uuid4().hex
    output = evidence / (stem + ".json")
    report = {"schema_version": 1, "root": str(root), **identity, "command": command[0],
              "status": "needs_recovery", "finalization": "not_completed", "harness_returncode": None}
    code = 0
    with output.open("x", encoding="utf-8") as stream:
        try:
            for receipt in evidence.glob("device-batch-*.release.json"):
                previous = read_report(receipt)
                require(not (previous.get("ok") is True and previous.get("state") == "released"
                             and previous.get("run_id") == identity["run_id"]), "run_already_released")
            try:
                code = subprocess.run([config["raw_harness"], *command], check=False).returncode
            except OSError:
                code = 1
                report["harness_launch_failed"] = True
            report["harness_returncode"] = code
            if code == 0 and command[0] not in ("stop", "release"):
                report.update(status="completed", finalization="not_requested")
            else:
                report["stage"] = "recovery_evidence"
                state = read_report(run_output / "run.json")
                require(state.get("run_id") == identity["run_id"]
                        and state.get("run_root") == "/mnt/us/bilicomics-physical-" + identity["run_id"]
                        and state.get("lease_owner") == identity["owner_thread"]
                        and state.get("lease_run_id") == identity["run_id"], "run_identity_mismatch")
                checks = state.get("recovery_checks", {})
                require(state.get("phase") == "stopped"
                        and state.get("watchdog", {}).get("recovery_complete") is True
                        and checks.get("passed") is True
                        and checks.get("owned_process_snapshot_passed") is True, "recovery_incomplete")
                lease = [sys.executable, config["lease_utility"], "--lease-dir", config["lease_dir"],
                         "--token-file", config["token_file"], "--owner-thread", identity["owner_thread"],
                         "--run-id", identity["run_id"]]
                report["stage"] = "lease_identity"
                status_path = evidence / (stem + ".status.json")
                report["lease_status_report"] = str(status_path)
                held = host_report([*lease, "--output", str(status_path), "status"], status_path)
                digest = state.get("lease_token_sha256")
                require(isinstance(digest, str) and len(digest) == 64
                        and all(held.get(key) == value for key, value in identity.items())
                        and held.get("ok") is True and held.get("state") == "held"
                        and held.get("ownership_verified") is True
                        and held.get("lease_token_sha256") == digest, "lease_identity_mismatch")
                report["stage"] = "transport_closure"
                closed_path = evidence / (stem + ".closure.json")
                report["closure_report"] = str(closed_path)
                closed = host_report([sys.executable, config["closure_helper"], "--output", str(closed_path)], closed_path)
                require(all(closed.get(key) == value for key, value in identity.items())
                        and closed.get("connections_closed") is True
                        and closed.get("active_owned_transports") == []
                        and type(closed.get("unreadable_host_process_count")) is int
                        and closed["unreadable_host_process_count"] == 0
                        and all(closed.get(key) is False for key in ("signals_sent", "device_polled", "lease_accessed")),
                        "transports_not_closed")
                report["stage"] = "lease_release"
                release_path = evidence / (stem + ".release.json")
                report["release_report"] = str(release_path)
                released = host_report([*lease, "--output", str(release_path), "release",
                                        "--native-restored-and-connections-closed"], release_path)
                require(all(released.get(key) == value for key, value in identity.items())
                        and released.get("ok") is True and released.get("state") == "released"
                        and released.get("owner_and_token_checked") is True
                        and released.get("native_restored_and_connections_closed") is True
                        and released.get("lease_token_sha256") == digest, "release_not_confirmed")
                report.update(status="failed" if code else "completed", finalization="released")
        except (RuntimeError, OSError, ValueError, KeyError, TypeError, AttributeError) as error:
            report["reason"] = str(error) if isinstance(error, RuntimeError) else type(error).__name__
            code = code or 2
        json.dump(report, stream, indent=2)
        stream.write("\n")
    print(json.dumps({**report, "report": str(output)}, indent=2))
    return code if code >= 0 else 128 - code


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--config", type=Path, required=True)
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args(argv)
    command = args.command[1:] if args.command[:1] == ["--"] else args.command
    try:
        return execute(read_report(args.config), command)
    except (RuntimeError, OSError, ValueError, KeyError, TypeError) as error:
        reason = str(error) if isinstance(error, RuntimeError) else type(error).__name__
        print(json.dumps({"status": "needs_recovery", "reason": reason}), file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
