#!/usr/bin/env python3
"""Analyze a completed, restored BiliComics startup A/B batch on test-env.

The primary endpoints use CLOCK_MONOTONIC in both the request helper and
application trace. Shell uptime samples are retained as separate coarse phase
observations and never substitute for either endpoint.
"""

import argparse
from collections import defaultdict
from decimal import Decimal, InvalidOperation
import hashlib
from io import BytesIO
import json
import math
import os
from pathlib import Path
import re
import stat
import statistics
import sys


BASELINE_COMMIT = "f50ada15dd934c986e8305a1f9dd8e46f51b2529"
VARIANTS = ("baseline", "candidate")
SOFTWARE_VARIANTS = ("baseline", "candidate", "compat", "diagnostic")
EXCLUDED_CASES = ("compat-01", "diagnostic-01")
MEASURED_ROUNDS = 6
RESTORATION = ("success", "same_boot", "install_unchanged", "lighting_restored",
               "transport_closed", "native_restored", "lease_released")
REQUIRED_FILES = ("request.json", "performance.jsonl", "shell.tsv", "recovery.json",
                  "input-fds-at-content.txt", "memory-at-content.txt", "final.png", "final.json",
                  "lifecycle.jsonl", "post-business-target-ns.txt", "post-business-gate.json", "deployed-identity.json")
IDENTITY_FIELDS = ("source_commit", "source_fingerprint", "binary_sha256",
                   "launcher_sha256", "package_sha256", "compiler_fingerprint")
FRAME_FIELDS = ("render_sequence", "rendered", "initial_frame", "forced_full",
                "complete_frame_replacement", "framebuffer_size", "framebuffer_stride",
                "framebuffer_bits_per_pixel", "rotation_degrees", "output_inverted",
                "black_and_white", "refresh_region", "refresh_waveform", "refresh_abi",
                "refresh_flashing", "refresh_ioctl_returned", "refresh_ioctl_attempts",
                "refresh_accepted", "refresh_errno", "completion_errno",
                "preceding_update_marker", "accepted_update_marker", "completed_update_marker",
                "driver_completion_observed", "panel_completion_observed")
MEMORY_FIELDS = ("VmPeak", "VmSize", "VmLck", "VmPin", "VmHWM", "VmRSS", "RssAnon",
                 "RssFile", "RssShmem", "VmData", "VmStk", "VmExe", "VmLib", "VmPTE",
                 "VmSwap", "HugetlbPages")


class AnalysisError(RuntimeError):
    pass


def require(condition, reason):
    if not condition:
        raise AnalysisError(reason)


def integer(value, name, minimum=0):
    require(type(value) is int and minimum <= value <= (1 << 64) - 1, "invalid_" + name)
    return value


def read_bytes(path, limit=8 * 1024 * 1024):
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC)
    with os.fdopen(fd, "rb") as stream:
        before = os.fstat(stream.fileno())
        require(stat.S_ISREG(before.st_mode) and before.st_nlink == 1
                and 0 <= before.st_size <= limit, "unsafe_or_oversized_" + path.name)
        content = stream.read(limit + 1)
        after = os.fstat(stream.fileno())
    require(len(content) <= limit and len(content) == before.st_size
            and (before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns, before.st_ctime_ns)
            == (after.st_dev, after.st_ino, after.st_size, after.st_mtime_ns, after.st_ctime_ns),
            "changed_" + path.name)
    return content


def unique_object(pairs):
    value = {}
    for key, entry in pairs:
        require(key not in value, "duplicate_json_key")
        value[key] = entry
    return value


def decode_json(content):
    return json.loads(content, object_pairs_hook=unique_object,
                      parse_constant=lambda _: (_ for _ in ()).throw(AnalysisError("nonfinite_json")))


def sha(content):
    return hashlib.sha256(content).hexdigest()


def delta_ms(end, start):
    require(type(end) is int and type(start) is int and 0 <= end - start <= 600_000_000_000,
            "invalid_or_unbounded_monotonic_interval")
    return round((end - start) / 1_000_000, 6)


def validate_variants(run):
    variants = run.get("variants")
    require(isinstance(variants, dict) and set(variants) == set(SOFTWARE_VARIANTS), "invalid_variants")
    output = {}
    builds = []
    for name in SOFTWARE_VARIANTS:
        source = variants[name]
        require(isinstance(source, dict), "invalid_variant_identity")
        identity = {}
        for field in IDENTITY_FIELDS:
            value = source.get(field)
            pattern = r"[0-9a-f]{40}" if field == "source_commit" else r"[0-9a-f]{64}"
            require(isinstance(value, str) and re.fullmatch(pattern, value), "missing_or_invalid_" + field)
            identity[field] = value
        for field in ("worker_sha256", "instrumented_launcher_sha256"):
            if field in source:
                require(isinstance(source[field], str) and re.fullmatch(r"[0-9a-f]{64}", source[field]),
                        "invalid_" + field)
                identity[field] = source[field]
        require("worker_sha256" in identity, "missing_worker_sha256")
        for field in ("native_query_sha256", "deployed_native_query_sha256"):
            require(field in source, "missing_" + field)
            value = source[field]
            require(value is None or (isinstance(value, str) and re.fullmatch(r"[0-9a-f]{64}", value)),
                    "invalid_" + field)
            identity[field] = value
        build = source.get("build_configuration", run.get("build_configuration"))
        require(build is None or (isinstance(build, dict) and build), "invalid_build_configuration")
        builds.append(build)
        if build is not None:
            identity["build_configuration"] = build
        output[name] = identity
    require(output["baseline"]["source_commit"] == BASELINE_COMMIT, "baseline_commit_differs")
    require(output["baseline"]["source_fingerprint"] != output["candidate"]["source_fingerprint"],
            "variant_source_fingerprints_are_identical")
    require(all(build == builds[0] for build in builds), "variant_build_configurations_differ")
    require(all(output[name]["compiler_fingerprint"] == output["baseline"]["compiler_fingerprint"] for name in SOFTWARE_VARIANTS),
            "compiler_configuration_fingerprints_differ")
    require(output["baseline"]["native_query_sha256"] is None
            and output["baseline"]["deployed_native_query_sha256"] is None,
            "baseline_unexpected_native_query_helper")
    candidate = output["candidate"]
    require(candidate["native_query_sha256"] is not None
            and candidate["deployed_native_query_sha256"] == candidate["native_query_sha256"],
            "candidate_native_query_deployment_differs")
    shared = (*IDENTITY_FIELDS, "worker_sha256", "native_query_sha256")
    for name in ("compat", "diagnostic"):
        require(all(output[name][field] == candidate[field] for field in shared),
                name + "_is_not_the_same_candidate_software")
    require(output["compat"]["deployed_native_query_sha256"] is None,
            "compatibility_helper_was_not_omitted")
    require(output["diagnostic"]["deployed_native_query_sha256"] == candidate["native_query_sha256"],
            "diagnostic_native_query_deployment_differs")
    return output


