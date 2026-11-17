#!/usr/bin/env python3
"""Exercise startup route-refresh evidence analysis and the host clock helper without devices."""

import argparse
import importlib.util
import json
import os
from pathlib import Path
import stat
import subprocess
import sys
import tempfile
import time
import unittest

from PIL import Image


sys.dont_write_bytecode = True
DIRECTORY = Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location("bili_startup_analysis", DIRECTORY / "analyze_startup_ab.py")
ANALYSIS = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(ANALYSIS)
CLOCK = None


def write_json(path, value):
    path.write_text(json.dumps(value) + "\n", encoding="utf-8")


def write_trace(path, rows):
    path.write_text("".join(json.dumps(row) + "\n" for row in rows), encoding="utf-8")


def read_trace(path):
    return [json.loads(line) for line in path.read_text().splitlines()]


def schedule():
    cases = ["baseline-warmup", "candidate-warmup"]
    for number in range(1, 7):
        order = ("baseline", "candidate") if number % 2 else ("candidate", "baseline")
        cases.extend(f"{name}-{number:02d}" for name in order)
    return cases


def envelope(pid, stamp, kind, stage, data):
    return {"schema_version": 1, "process_id": pid, "operation_id": 0, "kind": kind,
            "stage": stage, "monotonic_ns": stamp, "thread_cpu_ns": 1, "thread_id": pid,
            "trace_elapsed_us": 1, "data": data}


def timing(origin, start_ms, end_ms):
    return {"started_monotonic_ns": origin + start_ms * 1_000_000,
            "finished_monotonic_ns": origin + end_ms * 1_000_000,
            "started_thread_cpu_ns": 1, "finished_thread_cpu_ns": 2,
            "wall_ns": (end_ms - start_ms) * 1_000_000, "thread_cpu_ns": 1, "same_thread": True}


def frame(pid, origin, sequence, render_start, render_end, submit_start, submit_end,
          rendered=True, initial=False, kind="native", stage="frame"):
    data = {"render_sequence": sequence, "rendered": rendered, "initial_frame": initial,
            "forced_full": initial, "complete_frame_replacement": False, "framebuffer_size": [2, 2],
            "framebuffer_stride": 2, "framebuffer_bits_per_pixel": 8, "rotation_degrees": 0,
            "output_inverted": False, "black_and_white": False,
            "refresh_region": {"x": 0, "y": 0, "width": 2, "height": 2},
            "refresh_waveform": "gc16", "refresh_abi": "fixture", "refresh_flashing": initial,
            "refresh_ioctl_returned": True, "refresh_ioctl_attempts": 1, "refresh_accepted": True,
            "refresh_errno": None, "completion_errno": None, "preceding_update_marker": None,
            "accepted_update_marker": sequence + 1, "completed_update_marker": None,
            "driver_completion_observed": False, "panel_completion_observed": False,
            "stages": {"slint_render": timing(origin, render_start, render_end) if rendered else None,
                       "refresh_submit": timing(origin, submit_start, submit_end)}}
    return envelope(pid, origin + (submit_end + 20) * 1_000_000, kind, stage, data)


