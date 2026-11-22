"""Replay a closed, qualified offline Library batch; emit timing-only JSON on stdout."""

import argparse
import json
from pathlib import Path
from statistics import median
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "library-to-shelf-20261119"))
from project_archived import bounds, one, require


def load(path):
    return json.loads(path.read_text())


def records(path):
    return [json.loads(line) for line in path.read_text().splitlines()]


def duration(stage):
    start, finish = bounds(stage)
    cpu_start, cpu_finish = stage["started_thread_cpu_ns"], stage["finished_thread_cpu_ns"]
    require(stage["same_thread"] is True and type(cpu_start) is type(cpu_finish) is int
            and 0 <= cpu_start <= cpu_finish and cpu_finish - cpu_start == stage["thread_cpu_ns"],
            "Invalid frame-stage CPU clocks")
    return finish - start, cpu_finish - cpu_start


def shelf_order(stages):
    wait, render, fingerprint, write, submit, complete = (bounds(stages[key]) for key in
        ("preceding_update_wait", "slint_render", "rgb_fingerprint", "framebuffer_write", "refresh_submit",
         "refresh_complete_wait"))
    require(render[1] <= fingerprint[0] and max(fingerprint[1], wait[1]) <= write[0]
            and write[1] <= submit[0] and submit[1] <= complete[0], "Unsettled mapping or invalid Shelf clocks")
    if wait[1] <= render[0]:
        return "wait_then_render"
    require(render[1] <= wait[0] and (fingerprint[1] <= wait[0] or wait[1] <= fingerprint[0]),
            "Overlapping stages on the UI thread")
    return "render_then_wait"


