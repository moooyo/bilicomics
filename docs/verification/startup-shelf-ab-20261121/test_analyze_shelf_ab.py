"""Synthetic evidence only; these tests never access a device or private logs."""

import importlib.util
import json
from pathlib import Path
import tempfile
import unittest


HERE = Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location("analyze_shelf_ab", HERE / "analyze_shelf_ab.py")
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


def trace_rows(run, process_id, base_ns, elapsed_ns, order, *, include_send=True, include_driver=True):
    release_after_ns = base_ns + 62_000_000
    rendered_ns = release_after_ns + elapsed_ns
    rows = [envelope("trace_started", base_ns + 63_000_000, {
        "schema": "kindle-shelf-render-log-v1",
        "run_id": run,
        "boot_id": DEVICE_BOOT,
        "clock": "CLOCK_MONOTONIC",
    }, process_id)]
    rows.append(envelope("shelf_rendered", rendered_ns + 1_000, {
        "shelf_state": "ready",
        "render_sequence": 2,
        "render_order": order,
        "rendered_monotonic_ns": rendered_ns,
    }, process_id))
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


def write_case(root, case, index, elapsed_ns=1_000_000_000, order=None, *,
               include_send=True, include_driver=True, failed=False, released=True):
    run = case["run"]
    path = Path(case["batch"])
    path.mkdir()
    (path / "attempted").touch()
    pid = 1000 + index
    base_ns = index * 10_000_000_000
    order = order or ("wait_then_render" if case["arm"] == "A" else "render_then_wait")
    save(path / "admission.json", {
        "run": run,
        "owner": OWNER,
        "host_boot": HOST_BOOT,
        "route_bytes": 1,
        "probe_bytes": 1,
    })
    save(path / "origin.json", {"boot": DEVICE_BOOT})
    save(path / "retired.json", {"natural": True, "app_pid": pid})
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
        process_id=pid,
    )
    rows = trace_rows(
        run,
        pid,
        base_ns,
        elapsed_ns,
        order,
        include_send=include_send,
        include_driver=include_driver,
    )
    save(path / "card.json", card)
    with (path / (run + ".jsonl")).open("w", encoding="utf-8", newline="\n") as output:
        for row in rows:
            output.write(json.dumps(row, separators=(",", ":"), allow_nan=False) + "\n")
    audited = ANALYSIS.LOG_BOUNDS.audit(card, rows)
    save(path / "intervals.json", audited)


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
    return {"schema": "bili-shelf-ab-v1", "owner": OWNER, "host_boot": HOST_BOOT, "cases": cases}


class ShelfAbAnalysisTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="shelf-ab-analysis-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.plan = make_plan(self.root)

    def write_all(self, *, measured_ns=None, overrides=None):
        measured_ns = measured_ns or {}
        overrides = overrides or {}
        for index, case in enumerate(self.plan["cases"]):
            elapsed = measured_ns.get((case["pair"], case["arm"]), 1_000_000_000)
            options = overrides.get((case["pair"], case["arm"]), {})
            write_case(self.root, case, index, elapsed, **options)

    def test_schedule_has_two_excluded_warmups_and_six_alternating_pairs(self):
        self.assertEqual(ANALYSIS.SCHEDULE[:2], [(0, "A"), (0, "B")])
        self.assertEqual(
            [tuple(case.values())[:2] for case in self.plan["cases"]],
            ANALYSIS.SCHEDULE,
        )

    def test_uses_median_paired_difference_and_excludes_warmups(self):
        arm_times = {
            (1, "A"): 1_000_000_000,
            (2, "A"): 1_000_000_000,
            (3, "A"): 1_001_000_000,
            (4, "A"): 1_001_000_000,
            (5, "A"): 1_010_000_000,
            (6, "A"): 1_010_000_000,
            (1, "B"): 1_000_000_000,
            (2, "B"): 1_010_000_000,
            (3, "B"): 1_010_000_000,
            (4, "B"): 1_010_000_000,
            (5, "B"): 1_010_000_000,
            (6, "B"): 1_010_000_000,
            (0, "A"): 5_000_000_000,
            (0, "B"): 6_000_000_000,
        }
        self.write_all(measured_ns=arm_times)
        result = ANALYSIS.analyze_plan(self.plan)
        summary = result["nominal_intention_to_treat"]["renderer_return"]
        self.assertEqual(summary["n_pairs"], 6)
        self.assertEqual(summary["median_paired_B_minus_A_ns"], [3_500_000, 5_500_000])
        self.assertNotEqual(summary["median_paired_B_minus_A_ns"], [-9_000_000, -9_000_000])
        self.assertFalse(result["warmups_included_in_primary_statistics"])

    def test_b_wait_first_fallback_stays_in_nominal_result(self):
        self.write_all(overrides={(1, "B"): {"order": "wait_then_render"}})
        result = ANALYSIS.analyze_plan(self.plan)
        pair = result["pairs"][0]
        self.assertEqual(pair["status"], "complete")
        self.assertFalse(pair["actual_order_qualified"])
        self.assertEqual(result["nominal_intention_to_treat"]["renderer_return"]["n_pairs"], 6)
        self.assertEqual(result["actual_order_qualified_pairs_descriptive_only"]["renderer_return"]["n_pairs"], 5)
        self.assertEqual(result["actual_order_counts"]["B_wait_first_fallback"], 1)

    def test_initial_fill_is_invalid_and_stops_before_later_attempts(self):
        for index, case in enumerate(self.plan["cases"][:4]):
            order = "initial_fill" if (case["pair"], case["arm"]) == (1, "B") else None
            write_case(self.root, case, index, order=order)
        result = ANALYSIS.analyze_plan(self.plan)
        self.assertEqual(result["cases"][1]["status"], "closed-unqualified")
        self.assertEqual(result["cases"][1]["render_order"], "initial_fill")
        self.assertEqual(result["nominal_intention_to_treat"]["renderer_return"]["n_pairs"], 0)

        later = self.plan["cases"][4]
        write_case(self.root, later, 4)
        with self.assertRaisesRegex(ANALYSIS.AnalysisError, "later visit was attempted"):
            ANALYSIS.analyze_plan(self.plan)

    def test_send_and_driver_endpoints_remain_separate_and_optional(self):
        self.write_all(overrides={(1, "B"): {"include_send": False, "include_driver": False}})
        result = ANALYSIS.analyze_plan(self.plan)
        pair = result["pairs"][0]
        self.assertIsNotNone(pair["endpoints"]["renderer_return"]["B_minus_A_ns"])
        self.assertIsNone(pair["endpoints"]["accepted_send"]["B_minus_A_ns"])
        self.assertIsNone(pair["endpoints"]["driver_completion"]["B_minus_A_ns"])
        self.assertEqual(result["nominal_intention_to_treat"]["renderer_return"]["n_pairs"], 6)
        self.assertEqual(result["nominal_intention_to_treat"]["accepted_send"]["n_pairs"], 5)

    def test_only_consecutive_completed_pairs_enter_the_summary(self):
        for index, case in enumerate(self.plan["cases"][:10]):
            write_case(self.root, case, index)
        result = ANALYSIS.analyze_plan(self.plan)
        self.assertEqual(result["nominal_intention_to_treat"]["renderer_return"]["n_pairs"], 4)
        self.assertTrue(all(case["status"] == "not-started" for case in result["cases"][8:]))
        self.assertIsNone(result["primary_threshold"]["passes_predeclared_primary_threshold"])

    def test_attempt_without_verified_release_fails_closed(self):
        case = self.plan["cases"][0]
        write_case(self.root, case, 0, released=False)
        with self.assertRaisesRegex(ANALYSIS.AnalysisError, "lacks a verified release"):
            ANALYSIS.analyze_plan(self.plan)

    def test_plan_rejects_reordered_or_replaced_cases(self):
        self.plan["cases"][3], self.plan["cases"][4] = self.plan["cases"][4], self.plan["cases"][3]
        with self.assertRaisesRegex(ANALYSIS.AnalysisError, "predeclared"):
            ANALYSIS.validate_plan(self.plan)


if __name__ == "__main__":
    unittest.main()