def validate_schedule(cases):
    require(isinstance(cases, list) and all(isinstance(case, str) for case in cases)
            and len(cases) == len(set(cases)), "invalid_case_schedule")
    require(len(cases) == 2 + 2 * MEASURED_ROUNDS + len(EXCLUDED_CASES), "insufficient_or_invalid_pairs")
    rounds = MEASURED_ROUNDS
    expected = ["baseline-warmup", "candidate-warmup"]
    for number in range(1, rounds + 1):
        order = VARIANTS if number % 2 else tuple(reversed(VARIANTS))
        expected.extend(f"{name}-{number:02d}" for name in order)
    expected.extend(EXCLUDED_CASES)
    require(cases == expected, "case_order_is_not_warmup_then_alternating_ab_ba_and_excluded_checks")
    return rounds


def validate_case_metadata(run):
    metadata = run.get("case_metadata")
    require(isinstance(metadata, dict) and set(metadata) == set(run["cases"]), "invalid_case_metadata")
    output = {}
    for case in run["cases"]:
        variant = case.split("-", 1)[0]
        role = ("compatibility" if variant == "compat" else "diagnostic" if variant == "diagnostic"
                else "warmup" if case.endswith("-warmup") else "primary")
        value = metadata[case]
        require(isinstance(value, dict) and value.get("software_variant") == variant
                and value.get("measurement_role") == role, "case_role_or_software_variant_differs")
        output[case] = {"software_variant": variant, "measurement_role": role}
    return output


def validate_deployment_context(run):
    run_id = run.get("run_id")
    require(isinstance(run_id, str) and re.fullmatch(r"native-query-[0-9]{8}-[0-9a-f]{8}", run_id),
            "invalid_native_query_run_identity")
    stage = run.get("deployment_stage")
    require(stage == "/mnt/us/bilicomics-" + run_id, "native_query_deployment_stage_differs")
    protocol = run.get("native_query_abi_probe")
    require(isinstance(protocol, dict) and protocol.get("case") == "diagnostic-01"
            and protocol.get("opcodes") == list(range(1, 9))
            and protocol.get("request_sha256") == sha(bytes(range(1, 9)))
            and protocol.get("excluded_from_primary_metrics") is True,
            "native_query_abi_probe_protocol_differs")
    return stage + "/diagnostic/app/bilicomics-native-query"


def trace_records(content):
    require(content and content.endswith(b"\n"), "incomplete_performance_trace")
    records = [decode_json(line) for line in content.splitlines()]
    require(records and len(records) <= 100_000, "invalid_trace_record_count")
    for row in records:
        require(isinstance(row, dict) and row.get("schema_version") == 1
                and isinstance(row.get("data"), (dict, type(None))), "unsupported_performance_schema")
        integer(row.get("process_id"), "process_id", 2)
    require(len({row["process_id"] for row in records}) == 1, "multiple_application_processes")
    starts = [row for row in records if row.get("kind") == "diagnostic" and row.get("stage") == "trace_started"]
    stops = [row for row in records if row.get("kind") == "diagnostic" and row.get("stage") == "trace_stopped"]
    require(len(starts) == len(stops) == 1 and isinstance(stops[0]["data"], dict)
            and stops[0]["data"].get("reason") == "finished"
            and stops[0]["data"].get("truncated") is False, "trace_did_not_finish_cleanly")
    require(records[0] is starts[0] and records[-1] is stops[0], "trace_start_or_terminal_is_not_at_the_boundary")
    return records


def calibration_observations(directory, origin, application_pid, process_enter_ns):
    paths = sorted(directory.glob("calibration-*"))
    require(1 <= len(paths) <= 8, "missing_or_unbounded_calibration_traces")
    observations = []
    for path in paths:
        require(re.fullmatch(r"calibration-[1-9][0-9]{0,2}\.jsonl", path.name),
                "noncanonical_calibration_trace_name")
        content = read_bytes(path)
        records = trace_records(content)
        require(all(row.get("kind") in ("diagnostic", "native-query") for row in records),
                "calibration_contains_unexpected_kind")
        require(records[0]["process_id"] != application_pid, "calibration_reuses_ui_process")
        stamps = [integer(row.get("monotonic_ns"), "calibration_timestamp", 1) for row in records]
        require(stamps == sorted(stamps) and origin <= stamps[0] <= stamps[-1] <= process_enter_ns,
                "calibration_clock_or_phase_order_differs")
        session = [row for row in records if row.get("kind") == "native-query"]
        require(session and len({integer(row.get("operation_id"), "calibration_operation", 1) for row in session}) == 1,
                "calibration_session_is_missing_or_not_unique")
        require(session[0].get("stage") == "session_ready"
                and isinstance(session[0]["data"], dict) and set(session[0]["data"]) == {"requests"}
                and type(session[0]["data"]["requests"]) is int and session[0]["data"]["requests"] == 0,
                "calibration_session_not_ready")
        require(session[-1].get("stage") == "session_finished"
                and isinstance(session[-1]["data"], dict), "calibration_session_did_not_finish")
        queries = session[1:-1]
        require(len(queries) in (3, 5) and all(row.get("stage") == "query" for row in queries),
                "calibration_query_count_or_stage_differs")
        properties = []
        for number, row in enumerate(queries, 1):
            data = row["data"]
            require(isinstance(data, dict) and set(data) == {"property", "requests"}
                    and type(data["requests"]) is int and data["requests"] == number,
                    "calibration_query_sequence_differs")
            properties.append(integer(data["property"], "calibration_property", 1))
        require(properties in ([1, 2, 1], [1, 2, 3, 3, 1]),
                "calibration_property_sequence_differs")
        require(session[-1]["data"] == {"requests": len(queries)}, "calibration_finished_count_differs")
        session_ns = integer(session[-1]["monotonic_ns"], "calibration_finish", 1) - integer(
            session[0]["monotonic_ns"], "calibration_ready", 1)
        require(0 <= session_ns <= 2_000_000_000, "calibration_session_exceeds_capture_budget")
        observations.append({"file": path.name, "sha256": sha(content), "process_id": records[0]["process_id"],
            "operation_id": session[0]["operation_id"], "requests": len(queries), "properties": properties,
            "capture_path": "automatic_portrait" if len(queries) == 5 else "fixed_portrait",
            "after_native_request_ms": delta_ms(stamps[0], origin), "trace_finished_after_request_ms": delta_ms(stamps[-1], origin),
            "session_observed_ms": round(session_ns / 1_000_000, 6),
            "session_ready_and_finished": True,
            "scope": "Successful per-query observations followed by clean EOF/exit and session_finished; not a timer-based success inference or the complete capture span."})
    return observations


