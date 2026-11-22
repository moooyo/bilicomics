#!/usr/bin/env python3
"""Replay the closed Scribe first-Shelf-render A/B series without device access."""

import argparse
import importlib.util
import json
from pathlib import Path
import re
import stat
import sys


HERE = Path(__file__).resolve().parent
LOG_BOUNDS_PATH = HERE.parent / "startup-interactive-kindle-20261121" / "log_bounds.py"
MAX_JSON_BYTES = 64 * 1024
MAX_TRACE_BYTES = 16 * 1024
UUID = re.compile(r"[0-9a-f]{8}-(?:[0-9a-f]{4}-){3}[0-9a-f]{12}\Z")
SCHEMA = "bili-shelf-ab-v1"
SCHEDULE = [(0, "A"), (0, "B")]
for _pair in range(1, 7):
    SCHEDULE.extend(((_pair, "A"), (_pair, "B")) if _pair % 2 else ((_pair, "B"), (_pair, "A")))
ENDPOINTS = {
    "renderer_return": "renderer_return_interval_ns",
    "accepted_send": "accepted_send_interval_ns",
    "driver_completion": "driver_completion_interval_ns",
}


def load_log_bounds():
    spec = importlib.util.spec_from_file_location("bili_shelf_ab_log_bounds", LOG_BOUNDS_PATH)
    if spec is None or spec.loader is None:
        raise RuntimeError("Cannot load the existing Shelf log auditor")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


LOG_BOUNDS = load_log_bounds()


class AnalysisError(ValueError):
    """Raised when the plan or closed evidence cannot support analysis."""


def require(condition, message):
    if not condition:
        raise AnalysisError(message)


def exact_keys(value, expected, label):
    require(type(value) is dict and set(value) == expected, f"Invalid {label} fields")


def read_regular(path, maximum):
    info = path.lstat()
    require(stat.S_ISREG(info.st_mode) and info.st_nlink == 1 and 0 <= info.st_size <= maximum,
            "Unsafe or oversized evidence file")
    content = path.read_bytes()
    require(len(content) == info.st_size, "Evidence file changed while reading")
    return content


def path_present(path):
    try:
        path.lstat()
        return True
    except FileNotFoundError:
        return False


def read_json(path):
    try:
        content = read_regular(path, MAX_JSON_BYTES).decode("utf-8")
        return json.loads(
            content,
            object_pairs_hook=LOG_BOUNDS.unique_object,
            parse_constant=LOG_BOUNDS.reject_constant,
        )
    except (OSError, UnicodeDecodeError, json.JSONDecodeError, LOG_BOUNDS.AuditError) as error:
        raise AnalysisError("Invalid or unavailable JSON evidence") from error


def read_jsonl(path):
    try:
        content = read_regular(path, MAX_TRACE_BYTES).decode("utf-8")
    except (OSError, UnicodeDecodeError) as error:
        raise AnalysisError("Invalid or unavailable Shelf trace") from error
    lines = content.splitlines()
    require(content.endswith("\n") and lines and all(line.strip() for line in lines),
            "Shelf trace is incomplete or empty")
    try:
        return [
            json.loads(
                line,
                object_pairs_hook=LOG_BOUNDS.unique_object,
                parse_constant=LOG_BOUNDS.reject_constant,
            )
            for line in lines
        ]
    except (json.JSONDecodeError, LOG_BOUNDS.AuditError) as error:
        raise AnalysisError("Invalid Shelf trace JSON") from error


def validate_plan(plan):
    exact_keys(plan, {"schema", "owner", "host_boot", "cases"}, "series plan")
    require(plan["schema"] == SCHEMA, "Unsupported series plan schema")
    require(type(plan["owner"]) is str and UUID.fullmatch(plan["owner"]) is not None,
            "Invalid series owner identity")
    require(type(plan["host_boot"]) is str and UUID.fullmatch(plan["host_boot"]) is not None,
            "Invalid series host boot identity")
    cases = plan["cases"]
    require(type(cases) is list and len(cases) == len(SCHEDULE), "Series must declare all 14 visits")
    seen_runs = set()
    seen_batches = set()
    for case, (pair, arm) in zip(cases, SCHEDULE):
        exact_keys(case, {"pair", "arm", "run", "batch"}, "case ledger row")
        require(type(case["pair"]) is int and (case["pair"], case["arm"]) == (pair, arm),
                "Case ledger differs from the predeclared warmup and alternating schedule")
        require(type(case["run"]) is str and LOG_BOUNDS.RUN_ID.fullmatch(case["run"]) is not None,
                "Invalid case run identity")
        require(case["run"] not in seen_runs, "Case run identities must be unique")
        require(type(case["batch"]) is str and Path(case["batch"]).is_absolute(),
                "Case batch path must be absolute")
        batch = Path(case["batch"])
        require(batch.name == "bilicomics-interactive-" + case["run"],
                "Case batch path is not bound to its run identity")
        require(case["batch"] not in seen_batches, "Case batch paths must be unique")
        seen_runs.add(case["run"])
        seen_batches.add(case["batch"])
    return cases


