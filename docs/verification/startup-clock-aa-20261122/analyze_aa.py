"""Summarize a closed, anonymized native Shelf A/A projection. Does not contact a device."""

import argparse
import json
from pathlib import Path
from statistics import median


SCHEDULE = [(pair, arm) for pair, order in enumerate(("AB", "AB", "BA", "AB", "BA")) for arm in order]
CLOCKS = set("release_before release_after launch_entry pre_app_invocation rust_main_entry trace_started shelf_rendered shelf_submitted driver_completed state_produced state_consumed".split())
OPTIONAL = {"shelf_submitted", "driver_completed", "state_produced", "state_consumed"}
CONTRACT = set("same_run same_boot same_process same_launcher_and_elf qualified_shelf natural_exit native_restored lease_released".split())
SEGMENTS = {
    "card_to_launch_entry": ("card", "launch_entry"), "launch_entry_to_pre_app_invocation": ("launch_entry", "pre_app_invocation"),
    "pre_app_invocation_to_rust_main": ("pre_app_invocation", "rust_main_entry"), "rust_main_to_header": ("rust_main_entry", "trace_started"),
    "card_to_header": ("card", "trace_started"), "header_to_renderer": ("trace_started", "shelf_rendered"),
    "card_to_renderer": ("card", "shelf_rendered"), "header_to_state_producer": ("trace_started", "state_produced"),
    "state_producer_to_ui_observation": ("state_produced", "state_consumed"), "state_producer_to_renderer": ("state_produced", "shelf_rendered"),
    "renderer_to_send": ("shelf_rendered", "shelf_submitted"), "send_to_driver": ("shelf_submitted", "driver_completed"),
    "card_to_send": ("card", "shelf_submitted"), "card_to_driver": ("card", "driver_completed"),
}


def require(condition, message):
    if not condition:
        raise ValueError(message)


def unique(items):
    result = dict(items)
    require(len(result) == len(items), "Duplicate JSON key")
    return result


def difference(later, earlier):
    if later is None or earlier is None:
        return None
    return [later[0] - earlier[1], later[1] - earlier[0]]


def middle(values):
    return [median(value[index] for value in values) for index in (0, 1)] if values else None


def visit(row):
    require(set(row) == {"pair", "arm", "clock_offset_ns", "evidence"}, "Invalid anonymous visit fields")
    clock, evidence = row["clock_offset_ns"], row["evidence"]
    require(set(clock) == CLOCKS and set(evidence) == CONTRACT | {"accepted_send", "same_marker_driver"}, "Missing or unknown clock/evidence field")
    require(all(value is True for key, value in evidence.items() if key in CONTRACT), "Unbound, unqualified or unreleased visit")
    require(all((key in OPTIONAL and value is None) or (type(value) is int and value >= 0) for key, value in clock.items()), "Invalid device-clock offset")
    path = [clock[key] for key in ("release_before", "release_after", "launch_entry", "pre_app_invocation", "rust_main_entry", "trace_started", "shelf_rendered")]
    require(path == sorted(path), "Invalid launch landmark order")
    send, driver = clock["shelf_submitted"], clock["driver_completed"]
    require(evidence["accepted_send"] is (send is not None) and (send is None or send >= path[-1]), "Invalid accepted SEND")
    require(evidence["same_marker_driver"] is (driver is not None) and (driver is None or (send is not None and driver >= send)), "Invalid same-marker completion")
    produced, consumed = clock["state_produced"], clock["state_consumed"]
    require(produced is None or path[-2] <= produced <= path[-1], "State producer outside header-to-render interval")
    require(consumed is None or (produced is not None and produced <= consumed <= path[-1]), "Unbound UI consumption")
    stamps = {key: [value, value] if value is not None else None for key, value in clock.items()}
    stamps["card"] = path[:2]
    return stamps["card"], max(value for value in clock.values() if value is not None), {
        key: difference(stamps[end], stamps[start]) for key, (start, end) in SEGMENTS.items()}


def summarize_group(pairs):
    result = {}
    for key in SEGMENTS:
        included = [(first[key], second[key]) for _, first, second in pairs if first[key] is not None and second[key] is not None]
        diffs = [difference(second, first) for first, second in included]
        earlier = sum(delta[1] < 0 for delta in diffs)
        later = sum(delta[0] > 0 for delta in diffs)
        result[key] = {"pairs": len(diffs), "median_first_ns": middle([first for first, _ in included]),
                       "median_second_ns": middle([second for _, second in included]), "median_second_minus_first_ns": middle(diffs),
                       "second_earlier": earlier, "second_later": later, "indeterminate": len(diffs) - earlier - later}
    return result


def analyze(projection):
    require(type(projection) is dict and set(projection) == {"schema", "clock", "cases"} and
            projection["schema"] == "bili-shelf-clock-aa-projection-v1" and projection["clock"] == "CLOCK_MONOTONIC", "Unknown projection or clock")
    rows = projection["cases"]
    require(type(rows) is list and len(rows) <= len(SCHEDULE), "Invalid closed-prefix length")
    visits, pairs, gaps = [], [], []
    last_end = None
    for index, row in enumerate(rows):
        pair, arm = SCHEDULE[index]
        require(type(row.get("pair")) is int and (row["pair"], row.get("arm")) == (pair, arm), "Missing, repeated or reordered visit")
        release, last, timing = visit(row)
        require(release[0] == 0 if last_end is None else release[0] > last_end, "Wrong origin, overlapping visits or discontinuous clock")
        last_end = last
        if index > 2:
            previous = visits[-1]
            kind = "within_pair" if previous[0] == pair else "between_pairs"
            gaps.append({"from_pair": previous[0], "to_pair": pair, "kind": kind, "release_to_release_ns": difference(release, previous[2])})
        if pair > 0 and index % 2 == 1:
            pairs.append((visits[-1][1] + arm, visits[-1][3], timing))
        visits.append((pair, arm, release, timing))

    grouped = {group: summarize_group(pairs if group == "all" else [item for item in pairs if item[0] == group])
               for group in ("all", "AB", "BA")}
    gaps_summary = {}
    for kind in ("within_pair", "between_pairs"):
        intervals = [row["release_to_release_ns"] for row in gaps if row["kind"] == kind]
        gaps_summary[kind] = {"count": len(intervals), "median_ns": middle(intervals),
                              "min_lower_ns": min((row[0] for row in intervals), default=None),
                              "max_upper_ns": max((row[1] for row in intervals), default=None)}
    return {"scope": "Identical treatment; positional differences and cadence are descriptive, not a speedup or a device-contention measurement.",
            "closed_visits": len(visits), "warmups_excluded": min(2, len(visits)), "complete_measured_pairs": len(pairs),
            "unpaired_measured_visits": max(0, len(visits) - 2) % 2, "complete_series": len(rows) == len(SCHEDULE),
            "measured_visits": [{"pair": pair, "label": arm, "position": 1 + index % 2, "durations_ns": timing}
                                for index, (pair, arm, _, timing) in enumerate(visits[2:])],
            "measured_pairs": [{"pair": number, "order": order, "second_minus_first_ns": {
                key: difference(second[key], first[key]) for key in SEGMENTS}} for number, (order, first, second) in enumerate(pairs, 1)],
            "groups": grouped, "adjacent_measured_releases": gaps, "release_cadence": gaps_summary}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("projection", type=Path)
    projection = json.loads(parser.parse_args().projection.read_text(encoding="utf-8"), object_pairs_hook=unique)
    print(json.dumps(analyze(projection), indent=2))