def abi_probe_observation(directory, expected_helper_sha256, expected_helper_executable):
    response_name = "native-query-8op-response.bin"
    stderr_name = "native-query-8op-stderr.txt"
    process_name = "native-query-8op-process.json"
    ledger_name = "owned-processes.tsv"
    response = read_bytes(directory / response_name, 4 + 8 * (4 + 128))
    stderr = read_bytes(directory / stderr_name, 8192)
    process_bytes = read_bytes(directory / process_name, 8192)
    ledger_bytes = read_bytes(directory / ledger_name)
    process = decode_json(process_bytes)
    require(isinstance(process, dict) and type(process.get("schema_version")) is int
            and process["schema_version"] == 2, "native_query_abi_probe_unsupported_process_schema")
    require(isinstance(process, dict) and type(process.get("exit_status")) is int
            and process["exit_status"] == 0 and process.get("timed_out") is False
            and process.get("process_exited") is True, "native_query_abi_probe_did_not_exit_cleanly")
    require(isinstance(expected_helper_sha256, str) and re.fullmatch(r"[0-9a-f]{64}", expected_helper_sha256)
            and process.get("helper_sha256") == expected_helper_sha256,
            "native_query_abi_probe_helper_identity_differs")
    for field in ("identity_observed", "helper_file_identity_stable", "case_token_inherited",
                  "stdin_gate_verified", "stdin_released"):
        require(process.get(field) is True, "native_query_abi_probe_" + field + "_is_not_proven")
    for field in ("case_token_observed", "helper_executable_verified"):
        require(field not in process or process[field] is True, "native_query_abi_probe_" + field + "_is_not_proven")
    pid = integer(process.get("pid"), "abi_probe_pid", 2)
    birth = integer(process.get("start_ticks"), "abi_probe_start_ticks", 1)
    parent = integer(process.get("parent_pid"), "abi_probe_parent_pid", 2)
    group = integer(process.get("process_group"), "abi_probe_process_group", 1)
    session = integer(process.get("session_id"), "abi_probe_session_id", 1)
    require(process.get("helper_executable") == expected_helper_executable,
            "native_query_abi_probe_executable_path_differs")
    require(ledger_bytes and ledger_bytes.endswith(b"\n"), "native_query_abi_probe_missing_process_ledger")
    observed_in_ledger = False
    for line in ledger_bytes.decode("utf-8").splitlines():
        fields = line.split(maxsplit=5)
        # A nonmatching exited process may have lost its executable symlink
        # during the original snapshot; only a complete helper row can bind ABI.
        require(5 <= len(fields) <= 6 and all(re.fullmatch(r"[0-9]+", value) for value in fields[:5]),
                "native_query_abi_probe_invalid_process_ledger")
        values = [integer(int(value), "abi_ledger_field") for value in fields[:5]]
        if tuple(values) == (pid, birth, parent, group, session) and len(fields) == 6 and fields[5] == expected_helper_executable:
            observed_in_ledger = True
    require(observed_in_ledger, "native_query_abi_probe_identity_not_in_owned_ledger")
    require(stderr == b"", "native_query_abi_probe_stderr_is_not_empty")
    require(response.startswith(b"AQ1\n"), "native_query_abi_probe_handshake_differs")
    cursor = 4
    replies = []
    for property_id in range(1, 9):
        require(cursor + 4 <= len(response), "native_query_abi_probe_incomplete_header")
        opcode, status, high, low = response[cursor:cursor + 4]
        cursor += 4
        length = high * 256 + low
        require(opcode == property_id and status == 0 and length <= 128,
                "native_query_abi_probe_reply_opcode_status_or_length_differs")
        require(cursor + length <= len(response), "native_query_abi_probe_incomplete_payload")
        payload = response[cursor:cursor + length]
        cursor += length
        replies.append({"opcode": opcode, "status": status, "payload_sha256": sha(payload)})
    require(cursor == len(response), "native_query_abi_probe_trailing_data")
    return {"helper_sha256": expected_helper_sha256, "exit_status": 0, "timed_out": False,
            "process_exited": True, "reply_count": 8, "opcodes": list(range(1, 9)), "replies": replies,
            "identity_proof_status": "observed_helper_identity_matched_owned_process_ledger",
            "raw_response_sha256": sha(response), "stderr_bytes": 0,
            "file_sha256": {response_name: sha(response), stderr_name: sha(stderr), process_name: sha(process_bytes),
                            ledger_name: sha(ledger_bytes)},
            "scope": "Independent excluded direct C ABI probe after UI/native restoration; eight framed successful replies and clean EOF/exit, separate from the Rust capture session's three or five queries; values are not exported"}


def frame_observation(row, origin):
    data = row["data"]
    require(isinstance(data, dict), "invalid_frame_payload")
    result = {field: data.get(field) for field in FRAME_FIELDS}
    result.update(record_kind=row["kind"], record_stage=row["stage"])
    stages = data.get("stages")
    require(isinstance(stages, dict), "invalid_frame_stage_map")
    result["stages"] = stages
    result["record_monotonic_ns"] = integer(row.get("monotonic_ns"), "frame_record_timestamp", 1)
    result["record_after_request_ms"] = delta_ms(result["record_monotonic_ns"], origin)
    result["send_accepted_monotonic_ns"] = None
    result["send_accepted_after_request_ms"] = None
    submit = stages.get("refresh_submit")
    if data.get("refresh_accepted") is True:
        require(isinstance(submit, dict), "accepted_frame_has_no_submit_stage")
        started = integer(submit.get("started_monotonic_ns"), "refresh_submit_start", 1)
        finished = integer(submit.get("finished_monotonic_ns"), "refresh_submit_timestamp", 1)
        require(started <= finished <= result["record_monotonic_ns"], "invalid_submit_clock_order")
        require(data.get("refresh_ioctl_returned") is True
                and type(data.get("refresh_ioctl_attempts")) is int
                and data["refresh_ioctl_attempts"] >= 1, "accepted_frame_has_no_returned_ioctl")
        result["send_accepted_monotonic_ns"] = finished
        result["send_accepted_after_request_ms"] = delta_ms(finished, origin)
    require(data.get("panel_completion_observed") is False, "unexpected_panel_completion_claim")
    return result


