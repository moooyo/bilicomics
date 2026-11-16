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


BASELINE_COMMIT = "06ad90b02edb28db43abeed3031932a91dc2f8df"
VARIANTS = ("baseline", "candidate")
RESTORATION = ("success", "same_boot", "install_unchanged", "lighting_restored",
               "transport_closed", "native_restored", "lease_released")
REQUIRED_FILES = ("request.json", "performance.jsonl", "shell.tsv", "recovery.json",
                  "input-fds-at-content.txt", "memory-at-content.txt", "final.png", "final.json",
                  "lifecycle.jsonl")
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
    require(isinstance(variants, dict) and set(variants) == set(VARIANTS), "invalid_variants")
    output = {}
    builds = []
    for name in VARIANTS:
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
        build = source.get("build_configuration", run.get("build_configuration"))
        require(build is None or (isinstance(build, dict) and build), "invalid_build_configuration")
        builds.append(build)
        if build is not None:
            identity["build_configuration"] = build
        output[name] = identity
    require(output["baseline"]["source_commit"] == BASELINE_COMMIT, "baseline_commit_differs")
    require(output["baseline"]["source_fingerprint"] != output["candidate"]["source_fingerprint"],
            "variant_source_fingerprints_are_identical")
    require(builds[0] == builds[1], "variant_build_configurations_differ")
    require(output["baseline"]["compiler_fingerprint"] == output["candidate"]["compiler_fingerprint"],
            "compiler_configuration_fingerprints_differ")
    return output


def validate_schedule(cases):
    require(isinstance(cases, list) and all(isinstance(case, str) for case in cases)
            and len(cases) == len(set(cases)), "invalid_case_schedule")
    require(12 <= len(cases) <= 102 and (len(cases) - 2) % 2 == 0, "insufficient_or_invalid_pairs")
    rounds = (len(cases) - 2) // 2
    expected = ["baseline-warmup", "candidate-warmup"]
    for number in range(1, rounds + 1):
        order = VARIANTS if number % 2 else tuple(reversed(VARIANTS))
        expected.extend(f"{name}-{number:02d}" for name in order)
    require(cases == expected, "case_order_is_not_warmup_then_alternating_ab_ba")
    return rounds


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
    return records


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

    cases = ("baseline-05", "candidate-05")
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


def analyze_case(batch, case):
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
    records = trace_records(inputs["performance.jsonl"])
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
              "first_startup_frame": first_startup, "first_business_frame": first_business,
              "frame_attempts": frames, "startup_events": events,
              "startup_stage_wall_ms": {name: {"attempts": len(values), "sum": round(sum(values), 6),
                                                "values": values} for name, values in sorted(spans.items())},
              "shell_observations": shell_observations(inputs["shell.tsv"]),
              "input_devices": input_nodes(inputs["input-fds-at-content.txt"]), "memory": memory,
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
                result[field]["observation_phase"] = "early_content_observation" if field == "framebuffer_capture" else "scheduled_post_ready_observation"
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
        require(target_ns == ready_ns + 1_000_000_000, "settled_target_is_not_ready_plus_one_second")
        stop = next(row for row in records if row.get("kind") == "diagnostic" and row.get("stage") == "trace_stopped")
        stopped_ns = integer(stop.get("monotonic_ns"), "trace_finished_timestamp", 1)
        require(target_ns <= observed_ns < stopped_ns, "settled_capture_did_not_start_at_or_after_target_while_active")
        result["settled_framebuffer_capture"]["observation_timing"] = {
            "target_monotonic_ns": target_ns, "target_after_ui_ready_ms": 1000,
            "capture_request": settled_request, "capture_start_after_ui_ready_ms": delta_ms(observed_ns, ready_ns),
            "deadline_slip_ms": delta_ms(observed_ns, target_ns),
            "scope": "Scheduled framebuffer capture start only; not a startup-readiness or SEND endpoint"}
        result["file_sha256"].update({"settled-request.json": sha(request_bytes),
                                      "settled-deadline-ns.txt": sha(target_bytes)})
    require(not case.endswith("-05") or all(field in result for _, field in raw_files),
            "missing_measured_early_or_settled_framebuffer_capture")
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
    rows = []
    for case in run["cases"]:
        try:
            rows.append(analyze_case(batch, case))
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
                       if row["variant"] == variant and not row["warmup"]]) for name in sorted(metric_names)}
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
            "protocol": "One warmup per variant, then alternating AB/BA pairs in one Kindle boot; no cache eviction.",
            "scope": "Native appmgr/Scriptlet request to software readiness and first business SEND acceptance.",
            "clock": {"primary": "CLOCK_MONOTONIC", "request": "startup-clock immediately before execvp",
                      "software_ready": "startup/ui_ready envelope monotonic_ns",
                      "content_send": "native/frame refresh_submit.finished_monotonic_ns, rendered and accepted, with slint_render starting after ui_ready",
                      "shell_uptime": "Separate coarse phase observations only; never used for primary endpoints"},
            "source_report_sha256": sha(report_bytes), "variants": variants, "cases": rows,
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
                            "The content-frame.raw pair retains every early visible-pixel difference; it is not required to match and is never masked.",
                            "The settled-frame.raw pair must match at a scheduled target of ui_ready plus one second; actual capture-start lateness is reported separately.",
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
