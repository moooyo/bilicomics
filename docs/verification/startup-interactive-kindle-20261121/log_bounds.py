"""Project one native card click and audit its first qualified Shelf frame."""

import argparse
import json
from pathlib import Path
import re


CLOCK = "CLOCK_MONOTONIC"
ORIGIN_SCHEMA = "kindle-shelf-render-origin-v1"
TRACE_SCHEMA = "kindle-shelf-render-log-v1"
RUN_ID = re.compile(r"bili-shelf-[0-9a-f]{32}\Z")
BOOT_ID = re.compile(r"[0-9a-f]{8}-(?:[0-9a-f]{4}-){3}[0-9a-f]{12}\Z")
CLICK_FIELDS = {
    "clock",
    "target",
    "ok",
    "mode",
    "hold_requested_ms",
    "down_confirmed",
    "buttons_clear_after",
    "up_sent",
    "down_before_ns",
    "down_after_ns",
    "up_before_ns",
    "up_after_ns",
}
ORIGIN_FIELDS = {"schema", "run_id", "boot_id", "process_id", "click"}
ENVELOPE_FIELDS = {
    "schema_version",
    "process_id",
    "operation_id",
    "kind",
    "stage",
    "monotonic_ns",
    "thread_cpu_ns",
    "thread_id",
    "trace_elapsed_us",
    "data",
}
TRACE_DATA_FIELDS = {
    "trace_started": {"schema", "run_id", "boot_id", "clock"},
    "shelf_rendered": {"shelf_state", "render_sequence", "rendered_monotonic_ns"},
    "shelf_submitted": {"shelf_state", "render_sequence", "update_marker", "submitted_monotonic_ns"},
    "shelf_driver_completed": {"shelf_state", "render_sequence", "update_marker", "completed_monotonic_ns"},
    "trace_stopped": {"reason", "truncated"},
}
SHELF_RENDERED_ORDER_FIELDS = TRACE_DATA_FIELDS["shelf_rendered"] | {"render_order"}
RENDER_ORDERS = {"render_then_wait", "wait_then_render", "initial_fill"}
TRACE_STOP_REASONS = {
    "finished",
    "byte_limit",
    "record_too_large",
    "serialization_error",
    "io_error",
    "operation_ids_exhausted",
    "poisoned",
}


class AuditError(ValueError):
    """Raised when the supplied evidence cannot support the requested intervals."""


def require(condition, message):
    if not condition:
        raise AuditError(message)


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        require(key not in result, f"Duplicate JSON key: {key}")
        result[key] = value
    return result


def reject_constant(value):
    raise AuditError(f"Invalid JSON number: {value}")


def read_json(path):
    return json.loads(
        Path(path).read_text(encoding="utf-8"),
        object_pairs_hook=unique_object,
        parse_constant=reject_constant,
    )


def read_jsonl(path):
    lines = Path(path).read_text(encoding="utf-8").splitlines()
    require(lines and all(line.strip() for line in lines), "Trace is empty or contains an empty line")
    return [
        json.loads(line, object_pairs_hook=unique_object, parse_constant=reject_constant)
        for line in lines
    ]


def exact_keys(value, expected, label):
    require(type(value) is dict, f"{label} must be a JSON object")
    actual = set(value)
    missing, extra = expected - actual, actual - expected
    require(not missing, f"{label} is missing fields: {', '.join(sorted(missing))}")
    require(not extra, f"{label} has unsupported fields: {', '.join(sorted(extra))}")


def integer(value, label, *, positive=False):
    require(type(value) is int, f"{label} must be an integer")
    require(value > 0 if positive else value >= 0, f"Invalid {label}")
    return value


def identities(run_id, boot_id, process_id):
    require(type(run_id) is str and RUN_ID.fullmatch(run_id) is not None, "Invalid run ID")
    require(type(boot_id) is str and BOOT_ID.fullmatch(boot_id) is not None, "Invalid device boot ID")
    integer(process_id, "application process ID", positive=True)
    return run_id, boot_id, process_id


def project_card_origin(click, *, run_id, boot_id, process_id):
    """Return a content-free origin projection for the physical host controller."""
    identities(run_id, boot_id, process_id)
    require(type(click) is dict, "Raw card click must be a JSON object")
    missing = CLICK_FIELDS - set(click)
    require(not missing, f"Raw card click is missing fields: {', '.join(sorted(missing))}")
    projected_click = {field: click[field] for field in CLICK_FIELDS}
    validate_click(projected_click)
    return {
        "schema": ORIGIN_SCHEMA,
        "run_id": run_id,
        "boot_id": boot_id,
        "process_id": process_id,
        "click": projected_click,
    }