def input_nodes(content):
    rows = content.decode("utf-8").splitlines()
    require(2 <= len(rows) <= 256, "invalid_input_fd_evidence")
    nodes = set()
    for row in rows:
        fields = row.split()
        require(fields and re.fullmatch(r"/dev/input/event[0-9]+", fields[-1]), "noncanonical_input_node")
        nodes.add(fields[-1])
    require(len(nodes) >= 2, "fewer_than_two_input_nodes")
    return sorted(nodes)


def memory_values(content):
    values = {}
    for row in content.decode("ascii").splitlines():
        name, separator, raw = row.partition(":")
        if name not in MEMORY_FIELDS and name != "Threads":
            continue
        require(separator and name not in values, "invalid_memory_evidence")
        fields = raw.split()
        require((len(fields) == 2 and fields[1] == "kB") if name != "Threads" else len(fields) == 1,
                "invalid_memory_units")
        require(fields[0].isdecimal(), "invalid_memory_value")
        values[name] = int(fields[0])
    require(values.get("VmRSS", 0) > 0 and values.get("VmHWM", 0) >= values["VmRSS"],
            "incomplete_memory_evidence")
    return values


def shell_observations(content):
    observations = []
    for line in content.decode("utf-8").splitlines():
        fields = line.split()
        require(2 <= len(fields) <= 32 and re.fullmatch(r"[a-z][a-z0-9_-]{0,80}", fields[1]),
                "invalid_shell_stage_record")
        try:
            at = Decimal(fields[0])
        except InvalidOperation as error:
            raise AnalysisError("invalid_shell_uptime") from error
        require(at.is_finite() and 0 <= at < Decimal(1 << 32), "invalid_shell_uptime")
        observations.append({"stage": fields[1], "uptime_seconds": str(at)})
    require(observations and len(observations) <= 4096, "empty_or_unbounded_shell_observations")
    return observations


def png_identity(content):
    from PIL import Image

    with Image.open(BytesIO(content)) as image:
        require(image.format == "PNG" and image.width > 0 and image.height > 0
                and image.width * image.height <= 32_000_000, "invalid_final_png")
        require(image.mode in ("1", "L", "RGB", "RGBA"), "unsupported_final_png_mode")
        rgb = image.convert("RGB")
        return {"width": rgb.width, "height": rgb.height, "comparison_mode": "RGB",
                "pixel_sha256": sha(rgb.tobytes()), "file_sha256": sha(content)}


def logical_capture(metadata, identity):
    require(isinstance(metadata, dict) and metadata.get("backend") == "kindle"
            and metadata.get("demo") is True and metadata.get("offline") is True
            and metadata.get("route") == "shelf" and metadata.get("suspended") is False
            and metadata.get("pixel_semantics") == "Logical Slint snapshot before framebuffer compensation",
            "final_png_is_not_the_expected_logical_shelf_snapshot")
    startup = metadata.get("startup")
    require(isinstance(startup, dict) and startup.get("booting") is False
            and startup.get("error") is False, "final_logical_snapshot_is_not_ready")
    require(metadata.get("snapshot_size") == [identity["width"], identity["height"]],
            "logical_snapshot_dimensions_differ_from_metadata")
    require(metadata.get("native_epdc_mode") in ("Y8", "Y8INV"), "unknown_capture_epdc_mode")
    return {**identity, "semantics": metadata["pixel_semantics"],
            "profile": metadata.get("profile"), "native_epdc_mode": metadata["native_epdc_mode"],
            "route": "shelf", "captured_at": "Application exit after the bounded post-ready duration",
            "physical_framebuffer_capture": False}


def framebuffer_identity(content, native_epdc_mode):
    header, separator, pixels = content.partition(b"\n")
    require(separator and len(header) <= 8192, "invalid_framebuffer_header")
    geometry = decode_json(header)
    require(isinstance(geometry, dict) and geometry.get("read_only") is True,
            "framebuffer_capture_is_not_read_only")
    for field in ("width", "height", "virtual_width", "virtual_height", "line_length", "smem_len"):
        integer(geometry.get(field), "framebuffer_" + field, 1)
    for field in ("xoffset", "yoffset", "rotate", "visual", "grayscale", "visible_bytes"):
        integer(geometry.get(field), "framebuffer_" + field)
    width, height = geometry["width"], geometry["height"]
    require(geometry.get("bits_per_pixel") == 8 and 0 < width <= 10000 and 0 < height <= 10000
            and geometry["xoffset"] + width <= geometry["virtual_width"]
            and geometry["yoffset"] + height <= geometry["virtual_height"]
            and geometry["xoffset"] + width <= geometry["line_length"] <= 65536
            and geometry["visible_bytes"] == len(pixels) == width * height
            and geometry["rotate"] in range(4)
            and (geometry["grayscale"] == 1 or geometry["visual"] in (0, 1)),
            "unsupported_framebuffer_capture_geometry")
    first = geometry["yoffset"] * geometry["line_length"] + geometry["xoffset"]
    require(first + (height - 1) * geometry["line_length"] + width <= geometry["smem_len"],
            "framebuffer_capture_exceeds_mapped_memory")
    require(native_epdc_mode in ("Y8", "Y8INV"), "unknown_framebuffer_epdc_mode")
    invert = bytes(255 - value for value in range(256))
    memory = pixels.translate(invert) if geometry["visual"] == 0 else pixels
    projected = memory.translate(invert) if native_epdc_mode == "Y8INV" else memory
    return {"geometry": geometry, "file_sha256": sha(content), "visible_raw_sha256": sha(pixels),
            "declared_visual_gray_sha256": sha(memory), "derived_epdc_projection_sha256": sha(projected),
            "native_epdc_mode": native_epdc_mode,
            "mode_provenance": "Same isolated application's native EPDC mode in its final logical-snapshot sidecar; no intervening lifecycle transitions permitted",
            "framebuffer_visual_inversion_applied": geometry["visual"] == 0,
            "epdc_inversion_applied": native_epdc_mode == "Y8INV",
            "application_software_inversion_reapplied": False,
            "scope": "Read-only framebuffer memory at the named observation; derived projection is not an optical observation",
            "physical_panel_update_confirmed": False}


