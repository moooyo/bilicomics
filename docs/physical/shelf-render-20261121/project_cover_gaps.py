"""Project cover attachment gaps and selected worker spans from either closed Shelf study."""

import argparse
import json
from pathlib import Path
from statistics import median

from project_frames import bounds, load, project, records, require


WORKER_STAGES = ("state-catalog", "state-covers", "state-demo-load", "state-demo-seed")


def project_case(batch, case):
    rows = records(batch / case["case"] / "performance.jsonl")
    pid = case["pid"]
    producer, consumer = (case["state_clocks_ns"][key] for key in ("producer_observation", "ui_record"))
    ready = case["release_monotonic_ns"] + case["ns"]["release_to_send"] - case["ns"]["ready_to_send"]

    def stage(name):
        return [row for row in rows if (row["kind"], row["stage"]) == ("startup", name) and row["monotonic_ns"] <= ready]

    worker = {}
    for name in WORKER_STAGES:
        span = stage(name)
        require([row["data"]["phase"] for row in span] == ["begin", "end"], f"Missing or repeated {name} span")
        begin, end = span
        require(begin["process_id"] == end["process_id"] == pid and begin["thread_id"] == end["thread_id"] != pid
                and begin["monotonic_ns"] <= end["monotonic_ns"] <= producer and end["data"]["outcome"] in ("success", "partial")
                and type(end["data"]["wall_us"]) is int and end["data"]["wall_us"] >= 0, f"Invalid worker span: {name}")
        worker[name] = end["data"]["wall_us"] / 1000

    chunks = stage("attach-covers")
    require([row["data"]["phase"] for row in chunks] == ["begin", "end", "begin", "end"]
            and [row["data"]["outcome"] for row in chunks[1::2]] == ["partial", "success"]
            and all(row["process_id"] == row["thread_id"] == pid for row in chunks), "Expected two UI cover chunks")
    first, gap_start, gap_end, last = (row["monotonic_ns"] for row in chunks)
    require(consumer <= first <= gap_start <= gap_end <= last <= ready, "Cover chunks outside pre-ready attachment")
    frames, previous = [], None
    for row in rows[:case["shelf_performance_line"] - 1]:
        if (row["kind"], row["stage"]) != ("native", "frame") or not row["data"]["rendered"]:
            continue
        frame = row["data"]
        start, end = bounds(frame["timing"])
        if gap_start < end and start < gap_end:
            require(row["process_id"] == row["thread_id"] == pid and gap_start <= start <= end <= gap_end,
                    "Frame on a different thread or straddling a cover chunk")
            wait_start, wait_end = bounds(frame["stages"]["preceding_update_wait"])
            require(start <= wait_start <= wait_end <= end, "Wait outside frame")
            changed = None
            if frame["refresh_accepted"]:
                require(previous is not None and frame["preceding_update_marker"] == previous["accepted_update_marker"] > 0
                        and frame["accepted_update_marker"] > 0 and all(isinstance(item.get("rgb_fingerprint"), str)
                        and len(item["rgb_fingerprint"]) == 16 for item in (previous, frame)), "Missing preceding accepted RGB frame")
                changed = frame["rgb_fingerprint"] != previous["rgb_fingerprint"]
            frames.append({"sequence": frame["render_sequence"], "accepted": frame["refresh_accepted"], "changed_rgb": changed,
                           "flashing": frame["refresh_flashing"], "full_replacement": frame["complete_frame_replacement"],
                           "region": frame["refresh_region"], "previous_wait_ms": (wait_end - wait_start) / 1e6})
        if frame["refresh_accepted"]:
            previous = frame
    return {"case": case["case"], "worker_wall_ms": worker, "cover_gap_ms": (gap_end - gap_start) / 1e6, "frames_in_gap": frames}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("batch", type=Path)
    parser.add_argument("paired_result", type=Path)
    args = parser.parse_args()
    verified = project(args.batch, load(args.paired_result), include_release_clock=True)
    cases = [project_case(args.batch, case) for case in verified["cases"]]
    candidate = [case for case in cases if case["case"].startswith("b")]
    print(json.dumps({"run": verified["run"], "candidate_worker_wall_median_ms": {
        name: round(median(case["worker_wall_ms"][name] for case in candidate), 6) for name in WORKER_STAGES}, "cases": cases}, indent=2))
