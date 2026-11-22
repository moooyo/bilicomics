"""Recalculate a closed ordinary desktop Shelf presentation and interaction experiment without launching it."""

import argparse
import json
from pathlib import Path
from statistics import median


SCHEDULE = ["w00", *(f"s{number:02d}" for number in range(1, 7))]


def require(condition, message):
    if not condition:
        raise ValueError(message)


def unique(items):
    result = dict(items)
    require(len(result) == len(items), "Duplicate JSON key")
    return result


def load(path):
    return json.loads(path.read_text(encoding="utf-8"), object_pairs_hook=unique)


def bounds(record, key):
    value = record[key]
    require(type(value) is list and len(value) == 2 and all(type(stamp) is int for stamp in value)
            and 0 < value[0] <= value[1], f"Invalid X11 or input interval: {key}")
    return value


def ordered(*spans):
    return all(before[1] <= after[0] for before, after in zip(spans, spans[1:]))


def raw_case(directory, case):
    require(load(directory / "case.json") == case, "Raw case differs from delivered projection")
    life = [json.loads(line) for line in (directory / "lifecycle.jsonl").read_text(encoding="utf-8").splitlines()]
    pid = case["pid"]
    require(not (directory / "performance.jsonl").exists() and all(row["process_id"] == pid for row in life), "Mixed processes or unwanted performance profiling")
    require([row["monotonic_ns"] for row in life if row.get("stage") == "process_enter" and row["event"] == "startup-stage"]
            == [case["process_enter_ns"]], "Application entry differs")
    terminal, = [row for row in life if row["event"] == "ready"]
    resources = terminal["reader_resources"]
    require(terminal["route"] == "shelf" and case["final"]["lifecycle_ready_ns"] == terminal["monotonic_ns"] and
            case["final"]["ready_covers"] == resources["cover_resident_count"] and
            case["final"]["resident_bytes"] == resources["cover_resident_bytes"], "Different readiness inventory")
    final = load(directory / "final.json")
    catalog = final["ui"]["catalog"]
    require(final["process_id"] == pid and final["route"] == "shelf" and final["offline_requested"] is True and
            final["offline"] is True and final["demo"] is False and final["backend"] == "desktop-software" and
            final["profile"] == "scribe" and not final["startup"]["error"] and not final["startup"]["booting"] and
            catalog["shelf_menu"] is True and catalog["shelf_page"] == 1 and catalog["shelf_pages"] == 2,
            "Semantic final state contradicts the observed click response")


def inspect(name, case):
    launch, enter = case["launcher_ns"], case["process_enter_ns"]
    require(case["case"] == name and all(type(value) is int and value > 0 for value in (case["pid"], case["xid"], launch, enter))
            and launch <= enter, "Invalid host origin, child entry or window identity")
    feedback, shelf, click, menu, final = (case[key] for key in ("feedback", "shelf", "input", "response", "final"))
    require(final["normal_exit"] is True and final["offline"] is True and final["demo"] is False and final["menu_open"] is True
            and (final["route"], final["backend"], final["profile"], final["shelf_page"], final["shelf_pages"]) == ("shelf", "desktop-software", "scribe", 1, 2)
            and (final["ready_covers"], final["resident_bytes"]) == (10, 5_324_800), "Wrong ordinary formal fixture or semantic menu state")
    require(click["point"] == [1708, 136] and click["pid_xid_verified"] is True and click["topmost_target_verified"] is True
            and click["server_press_verified"] is True and click["server_release_verified"] is True
            and click["focus"] in ("pointer-root", "owned-window", "display-root") and click["hold_target_ms"] == 16,
            "Tap did not target the qualified control on the owned window")

    loading, activity, feedback_miss = (None if feedback[key] is None else bounds(feedback, key) for key in ("title_ns", "activity_ns", "last_negative_ns"))
    shelf_miss = None if shelf["last_negative_ns"] is None else bounds(shelf, "last_negative_ns")
    bar, book, root_bar, root_book = (bounds(shelf, key) for key in ("bar_ns", "title_ns", "root_bar_ns", "root_title_ns"))
    closed, root_closed, last_negative, match, root_match, stable = (bounds(menu, key) for key in
        ("prepress_closed_ns", "prepress_root_closed_ns", "last_negative_ns", "first_match_ns", "root_match_ns", "confirmation_ns"))
    press, release = bounds(click, "press_ns"), bounds(click, "release_ns")
    require(enter <= bar[0] and ordered(bar, book, root_bar, root_book) and
            (shelf_miss is None or enter <= shelf_miss[0] <= shelf_miss[1] <= bar[0]), "Invalid non-cover Shelf reads")
    observed_feedback = [sample for sample in (feedback_miss, loading, activity) if sample is not None]
    require(all(enter <= sample[0] <= sample[1] <= bar[0] for sample in observed_feedback) and ordered(*observed_feedback),
            "Invalid optional feedback observations")
    require(ordered(root_book, closed, root_closed, press, release, match, root_match, stable) and
            (last_negative == root_closed or release[1] <= last_negative[0]) and last_negative[1] <= match[0]
            and release[0] - press[1] >= 16_000_000 and stable[0] - root_match[1] >= 80_000_000
            and type(final["lifecycle_ready_ns"]) is int and enter <= final["lifecycle_ready_ns"] <= stable[1],
            "Missing ordering from presented Shelf through fresh tap to stable response")
    visible, response = root_book[1], root_match[1]
    return {"launch_ns": launch, "end_ns": stable[1], "costs_ns": {
        "launch_to_feedback_upper": None if loading is None else loading[1] - launch,
        "launch_to_initial_activity_upper": None if activity is None else activity[1] - launch,
        "launch_to_shelf_lower": 0 if shelf_miss is None else shelf_miss[0] - launch,
        "launch_to_shelf_upper": visible - launch, "launch_to_press": press[0] - launch,
        "shelf_to_press_probe_overhead": press[0] - visible,
        "press_confirmed_to_release_request": release[0] - press[1], "press_to_release_return": release[1] - press[0],
        "release_return_to_response": response - release[1], "launch_to_response_upper": response - launch,
        "entry_to_response_upper": response - enter, "press_to_response_upper": response - press[0],
        "launch_to_response_pixels_lower_not_readiness": max(press[0], last_negative[0]) - launch,
        "launch_to_first_window_response_read_end": match[1] - launch}}