def compare_framebuffer_pair(batch, rows, capture_field, filename):
    from PIL import Image, ImageChops

    cases = ("baseline-06", "candidate-06")
    selected = {case: next(row for row in rows if row["case"] == case) for case in cases}
    require(all(capture_field in selected[case] for case in cases), "missing_" + filename)
    captures = {case: selected[case][capture_field] for case in cases}
    first, second = (captures[case] for case in cases)
    geometry_match = first["geometry"] == second["geometry"]
    mode_match = first["native_epdc_mode"] == second["native_epdc_mode"]
    output = {"cases": list(cases), "filename": filename, "geometry_match": geometry_match,
              "native_epdc_modes_match": mode_match,
              "visible_raw_sha256": {case: captures[case]["visible_raw_sha256"] for case in cases},
              "whole_visible_frame_equal": geometry_match and mode_match
                  and first["visible_raw_sha256"] == second["visible_raw_sha256"],
              "comparison_scope": "Every visible 8-bit framebuffer pixel; no excluded regions or masks",
              "physical_panel_update_confirmed": False}
    same_size = all(first["geometry"][field] == second["geometry"][field] for field in ("width", "height"))
    if not same_size:
        output.update(pixel_difference_status="incomparable_dimensions", changed_pixels=None,
                      changed_fraction=None, maximum_absolute_difference=None,
                      mean_absolute_difference=None, changed_bounding_box=None)
        return output
    images = []
    for case in cases:
        content = read_bytes(batch / "outputs" / "out" / case / filename, 64 * 1024 * 1024)
        require(sha(content) == captures[case]["file_sha256"], "changed_framebuffer_comparison_input")
        pixels = content.partition(b"\n")[2]
        geometry = captures[case]["geometry"]
        images.append(Image.frombytes("L", (geometry["width"], geometry["height"]), pixels))
    difference = ImageChops.difference(*images)
    histogram = difference.histogram()
    total = first["geometry"]["width"] * first["geometry"]["height"]
    changed = total - histogram[0]
    maximum = max(index for index, count in enumerate(histogram) if count)
    bounds = difference.getbbox()
    output.update(pixel_difference_status="compared_every_visible_pixel", visible_pixels=total,
                  changed_pixels=changed, changed_fraction=round(changed / total, 9),
                  maximum_absolute_difference=maximum,
                  mean_absolute_difference=round(sum(index * count for index, count in enumerate(histogram)) / total, 9),
                  changed_bounding_box=list(bounds) if bounds is not None else None)
    return output


def capture_window_observation(directory, prefix, business_send_ns, trace_finished_ns):
    values = {}
    digests = {}
    for phase in ("request", "complete"):
        filename = prefix + "-" + phase + ".json"
        content = read_bytes(directory / filename, 4096)
        value = decode_json(content)
        require(isinstance(value, dict), "invalid_" + prefix + "_capture_" + phase + "_marker")
        integer(value.get("monotonic_ns"), prefix + "_capture_" + phase + "_timestamp", 1)
        integer(value.get("pid"), prefix + "_capture_" + phase + "_pid", 2)
        if value.get("boottime_ns") is not None:
            integer(value["boottime_ns"], prefix + "_capture_" + phase + "_boottime", 1)
        values[phase] = value
        digests[filename] = sha(content)
    start = values["request"]["monotonic_ns"]
    end = values["complete"]["monotonic_ns"]
    require(business_send_ns <= start <= end, "invalid_" + prefix + "_capture_window")
    require(end < trace_finished_ns, prefix + "_capture_reaches_or_exceeds_ui_trace_terminal")
    return {"request_monotonic_ns": start, "complete_monotonic_ns": end,
            "request_after_first_business_send_ms": delta_ms(start, business_send_ns),
            "complete_after_first_business_send_ms": delta_ms(end, business_send_ns),
            "bracket_wall_ms": delta_ms(end, start),
            "scope": "Clock request before framebuffer-helper exec through completion marker after helper success; a software observation bracket, not optical-panel timing"}, digests


