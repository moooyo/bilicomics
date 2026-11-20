#!/usr/bin/env python3
"""Read a closed, isolated 1+5 research batch on test-env; emit timing-only JSON.

Usage: python3 -B project_fresh.py BATCH_ROOT [--failed-only]
No device, control command, private token/owner file, or network access.
"""

import argparse
import hashlib
import json
from pathlib import Path
import re

from project_archived import distribution, ms, observed_times, require


CASES = ["candidate-warmup", *(f"candidate-{i:02}" for i in range(1, 6))]
DOC = "/mnt/us/documents/bilicomics.sh"
APP = "/mnt/us/bilicomics/app/bilicomics"
RESTORED = ("native_restored", "transport_closed", "lease_released", "same_boot", "install_unchanged",
            "catalog_restored", "registration_unchanged", "temporary_stage_removed", "lighting_restored", "input_roster_unchanged")
RECOVERED = ("passed", "native_restored", "owned_processes_gone", "production_lock_absent", "entry_seen", "worker_seen",
             "generation_bound", "content_observed", "entry_alive_at_content", "entry_gone", "worker_gone",
             "exit_status_observed", "entry_status_observed")
RELEASED = ("ok", "native_restored_and_connections_closed", "owner_and_token_checked",
            "only_owned_owner_json_removed_then_rmdir", "lease_directory_absent_after_release")
PACKED_SHELF_SHA = "12ff9def5d77c72f473a4703635a192e9c79efaa71420605a5145bbc436dacc3"
LOGICAL_SHELF_SHA = "0a0df97df88bbb46149381fa1c995c945d81a8b6179c28ec66094cba100d19da"


class Sources:
    def __init__(self, path):
        self.path = path.resolve(strict=True)
        self.hashes = {}

    def read(self, name, limit=1_048_576):
        path = self.path / name
        require(path.is_file() and path.resolve(strict=True) == path and 0 < path.stat().st_size <= limit,
                f"Invalid source file: {name}")
        data = path.read_bytes()
        require(0 < len(data) <= limit, f"Unbounded source file: {name}")
        self.hashes[name] = hashlib.sha256(data).hexdigest()
        return data

    def json(self, name):
        return json.loads(self.read(name))

    def records(self, name):
        return [json.loads(row) for row in self.read(name, 8_388_608).splitlines()]

    def exists(self, name):
        return (self.path / name).exists()


def all_true(record, fields):
    for key in fields:
        require(record.get(key) is True, f"Missing or negative proof: {key}")


def positive(value):
    require(type(value) is int and value > 0, "Expected a positive integer clock or identity")
    return value


def confirmed_command(src, report, name, stdout):
    commands = [c for c in report["commands"] if c["name"] == name]
    require(len(commands) == 1 and commands[0]["returncode"] == 0 and commands[0]["timeout"] is False
            and commands[0]["interrupted"] is False and src.read(f"evidence/{name}.stdout", 128) == stdout,
            f"Missing successful native guard: {name}")


def check_click(record, origin, enter):
    require(record["clock"] == "CLOCK_MONOTONIC" and record["mode"] == "XTest-button-1"
            and record["hold_requested_ms"] == 50, "Unexpected click observer or clock")
    all_true(record, ("ok", "down_confirmed", "up_sent", "buttons_clear_after"))
    origins = {k: positive(record[k + "_ns"]) for k in ("down_before", "down_after", "up_before", "up_after")}
    require(origin <= origins["down_before"] <= origins["down_after"] < origins["up_before"] <= origins["up_after"] <= enter,
            "Input injection brackets do not precede the application process")
    return origins


def check_capture(src, complete):
    start = positive(src.json("qualified-capture-request.json")["monotonic_ns"])
    end = positive(src.json("qualified-capture-complete.json")["monotonic_ns"])
    require(complete <= start <= end, "Final memory capture not after first Shelf driver completion")
    data = src.read("qualified-frame.raw", 4_613_824)
    header, separator, pixels = data.partition(b"\n")
    require(separator and len(header) < 1024, "Missing bounded framebuffer header")
    geom = json.loads(header)
    require(geom["read_only"] is True and (geom["width"], geom["height"], geom["bits_per_pixel"]) == (1860, 2480, 8)
            and geom["visible_bytes"] == len(pixels) == 1860 * 2480, "Wrong packed Gray8 extent")
    digest = hashlib.sha256(pixels).hexdigest()
    return {"width": 1860, "height": 2480, "visible_gray8_sha256": digest,
            "matches_qualified_offline_shelf": digest == PACKED_SHELF_SHA,
            "driver_complete_to_capture_request_ms": ms(start - complete), "capture_bracket_ms": ms(end - start),
            "scope": "Later read-only framebuffer memory from the last case, not an optical observation or timing endpoint."}


