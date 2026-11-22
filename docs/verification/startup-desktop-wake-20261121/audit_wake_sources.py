"""Audit callback provenance and X11 observation bounds in a closed desktop wake comparison."""

import argparse
from collections import Counter
import json
from pathlib import Path
from statistics import median
import sys

for sibling in ("startup-cover-worker-20261121", "startup-direct-png-20261121"):
    sys.path.append(str(Path(__file__).resolve().parent.parent / sibling))
import stage_costs
import summarize_pairs


SOURCES = {"maintenance_timer", "runtime_wakeup", "desktop_startup_fallback"}
FULL_SUBMIT = "desktop_software_full_submit"
POST_TURN = "desktop_post_submit_turn"
UNAVAILABLE = "desktop_submit_probe_unavailable"


def checked_hook(event, case, stages, shown, first, gate, workers, title, activity):
    require = stage_costs.require
    require(not stages.get(UNAVAILABLE), "Formal full-submit hook was reported unavailable")
    submit, turn = event(FULL_SUBMIT), event(POST_TURN)
    submitted_at, turned_at = submit["monotonic_ns"], turn["monotonic_ns"]
    mode = {"A": "observe", "B": "wake"}[case["selected_arm"]]
    surface, decision = submit["data"], turn["data"]
    require(case["bootstrap_probe_mode"] == surface["mode"] == decision["mode"] == mode and
            case["process_probe_environment_verified"] is True and surface["ui_thread"] is True and
            (surface["width"], surface["height"]) == (1860, 2480) and shown <= submitted_at <= turned_at,
            "Invalid successful full-window submission or treatment identity")
    guards = ("workers_started_before", "window_alive", "booting", "suspended", "quitting", "cancelled", "driver_available", "will_drive", "ui_thread")
    require(all(type(decision[key]) is bool for key in guards) and decision["ui_thread"] is True and
            type(decision["delay_ms"]) is int and decision["delay_ms"] == 16, "Invalid post-submit decision")
    for row, keys in ((submit, ("width", "height", "ui_thread", "mode")), (turn, (*guards, "delay_ms", "mode"))):
        stored = case["wake"][row["stage"]]
        require(stored["monotonic_ns"] == row["monotonic_ns"] and
                all(stored[key] == row["data"][key] for key in keys), "Observer's research record differs from trace")
    if workers != turned_at:
        require(decision["workers_started_before"] == (workers < turned_at), "Worker admission disagrees with post-submit turn")
    eligible = (decision["window_alive"] and decision["booting"] and not decision["workers_started_before"]
                and not decision["suspended"] and not decision["quitting"] and not decision["cancelled"] and decision["driver_available"])
    drive = decision["will_drive"]
    require(drive == (mode == "wake" and eligible), "Post-submit decision disagrees with guards")
    source = "desktop_startup_fallback"
    fallback = gate["data"]["source"] == source
    require(not fallback or (drive and turned_at <= workers), "Fallback admission has no preceding drive decision")
    require(first["data"]["source"] != source or (drive and turned_at <= first["monotonic_ns"]),
            "First fallback driver has no preceding drive decision")
    owns = fallback and drive
    expected_owner = (first["data"]["source"] == gate["data"]["source"] == (source if mode == "wake" else "maintenance_timer")
                      and (owns if mode == "wake" else not drive))
    require(not stages.get("first_scene_rendered") and not first["data"]["render_notifier_registered"],
            "Unexpected public notifier path in this locked software-renderer comparison")
    return {"mode": mode, "submit_to_turn_ns": turned_at - submitted_at,
            "submit_record_minus_title_read_end_ns": submitted_at - title[1],
            "submit_record_minus_activity_read_end_ns": submitted_at - activity[1],
            "workers_minus_full_submit_record_ns": workers - submitted_at,
            "submit_record_before_workers": submitted_at <= workers,
            "turn_will_drive": drive, "turn_drove_worker_admission": owns, "expected_owner_observed": expected_owner}