def endpoint_interval(result, name):
    value = result.get(name)
    if value is None:
        return None
    require(type(value) is list and len(value) == 2 and
            all(type(item) is int and item >= 0 for item in value) and value[0] <= value[1],
            "Invalid audited endpoint interval")
    return value


def closed_case(case, plan):
    pair, arm, run = case["pair"], case["arm"], case["run"]
    batch = Path(case["batch"])
    if not path_present(batch):
        return case_result(pair, arm, "not-started", "batch-not-present")
    info = batch.lstat()
    if not stat.S_ISDIR(info.st_mode):
        return case_result(pair, arm, "closed-unqualified", "unsafe-batch-path")

    attempted = batch / "attempted"
    if not path_present(attempted):
        if any(path_present(batch / leaf) for leaf in ("released.json", "failure.json")):
            return case_result(pair, arm, "closed-unqualified", "attempt-marker-missing")
        return case_result(pair, arm, "not-started", "visit-not-attempted")

    released_path = batch / "released.json"
    if not path_present(released_path):
        return case_result(pair, arm, "unreleased", "release-not-verified")

    try:
        admission = read_json(batch / "admission.json")
        origin = read_json(batch / "origin.json")
        retired = read_json(batch / "retired.json")
        released = read_json(released_path)
        require(admission.get("run") == run and admission.get("owner") == plan["owner"] and
                admission.get("host_boot") == plan["host_boot"], "Admission does not match the predeclared case")
        require(released.get("state") == "released" and released.get("owner_thread") == plan["owner"] and
                released.get("run_id") == run and released.get("owner_and_token_checked") is True and
                released.get("native_restored_and_connections_closed") is True and
                released.get("device_disposition") == "original-native-restored",
                "Case lacks verified native restoration and exact lease release")
        require(retired.get("natural") is True and type(retired.get("app_pid")) is int and
                retired["app_pid"] > 0, "Case did not naturally retire")
        require(not any(path_present(batch / leaf) for leaf in ("stop-intended", "partial-trace", "private-token")),
                "Case contains a disqualifying stop, partial trace, or retained token")
        if path_present(batch / "failure.json"):
            return case_result(pair, arm, "closed-unqualified", "operator-recorded-failure")

        card = read_json(batch / "card.json")
        rows = read_jsonl(batch / (run + ".jsonl"))
        require(card.get("run_id") == run and card.get("process_id") == retired["app_pid"] and
                card.get("boot_id") == origin.get("boot"), "Card origin does not match its natural process")
        result = LOG_BOUNDS.audit(card, rows)
        stored = read_json(batch / "intervals.json")
        require(stored.get("run_id") == result["run_id"] and
                stored.get("process_id") == result["process_id"] and
                stored.get("render_order") == result.get("render_order") and
                all(stored.get(name) == result[name] for name in ENDPOINTS.values()),
                "Stored intervals differ from the original log audit")
    except (OSError, TypeError, KeyError, ValueError):
        return case_result(pair, arm, "closed-unqualified", "invalid-closed-evidence")

    render_events = [row for row in rows if type(row) is dict and row.get("stage") == "shelf_rendered"]
    render_order = None
    shelf_state = None
    if len(render_events) == 1 and type(render_events[0].get("data")) is dict:
        observed_order = render_events[0]["data"].get("render_order")
        if observed_order in ("wait_then_render", "render_then_wait", "initial_fill"):
            render_order = observed_order
        shelf_state = render_events[0]["data"].get("shelf_state")

    reason = None
    if render_order is None:
        reason = "missing-or-unknown-render-order"
    elif render_order == "initial_fill":
        reason = "initial-fill-is-not-a-measured-render"
    elif shelf_state not in ("ready", "empty"):
        reason = "not-a-normal-ready-or-empty-shelf"
    elif arm == "A" and render_order != "wait_then_render":
        reason = "render-order-does-not-match-arm"
    elif arm == "B" and render_order not in ("render_then_wait", "wait_then_render"):
        reason = "render-order-does-not-match-arm"

    intervals = {name: endpoint_interval(result, key) for name, key in ENDPOINTS.items()}
    if reason is None and intervals["renderer_return"] is None:
        reason = "missing-primary-renderer-return"
    if reason is not None:
        output = case_result(pair, arm, "closed-unqualified", reason)
        output["render_order"] = render_order
        output["observed_endpoints_ns"] = intervals
    else:
        fallback = arm == "B" and render_order == "wait_then_render"
        output = case_result(
            pair,
            arm,
            "qualified-fallback" if fallback else "qualified",
            "default-fell-back-to-wait-first" if fallback else None,
        )
        output.update({
            "render_order": render_order,
            "nominal_primary_eligible": True,
            "actual_order_qualified": not fallback,
            "observed_endpoints_ns": intervals,
        })

    click = card["click"]
    trace_end = rows[-1]["monotonic_ns"] if rows and type(rows[-1].get("monotonic_ns")) is int else None
    if type(trace_end) is int:
        output["_chronology"] = {
            "boot_id": card["boot_id"],
            "next_click_down_before_ns": click.get("down_before_ns"),
            "trace_end_ns": trace_end,
        }
    return output


