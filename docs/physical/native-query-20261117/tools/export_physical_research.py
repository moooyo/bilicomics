#!/usr/bin/env python3
"""Export saved native-query research evidence without running tests or apps."""

from collections import Counter
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import statistics

from PIL import Image


EXPECTED = {
    "report": "8d633227a9e9f7bfb36728aea5c6ac09e17c07f146c5f1173b96a6ca8542edac",
    "primary": "1475464028f82974e5f58ed0175ff66730bdd162c1c58e55cc9b1b3acc4bb577",
    "projector": "e991f01792fa9b771d12a64c18e450c1435c7aca7ca24d8f22a374e451fbc01e",
    "analyzer": "55e80ecb8bc0c51b3da536bab29c0ea67074db27f00861685a4d648b858e4a28",
    "control2": "fd0876fc6d6f3202cffa37a7ec1089d3b02dab21d9b3e1d4448085fefc0189ac",
    "compiler": "ef366827161fc01b775c24c85bbbec4fe4ac72d02c28404285fc5f8a13d7ff6d",
    "software_identity": "444ff551e260ac7132bcd8c7ea805d4dd2a5ead9144f0ddddee341a7fe3ed23e",
}
PREFLIGHT = ("lease-contention-r2.json", "partial-batch-r3.json", "transport-failure-r1.json")
FRAME_KEYS = ("send_accepted_after_request_ms", "record_after_request_ms", "refresh_accepted",
              "refresh_ioctl_returned", "refresh_ioctl_attempts", "refresh_abi", "refresh_waveform",
              "refresh_flashing", "accepted_update_marker", "completed_update_marker",
              "driver_completion_observed", "panel_completion_observed", "rendered", "render_sequence")
VARIANT_KEYS = ("source_commit", "source_fingerprint", "binary_sha256", "worker_sha256", "package_sha256",
                "native_query_sha256", "deployed_native_query_sha256", "launcher_sha256",
                "instrumented_launcher_sha256", "compiler_fingerprint", "build_configuration")
RAW_KEYS = ("width", "height", "virtual_width", "virtual_height", "xoffset", "yoffset",
            "bits_per_pixel", "line_length", "smem_len", "visual", "grayscale", "rotate",
            "red_offset", "red_length", "red_msb_right", "green_offset", "green_length",
            "green_msb_right", "blue_offset", "blue_length", "blue_msb_right", "byte_order",
            "visible_bytes", "read_only")


def require(value, message):
    if not value:
        raise AssertionError(message)


def digest(path):
    output = hashlib.sha256()
    with Path(path).open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            output.update(chunk)
    return output.hexdigest()


def byte_digest(payload):
    return hashlib.sha256(payload).hexdigest()


def load(path, expected=None):
    if expected is not None:
        require(digest(path) == expected, "A selected saved source changed")
    return json.loads(Path(path).read_text(encoding="utf-8"))


def lines(path, expected=None):
    if expected is not None:
        require(digest(path) == expected, "A selected saved trace changed")
    return [json.loads(line) for line in Path(path).read_text(encoding="utf-8").splitlines() if line.strip()]