def inspect(directory, case):
    baseline = stage_costs.read_case(directory, case, "shelf", 10)
    _, bounds, _, enter, settled = summarize_pairs.read_case(directory.parent, directory.name, case)
    pid, ready = baseline["pid"], baseline["ui_ready_ns"]
    require = stage_costs.require
    stages = {}
    for row in stage_costs.rows(directory / "performance.jsonl"):
        if row["kind"] == "startup":
            stages.setdefault(row["stage"], []).append(row)
    journal = stage_costs.rows(directory / "lifecycle.jsonl")

    def event(key, ui=True):
        values = stages.get(key, [])
        require(len(values) == 1 and (values[0]["thread_id"] == pid) == ui, f"{directory.name}: invalid {key}")
        return values[0]

    def observed(key):
        stamps = [row["monotonic_ns"] for row in journal if row.get("stage") == key and row["event"] == "startup-stage"]
        if key == "first_scene_rendered" and not stamps and key not in stages:
            return None
        row = event(key)
        stamp = row["data"]["observed"]["monotonic_ns"]
        require(stamps == [stamp] and enter <= stamp <= row["monotonic_ns"], f"{directory.name}: inconsistent {key}")
        return stamp

    shown, workers, render = (observed(key) for key in ("window_shown", "workers_started", "first_scene_rendered"))
    first, gate = (event(key) for key in ("driver_first_entry", "ui_worker_gate"))
    registered = first["data"]["render_notifier_registered"]
    require(type(registered) is bool and gate["data"]["render_notifier_registered"] is registered,
            "Notifier registration changed during startup")
    for callback in (first, gate):
        rendered = render is not None and render <= callback["monotonic_ns"]
        data = callback["data"]
        require(data["source"] in SOURCES and data["first_scene_rendered"] is rendered and
                (registered or not rendered) and (data["source"] != "desktop_startup_fallback" or
                (not registered and case["selected_arm"] == "B")), "Invalid callback or notifier provenance")
    require((registered or render is None) and (not registered or gate["data"]["first_scene_rendered"]) and
            gate["data"]["workers_admitted_by_ui"] is True, "Desktop admission bypassed its render or UI guard")
    font_queue, state_queue = (event(key)["monotonic_ns"] for key in ("font_worker_queued", "state_worker_queued"))
    began = event("state_worker_begin", ui=False)["monotonic_ns"]
    require(enter <= shown <= first["monotonic_ns"] <= workers <= font_queue <= state_queue <= gate["monotonic_ns"] <= ready
            and state_queue <= began <= case["stages"]["producer_ns"], "Invalid worker or driver order")

    title = case["feedback_sample_ns"]
    activity = case["initial_activity_sample_ns"]
    match, later = case["first_exact_shelf_sample_ns"], case["second_exact_shelf_sample_ns"]
    require(type(case["xid"]) is int and case["xid"] > 0 and case["visible_marker_count"] == 10 and
            case["ready_resident_bytes"] == 5_324_800 and type(activity) is list and len(activity) == 2 and
            all(type(stamp) is int for stamp in activity) and title[1] <= activity[0] <= activity[1] <= match[0] and
            later[0] - match[1] >= 80_000_000, "Missing ordered initial activity or stable complete-Shelf observation")
    ordered = max(title[1], activity[1]) <= min(workers, font_queue, state_queue)
    hook = checked_hook(event, case, stages, shown, first, gate, workers, title, activity)
    before_queue = max(title[1], activity[1]) <= state_queue
    before_prepared = max(title[1], activity[1]) <= case["stages"]["producer_ns"]
    wake = case["wake"]
    expected = {"pid": pid, "process_enter_ns": enter, "window_shown_ns": shown, "first_scene_rendered_ns": render,
                "workers_started_ns": workers, "font_worker_queued_ns": font_queue, "state_worker_queued_ns": state_queue,
                "state_worker_begin_ns": began, "visible_before_workers_started": ordered,
                "visible_before_state_worker_queued": before_queue, "visible_before_state_prepared": before_prepared}
    require(all(wake[key] == value for key, value in expected.items()), "Stored wake observation disagrees with trace or X11 clocks")
    for row in (first, gate):
        stored = wake[row["stage"]]
        keys = ("source", "render_notifier_registered", "first_scene_rendered")
        require(stored["monotonic_ns"] == row["monotonic_ns"] and all(stored[key] == row["data"][key] for key in keys),
                "Stored callback provenance differs from trace")
    require(wake["ui_worker_gate"]["workers_admitted_by_ui"] is True, "Stored UI gate did not own admission")
    start_name, gate_name = first["data"]["source"], gate["data"]["source"]
    return {"case": directory.name, "pid": pid, "enter_ns": enter, "settled_ns": settled, "research_hook": hook,
            "first_driver": start_name, "admission_callback": gate_name, "notifier_registered": registered,
            "render_event_observed": render is not None, "first_driver_after_render": first["data"]["first_scene_rendered"],
            "entry_to_first_driver_ns": first["monotonic_ns"] - enter, "entry_to_workers_started_ns": workers - enter,
            "entry_to_ready_ns": ready - enter, "title_and_activity_observed_before_workers": ordered,
            "title_and_activity_observed_before_state_queue": before_queue, "title_and_activity_observed_before_state_prepared": before_prepared,
            "workers_minus_later_positive_read_end_ns": workers - max(title[1], activity[1]),
            "entry_to_feedback_bracket_ns": case["stages"]["enter_to_feedback_bracket_ns"], "entry_to_shelf_bracket_ns": bounds}


