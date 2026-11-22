#!/usr/bin/env python3
"""Replay a closed ordinary Library preflight A/B series without device access."""

import argparse
import importlib.util
import json
from pathlib import Path
import re
import stat


HERE = Path(__file__).resolve().parent
LOG_BOUNDS_PATH = HERE.parent / "startup-interactive-kindle-20261121" / "log_bounds.py"
MAX_JSON_BYTES = 64 * 1024
MAX_TRACE_BYTES = 16 * 1024
SCHEMA = "bili-library-admission-ab-v1"
ANALYSIS_SCHEMA = "bili-library-admission-analysis-v1"
ELF_SIZE = 74392336
UUID = re.compile(r"[0-9a-f]{8}-(?:[0-9a-f]{4}-){3}[0-9a-f]{12}\Z")
SCHEDULE = [(0, "A"), (0, "B")]
for _pair in range(1, 7):
    SCHEDULE.extend(((_pair, "A"), (_pair, "B")) if _pair % 2 else ((_pair, "B"), (_pair, "A")))
ENDPOINTS = {
    "renderer_return": "renderer_return_interval_ns",
    "accepted_send": "accepted_send_interval_ns",
    "driver_completion": "driver_completion_interval_ns",
}
ARM_STRATEGY = {"A": "repeated", "B": "worker-only"}
ARM_FIELDS = {"schema", "run", "arm", "parent_admission", "application_bytes", "default_render_order"}
PROBE_FAILURE_SCHEMA = "bili-native-probe-failure-v1"
PROBE_FAILURE_FIELDS = {"schema", "phase", "action", "step", "tab_request", "card_request"}
PROBE_PHASE_ACTIONS = {
    "native-navigation": {"read-navigation", "request-target-tab"},
    "identify-card": {"identify-card"},
    "card-release": {"open-card"},
}
PROBE_STEPS = {"native-guard", "operation-request", "response-validation", "receipt-accepted"}
REQUEST_STATES = {"not-requested", "dispatch-possible", "receipt-confirmed"}


def load_log_bounds():
    spec = importlib.util.spec_from_file_location("bili_library_admission_log_bounds", LOG_BOUNDS_PATH)
    if spec is None or spec.loader is None:
        raise RuntimeError("Cannot load the existing Shelf log auditor")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


LOG_BOUNDS = load_log_bounds()

SHELF_AB_PATH = HERE.parent / "startup-shelf-ab-20261121" / "analyze_shelf_ab.py"
SHELF_AB_SPEC = importlib.util.spec_from_file_location("bili_library_admission_shelf_ab", SHELF_AB_PATH)
if SHELF_AB_SPEC is None or SHELF_AB_SPEC.loader is None:
    raise RuntimeError("Cannot load the existing paired Shelf analyzer")
SHELF_AB = importlib.util.module_from_spec(SHELF_AB_SPEC)
SHELF_AB_SPEC.loader.exec_module(SHELF_AB)


class AnalysisError(ValueError):
    """Raised when the plan or closed evidence cannot support analysis."""


def require(condition, message):
    if not condition:
        raise AnalysisError(message)


def exact_keys(value, expected, label):
    require(type(value) is dict and set(value) == expected, f"Invalid {label} fields")


def read_regular(path, maximum, *, allow_empty=False):
    info = path.lstat()
    require(stat.S_ISREG(info.st_mode) and info.st_nlink == 1
            and (0 if allow_empty else 1) <= info.st_size <= maximum,
            "Unsafe, empty or oversized evidence file")
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


def case_result(pair, arm, status, reason):
    return {
        "pair": pair,
        "arm": arm,
        "status": status,
        "reason": reason,
        "nominal_primary_eligible": False,
        "render_order": None,
        "observed_endpoints_ns": {name: None for name in ENDPOINTS},
    }


def expected_arm_marker(case):
    return {
        "schema": "bili-library-admission-arm-v1",
        "run": case["run"],
        "arm": case["arm"],
        "parent_admission": ARM_STRATEGY[case["arm"]],
        "application_bytes": ELF_SIZE,
        "default_render_order": True,
    }


def validate_probe_failure(value, failure_phase):
    exact_keys(value, PROBE_FAILURE_FIELDS, "probe failure receipt")
    require(value["schema"] == PROBE_FAILURE_SCHEMA, "Unexpected probe failure schema")
    require(value["phase"] in PROBE_PHASE_ACTIONS and value["phase"] == failure_phase,
            "Probe failure phase differs from the original failure")
    require(value["action"] in PROBE_PHASE_ACTIONS[value["phase"]],
            "Probe failure action does not match its phase")
    require(value["step"] in PROBE_STEPS, "Unknown probe failure step")
    require(value["tab_request"] in REQUEST_STATES and value["card_request"] in REQUEST_STATES,
            "Unknown probe request state")
    return value