def write_json(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("x", encoding="utf-8", newline="\n") as target:
        json.dump(value, target, indent=2, ensure_ascii=False)
        target.write("\n")


def stats(values):
    return {"n": len(values), "values": values, "minimum": min(values), "maximum": max(values),
            "median": round(statistics.median(values), 6), "mean": round(statistics.mean(values), 6)}


def privacy_check(value):
    forbidden = {"owner", "owner_thread", "lease_token", "token", "nonce", "account", "account_id",
                 "credential", "password", "private_key", "cookie", "session"}
    if isinstance(value, dict):
        require(not (set(key.lower() for key in value) & forbidden), "Private identity fields cannot be exported")
        for item in value.values():
            privacy_check(item)
    elif isinstance(value, list):
        for item in value:
            privacy_check(item)
    elif isinstance(value, str):
        require(not any(marker in value for marker in ("SESSDATA=", "bili_jct=", "BEGIN PRIVATE KEY")),
                "Private values cannot be exported")


def main(args):
    batch = args.batch_root.resolve(strict=True)
    software = args.software_root.resolve(strict=True)
    projection_root = software / "primary-projection-97f1de5a"
    tools = software / "measurement-tools-r2"
    report = load(batch / "report.json", EXPECTED["report"])
    primary = load(projection_root / "primary-qualified-projection.json", EXPECTED["primary"])
    privacy_check(primary)
    require(report["success"] is False and primary["whole_batch_success"] is False
            and primary["whole_batch_status"] == "failed" and primary["status"] == "primary_qualified_projection",
            "This research must retain the failed whole-batch status")
    require(primary["source_report_sha256"] == EXPECTED["report"]
            and primary["frozen_analyzer_sha256"] == EXPECTED["analyzer"]
            and primary["projection_script_sha256"] == EXPECTED["projector"], "The independent projection has another source")
    frozen = load(tools / "frozen-tools-r2.json", EXPECTED["control2"])
    identity = load(software / "software-identity-r2/summary.json", EXPECTED["software_identity"])
    require(identity["passed"] is True, "The selected source/software identity collection changed")
    require(frozen["verification"] == {"controller_tests_executed": 33, "failed": 0,
            "unmodified_prior_abi_probe_tests": 27, "unmodified_prior_analysis_and_clock_tests": 65,
            "all_120_prior_tests_rerun": False}, "The recorded host-test inventory changed")
    require(len(primary["primary_rows"]) == 14 and primary["measured_pairs"] == 6
            and primary["warmups_excluded_from_statistics"] == 2, "The predefined primary sample set changed")
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    files = []

    def remember(path, role, source_tag, source_sha=None, exact=False):
        files.append({"path": path.relative_to(output).as_posix(), "bytes": path.stat().st_size,
                      "sha256": digest(path), "role": role, "source_tag": source_tag,
                      "source_sha256": source_sha, "original_bytes": exact})

    def copy(original, name, role, source_tag, expected=None):
        require(original.is_file() and not original.is_symlink(), "The selected artifact is not a regular file")
        source_sha = digest(original)
        if expected is not None:
            require(source_sha == expected, "The selected frozen source changed")
        target = output / name
        target.parent.mkdir(parents=True, exist_ok=True)
        require(not target.exists(), "Refusing to overwrite an exported artifact")
        shutil.copyfile(original, target)
        require(digest(target) == source_sha, "Copying changed saved artifact bytes")
        remember(target, role, source_tag, source_sha, True)
        return target

    def project(name, value, role, source_tag, source_sha=None):
        target = output / name
        write_json(target, value)
        remember(target, role, source_tag, source_sha)
        return target

    copy(projection_root / "primary-qualified-projection.json", "primary-qualified-projection.json",
         "Original independent primary-only projection; whole batch remains failed", "primary-subset", EXPECTED["primary"])
    copy(projection_root / "project_primary.py", "tools/project_primary.py", "New independent projection source",
         "primary-subset-projector", EXPECTED["projector"])
    copy(tools / "analyze_startup_ab.py", "tools/as-built-analyze_startup_ab.py",
         "Unmodified as-built analyzer with complete-batch rules intact", "as-built-analyzer", EXPECTED["analyzer"])
    for name in ("startup-clock.c", "startup_ab.py", "INCIDENT_AND_FIX.md"):
        copy(tools / name, "tools/control2/" + name, "Frozen controller/clock/narrow-fix source",
             "control2-frozen33", frozen["files"][name]["sha256"])
    copy(tools / "frozen-tools-r2.json", "sources/frozen-tools-control2.json", "Original recorded host-tool inventory",
         "control2-frozen33", EXPECTED["control2"])
    copy(software / "software-identity-r2/compiler-profile.json", "sources/compiler-profile.json",
         "Original matched compiler controls", "fresh-software-identity-r2", EXPECTED["compiler"])
    copy(Path(__file__), "tools/export_physical_research.py", "Saved-artifact allowlist exporter", "public-exporter")
    public_rows, metrics = [], {}

    def summarize_row(row):
        tag = row["case"]
        directory = batch / "outputs/out" / tag
        source_hashes = row["file_sha256"]
        final = load(directory / "final.json", source_hashes["final.json"])
        lifecycle = lines(directory / "lifecycle.jsonl", source_hashes["lifecycle.jsonl"])
        performance = lines(directory / "performance.jsonl", source_hashes["performance.jsonl"])
        same_pid = final["process_id"] == row["application_pid"] and all(
            value.get("process_id", row["application_pid"]) == row["application_pid"] for value in lifecycle + performance)
        require(same_pid, "A predefined primary/compatibility row lost its process binding")
        ready = next(event["monotonic_ns"] for event in row["startup_events"] if event["stage"] == "ui_ready")
        send = row["first_business_frame"]["send_accepted_monotonic_ns"]
        values = dict(row["metrics"])
        values["ready_to_business_send_ms"] = round((send - ready) / 1e6, 6)
        metrics[tag] = values
        gate = row["post_business_gate"]
        return {"source_tag": tag, "measurement_role": row["measurement_role"],
                "software_variant": row["software_variant"], "warmup": row["warmup"],
                "included_in_primary_statistics": row["included_in_primary_statistics"],
                "deployed_identity": row["deployed_identity"], "metrics": values,
                "same_pid_final_lifecycle_performance": same_pid,
                "lifecycle_event_counts": dict(Counter(record.get("event", "missing") for record in lifecycle)),
                "final_suspended": final["suspended"], "input_nodes": row["input_devices"],
                "memory_kib": row["memory"], "resource_observation_scope": row["resource_observation_scope"],
                "post_business_gate": {"reference": gate["reference"], "offset_ms": gate["offset_ms"],
                    "deadline_slip_ms": gate["deadline_slip_ms"],
                    "actual_request_after_business_send_ms": round((gate["request"]["monotonic_ns"] - send) / 1e6, 6),
                    "scope": gate["scope"]},
                "first_startup_frame": {key: row["first_startup_frame"][key] for key in FRAME_KEYS},
                "first_business_frame": {key: row["first_business_frame"][key] for key in FRAME_KEYS},
                "recovery": row["recovery"], "final_logical_capture": row["final_capture"],
                "private_original_file_sha256": source_hashes}

    for row in primary["primary_rows"]:
        public_rows.append(summarize_row(row))
    compat = primary["compatibility"]
    compatibility = {"status": compat["status"], "included_in_primary_statistics": False,
                     "case": summarize_row(compat["case"])}
    diag_dir = batch / "outputs/out/diagnostic-01"
    diagnostic_source = primary["diagnostic"]
    probe_path = diag_dir / "native-query-8op-process.json"
    probe = load(probe_path, diagnostic_source["file_sha256"]["native-query-8op-process.json"])
    probe_keys = ("pid", "helper_executable", "helper_sha256", "exit_status", "process_exited", "timed_out",
                  "identity_observed", "stdin_gate_verified", "stdin_released", "helper_executable_verified", "reason")
    require(probe["reason"] == "reserved-descriptor-in-use" and probe["pid"] is None
            and probe["helper_executable"] is None and probe["helper_sha256"] is None
            and probe["stdin_released"] is False, "The independent ABI attempt has another execution scope")
    diag_performance = lines(diag_dir / "performance.jsonl", diagnostic_source["file_sha256"]["performance.jsonl"])
    ready_records = [record for record in diag_performance if record.get("kind") == "startup" and record.get("stage") == "ui_ready"]
    require(len(ready_records) == 1, "The diagnostic window did not have one ready event")
    diag_final = load(diag_dir / "final.json")
    diag_recovery = load(diag_dir / "recovery.json", diagnostic_source["file_sha256"]["recovery.json"])
    rust_sessions = []
    for session in diagnostic_source["rust_calibrations"]:
        trace = lines(diag_dir / session["file"], session["sha256"])
        queries = [record for record in trace if record.get("kind") == "native-query" and record.get("stage") == "query"]
        finishes = [record for record in trace if record.get("kind") == "native-query" and record.get("stage") == "session_finished"]
        require(len(queries) == 5 and len(finishes) == 1, "The retained Rust session changed")
        finish = finishes[0]
        require(finish["data"]["requests"] == 5 and session["session_ready_and_finished"] is True,
                "The retained Rust session did not finish its five requests")
        same_ui_process = session["process_id"] == ready_records[0]["process_id"]
        rust_sessions.append({key: value for key, value in session.items() if key != "process_id"})
        rust_sessions[-1].update({"successful_query_records": len(queries), "finish_requests": finish["data"]["requests"],
            "distinct_properties": len(set(session["properties"])), "same_process_as_later_window": same_ui_process,
            "later_window_ready_after_session_finished_ms": round((ready_records[0]["monotonic_ns"] - finish["monotonic_ns"]) / 1e6, 6)})
    diagnostic = {"whole_case_qualified": False, "included_in_primary_statistics": False,
        "rust_session_status": diagnostic_source["rust_calibration_status"], "rust_sessions": rust_sessions,
        "direct_eight_op": {"qualified": False, "status": diagnostic_source["direct_eight_op_abi_status"],
            "requested_operations": 8, "executed_operations": 0, "process_receipt_sha256": digest(probe_path),
            **{key: probe[key] for key in probe_keys}},
        "later_window": {"ready_events": len(ready_records),
            "final_matches_window_pid": diag_final["process_id"] == ready_records[0]["process_id"],
            "active_final_shelf": diag_final["route"] == "shelf" and diag_final["suspended"] is False,
            "launcher_exit_status": diag_recovery["launcher_exit_status"],
            "diagnostic_recovery_passed": diag_recovery["passed"],
            "final_metadata_sha256": digest(diag_dir / "final.json")},
        "scope": "Independent Rust calibration session and a later normal UI window; the pre-spawn eight-op refusal keeps the whole diagnostic case unqualified"}
    capture_rows = []
    for variant in ("baseline", "candidate"):
        tag = variant + "-06"
        row = next(item for item in primary["primary_rows"] if item["case"] == tag)
        directory = batch / "outputs/out" / tag
        ready = next(event["monotonic_ns"] for event in row["startup_events"] if event["stage"] == "ui_ready")
        send = row["first_business_frame"]["send_accepted_monotonic_ns"]
        liveness_path = directory / "capture-liveness.json"
        live = load(liveness_path)
        require(live["application_pid"] == row["application_pid"] and all(live[key] is True for key in
            ("early_before", "early_after", "settled_before", "settled_after")), "The saved captures did not retain the same live application")
        for phase, raw_name in (("early", "content-frame.raw"), ("settled", "settled-frame.raw")):
            request_path, complete_path = directory / (phase + "-request.json"), directory / (phase + "-complete.json")
            request, complete = load(request_path), load(complete_path)
            require(complete["monotonic_ns"] >= request["monotonic_ns"] >= send, "Saved capture chronology changed")
            gate = row["post_business_gate"]
            if phase == "early":
                require(complete["monotonic_ns"] < gate["target_monotonic_ns"],
                        "The saved early capture window no longer precedes the post-business gate")
                observation_scope = "Instrumented early helper request/completion window before the common post-business gate; not the exact read instant or an instantaneous SEND image"
            else:
                require(request["monotonic_ns"] >= gate["request"]["monotonic_ns"],
                        "The saved settled capture window no longer follows the post-business gate")
                observation_scope = "Instrumented settled helper request/completion window after the common post-business gate; not the exact read instant or an instantaneous SEND image"
            raw = (directory / raw_name).read_bytes()
            header_bytes, pixels = raw.split(b"\n", 1)
            header = json.loads(header_bytes)
            require(header["width"] == 1860 and header["height"] == 2480 and header["bits_per_pixel"] == 8
                    and header["line_length"] == 1872 and header["read_only"] is True and len(pixels) == 1860 * 2480,
                    "Unsupported saved framebuffer geometry")
            require(byte_digest(pixels) == primary["framebuffer_comparisons"][phase]["visible_raw_sha256"][tag],
                    "The application raw pixels differ from the qualified subset comparison")
            target = output / "images" / (variant + "-" + phase + ".png")
            target.parent.mkdir(parents=True, exist_ok=True)
            Image.frombytes("L", (1860, 2480), pixels).save(target, format="PNG")
            with Image.open(target) as image:
                require(image.tobytes() == pixels, "Lossless projection changed the visible memory pixels")
            remember(target, "Lossless synthetic application memory projection", tag + "/" + raw_name, byte_digest(raw))
            observation = {"request_monotonic_ns": request["monotonic_ns"], "completion_monotonic_ns": complete["monotonic_ns"],
                "request_after_business_send_ms": round((request["monotonic_ns"] - send) / 1e6, 6),
                "completion_after_business_send_ms": round((complete["monotonic_ns"] - send) / 1e6, 6),
                "request_after_ready_ms": round((request["monotonic_ns"] - ready) / 1e6, 6),
                "completion_after_ready_ms": round((complete["monotonic_ns"] - ready) / 1e6, 6),
                "between_markers_ms": round((complete["monotonic_ns"] - request["monotonic_ns"]) / 1e6, 6),
                "same_application_alive_before": live[phase + "_before"], "same_application_alive_after": live[phase + "_after"],
                "scope": observation_scope,
                "private_request_sha256": digest(request_path), "private_completion_sha256": digest(complete_path),
                "private_liveness_sha256": digest(liveness_path)}
            if phase == "settled":
                target_path = directory / "settled-deadline-ns.txt"
                deadline = int(target_path.read_text().strip())
                observation.update({"scheduled_target_after_ready_ms": round((deadline - ready) / 1e6, 6),
                    "scheduled_target_after_business_send_ms": round((deadline - send) / 1e6, 6),
                    "request_lateness_ms": round((request["monotonic_ns"] - deadline) / 1e6, 6),
                    "private_target_record_sha256": digest(target_path)})
            metadata = {"source_tag": tag, "phase": phase, "optical_capture": False,
                "evidence_kind": "Derived lossless Gray8 memory PNG; raw retained privately", "raw_filename": raw_name,
                "raw_sha256": byte_digest(raw), "visible_pixels_sha256": byte_digest(pixels), "png_sha256": digest(target),
                "pixel_processing": "None; all packed visible bytes without resize, crop, inversion, rotation, masks, or tone changes",
                "raw_header": {key: header[key] for key in RAW_KEYS}, "observation": observation,
                "source_commit": primary["variants"][variant]["source_commit"],
                "binary_sha256": primary["variants"][variant]["binary_sha256"]}
            project("images/" + variant + "-" + phase + ".json", metadata, "Projected memory-image sidecar", tag)
            capture_rows.append({"file": target.relative_to(output).as_posix(), **metadata})
        final = row["final_capture"]
        target = copy(directory / "final.png", "images/" + variant + "-final-logical.png",
                      "Original logical application PNG", tag, final["file_sha256"])
        with Image.open(target) as image:
            require(image.size == (1860, 2480) and byte_digest(image.convert("RGB").tobytes()) == final["pixel_sha256"],
                    "The original logical snapshot changed")
        project("images/" + variant + "-final-logical.json", {"source_tag": tag, "optical_capture": False,
            "evidence_kind": "Original logical Slint PNG copied byte for byte", "file_sha256": digest(target),
            "rgb_pixels_sha256": final["pixel_sha256"], "width": 1860, "height": 2480,
            "semantics": final["semantics"], "captured_at": final["captured_at"],
            "source_commit": primary["variants"][variant]["source_commit"]}, "Original logical snapshot sidecar", tag)
    release_path, closure_path = batch / "evidence/lease-released.json", batch / "evidence/transport-closure.json"
    release, closure = load(release_path), load(closure_path)
    release_fields = ("ok", "owner_and_token_checked", "native_restored_and_connections_closed",
                      "only_owned_owner_json_removed_then_rmdir", "lease_directory_absent_after_release", "private_token_retained")
    require(all(release[key] is True for key in release_fields) and closure["connections_closed"] is True
            and closure["active_owned_transports"] == [], "Recorded recovery/closure is incomplete")
    require(report["preservation_path_count"] == 9, "The selected-file preservation scope changed")
    paired = {}
    for metric in ("software_ready_ms", "content_send_ms"):
        values = [round(metrics[f"candidate-{index:02d}"][metric] - metrics[f"baseline-{index:02d}"][metric], 6)
                  for index in range(1, 7)]
        paired[metric] = {"candidate_minus_baseline": stats(values), "win_counts": primary["paired_win_counts"][metric]}
    summary = {"schema_version": 1,
        "decision": {"native_query_helper_adopted": False, "main_import_scope": "Research reports and evidence only; no experimental runtime implementation",
                     "experimental_source_commit": primary["variants"]["candidate"]["source_commit"],
                     "reason": "No overall startup benefit established; direct eight-operation diagnostic never executed"},
        "whole_batch": {"success": False, "status": "failed", "selected_cases": len(report["cases"]),
                        "controller_completed_cases": len(report["completed_cases"]),
                        "source_report_sha256": EXPECTED["report"],
                        "failure_scope": "Independent eight-op process refused before spawn; diagnostic whole case unqualified despite normal UI exit"},
        "primary": {"status": primary["status"], "source_projection_sha256": EXPECTED["primary"],
                    "original_analyzer_sha256": EXPECTED["analyzer"], "new_projection_script_sha256": EXPECTED["projector"],
                    "analyzer_source_rules_modified": False, "launches": 14, "warmups_excluded": 2, "pairs": 6,
                    "all_six_pairs_retained": True, "slow_candidate_01_and_06_retained": True,
                    "summary": primary["summary"], "paired_rounds": primary["paired_rounds"], "paired_deltas": paired,
                    "paired_delta_units": "Milliseconds, candidate minus baseline; positive is slower",
                    "cases": public_rows},
        "variants": {tag: {key: value[key] for key in VARIANT_KEYS} for tag, value in primary["variants"].items()},
        "software_identity_source_sha256": EXPECTED["software_identity"],
        "compiler_profile_sha256": EXPECTED["compiler"],
        "compatibility": compatibility, "diagnostic": diagnostic,
        "post_business_observation": report["post_business_observation"],
        "framebuffer_comparisons": primary["framebuffer_comparisons"],
        "final_primary_logical_pixels_match": primary["final_primary_logical_pixels_match"],
        "memory_scope": "One procfs sample after first-business SEND plus 1500 ms gate; no instantaneous or continuously monitored whole-lifetime peak claim",
        "image_observations": capture_rows,
        "recovery": {**primary["restoration"], "selected_preservation_file_count": 9,
            "preservation_scope": report["preservation_scope"], "whole_tree_census": False,
            "exact_lease_release": {"receipt_sha256": digest(release_path), **{key: release[key] for key in release_fields},
                                   "owner_run_token_or_token_hash_exported": False},
            "transport": {"receipt_sha256": digest(closure_path), "connections_closed": True, "active_owned_transport_count": 0},
            "native_home_pixels_reviewed_privately_by_primary_agent": True,
            "native_home_images_exported": False},
        "host_verification_inventory": {"current_controller_tests_executed": 33,
            "unchanged_prior_abi_probe_tests": 27, "unchanged_prior_analysis_and_clock_tests": 65,
            "latest_recorded_inventory_total": 125, "all_125_executed_together": False,
            "all_120_prior_tests_rerun": False, "frozen_control2_sha256": EXPECTED["control2"],
            "earlier_failed_partial_batch_reclassified": False},
        "scope": "Instrumented warm native launches on one device with synthetic offline demo; not physical-tap or optical panel latency",
        "not_qualified": ["Complete physical batch", "Eight-operation ABI", "New touch or rotation", "Sleep/wake", "Other models or firmware"],
        "new_device_application_or_test_execution_during_export": False,
        "limitations": primary["limitations"] + [
            "The independent Rust calibration and the subsequent UI window used different processes; neither qualifies the unstarted eight-op probe.",
            "Memory image request/completion windows are delayed observations, not the instantaneous first-SEND display or optical evidence.",
            "The candidate runtime remains experimental and is not adopted into main."]}
    privacy_check(summary)
    project("summary.json", summary, "Anonymous research summary; no global passed claim", "research-only", EXPECTED["primary"])
    preserved = []
    for name in PREFLIGHT:
        source = args.preflight_root / name
        require(source.is_file(), "A preserved Root preflight record is missing")
        preserved.append({"path": "preflight/" + name, "sha256": digest(source), "bytes": source.stat().st_size,
                          "preserved_existing_file": True, "rewritten_or_copied_by_exporter": False})
    readme = '''# Native-query helper research: 2026-11-17

**The whole physical batch failed. The helper is not adopted into main.**
This record preserves an independently qualified primary subset and the reason
the additional diagnostic did not complete. Only reports and evidence are
intended for main; the runtime experiment remains on source commit `f574b269`.
No device, lease, application, or tests were accessed or run during this export.

## Six retained primary pairs

The predefined primary set contains 14 launches: two warmups and six complete
AB/BA pairs. All six pairs remain included, including slow candidate-01 and
candidate-06. Baseline is fresh source `f50ada15`, application `19708424`;
candidate is source `f574b269`, application `806a406d`, helper `ecb336e4`.
Full source fingerprints, package/executable/launcher hashes and matched release
controls are in [summary.json](summary.json).

| Primary endpoint | Baseline median | Candidate median | Candidate minus baseline mean |
| --- | ---: | ---: | ---: |
| Software ready | 5864.800538 ms | 5798.234347 ms | +181.221051 ms |
| First business SEND accepted | 6643.891885 ms | 6650.601077 ms | +103.429307 ms |

For business SEND, the candidate is faster in three pairs and slower in three;
the difference of independent medians is +6.709192 ms. The separately calculated
paired median differs from that difference and is retained alongside every
sample, range, mean and signed pair delta. These observations establish no
overall startup benefit. They are instrumented warm process launches with
profiling/demo work, not tap-to-panel latency, cold boot, or a universal result.

[primary-qualified-projection.json](primary-qualified-projection.json) is the
original independent projection. Its new [projector](tools/project_primary.py)
calls the unchanged frozen [as-built analyzer](tools/as-built-analyze_startup_ab.py)
functions; it does not rewrite the analyzer's whole-batch rules or promote the
original report's `success=false`. The selected batch had 16 cases and 15
controller-completed cases. Compatibility and diagnostics are outside primary
statistics.

## Compatibility and incomplete diagnostic

The helper-omitted compatibility case completed and restored native control.
The diagnostic's independent Rust calibration has five successful queries over
three property types, followed by finish(requests=5), clean EOF and helper exit.
The later UI window has its own process identity, readiness/accepted frames and
normal launcher status zero. This is not a five-query session inside that UI
process, nor coverage of eight ABI operations.

The separate eight-operation process was refused before spawn with
`reserved-descriptor-in-use`: PID, executable and hash are null, identity was
not observed and stdin was not released. It is not a helper timeout or an ABI
pass. The diagnostic whole case remains unqualified and the global batch remains
failed. No further descriptor or sleep testing was performed for this decision.

## Sources, controls and earlier failures

The narrowly scoped controller lock-retirement fix and its incident record are
preserved under [tools/control2/](tools/control2/). Its frozen record documents
33 newly executed controller regressions, plus unchanged prior sets of 27 ABI
probe and 65 analysis/clock tests: a latest inventory of 125, not a single rerun
of 125. The original 120-test record and rejected partial/preflight history are
not reclassified. Root's existing three [preflight records](preflight/) are
preserved byte for byte and are not rewritten by this exporter.

Saved recovery flags retain native/lighting restoration, transport closure,
same boot/input roster, temporary-stage removal and exact owned-lease release.
Installation preservation covers only nine selected regular-file identities
or recorded absences, not a whole-tree census. Native Home pixels were privately
reviewed; no native Home, native before/final content, owner/token/configuration,
raw private logs, databases or output archives are public here.

## Six synthetic application images

| Last measured pair | Early memory projection | Later memory projection | Original logical PNG |
| --- | --- | --- | --- |
| Baseline-06 | [early](images/baseline-early.png) | [settled](images/baseline-settled.png) | [logical](images/baseline-final-logical.png) |
| Candidate-06 | [early](images/candidate-early.png) | [settled](images/candidate-settled.png) | [logical](images/candidate-final-logical.png) |

The four Gray8 PNGs losslessly project every 1860 x 2480 packed visible byte
from saved memory (device stride 1872). No resizing, cropping, inversion,
rotation, masks or tone changes were applied. Original raw files remain private;
raw, pixel and PNG hashes are retained. The other two PNGs are untouched logical
Slint snapshots before framebuffer compensation.

Each memory-image sidecar retains actual request and completion markers,
offsets from readiness/business SEND, and same-application liveness before and
after capture. These are helper request/completion windows, not instantaneous
SEND images or exact read instants. Both early windows finish before the shared
first-business SEND +1500 ms gate; the later settled windows occur after that
gate. The later scheduled target and lateness remain explicit. All last-pair memory pixels
and final logical pixels match between variants, but no optical panel completion
or stability at an earlier time is established.

Memory/input samples also follow the common gate. VmHWM is the kernel high-water
value when sampled; no continuous full-lifetime peak profile is claimed. SEND
acceptance, marker/driver completion and panel observation remain separate.
No new touch, rotation, sleep/wake, model or firmware qualification is established.

[manifest.json](manifest.json) binds the public files and separately lists the
existing preserved preflight records. Original private sources are identified
by hash; the failed batch and frozen raw evidence were not modified.
'''
    target = output / "README.md"
    target.write_text(readme, encoding="utf-8", newline="\n")
    remember(target, "Research decision and explicit qualification limits", "research-only")
    require(len(list((output / "images").glob("*.png"))) == 6, "Exactly six synthetic application PNGs are permitted")
    manifest = {"schema_version": 1, "whole_batch_success": False, "helper_adopted": False,
        "scope": "Saved research evidence export only; no new device/lease/application/test execution",
        "source_sha256": EXPECTED, "images": 6, "new_files": sorted(files, key=lambda row: row["path"]),
        "preserved_existing_preflight_files": preserved,
        "excluded": ["Private raw reports and configuration", "Owner/token values", "Native Home/before/final content",
                     "Databases", "Raw logs and output archives", "Raw framebuffer payloads", "Runtime implementation adoption"]}
    write_json(output / "manifest.json", manifest)
    for path in (output / "images").glob("*.png"):
        path.chmod(0o444)
    print(json.dumps({"summary_sha256": digest(output / "summary.json"), "manifest_sha256": digest(output / "manifest.json"),
        "new_files": len(files) + 1, "preserved_preflight_files_not_written": 3, "pngs": 6,
        "whole_batch_success": False, "helper_adopted": False, "device_or_tests_accessed_during_export": False}))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--batch-root", type=Path, required=True)
    parser.add_argument("--software-root", type=Path, required=True)
    parser.add_argument("--preflight-root", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    main(parser.parse_args())
