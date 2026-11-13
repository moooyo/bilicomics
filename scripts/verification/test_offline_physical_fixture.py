"""Check offline physical-fixture evidence using owned files and mocked transport.

Run only on test-env with BILICOMICS_REMOTE_VERIFICATION=1. These tests never
construct a device connection, acquire a lease, execute an ARM input, or access
a real framebuffer. Captured bytes come from the generated synthetic PNGs.
"""

from copy import deepcopy
import io
import json
import os
from pathlib import Path
import shutil
import struct
from subprocess import CompletedProcess
import sys
import tarfile
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch
import zipfile

from PIL import Image

import physical_device as physical
from physical import distinct_reader_fixture as fixture


PID = 1001
OPERATION = 31
GEOMETRY = {"width": 1980, "height": 2640, "bits_per_pixel": 8, "visual": 1, "grayscale": 1,
            "line_length": 1980, "visible_bytes": 1980 * 2640, "rotate": 0,
            "xoffset": 0, "yoffset": 0, "read_only": True}
CANVAS = [GEOMETRY["width"], GEOMETRY["height"]]
# The fitted image occupies only part of the full reader canvas.
FRAME = [1902, 2640]


def setUpModule():
    if sys.platform != "linux" or os.environ.get("BILICOMICS_REMOTE_VERIFICATION") != "1":
        raise RuntimeError("Run offline physical-fixture tests only through ssh test-env")


class OfflinePhysicalFixtureTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.workspace = tempfile.TemporaryDirectory(prefix="bilicomics-offline-physical-tests-")
        cls.root = Path(cls.workspace.name)
        cls.source = cls.root / "guest"
        cls.manifest = fixture.generate_fixture(cls.source)
        cls.manifest_sha = physical.file_hash(cls.source / "manifest.json")
        # This is an ELF header-format fixture, not executable guard code.
        header = bytearray(64)
        header[:7] = b"\x7fELF\x01\x01\x01"
        struct.pack_into("<H", header, 18, 40)
        cls.guard_bytes = bytes(header)
        cls.guard = cls.root / "guard-header-only"
        cls.guard.write_bytes(cls.guard_bytes)

    @classmethod
    def tearDownClass(cls):
        cls.workspace.cleanup()

    def setUp(self):
        self.output = self.root / self._testMethodName
        self.output.mkdir()

    def input_arguments(self, **changes):
        values = {"content_mode": "distinct_fixture", "fixture_directory": self.source,
                  "expected_fixture_sha256": self.manifest_sha, "socket_guard": self.guard,
                  "expected_guard_sha256": physical.sha256(self.guard_bytes)}
        values.update(changes)
        return SimpleNamespace(**values)

    def reader(self, page=4):
        return {"comic_id": self.manifest["comic_id"], "episode_id": self.manifest["episode_id"],
                "revision": self.manifest["revision"], "reader_generation": 7, "render_version": 11,
                "route": "reader", "launch_preference": "resume", "mode": "page", "core_mode": "page",
                "fit": "page", "direction": "ltr", "rotation": 0, "landscape_layout": "single",
                "cover_alone": True, "dither": False, "footer": True, "demo_strip": False,
                "page_id": self.manifest["pages"][page]["page_id"], "page_index": page, "page_number": page + 1,
                "crop_index": 0, "crop_count": 1, "source_x": 0.0, "source_y": 0.0, "zoom_ratio": 1.0,
                "effective_spread": False, "transition": False, "menu": False, "sheet": "", "rendering": False,
                "frame_width": CANVAS[0], "frame_height": CANVAS[1], "second_frame_width": 0, "second_frame_height": 0,
                "viewport_width": GEOMETRY["width"], "viewport_height": GEOMETRY["height"]}

    def trace(self, page=4, pid=PID, mode="Y8"):
        return [{"process_id": pid, "event": "ready", "monotonic_ns": 100, "native_epdc_mode": mode},
                {"process_id": pid, "event": "reader-ready", "monotonic_ns": 200,
                 "route": "reader", "reader_loading": False, "frame_size": CANVAS.copy(),
                 "native_epdc_mode": mode, "reader_state": self.reader(page),
                 "reader_resources": {"reader_active": True, "reader_generation": 7,
                                      "page_size": {"width": CANVAS[0], "height": CANVAS[1]}}}]

    def performance(self, pid=PID, mode="Y8", deferred=False):
        def record(stage, clock, data=None, kind="reader"):
            return {"process_id": pid, "kind": kind, "stage": stage, "operation_id": OPERATION,
                    "monotonic_ns": clock, "data": data or {}}
        requests = ([record("navigation_request", 120), record("render_deferred", 130)] if deferred else
                    [record("render_request", 120, {"source_kind": "receipt", "generation": 7, "version": 11})])
        native = {"reader_operation_id": OPERATION, "render_sequence": 1, "rendered": True, "refresh_accepted": True,
                  "rotation_degrees": 0, "refresh_ioctl_returned": True, "refresh_ioctl_attempts": 1,
                  "refresh_errno": None, "framebuffer_bits_per_pixel": 8,
                  "framebuffer_size": [GEOMETRY["width"], GEOMETRY["height"]],
                  "framebuffer_stride": GEOMETRY["line_length"], "output_inverted": mode == "Y8INV",
                  "stages": {"framebuffer_write": {"started_monotonic_ns": 205, "finished_monotonic_ns": 210},
                             "refresh_submit": {"started_monotonic_ns": 211, "finished_monotonic_ns": 220}}}
        return requests + [record("reader_model_published", 180), record("frame", 230, native, "native")]

    def harness(self, *, page=4, mode="Y8", traces=None, performance=None):
        harness = physical.Harness.__new__(physical.Harness)
        harness.root = "/synthetic/offline-physical"
        harness.logs = harness.root + "/data/logs"
        harness.output = self.output
        harness.state = {"content_mode": "distinct_fixture", "fixture_manifest": self.manifest,
                         "fixture_manifest_sha256": self.manifest_sha, "profile": "scribe2025"}
        harness.lease_identity = {"lease_owner": "mock-owner", "lease_run_id": "mock-run",
                                  "lease_token_sha256": "a" * 64}
        harness.run_token = Mock(return_value="b" * 64)
        harness.native_identity = Mock(return_value={
            "pid": PID, "start_ticks": 1234, "exe_sha256": "c" * 64,
            "offline_guest_native_arguments_verified": True,
            "portrait_calibration": {"direction": "U", "raw_rotation": 0}})
        harness.trace = Mock(return_value=self.trace(page, mode=mode) if traces is None else traces)
        records = self.performance(mode=mode) if performance is None else performance

        def read(path, **kwargs):
            self.assertEqual(path, harness.logs + "/performance.jsonl")
            return b"".join(json.dumps(value).encode("ascii") + b"\n" for value in records)

        harness.read = Mock(side_effect=read)
        harness.remote = Mock(side_effect=AssertionError("Unexpected transport request in a unit fixture"))
        return harness

    def source_projection(self, page=4):
        # Decode the actual PNG with Pillow, independently of the seed decoder.
        with Image.open(self.source / self.manifest["pages"][page]["file"]) as image:
            image.load()
            self.assertEqual((image.mode, image.size), ("L", (1700, 2360)))
            fitted = image.resize(tuple(FRAME), Image.Resampling.NEAREST)
        canvas = Image.new("L", (GEOMETRY["width"], GEOMETRY["height"]), 255)
        canvas.paste(fitted, ((GEOMETRY["width"] - FRAME[0]) // 2, 0))
        return canvas.tobytes()

    def attach_framebuffer(self, harness, pixels, *, changed_geometry=False, after_capture=None):
        info_calls = [0]

        def remote(script, **kwargs):
            if script.endswith("/physical-framebuffer --info"):
                geometry = dict(GEOMETRY)
                info_calls[0] += 1
                if changed_geometry and info_calls[0] == 2:
                    geometry["rotate"] = 1
                return CompletedProcess([], 0, json.dumps(geometry).encode("ascii"), b"")
            if script.endswith("/physical-framebuffer --capture"):
                if after_capture is not None:
                    after_capture()
                content = json.dumps(GEOMETRY).encode("ascii") + b"\n" + pixels
                return CompletedProcess([], 0, content, b"")
            self.assertIn("com.lab126.powerd state", script)
            self.assertIn("lifecycle-request", script)
            return CompletedProcess([], 0, b"", b"")

        harness.remote = Mock(side_effect=remote)

    def pending_request(self):
        return {"process_id": PID, "kind": "reader", "stage": "render_request",
                "operation_id": OPERATION + 1, "monotonic_ns": 250,
                "data": {"source_kind": "receipt", "generation": 7, "version": 12}}

    def publish_next_fixture(self, harness, records):
        traces = self.trace(page=5)
        for value in traces:
            value["monotonic_ns"] += 1000
        traces[-1]["reader_state"]["render_version"] = 12
        harness.trace.return_value = traces
        fresh = self.performance()
        for value in fresh:
            value["operation_id"] += 1
            value["monotonic_ns"] += 1000
        fresh[0]["data"]["version"] = 12
        fresh[-1]["data"]["reader_operation_id"] += 1
        fresh[-1]["data"]["render_sequence"] += 1
        for timing in fresh[-1]["data"]["stages"].values():
            timing["started_monotonic_ns"] += 1000
            timing["finished_monotonic_ns"] += 1000
        records[:] = fresh

    def publish_same_page_view(self, harness, records, *, menu=False, new_version=True):
        if new_version:
            self.publish_next_fixture(harness, records)
            reader = harness.trace.return_value[-1]["reader_state"]
            reader.update(page_id=self.manifest["pages"][4]["page_id"], page_index=4, page_number=5)
            return
        traces = self.trace()
        traces[-1]["monotonic_ns"] += 1000
        traces[-1]["reader_state"]["menu"] = menu
        harness.trace.return_value = traces
        fresh = self.performance()
        fresh[-1]["monotonic_ns"] += 1000
        fresh[-1]["data"]["render_sequence"] += 1
        for timing in fresh[-1]["data"]["stages"].values():
            timing["started_monotonic_ns"] += 1000
            timing["finished_monotonic_ns"] += 1000
        records[:] = fresh

    def publish_cache_snapshot(self, harness, cache):
        later = deepcopy(harness.trace.return_value[-1])
        later["monotonic_ns"] += 1000
        for key in ("source_cache_budget", "source_cache_bytes", "source_cache_entries"):
            later["reader_state"].pop(key, None)
        later["reader_state"].update(cache)
        harness.trace.return_value = [*harness.trace.return_value, later]

    def run_retry_checkpoint(self, harness, *, wait=2, on_sleep=None):
        harness.state.update(phase="running", checkpoints={})
        harness.args = SimpleNamespace(name="candidate", require="reader", wait_seconds=wait,
                                       capture=True, compare_to=None, expect="retained")
        harness.done = Mock(return_value=None)
        harness.save = Mock()
        if not isinstance(harness.capture, Mock):
            harness.capture = Mock(wraps=harness.capture)
        clock = SimpleNamespace(now=0.0, sleeps=[])
        self.retry_clock = clock

        def sleep(seconds):
            clock.sleeps.append(seconds)
            clock.now += seconds
            if on_sleep is not None:
                on_sleep()

        with patch.object(physical.time, "monotonic", side_effect=lambda: clock.now), \
                patch.object(physical.time, "sleep", side_effect=sleep):
            return harness.checkpoint()

    def test_capture_pending_gates_retry_and_accept_only_the_new_matching_frame(self):
        for boundary in ("before", "after"):
            with self.subTest(boundary=boundary):
                records = self.performance()
                harness = self.harness(performance=records)
                harness.output = self.output / boundary
                harness.output.mkdir()
                captures = []

                def capture(name, checkpoint):
                    captures.append(name)
                    if boundary == "before" and len(captures) == 1:
                        records.append(self.pending_request())
                    return physical.Harness.capture(harness, name, checkpoint)

                harness.capture = Mock(side_effect=capture)
                self.attach_framebuffer(harness, self.source_projection(), after_capture=(
                    lambda: records.append(self.pending_request())) if boundary == "after" else None)

                def ready():
                    self.assertFalse(list(harness.output.glob("*.framebuffer.*")))
                    self.assertFalse(list(harness.output.glob("*.png")))
                    self.publish_next_fixture(harness, records)
                    self.attach_framebuffer(harness, self.source_projection(page=5))

                result = self.run_retry_checkpoint(harness, on_sleep=ready)
                checkpoint = harness.state["checkpoints"]["candidate"]
                self.assertEqual(self.retry_clock.sleeps, [0.5])
                self.assertEqual(result["reader_state"]["page_index"], 5)
                self.assertEqual(checkpoint["reader_state"]["render_version"], 12)
                self.assertEqual(checkpoint["fixture_publication"]["operation_id"], OPERATION + 1)
                self.assertEqual(checkpoint["framebuffer"]["fixture_marker"]["decoded_page_number"], 6)
                self.assertEqual(len(set(captures)), 2)
                self.assertEqual([attempt["status"] for attempt in checkpoint["capture_attempts"]],
                                 ["reader-publication-pending", "accepted"])
                rejected = json.loads(Path(checkpoint["capture_attempts"][0]["diagnostic"]).read_text())
                self.assertFalse(rejected["checkpoint_accepted"])
                self.assertFalse(rejected["framebuffer_payload_retained"])
                self.assertEqual(rejected["candidate_checkpoint"]["reader_state"]["page_index"], 4)
                self.assertFalse((harness.output / "candidate.framebuffer.raw").exists())
                self.assertEqual(physical.file_hash(Path(checkpoint["framebuffer"]["raw_framebuffer"]["path"])),
                                 physical.sha256(self.source_projection(page=5)))

    def test_capture_pending_forever_uses_the_original_deadline_without_stale_acceptance(self):
        records = self.performance()
        harness = self.harness(performance=records)

        def capture(name, checkpoint):
            records.append(self.pending_request())
            return physical.Harness.capture(harness, name, checkpoint)

        harness.capture = Mock(side_effect=capture)
        self.attach_framebuffer(harness, self.source_projection())
        with self.assertRaisesRegex(RuntimeError, "wait deadline expired"):
            self.run_retry_checkpoint(harness, wait=1)
        self.assertEqual(self.retry_clock.now, 1)
        self.assertEqual(harness.capture.call_count, 1)
        self.assertNotIn("candidate", harness.state["checkpoints"])
        harness.save.assert_not_called()
        self.assertFalse(list(self.output.glob("*.framebuffer.*")))
        report = json.loads(next(self.output.glob("candidate.checkpoint-failure-*.json")).read_text())
        self.assertEqual(report["terminal_stage"], "wait_deadline")
        self.assertFalse(report["checkpoint_accepted"])
        self.assertEqual(report["capture_attempts"][0]["status"], "reader-publication-pending")

    def test_capture_retry_does_not_start_a_new_attempt_at_the_deadline(self):
        records = self.performance()
        harness = self.harness(performance=records)

        def capture(name, checkpoint):
            self.retry_clock.now = 0.75
            records.append(self.pending_request())
            return physical.Harness.capture(harness, name, checkpoint)

        harness.capture = Mock(side_effect=capture)
        self.attach_framebuffer(harness, self.source_projection())
        with self.assertRaisesRegex(RuntimeError, "wait deadline expired"):
            self.run_retry_checkpoint(harness, wait=1,
                                      on_sleep=lambda: self.publish_next_fixture(harness, records))
        self.assertEqual(self.retry_clock.sleeps, [0.25])
        self.assertEqual(harness.capture.call_count, 1)
        self.assertNotIn("candidate", harness.state["checkpoints"])

    def test_capture_pending_never_softens_malformed_or_foreign_proofs(self):
        faults = ("operation", "clock", "generation", "version", "rotation", "submission", "identity", "epdc")
        for fault in faults:
            with self.subTest(fault=fault):
                records = self.performance()
                harness = self.harness(performance=records)
                pending = self.pending_request()

                def capture(name, checkpoint):
                    if fault == "operation":
                        pending["operation_id"] = True
                    elif fault == "clock":
                        pending["monotonic_ns"] = "250"
                    elif fault == "generation":
                        pending["data"]["generation"] += 1
                    elif fault == "version":
                        pending["data"]["version"] = 11
                    elif fault == "rotation":
                        records[-1]["data"]["rotation_degrees"] = 180
                    elif fault == "submission":
                        records[-1]["data"]["refresh_ioctl_attempts"] = "1"
                    elif fault == "identity":
                        harness.native_identity.return_value = {**checkpoint["native_process"], "pid": PID + 1}
                    else:
                        harness.trace.return_value[-1]["native_epdc_mode"] = "unknown"
                    records.append(pending)
                    return physical.Harness.capture(harness, name, checkpoint)

                harness.capture = Mock(side_effect=capture)
                self.attach_framebuffer(harness, self.source_projection())
                with self.assertRaises(RuntimeError) as rejected:
                    self.run_retry_checkpoint(harness)
                self.assertNotIsInstance(rejected.exception, physical.PendingReaderPublication)
                self.assertEqual(self.retry_clock.sleeps, [])
                self.assertEqual(harness.capture.call_count, 1)
                self.assertNotIn("candidate", harness.state["checkpoints"])
                harness.save.assert_not_called()

    def test_post_capture_pending_preserves_response_and_projection_failures(self):
        for fault in ("json", "length", "geometry", "projection", "transport"):
            with self.subTest(fault=fault):
                records = self.performance()
                harness = self.harness(performance=records)
                pixels = self.source_projection()
                self.attach_framebuffer(harness, pixels[:-1] if fault == "length" else pixels,
                                        changed_geometry=fault == "geometry",
                                        after_capture=lambda: records.append(self.pending_request()))
                remote = harness.remote.side_effect

                def response(script, **kwargs):
                    if script.endswith("/physical-framebuffer --capture") and fault == "transport":
                        raise TimeoutError("Synthetic uncertain capture transport.")
                    value = remote(script, **kwargs)
                    if script.endswith("/physical-framebuffer --capture") and fault == "json":
                        return CompletedProcess([], 0, b"invalid-json\n" + pixels, b"")
                    return value

                harness.remote.side_effect = response
                project = physical.framebuffer_panel_projection

                def projection(*args):
                    value = project(*args)
                    if fault == "projection":
                        value["status"] = "unavailable"
                    return value

                with patch.object(physical, "framebuffer_panel_projection", side_effect=projection):
                    with self.assertRaises((RuntimeError, ValueError, TimeoutError)) as rejected:
                        self.run_retry_checkpoint(harness)
                self.assertNotIsInstance(rejected.exception, physical.PendingReaderPublication)
                self.assertEqual(self.retry_clock.sleeps, [])
                self.assertEqual(harness.capture.call_count, 1)
                self.assertNotIn("candidate", harness.state["checkpoints"])
                self.assertFalse(list(self.output.glob("*.framebuffer.*")))

    def test_retry_with_a_new_model_still_rejects_old_page_pixels(self):
        records = self.performance()
        harness = self.harness(performance=records)
        self.attach_framebuffer(harness, self.source_projection(),
                                after_capture=lambda: records.append(self.pending_request()))

        def ready_with_stale_pixels():
            self.publish_next_fixture(harness, records)
            self.attach_framebuffer(harness, self.source_projection())

        with self.assertRaisesRegex(RuntimeError, "marker differs"):
            self.run_retry_checkpoint(harness, on_sleep=ready_with_stale_pixels)
        self.assertEqual(harness.capture.call_count, 2)
        self.assertEqual(len({call.args[0] for call in harness.capture.call_args_list}), 2)
        self.assertNotIn("candidate", harness.state["checkpoints"])
        self.assertFalse(list(self.output.glob("*.framebuffer.json")))

    def test_completed_same_page_publications_retry_before_or_during_capture(self):
        for boundary in ("before", "after"):
            with self.subTest(boundary=boundary):
                records = self.performance()
                harness = self.harness(performance=records)
                harness.output = self.output / boundary
                harness.output.mkdir()
                names = []

                def capture(name, checkpoint):
                    names.append(name)
                    if boundary == "before" and len(names) == 1:
                        self.publish_same_page_view(harness, records)
                    return physical.Harness.capture(harness, name, checkpoint)

                harness.capture = Mock(side_effect=capture)
                self.attach_framebuffer(harness, self.source_projection(), after_capture=(
                    lambda: self.publish_same_page_view(harness, records)) if boundary == "after" else None)

                def ready():
                    self.assertFalse(list(harness.output.glob("*.framebuffer.*")))
                    self.attach_framebuffer(harness, self.source_projection())

                result = self.run_retry_checkpoint(harness, on_sleep=ready)
                self.assertEqual(self.retry_clock.sleeps, [0.5])
                self.assertEqual(result["reader_state"]["page_index"], 4)
                self.assertEqual(result["reader_state"]["render_version"], 12)
                self.assertEqual(len(set(names)), 2)
                attempts = harness.state["checkpoints"]["candidate"]["capture_attempts"]
                self.assertEqual([attempt["status"] for attempt in attempts], ["reader-view-changed", "accepted"])
                rejected = json.loads(Path(attempts[0]["diagnostic"]).read_text())
                self.assertEqual(rejected["observed_checkpoint"]["reader_state"]["render_version"], 12)
                self.assertFalse(rejected["framebuffer_payload_retained"])

    def test_menu_toggle_with_the_same_publication_retries_capture(self):
        records = self.performance()
        harness = self.harness(performance=records)
        snapshot = physical.fixture_reader_snapshot

        def menu_snapshot(values, manifest, kind):
            # The supervised reader adapter permits the menu. Keep the base
            # fixture's other identity and native-input checks for this mock.
            normalized = deepcopy(values)
            menu = normalized[-1]["reader_state"]["menu"]
            self.assertIs(type(menu), bool)
            for value in normalized:
                if isinstance(value.get("reader_state"), dict):
                    value["reader_state"]["menu"] = False
            result = snapshot(normalized, manifest, kind)
            result["reader_state"]["menu"] = menu
            return result

        self.attach_framebuffer(harness, self.source_projection(), after_capture=(
            lambda: self.publish_same_page_view(harness, records, menu=True, new_version=False)))
        with patch.object(physical, "fixture_reader_snapshot", side_effect=menu_snapshot):
            result = self.run_retry_checkpoint(harness, on_sleep=(
                lambda: self.attach_framebuffer(harness, self.source_projection())))
        self.assertTrue(result["reader_state"]["menu"])
        self.assertEqual(result["reader_state"]["render_version"], 11)
        self.assertEqual(harness.capture.call_count, 2)
        self.assertEqual(self.retry_clock.sleeps, [0.5])
        self.assertEqual(harness.state["checkpoints"]["candidate"]["fixture_publication"]["operation_id"], OPERATION)

    def test_completed_view_changes_do_not_soften_bad_payloads_or_foreign_boundaries(self):
        for fault in ("source", "identity", "epdc", "geometry", "json", "projection"):
            with self.subTest(fault=fault):
                records = self.performance()
                harness = self.harness(performance=records)
                pixels = self.source_projection()

                def changed():
                    if fault == "source":
                        self.publish_next_fixture(harness, records)
                    else:
                        self.publish_same_page_view(harness, records)
                    if fault == "identity":
                        harness.native_identity.return_value = {**harness.native_identity.return_value, "pid": PID + 1}
                    elif fault == "epdc":
                        harness.trace.return_value[-1]["native_epdc_mode"] = "Y8INV"
                        records[-1]["data"]["output_inverted"] = True

                self.attach_framebuffer(harness, pixels if fault in ("json", "projection") else pixels[:-1],
                                        changed_geometry=fault == "geometry", after_capture=changed)
                remote = harness.remote.side_effect

                def response(script, **kwargs):
                    result = remote(script, **kwargs)
                    if fault == "json" and script.endswith("/physical-framebuffer --capture"):
                        return CompletedProcess([], 0, b"invalid-json\n" + pixels, b"")
                    return result

                harness.remote.side_effect = response
                project = physical.framebuffer_panel_projection

                def projection(*args):
                    value = project(*args)
                    if fault == "projection":
                        value["status"] = "unavailable"
                    return value

                with patch.object(physical, "framebuffer_panel_projection", side_effect=projection):
                    with self.assertRaises((RuntimeError, ValueError)) as rejected:
                        self.run_retry_checkpoint(harness)
                self.assertNotIsInstance(rejected.exception, physical.RetryableReaderCaptureChange)
                self.assertEqual(self.retry_clock.sleeps, [])
                self.assertEqual(harness.capture.call_count, 1)
                self.assertNotIn("candidate", harness.state["checkpoints"])

    def test_native_only_advance_still_rejects_and_retains_wrong_page_pixels(self):
        records = self.performance()
        records[-1]["data"]["render_sequence"] = 1
        harness = self.harness(performance=records)
        pixels = self.source_projection(page=5)

        def redraw():
            fresh = deepcopy(records)
            fresh[-1]["monotonic_ns"] += 1000
            fresh[-1]["data"]["render_sequence"] += 1
            for timing in fresh[-1]["data"]["stages"].values():
                timing["started_monotonic_ns"] += 1000
                timing["finished_monotonic_ns"] += 1000
            records[:] = fresh

        self.attach_framebuffer(harness, pixels, after_capture=redraw)
        with self.assertRaisesRegex(RuntimeError, "marker differs"):
            self.run_retry_checkpoint(harness)
        self.assertEqual(self.retry_clock.sleeps, [])
        self.assertEqual(harness.capture.call_count, 1)
        self.assertNotIn("candidate", harness.state["checkpoints"])
        self.assertEqual(physical.file_hash(self.output / "candidate.framebuffer.raw"), physical.sha256(pixels))

    def test_completed_view_retry_keeps_the_original_deadline(self):
        records = self.performance()
        harness = self.harness(performance=records)

        def changed():
            self.retry_clock.now = 0.75
            self.publish_same_page_view(harness, records)

        self.attach_framebuffer(harness, self.source_projection(), after_capture=changed)
        with self.assertRaisesRegex(RuntimeError, "wait deadline expired"):
            self.run_retry_checkpoint(harness, wait=1)
        self.assertEqual(self.retry_clock.now, 1)
        self.assertEqual(self.retry_clock.sleeps, [0.25])
        self.assertEqual(harness.capture.call_count, 1)
        self.assertNotIn("candidate", harness.state["checkpoints"])

    def test_capture_change_allowlist_rejects_unknown_views_and_malformed_telemetry(self):
        records = self.performance()
        harness = self.harness(performance=records)
        before = harness.checkpoint_value("reader")
        self.publish_same_page_view(harness, records)
        after = harness.checkpoint_value("reader")
        self.assertTrue(physical.retryable_reader_capture_change(before, after))
        faults = (("reader_state", "sheet", "unknown-sheet"), ("reader_state", "rotation", 180),
                  ("reader_state", "page_id", "foreign-page"), ("reader_state", "viewport_width", 1),
                  ("reader_state", "zoom_ratio", float("nan")), ("reader_state", "zoom_ratio", True),
                  ("reader_state", "unknown_future_view", True))
        for container, key, value in faults:
            with self.subTest(key=key):
                changed = deepcopy(after)
                changed[container][key] = value
                self.assertFalse(physical.retryable_reader_capture_change(before, changed))
        for key, value in (("dirty_region", {"x": 0, "y": 0, "width": -1, "height": 1}),
                           ("stages", {"unknown_stage": {}}), ("render_sequence", True)):
            with self.subTest(key=key):
                old, changed = deepcopy(before), deepcopy(after)
                if key == "dirty_region":
                    old["fixture_publication"]["native_frame"][key] = {
                        "x": 0, "y": 0, "width": GEOMETRY["width"], "height": GEOMETRY["height"]}
                changed["fixture_publication"]["native_frame"][key] = value
                self.assertFalse(physical.retryable_reader_capture_change(old, changed))
        same_sequence = deepcopy(after)
        same_sequence["fixture_publication"]["native_frame"]["render_sequence"] = 1
        self.assertFalse(physical.retryable_reader_capture_change(before, same_sequence))

    def test_resource_only_cache_progress_and_eviction_keep_the_pixel_gate(self):
        for name, old, new in (("progress", (0, 0), (400, 2)),
                               ("eviction", (400, 2), (100, 1)), ("empty", (100, 1), (0, 0))):
            with self.subTest(name=name):
                harness = self.harness()
                harness.output = self.output / name
                harness.output.mkdir()
                harness.trace.return_value[-1]["reader_state"].update(
                    source_cache_budget=1024, source_cache_bytes=old[0], source_cache_entries=old[1])
                cache = {"source_cache_budget": 1024, "source_cache_bytes": new[0], "source_cache_entries": new[1]}
                pixels = self.source_projection()
                self.attach_framebuffer(harness, pixels, after_capture=lambda: self.publish_cache_snapshot(harness, cache))
                result = self.run_retry_checkpoint(harness, on_sleep=lambda: self.attach_framebuffer(harness, pixels))
                self.assertEqual(result["reader_state"]["source_cache_bytes"], new[0])
                self.assertEqual(result["reader_state"]["source_cache_entries"], new[1])
                self.assertEqual(harness.capture.call_count, 2)
                self.assertEqual(self.retry_clock.sleeps, [0.5])
                self.assertEqual(physical.file_hash(harness.output / "candidate.framebuffer.raw"), physical.sha256(pixels))
                attempt = harness.state["checkpoints"]["candidate"]["capture_attempts"][0]
                diagnostic = json.loads(Path(attempt["diagnostic"]).read_text())
                self.assertTrue(diagnostic["framebuffer_payload_retained"])
                change = physical.RetryableReaderCaptureChange(
                    diagnostic["candidate_checkpoint"], diagnostic["observed_checkpoint"])
                self.assertFalse(change.reader_view_changed)

    def test_cache_counts_and_budget_changes_remain_fatal(self):
        initial = {"source_cache_budget": 1024, "source_cache_bytes": 100, "source_cache_entries": 1}
        changes = [{"source_cache_bytes": True}, {"source_cache_entries": True}, {"source_cache_budget": True},
                   {"source_cache_bytes": -1}, {"source_cache_entries": -1}, {"source_cache_budget": -1},
                   {"source_cache_bytes": 1025}, {"source_cache_entries": 101},
                   {"source_cache_entries": 0}, {"source_cache_bytes": 0},
                   {"source_cache_budget": 2048}, {"source_cache_bytes": 100.0}]
        cases = [{**initial, **change} for change in changes]
        cases.extend({key: value for key, value in initial.items() if key != missing} for missing in initial)
        pixels = self.source_projection()
        for cache in cases:
            with self.subTest(cache=cache):
                harness = self.harness()
                harness.trace.return_value[-1]["reader_state"].update(initial)
                self.attach_framebuffer(harness, pixels, after_capture=lambda: self.publish_cache_snapshot(harness, cache))
                with self.assertRaises(RuntimeError) as rejected:
                    self.run_retry_checkpoint(harness)
                self.assertNotIsInstance(rejected.exception, physical.RetryableReaderCaptureChange)
                self.assertEqual(harness.capture.call_count, 1)
                self.assertEqual(self.retry_clock.sleeps, [])
                self.assertNotIn("candidate", harness.state["checkpoints"])

    def test_resource_only_cache_change_retains_and_rejects_bad_pixels(self):
        harness = self.harness()
        harness.trace.return_value[-1]["reader_state"].update(
            source_cache_budget=1024, source_cache_bytes=0, source_cache_entries=0)
        cache = {"source_cache_budget": 1024, "source_cache_bytes": 100, "source_cache_entries": 1}
        pixels = self.source_projection(page=5)
        self.attach_framebuffer(harness, pixels, after_capture=lambda: self.publish_cache_snapshot(harness, cache))
        with self.assertRaisesRegex(RuntimeError, "marker differs"):
            self.run_retry_checkpoint(harness)
        self.assertEqual(harness.capture.call_count, 1)
        self.assertEqual(self.retry_clock.sleeps, [])
        self.assertNotIn("candidate", harness.state["checkpoints"])
        self.assertEqual(physical.file_hash(self.output / "candidate.framebuffer.raw"), physical.sha256(pixels))

    def test_inputs_bind_guard_manifest_and_every_guest_file(self):
        value = physical.fixture_inputs(self.input_arguments())
        self.assertEqual(value["manifest_sha256"], self.manifest_sha)
        self.assertEqual(value["guard"], self.guard_bytes)
        self.assertEqual(value["guard_sha256"], physical.sha256(self.guard_bytes))
        self.assertEqual(value["manifest"]["account_key"], "guest")
        self.assertEqual(len(value["files"]), 13)
        for item in self.manifest["files"]:
            content, mode = value["files"][item["path"]]
            self.assertEqual((len(content), physical.sha256(content), mode), (item["bytes"], item["sha256"], 0o600))

    def test_inputs_reject_mixed_modes_missing_bindings_and_wrong_hashes(self):
        changes = [{"content_mode": "demo"}, {"content_mode": "unknown"},
                   {"expected_fixture_sha256": "0" * 64}, {"expected_guard_sha256": "0" * 64}]
        changes.extend({field: None} for field in ("fixture_directory", "expected_fixture_sha256",
                                                   "socket_guard", "expected_guard_sha256"))
        for change in changes:
            with self.subTest(change=change), self.assertRaises(RuntimeError):
                physical.fixture_inputs(self.input_arguments(**change))
        invalid = self.output / "not-arm"
        invalid.write_bytes(b"not an executable")
        with self.assertRaisesRegex(RuntimeError, "ARM ELF"):
            physical.fixture_inputs(self.input_arguments(socket_guard=invalid, expected_guard_sha256=physical.file_hash(invalid)))

    def test_inputs_delegate_real_credential_tree_rejection(self):
        contaminated = self.output / "contaminated"
        shutil.copytree(self.source, contaminated)
        private = contaminated / "data/account/private"
        private.mkdir(parents=True)
        (private / "session.json").write_text("{}", encoding="ascii")
        with patch.object(fixture, "validate_fixture", wraps=fixture.validate_fixture) as validator:
            with self.assertRaisesRegex(fixture.FixtureError, "credential account tree"):
                physical.fixture_inputs(self.input_arguments(fixture_directory=contaminated))
        validator.assert_called_once_with(contaminated)

    def test_guard_proof_binds_arm_process_and_complete_denials(self):
        proof = dict.fromkeys(physical.GUARD_FIELDS, 0)
        proof.update(offline_socket_guard=1, schema_version=1, mode=2, process_id=PID, installed=1,
                     checks_passed=1, seccomp_mode=2, audit_arch=1073741864, denied_socket_mask=63,
                     denied_socketpair_mask=63, unix_socket_allowed=1, unix_socketpair_allowed=1,
                     socketcall_checked=1, socketcall_denied=1, no_new_privs=1)
        self.assertEqual(physical.socket_guard_proof(json.dumps(proof).encode(), 2, PID), proof)
        for update in ({"process_id": PID - 1}, {"mode": 1}, {"denied_socket_mask": 31},
                       {"audit_arch": 3221225534}, {"installed": True}, {"extra": 1}):
            with self.subTest(update=update), self.assertRaises(RuntimeError):
                physical.socket_guard_proof(json.dumps({**proof, **update}).encode(), 2, PID)

    def test_non_demo_ready_checkpoint_has_complete_fixture_identity(self):
        harness = self.harness()
        checkpoint = harness.checkpoint_value("reader")
        self.assertFalse(any(value.get("event") == "ui-state" for value in harness.trace()))
        self.assertNotIn("ui", checkpoint)
        self.assertEqual(checkpoint["reader_state"], self.reader())
        self.assertEqual(checkpoint["fixture_manifest_sha256"], self.manifest_sha)
        self.assertEqual(checkpoint["fixture_evidence_class"], "controlled-offline-receipt-fixture")
        self.assertEqual(checkpoint["fixture_publication"]["binding"], "render_request")
        self.assertTrue(checkpoint["fixture_publication"]["request_generation_version_verified"])
        self.assertTrue(physical.fixture_ready_hint(self.trace(), self.manifest, "reader"))
        harness.native_identity.assert_called_once()
        harness.remote.assert_not_called()

    def test_old_process_hints_never_replace_current_identity(self):
        old_trace = self.trace(pid=PID - 1)
        self.assertTrue(physical.fixture_ready_hint(old_trace, self.manifest, "reader"))
        with self.assertRaisesRegex(RuntimeError, "not ready"):
            self.harness(traces=old_trace).checkpoint_value("reader")
        with self.assertRaisesRegex(RuntimeError, "model publication"):
            self.harness(performance=self.performance(pid=PID - 1)).checkpoint_value("reader")

    def test_stale_ready_closed_and_sleep_states_are_not_awake_evidence(self):
        for event in ("reader-closed", "quit"):
            with self.subTest(event=event), self.assertRaisesRegex(RuntimeError, "closed reader"):
                physical.fixture_reader_snapshot(self.trace() + [{"event": event, "process_id": PID}], self.manifest, "reader")
        stale = self.trace() + [{"process_id": PID, "event": "reader-state", "monotonic_ns": 210,
                               "reader_state": self.reader(5)}]
        with self.assertRaisesRegex(RuntimeError, "stale"):
            physical.fixture_reader_snapshot(stale, self.manifest, "reader")
        suspended = self.trace() + [{"process_id": PID, "event": "suspend", "monotonic_ns": 240,
                                   "suspended": True, "reader_state": self.reader()}]
        for kind in ("ready", "reader", "resumed"):
            with self.subTest(kind=kind), self.assertRaises(RuntimeError):
                self.harness(traces=suspended).checkpoint_value(kind)
        checkpoint = self.harness(traces=suspended).checkpoint_value("suspended")
        self.assertEqual(checkpoint["fixture_publication"], {"binding": "suspended-no-frame-capture", "marker_required": False})
        with self.assertRaisesRegex(RuntimeError, "sleep or unlock"):
            self.harness(traces=suspended).capture("sleep", checkpoint)

    def test_changed_descriptor_generation_and_anchor_cannot_reuse_readiness(self):
        changes = ({"comic_id": "other"}, {"episode_id": "other"}, {"revision": "other"},
                   {"page_id": "0" * 64}, {"page_index": 5}, {"page_number": 12},
                   {"reader_generation": 8}, {"render_version": 12}, {"rendering": 0})
        for change in changes:
            traces = self.trace()
            traces[-1]["reader_state"].update(change)
            with self.subTest(change=change), self.assertRaises(RuntimeError):
                self.harness(traces=traces).checkpoint_value("reader")
        narrow = self.trace()
        narrow[-1]["reader_state"].update(frame_width=FRAME[0], frame_height=FRAME[1])
        narrow[-1]["frame_size"] = FRAME.copy()
        narrow[-1]["reader_resources"]["page_size"] = {"width": FRAME[0], "height": FRAME[1]}
        with self.assertRaisesRegex(RuntimeError, "frame or viewport geometry"):
            self.harness(traces=narrow).checkpoint_value("reader")
        before = {"native_process": {"pid": PID}, "reader_state": self.reader()}
        self.assertFalse(physical.fixture_ready_hint(self.trace(5), self.manifest, "reader", before))
        self.assertTrue(physical.fixture_ready_hint(self.trace(5), self.manifest, "reader", before, "next-page"))
        changed = self.trace(5)
        changed[-1]["reader_state"]["launch_preference"] = "shelf"
        self.assertFalse(physical.fixture_ready_hint(changed, self.manifest, "reader", before, "next-page"))

    def test_deferred_publication_retains_an_explicit_evidence_gap(self):
        records = self.performance(deferred=True)
        snapshot = physical.fixture_reader_snapshot(self.trace(), self.manifest, "reader")
        value = physical.fixture_publication(records, snapshot, PID)
        self.assertEqual(value["binding"], "deferred_without_render_request")
        self.assertEqual(value["latency_origin"], "navigation_request")
        self.assertFalse(value["render_request_present"])
        self.assertFalse(value["request_generation_version_verified"])
        self.assertTrue(value["marker_required"])
        for missing in ("navigation_request", "render_deferred"):
            with self.subTest(missing=missing), self.assertRaisesRegex(RuntimeError, "deferred navigation chain"):
                physical.fixture_publication([value for value in records if value["stage"] != missing], snapshot, PID)

    def test_new_requests_and_incomplete_native_frames_reject_old_publications(self):
        snapshot = physical.fixture_reader_snapshot(self.trace(), self.manifest, "reader")
        for stage in ("navigation_request", "render_request", "render_deferred", "reader_transition"):
            pending = {"process_id": PID, "kind": "reader", "stage": stage,
                       "operation_id": OPERATION + 1, "monotonic_ns": 190}
            with self.subTest(stage=stage), self.assertRaisesRegex(RuntimeError, "newer reader request"):
                physical.fixture_publication(self.performance() + [pending], snapshot, PID)
        for change in ({"refresh_accepted": False}, {"refresh_ioctl_returned": False},
                       {"rotation_degrees": 180}, {"framebuffer_bits_per_pixel": 24}):
            records = self.performance()
            records[-1]["data"].update(change)
            with self.subTest(change=change), self.assertRaisesRegex(RuntimeError, "native frame"):
                physical.fixture_publication(records, snapshot, PID)
        records = self.performance()
        records[-1]["monotonic_ns"] = 199
        with self.assertRaisesRegex(RuntimeError, "native frame"):
            physical.fixture_publication(records, snapshot, PID)

    def test_actual_png_markers_accept_both_explicit_epdc_modes(self):
        source = self.source_projection()
        for mode in ("Y8", "Y8INV"):
            with self.subTest(mode=mode):
                harness = self.harness(mode=mode)
                checkpoint = harness.checkpoint_value("reader")
                raw = source.translate(physical.INVERT_BYTES) if mode == "Y8INV" else source
                self.attach_framebuffer(harness, raw)
                capture = harness.capture(mode.lower(), checkpoint)
                self.assertEqual(capture["fixture_marker"]["decoded_page_number"], 5)
                self.assertEqual(capture["fixture_marker"]["fixture_manifest_sha256"], self.manifest_sha)
                self.assertEqual(capture["fixture_marker"]["canvas_size"], CANVAS)
                self.assertEqual(capture["fixture_marker"]["fit_image_size"], FRAME)
                self.assertNotEqual(capture["fixture_marker"]["canvas_size"], capture["fixture_marker"]["fit_image_size"])
                self.assertEqual(capture["fixture_marker"]["fit_image_offset"], [39, 0])
                self.assertEqual(capture["raw_framebuffer"]["sha256"], physical.sha256(raw))
                self.assertFalse(capture["physical_panel_update_confirmed"])
                with Image.open(capture["panel_projection"]["path"]) as image:
                    self.assertEqual(image.tobytes(), source)

    def test_marker_rejects_wrong_page_orientation_and_polarity(self):
        harness = self.harness()
        checkpoint = harness.checkpoint_value("reader")
        memory = physical.framebuffer_memory_pixels(GEOMETRY, self.source_projection())
        projection = physical.framebuffer_panel_projection(GEOMETRY, memory, "Y8")
        variants = []
        unknown = deepcopy(checkpoint)
        unknown["native_process"]["portrait_calibration"]["direction"] = "L"
        variants.append((GEOMETRY, projection, unknown))
        residual = deepcopy(checkpoint)
        residual["fixture_publication"]["native_frame"]["rotation_degrees"] = 180
        variants.append((GEOMETRY, projection, residual))
        variants.append(({**GEOMETRY, "rotate": 1}, projection, checkpoint))
        wrong_mode = deepcopy(checkpoint)
        wrong_mode["native_epdc_mode"] = "Y8INV"
        variants.append((GEOMETRY, projection, wrong_mode))
        variants.append((GEOMETRY, {**projection, "pixels": projection["pixels"].translate(physical.INVERT_BYTES)}, checkpoint))
        for geometry, pixels, state in variants:
            with self.subTest(geometry=geometry["rotate"], mode=state["native_epdc_mode"]), self.assertRaises(RuntimeError):
                physical.fixture_marker(geometry, pixels, state, self.manifest)
        self.attach_framebuffer(harness, self.source_projection(5))
        with self.assertRaisesRegex(RuntimeError, "pinned source page"):
            harness.capture("wrong-page", checkpoint)
        self.assertFalse((self.output / "wrong-page.png").exists())
        self.assertFalse((self.output / "wrong-page.framebuffer.json").exists())

    def test_down_scanout_uses_bound_logical_framebuffer_coordinates_without_an_extra_rotation(self):
        harness = self.harness()
        checkpoint = harness.checkpoint_value("reader")
        checkpoint["native_process"]["portrait_calibration"].update(direction="D", fixed=False)
        memory = physical.framebuffer_memory_pixels(GEOMETRY, self.source_projection())
        projection = physical.framebuffer_panel_projection(GEOMETRY, memory, "Y8")
        marker = physical.fixture_marker(GEOMETRY, projection, checkpoint, self.manifest)
        self.assertEqual(marker["decoded_page_number"], 5)
        self.assertEqual(marker["startup_portrait_direction"], "D")
        self.assertEqual(marker["framebuffer_transform_degrees"], 0)
        self.assertFalse(marker["physical_panel_update_confirmed"])

    def test_capture_rejects_geometry_and_page_changes_before_artifact_acceptance(self):
        for fault in ("geometry", "reader"):
            with self.subTest(fault=fault):
                harness = self.harness()
                checkpoint = harness.checkpoint_value("reader")
                changed = (lambda: setattr(harness.trace, "return_value", self.trace(5))) if fault == "reader" else None
                self.attach_framebuffer(harness, self.source_projection(), changed_geometry=fault == "geometry", after_capture=changed)
                with self.assertRaisesRegex(RuntimeError, "changed during capture"):
                    harness.capture(fault, checkpoint)
                self.assertFalse((self.output / (fault + ".framebuffer.json")).exists())
                self.assertFalse((self.output / (fault + ".png")).exists())

    def test_real_deploy_preserves_content_mode_through_tar_file_modes(self):
        package = self.output / "synthetic-package.zip"
        script = self.output / "synthetic-wrapper.sh"
        script.write_bytes(b"#!/bin/sh\nexit 0\n")
        with zipfile.ZipFile(package, "w") as archive:
            archive.writestr("bilicomics/app/bilicomics", self.guard_bytes)
            archive.writestr("bilicomics/data/device-allowlist", b"# No accepted runtime pair\n")
            archive.writestr("bilicomics/launch.sh", script.read_bytes())
            archive.writestr("notices/synthetic.txt", b"Synthetic deployment test input\n")

        def proof(mode):
            value = dict.fromkeys(physical.GUARD_FIELDS, 0)
            value.update(offline_socket_guard=1, schema_version=1, mode=mode, process_id=PID, installed=1,
                         checks_passed=1, seccomp_mode=2, audit_arch=1073741864, denied_socket_mask=63,
                         denied_socketpair_mask=63, unix_socket_allowed=1, unix_socketpair_allowed=1,
                         socketcall_checked=1, socketcall_denied=1, no_new_privs=1)
            return json.dumps(value).encode("ascii")

        for content_mode in ("demo", "distinct_fixture"):
            with self.subTest(content_mode=content_mode):
                harness = self.harness()
                harness.output = self.output / content_mode
                harness.output.mkdir()
                harness.state.update(phase="preflight", expected_firmware="synthetic-firmware",
                                     preflight={"native_epdc_mode": "Y8"})
                harness.args = self.input_arguments(
                    content_mode=content_mode, approve_runtime_pair=True, package=package,
                    expected_package_sha256=physical.file_hash(package), framebuffer_helper=self.guard,
                    expected_helper_sha256=physical.file_hash(self.guard), expected_binary_sha256=physical.sha256(self.guard_bytes),
                    wrapper=script, watchdog=script, event_wrapper=script)
                if content_mode == "demo":
                    for field in ("fixture_directory", "expected_fixture_sha256", "socket_guard", "expected_guard_sha256"):
                        setattr(harness.args, field, None)
                staged = {}

                def remote(command, **kwargs):
                    if command.startswith("tar -xf "):
                        with tarfile.open(fileobj=io.BytesIO(kwargs["data"]), mode="r:") as archive:
                            for member in archive.getmembers():
                                with archive.extractfile(member) as stream:
                                    staged[member.name] = stream.read()
                        return CompletedProcess([], 0, b"", b"")
                    if command.startswith("sha256sum "):
                        hashes = "".join(f"{physical.sha256(data)}  {harness.root}/{name}\n"
                                         for name, data in sorted(staged.items()))
                        return CompletedProcess([], 0, hashes.encode("ascii"), b"")
                    if command.endswith("/physical-framebuffer --info"):
                        return CompletedProcess([], 0, json.dumps(GEOMETRY).encode("ascii"), b"")
                    if command.endswith(" --probe"):
                        return CompletedProcess([], 0, proof(1), b"")
                    if command.startswith("query=$(command -v lipc-get-prop)"):
                        return CompletedProcess([], 0, b"Y8\n", proof(2))
                    self.assertTrue(command.startswith("[ ! -e "), "Unexpected mocked deployment transport")
                    return CompletedProcess([], 0, b"", b"")

                harness.remote = Mock(side_effect=remote)
                harness.write = Mock()
                harness.save = Mock()
                result = harness.deploy()
                self.assertEqual(result["phase"], "deployed")
                self.assertEqual(staged["data/logs/content-mode"], (content_mode + "\n").encode("ascii"))
                self.assertEqual(harness.state["content_mode"], content_mode)
                self.assertIsInstance(harness.state["content_mode"], str)
                harness.save.assert_called_once()

    def test_resource_only_ready_snapshot_keeps_current_model_submission(self):
        traces = self.trace()
        later = deepcopy(traces[-1])
        later["monotonic_ns"] = 300
        later["reader_resources"]["decoded_cache_entries"] = 3
        traces.append(later)
        snapshot = physical.fixture_reader_snapshot(traces, self.manifest, "reader")
        performance = self.performance()
        self.assertGreater(snapshot["state_record"]["monotonic_ns"], performance[-1]["monotonic_ns"])
        value = physical.fixture_publication(performance, snapshot, PID)
        self.assertEqual(value["operation_id"], OPERATION)
        self.assertEqual(value["native_frame"], performance[-1]["data"])
        self.assertTrue(value["request_generation_version_verified"])
        pending = {"process_id": PID, "kind": "reader", "stage": "navigation_request",
                   "operation_id": OPERATION + 1, "monotonic_ns": 310}
        with self.assertRaisesRegex(RuntimeError, "newer reader request"):
            physical.fixture_publication(performance + [pending], snapshot, PID)

    def test_empty_native_samples_do_not_hide_success_or_the_latest_failure(self):
        performance = self.performance()
        snapshot = physical.fixture_reader_snapshot(self.trace(), self.manifest, "reader")
        empty = deepcopy(performance[-1])
        empty["monotonic_ns"] = 300
        empty["data"].update(refresh_ioctl_attempts=0, refresh_accepted=None, refresh_ioctl_returned=False,
                             refresh_errno=None, refresh_region=None, dirty_region={"width": 0, "height": 0}, stages={})
        value = physical.fixture_publication(performance + [empty], snapshot, PID)
        self.assertEqual(value["native_frame"], performance[-1]["data"])
        self.assertEqual(value["empty_native_frames_excluded"], 1)
        failed = deepcopy(performance[-1])
        failed["monotonic_ns"] = 320
        failed["data"].update(refresh_accepted=False, refresh_ioctl_returned=True, refresh_errno=5)
        later_empty = deepcopy(empty)
        later_empty["monotonic_ns"] = 330
        with self.assertRaises(RuntimeError):
            physical.fixture_publication(performance + [failed, later_empty], snapshot, PID)
        for change in ({"dirty_region": {"width": 10, "height": 10}}, {"refresh_errno": 5},
                       {"refresh_accepted": False}, {"refresh_region": {"width": 10, "height": 10}}):
            malformed = deepcopy(empty)
            malformed["data"].update(change)
            with self.subTest(change=change), self.assertRaisesRegex(RuntimeError, "without a refresh submission"):
                physical.fixture_publication(performance + [malformed], snapshot, PID)

    def test_native_identity_uses_guard_sidecar_while_application_log_is_buffered(self):
        self.assertIn("ui-socket-guard.json", physical.SAFE_LOGS)
        for fault in (None, "downward", "fixed", "wrong_pid", "missing_sidecar", "extra_arguments", "legacy_no_new_privs",
                      "missing_seccomp", "invalid_seccomp", "mismatched_no_new_privs"):
            with self.subTest(fault=fault):
                harness = self.harness()
                # Restore the real method hidden by the general capture stub.
                del harness.native_identity
                harness.state.update(binary_name="bilicomics-native", binary_sha256="c" * 64, exit_after_ms=60000)
                executable = harness.root + "/app/" + harness.state["binary_name"]
                token = harness.run_token()
                proof = dict.fromkeys(physical.GUARD_FIELDS, 0)
                proof.update(offline_socket_guard=1, schema_version=1, mode=2, process_id=PID, installed=1,
                             checks_passed=1, seccomp_mode=2, audit_arch=1073741864, denied_socket_mask=63,
                             denied_socketpair_mask=63, unix_socket_allowed=1, unix_socketpair_allowed=1,
                             socketcall_checked=1, socketcall_denied=1, no_new_privs=1)
                harness.state["socket_guard"] = {"passed": True, "probe": {**proof, "mode": 1, "process_id": PID + 10}}
                if fault == "wrong_pid":
                    proof["process_id"] = PID + 1
                application_log = harness.logs + "/application.log"
                sidecar = harness.logs + "/ui-socket-guard.json"
                files = {
                    harness.logs + "/native.identity": f"{PID}|1234|{executable}|1000|{token}\n".encode("ascii"),
                    application_log: b"",
                    sidecar: json.dumps(proof).encode("ascii") + b"\n",
                }
                if fault == "missing_sidecar":
                    del files[sidecar]

                def read(path, *, required=True):
                    if path not in files:
                        raise RuntimeError("The required guard proof sidecar is missing.")
                    return files[path]

                def remote(command, **kwargs):
                    if f"sha256sum /proc/{PID}/exe" in command:
                        self.assertIn("BILICOMICS_PHYSICAL_TOKEN=" + token, command)
                        self.assertIn("for flag in --offline --kindle;", command)
                        process_stat = f"{PID} (bilicomics) " + " ".join(["S"] + ["0"] * 18 + ["1234"])
                        result = f"{process_stat}\n{executable}\n{'c' * 64}  /proc/{PID}/exe\n"
                        return CompletedProcess([], 0, result.encode("ascii"), b"")
                    self.assertIn(f"cat /proc/{PID}/status", command)
                    self.assertIn("native-portrait-orientation", command)
                    self.assertIn("BILICOMICS_LAUNCH_TOKEN=$nonce", command)
                    self.assertIn('[ "$generation" = "1:$nonce" ]', command)
                    arguments = [executable, "--kindle", "--device", "scribe2025", "--data-dir", harness.root + "/data",
                                 "--performance-trace", harness.logs + "/performance.jsonl", "--offline", "--screen", "reader",
                                 "--lifecycle-trace", harness.logs + "/lifecycle.jsonl", "--exit-after-ms", "60000"]
                    if fault == "extra_arguments":
                        arguments.append("--demo")
                    status = b"Name:\tbilicomics\nSeccomp:\t2\nNoNewPrivs:\t1\nUid:\t0 0 0 0\nGid:\t0 0 0 0\n"
                    if fault in ("legacy_no_new_privs", "missing_seccomp", "invalid_seccomp"):
                        status = status.replace(b"NoNewPrivs:\t1\n", b"")
                    if fault == "missing_seccomp":
                        status = status.replace(b"Seccomp:\t2\n", b"")
                    elif fault == "invalid_seccomp":
                        status = status.replace(b"Seccomp:\t2\n", b"Seccomp:\t1\n")
                    elif fault == "mismatched_no_new_privs":
                        status = status.replace(b"NoNewPrivs:\t1\n", b"NoNewPrivs:\t0\n")
                    direction = "D" if fault == "downward" else "FIXED" if fault == "fixed" else "U"
                    result = status + b"\0" + b"\0".join(value.encode("ascii") for value in arguments) \
                        + b"\0\0" + (direction + ":2\n").encode("ascii")
                    return CompletedProcess([], 0, result, b"")

                harness.read = Mock(side_effect=read)
                harness.remote = Mock(side_effect=remote)
                if fault in (None, "downward", "fixed", "legacy_no_new_privs"):
                    identity = harness.native_identity()
                    self.assertEqual(identity["pid"], PID)
                    self.assertEqual(identity["live_seccomp_mode"], 2)
                    self.assertEqual(identity["socket_guard_proof"], proof)
                    self.assertEqual(identity["socket_guard_proof"]["no_new_privs"], 1)
                    self.assertEqual(identity["live_no_new_privs"], None if fault == "legacy_no_new_privs" else 1)
                    self.assertIn("pre-exec prctl", identity["live_no_new_privs_scope"])
                    self.assertTrue(identity["offline_guest_native_arguments_verified"])
                    self.assertFalse(identity["demo_offline_native_arguments_verified"])
                    self.assertEqual(identity["portrait_calibration"]["direction"], "D" if fault == "downward" else "U")
                    self.assertEqual(identity["portrait_calibration"]["fixed"], fault == "fixed")
                    self.assertEqual(identity["portrait_calibration"]["raw_rotation"], 2)
                    self.assertEqual(harness.remote.call_count, 2)
                else:
                    with self.assertRaises(RuntimeError):
                        harness.native_identity()
                self.assertEqual(files[application_log], b"")
                self.assertNotIn(application_log, [call.args[0] for call in harness.read.call_args_list])
                self.assertIn(sidecar, [call.args[0] for call in harness.read.call_args_list])

    def test_resume_reannounces_retained_model_without_another_reader_ready(self):
        # Preserve the observed ordering; all process reads remain synthetic.
        old_ready = 270444598670470
        suspend_time = 270689834174715
        reannounce_time = 270703530305793
        resume_time = 270703573599947
        frame_time = 270703761926639
        pid, birth, operation = 23688, 746256450, 10
        traces = self.trace(page=3, pid=pid)
        traces[0]["monotonic_ns"] = old_ready - 1000
        traces[-1]["monotonic_ns"] = old_ready
        traces[-1]["reader_state"]["render_version"] = 4
        retained = deepcopy(traces[-1]["reader_state"])
        traces.extend([
            {"process_id": pid, "event": "suspend", "monotonic_ns": suspend_time, "suspended": True,
             "reader_state": deepcopy(retained), "native_epdc_mode": "Y8"},
            {"process_id": pid, "event": "resume", "monotonic_ns": resume_time, "suspended": False,
             "reader_state": deepcopy(retained), "native_epdc_mode": "Y8"},
        ])
        performance = self.performance(pid=pid)
        for value in performance:
            value["operation_id"] = operation
        performance[0]["monotonic_ns"] = old_ready - 500
        performance[0]["data"]["version"] = 4
        performance[1]["monotonic_ns"] = old_ready - 250
        performance[2]["monotonic_ns"] = old_ready + 500
        performance[2]["data"]["reader_operation_id"] = operation
        performance[2]["data"]["stages"] = {
            "framebuffer_write": {"started_monotonic_ns": old_ready + 100, "finished_monotonic_ns": old_ready + 200},
            "refresh_submit": {"started_monotonic_ns": old_ready + 201, "finished_monotonic_ns": old_ready + 300},
        }
        reannounced = deepcopy(performance[1])
        reannounced["monotonic_ns"] = reannounce_time
        resumed_frame = deepcopy(performance[2])
        resumed_frame["monotonic_ns"] = frame_time
        resumed_frame["data"]["stages"] = {
            "framebuffer_write": {"started_monotonic_ns": resume_time + 100, "finished_monotonic_ns": resume_time + 200},
            "refresh_submit": {"started_monotonic_ns": resume_time + 201, "finished_monotonic_ns": resume_time + 300},
        }
        performance.extend([reannounced, resumed_frame])

        def checkpoint(lifecycle, records):
            harness = self.harness(traces=lifecycle, performance=records)
            harness.native_identity.return_value.update(pid=pid, start_ticks=birth)
            return harness.checkpoint_value("resumed")

        value = checkpoint(traces, performance)
        publication = value["fixture_publication"]
        self.assertEqual(sum(event["event"] == "reader-ready" for event in traces), 1)
        self.assertLess(old_ready, publication["published_monotonic_ns"])
        self.assertEqual(publication["operation_id"], operation)
        self.assertEqual(publication["published_monotonic_ns"], reannounce_time)
        self.assertEqual(publication["state_evidence_event"], "resume")
        self.assertEqual(publication["state_evidence_monotonic_ns"], resume_time)
        self.assertEqual(publication["native_frame"], resumed_frame["data"])
        self.assertEqual(value["reader_state"], retained)
        self.assertEqual((value["native_process"]["pid"], value["native_process"]["start_ticks"]), (pid, birth))
        for fault, message in (("early_state", "chronology"), ("changed_version", "stale"),
                               ("new_request", "newer reader request"), ("closed", "closed reader"),
                               ("suspended", "power state")):
            lifecycle, records = deepcopy(traces), deepcopy(performance)
            if fault == "early_state":
                lifecycle[-1]["monotonic_ns"] = reannounce_time - 1
            elif fault == "changed_version":
                lifecycle[-1]["reader_state"]["render_version"] = 5
            elif fault == "new_request":
                records.append({"process_id": pid, "kind": "reader", "stage": "navigation_request",
                                "operation_id": operation + 1, "monotonic_ns": frame_time + 1})
            elif fault == "closed":
                lifecycle.append({"process_id": pid, "event": "reader-closed", "monotonic_ns": frame_time + 1})
            else:
                lifecycle.append({"process_id": pid, "event": "suspend", "monotonic_ns": frame_time + 1,
                                  "suspended": True, "reader_state": deepcopy(retained)})
            with self.subTest(fault=fault), self.assertRaisesRegex(RuntimeError, message):
                checkpoint(lifecycle, records)


if __name__ == "__main__":
    unittest.main()
