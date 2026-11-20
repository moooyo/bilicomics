#!/usr/bin/env python3
"""Project the six retained --library starts from the public 2026-11-18 archive."""

import hashlib
import json
from pathlib import Path
from statistics import median


SOURCE = Path(__file__).resolve().parents[1] / "native-library-entry-20261118" / "formal-ab"
FRAME_STAGES = ("preceding_update_wait", "slint_render", "rgb_fingerprint", "framebuffer_write",
                "refresh_submit", "refresh_complete_wait")


def require(condition, message):
    if not condition:
        raise ValueError(message)


def one(records, label):
    values = list(records)
    require(len(values) == 1, f"Expected one {label}, found {len(values)}")
    return values[0]


def bounds(stage):
    start, end = stage["started_monotonic_ns"], stage["finished_monotonic_ns"]
    require(type(start) is int and type(end) is int and 0 < start <= end
            and stage["wall_ns"] == end - start, "Invalid frame-stage clocks")
    return start, end


def project(case, item):
    require(case["case"] == item["case"] and case["variant"] == "candidate" and case["warmup"] is False,
            "Wrong primary case")
    return observed_times(case["final"], [r["fields"] for r in case["lifecycle_selected_records"]],
                          case["performance_selected_records"], case["request"]["monotonic_ns"], case["derived_endpoints"])


def observed_times(final, life, perf, origin, archive=None, *, private_shelf_render=False):
    require(final["backend"] == "kindle" and final["profile"] == "scribe" and final["demo"] is True
            and final["offline"] is True and final["route"] == "shelf" and final["startup"]["error"] is False
            and final["startup"]["booting"] is False and final["suspended"] is False, "Wrong final scene")
    enter = one((r for r in life if (r["event"], r.get("stage")) == ("startup-stage", "process_enter")),
                "process entry")["monotonic_ns"]
    require(one((r for r in life if r["event"] == "ready"), "lifecycle ready")["route"] == "shelf",
            "Ready route is not Shelf")
    ready_record = one((r for r in perf if (r["fields"]["kind"], r["fields"]["stage"]) == ("startup", "ui_ready")),
                       "performance ui_ready")
    ready = ready_record["fields"]["monotonic_ns"]
    frames = []
    for record in perf:
        row = record["fields"]
        if (row["kind"], row["stage"]) != ("native", "frame"):
            continue
        data = row["data"]
        if data["rendered"] is not True or data["refresh_accepted"] is not True:
            continue
        stages = data["stages"]
        render_start, render_end = bounds(stages["slint_render"])
        submit_start, send = bounds(stages["refresh_submit"])
        require(render_end <= submit_start <= send <= row["monotonic_ns"] and data["refresh_ioctl_returned"] is True
                and data["refresh_ioctl_attempts"] >= 1 and data["refresh_errno"] is None, "Unproven SEND")
        frames.append((send, record["source_line_1based"], render_start, data, row["monotonic_ns"]))
    splash, _, _, initial, _ = min(frames, key=lambda f: f[0])
    require(initial["initial_frame"] is True and initial["render_sequence"] == 1, "Missing initial app frame")
    send, line, _, shelf, recorded = min((f for f in frames if ready <= f[2]), key=lambda f: f[0])
    marker = shelf["accepted_update_marker"]
    require(shelf["complete_frame_replacement"] is True and shelf["framebuffer_bits_per_pixel"] == 8
            and shelf["refresh_region"] == {"x": 0, "y": 0, "width": 1860, "height": 2480},
            "First accepted Shelf frame is not a full Scribe frame")
    require(type(marker) is int and marker > 0 and shelf["completed_update_marker"] == marker
            and shelf["driver_completion_observed"] is True and shelf["completion_errno"] is None
            and shelf["panel_completion_observed"] is False, "Shelf completion is not bound to its SEND")
    stage_times = {k: bounds(shelf["stages"][k]) for k in FRAME_STAGES}
    ordered = (("slint_render", "rgb_fingerprint", "preceding_update_wait", "framebuffer_write",
                "refresh_submit", "refresh_complete_wait") if private_shelf_render else FRAME_STAGES)
    require(all(stage_times[a][1] <= stage_times[b][0] for a, b in zip(ordered, ordered[1:])),
            "Invalid Shelf stage order or overlapping transactions")
    if private_shelf_render:
        preceding = [data for when, _, _, data, _ in frames if when < send]
        previous_marker = shelf["preceding_update_marker"]
        require(preceding and type(previous_marker) is int and previous_marker > 0 and
                previous_marker == preceding[-1]["accepted_update_marker"] and previous_marker != marker and
                shelf["initial_frame"] is False, "Unproven preceding update for private Shelf render")
    complete = stage_times["refresh_complete_wait"][1]
    require(origin < enter <= splash < ready < send <= complete <= recorded, "Invalid endpoint order")
    if archive is not None:
        require((ready_record["source_line_1based"], ready, line, send) ==
                (archive["trace_ready_line_1based"], archive["ui_ready_monotonic_ns"], archive["trace_selected_frame_line_1based"],
                 archive["first_shelf_send_accepted_monotonic_ns"]), "Endpoint disagrees with archive")
    intervals = {"request_to_process_enter": enter - origin, "request_to_app_splash_send": splash - origin,
                 "request_to_ui_ready": ready - origin, "request_to_shelf_send": send - origin,
                 "request_to_shelf_driver_complete": complete - origin, "process_enter_to_ui_ready": ready - enter,
                 "ui_ready_to_shelf_send": send - ready, "shelf_send_to_driver_complete": complete - send}
    return marker, intervals, {k: end - start for k, (start, end) in stage_times.items()}


