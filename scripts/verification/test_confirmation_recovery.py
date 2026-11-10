"""Exercise host confirmation failures without any real device transport.

Run only on the designated remote worker. All device operations and lease
construction are replaced with fixtures; host evidence uses temporary files.
"""

from __future__ import annotations

from io import StringIO
import json
import os
from pathlib import Path
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch

import physical_device
from physical_device import Harness


def setUpModule():
    if sys.platform != "linux" or os.environ.get("BILICOMICS_REMOTE_VERIFICATION") != "1":
        raise RuntimeError("Run confirmation recovery fixtures only through ssh test-env")


class ConfirmationRecoveryTests(unittest.TestCase):
    def fixture(self, phase="running"):
        directory = tempfile.TemporaryDirectory(prefix="bilicomics-confirmation-recovery-")
        self.addCleanup(directory.cleanup)
        harness = Harness.__new__(Harness)
        harness.root = "/mnt/us/bilicomics-physical-fixture"
        harness.logs = harness.root + "/data/logs"
        harness.output = Path(directory.name)
        harness.state_path = harness.output / "run.json"
        harness.state = {"phase": phase, "binary_name": "bilicomics-native", "binary_sha256": "a" * 64,
                         "checkpoints": {"retained": {"reader_state": {"page_index": 1}}},
                         "observations": [], "physical_panel_review": "pending", "assisted_physical_input": True}
        harness.args = SimpleNamespace(command="confirm", observer="fixture operator", note="fixture observation",
                                       home=False, touch=False, panel=True, checkpoint="retained", input_source=None)
        harness.run_token = Mock(return_value="c" * 64)
        harness.remote = Mock(side_effect=AssertionError("Unexpected device transport in a host confirmation."))
        harness.write = Mock(side_effect=AssertionError("Unexpected device write in a host confirmation."))
        harness.done = Mock(side_effect=AssertionError("Unexpected device polling in a host confirmation."))
        harness.stop = Mock(wraps=harness.stop)
        return harness

    def invoke(self, harness, *, fails=True):
        output, error = StringIO(), StringIO()
        with patch.object(physical_device, "arguments", return_value=harness.args), \
                patch.object(physical_device, "Harness", return_value=harness), \
                patch.object(physical_device.sys, "stdout", output), \
                patch.object(physical_device.sys, "stderr", error):
            if fails:
                with self.assertRaises(SystemExit) as failure:
                    physical_device.main()
                self.assertEqual(failure.exception.code, 1)
            else:
                physical_device.main()
        if fails:
            self.assertEqual(output.getvalue(), "")
            report = json.loads(error.getvalue())
            self.assertEqual(report["status"], "failed")
            self.assertEqual(report["operation"], harness.args.command)
            return report
        self.assertEqual(error.getvalue(), "")
        return json.loads(output.getvalue())

    def assert_host_failure(self, harness, phase):
        harness.remote.assert_not_called()
        harness.write.assert_not_called()
        harness.done.assert_not_called()
        harness.stop.assert_not_called()
        self.assertEqual(harness.state["phase"], phase)
        saved = json.loads(harness.state_path.read_text())
        self.assertEqual(saved["observations"], [])
        self.assertEqual(saved["physical_panel_review"], "pending")
        self.assertEqual(saved["last_failure"]["operation"], "confirm")
        self.assertNotIn("failure_recovery", saved)

    def test_missing_framebuffer_confirmation_never_stops_an_active_run(self):
        for phase in ("starting", "running"):
            with self.subTest(phase=phase):
                harness = self.fixture(phase)
                report = self.invoke(harness)
                self.assertIn("existing demo framebuffer checkpoint", report["message"])
                self.assert_host_failure(harness, phase)

    def test_invalid_confirmation_parameters_do_not_fabricate_observations(self):
        for changes in ({"observer": " "}, {"note": " "}, {"panel": False},
                        {"checkpoint": "missing"}, {"panel": False, "home": True},
                        {"panel": False, "touch": True},
                        {"panel": False, "input_source": "physical-sleep-and-wake", "checkpoint": "missing"}):
            with self.subTest(changes=changes):
                harness = self.fixture()
                harness.args.__dict__.update(changes)
                self.invoke(harness)
                self.assert_host_failure(harness, "running")

    def test_combined_confirmation_is_not_saved_if_a_later_precondition_fails(self):
        harness = self.fixture()
        harness.state["checkpoints"]["retained"]["framebuffer"] = {"synthetic": True}
        harness.state["assisted_physical_input"] = False
        harness.args.input_source = "physical-sleep-and-wake"
        self.invoke(harness)
        self.assert_host_failure(harness, "running")

    def test_valid_confirmation_records_only_the_explicit_host_observation(self):
        harness = self.fixture()
        harness.state["checkpoints"]["retained"]["framebuffer"] = {"synthetic": True}
        report = self.invoke(harness, fails=False)
        self.assertTrue(report["physical_panel_observed"])
        self.assertEqual(report["checkpoint"], "retained")
        self.assertEqual(harness.state["observations"], [report])
        self.assertEqual(harness.state["physical_panel_review"], "operator observation recorded")
        self.assertEqual(harness.state["phase"], "running")
        harness.remote.assert_not_called()
        harness.write.assert_not_called()
        harness.done.assert_not_called()
        harness.stop.assert_not_called()

    def test_host_confirmation_save_error_does_not_initiate_device_recovery(self):
        harness = self.fixture()
        harness.state["checkpoints"]["retained"]["framebuffer"] = {"synthetic": True}
        harness.save = Mock(side_effect=OSError("Fixture host storage failure."))
        self.invoke(harness)
        harness.remote.assert_not_called()
        harness.write.assert_not_called()
        harness.done.assert_not_called()
        harness.stop.assert_not_called()
        self.assertEqual(harness.state["phase"], "running")

    def test_start_and_checkpoint_transport_failures_keep_the_stop_request_fail_safe(self):
        for operation in ("start", "checkpoint"):
            with self.subTest(operation=operation):
                harness = self.fixture("deployed" if operation == "start" else "running")
                harness.args.command = operation
                harness.args.__dict__.update({"screen": "reader", "exit_after_ms": 1000, "assisted": False,
                                              "init_timeout": 10, "session_timeout": 30,
                                              "name": "candidate", "require": "ready", "wait_seconds": 0,
                                              "capture": False, "compare_to": None, "expect": "retained"})
                completed = {"reason": "operator-stop", "recovery_complete": True, "launcher_exit_status": "143"}
                harness.done = Mock(side_effect=([None] if operation == "checkpoint" else []) + [None, completed])
                harness.write = Mock()
                harness.collect = Mock()
                harness.state["recovery_checks"] = {"passed": True}
                harness.trace = Mock(return_value=[{"event": "ready", "process_id": 44},
                    {"event": "ui-state", "process_id": 44, "demo": True, "view_model_fixture_only": False,
                     "state": {"route": "shelf"}}])
                harness.read = Mock(return_value=(f"44|12345|{harness.root}/app/bilicomics-native|43|"
                                                 + harness.run_token() + "\n").encode())
                harness.remote = Mock(side_effect=RuntimeError("Fixture device transport failure."))
                self.invoke(harness)
                harness.remote.assert_called_once()
                harness.stop.assert_called_once_with()
                harness.write.assert_any_call(harness.logs + "/stop.request", (harness.run_token() + "\n").encode())
                harness.collect.assert_called_once_with()
                self.assertEqual(harness.state["phase"], "stopped")
                self.assertEqual(harness.state["last_failure"]["operation"], operation)
                self.assertEqual(harness.state["observations"], [])
                self.assertNotIn("candidate", harness.state["checkpoints"])


if __name__ == "__main__":
    unittest.main(verbosity=2)
