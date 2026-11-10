"""Exercise checkpoint polling with synthetic traces and mocked transport only.

Run on the designated remote worker. These tests never contact a device, read a
framebuffer, acquire a shared lease, or run the application.
"""

from __future__ import annotations

from copy import deepcopy
import json
import os
from pathlib import Path
from subprocess import CompletedProcess
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch

import physical_device
from physical_device import Harness, checkpoint_ready_hint


def setUpModule():
    if sys.platform != "linux" or os.environ.get("BILICOMICS_REMOTE_VERIFICATION") != "1":
        raise RuntimeError("Run checkpoint polling fixtures only through ssh test-env")


def reader_state(page=0):
    return {"comic_id": "demo", "episode_id": "demo-episode", "revision": "fixture-v1",
            "reader_generation": 1, "route": "reader", "launch_preference": "shelf",
            "mode": "page", "core_mode": "page", "fit": "contain", "direction": "ltr",
            "rotation": 0, "landscape_layout": "single", "cover_alone": True,
            "dither": True, "footer": False, "demo_strip": False,
            "page_id": f"demo-page-{page}", "page_index": page, "crop_index": 0,
            "crop_count": 1, "source_x": 0, "source_y": 0, "zoom_ratio": 1}


def reader_trace(page=0, pid=44):
    return [{"process_id": pid, "event": "ready"},
            {"process_id": pid, "event": "ui-state", "demo": True, "view_model_fixture_only": False,
             "state": {"route": "reader", "reader": {"loading": False, "page": page}}},
            {"process_id": pid, "event": "reader-ready", "reader_loading": False,
             "frame_size": [1980, 2640], "reader_state": reader_state(page)}]


def power_trace(event, page=0):
    return reader_trace(page) + [{"process_id": 44, "event": event,
                                "suspended": event == "suspend", "reader_state": reader_state(page)}]


class CheckpointHintTests(unittest.TestCase):
    def test_initial_readiness_requires_a_consistent_native_demo_candidate(self):
        values = reader_trace()
        self.assertFalse(checkpoint_ready_hint([], "ready"))
        self.assertFalse(checkpoint_ready_hint(values[:1], "ready"))
        self.assertTrue(checkpoint_ready_hint(values, "ready"))
        self.assertTrue(checkpoint_ready_hint(values, "reader"))
        for field, value in (("demo", False), ("view_model_fixture_only", True), ("process_id", 99)):
            with self.subTest(field=field):
                changed = deepcopy(values)
                changed[1][field] = value
                self.assertFalse(checkpoint_ready_hint(changed, "reader"))

    def test_latest_ui_and_reader_candidates_override_old_ready_records(self):
        values = reader_trace()
        for ui in ({"route": "reader", "reader": {"loading": True}}, {"route": "shelf"}):
            with self.subTest(ui=ui):
                changed = deepcopy(values[1])
                changed["state"] = ui
                self.assertFalse(checkpoint_ready_hint(values + [changed], "reader"))
        for update in ({"reader_loading": True}, {"frame_size": [0, 2640]},
                       {"frame_size": ["1980", 2640]}, {"frame_size": None}, {"reader_state": None}):
            with self.subTest(update=update):
                changed = {**values[-1], **update}
                self.assertFalse(checkpoint_ready_hint(values + [changed], "reader"))
        changed = {"process_id": 44, "event": "resume", "suspended": False, "reader_state": reader_state(1)}
        self.assertFalse(checkpoint_ready_hint(values + [changed], "reader"))

    def test_sleep_and_resume_use_latest_power_without_requiring_another_ready_event(self):
        self.assertFalse(checkpoint_ready_hint(reader_trace(), "suspended"))
        self.assertFalse(checkpoint_ready_hint(reader_trace(), "resumed"))
        suspended = power_trace("suspend")
        self.assertTrue(checkpoint_ready_hint(suspended, "suspended"))
        for kind in ("ready", "reader", "resumed"):
            self.assertFalse(checkpoint_ready_hint(suspended, kind))
        resumed = suspended + power_trace("resume")[-1:]
        self.assertTrue(checkpoint_ready_hint(resumed, "resumed"))
        resumed[-1]["reader_state"].update({"render_version": 9, "rendering": False})
        self.assertTrue(checkpoint_ready_hint(resumed, "resumed"))
        self.assertFalse(checkpoint_ready_hint(resumed, "suspended"))
        resumed[-1]["suspended"] = True
        self.assertFalse(checkpoint_ready_hint(resumed, "resumed"))

    def test_comparison_hints_wait_for_the_target_page_and_retained_anchor(self):
        before = {"native_process": {"pid": 44}, "reader_state": reader_state()}
        for page in (0, 2):
            self.assertFalse(checkpoint_ready_hint(reader_trace(page), "reader", before, "next-page"))
        self.assertTrue(checkpoint_ready_hint(reader_trace(1), "reader", before, "next-page"))
        self.assertFalse(checkpoint_ready_hint(reader_trace(1, pid=99), "reader", before, "next-page"))
        self.assertTrue(checkpoint_ready_hint(power_trace("suspend"), "suspended", before))
        self.assertTrue(checkpoint_ready_hint(power_trace("resume"), "resumed", before))
        self.assertFalse(checkpoint_ready_hint(power_trace("resume", page=1), "resumed", before))
        changed = reader_trace(1)
        changed[-1]["reader_state"]["page_id"] = before["reader_state"]["page_id"]
        self.assertFalse(checkpoint_ready_hint(changed, "reader", before, "next-page"))
        changed = reader_trace(1)
        changed[-1]["reader_state"]["reader_generation"] = 2
        self.assertFalse(checkpoint_ready_hint(changed, "reader", before, "next-page"))

    def test_malformed_hints_are_not_readiness(self):
        for values in (None, [None], [{"event": "ready", "process_id": "44"}],
                       reader_trace()[:1] + [{"event": "ui-state", "process_id": 44,
                                             "demo": True, "view_model_fixture_only": False, "state": []}]):
            with self.subTest(values=values):
                self.assertFalse(checkpoint_ready_hint(values, "reader"))