def project_case(path, published, expected_rgb, *, include_release_clock=False):
    final, click = (load(path / leaf) for leaf in ("final.json", "click.json"))
    perf, life = (records(path / leaf) for leaf in ("performance.jsonl", "lifecycle.jsonl"))
    pid = final["process_id"]
    require(type(pid) is int and pid > 0 and all(row["process_id"] == pid for row in perf + life),
            "Evidence contains a different process")
    require(final["backend"] == "kindle" and final["profile"] == "scribe" and final["demo"] is True
            and final["offline"] is True and final["route"] == "shelf" and final["suspended"] is False
            and final["startup"]["booting"] is False and final["startup"]["error"] is False
            and one((row for row in life if row["event"] == "ready"), "ready")["route"] == "shelf",
            "Not a completed offline Shelf")
    origin = click["up_before_ns"]
    require(click["clock"] == "CLOCK_MONOTONIC" and click["target"] == "open-card" and click["ok"] is True
            and click["mode"] == "XTest-button-1" and click["hold_requested_ms"] == 50 and click["down_confirmed"] is True
            and click["buttons_clear_after"] is True and click["up_sent"] is True and 0 < click["down_before_ns"]
            <= click["down_after_ns"] <= origin - 50_000_000 and origin <= click["up_after_ns"]
            and click["up_after_ns"] - origin == published["release_bracket_ns"], "Wrong release bracket")

    def main_stage(name):
        row = one((row for row in perf if (row["kind"], row["stage"]) == ("startup", name)), name)
        require(row["thread_id"] == pid, f"{name} not observed on the UI thread")
        return row

    ready = main_stage("ui_ready")["monotonic_ns"]
    enter = one((row["monotonic_ns"] for row in life if (row["event"], row.get("stage")) ==
                 ("startup-stage", "process_enter")), "process entry")
    prepared = main_stage("state_prepared")
    published_ns, consumed = prepared["data"]["observed"]["monotonic_ns"], prepared["monotonic_ns"]
    require(one((row["monotonic_ns"] for row in life if (row["event"], row.get("stage")) ==
                ("startup-stage", "state_prepared")), "prepared state") == published_ns, "Producer clocks differ")
    require(origin < enter <= published_ns <= consumed <= ready and ready - origin == published["shelf_ready_ns"]
            and enter - origin == published["process_entry_ns"], "Startup clocks disagree with the paired result")

    frames = [(line, row) for line, row in enumerate(perf, 1)
              if (row["kind"], row["stage"]) == ("native", "frame") and row["data"]["refresh_accepted"] is True]
    rendered = [(line, row["data"]) for line, row in frames if row["data"]["rendered"] is True]
    shelf = [(line, data) for line, data in rendered if bounds(data["stages"]["slint_render"])[0] >= ready]
    require(shelf, "No rendered frame after Shelf readiness")
    line, frame = shelf[0]
    require(perf[line - 1]["thread_id"] == pid, "Shelf recorded on a different thread")
    require(expected_rgb and frame["rgb_fingerprint"] == expected_rgb, "Not the published complete Shelf pixels")
    loading = [data for index, data in rendered if index < line]
    marker = frame["accepted_update_marker"]
    require(loading and loading[0]["initial_frame"] is True and loading[0]["render_sequence"] == 1
            and len(loading) == published["loading_frames"] and not frame["initial_frame"], "Wrong loading sequence")
    predecessors = [row["data"] for index, row in frames if index < line]
    preceding = predecessors[-1]
    prior_marker = preceding["accepted_update_marker"]
    require(type(prior_marker) is type(marker) is int and prior_marker > 0 and marker > 0
            and marker not in {data["accepted_update_marker"] for data in predecessors}
            and frame["preceding_update_marker"] == prior_marker and marker == published["marker"]
            and frame["completed_update_marker"] == marker and frame["driver_completion_observed"] is True
            and frame["completion_errno"] is None and frame["panel_completion_observed"] is False,
            "Wrong preceding or own update receipt")
    require(frame["complete_frame_replacement"] is True and frame["refresh_ioctl_returned"] is True
            and frame["refresh_ioctl_attempts"] >= 1 and frame["refresh_errno"] is None
            and frame["framebuffer_bits_per_pixel"] == 8 and frame["refresh_region"] ==
            {"x": 0, "y": 0, "width": 1860, "height": 2480}, "Not a successful complete Shelf submission")
    stages = frame["stages"]
    order = shelf_order(stages)
    wait, render, submit, complete = (bounds(stages[key]) for key in
        ("preceding_update_wait", "slint_render", "refresh_submit", "refresh_complete_wait"))
    begin, end = bounds(frame["timing"])
    require(bounds(preceding["stages"]["refresh_submit"])[1] <= begin and ready <= begin <= min(wait[0], render[0])
            and complete[1] <= end <= perf[line - 1]["monotonic_ns"], "Frame stages outside transaction clocks")
    require((submit[1] - origin, complete[1] - origin) == (published["send_ns"], published["driver_ns"]),
            "Shelf endpoints differ from the qualified paired result")
    splash = bounds(loading[0]["stages"]["refresh_submit"])[1]
    require(enter < splash <= ready and splash - origin == published["first_loading_send_ns"],
            "Wrong first loading submission")
    metrics = {"release_to_rust": enter - origin, "release_to_send": submit[1] - origin,
               "release_to_driver": complete[1] - origin, "loading_send_to_ready": ready - splash,
               "ready_to_render_start": render[0] - ready, "ready_to_send": submit[1] - ready,
               "send_to_driver": complete[1] - submit[1], "state_observed_to_ui_record": consumed - published_ns}
    for label, key in (("render", "slint_render"), ("previous_wait", "preceding_update_wait"),
                       ("fingerprint", "rgb_fingerprint"), ("write", "framebuffer_write")):
        metrics[label], metrics[label + "_cpu"] = duration(stages[key])
    if order == "wait_then_render":
        metrics["serial_render_wait_overlap_ceiling"] = min(metrics["previous_wait"], metrics["render"])
    third_wait = None
    if len(loading) >= 3:
        third_wait = bounds(loading[2]["stages"]["preceding_update_wait"])[0]
        require(splash <= third_wait <= ready, "Third loading wait is outside startup feedback")
        metrics["state_observed_minus_third_wait_start"] = published_ns - third_wait
    case = {"case": path.name, "pid": pid, "shelf_performance_line": line, "previous_marker": prior_marker,
            "shelf_marker": marker, "loading_frames": len(loading), "order": order, "ns": metrics,
            "state_clocks_ns": {"producer_observation": published_ns, "ui_record": consumed, "third_loading_wait_start": third_wait}}
    if include_release_clock:
        case["release_monotonic_ns"] = origin
    return case