def validate_click(click):
    exact_keys(click, CLICK_FIELDS, "Card click")
    require(click["clock"] == CLOCK and click["target"] == "open-card" and click["ok"] is True,
            "Card click is not a successful CLOCK_MONOTONIC open-card action")
    require(click["mode"] == "XTest-button-1" and type(click["hold_requested_ms"]) is int and
            click["hold_requested_ms"] == 50 and
            click["down_confirmed"] is True and click["buttons_clear_after"] is True and
            click["up_sent"] is True, "Card click did not complete the expected XTest button cycle")
    down_before = integer(click["down_before_ns"], "button-down lower bound", positive=True)
    down_after = integer(click["down_after_ns"], "button-down upper bound", positive=True)
    up_before = integer(click["up_before_ns"], "button-up lower bound", positive=True)
    up_after = integer(click["up_after_ns"], "button-up upper bound", positive=True)
    require(down_before <= down_after <= up_before - 50_000_000 and
            up_before <= up_after,
            "Invalid or incomplete XTest release bracket")
    return up_before, up_after


def trace_row(row, label):
    exact_keys(row, ENVELOPE_FIELDS, label)
    require(type(row["schema_version"]) is int and row["schema_version"] == 1,
            f"Unsupported {label} schema version")
    process_id = integer(row["process_id"], f"{label} process ID", positive=True)
    integer(row["operation_id"], f"{label} operation ID")
    monotonic_ns = integer(row["monotonic_ns"], f"{label} CLOCK_MONOTONIC timestamp", positive=True)
    require(row["thread_cpu_ns"] is None or
            (type(row["thread_cpu_ns"]) is int and row["thread_cpu_ns"] >= 0),
            f"Invalid {label} thread CPU timestamp")
    require(row["thread_id"] is None or
            (type(row["thread_id"]) is int and row["thread_id"] > 0),
            f"Invalid {label} thread ID")
    integer(row["trace_elapsed_us"], f"{label} trace elapsed time")
    require(type(row["data"]) is dict, f"{label} data must be an object")
    return process_id, monotonic_ns


def trace_header(row):
    header_fields = {"schema", "run_id", "boot_id", "clock"}
    exact_keys(row["data"], header_fields, "Shelf render trace header")
    require(row["data"]["schema"] == TRACE_SCHEMA and row["data"]["clock"] == CLOCK,
            "Unexpected Shelf render trace schema or clock")
    run_id, boot_id = row["data"]["run_id"], row["data"]["boot_id"]
    require(type(run_id) is str and RUN_ID.fullmatch(run_id) is not None, "Invalid trace run ID")
    require(type(boot_id) is str and BOOT_ID.fullmatch(boot_id) is not None, "Invalid trace boot ID")
    return run_id, boot_id


def interval(endpoint_ns, release_before_ns, release_after_ns, label):
    integer(endpoint_ns, label, positive=True)
    require(endpoint_ns >= release_after_ns, f"{label} predates the confirmed XTest release")
    return [endpoint_ns - release_after_ns, endpoint_ns - release_before_ns]


def event_data(row, expected, label):
    exact_keys(row["data"], expected, label)
    return row["data"]


def trace_data(stage, data, label):
    require(type(data) is dict, f"{label} must be a JSON object")
    if stage == "shelf_rendered" and set(data) == SHELF_RENDERED_ORDER_FIELDS:
        require(type(data["render_order"]) is str and data["render_order"] in RENDER_ORDERS,
                "Unsupported Shelf render order")
    else:
        exact_keys(data, TRACE_DATA_FIELDS[stage], label)
    return data