class PollClock:
    def __init__(self):
        self.now = 0.0
        self.sleeps = []

    def sleep(self, seconds):
        self.sleeps.append(seconds)
        self.now += seconds


class CheckpointPollingTests(unittest.TestCase):
    def fixture(self, traces, *, kind="reader", wait=0, capture=False, compare=False, expect="retained", fault=None):
        directory = tempfile.TemporaryDirectory(prefix="bilicomics-checkpoint-polling-")
        self.addCleanup(directory.cleanup)
        harness = Harness.__new__(Harness)
        harness.root = "/mnt/us/bilicomics-physical-fixture"
        harness.logs = harness.root + "/data/logs"
        harness.output = Path(directory.name)
        harness.state = {"phase": "running", "binary_name": "bilicomics-native", "binary_sha256": "a" * 64,
                         "profile": "scribe2025", "checkpoints": {}}
        harness.lease_identity = {"lease_owner": "fixture-owner", "lease_run_id": "fixture-run",
                                  "lease_token_sha256": "b" * 64}
        harness.args = SimpleNamespace(name="candidate", require=kind, wait_seconds=wait, capture=capture,
                                       compare_to="before" if compare else None, expect=expect)
        harness.run_token = Mock(return_value="c" * 64)
        harness.done = Mock(return_value=None)
        harness.events = []
        harness.save = Mock(side_effect=lambda: harness.events.append("save"))
        harness.capture = Mock(side_effect=lambda *_: harness.events.append("capture") or {"synthetic": True})
        executable = harness.root + "/app/bilicomics-native"
        identity = {"pid": 44, "start_ticks": 12345, "exe_path": executable, "exe_sha256": "a" * 64,
                    "demo_offline_native_arguments_verified": True}
        if compare:
            harness.state["checkpoints"]["before"] = {"native_process": identity, "reader_state": reader_state()}
        snapshots = iter(traces)
        current = []

        def read(path, *, required=True):
            nonlocal current
            if path == harness.logs + "/lifecycle.jsonl":
                harness.events.append("trace")
                current = next(snapshots, current)
                return b"".join(json.dumps(value).encode() + b"\n" for value in current)
            self.assertEqual(path, harness.logs + "/native.identity")
            harness.events.append("identity")
            token = "d" * 64 if fault == "token" else harness.run_token()
            return f"44|12345|{executable}|43|{token}\n".encode()

        def remote(script, **kwargs):
            if "sha256sum /proc/44/exe" in script:
                harness.events.append("full-proof")
                if fault == "arguments":
                    raise RuntimeError("The fixture native arguments were not verified.")
                ticks = "54321" if fault == "birth" else "12345"
                path = "/outside/bilicomics" if fault == "path" else executable
                digest = "e" * 64 if fault == "digest" else "a" * 64
                stat = "44 (bilicomics) " + " ".join(["S"] + ["0"] * 18 + [ticks])
                return CompletedProcess([], 0, f"{stat}\n{path}\n{digest}  /proc/44/exe\n".encode(), b"")
            if script == harness.root + "/app/physical-framebuffer --capture":
                harness.events.append("framebuffer")
                return CompletedProcess([], 0, b'{"visible_bytes": 1}\n\x00', b"")
            self.assertIn("lifecycle-request", script)
            self.assertIn("native-baseline.tsv", script)
            self.assertIn("com.lab126.powerd state", script)
            harness.events.append("display-gate")
            return CompletedProcess([], 0, b"", b"")

        harness.read = Mock(side_effect=read)
        harness.remote = Mock(side_effect=remote)
        harness.native_identity = Mock(wraps=harness.native_identity)
        return harness

    def run_checkpoint(self, harness):
        clock = PollClock()
        self.clock = clock
        with patch.object(physical_device.time, "monotonic", side_effect=lambda: clock.now), \
                patch.object(physical_device.time, "sleep", side_effect=clock.sleep):
            return harness.checkpoint()

    def assert_not_accepted(self, harness):
        self.assertNotIn("candidate", harness.state["checkpoints"])
        self.assertFalse((harness.output / "candidate.checkpoint.json").exists())
        harness.capture.assert_not_called()
        harness.save.assert_not_called()

    def test_not_ready_polls_skip_full_proof_and_keep_the_original_deadline(self):
        harness = self.fixture([[]], wait=1)
        with self.assertRaisesRegex(RuntimeError, "untrusted lifecycle hint"):
            self.run_checkpoint(harness)
        self.assertEqual(self.clock.now, 1)
        self.assertEqual(self.clock.sleeps, [0.5, 0.5])
        self.assertEqual(harness.events, ["trace"] * 3)
        harness.native_identity.assert_not_called()
        harness.remote.assert_not_called()
        self.assert_not_accepted(harness)

    def test_next_page_waits_for_the_target_hint_then_performs_full_proof(self):
        harness = self.fixture([reader_trace(), reader_trace(), reader_trace(1)], wait=2,
                               capture=True, compare=True, expect="next-page")
        result = self.run_checkpoint(harness)
        self.assertEqual(self.clock.sleeps, [0.5, 0.5])
        self.assertEqual(result["reader_state"], reader_state(1))
        self.assertEqual(harness.events, ["trace", "trace", "trace", "identity", "full-proof", "trace", "capture", "save"])
        self.assertTrue(harness.state["checkpoints"]["candidate"]["comparison"]["passed"])

    def test_sleep_and_resume_wait_for_the_latest_transition_then_validate(self):
        for kind, traces in (("suspended", [reader_trace(), power_trace("suspend")]),
                             ("resumed", [reader_trace(), power_trace("suspend"), power_trace("resume")])):
            with self.subTest(kind=kind):
                harness = self.fixture(traces, kind=kind, wait=2, compare=True)
                self.run_checkpoint(harness)
                self.assertEqual(harness.native_identity.call_count, 1)
                self.assertEqual(harness.events.count("full-proof"), 1)
                self.assertEqual(harness.state["checkpoints"]["candidate"]["latest_power"]["event"],
                                 "suspend" if kind == "suspended" else "resume")

    def test_forged_foreign_and_stale_hints_cannot_bypass_full_validation(self):
        stale = reader_trace()
        stale[1]["state"]["reader"]["loading"] = True
        cases = [("foreign", [reader_trace(pid=99)], None), ("stale", [reader_trace(), stale], None)]
        cases.extend((fault, [reader_trace()], fault) for fault in ("token", "birth", "path", "digest", "arguments"))
        for name, traces, fault in cases:
            with self.subTest(name=name):
                harness = self.fixture(traces, capture=True, fault=fault)
                with self.assertRaises(RuntimeError):
                    self.run_checkpoint(harness)
                self.assertEqual(harness.native_identity.call_count, 1)
                self.assert_not_accepted(harness)

    def test_fresh_full_state_must_still_pass_the_original_semantic_comparison(self):
        harness = self.fixture([reader_trace(1), reader_trace(2)], capture=True, compare=True, expect="next-page")
        with self.assertRaisesRegex(RuntimeError, "did not advance to the next source page"):
            self.run_checkpoint(harness)
        self.assertEqual(harness.native_identity.call_count, 1)
        self.assert_not_accepted(harness)

    def test_every_accepted_checkpoint_runs_full_proof_before_capture_and_save(self):
        harness = self.fixture([reader_trace()], capture=True)
        self.run_checkpoint(harness)
        harness.args.name = "second"
        self.run_checkpoint(harness)
        self.assertEqual(harness.events, ["trace", "identity", "full-proof", "trace", "capture", "save"] * 2)
        self.assertEqual(harness.native_identity.call_count, 2)
        self.assertEqual(harness.remote.call_count, 2)
        for call in harness.remote.call_args_list:
            script = call.args[0]
            self.assertIn("cat /proc/44/stat", script)
            self.assertIn("readlink /proc/44/exe", script)
            self.assertIn("sha256sum /proc/44/exe", script)
            self.assertIn("BILICOMICS_PHYSICAL_TOKEN=", script)
            self.assertIn("for flag in --demo --offline --kindle", script)

    def test_capture_keeps_full_proof_on_both_sides_of_the_framebuffer_read(self):
        harness = self.fixture([reader_trace()])
        checkpoint = harness.checkpoint_value("reader")
        harness.events.clear()
        harness.check_geometry = Mock(side_effect=RuntimeError("fixture capture boundary"))
        with self.assertRaisesRegex(RuntimeError, "fixture capture boundary"):
            Harness.capture(harness, "candidate", checkpoint)
        self.assertEqual(harness.events, ["display-gate", "identity", "full-proof", "trace", "framebuffer",
                                         "display-gate", "identity", "full-proof", "trace"])
        self.assertEqual(harness.native_identity.call_count, 3)
        self.assertFalse(list(harness.output.iterdir()))


if __name__ == "__main__":
    unittest.main(verbosity=2)
