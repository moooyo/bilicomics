#!/usr/bin/env python3
"""Qualify the predefined primary subset of a retained failed startup batch.

This composes frozen analyzer functions without changing that analyzer or the
source report. It never qualifies the whole batch and never opens a device.
"""

import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import statistics
import sys


sys.dont_write_bytecode = True
FROZEN_ANALYZER_SHA256 = "55e80ecb8bc0c51b3da536bab29c0ea67074db27f00861685a4d648b858e4a28"
EXTRA_RESTORATION = ("input_roster_unchanged", "temporary_stage_removed")


def load_frozen(path):
    content = path.read_bytes()
    if hashlib.sha256(content).hexdigest() != FROZEN_ANALYZER_SHA256:
        raise RuntimeError("The selected analyzer is not the frozen raw-window revision")
    specification = importlib.util.spec_from_file_location("frozen_native_query_analysis", path)
    module = importlib.util.module_from_spec(specification)
    specification.loader.exec_module(module)
    return module


def bind_row(module, row, metadata, variants, previous_finished=None):
    row.update(metadata)
    selected = variants[row["software_variant"]]
    module.require(row["deployed_identity"]["binary_sha256"] == selected["binary_sha256"]
                   and row["deployed_identity"]["native_query_sha256"] == selected["deployed_native_query_sha256"],
                   "case_deployed_binary_or_helper_identity_differs")
    row["included_in_primary_statistics"] = metadata["measurement_role"] == "primary"
    module.require(previous_finished is None or row["request"]["monotonic_ns"] > previous_finished,
                   "actual_case_request_order_or_overlap_differs")
    return row