def case_row(src, report, name):
    route = report["route"]
    meta = report["metadata_observations"][name]
    for side in ("catalog_before", "catalog_after"):
        item = meta[side]
        require((item["p_location"], item["p_mimeType"], item["p_titles_0_nominal"]) ==
                (DOC, "text/x-shellscript", "BiliComics for Kindle"), "Unexpected indexed entry")
    recovery = src.json("recovery.json")
    all_true(recovery, RECOVERED)
    require(recovery["reason"] == "normal-exit" and recovery["worker_signal_sent"] is False
            and type(recovery["worker_exit_status"]) is int and recovery["worker_exit_status"] == 0
            and type(recovery["entry_status"]) is int and recovery["entry_status"] == 0, "Not a normal recovered exit")
    identity = (positive(recovery["application_pid"]), positive(recovery["application_birth"]))
    proc = src.read("application.pid", 512).decode("ascii").split()
    require(len(proc) == 6 and proc[-1] == APP and [str(x) for x in identity] == proc[:2], "Process binding differs")
    life, perf, final = src.records("lifecycle.jsonl"), src.records("performance.jsonl"), src.json("final.json")
    require(final["process_id"] == identity[0] and all(r["process_id"] == identity[0] for r in life), "Mixed process IDs")
    require(all(r["event"] not in ("suspend", "resume") for r in life), "Sleep or resume during measurement")
    origin = positive(src.json("request.json")["monotonic_ns"])
    stable = positive(src.json("native-stable-start.json")["monotonic_ns"])
    require(origin - stable >= 8_000_000_000, "Missing excluded native-stable dwell")
    marker, spans, stages = observed_times(final, life, [{"fields": r, "source_line_1based": i}
                                         for i, r in enumerate(perf, 1)], origin)
    endpoints = {k.removeprefix("request_to_"): origin + t for k, t in spans.items() if k.startswith("request_to_")}
    if route == "click":
        origins = check_click(src.json("click.json"), origin, endpoints["process_enter"])
        groups = {"xtest_" + label + "_to": {k: end - start for k, end in endpoints.items()} for label, start in origins.items()}
        groups["input_brackets"] = {"down_xsync": origins["down_after"] - origins["down_before"],
                                    "up_xsync": origins["up_after"] - origins["up_before"],
                                    "down_before_to_up_before": origins["up_before"] - origins["down_before"]}
    else:
        require(not src.exists("click.json"), "Unexpected click record for a native URI run")
        groups = {"native_uri_request_to": {k: end - origin for k, end in endpoints.items()}}
    groups["application_intervals"] = {k: v for k, v in spans.items() if not k.startswith("request_to_")}
    groups["shelf_frame_stages"] = stages
    result = {"case": name, "shelf_accepted_and_completed_marker": marker,
              "ms": {label: {k: ms(v) for k, v in values.items()} for label, values in groups.items()}}
    if name == CASES[-1]:
        result["final_framebuffer"] = check_capture(src, endpoints["shelf_driver_complete"])
    result["source_sha256"] = src.hashes
    return result, groups, identity, (origin, endpoints["shelf_driver_complete"])