def audit(trial):
    report = stage_costs.load(trial / "report.json")
    schedule = summarize_pairs.SCHEDULE
    require = stage_costs.require
    require(not (trial / "failure.json").exists() and report["clock"] == "CLOCK_MONOTONIC" and
            report["schedule"] == schedule and report["warmups_excluded"] == schedule[:2] and
            set(report["cases"]) == set(schedule) and set(report["sources"]) == {"A", "B"} and
            all(report["sources"].values()) and report["sources"]["A"] != report["sources"]["B"], "Incomplete or unidentified trial")
    cases = {name: inspect(trial / name, report["cases"][name]) for name in schedule}
    require(all(cases[before]["settled_ns"] < cases[after]["enter_ns"] for before, after in zip(schedule, schedule[1:])),
            "Cases overlap or chronology differs from alternate schedule")
    pairs = []
    require(len(report["pairs"]) == 6, "Missing pair")
    for number, stored in enumerate(report["pairs"], 1):
        control, candidate = [cases[f"{arm}{number:02d}"]["entry_to_shelf_bracket_ns"] for arm in "ab"]
        order = "AB" if number % 2 else "BA"
        interval = [candidate[0] - control[1], candidate[1] - control[0]]
        upper_delta = candidate[1] - control[1]
        require(stored["pair"] == number and stored["order"] == order and stored["B_minus_A_shelf_bounds_ns"] == interval
                and stored["B_minus_A_first_observed_shelf_ns"] == upper_delta, "Paired endpoints disagree")
        pairs.append({"number": number, "order": order, "B_minus_A_shelf_bounds_ns": interval,
                      "B_minus_A_first_observed_shelf_ns": upper_delta,
                      "classification": "B_earlier" if interval[1] < 0 else "B_later" if interval[0] > 0 else "overlap_or_touch"})
    counts = {}
    for arm in "AB":
        measured = [cases[name] for name in schedule[2:] if name[0].upper() == arm]
        counts[arm] = {key: dict(Counter(case[key] for case in measured)) for key in ("first_driver", "admission_callback")}
    wins = sum(pair["classification"] == "B_earlier" for pair in pairs)
    order_medians = {order: median(pair["B_minus_A_first_observed_shelf_ns"] for pair in pairs if pair["order"] == order)
                     for order in ("AB", "BA")}
    feedback_ok = all(case["title_and_activity_observed_before_workers"] for case in cases.values())
    ownership_ok = all(case["research_hook"]["expected_owner_observed"] for case in cases.values())
    submit_ok = all(case["research_hook"]["submit_record_before_workers"] for case in cases.values())
    presentation_ok = wins >= 5 and all(value < 0 for value in order_medians.values())
    return {"scope": "Formal same-ELF observe/wake comparison only; stock qualification is separate. A successful software-backend submit and a later callback are not X11 pixels or panel receipts. Use the independent crops and full-window brackets. The delay setting is not an observed saving.",
            "sources": report["sources"], "warmups_excluded_from_comparison": schedule[:2], "cases": list(cases.values()),
            "measured_callback_counts": counts, "pairs": pairs, "definite_B_earlier": wins,
            "median_observed_upper_delta_ns_by_order": order_medians,
            "all_title_and_activity_reads_completed_before_workers": feedback_ok,
            "all_full_submit_records_before_workers": submit_ok, "all_expected_callback_owners_observed": ownership_ok,
            "presentation_timing_gate_passed": presentation_ok,
            "formal_provenance_feedback_and_presentation_passed": feedback_ok and submit_ok and ownership_ok and presentation_ok}