def narrow_rows(rows, run_id, boot_id, process_id=None):
    """Project an approved trace through fixed payload allowlists without timing analysis."""
    require(type(run_id) is str and RUN_ID.fullmatch(run_id) is not None, "Invalid projection run ID")
    require(type(boot_id) is str and BOOT_ID.fullmatch(boot_id) is not None, "Invalid projection boot ID")
    expected_pid = None if process_id is None else integer(process_id, "bound application PID", positive=True)
    require(type(rows) is list and rows, "Shelf render trace is empty")
    projected = []
    seen_pid = None
    header_count = 0
    for index, row in enumerate(rows, 1):
        pid, _ = trace_row(row, f"Trace record {index}")
        if seen_pid is None:
            seen_pid = pid
        require(pid == seen_pid and (expected_pid is None or pid == expected_pid),
                "Shelf render trace does not match one bound application process")
        stage = row["stage"]
        require(stage in TRACE_DATA_FIELDS, f"Unsupported Shelf render trace stage: {stage}")
        trace_data(stage, row["data"], f"{stage} payload")
        if stage == "trace_started":
            header_count += 1
            require(row["kind"] == "diagnostic" and row["operation_id"] == 0 and
                    trace_header(row) == (run_id, boot_id),
                    "Shelf render trace does not match the controlled run and device boot")
        elif stage == "trace_stopped":
            require(row["kind"] == "diagnostic" and row["operation_id"] == 0 and
                    row["data"]["reason"] in TRACE_STOP_REASONS and
                    type(row["data"]["truncated"]) is bool,
                    "Invalid Shelf render trace footer")
        else:
            require(row["kind"] == "startup" and row["operation_id"] == 0,
                    "Shelf render trace event has an unexpected operation identity")
            require(row["data"]["shelf_state"] in ("ready", "empty", "signed_out"),
                    "Shelf render trace event has an unsupported Shelf state")
            integer(row["data"]["render_sequence"], f"{stage} render sequence", positive=True)
            timestamp_key = {
                "shelf_rendered": "rendered_monotonic_ns",
                "shelf_submitted": "submitted_monotonic_ns",
                "shelf_driver_completed": "completed_monotonic_ns",
            }[stage]
            integer(row["data"][timestamp_key], f"{stage} timestamp", positive=True)
            if stage != "shelf_rendered":
                integer(row["data"]["update_marker"], f"{stage} update marker", positive=True)
        projected.append({
            "schema_version": row["schema_version"],
            "process_id": pid,
            "operation_id": row["operation_id"],
            "kind": row["kind"],
            "stage": stage,
            "monotonic_ns": row["monotonic_ns"],
            "thread_cpu_ns": row["thread_cpu_ns"],
            "thread_id": row["thread_id"],
            "trace_elapsed_us": row["trace_elapsed_us"],
            "data": dict(row["data"]),
        })
    require(header_count == 1, "Shelf render trace must have exactly one run-bound header")
    return projected