def project(path, failed_only=False):
    src = Sources(path)
    report = src.json("report.json")
    require(report.get("finished_at_utc") and report.get("lease_released") is True, "Batch is not closed and released")
    run, route = report["run_id"], report["route"]
    require(re.fullmatch(r"lib-t-1119-[0-9a-f]{8}", run) and route in ("uri", "click"), "Unexpected run identity")
    release = src.json("evidence/lease-released.json")
    require(release["run_id"] == run and release["operation"] == "release" and release["state"] == "released",
            "Release receipt does not bind this run")
    all_true(release, RELEASED)
    base = {"schema_version": 1, "run": run, "route": route, "source_sha256": src.hashes,
            "exact_lease_released_after_restoration": True}
    if failed_only:
        require(report["success"] is False, "This is not a failed run")
        return {**base, "status": "failed", "completed_cases": [n for n in CASES if n in report.get("completed_cases", [])],
                "scope": "Failure receipt only; no timing samples included."}
    all_true(report, ("success", *RESTORED))
    require(report["cases"] == report["completed_cases"] == CASES and report["first_in_batch"] == CASES[0]
            and report["repeated_launches"] == CASES[1:] and report["primary_rounds"] == 5
            and report.get("cancelled") is False and report["native_uri"] == "file://" + DOC,
            "Incomplete or altered 1+5 schedule; no aggregate")
    restoration = {k: True for k in RESTORED}
    if route == "click":
        all_true(report, ("original_native_tab_restored",))
        confirmed_command(src, report, "click-restore-recovery", b"native-tab-and-pointer-restored\n")
        restoration["original_native_tab_restored"] = True
    admission = src.json("admission.json")
    require((admission["run_id"], admission["route"], admission["first_case"], admission["warm_repetitions"]) ==
            (run, route, CASES[0], CASES[1:]), "Admission does not bind this route or schedule")
    plugin = admission["plugin_sha256"]
    require((route == "uri" and plugin == {}) or (route == "click" and {"plugin.py", "assets/dispatch.sh"} <= plugin.keys()
            and all(re.fullmatch(r"[a-f0-9]{64}", v) for v in plugin.values())), "Route/plugin admission mismatch")
    arm = report["variants"]["candidate"]
    require(arm["startup_arguments"] == ["--demo", "--offline", "--screen", "shelf"] and arm["entry_arguments"] == ["--library"]
            and arm["binary_sha256"] == "04fc6f93b4327911ca1a3bb5200175c9f2e13fb21ac5e56280012ff44b6e2e59"
            and arm["source_commit"] == "39fac822a982868078201e605e50bdacf1dcf3ed", "Unexpected candidate or fixture")
    rows, repeated, identities, previous = [], [], set(), 0
    for name in CASES:
        case_src = Sources(src.path / "outputs" / "out" / name)
        row, groups, proc, (origin, end) = case_row(case_src, report, name)
        case_src.read("final.png", 8_388_608)
        row["final_logical_png_matches_archived_demo_shelf"] = case_src.hashes["final.png"] == LOGICAL_SHELF_SHA
        if route == "click":
            confirmed_command(src, report, "click-prepare-" + name, b"library-and-expected-bili-card-confirmed\n")
            row["library_tab_and_named_card_confirmed_before_input"] = True
        require(proc not in identities and origin > previous, "Reused process or overlapping launches")
        identities.add(proc)
        previous = end
        rows.append(row)
        if name != CASES[0]:
            repeated.append(groups)
    return {**base, "status": "complete", "application_sha256": arm["binary_sha256"], "package_sha256": arm["package_sha256"],
            "runtime_commit": arm["build_configuration"]["runtime_source_commit"], "package_commit": arm["source_commit"],
            "controller_sha256": admission["adapter_sha256"], "click_dispatch_sha256": plugin.get("assets/dispatch.sh"),
            "native_uri": "file://" + DOC, "distinct_application_pid_and_birth_pairs": len(identities),
            "restoration": restoration, "endpoints": {
                "origin": ("Kindle CLOCK_MONOTONIC before request-record close and native URI dispatch; not a tap." if route == "uri" else
                           "Kindle CLOCK_MONOTONIC around XTest button-1 down and up, before XTest send and after XSync. Primary origin: up_before. Not evdev, hardware touch, or a digitizer timestamp."),
                "app_splash_send": "Initial accepted rendered application frame; native handler may have already shown feedback.",
                "ui_ready": "Unique performance startup/ui_ready, with separate lifecycle ready and final Shelf route checks.",
                "shelf_send": "Earliest accepted SEND after Slint renders at or after ui_ready, verified as a full 1860 by 2480 Gray8 replacement.",
                "shelf_driver_complete": "End of the matching marker's successful driver wait, not optical completion."},
            "scope": "Single Scribe, new processes with retained caches, offline demo. First launch separated; no reboot or cache eviction. Not live-account, all-cover, optical, or input-ready timing. Includes diagnostic RGB fingerprinting.",
            "statistics": "Milliseconds rounded after calculation in nanoseconds. Min, median, max are for the five repeated cases only; marginal medians are not additive.",
            "first_in_batch": rows[0], "repeated_summary_ms": {k: distribution([sample[k] for sample in repeated]) for k in repeated[0]},
            "repeated_samples": rows[1:]}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("batch", type=Path)
    parser.add_argument("--failed-only", action="store_true", help="Emit a closed failed-batch receipt without timing")
    args = parser.parse_args()
    print(json.dumps(project(args.batch, args.failed_only), indent=2))