def make_batch(batch):
    cases = schedule()
    common_build = {"profile": "release", "target": "armv7-unknown-linux-musleabihf",
                    "default_features": False, "features": ["kindle"], "lto": "thin",
                    "codegen_units": 1, "incremental": False, "jobs": 6}
    variants = {}
    for number, name in enumerate(("baseline", "candidate"), 1):
        variants[name] = {"source_commit": "b" * 40,
                          "source_fingerprint": "1" * 64, "binary_sha256": "3" * 64,
                          "launcher_sha256": "5" * 64, "package_sha256": "7" * 64,
                          "worker_sha256": "9" * 64, "compiler_fingerprint": "c" * 64,
                          "instrumented_launcher_sha256": ("d" if number == 1 else "e") * 64,
                          "startup_arguments": list(ANALYSIS.STARTUP_ARGUMENTS[name]),
                          "build_configuration": common_build}
    report = {name: True for name in ANALYSIS.RESTORATION}
    metadata = {case: {"software_variant": case.split("-", 1)[0],
                      "measurement_role": "warmup" if case.endswith("-warmup") else "primary"}
                for case in cases}
    run_id = "startup-route-20261117-1234abcd"
    deployment_stage = "/mnt/us/bilicomics-" + run_id
    report.update(variants=variants, cases=cases, case_metadata=metadata,
                  run_id=run_id, deployment_stage=deployment_stage)
    write_json(batch / "report.json", report)
    for index, case in enumerate(cases):
        directory = batch / "outputs" / "out" / case
        directory.mkdir(parents=True)
        origin = 100_000_000_000 + index * 10_000_000_000
        pid = 1000 + index
        write_json(directory / "request.json", {"monotonic_ns": origin,
                   "boottime_ns": origin + 2_000_000_000, "pid": 2000 + index})
        variant = variants[case.split("-", 1)[0]]
        write_json(directory / "deployed-identity.json", {"binary_sha256": variant["binary_sha256"],
                   "launcher_sha256": variant["instrumented_launcher_sha256"]})
        rows = [envelope(pid, origin + 11_000_000, "diagnostic", "trace_started", {}),
                envelope(pid, origin + 12_000_000, "startup", "process_enter",
                         {"observed": {"monotonic_ns": origin + 10_000_000, "elapsed_ms": 0}}),
                frame(pid, origin, 0, 0, 0, 20, 25, rendered=False,
                      kind="startup", stage="native_initial_setup"),
                frame(pid, origin, 1, 60, 80, 90, 100, initial=True),
                envelope(pid, origin + 500_000_000, "startup", "ui_ready", None),
                frame(pid, origin, 2, 0, 0, 510, 520, rendered=False),
                frame(pid, origin, 3, 550, 570, 580, 600),
                envelope(pid, origin + 7_000_000_000, "diagnostic", "trace_stopped",
                         {"reason": "finished", "truncated": False})]
        write_trace(directory / "performance.jsonl", rows)
        # Deliberately unrelated to MONOTONIC endpoints: uptime is not an endpoint.
        (directory / "shell.tsv").write_text("5.00 document_entry\n5.05 native_take_begin\n5.10 native_take_done\n")
        write_json(directory / "recovery.json", {"passed": True, "native_restored": True,
                   "owned_processes_gone": True, "production_lock_absent": True, "launcher_exit_status": 0})
        (directory / "input-fds-at-content.txt").write_text("8 /dev/input/event1\n9 /dev/input/event2\n")
        (directory / "memory-at-content.txt").write_text("VmRSS:\t1024 kB\nVmHWM:\t2048 kB\nRssFile:\t512 kB\n")
        image = Image.new("RGB", (2, 2))
        image.putdata([(16, 32, 48), (64, 80, 96), (112, 128, 144), (160, 176, 192)])
        image.save(directory / "final.png")
        write_json(directory / "final.json", {"process_id": pid, "backend": "kindle", "demo": True,
                   "offline": True, "route": "shelf", "suspended": False, "profile": "scribe",
                   "pixel_semantics": "Logical Slint snapshot before framebuffer compensation",
                   "startup": {"booting": False, "error": False}, "snapshot_size": [2, 2],
                   "native_epdc_mode": "Y8"})
        write_trace(directory / "lifecycle.jsonl", [
            {"process_id": pid, "event": "ready", "native_epdc_mode": "Y8"},
            {"process_id": pid, "event": "quit", "native_epdc_mode": "Y8"}])
        target = origin + 2_100_000_000
        gate = {"monotonic_ns": target + 10_000_000, "boottime_ns": target + 2_010_000_000,
                "pid": 3000 + index}
        (directory / "post-business-target-ns.txt").write_text(str(target) + "\n")
        write_json(directory / "post-business-gate.json", gate)
        if case.endswith("-06"):
            geometry = {"width": 2, "height": 2, "virtual_width": 4, "virtual_height": 4,
                        "xoffset": 0, "yoffset": 0, "bits_per_pixel": 8, "line_length": 4,
                        "smem_len": 16, "visual": 1, "grayscale": 1, "rotate": 3,
                        "visible_bytes": 4, "byte_order": "little", "read_only": True}
            # Captured framebuffer bytes deliberately differ from the logical PNG.
            raw_frame = json.dumps(geometry).encode() + b"\n\x00\x55\xaa\xff"
            (directory / "content-frame.raw").write_bytes(raw_frame)
            (directory / "settled-frame.raw").write_bytes(raw_frame)
            (directory / "settled-deadline-ns.txt").write_text(str(target) + "\n")
            write_json(directory / "early-request.json", {"monotonic_ns": origin + 700_000_000,
                       "boottime_ns": origin + 2_700_000_000, "pid": 5000 + index})
            write_json(directory / "early-complete.json", {"monotonic_ns": origin + 750_000_000,
                       "boottime_ns": origin + 2_750_000_000, "pid": 6000 + index})
            write_json(directory / "settled-request.json", gate)
            write_json(directory / "settled-complete.json", {"monotonic_ns": target + 60_000_000,
                       "boottime_ns": target + 2_060_000_000, "pid": 7000 + index})
            write_json(directory / "capture-liveness.json", {"application_pid": pid, "start_ticks": 123000 + index,
                       "early_before": True, "early_after": True, "settled_before": True, "settled_after": True})
    return batch


class AnalyzerTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="bili-startup-analysis-test-")
        self.addCleanup(self.temporary.cleanup)
        self.batch = make_batch(Path(self.temporary.name))

    def directory(self, case="baseline-01"):
        return self.batch / "outputs" / "out" / case

    def test_complete_batch_uses_monotonic_and_excludes_warmups(self):
        result = ANALYSIS.analyze_batch(self.batch)
        self.assertEqual(result["status"], "passed")
        self.assertEqual(result["measured_pairs"], 6)
        self.assertEqual(len(result["cases"]), 14)
        self.assertEqual(len(result["primary_cases"]), 12)
        self.assertEqual(result["warmup_cases"], ["baseline-warmup", "candidate-warmup"])
        for variant in ("baseline", "candidate"):
            self.assertEqual(result["summary"][variant]["software_ready_ms"]["n"], 6)
            self.assertEqual(result["summary"][variant]["software_ready_ms"]["median"], 500)
            self.assertEqual(result["summary"][variant]["content_send_ms"]["median"], 600)
        first = result["cases"][0]
        self.assertEqual(first["metrics"]["first_startup_send_ms"], 100)
        self.assertEqual(first["first_business_frame"]["render_sequence"], 3)
        self.assertEqual(first["first_business_frame"]["record_after_request_ms"], 620)
        self.assertFalse(first["final_capture"]["physical_framebuffer_capture"])

    def test_frame_started_before_ready_is_skipped_despite_later_acceptance(self):
        path = self.directory() / "performance.jsonl"
        rows = read_trace(path)
        origin = json.loads((self.directory() / "request.json").read_text())["monotonic_ns"]
        rows.insert(-2, frame(rows[0]["process_id"], origin, 77, 490, 510, 520, 530))
        write_trace(path, rows)
        result = ANALYSIS.analyze_batch(self.batch)
        selected = next(row for row in result["cases"] if row["case"] == "baseline-01")
        self.assertEqual(selected["metrics"]["content_send_ms"], 600)
        self.assertEqual(selected["first_business_frame"]["render_sequence"], 3)

    def test_no_business_frame_when_only_render_started_before_ready(self):
        path = self.directory() / "performance.jsonl"
        rows = read_trace(path)
        origin = json.loads((self.directory() / "request.json").read_text())["monotonic_ns"]
        rows[-2] = frame(rows[0]["process_id"], origin, 77, 490, 510, 520, 530)
        write_trace(path, rows)
        with self.assertRaisesRegex(ANALYSIS.AnalysisError, "missing_qualified"):
            ANALYSIS.analyze_batch(self.batch)

    def test_case_failed_recovery_is_rejected(self):
        path = self.directory() / "recovery.json"
        value = json.loads(path.read_text())
        value["owned_processes_gone"] = False
        write_json(path, value)
        with self.assertRaisesRegex(ANALYSIS.AnalysisError, "case_recovery_failed"):
            ANALYSIS.analyze_batch(self.batch)

    def test_unreleased_batch_is_rejected(self):
        path = self.batch / "report.json"
        value = json.loads(path.read_text())
        value["lease_released"] = False
        write_json(path, value)
        with self.assertRaisesRegex(ANALYSIS.AnalysisError, "not_successfully_restored"):
            ANALYSIS.analyze_batch(self.batch)

    def test_changed_input_node_is_rejected(self):
        (self.directory("candidate-01") / "input-fds-at-content.txt").write_text(
            "8 /dev/input/event1\n9 /dev/input/event3\n")
        with self.assertRaisesRegex(ANALYSIS.AnalysisError, "selected_input_devices_differ"):
            ANALYSIS.analyze_batch(self.batch)

    def test_sleep_wake_in_a_non_capture_case_is_rejected(self):
        path = self.directory("candidate-01") / "lifecycle.jsonl"
        rows = read_trace(path)
        pid = rows[0]["process_id"]
        rows[1:1] = [{"process_id": pid, "event": "suspend", "native_epdc_mode": "Y8"},
                     {"process_id": pid, "event": "resume", "native_epdc_mode": "Y8"}]
        write_trace(path, rows)
        with self.assertRaisesRegex(ANALYSIS.AnalysisError, "lifecycle_transition_during_startup_case"):
            ANALYSIS.analyze_batch(self.batch)

    def test_terminal_null_has_a_controlled_rejection(self):
        path = self.directory() / "performance.jsonl"
        rows = read_trace(path)
        rows[-1]["data"] = None
        write_trace(path, rows)
        with self.assertRaisesRegex(ANALYSIS.AnalysisError, "trace_did_not_finish_cleanly"):
            ANALYSIS.analyze_batch(self.batch)

    def test_different_release_flags_are_rejected(self):
        path = self.batch / "report.json"
        value = json.loads(path.read_text())
        value["variants"]["candidate"]["build_configuration"]["lto"] = False
        write_json(path, value)
        with self.assertRaisesRegex(ANALYSIS.AnalysisError, "build_configurations_differ"):
            ANALYSIS.analyze_batch(self.batch)

    def test_logical_and_framebuffer_pixels_have_distinct_evidence_scopes(self):
        result = ANALYSIS.analyze_batch(self.batch)
        selected = next(row for row in result["cases"] if row["case"] == "baseline-06")
        self.assertTrue(result["final_logical_business_pixels_match"])
        self.assertTrue(result["early_framebuffer_memory_pixels_match"])
        self.assertTrue(result["settled_framebuffer_memory_pixels_match"])
        self.assertNotEqual(selected["final_capture"]["pixel_sha256"],
                            selected["framebuffer_capture"]["visible_raw_sha256"])
        self.assertFalse(selected["framebuffer_capture"]["physical_panel_update_confirmed"])
        self.assertEqual(result["framebuffer_comparisons"]["settled"]["cases"], ["baseline-06", "candidate-06"])
        self.assertEqual(selected["settled_framebuffer_capture"]["observation_timing"]["deadline_slip_ms"], 10)
        self.assertEqual(selected["framebuffer_capture"]["capture_window"]["bracket_wall_ms"], 50)
        self.assertEqual(selected["settled_framebuffer_capture"]["capture_window"]["bracket_wall_ms"], 50)
        self.assertEqual(selected["capture_liveness_status"], "same_ui_process_verified_before_and_after_both_capture_windows")
        self.assertFalse(any("settled" in name for name in selected["metrics"]))

    def test_early_framebuffer_difference_is_retained_without_mask_or_failure(self):
        path = self.directory("candidate-06") / "content-frame.raw"
        value = path.read_bytes()
        path.write_bytes(value[:-1] + b"\x00")
        result = ANALYSIS.analyze_batch(self.batch)
        self.assertEqual(result["status"], "passed")
        self.assertFalse(result["early_framebuffer_memory_pixels_match"])
        self.assertTrue(result["settled_framebuffer_memory_pixels_match"])
        compared = result["framebuffer_comparisons"]["early"]
        self.assertEqual(compared["visible_pixels"], 4)
        self.assertEqual(compared["changed_pixels"], 1)
        self.assertEqual(compared["changed_fraction"], 0.25)
        self.assertEqual(compared["maximum_absolute_difference"], 255)
        self.assertEqual(compared["mean_absolute_difference"], 63.75)
        self.assertEqual(compared["changed_bounding_box"], [1, 1, 2, 2])
        self.assertEqual(result["summary"]["candidate"]["content_send_ms"]["median"], 600)

    def test_settled_framebuffer_difference_is_rejected(self):
        path = self.directory("candidate-06") / "settled-frame.raw"
        value = path.read_bytes()
        path.write_bytes(value[:-1] + b"\x00")
        with self.assertRaisesRegex(ANALYSIS.AnalysisError, "settled_framebuffer_pixels_or_geometry_differ"):
            ANALYSIS.analyze_batch(self.batch)

    def test_settled_target_must_be_1500ms_after_first_business_send(self):
        path = self.directory("candidate-06") / "settled-deadline-ns.txt"
        path.write_text(str(int(path.read_text()) + 1_000_000) + "\n")
        with self.assertRaisesRegex(ANALYSIS.AnalysisError, "settled_target_is_not_first_send_plus_1500ms"):
            ANALYSIS.analyze_batch(self.batch)

    def test_settled_capture_before_target_is_rejected(self):
        directory = self.directory("candidate-06")
        path = directory / "settled-request.json"
        value = json.loads(path.read_text())
        value["monotonic_ns"] = int((directory / "settled-deadline-ns.txt").read_text()) - 1
        write_json(path, value)
        with self.assertRaisesRegex(ANALYSIS.AnalysisError, "settled_capture_did_not_start_at_or_after_target"):
            ANALYSIS.analyze_batch(self.batch)

    def test_settled_lateness_is_reported_without_replacing_startup_endpoints(self):
        directory = self.directory("candidate-06")
        path = directory / "settled-request.json"
        value = json.loads(path.read_text())
        target = int((directory / "settled-deadline-ns.txt").read_text())
        value["monotonic_ns"] = target + 12_345_678
        write_json(path, value)
        write_json(directory / "post-business-gate.json", value)
        result = ANALYSIS.analyze_batch(self.batch)
        selected = next(row for row in result["cases"] if row["case"] == "candidate-06")
        self.assertEqual(selected["settled_framebuffer_capture"]["observation_timing"]["deadline_slip_ms"], 12.345678)
        self.assertEqual(selected["metrics"]["software_ready_ms"], 500)
        self.assertEqual(selected["metrics"]["content_send_ms"], 600)

    def test_common_gate_is_required_in_a_non_capture_case(self):
        path = self.directory("candidate-01") / "post-business-target-ns.txt"
        path.write_text(str(int(path.read_text()) + 1_000_000) + "\n")
        with self.assertRaisesRegex(ANALYSIS.AnalysisError, "post_business_target_is_not_first_send_plus_1500ms"):
            ANALYSIS.analyze_batch(self.batch)

    def test_common_gate_before_target_is_rejected(self):
        path = self.directory("candidate-01") / "post-business-gate.json"
        value = json.loads(path.read_text())
        value["monotonic_ns"] = int((path.parent / "post-business-target-ns.txt").read_text()) - 1
        write_json(path, value)
        with self.assertRaisesRegex(ANALYSIS.AnalysisError, "post_business_gate_is_early_or_after_shutdown"):
            ANALYSIS.analyze_batch(self.batch)

    def test_settled_capture_and_gate_must_be_the_same_record(self):
        path = self.directory("candidate-06") / "settled-request.json"
        value = json.loads(path.read_text())
        value["pid"] += 1
        write_json(path, value)
        with self.assertRaisesRegex(ANALYSIS.AnalysisError, "settled_capture_does_not_match_common_post_business_gate"):
            ANALYSIS.analyze_batch(self.batch)

    def test_raw_capture_completion_cannot_precede_its_request(self):
        directory = self.directory("candidate-06")
        for prefix in ("early", "settled"):
            with self.subTest(prefix=prefix):
                path = directory / (prefix + "-complete.json")
                original = path.read_bytes()
                value = json.loads(original)
                value["monotonic_ns"] = json.loads((directory / (prefix + "-request.json")).read_text())["monotonic_ns"] - 1
                write_json(path, value)
                with self.assertRaisesRegex(ANALYSIS.AnalysisError, "invalid_" + prefix + "_capture_window"):
                    ANALYSIS.analyze_batch(self.batch)
                path.write_bytes(original)

    def test_raw_capture_completion_must_precede_ui_terminal(self):
        directory = self.directory("candidate-06")
        terminal = read_trace(directory / "performance.jsonl")[-1]["monotonic_ns"]
        for prefix in ("early", "settled"):
            with self.subTest(prefix=prefix):
                path = directory / (prefix + "-complete.json")
                original = path.read_bytes()
                value = json.loads(original)
                value["monotonic_ns"] = terminal
                write_json(path, value)
                with self.assertRaisesRegex(ANALYSIS.AnalysisError, prefix + "_capture_reaches_or_exceeds_ui_trace_terminal"):
                    ANALYSIS.analyze_batch(self.batch)
                path.write_bytes(original)

    def test_early_raw_capture_cannot_precede_first_business_send(self):
        path = self.directory("candidate-06") / "early-request.json"
        value = json.loads(path.read_text())
        origin = json.loads((path.parent / "request.json").read_text())["monotonic_ns"]
        value["monotonic_ns"] = origin + 599_000_000
        write_json(path, value)
        with self.assertRaisesRegex(ANALYSIS.AnalysisError, "invalid_early_capture_window"):
            ANALYSIS.analyze_batch(self.batch)

    def test_raw_capture_requires_live_ui_before_and_after_both_windows(self):
        path = self.directory("candidate-06") / "capture-liveness.json"
        original = json.loads(path.read_text())
        for field in ("early_before", "early_after", "settled_before", "settled_after"):
            for update in (None, False, 1):
                with self.subTest(field=field, update=update):
                    value = dict(original)
                    value[field] = update
                    write_json(path, value)
                    with self.assertRaisesRegex(ANALYSIS.AnalysisError, "capture_liveness_is_not_proven"):
                        ANALYSIS.analyze_batch(self.batch)

    def test_raw_capture_liveness_must_bind_the_ui_pid_and_positive_birth(self):
        path = self.directory("candidate-06") / "capture-liveness.json"
        original = json.loads(path.read_text())
        value = dict(original)
        value["application_pid"] += 1
        write_json(path, value)
        with self.assertRaisesRegex(ANALYSIS.AnalysisError, "capture_liveness_ui_process_differs"):
            ANALYSIS.analyze_batch(self.batch)
        value = dict(original)
        value["start_ticks"] = 0
        write_json(path, value)
        with self.assertRaisesRegex(ANALYSIS.AnalysisError, "invalid_capture_application_start_ticks"):
            ANALYSIS.analyze_batch(self.batch)

    def test_early_and_settled_raw_capture_windows_cannot_overlap(self):
        directory = self.directory("candidate-06")
        path = directory / "early-complete.json"
        value = json.loads(path.read_text())
        settled_request = json.loads((directory / "settled-request.json").read_text())["monotonic_ns"]
        value["monotonic_ns"] = settled_request + 1
        write_json(path, value)
        with self.assertRaisesRegex(ANALYSIS.AnalysisError, "early_and_settled_capture_windows_overlap_or_are_reversed"):
            ANALYSIS.analyze_batch(self.batch)

    def test_warmups_are_retained_without_entering_primary_statistics(self):
        for case in ("baseline-warmup", "candidate-warmup"):
            path = self.directory(case) / "performance.jsonl"
            rows = read_trace(path)
            ready = next(row for row in rows if row["kind"] == "startup" and row["stage"] == "ui_ready")
            ready["monotonic_ns"] -= 50_000_000
            write_trace(path, rows)
        result = ANALYSIS.analyze_batch(self.batch)
        warmups = [row for row in result["cases"] if row["measurement_role"] == "warmup"]
        self.assertEqual([row["metrics"]["software_ready_ms"] for row in warmups], [450, 450])
        self.assertTrue(all(not row["included_in_primary_statistics"] for row in warmups))
        self.assertEqual(result["summary"]["candidate"]["software_ready_ms"]["n"], 6)
        self.assertEqual(result["summary"]["candidate"]["software_ready_ms"]["median"], 500)

    def test_warmup_role_cannot_be_promoted_to_primary(self):
        path = self.batch / "report.json"
        value = json.loads(path.read_text())
        value["case_metadata"]["baseline-warmup"]["measurement_role"] = "primary"
        write_json(path, value)
        with self.assertRaisesRegex(ANALYSIS.AnalysisError, "case_role_or_software_variant_differs"):
            ANALYSIS.analyze_batch(self.batch)

    def test_source_package_and_compiler_identities_must_be_shared(self):
        path = self.batch / "report.json"
        original = json.loads(path.read_text())
        for field in (*ANALYSIS.IDENTITY_FIELDS, "worker_sha256"):
            with self.subTest(field=field):
                value = json.loads(json.dumps(original))
                value["variants"]["candidate"][field] = "0" * (40 if field == "source_commit" else 64)
                write_json(path, value)
                with self.assertRaisesRegex(ANALYSIS.AnalysisError, "variant_" + field + "_differs"):
                    ANALYSIS.analyze_batch(self.batch)

    def test_frozen_startup_arguments_cannot_be_added_removed_or_reordered(self):
        path = self.batch / "report.json"
        original = json.loads(path.read_text())
        changes = (("baseline", ["--demo", "--offline"]),
                   ("baseline", ["--demo", "--offline", "--serial-startup-preparation"]),
                   ("baseline", ["--demo", "--offline", "--legacy-startup-route-refresh", "--serial-startup-preparation"]),
                   ("candidate", ["--demo", "--offline", "--legacy-startup-route-refresh"]),
                   ("candidate", ["--demo", "--offline", "--serial-startup-preparation"]),
                   ("candidate", ["--offline", "--demo"]),
                   ("candidate", ["--demo", "--offline", "--screen", "shelf"]))
        for name, arguments in changes:
            with self.subTest(variant=name, arguments=arguments):
                value = json.loads(json.dumps(original))
                value["variants"][name]["startup_arguments"] = arguments
                write_json(path, value)
                with self.assertRaisesRegex(ANALYSIS.AnalysisError, "startup_arguments_differ"):
                    ANALYSIS.analyze_batch(self.batch)

    def test_build_configuration_and_instrumented_launcher_are_required(self):
        path = self.batch / "report.json"
        original = json.loads(path.read_text())
        for field in ("build_configuration", "instrumented_launcher_sha256"):
            with self.subTest(field=field):
                value = json.loads(json.dumps(original))
                value["variants"]["candidate"].pop(field)
                write_json(path, value)
                with self.assertRaisesRegex(ANALYSIS.AnalysisError, field):
                    ANALYSIS.analyze_batch(self.batch)

    def test_mismatched_deployed_application_or_launcher_is_rejected(self):
        path = self.directory("candidate-01") / "deployed-identity.json"
        original = json.loads(path.read_text())
        for field in ("binary_sha256", "launcher_sha256"):
            with self.subTest(field=field):
                value = dict(original)
                value[field] = "0" * 64
                write_json(path, value)
                with self.assertRaisesRegex(ANALYSIS.AnalysisError, "case_deployed_binary_or_launcher_identity_differs"):
                    ANALYSIS.analyze_batch(self.batch)

    def test_legacy_route_refresh_argument_requires_distinct_instrumented_launchers(self):
        path = self.batch / "report.json"
        value = json.loads(path.read_text())
        value["variants"]["candidate"]["instrumented_launcher_sha256"] = value["variants"]["baseline"]["instrumented_launcher_sha256"]
        write_json(path, value)
        with self.assertRaisesRegex(ANALYSIS.AnalysisError, "legacy_route_refresh_argument_did_not_change_instrumented_launcher"):
            ANALYSIS.analyze_batch(self.batch)

    def test_extra_variant_and_missing_predefined_case_are_rejected(self):
        path = self.batch / "report.json"
        original = json.loads(path.read_text())
        value = json.loads(json.dumps(original))
        value["variants"]["extra"] = dict(value["variants"]["candidate"])
        write_json(path, value)
        with self.assertRaisesRegex(ANALYSIS.AnalysisError, "invalid_variants"):
            ANALYSIS.analyze_batch(self.batch)
        value = json.loads(json.dumps(original))
        value["cases"].pop()
        write_json(path, value)
        with self.assertRaisesRegex(ANALYSIS.AnalysisError, "insufficient_or_invalid_pairs"):
            ANALYSIS.analyze_batch(self.batch)

    def test_fresh_stage_must_be_bound_to_the_run_identity(self):
        path = self.batch / "report.json"
        value = json.loads(path.read_text())
        value["deployment_stage"] += "-different"
        write_json(path, value)
        with self.assertRaisesRegex(ANALYSIS.AnalysisError, "startup_route_deployment_stage_differs"):
            ANALYSIS.analyze_batch(self.batch)

    def test_truncated_framebuffer_bytes_are_rejected(self):
        path = self.directory("candidate-06") / "content-frame.raw"
        path.write_bytes(path.read_bytes()[:-1])
        with self.assertRaisesRegex(ANALYSIS.AnalysisError, "framebuffer_capture_geometry"):
            ANALYSIS.analyze_batch(self.batch)

    def test_alternating_pair_order_is_required(self):
        path = self.batch / "report.json"
        value = json.loads(path.read_text())
        value["cases"][4:6] = list(reversed(value["cases"][4:6]))
        write_json(path, value)
        with self.assertRaisesRegex(ANALYSIS.AnalysisError, "alternating_ab_ba"):
            ANALYSIS.analyze_batch(self.batch)

    def test_actual_case_timing_must_follow_declared_counterbalanced_order(self):
        first = self.directory("baseline-01")
        second = self.directory("candidate-01")
        for path in list(first.iterdir()):
            matching = second / path.name
            first_bytes, second_bytes = path.read_bytes(), matching.read_bytes()
            path.write_bytes(second_bytes)
            matching.write_bytes(first_bytes)
        # Preserve correct per-variant deployment proofs while swapping the
        # actual launch intervals, so only temporal order reveals the mismatch.
        report = json.loads((self.batch / "report.json").read_text())
        for case in ("baseline-01", "candidate-01"):
            variant = report["variants"][case.split("-", 1)[0]]
            write_json(self.directory(case) / "deployed-identity.json", {
                "binary_sha256": variant["binary_sha256"], "launcher_sha256": variant["instrumented_launcher_sha256"]})
        with self.assertRaisesRegex(ANALYSIS.AnalysisError, "actual_case_request_order_or_overlap_differs"):
            ANALYSIS.analyze_batch(self.batch)

    def test_cli_writes_new_evidence_and_refuses_to_replace_it(self):
        command = [sys.executable, str(DIRECTORY / "analyze_startup_ab.py"), "--batch-dir", str(self.batch)]
        first = subprocess.run(command, capture_output=True, check=False)
        self.assertEqual(first.returncode, 0, first.stderr.decode())
        output = self.batch / "analysis.json"
        original = output.read_bytes()
        second = subprocess.run(command, capture_output=True, check=False)
        self.assertEqual(second.returncode, 1)
        self.assertEqual(original, output.read_bytes())


class ClockTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="bili-startup-clock-test-")
        self.addCleanup(self.temporary.cleanup)
        self.directory = Path(self.temporary.name)
        self.output = self.directory / "request.json"

    def command(self, *arguments):
        return [str(CLOCK), str(self.output), "--", *arguments]

    def test_exact_arguments_exit_status_and_clock_record(self):
        arguments = ["space separated", "$(never-execute)", "quote\"'", "-n", ""]
        result = subprocess.run(self.command(sys.executable, "-c",
            "import json,sys;print(json.dumps(sys.argv[1:]));sys.exit(7)", *arguments),
            capture_output=True, check=False)
        self.assertEqual(result.returncode, 7, result.stderr.decode())
        self.assertEqual(json.loads(result.stdout), arguments)
        value = json.loads(self.output.read_text())
        self.assertEqual(set(value), {"monotonic_ns", "boottime_ns", "pid"})
        self.assertGreater(value["monotonic_ns"], 0)
        self.assertGreater(value["pid"], 1)
        self.assertTrue(value["boottime_ns"] is None or value["boottime_ns"] > 0)
        self.assertEqual(stat.S_IMODE(self.output.stat().st_mode), 0o600)

    def test_existing_record_is_retained_and_command_is_not_executed(self):
        self.output.write_bytes(b"retained evidence\n")
        marker = self.directory / "must-not-exist"
        result = subprocess.run(self.command(sys.executable, "-c",
            "import pathlib,sys;pathlib.Path(sys.argv[1]).touch()", str(marker)),
            capture_output=True, check=False)
        self.assertEqual(result.returncode, 73)
        self.assertEqual(self.output.read_bytes(), b"retained evidence\n")
        self.assertFalse(marker.exists())

    def test_symlink_record_is_rejected_without_following_it(self):
        target = self.directory / "target"
        target.write_bytes(b"retained target\n")
        self.output.symlink_to(target)
        result = subprocess.run(self.command("/bin/true"), capture_output=True, check=False)
        self.assertEqual(result.returncode, 73)
        self.assertTrue(self.output.is_symlink())
        self.assertEqual(target.read_bytes(), b"retained target\n")

    def test_missing_executable_preserves_the_request_record(self):
        result = subprocess.run(self.command(str(self.directory / "missing-program")),
                                capture_output=True, check=False)
        self.assertEqual(result.returncode, 127)
        self.assertGreater(json.loads(self.output.read_text())["monotonic_ns"], 0)

    def test_absolute_wait_records_after_target_and_preserves_arguments_and_exit(self):
        target = time.monotonic_ns() + 100_000_000
        arguments = ["space separated", "$(never-execute)", "--"]
        command = [str(CLOCK), "--wait-until", str(target), str(self.output), "--", sys.executable,
                   "-c", "import json,sys;print(json.dumps(sys.argv[1:]));sys.exit(13)", *arguments]
        result = subprocess.run(command, capture_output=True, check=False)
        self.assertEqual(result.returncode, 13, result.stderr.decode())
        self.assertEqual(json.loads(result.stdout), arguments)
        self.assertGreaterEqual(json.loads(self.output.read_text())["monotonic_ns"], target)
        self.assertEqual(stat.S_IMODE(self.output.stat().st_mode), 0o600)

    def test_past_absolute_target_runs_immediately(self):
        target = time.monotonic_ns() - 1_000_000_000
        result = subprocess.run([str(CLOCK), "--wait-until", str(target), str(self.output), "--", "/bin/true"],
                                capture_output=True, check=False)
        self.assertEqual(result.returncode, 0, result.stderr.decode())
        self.assertGreaterEqual(json.loads(self.output.read_text())["monotonic_ns"], target)

    def test_future_target_over_budget_does_not_create_record_or_execute(self):
        marker = self.directory / "must-not-exist"
        target = time.monotonic_ns() + 60_000_000_000
        command = [str(CLOCK), "--wait-until", str(target), str(self.output), "--", sys.executable,
                   "-c", "import pathlib,sys;pathlib.Path(sys.argv[1]).touch()", str(marker)]
        result = subprocess.run(command, capture_output=True, check=False, timeout=5)
        self.assertEqual(result.returncode, 70)
        self.assertFalse(self.output.exists())
        self.assertFalse(marker.exists())

    def test_invalid_absolute_targets_are_rejected_before_creation(self):
        for target in ("", "-1", "+1", " 1", "1x", "18446744073709551616", "123456789012345678901"):
            with self.subTest(target=target):
                result = subprocess.run([str(CLOCK), "--wait-until", target, str(self.output), "--", "/bin/true"],
                                        capture_output=True, check=False)
                self.assertEqual(result.returncode, 64)
                self.assertFalse(self.output.exists())


def main(argv=None):
    global CLOCK
    require_linux = sys.platform == "linux"
    if not require_linux:
        raise SystemExit("Run host-only evidence tests on test-env.")
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--clock", type=Path, required=True)
    args, remaining = parser.parse_known_args(argv)
    CLOCK = args.clock.resolve(strict=True)
    if not CLOCK.is_file() or not os.access(CLOCK, os.X_OK):
        raise SystemExit("Select the host-built startup-clock executable.")
    unittest.main(argv=[sys.argv[0], *remaining])


if __name__ == "__main__":
    main()
