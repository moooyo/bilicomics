"""Describe pre-ready intervals from closed frame and native-card projections, without pooling batches."""

import argparse
import json
from pathlib import Path
from statistics import median


ROOT = Path(__file__).resolve().parent
DEFAULT_BATCHES = (
    (ROOT / "november20-frames.json", ROOT.parent / "library-pairs-20261120" / "paired-result.json"),
    (ROOT / "frames.json", ROOT / "paired-result.json"),
)
SCHEDULE = [f"{arm}{number:02d}" for number in range(7) for arm in ("ab" if number == 0 or number % 2 else "ba")]


def require(condition, message):
    if not condition:
        raise ValueError(message)


def integer(mapping, key):
    value = mapping.get(key)
    require(type(value) is int, f"Expected integer nanoseconds or count for {key}")
    return value


def case_times(frame, card):
    name = frame["case"]
    count = integer(frame, "loading_frames")
    spans, clocks = frame["ns"], frame["state_clocks_ns"]
    process, feedback, ready, send, driver = (integer(card, key) for key in (
        "process_entry_ns", "first_loading_send_ns", "shelf_ready_ns", "send_ns", "driver_ns"))
    require(0 < process < feedback <= ready <= send <= driver, f"Invalid card-relative timeline: {name}")
    require(count > 0 and count == integer(card, "loading_frames") and
            integer(frame, "shelf_marker") == integer(card, "marker"), f"Different Shelf frame: {name}")
    expected = {"release_to_rust": process, "release_to_send": send, "release_to_driver": driver,
                "loading_send_to_ready": ready - feedback, "ready_to_send": send - ready}
    require(all(integer(spans, key) == value for key, value in expected.items()),
            f"Frame and card intervals disagree: {name}")

    producer, consumer = (integer(clocks, key) for key in ("producer_observation", "ui_record"))
    lag = consumer - producer
    require(producer > 0 and lag >= 0 and lag == integer(spans, "state_observed_to_ui_record"),
            f"Invalid producer/UI clocks: {name}")
    third = clocks["third_loading_wait_start"]
    after_third = None
    if count >= 3:
        require(type(third) is int and third > 0, f"Missing third loading wait: {name}")
        after_third = producer - third
        require(after_third == integer(spans, "state_observed_minus_third_wait_start"),
                f"Different third loading wait: {name}")
    else:
        require(third is None, f"Unexpected third loading wait: {name}")

    before_producer = after_consumer = None
    relation = "at_or_after_feedback" if after_third is not None and after_third >= 0 else "unresolved"
    if "release_monotonic_ns" in frame:
        origin = integer(frame, "release_monotonic_ns")
        require(origin > 0 and origin + process <= producer <= consumer <= origin + ready and
                (third is None or origin + feedback <= third <= origin + ready),
                f"Release anchor disagrees with timeline: {name}")
        before_producer = producer - (origin + feedback)
        after_consumer = origin + ready - consumer
        relation = "before_feedback" if before_producer < 0 else "at_or_after_feedback"

    return {"case": name, "loading_frames": count, "producer_vs_feedback": relation,
            "ns": {"release_to_process": process, "release_to_feedback": feedback,
                   "process_to_feedback": feedback - process, "feedback_to_ready": ready - feedback,
                   "feedback_to_producer_signed": before_producer, "producer_to_ui_record": lag,
                   "ui_record_to_ready": after_consumer, "signed_outer_sum": ready - feedback - lag,
                   "producer_minus_third_wait_start": after_third}}


def ms(duration):
    return None if duration is None else round(duration / 1_000_000, 6)


def distribution(cases):
    fields = {}
    for key in cases[0]["ns"]:
        values = [case["ns"][key] for case in cases if case["ns"][key] is not None]
        summary = {"available": len(values), "min": None, "median": None, "max": None}
        if len(values) == len(cases):
            summary.update(min=ms(min(values)), median=ms(median(values)), max=ms(max(values)))
        fields[key] = summary
    return {"cases": [case["case"] for case in cases], "ms": fields}


def analyze(frames, cards):
    require(frames["run"] == cards["run"] and frames["source"] == cards["source"],
            "Different run or source identities")
    require(frames["warmups_excluded"] == cards["warmups_excluded"] == SCHEDULE[:2] and
            cards["schedule"] == SCHEDULE and set(cards["cases"]) == set(SCHEDULE),
            "Expected two warmups and six pairs")
    by_name = {frame["case"]: frame for frame in frames["cases"]}
    require(len(by_name) == len(frames["cases"]) and set(by_name) == set(SCHEDULE[2:]), "Missing or repeated measured case")
    cases = [case_times(by_name[name], cards["cases"][name]) for name in SCHEDULE[2:]]
    candidate = [case for case in cases if case["case"].startswith("b")]
    slow_half = sorted(candidate, key=lambda case: case["ns"]["feedback_to_ready"])[-3:]
    short_handoff = min(slow_half, key=lambda case: case["ns"]["producer_to_ui_record"])
    long_handoff = max(candidate, key=lambda case: case["ns"]["producer_to_ui_record"])
    return {"run": frames["run"], "source": frames["source"], "warmups_excluded": SCHEDULE[:2],
            "candidate": distribution(candidate), "both_arms_by_loading_frames": {
                count: distribution([case for case in cases if case["loading_frames"] == count])
                for count in sorted({case["loading_frames"] for case in cases})},
            "candidate_examples": {"shortest_handoff_among_three_slowest_to_ready": short_handoff["case"],
                                   "longest_handoff": long_handoff["case"]},
            "cases": [{"case": case["case"], "loading_frames": case["loading_frames"],
                       "producer_vs_feedback": case["producer_vs_feedback"],
                       "ms": {key: ms(value) for key, value in case["ns"].items()}} for case in cases]}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--batch", nargs=2, action="append", type=Path, metavar=("FRAMES", "PAIRED"),
                        help="repeat for independent batches; defaults to November 20 and 21")
    batches = []
    for frame_path, paired_path in parser.parse_args().batch or DEFAULT_BATCHES:
        batches.append(analyze(json.loads(frame_path.read_text(encoding="utf-8")),
                               json.loads(paired_path.read_text(encoding="utf-8"))))
    require(len({batch["run"] for batch in batches}) == len(batches), "Repeated batch")
    print(json.dumps({"scope": "Closed instrumented offline Shelf runs; frame counts describe groups, not causes or savings.",
                      "clock": "Card-relative intervals can be aligned to state CLOCK_MONOTONIC stamps only when the frame case includes release_monotonic_ns.",
                      "signed_outer_sum": "(feedback to producer) + (UI record to ready); not worker time. The first term may be negative.",
                      "availability": "Null means unidentified; group statistics require the field in every listed case.",
                      "batches": batches}, indent=2))


if __name__ == "__main__":
    main()
