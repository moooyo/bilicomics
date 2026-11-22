"""Synthetic records only; no device, private logs, or secrets are involved."""

import copy
import unittest

from log_bounds import AuditError, audit, narrow_rows, project_card_origin


RUN_ID = "bili-shelf-0123456789abcdef0123456789abcdef"
BOOT_ID = "01234567-89ab-cdef-0123-456789abcdef"
PID = 53


def click():
    return {
        "clock": "CLOCK_MONOTONIC",
        "target": "open-card",
        "ok": True,
        "mode": "XTest-button-1",
        "hold_requested_ms": 50,
        "down_confirmed": True,
        "buttons_clear_after": True,
        "up_sent": True,
        "down_before_ns": 10_000_000,
        "down_after_ns": 11_000_000,
        "up_before_ns": 61_000_000,
        "up_after_ns": 62_000_000,
    }


def origin(**changes):
    raw = click()
    raw.update(changes.pop("click_changes", {}))
    return project_card_origin(raw, run_id=RUN_ID, boot_id=BOOT_ID, process_id=PID)


def envelope(stage, monotonic_ns, data, *, process_id=PID, kind=None, operation_id=0):
    return {
        "schema_version": 1,
        "process_id": process_id,
        "operation_id": operation_id,
        "kind": kind or ("diagnostic" if stage in ("trace_started", "trace_stopped") else "startup"),
        "stage": stage,
        "monotonic_ns": monotonic_ns,
        "thread_cpu_ns": None,
        "thread_id": process_id,
        "trace_elapsed_us": monotonic_ns // 1000,
        "data": data,
    }


def trace(*, include_render=True, include_send=True, include_driver=True,
          render_order="wait_then_render"):
    rows = [envelope("trace_started", 63_000_000, {
        "schema": "kindle-shelf-render-log-v1",
        "run_id": RUN_ID,
        "boot_id": BOOT_ID,
        "clock": "CLOCK_MONOTONIC",
    })]
    if include_render:
        rendered = {
            "shelf_state": "ready",
            "render_sequence": 2,
            "rendered_monotonic_ns": 100_000_000,
        }
        if render_order is not None:
            rendered["render_order"] = render_order
        rows.append(envelope("shelf_rendered", 101_000_000, rendered))
    if include_send:
        rows.append(envelope("shelf_submitted", 111_000_000, {
            "shelf_state": "ready",
            "render_sequence": 2,
            "update_marker": 8,
            "submitted_monotonic_ns": 110_000_000,
        }))
    if include_driver:
        rows.append(envelope("shelf_driver_completed", 121_000_000, {
            "shelf_state": "ready",
            "render_sequence": 2,
            "update_marker": 8,
            "completed_monotonic_ns": 120_000_000,
        }))
    rows.append(envelope("trace_stopped", 130_000_000, {
        "reason": "finished",
        "truncated": False,
    }))
    return rows


