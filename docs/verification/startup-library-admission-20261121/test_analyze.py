"""Synthetic evidence only; these tests never access a device or private logs."""

import importlib.util
import json
from pathlib import Path
import tempfile
import unittest


HERE = Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location("analyze_library_admission", HERE / "analyze.py")
ANALYSIS = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(ANALYSIS)

OWNER = "01234567-89ab-cdef-0123-456789abcdef"
HOST_BOOT = "fedcba98-7654-3210-fedc-ba9876543210"
DEVICE_BOOT = "10293847-5647-3829-1029-384756473829"


def save(path, value):
    path.write_text(json.dumps(value, separators=(",", ":"), allow_nan=False) + "\n", encoding="utf-8")


def envelope(stage, monotonic_ns, data, process_id):
    diagnostic = stage in ("trace_started", "trace_stopped")
    return {
        "schema_version": 1,
        "process_id": process_id,
        "operation_id": 0,
        "kind": "diagnostic" if diagnostic else "startup",
        "stage": stage,
        "monotonic_ns": monotonic_ns,
        "thread_cpu_ns": None,
        "thread_id": process_id,
        "trace_elapsed_us": monotonic_ns // 1000,
        "data": data,
    }


def trace_rows(run, process_id, base_ns, elapsed_ns, *, include_send=True, include_driver=True,
               include_render_order=False):
    release_after_ns = base_ns + 62_000_000
    rendered_ns = release_after_ns + elapsed_ns
    rows = [envelope("trace_started", base_ns + 63_000_000, {
        "schema": "kindle-shelf-render-log-v1",
        "run_id": run,
        "boot_id": DEVICE_BOOT,
        "clock": "CLOCK_MONOTONIC",
    }, process_id)]
    rendered = {
        "shelf_state": "ready",
        "render_sequence": 2,
        "rendered_monotonic_ns": rendered_ns,
    }
    if include_render_order:
        rendered["render_order"] = "wait_then_render"
    rows.append(envelope("shelf_rendered", rendered_ns + 1_000, rendered, process_id))
    send_ns = rendered_ns + 20_000_000
    if include_send:
        rows.append(envelope("shelf_submitted", send_ns + 1_000, {
            "shelf_state": "ready",
            "render_sequence": 2,
            "update_marker": 17,
            "submitted_monotonic_ns": send_ns,
        }, process_id))
    if include_send and include_driver:
        driver_ns = send_ns + 365_000_000
        rows.append(envelope("shelf_driver_completed", driver_ns + 1_000, {
            "shelf_state": "ready",
            "render_sequence": 2,
            "update_marker": 17,
            "completed_monotonic_ns": driver_ns,
        }, process_id))
    stop_ns = rows[-1]["monotonic_ns"] + 1_000_000
    rows.append(envelope("trace_stopped", stop_ns, {
        "reason": "finished",
        "truncated": False,
    }, process_id))
    return rows


def write_case(case, index, elapsed_ns=1_000_000_000, *, include_send=True, include_driver=True,
               include_render_order=False, released=True, failed=False):
    run = case["run"]
    path = Path(case["batch"])
    path.mkdir()
    (path / "attempted").touch()
    process_id = 1000 + index
    base_ns = index * 10_000_000_000
    save(path / "launcher-arm.json", ANALYSIS.expected_arm_marker(case))
    save(path / "admission.json", {
        "run": run,
        "owner": OWNER,
        "host_boot": HOST_BOOT,
        "route_bytes": 1,
        "probe_bytes": 1,
    })
    save(path / "origin.json", {"boot": DEVICE_BOOT})
    save(path / "retired.json", {"natural": True, "app_pid": process_id})
    if released:
        save(path / "released.json", {
            "state": "released",
            "owner_thread": OWNER,
            "run_id": run,
            "owner_and_token_checked": True,
            "native_restored_and_connections_closed": True,
            "device_disposition": "original-native-restored",
        })
    if failed:
        save(path / "failure.json", {"phase": "fixture", "reason": "synthetic-failure"})

    click = {
        "clock": "CLOCK_MONOTONIC",
        "target": "open-card",
        "ok": True,
        "mode": "XTest-button-1",
        "hold_requested_ms": 50,
        "down_confirmed": True,
        "buttons_clear_after": True,
        "up_sent": True,
        "down_before_ns": base_ns + 10_000_000,
        "down_after_ns": base_ns + 11_000_000,
        "up_before_ns": base_ns + 61_000_000,
        "up_after_ns": base_ns + 62_000_000,
    }
    card = ANALYSIS.LOG_BOUNDS.project_card_origin(
        click,
        run_id=run,
        boot_id=DEVICE_BOOT,
        process_id=process_id,
    )
    rows = trace_rows(
        run,
        process_id,
        base_ns,
        elapsed_ns,
        include_send=include_send,
        include_driver=include_driver,
        include_render_order=include_render_order,
    )
    save(path / "card.json", card)
    with (path / (run + ".jsonl")).open("w", encoding="utf-8", newline="\n") as output:
        for row in rows:
            output.write(json.dumps(row, separators=(",", ":"), allow_nan=False) + "\n")
    save(path / "intervals.json", ANALYSIS.LOG_BOUNDS.audit(card, rows))