def visual_controls(report):
    geometry = report["geometry"]
    expected = {"shelf-bar": [80, 70, 1692, 130], "first-title": [96, 632, 300, 50], "menu-response": [1040, 632, 716, 70],
                "closed-response": [1040, 632, 716, 70], "menu-views": [1040, 356, 716, 130]}
    require(geometry["size"] == [1860, 2480] and geometry["open_point"] == [1708, 136] and
            geometry["qualification_only_close_point"] == [440, 140] and geometry["regions_xywh"] == expected and
            report["physical_device_used"] is False and report["isolated_xvfb_and_processes_retired"] is True, "Wrong visual qualification geometry or scope")
    cases = report["cases"]
    require([case["case"] for case in cases] == ["reference-no-click", "cached-click", "missing-click", "signed-out-no-click"],
            "Missing or repeated visual control")
    for data in cases:
        name = data["case"]
        clicked, signed_in = name in ("cached-click", "missing-click"), name != "signed-out-no-click"
        require(data["signed_in"] is signed_in and data["same_owned_window"] is True and data["normal_exit"] is True
                and data["catalog_titles_unchanged_before_launch"] == 20 and data["closed_populated_shelf_matched"] is signed_in
                and data["empty_control_login_prompt_matched"] is (not signed_in), f"{name}: ambiguous Shelf or identity")
        require(data["withheld_floor_ms"] >= 450 and data["withheld_observations"] >= 3 and data["open_response_observed_while_withheld"] is False
                and data["button_pairs"] == len(data["focus_at_click"]) == (2 if clicked else 0) and data["final_shelf_menu_open"] is False,
                f"{name}: invalid no-click or reversibility observation")
        require(all(data[key] is (True if clicked else None) for key in ("owned_window_and_root_menu_response_after_click", "opened_menu_still_visible_100ms_later", "closed_after_outside_click"))
                and data["ready_resident_covers"] == (0 if name == "missing-click" else 10)
                and data["visible_cover_codes"] == (list(range(1, 11)) if name in ("reference-no-click", "cached-click") else [None] * 10),
                f"{name}: unexpected menu response or cover control")
        if name == "missing-click":
            require(data["identical_labeled_placeholders"] == 10, "Missing-cover control did not show ten placeholders")
    return {"scope": "Independent untimed controls; no-input nonmatches apply only within their observation windows.",
            "qualified_cases": [case["case"] for case in cases]}


def summary(report, raw=None):
    require(report["schema"] == "ordinary-shelf-interaction-v1" and report["clock"] == "CLOCK_MONOTONIC" and
            report["poll_interval_ns"] == 4_000_000 and
            report["schedule"] == SCHEDULE and report["excluded_warmups"] == ["w00"] and set(report["cases"]) == set(SCHEDULE),
            "Expected exactly one warmup followed by six unchanged samples")
    if raw is not None:
        require(not (raw / "failure.json").exists() and {path.name for path in raw.iterdir() if path.is_dir()} == set(SCHEDULE),
                "Failed, incomplete or extended raw trial")
    checked = {}
    for name in SCHEDULE:
        case = report["cases"][name]
        if raw is not None:
            raw_case(raw / name, case)
        checked[name] = inspect(name, case)
    require(all(checked[before]["end_ns"] < checked[after]["launch_ns"] for before, after in zip(SCHEDULE, SCHEDULE[1:])),
            "Overlapping, repeated or reordered launches")
    measured = {name: checked[name]["costs_ns"] for name in SCHEDULE[1:]}
    def millis(value):
        return None if value is None else round(value / 1_000_000, 3)
    distributions = {}
    for key in next(iter(measured.values())):
        values = [metrics[key] for metrics in measured.values() if metrics[key] is not None]
        distributions[key] = {"observed": len(values), **{label: millis(statistic(values)) if values else None
                              for label, statistic in (("median", median), ("min", min), ("max", max))}}
    return {"scope": "Desktop host launch to an externally confirmed response on the same presented ordinary Shelf; no earliest-ready timestamp, cover-complete timing, native card or physical digitizer. The lower bound for response pixels is not a lower bound for interactivity.",
            "excluded_warmups": ["w00"], "cases_ms": {name: {key: millis(value) for key, value in metrics.items()} for name, metrics in measured.items()},
            "distribution_ms": distributions}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("results", type=Path)
    parser.add_argument("--raw", type=Path, help="Also check the existing private per-case records")
    parser.add_argument("--qualification", type=Path, help="Cross-check independent untimed visual controls")
    args = parser.parse_args()
    result = summary(load(args.results), args.raw)
    if args.qualification:
        result["visual_controls"] = visual_controls(load(args.qualification))
    print(json.dumps(result, indent=2))