def project(batch, analyzer):
    module = load_frozen(analyzer)
    module.require(sys.platform == "linux", "run_only_on_test_env")
    batch = batch.resolve(strict=True)
    report_path = batch / "report.json"
    report_bytes = module.read_bytes(report_path, 2 * 1024 * 1024)
    run = module.decode_json(report_bytes)
    module.require(isinstance(run, dict) and run.get("success") is False,
                   "projection_requires_the_retained_whole_batch_failure")
    restoration_fields = tuple(field for field in module.RESTORATION if field != "success") + EXTRA_RESTORATION
    module.require(all(run.get(field) is True for field in restoration_fields),
                   "failed_batch_restoration_is_not_complete")
    variants = module.validate_variants(run)
    rounds = module.validate_schedule(run.get("cases"))
    metadata = module.validate_case_metadata(run)
    expected_probe_executable = module.validate_deployment_context(run)
    primary_names = [name for name in run["cases"] if metadata[name]["measurement_role"] in ("warmup", "primary")]
    module.require(len(primary_names) == 14 and rounds == 6, "projection_primary_schedule_differs")
    module.require(run.get("completed_cases") == [*primary_names, "compat-01"],
                   "projection_completed_case_inventory_differs")
    rows = []
    previous_finished = None
    for name in primary_names:
        row = bind_row(module, module.analyze_case(batch, name, expected_probe_executable),
                       metadata[name], variants, previous_finished)
        previous_finished = row["trace_finished_monotonic_ns"]
        rows.append(row)
    module.require(len({tuple(row["input_devices"]) for row in rows}) == 1, "selected_input_devices_differ")
    captures = {(row["final_capture"]["width"], row["final_capture"]["height"],
                 row["final_capture"]["pixel_sha256"]) for row in rows}
    module.require(len(captures) == 1, "final_business_pixels_or_dimensions_differ")
    early = module.compare_framebuffer_pair(batch, rows, "framebuffer_capture", "content-frame.raw")
    settled = module.compare_framebuffer_pair(batch, rows, "settled_framebuffer_capture", "settled-frame.raw")
    module.require(settled["whole_visible_frame_equal"], "settled_framebuffer_pixels_or_geometry_differ")
    metric_names = set(rows[0]["metrics"])
    module.require(all(set(row["metrics"]) == metric_names for row in rows), "case_metric_inventory_differs")
    summary = {variant: {name: module.distribution([row["metrics"][name] for row in rows
                if row["variant"] == variant and row["included_in_primary_statistics"]])
                for name in sorted(metric_names)} for variant in module.VARIANTS}
    by_case = {row["case"]: row for row in rows}
    metrics = ("software_ready_ms", "content_send_ms")
    pairs = []
    for number in range(1, rounds + 1):
        pair = {variant: by_case[f"{variant}-{number:02d}"] for variant in module.VARIANTS}
        pairs.append({"round": number, "order": list(module.VARIANTS if number % 2 else reversed(module.VARIANTS)),
            "metrics": {metric: {"baseline": pair["baseline"]["metrics"][metric],
                "candidate": pair["candidate"]["metrics"][metric],
                "reduction_ms": round(pair["baseline"]["metrics"][metric] - pair["candidate"]["metrics"][metric], 6)}
                for metric in metrics}})
    reductions = {metric: {
        "difference_of_medians_ms": round(summary["baseline"][metric]["median"] - summary["candidate"][metric]["median"], 6),
        "median_paired_reduction_ms": round(statistics.median(pair["metrics"][metric]["reduction_ms"] for pair in pairs), 6),
        "percent_from_medians": round(100 * (summary["baseline"][metric]["median"] - summary["candidate"][metric]["median"])
                                      / summary["baseline"][metric]["median"], 3)} for metric in metrics}
    wins = {metric: {"candidate_faster": sum(pair["metrics"][metric]["reduction_ms"] > 0 for pair in pairs),
                    "equal": sum(pair["metrics"][metric]["reduction_ms"] == 0 for pair in pairs),
                    "candidate_slower": sum(pair["metrics"][metric]["reduction_ms"] < 0 for pair in pairs)} for metric in metrics}

    compatibility = bind_row(module, module.analyze_case(batch, "compat-01", expected_probe_executable),
                             metadata["compat-01"], variants, previous_finished)
    module.require(tuple(compatibility["input_devices"]) == tuple(rows[0]["input_devices"])
                   and (compatibility["final_capture"]["width"], compatibility["final_capture"]["height"],
                        compatibility["final_capture"]["pixel_sha256"]) in captures,
                   "compatibility_content_or_input_differs")

    diagnostic_directory = batch / "outputs/out/diagnostic-01"
    diagnostic_recovery_bytes = module.read_bytes(diagnostic_directory / "recovery.json")
    diagnostic_recovery = module.decode_json(diagnostic_recovery_bytes)
    module.require(isinstance(diagnostic_recovery, dict) and diagnostic_recovery.get("passed") is False
                   and all(diagnostic_recovery.get(field) is True for field in
                           ("native_restored", "owned_processes_gone", "production_lock_absent"))
                   and diagnostic_recovery.get("launcher_exit_status") == 0
                   and diagnostic_recovery.get("native_query_abi_probe_completed") == 0,
                   "diagnostic_failure_or_restoration_scope_differs")
    request_bytes = module.read_bytes(diagnostic_directory / "request.json")
    diagnostic_request = module.decode_json(request_bytes)
    origin = module.integer(diagnostic_request.get("monotonic_ns"), "diagnostic_request", 1)
    module.require(origin > compatibility["trace_finished_monotonic_ns"], "diagnostic_actual_case_order_differs")
    diagnostic_trace_bytes = module.read_bytes(diagnostic_directory / "performance.jsonl")
    diagnostic_trace = module.trace_records(diagnostic_trace_bytes)
    entries = [row for row in diagnostic_trace if row.get("kind") == "startup" and row.get("stage") == "process_enter"]
    module.require(len(entries) == 1, "diagnostic_process_entry_is_not_unique")
    entry_data = entries[0]["data"] or {}
    observed = entry_data.get("observed")
    entry_stamp = observed.get("monotonic_ns") if isinstance(observed, dict) else entries[0].get("monotonic_ns")
    entry_stamp = module.integer(entry_stamp, "diagnostic_process_entry", 1)
    deployment_bytes = module.read_bytes(diagnostic_directory / "deployed-identity.json")
    deployment = module.decode_json(deployment_bytes)
    module.require(deployment == {"binary_sha256": variants["diagnostic"]["binary_sha256"],
                   "native_query_sha256": variants["diagnostic"]["deployed_native_query_sha256"]},
                   "diagnostic_deployment_differs")
    calibrations = module.calibration_observations(diagnostic_directory, origin, diagnostic_trace[0]["process_id"], entry_stamp)
    process_bytes = module.read_bytes(diagnostic_directory / "native-query-8op-process.json")
    process = module.decode_json(process_bytes)
    module.require(isinstance(process, dict) and process.get("reason") == "reserved-descriptor-in-use"
                   and process.get("identity_observed") is False and process.get("stdin_released") is False
                   and process.get("exit_status") is None and process.get("pid") in (None, 0),
                   "diagnostic_pre_spawn_refusal_scope_differs")
    module.require(module.read_bytes(report_path, 2 * 1024 * 1024) == report_bytes,
                   "source_report_changed_during_projection")
    module.require(hashlib.sha256(analyzer.read_bytes()).hexdigest() == FROZEN_ANALYZER_SHA256,
                   "frozen_analyzer_changed_during_projection")
    return {"schema_version": 1, "status": "primary_qualified_projection", "whole_batch_status": "failed",
        "whole_batch_success": False, "source_report_sha256": module.sha(report_bytes),
        "frozen_analyzer_sha256": FROZEN_ANALYZER_SHA256,
        "scope": "Only the fourteen predefined primary launches are qualified by the unchanged frozen analyzer functions; the retained sixteen-launch batch remains failed and the direct eight-op ABI is not qualified",
        "variants": variants, "primary_launches": 14, "measured_pairs": rounds, "warmups_excluded_from_statistics": 2,
        "primary_cases": primary_names, "primary_rows": rows, "summary": summary, "paired_rounds": pairs,
        "median_reductions": reductions, "paired_win_counts": wins,
        "selected_primary_input_devices_match": True, "final_primary_logical_pixels_match": True,
        "framebuffer_comparisons": {"early": early, "settled": settled},
        "restoration": {field: run[field] for field in restoration_fields},
        "compatibility": {"status": "qualified_excluded_case", "included_in_primary_statistics": False, "case": compatibility},
        "diagnostic": {"whole_case_qualified": False, "included_in_primary_statistics": False,
            "rust_calibration_status": "qualified_independent_session_observations", "rust_calibrations": calibrations,
            "direct_eight_op_abi_qualified": False, "direct_eight_op_abi_status": "not_completed_pre_spawn_refusal",
            "direct_eight_op_abi_reason": "reserved-descriptor-in-use",
            "file_sha256": {"recovery.json": module.sha(diagnostic_recovery_bytes), "request.json": module.sha(request_bytes),
                "performance.jsonl": module.sha(diagnostic_trace_bytes), "deployed-identity.json": module.sha(deployment_bytes),
                "native-query-8op-process.json": module.sha(process_bytes)}},
        "limitations": ["This projection does not promote report.success or qualify the complete batch.",
            "Compatibility and diagnostic observations are outside all primary distributions.",
            "Every primary sample, including slow candidates, remains included; no outlier rejection is performed.",
            "These are instrumented warm-start software/SEND observations, not physical tap latency or optical panel completion.",
            "Memory/input sampling follows the retained post-business gate and is not a continuous lifetime peak profile."]}


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--batch-dir", type=Path, required=True)
    parser.add_argument("--analyzer", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args(argv)
    result = project(args.batch_dir, args.analyzer)
    result["projection_script_sha256"] = hashlib.sha256(Path(__file__).read_bytes()).hexdigest()
    content = (json.dumps(result, indent=2, sort_keys=True, allow_nan=False) + "\n").encode()
    descriptor = os.open(args.output, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW | os.O_CLOEXEC, 0o600)
    with os.fdopen(descriptor, "wb") as stream:
        stream.write(content)
    print(json.dumps({key: result[key] for key in ("status", "whole_batch_status", "whole_batch_success",
                     "primary_launches", "measured_pairs", "summary", "median_reductions", "paired_win_counts")}, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