def ms(ns):
    return round(ns / 1_000_000, 3)


def distribution(rows):
    return {k: {label: ms(func([r[k] for r in rows])) for label, func in
                (("min", min), ("median", median), ("max", max))} for k in rows[0]}


def main():
    hashes = {path: h for h, path in (line.split("  ", 1) for line in (SOURCE / "SHA256SUMS").read_text().splitlines())}

    def load(name):
        data = (SOURCE / name).read_bytes()
        require(hashlib.sha256(data).hexdigest() == hashes[name], f"Archived SHA-256 mismatch: {name}")
        return json.loads(data)

    summary, provenance = load("summary.json"), load("provenance.json")
    chosen = [r for r in summary["cases"] if r["variant"] == "candidate" and r["warmup"] is False]
    require([r["case"] for r in chosen] == [f"candidate-{i:02}" for i in range(1, 7)] and
            summary["schedule"]["all_cases_retained"] is True and summary["schedule"]["outliers_removed"] is False,
            "Primary candidate schedule differs")
    arm = provenance["variants"]["candidate"]
    require(provenance["run_id"] == summary["run_id"] and arm["entry_arguments"] == ["--library"]
            and arm["startup_arguments"] == ["--demo", "--offline", "--screen", "shelf"], "Arm provenance differs")
    samples, intervals, stages = [], [], []
    for item in chosen:
        path = item["evidence"]
        require(path == f"cases/{item['dispatch_order_1based']:02}-{item['case']}.json", "Unexpected case path")
        marker, times, frame = project(load(path), item)
        samples.append({"case": item["case"], "source": path, "source_sha256": hashes[path], "shelf_marker": marker,
                        "interval_ms": {k: ms(v) for k, v in times.items()}, "shelf_frame_ms": {k: ms(v) for k, v in frame.items()}})
        intervals.append(times)
        stages.append(frame)
    print(json.dumps({
        "source": "../native-library-entry-20261118/formal-ab", "run": summary["run_id"],
        "summary_sha256": hashes["summary.json"], "provenance_sha256": hashes["provenance.json"],
        "runtime_commit": arm["build_configuration"]["runtime_source_commit"], "package_commit": arm["source_commit"],
        "application_sha256": arm["binary_sha256"], "scope": "Stored physical Scribe evidence, new processes with retained caches, offline demo Shelf. Six primary --library starts; no new launches, warmups or detached controls in this projection.",
        "origin": summary["endpoints"]["origin"], "native_uri": summary["endpoints"]["native_uri"],
        "endpoints": {"app_splash_send": "Initial accepted rendered application frame, not earlier native handler feedback.",
                      "ui_ready": "Unique performance startup/ui_ready clock; the separate lifecycle Ready record confirms Shelf.",
                      "shelf_send": summary["endpoints"]["first_shelf_send"],
                      "shelf_driver_complete": "End of the driver's checked wait for the positive accepted Shelf marker, not the later trace-record timestamp."},
        "limits": "No finger-touch timestamp, optical observation, complete-cover or input-ready endpoint. Includes profiling (RGB fingerprinting).",
        "statistics": "Milliseconds rounded to 0.001 after calculation from monotonic nanoseconds. Each median uses its own six per-run spans; marginal medians are not additive.",
        "interval_ms": distribution(intervals), "shelf_frame_ms": distribution(stages), "samples": samples,
    }, indent=2))


if __name__ == "__main__":
    main()