def probe_failure_result(case, batch, failure):
    pair, arm, run = case["pair"], case["arm"], case["run"]
    probe = {key: failure[key] for key in ("phase", "action", "step", "tab_request", "card_request")}
    output = case_result(pair, arm, "closed-unqualified", "probe-failure-without-primary-render")
    output["probe_failure"] = probe
    states = (probe["tab_request"], probe["card_request"])
    output["native_input_conclusion"] = (
        "dispatch-ambiguous" if "dispatch-possible" in states else
        "receipt-confirmed" if "receipt-confirmed" in states else
        "no-requested-input"
    )
    post_request_evidence = (
        "observed-click.json",
        "retired.json",
        "card.json",
        "intervals.json",
        run + ".jsonl",
    )
    card_request_unresolved = any(path_present(batch / leaf) for leaf in post_request_evidence)
    if probe["card_request"] == "not-requested" and not card_request_unresolved:
        output.update({
            "status": "pre-card-input-not-requested",
            "reason": "card-input-not-requested-no-launch-evidence",
            "native_card_clicked": False,
            "candidate_application_started": False,
        })
    elif probe["card_request"] == "receipt-confirmed":
        output.update({
            "reason": "card-input-receipt-without-qualified-render",
            "native_card_clicked": True,
            "candidate_application_started": None,
        })
    else:
        output["reason"] = "card-input-dispatch-or-launch-unresolved"
        output["native_card_clicked"] = None
        output["candidate_application_started"] = None
    return output


def endpoint_interval(result, name):
    value = result.get(name)
    if value is None:
        return None
    require(type(value) is list and len(value) == 2 and
            all(type(item) is int and item >= 0 for item in value) and value[0] <= value[1],
            "Invalid audited endpoint interval")
    return value