def audit(card_origin, rows):
    exact_keys(card_origin, ORIGIN_FIELDS, "Card origin projection")
    require(card_origin["schema"] == ORIGIN_SCHEMA, "Unexpected card origin projection schema")
    run_id, boot_id, process_id = identities(
        card_origin["run_id"], card_origin["boot_id"], card_origin["process_id"]
    )
    exact_keys(card_origin["click"], CLICK_FIELDS, "Card click")
    release_before_ns, release_after_ns = validate_click(card_origin["click"])
    require(type(rows) is list and rows, "Shelf render trace is empty")
    checked = [trace_row(row, f"Trace record {index}") for index, row in enumerate(rows, 1)]
    require({pid for pid, _ in checked} == {process_id},
            "Shelf render trace does not match the bound application process")
    require(rows[0]["stage"] == "trace_started" and rows[-1]["stage"] == "trace_stopped",
            "Shelf render trace is missing or misorders its boundaries")
    header, footer = rows[0], rows[-1]
    require(header["kind"] == footer["kind"] == "diagnostic" and
            header["operation_id"] == footer["operation_id"] == 0,
            "Shelf render trace boundaries have invalid identity")
    trace_run_id, trace_boot_id = trace_header(header)
    require((trace_run_id, trace_boot_id) == (run_id, boot_id),
            "Shelf render trace does not match the controlled run and device boot")
    exact_keys(footer["data"], {"reason", "truncated"}, "Shelf render trace footer")
    require(footer["data"]["reason"] == "finished" and footer["data"]["truncated"] is False,
            "Shelf render trace is incomplete or truncated")

    stages = [row["stage"] for row in rows[1:-1]]
    require(stages in (
        [],
        ["shelf_rendered"],
        ["shelf_submitted"],
        ["shelf_rendered", "shelf_submitted"],
        ["shelf_submitted", "shelf_driver_completed"],
        ["shelf_rendered", "shelf_submitted", "shelf_driver_completed"],
    ), "Unexpected or incomplete Shelf render event sequence")
    events = rows[1:-1]
    require(all(row["kind"] == "startup" and row["operation_id"] == 0 for row in events),
            "Shelf render event has an unexpected operation identity")
    trace_start_ns, trace_end_ns = checked[0][1], checked[-1][1]
    require(trace_start_ns >= release_before_ns and trace_end_ns >= trace_start_ns and
            all(trace_start_ns <= row["monotonic_ns"] <= trace_end_ns for row in events),
            "Shelf render events fall outside the controlled launch trace")
    require(all(first["monotonic_ns"] <= second["monotonic_ns"]
                for first, second in zip(rows, rows[1:])),
            "Shelf render trace records are not in monotonic order")

    rendered = None
    submitted = None
    driver = None
    for row in events:
        if row["stage"] == "shelf_rendered":
            data = trace_data("shelf_rendered", row["data"], "Shelf renderer event")
            rendered = {
                "shelf_state": data["shelf_state"],
                "render_sequence": integer(data["render_sequence"], "Shelf render sequence", positive=True),
                "render_order": data.get("render_order"),
                "timestamp_ns": integer(data["rendered_monotonic_ns"], "Shelf renderer return", positive=True),
            }
            require(rendered["shelf_state"] in ("ready", "empty", "signed_out"),
                    "Shelf renderer event has an unsupported Shelf state")
            require(rendered["timestamp_ns"] <= row["monotonic_ns"],
                    "Shelf renderer event predates its recorded renderer return")
        elif row["stage"] == "shelf_submitted":
            data = event_data(row, {"shelf_state", "render_sequence", "update_marker", "submitted_monotonic_ns"},
                              "Shelf submission event")
            submitted = {
                "shelf_state": data["shelf_state"],
                "render_sequence": integer(data["render_sequence"], "Submitted Shelf render sequence", positive=True),
                "update_marker": integer(data["update_marker"], "Accepted Shelf update marker", positive=True),
                "timestamp_ns": integer(data["submitted_monotonic_ns"], "Accepted Shelf SEND", positive=True),
            }
            require(submitted["shelf_state"] in ("ready", "empty", "signed_out"),
                    "Shelf submission event has an unsupported Shelf state")
            require(submitted["timestamp_ns"] <= row["monotonic_ns"],
                    "Shelf submission event predates its accepted SEND")
        else:
            data = event_data(row, {"shelf_state", "render_sequence", "update_marker", "completed_monotonic_ns"},
                              "Shelf driver completion event")
            driver = {
                "shelf_state": data["shelf_state"],
                "render_sequence": integer(data["render_sequence"], "Completed Shelf render sequence", positive=True),
                "update_marker": integer(data["update_marker"], "Completed Shelf update marker", positive=True),
                "timestamp_ns": integer(data["completed_monotonic_ns"], "Shelf driver completion", positive=True),
            }
            require(driver["timestamp_ns"] <= row["monotonic_ns"],
                    "Shelf driver completion event predates its completion")

    if submitted is not None and rendered is not None:
        require((rendered["shelf_state"], rendered["render_sequence"]) ==
                (submitted["shelf_state"], submitted["render_sequence"]) and
                rendered["timestamp_ns"] <= submitted["timestamp_ns"],
                "Shelf render and accepted SEND belong to different frames")
    if submitted is None:
        require(driver is None, "Driver completion lacks a Shelf submission")
    if driver is not None:
        require((driver["shelf_state"], driver["render_sequence"], driver["update_marker"]) ==
                (submitted["shelf_state"], submitted["render_sequence"], submitted["update_marker"]) and
                driver["timestamp_ns"] >= submitted["timestamp_ns"],
                "Shelf driver completion does not confirm the submitted frame marker")

    status = (
        "no-qualified-frame" if rendered is None and submitted is None else
        "render-only" if submitted is None else
        "observed"
    )
    return {
        "schema": "kindle-shelf-render-intervals-v1",
        "status": status,
        "run_id": run_id,
        "process_id": process_id,
        "clock": CLOCK,
        "render_order": None if rendered is None else rendered["render_order"],
        "card_release_bracket_ns": [release_before_ns, release_after_ns],
        "renderer_return_interval_ns": None if rendered is None else interval(
            rendered["timestamp_ns"], release_before_ns, release_after_ns, "Shelf renderer return"
        ),
        "accepted_send_interval_ns": None if submitted is None else interval(
            submitted["timestamp_ns"], release_before_ns, release_after_ns, "Accepted Shelf SEND"
        ),
        "driver_completion_interval_ns": None if driver is None else interval(
            driver["timestamp_ns"], release_before_ns, release_after_ns, "Shelf driver completion"
        ),
        "landmarks_ns": {
            "shelf_rendered": None if rendered is None else rendered["timestamp_ns"],
            "shelf_submitted": None if submitted is None else submitted["timestamp_ns"],
            "shelf_driver_completed": None if driver is None else driver["timestamp_ns"],
        },
        "scope": "First observed qualified Shelf renderer and accepted SEND, if present; no touch-response or optical claim.",
        "interactive_response_claimed": False,
        "optical_completion_claimed": False,
    }


def parser():
    cli = argparse.ArgumentParser(description=__doc__)
    cli.add_argument("card_origin", type=Path, help="Sanitized, run-bound open-card XTest projection")
    cli.add_argument("shelf_render_trace", type=Path, help="One --shelf-render-trace JSONL file")
    return cli


def main():
    args = parser().parse_args()
    result = audit(read_json(args.card_origin), read_jsonl(args.shelf_render_trace))
    print(json.dumps(result, indent=2, allow_nan=False))


if __name__ == "__main__":
    try:
        main()
    except (AuditError, OSError, json.JSONDecodeError) as error:
        raise SystemExit(f"log_bounds: {error}") from None