def analyze_case(batch, case, expected_probe_executable):
    directory = batch / "outputs" / "out" / case
    require(directory.is_dir() and not directory.is_symlink(), "missing_or_linked_case_directory")
    inputs = {name: read_bytes(directory / name, 32 * 1024 * 1024 if name == "final.png" else 8 * 1024 * 1024)
              for name in REQUIRED_FILES}
    recovery = decode_json(inputs["recovery.json"])
    require(isinstance(recovery, dict) and all(recovery.get(name) is True for name in
            ("passed", "native_restored", "owned_processes_gone", "production_lock_absent"))
            and type(recovery.get("launcher_exit_status")) in (int, str)
            and recovery["launcher_exit_status"] in (0, "0"), "case_recovery_failed")
    request = decode_json(inputs["request.json"])
    require(isinstance(request, dict), "invalid_request_record")
    origin = integer(request.get("monotonic_ns"), "request_monotonic_ns", 1)
    integer(request.get("pid"), "request_pid", 2)
    if request.get("boottime_ns") is not None:
        integer(request["boottime_ns"], "request_boottime_ns", 1)
    deployment = decode_json(inputs["deployed-identity.json"])
    require(isinstance(deployment, dict) and set(deployment) == {"binary_sha256", "native_query_sha256"}
            and isinstance(deployment["binary_sha256"], str)
            and re.fullmatch(r"[0-9a-f]{64}", deployment["binary_sha256"])
            and (deployment["native_query_sha256"] is None or
                 (isinstance(deployment["native_query_sha256"], str)
                  and re.fullmatch(r"[0-9a-f]{64}", deployment["native_query_sha256"]))),
            "invalid_deployed_identity")
    records = trace_records(inputs["performance.jsonl"])
    process_entries = [row for row in records if row.get("kind") == "startup" and row.get("stage") == "process_enter"]
    require(len(process_entries) == 1, "process_entry_is_not_unique")
    ready = [row for row in records if row.get("kind") == "startup" and row.get("stage") == "ui_ready"]
    require(len(ready) == 1, "software_ready_is_not_unique")
    ready_ns = integer(ready[0].get("monotonic_ns"), "software_ready_timestamp", 1)
    frames = [frame_observation(row, origin) for row in records if
              (row.get("kind"), row.get("stage")) in
              (("native", "frame"), ("startup", "native_initial_setup"))]
    rendered = [row for row in frames if row["record_kind"] == "native" and row["rendered"] is True
                and row["send_accepted_monotonic_ns"] is not None]
    startup = [row for row in rendered if row["send_accepted_monotonic_ns"] < ready_ns]
    business = [row for row in rendered if isinstance(row["stages"].get("slint_render"), dict)
                and type(row["stages"]["slint_render"].get("started_monotonic_ns")) is int
                and type(row["stages"]["slint_render"].get("finished_monotonic_ns")) is int
                and row["stages"]["slint_render"]["started_monotonic_ns"] >= ready_ns
                and row["stages"]["slint_render"]["started_monotonic_ns"]
                    <= row["stages"]["slint_render"]["finished_monotonic_ns"]
                    <= row["stages"]["refresh_submit"]["started_monotonic_ns"]
                and row["send_accepted_monotonic_ns"] >= ready_ns]
    require(startup and business, "missing_qualified_startup_or_business_frame")
    first_startup = min(startup, key=lambda row: row["send_accepted_monotonic_ns"])
    first_business = min(business, key=lambda row: row["send_accepted_monotonic_ns"])
    stop = next(row for row in records if row.get("kind") == "diagnostic" and row.get("stage") == "trace_stopped")
    stopped_ns = integer(stop.get("monotonic_ns"), "trace_finished_timestamp", 1)
    target_bytes = inputs["post-business-target-ns.txt"]
    require(re.fullmatch(rb"[1-9][0-9]{0,19}\n?", target_bytes), "invalid_post_business_target_record")
    target_ns = integer(int(target_bytes), "post_business_target", 1)
    require(target_ns == first_business["send_accepted_monotonic_ns"] + 1_500_000_000,
            "post_business_target_is_not_first_send_plus_1500ms")
    gate = decode_json(inputs["post-business-gate.json"])
    require(isinstance(gate, dict), "invalid_post_business_gate")
    gate_ns = integer(gate.get("monotonic_ns"), "post_business_gate_timestamp", 1)
    integer(gate.get("pid"), "post_business_gate_pid", 2)
    if gate.get("boottime_ns") is not None:
        integer(gate["boottime_ns"], "post_business_gate_boottime", 1)
    require(target_ns <= gate_ns < stopped_ns, "post_business_gate_is_early_or_after_shutdown")
    metrics = {"software_ready_ms": delta_ms(ready_ns, origin),
               "first_startup_send_ms": first_startup["send_accepted_after_request_ms"],
               "content_send_ms": first_business["send_accepted_after_request_ms"]}
    events = []
    spans = defaultdict(list)
    for row in records:
        if row.get("kind") != "startup":
            continue
        data = row["data"] or {}
        observed = data.get("observed")
        stamp = observed.get("monotonic_ns") if isinstance(observed, dict) else row.get("monotonic_ns")
        stamp = integer(stamp, "startup_event_timestamp", 1)
        event = {"stage": row["stage"], "monotonic_ns": stamp, "after_request_ms": delta_ms(stamp, origin)}
        for field in ("wall_us", "cpu_us", "face_index", "registered_fonts", "registered_count"):
            if field in data and (data[field] is None or type(data[field]) is int):
                event[field] = data[field]
        for field in ("phase", "outcome", "gate"):
            if field in data and isinstance(data[field], str) and re.fullmatch(r"[a-z0-9_-]{1,80}", data[field]):
                event[field] = data[field]
        if type(data.get("wall_us")) is int:
            spans[row["stage"]].append(data["wall_us"] / 1000)
        events.append(event)
        if row["stage"] == "process_enter":
            metrics["request_to_process_enter_ms"] = delta_ms(stamp, origin)
            metrics["process_enter_to_ready_ms"] = delta_ms(ready_ns, stamp)
    require("request_to_process_enter_ms" in metrics, "missing_process_entry")
    memory = memory_values(inputs["memory-at-content.txt"])
    metrics.update(rss_kib=memory["VmRSS"], hwm_kib=memory["VmHWM"])
    result = {"case": case, "variant": case.split("-", 1)[0], "warmup": case.endswith("-warmup"),
              "request": request, "application_pid": records[0]["process_id"], "metrics": metrics,
              "trace_finished_monotonic_ns": stopped_ns,
              "deployed_identity": deployment,
              "first_startup_frame": first_startup, "first_business_frame": first_business,
              "frame_attempts": frames, "startup_events": events,
              "startup_stage_wall_ms": {name: {"attempts": len(values), "sum": round(sum(values), 6),
                                                "values": values} for name, values in sorted(spans.items())},
              "shell_observations": shell_observations(inputs["shell.tsv"]),
              "input_devices": input_nodes(inputs["input-fds-at-content.txt"]), "memory": memory,
              "post_business_gate": {"reference": "First qualified business SEND acceptance", "offset_ms": 1500,
                  "target_monotonic_ns": target_ns, "request": gate,
                  "deadline_slip_ms": delta_ms(gate_ns, target_ns),
                  "scope": "Instrumented post-business gate request, preceding exec; not a startup endpoint or the exact subsequent sampling time"},
              "resource_observation_scope": "Memory and input descriptors are sampled after the common first-business SEND plus 1500 ms gate; no exact-time or whole-lifetime peak claim",
              "final_capture": logical_capture(decode_json(inputs["final.json"]), png_identity(inputs["final.png"])),
              "recovery": {name: recovery[name] for name in
                           ("passed", "native_restored", "owned_processes_gone", "production_lock_absent", "launcher_exit_status")},
              "file_sha256": {name: sha(content) for name, content in inputs.items()}}
    require(decode_json(inputs["final.json"]).get("process_id") == records[0]["process_id"],
            "final_logical_snapshot_process_differs")
    lifecycle_rows = [decode_json(line) for line in inputs["lifecycle.jsonl"].splitlines()]
    require(lifecycle_rows and all(isinstance(row, dict) for row in lifecycle_rows),
            "invalid_startup_lifecycle_evidence")
    require(all(row.get("event") not in ("suspend", "resume") for row in lifecycle_rows),
            "lifecycle_transition_during_startup_case")
    require(all(row.get("process_id") == records[0]["process_id"] for row in lifecycle_rows),
            "startup_lifecycle_process_differs")
    mode = result["final_capture"]["native_epdc_mode"]
    require(any(row.get("event") == "ready" and row.get("native_epdc_mode") == mode
                for row in lifecycle_rows), "mode_has_no_same_process_ready_record")
    require(all(row.get("native_epdc_mode") in (None, mode) for row in lifecycle_rows),
            "native_epdc_mode_changed_during_startup_case")
    raw_files = (("content-frame.raw", "framebuffer_capture"),
                 ("settled-frame.raw", "settled_framebuffer_capture"))
    if any((directory / filename).exists() or (directory / filename).is_symlink()
           for filename, _ in raw_files):
        for filename, field in raw_files:
            raw = directory / filename
            if raw.exists() or raw.is_symlink():
                raw_bytes = read_bytes(raw, 64 * 1024 * 1024)
                result["file_sha256"][filename] = sha(raw_bytes)
                result[field] = framebuffer_identity(raw_bytes, result["final_capture"]["native_epdc_mode"])
                result[field]["observation_phase"] = "early_content_observation" if field == "framebuffer_capture" else "scheduled_post_business_observation"
    if "settled_framebuffer_capture" in result:
        request_bytes = read_bytes(directory / "settled-request.json", 4096)
        settled_request = decode_json(request_bytes)
        require(isinstance(settled_request, dict), "invalid_settled_request")
        observed_ns = integer(settled_request.get("monotonic_ns"), "settled_capture_start", 1)
        integer(settled_request.get("pid"), "settled_request_pid", 2)
        if settled_request.get("boottime_ns") is not None:
            integer(settled_request["boottime_ns"], "settled_request_boottime", 1)
        target_bytes = read_bytes(directory / "settled-deadline-ns.txt", 64)
        require(re.fullmatch(rb"[1-9][0-9]{0,19}\n?", target_bytes), "invalid_settled_deadline_record")
        target_ns = integer(int(target_bytes), "settled_deadline", 1)
        require(target_ns == first_business["send_accepted_monotonic_ns"] + 1_500_000_000,
                "settled_target_is_not_first_send_plus_1500ms")
        require(target_ns <= observed_ns < stopped_ns, "settled_capture_did_not_start_at_or_after_target_while_active")
        require(request_bytes == inputs["post-business-gate.json"] and target_bytes == inputs["post-business-target-ns.txt"],
                "settled_capture_does_not_match_common_post_business_gate")
        result["settled_framebuffer_capture"]["observation_timing"] = {
            "target_monotonic_ns": target_ns, "target_after_first_business_send_ms": 1500,
            "capture_request": settled_request,
            "capture_request_after_first_business_send_ms": delta_ms(observed_ns, first_business["send_accepted_monotonic_ns"]),
            "deadline_slip_ms": delta_ms(observed_ns, target_ns),
            "scope": "Scheduled framebuffer capture request before record write/close/exec; not exact framebuffer pread start, startup readiness or SEND endpoint"}
        result["file_sha256"].update({"settled-request.json": sha(request_bytes),
                                      "settled-deadline-ns.txt": sha(target_bytes)})
    require(case not in ("baseline-06", "candidate-06") or all(field in result for _, field in raw_files),
            "missing_measured_early_or_settled_framebuffer_capture")
    if case in ("baseline-06", "candidate-06"):
        windows = {}
        for prefix, field in (("early", "framebuffer_capture"), ("settled", "settled_framebuffer_capture")):
            window, digests = capture_window_observation(directory, prefix,
                first_business["send_accepted_monotonic_ns"], stopped_ns)
            for filename, digest in digests.items():
                require(filename not in result["file_sha256"] or result["file_sha256"][filename] == digest,
                        "capture_clock_evidence_changed_between_reads")
                result["file_sha256"][filename] = digest
            windows[prefix] = window
            result[field]["capture_window"] = window
        require(windows["early"]["complete_monotonic_ns"] <= windows["settled"]["request_monotonic_ns"],
                "early_and_settled_capture_windows_overlap_or_are_reversed")
        liveness_bytes = read_bytes(directory / "capture-liveness.json", 4096)
        liveness = decode_json(liveness_bytes)
        require(isinstance(liveness, dict)
                and integer(liveness.get("application_pid"), "capture_application_pid", 2) == records[0]["process_id"],
                "capture_liveness_ui_process_differs")
        integer(liveness.get("start_ticks"), "capture_application_start_ticks", 1)
        require(all(liveness.get(field) is True for field in
                    ("early_before", "early_after", "settled_before", "settled_after")),
                "capture_liveness_is_not_proven")
        result["file_sha256"]["capture-liveness.json"] = sha(liveness_bytes)
        result["capture_liveness_status"] = "same_ui_process_verified_before_and_after_both_capture_windows"
    calibration_paths = list(directory.glob("calibration-*"))
    if case == "diagnostic-01":
        process_entry = next(event["monotonic_ns"] for event in events if event["stage"] == "process_enter")
        result["native_query_calibrations"] = calibration_observations(directory, origin, records[0]["process_id"], process_entry)
        result["native_query_abi_probe"] = abi_probe_observation(directory, deployment["native_query_sha256"], expected_probe_executable)
    else:
        require(not calibration_paths, "unexpected_calibration_trace_outside_excluded_diagnostic_case")
    return result