def closed_case(case, plan, *, require_probe_failure=False):
    pair, arm, run = case["pair"], case["arm"], case["run"]
    batch = Path(case["batch"])
    if not path_present(batch):
        return case_result(pair, arm, "not-started", "batch-not-present")
    info = batch.lstat()
    if not stat.S_ISDIR(info.st_mode):
        return case_result(pair, arm, "closed-unqualified", "unsafe-batch-path")

    attempted_path = batch / "attempted"
    if not path_present(attempted_path):
        if any(path_present(batch / leaf) for leaf in ("released.json", "failure.json", "card.json")):
            return case_result(pair, arm, "closed-unqualified", "attempt-marker-missing")
        return case_result(pair, arm, "not-started", "visit-not-attempted")

    try:
        read_regular(attempted_path, 0, allow_empty=True)
    except (AnalysisError, OSError):
        return case_result(pair, arm, "closed-unqualified", "invalid-attempt-marker")

    released_path = batch / "released.json"
    if not path_present(released_path):
        return case_result(pair, arm, "unreleased", "release-not-verified")

    try:
        admission = read_json(batch / "admission.json")
        origin = read_json(batch / "origin.json")
        released = read_json(released_path)
        arm_marker = read_json(batch / "launcher-arm.json")
        exact_keys(arm_marker, ARM_FIELDS, "launcher arm marker")
        require(arm_marker == expected_arm_marker(case), "Launcher arm does not match the predeclared case")
        require(admission.get("run") == run and admission.get("owner") == plan["owner"] and
                admission.get("host_boot") == plan["host_boot"],
                "Admission does not match the predeclared case")
        require(released.get("state") == "released" and released.get("owner_thread") == plan["owner"] and
                released.get("run_id") == run and released.get("owner_and_token_checked") is True and
                released.get("native_restored_and_connections_closed") is True and
                released.get("device_disposition") == "original-native-restored",
                "Case lacks verified native restoration and exact lease release")
        require(type(origin.get("boot")) is str and UUID.fullmatch(origin["boot"]) is not None,
                "Invalid device boot identity")
        require(not any(path_present(batch / leaf) for leaf in
                        ("stop-intended", "partial-trace", "private-token")),
                "Case contains a disqualifying stop, partial trace, or retained token")
        failure_path = batch / "failure.json"
        probe_path = batch / "probe-failure.json"
        if path_present(probe_path) and not path_present(failure_path):
            return case_result(pair, arm, "closed-unqualified", "probe-failure-without-original-failure")
        if path_present(failure_path):
            failure = read_json(failure_path)
            exact_keys(failure, {"phase", "reason"}, "operator failure receipt")
            require(type(failure["phase"]) is str and type(failure["reason"]) is str,
                    "Invalid operator failure receipt")
            if path_present(probe_path):
                try:
                    probe_failure = validate_probe_failure(read_json(probe_path), failure["phase"])
                except (AnalysisError, OSError, TypeError, KeyError, ValueError):
                    return case_result(pair, arm, "closed-unqualified", "invalid-probe-failure-receipt")
                return probe_failure_result(case, batch, probe_failure)
            if require_probe_failure and failure["phase"] in PROBE_PHASE_ACTIONS:
                return case_result(pair, arm, "closed-unqualified", "probe-failure-receipt-missing")
            if require_probe_failure:
                return case_result(pair, arm, "closed-unqualified", "operator-recorded-failure")
            if failure["phase"] == "native-navigation":
                post_navigation_evidence = (
                    "activation-intended",
                    "click-intended",
                    "observed-click.json",
                    "retired.json",
                    "card.json",
                    "intervals.json",
                    run + ".jsonl",
                )
                if not any(path_present(batch / leaf) for leaf in post_navigation_evidence):
                    output = case_result(
                        pair,
                        arm,
                        "pre-click-navigation-failure",
                        "native-navigation-failed-before-activation",
                    )
                    output.update({
                        "native_card_clicked": False,
                        "candidate_application_started": False,
                    })
                    return output
                return case_result(
                    pair,
                    arm,
                    "closed-unqualified",
                    "native-navigation-failure-has-post-navigation-evidence",
                )
            retired = read_json(batch / "retired.json")
            require(retired.get("natural") is True and type(retired.get("app_pid")) is int and
                    retired["app_pid"] > 0, "Case did not naturally retire")
            return case_result(pair, arm, "closed-unqualified", "operator-recorded-failure")

        retired = read_json(batch / "retired.json")
        require(retired.get("natural") is True and type(retired.get("app_pid")) is int and
                retired["app_pid"] > 0, "Case did not naturally retire")
        card = read_json(batch / "card.json")
        rows = read_jsonl(batch / (run + ".jsonl"))
        require(card.get("run_id") == run and card.get("process_id") == retired["app_pid"] and
                card.get("boot_id") == origin["boot"],
                "Card origin does not match its natural process")
        result = LOG_BOUNDS.audit(card, rows)
        stored = read_json(batch / "intervals.json")
        require(stored == result, "Stored intervals differ from the original log audit")
    except (OSError, TypeError, KeyError, ValueError):
        return case_result(pair, arm, "closed-unqualified", "invalid-closed-evidence")

    render_events = [row for row in rows if type(row) is dict and row.get("stage") == "shelf_rendered"]
    shelf_state = None
    if len(render_events) == 1 and type(render_events[0].get("data")) is dict:
        shelf_state = render_events[0]["data"].get("shelf_state")
    intervals = {name: endpoint_interval(result, key) for name, key in ENDPOINTS.items()}
    reason = None
    if shelf_state not in ("ready", "empty"):
        reason = "not-a-normal-ready-or-empty-shelf"
    elif intervals["renderer_return"] is None:
        reason = "missing-primary-renderer-return"
    if reason is not None:
        output = case_result(pair, arm, "closed-unqualified", reason)
        output["observed_endpoints_ns"] = intervals
    else:
        output = case_result(pair, arm, "qualified", None)
        output.update({
            "nominal_primary_eligible": True,
            "render_order": result.get("render_order"),
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


def paired_summary(pairs):
    summary = {}
    for endpoint in ENDPOINTS:
        deltas = [
            pair["endpoints"][endpoint]["B_minus_A_ns"]
            for pair in pairs
            if pair["endpoints"][endpoint]["B_minus_A_ns"] is not None
        ]
        median_delta = None if not deltas else [
            SHELF_AB.median_outward([value[0] for value in deltas])[0],
            SHELF_AB.median_outward([value[1] for value in deltas])[1],
        ]
        summary[endpoint] = {
            "n_pairs": len(deltas),
            "median_paired_B_minus_A_ns": median_delta,
            "direction": SHELF_AB.interval_direction(median_delta),
        }
    return summary


def make_pairs(cases):
    pairs = []
    for pair_number in range(1, 7):
        members = [case for case in cases if case["pair"] == pair_number]
        a = next(case for case in members if case["arm"] == "A")
        b = next(case for case in members if case["arm"] == "B")
        endpoints = {}
        for endpoint in ENDPOINTS:
            a_interval = a["observed_endpoints_ns"].get(endpoint) if a["nominal_primary_eligible"] else None
            b_interval = b["observed_endpoints_ns"].get(endpoint) if b["nominal_primary_eligible"] else None
            delta = SHELF_AB.pair_delta(a_interval, b_interval)
            endpoints[endpoint] = {
                "A_ns": a_interval,
                "B_ns": b_interval,
                "B_minus_A_ns": delta,
                "direction": SHELF_AB.interval_direction(delta),
            }
        complete = endpoints["renderer_return"]["B_minus_A_ns"] is not None
        pairs.append({
            "pair": pair_number,
            "order": "AB" if pair_number % 2 else "BA",
            "status": "complete" if complete else "incomplete",
            "endpoints": endpoints,
        })
    return pairs


def analyze_plan(plan, *, require_probe_failure=False):
    declared = validate_plan(plan)
    results = [
        closed_case(case, plan, require_probe_failure=require_probe_failure)
        for case in declared
    ]
    validate_visit_order(results)

    stopped = False
    for result in results:
        if stopped and result["status"] not in ("not-started",):
            raise AnalysisError("A later visit was attempted after the first missing or invalid visit")
        if result["status"] == "unreleased":
            raise AnalysisError("An attempted visit lacks a verified release; stop and resolve its original batch")
        if result["status"] in (
            "not-started",
            "closed-unqualified",
            "pre-click-navigation-failure",
            "pre-card-input-not-requested",
        ):
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
    summary = paired_summary(pairs)
    primary = summary["renderer_return"]
    complete_pairs = primary["n_pairs"]
    faster_pairs = sum(
        pair["endpoints"]["renderer_return"]["direction"] == "B-faster"
        for pair in pairs
    )
    median_render = primary["median_paired_B_minus_A_ns"]
    full_series = complete_pairs == 6
    threshold = {
        "complete_six_pairs": full_series,
        "B_faster_in_at_least_five_pairs": None if not full_series else faster_pairs >= 5,
        "median_primary_improvement_at_least_100ms": None if not full_series or median_render is None
        else median_render[1] <= -100_000_000,
    }
    threshold["passes_predeclared_primary_threshold"] = (
        None if not full_series else all(value is True for value in threshold.values())
    )
    order_medians = {
        order: paired_summary([pair for pair in pairs if pair["order"] == order])
        for order in ("AB", "BA")
    }
    for result in results:
        result.pop("_chronology", None)
    return {
        "schema": ANALYSIS_SCHEMA,
        "scope": "Scribe; ordinary native Library card to the first qualifying normal Shelf renderer return; A repeats parent and worker admission, B validates in the worker.",
        "primary_endpoint": "renderer_return",
        "warmups": warmups,
        "warmups_included_in_primary_statistics": False,
        "cases": public_cases,
        "pairs": pairs,
        "paired_summary": summary,
        "paired_order_medians": order_medians,
        "primary_threshold": threshold,
        "method": {
            "pair_delta": "B_minus_A_interval = [B.lo - A.hi, B.hi - A.lo]",
            "median": "Median paired B-minus-A intervals; never subtract arm medians.",
            "secondary_endpoints": "Accepted SEND and same-marker driver completion are reported separately.",
            "render_order": "The existing trace may omit render_order; this is not a qualification failure.",
            "optical_or_clickability_claimed": False,
            "screenshots_or_followup_input_used": False,
            "probe_failure_contract": "bili-native-probe-failure-v1 required for supported probe failures"
            if require_probe_failure else "legacy-compatible; fixed probe-failure receipts are validated when present",
        },
    }


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--require-probe-failure",
        action="store_true",
        help="Require the fixed probe-failure receipt for new-series probe failures",
    )
    parser.add_argument("plan", type=Path, help="Predeclared plan.json from native_series.py")
    args = parser.parse_args(argv)
    try:
        report = analyze_plan(
            read_json(args.plan),
            require_probe_failure=args.require_probe_failure,
        )
    except (AnalysisError, LOG_BOUNDS.AuditError, OSError, json.JSONDecodeError) as error:
        raise SystemExit(f"analyze: {error}") from None
    print(json.dumps(report, indent=2, allow_nan=False))


if __name__ == "__main__":
    main()
