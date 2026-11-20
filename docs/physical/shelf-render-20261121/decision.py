"""Evaluate the predeclared thresholds on a closed Shelf experiment; print JSON to stdout."""

import argparse
import json
from pathlib import Path
from statistics import median

from project_frames import load, project, records, require
from project_launcher_stages import project as launcher_stages


def performance_checks(deltas, orders):
    require(len(orders) == 6 and list(orders) == ["AB", "BA"] * 3 and all(len(values) == 6 for values in deltas.values()),
            "Expected six alternating measured pairs")
    wins = sum(send < 0 and driver < 0 for send, driver in zip(deltas["release_to_send"], deltas["release_to_driver"], strict=True))
    group_medians = {group: median(delta for order, delta in zip(orders, deltas["ready_to_send"], strict=True) if order == group)
                     for group in ("AB", "BA")}
    return wins, group_medians, {
        "both_endpoints_faster_in_five_or_more_pairs": wins >= 5,
        "paired_driver_median_at_least_100ms_faster": median(deltas["release_to_driver"]) <= -100_000_000,
        "ready_to_send_faster_in_both_order_groups": all(value < 0 for value in group_medians.values()),
    }


def evaluate(batch, report):
    admission = load(batch / "admission.json")
    require(admission["protocol"] == "shelf-render-click-3", "Not the private-render experiment")
    frames = project(batch, report)
    pairs = frames["pairs"]
    feedback = {}
    stages = {}
    for name in admission["schedule"]:
        case = batch / name
        ready = load(case / "click.json")["up_before_ns"] + report["cases"][name]["shelf_ready_ns"]
        loading = [row["data"]["rgb_fingerprint"] for row in records(case / "performance.jsonl")
                   if (row["kind"], row["stage"]) == ("native", "frame") and row["data"]["rendered"]
                   and row["data"]["refresh_accepted"] and row["data"]["stages"]["refresh_submit"]["finished_monotonic_ns"] < ready]
        require(loading and all(isinstance(value, str) and len(value) == 16 for value in loading), "Missing loading evidence")
        feedback[name] = len(set(loading))
        stages[name] = launcher_stages(case)["interval_ms"]
    cases = {case["case"]: case for case in frames["cases"]}
    deltas = {key: [cases[f"b{number:02d}"]["ns"][key] - cases[f"a{number:02d}"]["ns"][key] for number in range(1, 7)]
              for key in ("release_to_send", "release_to_driver", "ready_to_send")}
    wins, group_medians, checks = performance_checks(deltas, [pair["order"] for pair in pairs])
    checks.update({
        "expected_transaction_order_in_all_measured_cases": all(case["order"] == ("wait_then_render" if case["case"][0] == "a" else "render_then_wait") for case in cases.values()),
        "changing_loading_feedback_every_launch": min(feedback.values()) >= 2,
        "final_pair_matches_qualified_visible_gray8": report["visible_matches_previous_packed_frame"] is True,
    })
    return {"run": frames["run"], "recommend_candidate_for_measured_profile": all(checks.values()), "checks": checks,
            "both_endpoints_faster_pairs": wins, "paired_driver_median_ms": median(deltas["release_to_driver"]) / 1e6,
            "paired_ready_to_send_median_ms_by_order": {key: value / 1e6 for key, value in group_medians.items()},
            "distinct_loading_frames": feedback, "launcher_uptime_intervals_ms": stages}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("batch", type=Path)
    parser.add_argument("paired_result", type=Path)
    args = parser.parse_args()
    print(json.dumps(evaluate(args.batch, load(args.paired_result)), indent=2))
