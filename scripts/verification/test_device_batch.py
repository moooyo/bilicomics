"""Exercise automatic host finalization using only mocked child processes.

Run on test-env. Temporary private roots hold synthetic evidence; neither
device transport nor the shared device lease is accessed by these tests.
"""

from contextlib import redirect_stdout
from io import StringIO
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

from physical import device_batch


def setUpModule():
    if sys.platform != "linux" or os.environ.get("BILICOMICS_REMOTE_VERIFICATION") != "1":
        raise RuntimeError("Run device batch fixtures only through ssh test-env")


class DeviceBatchTests(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory(prefix="bilicomics-device-batch-")
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)
        self.evidence = self.root / "evidence"
        self.run_output = self.evidence / "run"
        self.run_output.mkdir(parents=True)
        self.config = {"root": str(self.root), "run_output": str(self.run_output),
                       "run_id": "fixture-batch-01", "owner_thread": "fixture-owner",
                       "raw_harness": str(self.root / "raw-harness"),
                       "closure_helper": str(self.root / "closure.py"),
                       "lease_utility": str(self.root / "lease.py"),
                       "lease_dir": str(self.root / "synthetic-shared.lock"),
                       "token_file": str(self.root / "synthetic-private-token")}
        self.identity = {key: self.config[key] for key in ("run_id", "owner_thread")}
        self.digest = "a" * 64
        self.state = {"run_id": self.config["run_id"],
                      "run_root": "/mnt/us/bilicomics-physical-" + self.config["run_id"],
                      "lease_owner": self.config["owner_thread"], "lease_run_id": self.config["run_id"],
                      "lease_token_sha256": self.digest, "phase": "stopped",
                      "watchdog": {"recovery_complete": True},
                      "recovery_checks": {"passed": True, "owned_process_snapshot_passed": True}}
        self.held = {**self.identity, "ok": True, "state": "held", "ownership_verified": True,
                     "lease_token_sha256": self.digest}
        self.closed = {**self.identity, "connections_closed": True, "active_owned_transports": [],
                       "unreadable_host_process_count": 0, "signals_sent": False,
                       "device_polled": False, "lease_accessed": False}
        self.released = {**self.identity, "ok": True, "state": "released",
                         "owner_and_token_checked": True, "native_restored_and_connections_closed": True,
                         "lease_token_sha256": self.digest}
        self.raw_code = 7
        self.closure_code = 0
        self.release_code = 0
        self.events = []
        self.saved_paths = []

    def child(self, command, **options):
        if command[0] == self.config["raw_harness"]:
            self.events.append("harness")
            (self.run_output / "run.json").write_text(json.dumps(self.state), encoding="utf-8")
            return subprocess.CompletedProcess(command, self.raw_code)
        self.assertEqual(command[0], sys.executable)
        self.assertEqual(options["stdout"], subprocess.DEVNULL)
        self.assertEqual(options["stderr"], subprocess.DEVNULL)
        path = Path(command[command.index("--output") + 1])
        self.assertEqual(path.parent, self.evidence)
        self.assertFalse(path.exists())
        self.saved_paths.append(path)
        if command[1] == self.config["closure_helper"]:
            self.events.append("closure")
            report, code = self.closed, self.closure_code
        else:
            self.assertEqual(command[1], self.config["lease_utility"])
            for option, field in (("--lease-dir", "lease_dir"), ("--token-file", "token_file"),
                                  ("--owner-thread", "owner_thread"), ("--run-id", "run_id")):
                self.assertEqual(command[command.index(option) + 1], self.config[field])
            if command[-1] == "status":
                self.events.append("status")
                report, code = self.held, 0
            else:
                self.assertEqual(command[-2:], ["release", "--native-restored-and-connections-closed"])
                self.events.append("release")
                report, code = self.released, self.release_code
        with path.open("x", encoding="utf-8") as stream:
            json.dump(report, stream)
        return subprocess.CompletedProcess(command, code)

    def invoke(self, command=None):
        output = StringIO()
        with patch.object(device_batch.subprocess, "run", side_effect=self.child), redirect_stdout(output):
            code = device_batch.execute(self.config, command or ["checkpoint", "--name", "fixture"])
        report = json.loads(output.getvalue())
        saved = json.loads(Path(report["report"]).read_text(encoding="utf-8"))
        self.assertEqual(saved, {key: value for key, value in report.items() if key != "report"})
        self.assertFalse(Path(self.config["lease_dir"]).exists())
        self.assertFalse(Path(self.config["token_file"]).exists())
        return code, report

    def test_failed_harness_recovers_closes_and_releases_without_losing_failure(self):
        code, report = self.invoke()
        self.assertEqual(self.events, ["harness", "status", "closure", "release"])
        self.assertEqual(code, 7)
        self.assertEqual(report["harness_returncode"], 7)
        self.assertEqual(report["status"], "failed")
        self.assertEqual(report["finalization"], "released")
        self.assertEqual(json.loads((self.run_output / "run.json").read_text()), self.state)

    def test_successful_stop_and_release_alias_finalize(self):
        for operation in ("stop", "release"):
            with self.subTest(operation=operation):
                self.config["run_id"] = self.state["run_id"] = self.state["lease_run_id"] = operation
                self.state["run_root"] = "/mnt/us/bilicomics-physical-" + operation
                for report in (self.held, self.closed, self.released):
                    report["run_id"] = operation
                self.raw_code = 0
                self.events.clear()
                code, report = self.invoke([operation])
                self.assertEqual(code, 0)
                self.assertEqual(report["finalization"], "released")
                self.assertEqual(self.events, ["harness", "status", "closure", "release"])

    def test_successful_start_and_checkpoint_keep_the_batch(self):
        self.raw_code = 0
        for operation in ("start", "checkpoint"):
            with self.subTest(operation=operation):
                self.events.clear()
                code, report = self.invoke([operation])
                self.assertEqual(code, 0)
                self.assertEqual(report["finalization"], "not_requested")
                self.assertEqual(self.events, ["harness"])

    def test_incomplete_recovery_never_calls_host_helpers(self):
        for field in ("phase", "watchdog", "recovery_checks"):
            with self.subTest(field=field):
                original = self.state.pop(field)
                self.events.clear()
                code, report = self.invoke()
                self.assertEqual(code, 7)
                self.assertEqual(report["status"], "needs_recovery")
                self.assertEqual(self.events, ["harness"])
                self.state[field] = original
        self.state["recovery_checks"]["owned_process_snapshot_passed"] = 1
        self.events.clear()
        self.invoke()
        self.assertEqual(self.events, ["harness"])

    def test_mismatched_run_identity_never_closes_or_releases(self):
        for field in ("run_id", "run_root", "lease_owner", "lease_run_id"):
            with self.subTest(field=field):
                original, self.state[field] = self.state[field], "other"
                self.events.clear()
                code, report = self.invoke()
                self.assertEqual(code, 7)
                self.assertEqual(report["reason"], "run_identity_mismatch")
                self.assertEqual(self.events, ["harness"])
                self.state[field] = original

    def test_replacement_lease_token_is_rejected_before_closure(self):
        self.held["lease_token_sha256"] = "b" * 64
        code, report = self.invoke()
        self.assertEqual(code, 7)
        self.assertEqual(report["reason"], "lease_identity_mismatch")
        self.assertEqual(self.events, ["harness", "status"])

    def test_failed_or_uncertain_closure_never_releases(self):
        for changes in ({"connections_closed": False}, {"active_owned_transports": [{"pid": 1}]},
                        {"unreadable_host_process_count": 1}, {"unreadable_host_process_count": False},
                        {"owner_thread": "peer"}, {"run_id": "peer"}, {"device_polled": True},
                        {"signals_sent": True}, {"lease_accessed": True}):
            with self.subTest(changes=changes):
                original = dict(self.closed)
                self.closed.update(changes)
                self.events.clear()
                code, report = self.invoke()
                self.assertEqual(code, 7)
                self.assertEqual(report["status"], "needs_recovery")
                self.assertEqual(self.events, ["harness", "status", "closure"])
                self.closed = original
        self.closure_code = 2
        self.events.clear()
        self.invoke()
        self.assertEqual(self.events, ["harness", "status", "closure"])

    def test_failed_release_preserves_failure_and_reports_recovery_needed(self):
        self.release_code = 1
        self.released["ok"] = False
        code, report = self.invoke()
        self.assertEqual(code, 7)
        self.assertEqual(report["status"], "needs_recovery")
        self.assertEqual(report["stage"], "lease_release")

    def test_successful_stop_with_failed_closure_returns_recovery_failure(self):
        self.raw_code = 0
        self.closure_code = 2
        code, report = self.invoke(["stop"])
        self.assertEqual(code, 2)
        self.assertEqual(report["harness_returncode"], 0)
        self.assertEqual(report["status"], "needs_recovery")
        self.assertEqual(self.events, ["harness", "status", "closure"])

    def test_successful_stop_without_recovery_evidence_returns_failure(self):
        self.raw_code = 0
        self.state["recovery_checks"]["passed"] = False
        code, report = self.invoke(["stop"])
        self.assertEqual(code, 2)
        self.assertEqual(report["status"], "needs_recovery")
        self.assertEqual(self.events, ["harness"])

    def test_host_helper_timeout_never_releases_and_preserves_original_failure(self):
        original = self.child
        def delayed(command, **options):
            if len(command) > 1 and command[1] == self.config["closure_helper"]:
                self.assertEqual(options["timeout"], 20)
                self.events.append("closure-timeout")
                raise subprocess.TimeoutExpired(command, 20)
            return original(command, **options)
        self.child = delayed
        code, report = self.invoke()
        self.assertEqual(code, 7)
        self.assertEqual(self.events, ["harness", "status", "closure-timeout"])
        self.assertEqual(report["status"], "needs_recovery")
        self.assertEqual(report["reason"], "host_helper_timeout")
        self.assertEqual(report["stage"], "transport_closure")

    def test_released_run_refuses_future_harness_calls_and_keeps_old_evidence(self):
        self.invoke()
        previous = {path: path.read_bytes() for path in self.evidence.glob("device-batch-*.json")}
        self.events.clear()
        code, report = self.invoke(["start"])
        self.assertEqual(code, 2)
        self.assertEqual(report["reason"], "run_already_released")
        self.assertEqual(self.events, [])
        for path, content in previous.items():
            self.assertEqual(path.read_bytes(), content)


if __name__ == "__main__":
    unittest.main(verbosity=2)