def ms(nanoseconds):
    return round(nanoseconds / 1_000_000, 3)


def distribution(samples):
    keys = sorted(set.intersection(*(set(item["ns"]) for item in samples)))
    result = {}
    for key in keys:
        values = [item["ns"][key] for item in samples]
        result[key] = {"min": ms(min(values)), "median": ms(median(values)), "max": ms(max(values))}
    return result


def project(batch, report, *, include_release_clock=False):
    admission, released = (load(batch / leaf) for leaf in ("admission.json", "released.json"))
    require(admission["dry_only"] is False and not (batch / "failure.json").exists() and (batch / "uninstalled").is_file()
            and released["ok"] is True and released["state"] == "released" and released["owner_thread"] == admission["owner"]
            and released["run_id"] == batch.name == report["run"] == admission["run"] and admission["source"] == report["source"]
            and released["native_restored_and_connections_closed"] is True and released["owner_and_token_checked"] is True
            and all(report[key] is True for key in ("restored_native_tab_and_pointer", "installation_removed", "lease_released")),
            "Expected the matching closed real batch and qualified paired result")
    schedule = [arm + f"{pair:02}" for pair in range(7) for arm in ("ab" if pair == 0 or pair % 2 else "ba")]
    require(schedule == admission["schedule"] == report["schedule"] and set(report["cases"]) == set(schedule)
            and report["warmups_excluded"] == schedule[:2], "Invalid paired schedule")
    cases = [project_case(batch / name, report["cases"][name], report["common_shelf_rgb_fingerprint"],
                          include_release_clock=include_release_clock) for name in schedule[2:]]
    require(len({case["pid"] for case in cases}) == len(cases), "Reused measured process")
    groups = {f"{count}_loading_frames": [case for case in cases if case["loading_frames"] == count]
              for count in sorted({case["loading_frames"] for case in cases})}
    pairs = []
    for first, second in zip(cases[::2], cases[1::2]):
        control, candidate = sorted((first, second), key=lambda case: case["case"][0])
        pairs.append({"pair": int(first["case"][1:]), "order": (first["case"][0] + second["case"][0]).upper(),
                      "B_minus_A_ms": {key: ms(candidate["ns"][key] - control["ns"][key]) for key in
                        ("release_to_rust", "loading_send_to_ready", "ready_to_send", "render", "previous_wait",
                         "release_to_send", "release_to_driver")}})
    return {"run": admission["run"], "source": admission["source"], "scope": "Offline replay; no new launches or optical measurements.",
            "ceiling": "Only for wait-then-render: min(previous wait, private render) on that trace, excluding the diagnostic fingerprint. Not an observed or predicted speedup.",
            "warmups_excluded": schedule[:2], "all_cases_ms": distribution(cases), "by_loading_frames": {
                key: {"cases": [case["case"] for case in group], "ms": distribution(group)} for key, group in groups.items()},
            "pairs": pairs, "cases": cases}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("batch", type=Path, help="Closed case directory, optionally unpacked from the allowlisted archive")
    parser.add_argument("paired_result", type=Path)
    parser.add_argument("--include-release-clock", action="store_true", help="Include the checked same-case monotonic click origin for pre-ready analysis")
    args = parser.parse_args()
    print(json.dumps(project(args.batch, load(args.paired_result), include_release_clock=args.include_release_clock), indent=2))