def case_result(pair, arm, status, reason):
    return {
        "pair": pair,
        "arm": arm,
        "status": status,
        "reason": reason,
        "nominal_primary_eligible": False,
        "actual_order_qualified": False,
        "render_order": None,
        "observed_endpoints_ns": {name: None for name in ENDPOINTS},
    }


def validate_visit_order(results):
    previous = None
    for result in results:
        current = result.get("_chronology")
        if current is None:
            continue
        if previous is not None:
            require(current["boot_id"] == previous["boot_id"] and
                    type(previous["trace_end_ns"]) is int and
                    type(current["next_click_down_before_ns"]) is int and
                    previous["trace_end_ns"] <= current["next_click_down_before_ns"],
                    "Observed device visits do not match the declared order or one device boot")
        previous = current


def interval_direction(delta):
    if delta is None:
        return "unavailable"
    if delta[1] < 0:
        return "B-faster"
    if delta[0] > 0:
        return "A-faster"
    return "interval-overlaps-zero"


def pair_delta(a_interval, b_interval):
    if a_interval is None or b_interval is None:
        return None
    return [b_interval[0] - a_interval[1], b_interval[1] - a_interval[0]]


def median_outward(values):
    ordered = sorted(values)
    middle = len(ordered) // 2
    if len(ordered) % 2:
        return [ordered[middle], ordered[middle]]
    total = ordered[middle - 1] + ordered[middle]
    return [total // 2, (total + 1) // 2]


def paired_summary(pairs, *, actual_only=False):
    summary = {}
    for endpoint in ENDPOINTS:
        deltas = [
            pair["endpoints"][endpoint]["B_minus_A_ns"]
            for pair in pairs
            if pair["endpoints"][endpoint]["B_minus_A_ns"] is not None and
            (not actual_only or pair["actual_order_qualified"])
        ]
        median_delta = None if not deltas else [
            median_outward([value[0] for value in deltas])[0],
            median_outward([value[1] for value in deltas])[1],
        ]
        summary[endpoint] = {
            "n_pairs": len(deltas),
            "median_paired_B_minus_A_ns": median_delta,
            "direction": interval_direction(median_delta),
        }
    return summary


def make_pairs(cases):
    pairs = []
    for pair_number in range(1, 7):
        members = [case for case in cases if case["pair"] == pair_number]
        a = next(case for case in members if case["arm"] == "A")
        b = next(case for case in members if case["arm"] == "B")
        endpoint_rows = {}
        for endpoint in ENDPOINTS:
            a_interval = a["observed_endpoints_ns"].get(endpoint) if a["nominal_primary_eligible"] else None
            b_interval = b["observed_endpoints_ns"].get(endpoint) if b["nominal_primary_eligible"] else None
            delta = pair_delta(a_interval, b_interval)
            endpoint_rows[endpoint] = {
                "A_ns": a_interval,
                "B_ns": b_interval,
                "B_minus_A_ns": delta,
                "direction": interval_direction(delta),
            }
        complete = endpoint_rows["renderer_return"]["B_minus_A_ns"] is not None
        pairs.append({
            "pair": pair_number,
            "order": "AB" if pair_number % 2 else "BA",
            "status": "complete" if complete else "incomplete",
            "actual_order_qualified": a["actual_order_qualified"] and b["actual_order_qualified"],
            "endpoints": endpoint_rows,
        })
    return pairs


def analyze_plan(plan):
    declared = validate_plan(plan)
    results = [closed_case(case, plan) for case in declared]
    validate_visit_order(results)

    stopped = False
    for result in results:
        if stopped and result["status"] not in ("not-started",):
            raise AnalysisError("A later visit was attempted after the first missing or invalid visit")
        if result["status"] == "unreleased":
            raise AnalysisError("An attempted visit lacks a verified release; stop and resolve its original batch")
        if result["status"] in ("not-started", "closed-unqualified"):
            stopped = True

    warmups = [
        {key: value for key, value in result.items() if not key.startswith("_")}
        for result in results[:2]
    ]
    measured = results[2:]
    public_cases = [
        {key: value for key, value in result.items() if not key.startswith("_")}
        for result in measured
    ]
    pairs = make_pairs(public_cases)
    nominal_summary = paired_summary(pairs)
    actual_summary = paired_summary(pairs, actual_only=True)
    complete_pairs = nominal_summary["renderer_return"]["n_pairs"]
    faster_pairs = sum(
        pair["endpoints"]["renderer_return"]["direction"] == "B-faster"
        for pair in pairs
    )
    median_render = nominal_summary["renderer_return"]["median_paired_B_minus_A_ns"]
    full_series = complete_pairs == 6
    primary_gate = {
        "complete_six_pairs": full_series,
        "B_faster_in_at_least_five_pairs": None if not full_series else faster_pairs >= 5,
        "median_primary_improvement_at_least_100ms": None if not full_series or median_render is None
        else median_render[1] <= -100_000_000,
        "actual_order_qualified_in_all_six_pairs": None if not full_series else
        all(pair["actual_order_qualified"] for pair in pairs),
    }
    primary_gate["passes_predeclared_primary_threshold"] = (
        None if not full_series else all(value is True for value in primary_gate.values())
    )
    for result in results:
        result.pop("_chronology", None)
    return {
        "schema": "bili-shelf-ab-analysis-v1",
        "scope": "Scribe 5.19.5; ordinary native Library card to the first qualifying normal Shelf renderer return.",
        "primary_endpoint": "renderer_return",
        "warmups": warmups,
        "warmups_included_in_primary_statistics": False,
        "cases": public_cases,
        "pairs": pairs,
        "nominal_intention_to_treat": nominal_summary,
        "actual_order_qualified_pairs_descriptive_only": actual_summary,
        "actual_order_counts": {
            "B_private_render_first": sum(
                result["arm"] == "B" and result["render_order"] == "render_then_wait"
                for result in public_cases
            ),
            "B_wait_first_fallback": sum(
                result["arm"] == "B" and result["render_order"] == "wait_then_render"
                for result in public_cases
            ),
        },
        "primary_threshold": primary_gate,
        "method": {
            "pair_delta": "B_minus_A_interval = [B.lo - A.hi, B.hi - A.lo]",
            "median": "Median the paired B-minus-A intervals; never subtract arm medians.",
            "secondary_endpoints": "Accepted SEND and same-marker driver completion are reported separately.",
            "B_fallback": "A B wait-then-render fallback remains in nominal results and is excluded only from the descriptive actual-order-qualified subset.",
            "optical_or_clickability_claimed": False,
            "screenshots_or_followup_input_used": False,
        },
    }


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("plan", type=Path, help="Predeclared plan.json from series.py")
    args = parser.parse_args(argv)
    try:
        plan = read_json(args.plan)
        report = analyze_plan(plan)
    except (AnalysisError, LOG_BOUNDS.AuditError, OSError, json.JSONDecodeError) as error:
        raise SystemExit(f"analyze_shelf_ab: {error}") from None
    print(json.dumps(report, indent=2, allow_nan=False))


if __name__ == "__main__":
    main()