class LogBoundsTests(unittest.TestCase):
    def test_projects_only_bounded_click_data_and_explicit_run_identity(self):
        raw = click()
        raw["touch_coordinates"] = [12, 34]
        projected = project_card_origin(raw, run_id=RUN_ID, boot_id=BOOT_ID, process_id=PID)
        self.assertEqual(projected["run_id"], RUN_ID)
        self.assertEqual(projected["boot_id"], BOOT_ID)
        self.assertEqual(projected["process_id"], PID)
        self.assertEqual(projected["click"]["up_before_ns"], raw["up_before_ns"])
        self.assertNotIn("touch_coordinates", projected["click"])

    def test_reports_distinct_release_bracket_intervals(self):
        result = audit(origin(), trace())
        self.assertEqual(result["status"], "observed")
        self.assertEqual(result["card_release_bracket_ns"], [61_000_000, 62_000_000])
        self.assertEqual(result["renderer_return_interval_ns"], [38_000_000, 39_000_000])
        self.assertEqual(result["accepted_send_interval_ns"], [48_000_000, 49_000_000])
        self.assertEqual(result["driver_completion_interval_ns"], [58_000_000, 59_000_000])
        self.assertEqual(result["render_order"], "wait_then_render")
        self.assertFalse(result["interactive_response_claimed"])
        self.assertFalse(result["optical_completion_claimed"])

    def test_reads_each_known_render_order_for_arm_matching(self):
        for render_order in ("wait_then_render", "render_then_wait", "initial_fill"):
            with self.subTest(render_order=render_order):
                result = audit(origin(), trace(render_order=render_order))
                self.assertEqual(result["render_order"], render_order)

    def test_legacy_trace_without_render_order_remains_readable(self):
        rows = trace(render_order=None)
        result = audit(origin(), rows)
        projected = narrow_rows(rows, RUN_ID, BOOT_ID, process_id=PID)
        self.assertIsNone(result["render_order"])
        self.assertNotIn("render_order", projected[1]["data"])

    def test_rejects_unknown_render_order_during_audit_and_projection(self):
        rows = trace()
        rows[1]["data"]["render_order"] = "wait_then_wait"
        with self.assertRaises(AuditError):
            audit(origin(), rows)
        with self.assertRaises(AuditError):
            narrow_rows(rows, RUN_ID, BOOT_ID, process_id=PID)

    def test_renderer_only_is_reported_without_claiming_an_accepted_frame(self):
        result = audit(origin(), trace(include_send=False, include_driver=False))
        self.assertEqual(result["status"], "render-only")
        self.assertEqual(result["renderer_return_interval_ns"], [38_000_000, 39_000_000])
        self.assertIsNone(result["accepted_send_interval_ns"])
        self.assertIsNone(result["driver_completion_interval_ns"])
        self.assertIsNone(result["landmarks_ns"]["shelf_submitted"])

    def test_trace_without_a_qualified_frame_reports_no_software_endpoint(self):
        result = audit(origin(), trace(include_render=False, include_send=False, include_driver=False))
        self.assertEqual(result["status"], "no-qualified-frame")
        self.assertIsNone(result["render_order"])
        self.assertIsNone(result["renderer_return_interval_ns"])
        self.assertIsNone(result["accepted_send_interval_ns"])
        self.assertIsNone(result["driver_completion_interval_ns"])
        self.assertEqual(result["landmarks_ns"], {
            "shelf_rendered": None,
            "shelf_submitted": None,
            "shelf_driver_completed": None,
        })
        self.assertFalse(result["interactive_response_claimed"])
        self.assertFalse(result["optical_completion_claimed"])

    def test_accepted_send_fallback_does_not_invent_renderer_return_time(self):
        result = audit(origin(), trace(include_render=False, include_driver=False))
        self.assertEqual(result["status"], "observed")
        self.assertIsNone(result["render_order"])
        self.assertIsNone(result["renderer_return_interval_ns"])
        self.assertEqual(result["accepted_send_interval_ns"], [48_000_000, 49_000_000])
        self.assertIsNone(result["driver_completion_interval_ns"])

    def test_driver_receipt_is_optional_but_must_match_the_submitted_marker(self):
        result = audit(origin(), trace(include_driver=False))
        self.assertIsNone(result["driver_completion_interval_ns"])
        rows = trace()
        rows[-2]["data"]["update_marker"] = 9
        with self.assertRaises(AuditError):
            audit(origin(), rows)

    def test_rejects_mismatched_run_boot_or_process(self):
        rows = trace()
        rows[0]["data"]["run_id"] = "bili-shelf-ffffffffffffffffffffffffffffffff"
        with self.assertRaises(AuditError):
            audit(origin(), rows)
        rows = trace()
        rows[0]["data"]["boot_id"] = "fedcba98-7654-3210-fedc-ba9876543210"
        with self.assertRaises(AuditError):
            audit(origin(), rows)
        rows = trace()
        rows[1]["process_id"] = PID + 1
        with self.assertRaises(AuditError):
            audit(origin(), rows)

    def test_rejects_unknown_clock_domain_or_invalid_card_click(self):
        rows = trace()
        rows[0]["data"]["clock"] = "CLOCK_BOOTTIME"
        with self.assertRaises(AuditError):
            audit(origin(), rows)
        for changes in ({"clock": "CLOCK_BOOTTIME"}, {"target": "other-card"}, {"ok": False},
                        {"mode": "script"}, {"buttons_clear_after": False},
                        {"up_after_ns": 60_000_000}):
            with self.subTest(changes=changes):
                with self.assertRaises(AuditError):
                    projected = project_card_origin(click() | changes, run_id=RUN_ID, boot_id=BOOT_ID, process_id=PID)
                    audit(projected, trace())

    def test_rejects_wrong_stage_order_missing_events_or_truncation(self):
        rows = trace()
        rows[1], rows[2] = rows[2], rows[1]
        with self.assertRaises(AuditError):
            audit(origin(), rows)
        rows = trace()
        rows.pop(2)
        with self.assertRaises(AuditError):
            audit(origin(), rows)
        rows = trace()
        rows[-1]["data"]["truncated"] = True
        with self.assertRaises(AuditError):
            audit(origin(), rows)

    def test_rejects_mismatched_frame_sequence_marker_or_timestamp(self):
        changes = (
            (2, "render_sequence", 3),
            (2, "update_marker", 0),
            (2, "submitted_monotonic_ns", 99_000_000),
            (3, "completed_monotonic_ns", 109_000_000),
        )
        for index, field, value in changes:
            with self.subTest(index=index, field=field):
                rows = trace()
                rows[index]["data"][field] = value
                with self.assertRaises(AuditError):
                    audit(origin(), rows)

    def test_rejects_extra_private_fields_and_unknown_events(self):
        rows = trace()
        rows[1]["data"]["book_title"] = "private"
        with self.assertRaises(AuditError):
            audit(origin(), rows)
        rows = trace()
        rows.insert(-1, copy.deepcopy(rows[1]))
        rows[-2]["stage"] = "menu_opened"
        with self.assertRaises(AuditError):
            audit(origin(), rows)

    def test_rejects_duplicate_trace_boundary(self):
        rows = trace()
        rows.append(copy.deepcopy(rows[-1]))
        with self.assertRaises(AuditError):
            audit(origin(), rows)


if __name__ == "__main__":
    unittest.main()