def distribution(values):
    require(values and all(math.isfinite(value) for value in values), "invalid_summary_values")
    return {"n": len(values), "values": values, "median": round(statistics.median(values), 6),
            "mean": round(statistics.mean(values), 6), "minimum": min(values), "maximum": max(values)}


def analyze_batch(batch):
    require(sys.platform == "linux", "run_only_on_test_env")
    batch = batch.resolve(strict=True)
    report_bytes = read_bytes(batch / "report.json", 2 * 1024 * 1024)
    run = decode_json(report_bytes)
    require(isinstance(run, dict) and all(run.get(name) is True for name in RESTORATION),
            "batch_not_successfully_restored_and_released")
    variants = validate_variants(run)
    rounds = validate_schedule(run.get("cases"))
    metadata = validate_case_metadata(run)
    expected_probe_executable = validate_deployment_context(run)
    rows = []
    previous_trace_finished_ns = None
    for case in run["cases"]:
        try:
            row = analyze_case(batch, case, expected_probe_executable)
            row.update(metadata[case])
            selected = variants[row["software_variant"]]
            require(row["deployed_identity"]["binary_sha256"] == selected["binary_sha256"]
                    and row["deployed_identity"]["native_query_sha256"] == selected["deployed_native_query_sha256"],
                    "case_deployed_binary_or_helper_identity_differs")
            row["included_in_primary_statistics"] = metadata[case]["measurement_role"] == "primary"
            require(previous_trace_finished_ns is None or row["request"]["monotonic_ns"] > previous_trace_finished_ns,
                    "actual_case_request_order_or_overlap_differs")
            previous_trace_finished_ns = row["trace_finished_monotonic_ns"]
            rows.append(row)
        except (AnalysisError, OSError, ValueError, KeyError, TypeError) as error:
            raise AnalysisError(case + ": " + (str(error) if isinstance(error, AnalysisError) else type(error).__name__)) from error
    require(len({tuple(row["input_devices"]) for row in rows}) == 1, "selected_input_devices_differ")
    captures = {(row["final_capture"]["width"], row["final_capture"]["height"],
                 row["final_capture"]["pixel_sha256"]) for row in rows}
    require(len(captures) == 1, "final_business_pixels_or_dimensions_differ")
    early_comparison = compare_framebuffer_pair(batch, rows, "framebuffer_capture", "content-frame.raw")
    settled_comparison = compare_framebuffer_pair(batch, rows, "settled_framebuffer_capture", "settled-frame.raw")
    require(settled_comparison["whole_visible_frame_equal"], "settled_framebuffer_pixels_or_geometry_differ")
    metric_names = set(rows[0]["metrics"])
    require(all(set(row["metrics"]) == metric_names for row in rows), "case_metric_inventory_differs")
    summary = {variant: {name: distribution([row["metrics"][name] for row in rows
                       if row["variant"] == variant and row["included_in_primary_statistics"]]) for name in sorted(metric_names)}
               for variant in VARIANTS}
    by_case = {row["case"]: row for row in rows}
    paired = []
    primary = ("software_ready_ms", "content_send_ms")
    for number in range(1, rounds + 1):
        pair = {name: by_case[f"{name}-{number:02d}"] for name in VARIANTS}
        paired.append({"round": number, "order": list(VARIANTS if number % 2 else reversed(VARIANTS)),
                       "metrics": {metric: {"baseline": pair["baseline"]["metrics"][metric],
                                   "candidate": pair["candidate"]["metrics"][metric],
                                   "reduction_ms": round(pair["baseline"]["metrics"][metric]
                                                     - pair["candidate"]["metrics"][metric], 6)}
                                   for metric in primary}})
    reductions = {}
    for metric in primary:
        baseline = summary["baseline"][metric]["median"]
        candidate = summary["candidate"][metric]["median"]
        reductions[metric] = {"difference_of_medians_ms": round(baseline - candidate, 6),
                              "median_paired_reduction_ms": round(statistics.median(
                                  [pair["metrics"][metric]["reduction_ms"] for pair in paired]), 6),
                              "percent_from_medians": round(100 * (baseline - candidate) / baseline, 3)
                              if baseline > 0 else None}
    return {"schema_version": 1, "status": "passed", "evidence_class": "physical-device",
            "protocol": "One warmup per primary variant, six alternating AB/BA pairs, then excluded compatibility and diagnostic launches in one Kindle boot; no cache eviction.",
            "scope": "Native appmgr/Scriptlet request to software readiness and first business SEND acceptance.",
            "clock": {"primary": "CLOCK_MONOTONIC", "request": "startup-clock immediately before execvp",
                      "software_ready": "startup/ui_ready envelope monotonic_ns",
                      "content_send": "native/frame refresh_submit.finished_monotonic_ns, rendered and accepted, with slint_render starting after ui_ready",
                      "shell_uptime": "Separate coarse phase observations only; never used for primary endpoints"},
            "source_report_sha256": sha(report_bytes), "variants": variants, "cases": rows,
            "primary_cases": [row["case"] for row in rows if row["measurement_role"] in ("warmup", "primary")],
            "excluded_cases": [row for row in rows if row["measurement_role"] in ("compatibility", "diagnostic")],
            "excluded_checks": {
                "compatibility": {"case": "compat-01", "same_candidate_application_elf": True,
                    "optional_sibling_helper_omitted": True, "startup_and_native_restoration_succeeded": True,
                    "scope": "Independent staged directory with the same candidate package/application and no optional native-query sibling; no shared-file rename or synthetic success timer"},
                "diagnostic": {"case": "diagnostic-01", "same_candidate_application_and_helper": True,
                    "startup_and_native_restoration_succeeded": True,
                    "calibrations": by_case["diagnostic-01"]["native_query_calibrations"],
                    "direct_c_abi_probe": by_case["diagnostic-01"]["native_query_abi_probe"],
                    "scope": "Per-capture helper session/query/clean-finish observations; excluded from all primary performance distributions"}},
            "measured_pairs": rounds, "summary": summary, "paired_rounds": paired,
            "median_reductions": reductions, "selected_input_devices_match": True,
            "final_logical_business_pixels_match": True,
            "early_framebuffer_memory_pixels_match": early_comparison["whole_visible_frame_equal"],
            "settled_framebuffer_memory_pixels_match": True,
            "framebuffer_comparisons": {"early": early_comparison, "settled": settled_comparison},
            "restoration": {name: run[name] for name in RESTORATION},
            "limitations": ["Warm process launches in one device boot; not cold boot or cache-evicted starts.",
                            "Request timing precedes execvp of the native launch command; it is not physical-tap latency.",
                            "SEND acceptance is a successful submission, not driver completion or optical panel completion.",
                            "Driver completion flags are retained per attempt and never substituted for primary endpoints.",
                            "First business frame does not establish that every background cover has completed.",
                            "Final PNGs are logical Slint snapshots at application exit, before framebuffer compensation.",
                            "Compatibility and diagnostic launches are retained and validated but excluded from primary performance distributions.",
                            "Memory/input sampling follows a common first-business SEND plus 1500 ms gate; the clock marker is not an exact subsequent sample time or a whole-lifetime peak measurement.",
                            "The content-frame.raw pair retains every early visible-pixel difference; it is not required to match and is never masked.",
                            "The settled-frame.raw pair must match at a scheduled target of first business SEND acceptance plus 1500 ms; actual capture-request lateness is reported separately.",
                            "The scheduled framebuffer observation is separate from all primary startup endpoints; any EPDC projection remains derived rather than optical evidence.",
                            "No new touch, rotation, sleep/wake or model qualification.",
                            "Shell uptime records retain their limited resolution and scheduling delay.",
                            "Stage spans may overlap and must not be summed into end-to-end startup time."]}


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--batch-dir", type=Path, required=True)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args(argv)
    try:
        report = analyze_batch(args.batch_dir)
        output = args.output or args.batch_dir / "analysis.json"
        encoded = (json.dumps(report, indent=2, sort_keys=True, allow_nan=False) + "\n").encode("utf-8")
        fd = os.open(output, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW | os.O_CLOEXEC, 0o600)
        with os.fdopen(fd, "wb") as stream:
            stream.write(encoded)
        print(json.dumps({name: report[name] for name in ("status", "measured_pairs", "summary",
                         "median_reductions", "restoration", "selected_input_devices_match",
                         "final_logical_business_pixels_match", "early_framebuffer_memory_pixels_match",
                         "settled_framebuffer_memory_pixels_match")}, indent=2, allow_nan=False))
        return 0
    except (AnalysisError, OSError, ValueError, KeyError, TypeError, ImportError) as error:
        print(json.dumps({"status": "failed", "reason": str(error) if isinstance(error, AnalysisError)
                          else type(error).__name__}), file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
