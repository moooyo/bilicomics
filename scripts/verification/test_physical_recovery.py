"""Check physical-run recovery evidence with owned Linux host processes only.

Run only on test-env. The harness transport is replaced by local read-only
process snapshots; no Kindle connection or shared device lease is accessed.
"""

import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
from types import SimpleNamespace
import unittest

import physical_device as physical


def setUpModule():
    if sys.platform != "linux" or os.environ.get("BILICOMICS_REMOTE_VERIFICATION") != "1":
        raise RuntimeError("Run physical recovery fixtures only through ssh test-env")


class PhysicalRecoveryTests(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory(prefix="bilicomics-physical-recovery-")
        self.addCleanup(directory.cleanup)
        self.output = Path(directory.name)
        self.process = subprocess.Popen(
            [sys.executable, "-c", "import sys; sys.stdin.readline()"],
            stdin=subprocess.PIPE, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        )
        self.addCleanup(self.close_process)
        stat = Path(f"/proc/{self.process.pid}/stat").read_text().rsplit(") ", 1)[1].split()
        self.birth = stat[19]
        self.executable = os.readlink(f"/proc/{self.process.pid}/exe")
        self.owned = f"{self.process.pid}|{self.birth}|{self.executable}|0|application\n".encode()
        self.done = {"recovery_complete": True}
        self.logs = {
            "statusbar-before.txt": b"statusbar start/running, process 123\n",
            "statusbar-after.txt": b"statusbar start/running, process 456\n",
            "native-baseline.tsv": b"awesome|789|41|/fixture/awesome|S\n",
            "awesome-after.tsv": b"789|41|/fixture/awesome|S\n",
            "owned-processes.tsv": self.owned,
        }
        self.snapshots = []

    def close_process(self):
        if self.process.poll() is None:
            self.process.communicate(b"finished\n", timeout=10)
        elif self.process.stdin is not None:
            self.process.stdin.close()

    def stopped_process(self):
        self.process.communicate(b"finished\n", timeout=10)

    def harness(self, content_mode="demo"):
        harness = physical.Harness.__new__(physical.Harness)
        harness.args = SimpleNamespace()
        harness.output = self.output
        harness.state_path = self.output / "run.json"
        harness.logs = "/fixture/device/logs"
        harness.state = {"phase": "running", "content_mode": content_mode,
                         "run_id": "recovery-fixture", "observations": []}
        harness.read = lambda path, required=True: self.logs.get(path.rsplit("/", 1)[-1], b"")
        harness.done = lambda required=False: dict(self.done)

        def remote(script, *, data=None, check=True, timeout=20):
            self.assertIsNone(data)
            if script.startswith("[ ! -e "):
                # The production lock pathname is never accessed on this host.
                return subprocess.CompletedProcess([], 0, stdout=b"", stderr=b"")
            self.assertIn("snapshot_owned()", script)
            self.assertIn(str(self.process.pid), script)
            self.assertIn(self.birth, script)
            self.assertNotIn("kill ", script)
            self.snapshots.append(script)
            return subprocess.run(["sh", "-c", script], capture_output=True,
                                  check=check, timeout=timeout)

        harness.remote = remote
        return harness

    def test_demo_restoration_includes_the_observed_owned_process_snapshot(self):
        self.stopped_process()
        harness = self.harness()
        result = harness.collect()
        self.assertEqual(len(self.snapshots), 1)
        self.assertTrue(result["recovery_checks"]["passed"])
        self.assertTrue(result["recovery_checks"]["owned_process_snapshot_passed"])
        saved = json.loads((self.output / "post-restore-processes.json").read_text())
        self.assertTrue(saved["passed"])
        self.assertEqual(saved["processes"][0]["pid"], self.process.pid)
        self.assertEqual(saved["processes"][0]["expected_start_ticks"], int(self.birth))
        self.assertIn(saved["processes"][0]["state"], ("gone", "pid-reused", "zombie", "dead"))
        self.assertFalse(saved["signals_sent"])
        self.assertFalse(saved["waited_for_zombies"])
        self.assertTrue(json.loads(harness.state_path.read_text())["recovery_checks"]["passed"])

    def test_demo_with_an_active_owned_process_is_not_ready_for_lease_release(self):
        harness = self.harness()
        result = harness.collect()
        self.assertEqual(len(self.snapshots), 1)
        self.assertFalse(result["recovery_checks"]["passed"])
        self.assertFalse(result["recovery_checks"]["owned_process_snapshot_passed"])
        observed = harness.state["post_restore_process_snapshot"]
        self.assertEqual(observed["processes"][0]["state"], "active-match")
        self.assertFalse(observed["signals_sent"])

    def test_distinct_fixture_keeps_the_same_real_snapshot_boundary(self):
        self.stopped_process()
        harness = self.harness("distinct_fixture")
        result = harness.collect()
        self.assertEqual(len(self.snapshots), 1)
        self.assertTrue(result["recovery_checks"]["owned_process_snapshot_passed"])
        self.assertTrue(result["recovery_checks"]["passed"])

    def test_incomplete_native_recovery_does_not_manufacture_process_closure(self):
        for failure in ("watchdog", "statusbar", "window-manager"):
            with self.subTest(failure=failure):
                self.snapshots.clear()
                self.done["recovery_complete"] = failure != "watchdog"
                self.logs["statusbar-after.txt"] = (
                    b"statusbar stop/waiting\n" if failure == "statusbar"
                    else b"statusbar start/running, process 456\n"
                )
                self.logs["awesome-after.tsv"] = (
                    b"789|41|/fixture/awesome|T\n" if failure == "window-manager"
                    else b"789|41|/fixture/awesome|S\n"
                )
                harness = self.harness()
                result = harness.collect()
                self.assertFalse(result["recovery_checks"]["passed"])
                self.assertNotIn("owned_process_snapshot_passed", result["recovery_checks"])
                self.assertEqual(self.snapshots, [])
                self.assertNotIn("post_restore_process_snapshot", harness.state)


if __name__ == "__main__":
    unittest.main()