def write_pre_card_failure(case, *, phase="native-navigation", probe_failure=None, post_navigation_marker=None):
    path = Path(case["batch"])
    path.mkdir()
    (path / "attempted").touch()
    save(path / "launcher-arm.json", ANALYSIS.expected_arm_marker(case))
    save(path / "admission.json", {
        "run": case["run"],
        "owner": OWNER,
        "host_boot": HOST_BOOT,
    })
    save(path / "origin.json", {"boot": DEVICE_BOOT})
    save(path / "released.json", {
        "state": "released",
        "owner_thread": OWNER,
        "run_id": case["run"],
        "owner_and_token_checked": True,
        "native_restored_and_connections_closed": True,
        "device_disposition": "original-native-restored",
    })
    save(path / "failure.json", {
        "phase": phase,
        "reason": "Scoped SSH failed (255; withheld)",
    })
    if probe_failure is not None:
        save(path / "probe-failure.json", probe_failure)
    if post_navigation_marker is not None:
        (path / post_navigation_marker).touch()


def make_plan(root):
    cases = []
    for index, (pair, arm) in enumerate(ANALYSIS.SCHEDULE):
        run = "bili-shelf-" + f"{index + 1:032x}"
        cases.append({
            "pair": pair,
            "arm": arm,
            "run": run,
            "batch": str(root / ("bilicomics-interactive-" + run)),
        })
    return {"schema": ANALYSIS.SCHEMA, "owner": OWNER, "host_boot": HOST_BOOT, "cases": cases}


class LibraryAdmissionAnalysisTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="library-admission-analysis-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.plan = make_plan(self.root)

    def write_all(self, *, elapsed=None, options=None):
        elapsed = elapsed or {}
        options = options or {}
        for index, case in enumerate(self.plan["cases"]):
            duration = elapsed.get((case["pair"], case["arm"]), 1_000_000_000)
            write_case(case, index, duration, **options.get((case["pair"], case["arm"]), {}))

    def test_schedule_has_two_excluded_warmups_and_six_alternating_pairs(self):
        self.assertEqual(ANALYSIS.SCHEDULE[:2], [(0, "A"), (0, "B")])
        self.assertEqual(
            [(case["pair"], case["arm"]) for case in self.plan["cases"]],
            ANALYSIS.SCHEDULE,
        )

    def test_paired_median_uses_release_brackets_and_excludes_warmups(self):
        elapsed = {(pair, arm): 1_000_000_000 for pair in range(1, 7) for arm in "AB"}
        for pair in range(1, 6):
            elapsed[(pair, "B")] = 800_000_000
        elapsed[(6, "B")] = 950_000_000
        elapsed[(0, "A")] = 5_000_000_000
        elapsed[(0, "B")] = 6_000_000_000
        self.write_all(elapsed=elapsed)
        result = ANALYSIS.analyze_plan(self.plan)
        summary = result["paired_summary"]["renderer_return"]
        self.assertEqual(summary["n_pairs"], 6)
        self.assertEqual(summary["median_paired_B_minus_A_ns"], [-201_000_000, -199_000_000])
        self.assertTrue(result["primary_threshold"]["passes_predeclared_primary_threshold"])
        self.assertFalse(result["warmups_included_in_primary_statistics"])

    def test_five_faster_pairs_and_exactly_one_hundred_ms_upper_bound_pass(self):
        elapsed = {(pair, arm): 1_000_000_000 for pair in range(1, 7) for arm in "AB"}
        for pair in range(1, 6):
            elapsed[(pair, "B")] = 899_000_000
        self.write_all(elapsed=elapsed)
        result = ANALYSIS.analyze_plan(self.plan)
        self.assertTrue(result["primary_threshold"]["B_faster_in_at_least_five_pairs"])
        self.assertEqual(
            result["paired_summary"]["renderer_return"]["median_paired_B_minus_A_ns"],
            [-102_000_000, -100_000_000],
        )
        self.assertTrue(result["primary_threshold"]["passes_predeclared_primary_threshold"])

    def test_median_interval_overlapping_the_hundred_ms_gate_does_not_pass(self):
        elapsed = {(pair, arm): 1_000_000_000 for pair in range(1, 7) for arm in "AB"}
        for pair in range(1, 6):
            elapsed[(pair, "B")] = 900_000_000
        self.write_all(elapsed=elapsed)
        result = ANALYSIS.analyze_plan(self.plan)
        self.assertEqual(
            result["paired_summary"]["renderer_return"]["median_paired_B_minus_A_ns"],
            [-101_000_000, -99_000_000],
        )
        self.assertFalse(result["primary_threshold"]["passes_predeclared_primary_threshold"])

    def test_four_faster_pairs_do_not_pass(self):
        elapsed = {(pair, arm): 1_000_000_000 for pair in range(1, 7) for arm in "AB"}
        for pair in range(1, 5):
            elapsed[(pair, "B")] = 800_000_000
        self.write_all(elapsed=elapsed)
        result = ANALYSIS.analyze_plan(self.plan)
        self.assertFalse(result["primary_threshold"]["B_faster_in_at_least_five_pairs"])
        self.assertFalse(result["primary_threshold"]["passes_predeclared_primary_threshold"])

    def test_missing_render_order_is_valid_and_not_inferred(self):
        self.write_all()
        result = ANALYSIS.analyze_plan(self.plan)
        self.assertTrue(all(case["status"] == "qualified" for case in result["cases"]))
        self.assertTrue(all(case["render_order"] is None for case in result["cases"]))

    def test_send_and_driver_endpoints_remain_separate_and_optional(self):
        self.write_all(options={(1, "B"): {"include_send": False, "include_driver": False}})
        result = ANALYSIS.analyze_plan(self.plan)
        pair = result["pairs"][0]
        self.assertIsNotNone(pair["endpoints"]["renderer_return"]["B_minus_A_ns"])
        self.assertIsNone(pair["endpoints"]["accepted_send"]["B_minus_A_ns"])
        self.assertIsNone(pair["endpoints"]["driver_completion"]["B_minus_A_ns"])
        self.assertEqual(result["paired_summary"]["renderer_return"]["n_pairs"], 6)
        self.assertEqual(result["paired_summary"]["accepted_send"]["n_pairs"], 5)

    def test_arm_marker_binds_run_arm_strategy_size_and_render_policy(self):
        self.write_all()
        marker_path = Path(self.plan["cases"][-1]["batch"]) / "launcher-arm.json"
        marker = json.loads(marker_path.read_text(encoding="utf-8"))
        marker["parent_admission"] = "worker-only"
        save(marker_path, marker)
        result = ANALYSIS.analyze_plan(self.plan)
        self.assertEqual(result["cases"][-1]["status"], "closed-unqualified")
        self.assertEqual(result["cases"][-1]["reason"], "invalid-closed-evidence")
        self.assertEqual(result["pairs"][-1]["status"], "incomplete")
        self.assertEqual(result["paired_summary"]["renderer_return"]["n_pairs"], 5)

    def test_native_navigation_failure_without_click_or_retirement_is_not_a_qualified_visit(self):
        for index, case in enumerate(self.plan["cases"][:3]):
            write_case(case, index)
        failed = self.plan["cases"][3]
        write_pre_card_failure(failed)
        result = ANALYSIS.analyze_plan(self.plan)
        failure = result["cases"][1]
        self.assertEqual(failure["status"], "pre-click-navigation-failure")
        self.assertEqual(failure["reason"], "native-navigation-failed-before-activation")
        self.assertFalse(failure["native_card_clicked"])
        self.assertFalse(failure["candidate_application_started"])
        self.assertEqual(result["pairs"][0]["status"], "incomplete")
        self.assertEqual(result["paired_summary"]["renderer_return"]["n_pairs"], 0)
        self.assertEqual(result["primary_threshold"]["complete_six_pairs"], False)
        self.assertTrue(all(case["status"] == "not-started" for case in result["cases"][2:]))

    def test_pre_card_failure_with_any_click_or_app_marker_is_not_classified_as_no_app(self):
        for index, case in enumerate(self.plan["cases"][:3]):
            write_case(case, index)
        case = self.plan["cases"][3]
        write_pre_card_failure(case, post_navigation_marker="activation-intended")
        result = ANALYSIS.analyze_plan(self.plan)
        self.assertEqual(result["cases"][1]["status"], "closed-unqualified")
        self.assertEqual(
            result["cases"][1]["reason"],
            "native-navigation-failure-has-post-navigation-evidence",
        )
        self.assertNotIn("candidate_application_started", result["cases"][1])

    def test_fixed_probe_receipt_can_confirm_no_card_request_and_no_candidate_launch(self):
        for index, case in enumerate(self.plan["cases"][:3]):
            write_case(case, index)
        case = self.plan["cases"][3]
        write_pre_card_failure(case, probe_failure={
            "schema": "bili-native-probe-failure-v1",
            "phase": "native-navigation",
            "action": "read-navigation",
            "step": "operation-request",
            "tab_request": "not-requested",
            "card_request": "not-requested",
        })
        result = ANALYSIS.analyze_plan(self.plan, require_probe_failure=True)
        failure = result["cases"][1]
        self.assertEqual(failure["status"], "pre-card-input-not-requested")
        self.assertEqual(failure["native_input_conclusion"], "no-requested-input")
        self.assertFalse(failure["native_card_clicked"])
        self.assertFalse(failure["candidate_application_started"])
        self.assertEqual(result["pairs"][0]["status"], "incomplete")
        self.assertIsNone(result["pairs"][0]["endpoints"]["renderer_return"]["B_minus_A_ns"])
        self.assertEqual(result["paired_summary"]["renderer_return"]["n_pairs"], 0)

    def test_possible_tab_dispatch_stays_ambiguous_but_card_and_app_are_not_started(self):
        for index, case in enumerate(self.plan["cases"][:3]):
            write_case(case, index)
        write_pre_card_failure(self.plan["cases"][3], phase="native-navigation", probe_failure={
            "schema": "bili-native-probe-failure-v1",
            "phase": "native-navigation",
            "action": "request-target-tab",
            "step": "operation-request",
            "tab_request": "dispatch-possible",
            "card_request": "not-requested",
        })
        result = ANALYSIS.analyze_plan(self.plan, require_probe_failure=True)
        failure = result["cases"][1]
        self.assertEqual(failure["native_input_conclusion"], "dispatch-ambiguous")
        self.assertEqual(failure["probe_failure"]["tab_request"], "dispatch-possible")
        self.assertEqual(failure["probe_failure"]["card_request"], "not-requested")
        self.assertFalse(failure["native_card_clicked"])
        self.assertFalse(failure["candidate_application_started"])

    def test_possible_card_dispatch_never_means_not_clicked_or_not_started(self):
        for index, case in enumerate(self.plan["cases"][:3]):
            write_case(case, index)
        write_pre_card_failure(self.plan["cases"][3], phase="card-release", probe_failure={
            "schema": "bili-native-probe-failure-v1",
            "phase": "card-release",
            "action": "open-card",
            "step": "operation-request",
            "tab_request": "not-requested",
            "card_request": "dispatch-possible",
        })
        result = ANALYSIS.analyze_plan(self.plan, require_probe_failure=True)
        failure = result["cases"][1]
        self.assertEqual(failure["status"], "closed-unqualified")
        self.assertEqual(failure["reason"], "card-input-dispatch-or-launch-unresolved")
        self.assertIsNone(failure["native_card_clicked"])
        self.assertIsNone(failure["candidate_application_started"])

    def test_confirmed_card_receipt_without_origin_is_not_a_timing_sample(self):
        for index, case in enumerate(self.plan["cases"][:3]):
            write_case(case, index)
        write_pre_card_failure(self.plan["cases"][3], phase="card-release", probe_failure={
            "schema": "bili-native-probe-failure-v1",
            "phase": "card-release",
            "action": "open-card",
            "step": "receipt-accepted",
            "tab_request": "not-requested",
            "card_request": "receipt-confirmed",
        })
        result = ANALYSIS.analyze_plan(self.plan, require_probe_failure=True)
        failure = result["cases"][1]
        self.assertEqual(failure["reason"], "card-input-receipt-without-qualified-render")
        self.assertTrue(failure["native_card_clicked"])
        self.assertIsNone(failure["candidate_application_started"])
        self.assertIsNone(failure["observed_endpoints_ns"]["renderer_return"])
        self.assertEqual(result["paired_summary"]["renderer_return"]["n_pairs"], 0)

    def test_required_probe_receipt_missing_or_invalid_is_never_reclassified_as_no_input(self):
        for index, case in enumerate(self.plan["cases"][:3]):
            write_case(case, index)
        failed = self.plan["cases"][3]
        write_pre_card_failure(failed)
        result = ANALYSIS.analyze_plan(self.plan, require_probe_failure=True)
        self.assertEqual(result["cases"][1]["reason"], "probe-failure-receipt-missing")
        self.assertNotIn("native_card_clicked", result["cases"][1])
        receipt = Path(failed["batch"]) / "probe-failure.json"
        save(receipt, {
            "schema": "bili-native-probe-failure-v1",
            "phase": "native-navigation",
            "action": "open-card",
            "step": "operation-request",
            "tab_request": "not-requested",
            "card_request": "not-requested",
        })
        result = ANALYSIS.analyze_plan(self.plan, require_probe_failure=True)
        self.assertEqual(result["cases"][1]["reason"], "invalid-probe-failure-receipt")
        self.assertNotIn("candidate_application_started", result["cases"][1])

    def test_missing_retirement_without_an_explicit_pre_card_failure_is_not_inferred(self):
        case = self.plan["cases"][0]
        path = Path(case["batch"])
        path.mkdir()
        (path / "attempted").touch()
        save(path / "launcher-arm.json", ANALYSIS.expected_arm_marker(case))
        save(path / "admission.json", {"run": case["run"], "owner": OWNER, "host_boot": HOST_BOOT})
        save(path / "origin.json", {"boot": DEVICE_BOOT})
        save(path / "released.json", {
            "state": "released",
            "owner_thread": OWNER,
            "run_id": case["run"],
            "owner_and_token_checked": True,
            "native_restored_and_connections_closed": True,
            "device_disposition": "original-native-restored",
        })
        result = ANALYSIS.analyze_plan(self.plan)
        self.assertEqual(result["warmups"][0]["status"], "closed-unqualified")
        self.assertEqual(result["warmups"][0]["reason"], "invalid-closed-evidence")

    def test_attempt_without_verified_release_fails_closed(self):
        write_case(self.plan["cases"][0], 0, released=False)
        with self.assertRaisesRegex(ANALYSIS.AnalysisError, "lacks a verified release"):
            ANALYSIS.analyze_plan(self.plan)

    def test_later_attempt_after_invalid_visit_is_rejected(self):
        self.write_all()
        bad_marker = Path(self.plan["cases"][2]["batch"]) / "launcher-arm.json"
        bad_marker.unlink()
        with self.assertRaisesRegex(ANALYSIS.AnalysisError, "later visit was attempted"):
            ANALYSIS.analyze_plan(self.plan)

    def test_changed_device_boot_breaks_visit_chronology(self):
        self.write_all()
        origin_path = Path(self.plan["cases"][3]["batch"]) / "origin.json"
        save(origin_path, {"boot": "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"})
        card_path = Path(self.plan["cases"][3]["batch"]) / "card.json"
        card = json.loads(card_path.read_text(encoding="utf-8"))
        card["boot_id"] = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"
        save(card_path, card)
        run = self.plan["cases"][3]["run"]
        trace_path = Path(self.plan["cases"][3]["batch"]) / (run + ".jsonl")
        rows = ANALYSIS.read_jsonl(trace_path)
        rows[0]["data"]["boot_id"] = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"
        with trace_path.open("w", encoding="utf-8", newline="\n") as output:
            for row in rows:
                output.write(json.dumps(row, separators=(",", ":"), allow_nan=False) + "\n")
        save(Path(self.plan["cases"][3]["batch"]) / "intervals.json",
             ANALYSIS.LOG_BOUNDS.audit(card, rows))
        with self.assertRaisesRegex(ANALYSIS.AnalysisError, "one device boot"):
            ANALYSIS.analyze_plan(self.plan)

    def test_plan_rejects_reordered_or_replaced_cases(self):
        self.plan["cases"][3], self.plan["cases"][4] = self.plan["cases"][4], self.plan["cases"][3]
        with self.assertRaisesRegex(ANALYSIS.AnalysisError, "predeclared"):
            ANALYSIS.validate_plan(self.plan)


if __name__ == "__main__":
    unittest.main()