def stock_qualification(directory):
    require = stage_costs.require
    case = stage_costs.load(directory / "result.json")
    final = stage_costs.load(directory / "final.json")
    pid = case["pid"]
    require(case["case"] == directory.name == "check-stock" and case["selected_arm"] == "A0" and case["bootstrap_probe_mode"] is None
            and case["process_executable_matches"] is True and case["process_probe_environment_verified"] is True,
            "Expected the independent stock qualification without a research mode")
    require(final["process_id"] == pid and case["xid"] > 0 and final["backend"] == "desktop-software" and
            final["profile"] == "scribe" and final["route"] == "shelf" and final["demo"] is False and final["offline"] is True and
            final["offline_requested"] is True and not final["startup"]["booting"] and not final["startup"]["error"] and
            case["visible_marker_count"] == case["ready_resident_covers"] == 10 and case["ready_resident_bytes"] == 5_324_800,
            "Unexpected stock fixture")
    summarize_pairs.read_case(directory.parent, directory.name, case)
    rows = stage_costs.rows(directory / "performance.jsonl")
    life = stage_costs.rows(directory / "lifecycle.jsonl")
    require(all(row["process_id"] == pid for row in rows + life) and
            not any(row["stage"] in (FULL_SUBMIT, POST_TURN, UNAVAILABLE) for row in rows) and
            all(case["wake"][key] is None for key in (FULL_SUBMIT, POST_TURN)), "Mixed stock processes or research telemetry")
    def ui_event(key):
        values = [row for row in rows if (row["kind"], row["stage"]) == ("startup", key)]
        require(len(values) == 1 and values[0]["thread_id"] == pid, f"Unexpected stock event: {key}")
        return values[0]
    callbacks = {key: ui_event(key) for key in ("driver_first_entry", "ui_worker_gate")}
    no_render = not any(row["stage"] == "first_scene_rendered" for row in rows) and not any(row.get("stage") == "first_scene_rendered" for row in life)
    workers = ui_event("workers_started")["data"]["observed"]["monotonic_ns"]
    first_source, gate_source = (row["data"]["source"] for row in callbacks.values())
    require([row["monotonic_ns"] for row in life if row.get("stage") == "workers_started" and row["event"] == "startup-stage"] == [workers]
            and callbacks["driver_first_entry"]["monotonic_ns"] <= workers <= ui_event("font_worker_queued")["monotonic_ns"]
            <= ui_event("state_worker_queued")["monotonic_ns"] <= callbacks["ui_worker_gate"]["monotonic_ns"] <= case["stages"]["ready_ns"],
            "Invalid stock worker clocks")
    for key, row in callbacks.items():
        stored = case["wake"][key]
        require(row["data"]["source"] in SOURCES - {"desktop_startup_fallback"} and stored["monotonic_ns"] == row["monotonic_ns"] and
                all(stored[field] == row["data"][field] for field in ("source", "render_notifier_registered", "first_scene_rendered")),
                "Invalid stock callback source")
    title, activity = case["feedback_sample_ns"], case["initial_activity_sample_ns"]
    require(title[1] <= activity[0] <= activity[1] <= case["first_exact_shelf_sample_ns"][0] and
            case["wake"]["workers_started_ns"] == workers and case["wake"]["visible_before_workers_started"] is (activity[1] <= workers),
            "Invalid stock external observation or admission")
    supports = (first_source == gate_source == "maintenance_timer" and callbacks["ui_worker_gate"]["data"]["workers_admitted_by_ui"] is True
                and no_render and all(row["data"]["render_notifier_registered"] is False and
                row["data"]["first_scene_rendered"] is False for row in callbacks.values()) and activity[1] <= workers)
    return {"scope": "Stock populated desktop qualification only; not a formal case or timing endpoint.", "pid": pid,
            "first_driver": first_source, "admission_callback": gate_source, "public_render_event_absent": no_render,
            "title_and_activity_observed_before_workers": activity[1] <= workers,
            "stock_maintenance_hypothesis_supported": supports}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("trial", nargs="?", type=Path, help="Closed trial directory containing report.json and all 14 cases")
    parser.add_argument("--stock-case", type=Path, help="Audit only the separate untimed check-stock directory")
    options = parser.parse_args()
    if bool(options.trial) == bool(options.stock_case):
        parser.error("Select exactly one closed formal trial or --stock-case directory")
    result = stock_qualification(options.stock_case) if options.stock_case else audit(options.trial)
    print(json.dumps(result, indent=2))
