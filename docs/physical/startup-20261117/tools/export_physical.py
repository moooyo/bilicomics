#!/usr/bin/env python3
"""Export only saved, synthetic startup evidence; never contact a device."""

import argparse
from collections import Counter
import hashlib
import json
from pathlib import Path
import re
import shutil
import statistics

from PIL import Image


EXPECTED = {
    "report": "262850b321af04d8f28eab546ba3a3be35e1fcd64c4da3c82e455039b5d91745",
    "analysis": "9e140c7ac46489d56388485d4868f6731854d85acd38a9ac873ac793c137b770",
    "stage_projection": "e63e10e8f6abe0c10e6854283692b24d2902696d6a49ac379b2de605a5d4df1c",
    "compiler": "3c0653b868be7e1ff7bad053b10a16924d8fbc4a05fea4dff047a6548890739e",
    "frozen_tools": "25335f4ed493f0ee3d6ea4245a7c5658ee4c677f9123e7ed6398095f0afd2c44",
    "fixture": "a930027e7e297c89b61c963d2dd17d30c605bca3d4260e1805b7aea2257f3f79",
}
VARIANTS = ("baseline", "candidate")
FRAME_KEYS = (
    "send_accepted_after_request_ms", "record_after_request_ms", "record_stage",
    "render_sequence", "rendered", "initial_frame", "complete_frame_replacement",
    "forced_full", "refresh_accepted", "refresh_ioctl_returned", "refresh_abi",
    "refresh_ioctl_attempts", "refresh_waveform", "refresh_flashing", "refresh_region",
    "accepted_update_marker", "completed_update_marker", "preceding_update_marker",
    "driver_completion_observed", "panel_completion_observed", "refresh_errno",
    "completion_errno", "framebuffer_size", "framebuffer_bits_per_pixel",
    "framebuffer_stride", "rotation_degrees", "output_inverted", "black_and_white",
)
VARIANT_KEYS = (
    "source_commit", "source_fingerprint", "binary_sha256", "worker_sha256",
    "package_sha256", "launcher_sha256", "instrumented_launcher_sha256", "compiler_fingerprint",
)
RAW_HEADER_KEYS = (
    "width", "height", "virtual_width", "virtual_height", "xoffset", "yoffset",
    "bits_per_pixel", "line_length", "smem_len", "visual", "grayscale", "rotate",
    "red_offset", "red_length", "red_msb_right", "green_offset", "green_length",
    "green_msb_right", "blue_offset", "blue_length", "blue_msb_right", "byte_order",
    "visible_bytes", "read_only",
)


def require(value, message):
    if not value:
        raise AssertionError(message)


def sha256(path):
    result = hashlib.sha256()
    with Path(path).open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            result.update(chunk)
    return result.hexdigest()


def sha_bytes(value):
    return hashlib.sha256(value).hexdigest()


def read_json(path):
    return json.loads(Path(path).read_text(encoding="utf-8"))


def selected_json(path, expected):
    require(sha256(path) == expected, "A selected saved evidence identity changed")
    return read_json(path)


def write_json(path, record):
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("x", encoding="utf-8", newline="\n") as output:
        json.dump(record, output, ensure_ascii=False, indent=2)
        output.write("\n")


def numbers(values):
    require(values, "An evidence statistic has no observations")
    return {"n": len(values), "values": values, "minimum": min(values), "maximum": max(values),
            "median": round(statistics.median(values), 6), "mean": round(statistics.mean(values), 6)}


def json_lines(path, expected):
    require(sha256(path) == expected, "A selected original trace changed")
    return [json.loads(line) for line in path.read_text(encoding="utf-8").splitlines() if line.strip()]


def safe_stage_projection(value):
    forbidden = {"owner", "owner_thread", "lease_token", "token", "nonce", "account", "account_id",
                 "session", "cookie", "credential", "password", "private_key"}
    def inspect(item):
        if isinstance(item, dict):
            require(not (set(key.lower() for key in item) & forbidden), "Private fields cannot be exported")
            for child in item.values():
                inspect(child)
        elif isinstance(item, list):
            for child in item:
                inspect(child)
        elif isinstance(item, str):
            require(not any(marker in item for marker in ("SESSDATA=", "bili_jct=", "BEGIN PRIVATE KEY")),
                    "Private values cannot be exported")
    inspect(value)


def main(arguments):
    batch = arguments.batch_root.resolve(strict=True)
    software = arguments.software_root.resolve(strict=True)
    measurement = software / "measurement-tools"
    report = selected_json(batch / "report.json", EXPECTED["report"])
    analysis = selected_json(batch / "analysis-r1.json", EXPECTED["analysis"])
    stage_path = batch / "privatehost/stage-projection.json"
    stage = selected_json(stage_path, EXPECTED["stage_projection"])
    safe_stage_projection(stage)
    require(stage["source_analysis_sha256"] == EXPECTED["analysis"], "Stage evidence has another analysis source")
    require(report["success"] is True and not report["cancelled"] and analysis["status"] == "passed",
            "The selected physical batch did not complete successfully")
    require(analysis["source_report_sha256"] == EXPECTED["report"] and len(analysis["cases"]) == 12
            and analysis["measured_pairs"] == 5, "The selected report and analysis are not the accepted batch")
    fixture_path = software / "physical-inputs-r1/fixture-manifest.json"
    fixture = read_json(fixture_path)
    canonical = (json.dumps(fixture, sort_keys=True, separators=(",", ":")) + "\n").encode("utf-8")
    require(len(fixture) == 37 and sha_bytes(canonical) == EXPECTED["fixture"], "The selected fixture fingerprint changed")
    preparation = read_json(batch / "evidence/preparation.json")
    require(preparation["fixture_fingerprint"] == EXPECTED["fixture"], "The physical preparation used another fixture")
    compiler_path = software / "physical-inputs-r1/compiler-profile.json"
    compiler = selected_json(compiler_path, EXPECTED["compiler"])
    frozen_path = measurement / "frozen-tools-r2.json"
    frozen = selected_json(frozen_path, EXPECTED["frozen_tools"])
    require(frozen["host_tests"] == {"batch": 23, "analysis_and_clock": 26, "total": 49, "failures": 0},
            "The selected tool verification changed")
    output = arguments.output.resolve()
    output.mkdir(mode=0o700, parents=True, exist_ok=False)
    exported = []

    def add_file(path, role, source_tag, source_sha256=None, original_bytes=True):
        exported.append({"path": path.relative_to(output).as_posix(), "bytes": path.stat().st_size,
                         "sha256": sha256(path), "role": role, "source_tag": source_tag,
                         "source_sha256": source_sha256, "original_bytes": original_bytes})

    def copy(original, name, role, source_tag, expected=None):
        require(original.is_file() and not original.is_symlink(), "The selected saved input is not a regular file")
        identity = sha256(original)
        if expected is not None:
            require(identity == expected, "A selected saved source changed")
        target = output / name
        target.parent.mkdir(parents=True, exist_ok=True)
        require(not target.exists(), "Refusing to overwrite exported evidence")
        shutil.copyfile(original, target)
        require(sha256(target) == identity, "Copying changed saved evidence bytes")
        add_file(target, role, source_tag, identity)
        return target

    def project(name, record, role, source_tag, source_sha256=None):
        target = output / name
        write_json(target, record)
        add_file(target, role, source_tag, source_sha256, False)
        return target

    copy(stage_path, "stage-projection.json", "Original audited anonymous stage projection", "physical-stage-analysis",
         EXPECTED["stage_projection"])
    copy(compiler_path, "sources/compiler-profile.json", "Original shared compiler profile", "matched-build-controls", EXPECTED["compiler"])
    copy(frozen_path, "sources/frozen-tools-r2.json", "Original frozen host-tool identity record", "host-tools-r2", EXPECTED["frozen_tools"])
    for name in ("MEASUREMENT_PLAN.md", "analyze_startup_ab.py", "startup-clock.c"):
        copy(measurement / name, "tools/" + name, "Final BiliComics protocol or analysis source", "host-tools-r2",
             frozen["files"][name]["sha256"])
    copy(Path(__file__), "tools/export_physical.py", "Saved-artifact allowlist exporter", "public-exporter")
    sources = {variant: {key: analysis["variants"][variant][key] for key in VARIANT_KEYS} for variant in VARIANTS}
    require(all(source["compiler_fingerprint"] == EXPECTED["compiler"] for source in sources.values()),
            "Compiler profiles differ between variants")
    metrics_by_case, public_cases = {}, []
    for case in analysis["cases"]:
        tag = case["case"]
        require(re.fullmatch(r"(?:baseline|candidate)-(?:warmup|0[1-5])", tag), "The case source tag is not anonymous")
        directory = batch / "outputs/out" / tag
        identities = case["file_sha256"]
        final = selected_json(directory / "final.json", identities["final.json"])
        lifecycle = json_lines(directory / "lifecycle.jsonl", identities["lifecycle.jsonl"])
        performance = json_lines(directory / "performance.jsonl", identities["performance.jsonl"])
        recovery = selected_json(directory / "recovery.json", identities["recovery.json"])
        require(all(recovery[key] == case["recovery"][key] for key in case["recovery"]),
                "A normalized recovery result differs from the original")
        pids = {row["process_id"] for row in lifecycle if "process_id" in row}
        performance_pids = {row["process_id"] for row in performance if "process_id" in row}
        same_pid = pids == performance_pids == {case["application_pid"]} and final["process_id"] == case["application_pid"]
        events = Counter(row.get("event", "missing") for row in lifecycle)
        no_power_events = set(events).issubset({"reader-checkpoint", "ready", "startup-stage", "ui-state"})
        suspended = sum(row.get("suspended") is True for row in lifecycle)
        require(same_pid and no_power_events and suspended == 0 and final["suspended"] is False,
                "The saved lifecycle does not meet the accepted active same-process scope")
        ready_events = [event for event in case["startup_events"] if event["stage"] == "ui_ready"]
        require(len(ready_events) == 1, "The saved case does not have one readiness event")
        ready = ready_events[0]["monotonic_ns"]
        content = case["first_business_frame"]["send_accepted_monotonic_ns"]
        metrics = dict(case["metrics"])
        metrics["ready_to_content_send_ms"] = round((content - ready) / 1_000_000, 6)
        require(abs(metrics["ready_to_content_send_ms"] - (metrics["content_send_ms"] - metrics["software_ready_ms"])) < 0.000002,
                "The independent per-case endpoint interval differs")
        metrics_by_case[tag] = metrics
        public_cases.append({
            "source_tag": tag, "variant": case["variant"], "warmup": case["warmup"],
            "included_in_statistics": not case["warmup"], "metrics": metrics,
            "memory_kib": {key: case["memory"][key] for key in ("VmRSS", "VmHWM", "RssAnon", "RssFile", "RssShmem")},
            "memory_scope": "One /proc/status reading near first content; VmHWM at that reading, not continuous full-run peak monitoring",
            "input_nodes": case["input_devices"],
            "lifecycle": {"same_pid_in_final_lifecycle_and_performance": same_pid,
                          "records": len(lifecycle), "event_counts": dict(events),
                          "power_or_unknown_event_count": sum(count for event, count in events.items()
                               if event not in {"reader-checkpoint", "ready", "startup-stage", "ui-state"}),
                          "suspended_true_records": suspended, "final_suspended": final["suspended"],
                          "scope": "Recorded lifecycle only; no new sleep/wake or touch acceptance"},
            "first_startup_frame": {key: case["first_startup_frame"][key] for key in FRAME_KEYS},
            "first_business_frame": {key: case["first_business_frame"][key] for key in FRAME_KEYS},
            "recovery": {key: case["recovery"][key] for key in ("launcher_exit_status", "native_restored", "owned_processes_gone", "passed", "production_lock_absent")},
            "final_logical_capture": dict(case["final_capture"]),
            "private_original_file_sha256": {name: identities[name] for name in (
                "final.json", "final.png", "lifecycle.jsonl", "performance.jsonl", "recovery.json",
                "request.json", "memory-at-content.txt", "input-fds-at-content.txt")},
        })
    require(len([case for case in public_cases if case["warmup"]]) == 2, "Warmup selection changed")
    summaries = {}
    for variant in VARIANTS:
        measured = [case for case in public_cases if case["variant"] == variant and not case["warmup"]]
        require(len(measured) == 5, "The selected measured case count changed")
        summaries[variant] = {key: numbers([case["metrics"][key] for case in measured]) for key in measured[0]["metrics"]}
    metric_names = tuple(metrics_by_case["baseline-01"])
    pairs = []
    for index in range(1, 6):
        expected_order = ["baseline", "candidate"] if index % 2 else ["candidate", "baseline"]
        require(analysis["paired_rounds"][index - 1]["order"] == expected_order, "The saved pair order changed")
        pair = {"round": index, "order": expected_order, "metrics": {}}
        for key in metric_names:
            baseline = metrics_by_case[f"baseline-{index:02d}"][key]
            candidate = metrics_by_case[f"candidate-{index:02d}"][key]
            pair["metrics"][key] = {"baseline": baseline, "candidate": candidate,
                "candidate_minus_baseline": round(candidate - baseline, 6),
                "baseline_minus_candidate": round(baseline - candidate, 6)}
        pairs.append(pair)
    paired_deltas = {key: {"candidate_minus_baseline": numbers([pair["metrics"][key]["candidate_minus_baseline"] for pair in pairs]),
                          "baseline_minus_candidate": numbers([pair["metrics"][key]["baseline_minus_candidate"] for pair in pairs])}
                     for key in metric_names}
    release_path = batch / "evidence/lease-released.json"
    release = read_json(release_path)
    release_keys = ("ok", "owner_and_token_checked", "native_restored_and_connections_closed",
                    "only_owned_owner_json_removed_then_rmdir", "lease_directory_absent_after_release", "private_token_retained")
    require(all(release[key] is True for key in release_keys), "The saved exact-release receipt is incomplete")
    closure_path = batch / "evidence/transport-closure.json"
    closure = read_json(closure_path)
    require(closure["connections_closed"] is True and closure["active_owned_transports"] == [],
            "The saved transport closure receipt is incomplete")
    command_names = [command["name"] for command in report["commands"]]
    require(len(command_names) == 42 and all(command["returncode"] == 0 and not command["timeout"]
            and not command["interrupted"] for command in report["commands"]), "A recorded host operation failed")
    fallback_commands = sum("fallback" in name for name in command_names)
    require(fallback_commands == 0, "A fallback recovery command was recorded")
    image_records = []
    settled_observations = {}
    for variant in VARIANTS:
        tag = variant + "-05"
        directory = batch / "outputs/out" / tag
        selected = next(case for case in analysis["cases"] if case["case"] == tag)
        ready = next(event["monotonic_ns"] for event in selected["startup_events"] if event["stage"] == "ui_ready")
        target_path = directory / "settled-deadline-ns.txt"
        target = int(target_path.read_text().strip())
        observation_path = directory / "settled-request.json"
        observation = read_json(observation_path)
        require(target == ready + 1_000_000_000, "The saved settled target has another readiness origin")
        settled_observations[tag] = {"source_tag": tag, "scheduled_after_ready_ms": 1000,
            "actual_request_after_ready_ms": round((observation["monotonic_ns"] - ready) / 1_000_000, 6),
            "request_lateness_ms": round((observation["monotonic_ns"] - target) / 1_000_000, 6),
            "late": observation["monotonic_ns"] > target,
            "clock_scope": "CLOCK_MONOTONIC marker immediately before the framebuffer helper exec; not optical timing",
            "target_record_sha256": sha256(target_path), "request_record_sha256": sha256(observation_path)}
        for phase, raw_name in (("early", "content-frame.raw"), ("settled", "settled-frame.raw")):
            original = directory / raw_name
            content = original.read_bytes()
            header_bytes, pixels = content.split(b"\n", 1)
            header = json.loads(header_bytes)
            require(set(RAW_HEADER_KEYS).issubset(header) and header["width"] == 1860 and header["height"] == 2480
                    and header["bits_per_pixel"] == 8 and header["read_only"] is True
                    and header["line_length"] == 1872 and len(pixels) == 1860 * 2480 == header["visible_bytes"],
                    "The saved application framebuffer has unsupported geometry or payload")
            require(sha_bytes(pixels) == analysis["framebuffer_comparisons"][phase]["visible_raw_sha256"][tag],
                    "The selected framebuffer pixels differ from the accepted analysis")
            target_png = output / "images" / (variant + "-" + phase + ".png")
            target_png.parent.mkdir(parents=True, exist_ok=True)
            image = Image.frombytes("L", (1860, 2480), pixels)
            image.save(target_png, format="PNG")
            require(Image.open(target_png).tobytes() == pixels, "Lossless framebuffer projection changed pixels")
            add_file(target_png, "Lossless application framebuffer-memory projection", tag + "/" + raw_name,
                     sha_bytes(content), False)
            metadata = {"source_tag": tag, "phase": phase, "evidence_kind": "Derived lossless PNG projection of saved framebuffer memory",
                        "physical_device_used_during_export": False, "optical_capture": False,
                        "pixel_processing": "None; packed visible Gray8 bytes mapped directly to an 8-bit L PNG; no crop, resize, rotation, inversion, masking, or tone adjustment",
                        "raw_source_filename": raw_name, "raw_source_sha256": sha_bytes(content),
                        "visible_pixels_sha256": sha_bytes(pixels), "png_sha256": sha256(target_png),
                        "raw_header": {key: header[key] for key in RAW_HEADER_KEYS},
                        "source_payload_layout": "The framebuffer reader packed width bytes from each line_length-stride row; padding is absent from the saved visible payload",
                        "raw_retained_privately": True, "source_commit": sources[variant]["source_commit"],
                        "binary_sha256": sources[variant]["binary_sha256"]}
            if phase == "settled":
                metadata["scheduled_observation"] = settled_observations[tag]
            project("images/" + variant + "-" + phase + ".json", metadata, "Framebuffer projection sidecar", tag + "/" + raw_name,
                    sha_bytes(content))
            image_records.append({"file": target_png.relative_to(output).as_posix(), "metadata": "images/" + variant + "-" + phase + ".json", **metadata})
        original_png = directory / "final.png"
        final = selected["final_capture"]
        target_png = copy(original_png, "images/" + variant + "-final-logical.png",
                          "Unmodified logical Slint application PNG", tag + "/final.png", final["file_sha256"])
        with Image.open(target_png) as image:
            pixel_sha = sha_bytes(image.convert("RGB").tobytes())
            require(image.size == (1860, 2480) and pixel_sha == final["pixel_sha256"], "The selected final logical pixels changed")
        metadata = {"source_tag": tag, "evidence_kind": "Original logical Slint PNG copied byte for byte",
                    "optical_capture": False, "physical_framebuffer_capture": False,
                    "semantics": final["semantics"], "source_png_sha256": final["file_sha256"],
                    "png_sha256": sha256(target_png), "rgb_pixels_sha256": pixel_sha,
                    "width": 1860, "height": 2480, "pixel_processing": "None; original PNG bytes retained",
                    "source_commit": sources[variant]["source_commit"], "binary_sha256": sources[variant]["binary_sha256"]}
        project("images/" + variant + "-final-logical.json", metadata, "Logical snapshot sidecar", tag + "/final.png", final["file_sha256"])
        image_records.append({"file": target_png.relative_to(output).as_posix(), "metadata": "images/" + variant + "-final-logical.json", **metadata})
    summary = {
        "schema_version": 1, "status": "passed recorded physical startup A/B batch",
        "scope": "One selected Scribe, offline bundled synthetic demo, warm process starts in one boot; not cold boot or cache eviction",
        "export_scope": "Saved evidence processing only; no device, lease, application, or tests were accessed or run by this exporter",
        "source_evidence_sha256": {"report": EXPECTED["report"], "analysis": EXPECTED["analysis"], "stage_projection": EXPECTED["stage_projection"]},
        "variants": sources, "profile": preparation["profile"], "observed_firmware": preparation["firmware"],
        "fixture": {"scope": "isolated bundled synthetic offline demo; no live account", "files": 37,
                    "fingerprint": EXPECTED["fixture"], "fingerprint_definition": "SHA-256 of sorted-key compact JSON file list plus LF; differs from the pretty manifest file hash",
                    "manifest_file_sha256": sha256(fixture_path), "database_and_asset_payloads_exported": False},
        "tools": {"frozen_manifest_sha256": EXPECTED["frozen_tools"], "host_tests": frozen["host_tests"],
                  "host_test_log_sha256": {name: frozen["files"][name]["sha256"] for name in ("batch-tests-final-r2.log", "analysis-tests-settled.log")},
                  "protocol_sha256": frozen["files"]["MEASUREMENT_PLAN.md"]["sha256"],
                  "analysis_tool_sha256": frozen["files"]["analyze_startup_ab.py"]["sha256"],
                  "prepared_tool_sha256": preparation["host_tools"],
                  "framebuffer_reader_binary_sha256": preparation["physical_framebuffer_sha256"],
                  "startup_clock_binary_sha256": preparation["startup_clock_sha256"], "supervisor_sha256": preparation["supervisor_sha256"]},
        "compiler_profile_sha256": EXPECTED["compiler"], "compiler_profile": compiler,
        "clock": analysis["clock"], "clock_limit": "Request marker write/close/exec overhead is included; endpoints are not physical-tap or panel-completion latency",
        "sampling": {"total_cases": 12, "warmups_excluded_from_statistics": 2, "measured_pairs": 5,
                     "order": [pair["order"] for pair in pairs], "all_samples_retained": True,
                     "candidate_04_retained": True, "performance_and_demo_fingerprinting_enabled": True},
        "summary": summaries, "paired_rounds": pairs, "paired_deltas": paired_deltas,
        "paired_delta_units": "Milliseconds except rss_kib/hwm_kib; candidate-minus-baseline negative means faster/lower",
        "interval_definition": "ready_to_content_send_ms is computed separately per case from that case's monotonic timestamps, not from differences of independent medians",
        "cases": public_cases,
        "framebuffer_comparisons": analysis["framebuffer_comparisons"],
        "final_logical_business_pixels_match": analysis["final_logical_business_pixels_match"],
        "settled_observations": settled_observations, "images": image_records,
        "restoration": {"success": report["success"], "all_12_case_recovery_checks_passed": all(case["recovery"]["passed"] for case in public_cases),
                        "native_restored": report["native_restored"], "lighting_restored": report["lighting_restored"],
                        "selected_install_files_unchanged": report["install_unchanged"], "selected_preservation_file_count": report["preservation_path_count"],
                        "preservation_scope": report["preservation_scope"], "whole_tree_census": False,
                        "same_boot": report["same_boot"], "input_roster_unchanged": report["input_roster_unchanged"],
                        "selected_input_devices_match": analysis["selected_input_devices_match"],
                        "temporary_stage_removed": report["temporary_stage_removed"],
                        "host_commands": {"count": 42, "all_returned_zero": True, "timeouts": 0, "interruptions": 0,
                                          "recovery_fallback_commands_recorded": fallback_commands,
                                          "fallback_scope": "Recorded emergency recovery commands only; no claim that every input-hint fallback was absent"},
                        "transport": {"receipt_sha256": sha256(closure_path), "connections_closed": closure["connections_closed"],
                                      "active_owned_transport_count": len(closure["active_owned_transports"]),
                                      "device_polled_for_this_closure_receipt": closure["device_polled"],
                                      "lease_accessed_for_this_closure_receipt": closure["lease_accessed"]},
                        "lease_release": {"receipt_sha256": sha256(release_path), **{key: release[key] for key in release_keys},
                                          "owner_run_and_token_identity_values_exported": False},
                        "primary_agent_private_native_home_pixels_review": "Reported complete before export; no native image or contents exported",
                        "fresh_user_native_touch_confirmation": {"status": "pending at export", "old_confirmation_reused": False}},
        "limitations": analysis["limitations"] + [
            "VmRSS/VmHWM are readings near first content; VmHWM is the kernel high-water value at that sample, not continuous complete-run peak monitoring.",
            "The last-pair settled observation requests were late; equality does not prove a stable frame at exactly ready plus one second.",
            "Profiling and synthetic-demo RGB fingerprints add work. No identical speedup with profiling disabled is established.",
            "The ready-to-SEND interval and first-business render spans grew; no isolated cause or dictionary attribution is established.",
            "Lifecycle records contain no sleep/power events; this is not new sleep/wake acceptance or an end-to-end BOOTTIME proof.",
            "Native return is controller- and saved-pixel-supported; a fresh user touch confirmation remains pending at export.",
        ],
    }
    require(summary["restoration"]["selected_preservation_file_count"] == 4, "Preservation scope changed")
    project("summary.json", summary, "Public anonymous numeric and recovery projection", "physical-startup-analysis", EXPECTED["analysis"])
    ready_a, ready_b = (summaries[v]["software_ready_ms"]["median"] / 1000 for v in VARIANTS)
    send_a, send_b = (summaries[v]["content_send_ms"]["median"] / 1000 for v in VARIANTS)
    interval_a, interval_b = (summaries[v]["ready_to_content_send_ms"]["median"] for v in VARIANTS)
    readme = f'''# Physical startup evidence: 2026-11-17

This is a whitelist export of an already completed Scribe startup experiment.
No device, lease, application, or tests were accessed or run during export.
Only the bundled offline synthetic demo appears in the six public application
images. Native Home/library pixels, native before/final captures, private logs,
ownership files and values, output archives, credentials, and databases remain
private.

The software source tags are baseline `06ad90b` and candidate `585e571`.
[summary.json](summary.json) retains their full source, inventory, package,
launcher, executable, worker, compiler, fixture, and tool identities. The
37-file fixture fingerprint is `{EXPECTED['fixture']}`. Both builds used the
same release profile and compiler: thin LTO, one codegen unit, six Cargo jobs,
incremental compilation disabled, and the same ARM target/features. The frozen
host tooling records 49 successful fixture tests separately from device results.

## Timing scope

One warmup per variant is excluded. All five subsequent pairs are retained in
AB/BA/AB/BA/AB order, including the slower candidate-04 launch. These are warm
process launches in one boot with profiling and demo RGB fingerprints enabled,
without cache eviction. The clock begins immediately before the instrumented
appmgr/Scriptlet request. It includes marker and exec overhead and is not a
physical tap measurement.

| Endpoint median | Baseline | Candidate |
| --- | ---: | ---: |
| Software ready | {ready_a:.3f} s | {ready_b:.3f} s |
| First business SEND accepted | {send_a:.3f} s | {send_b:.3f} s |
| Actual per-case ready to SEND interval | {interval_a:.3f} ms | {interval_b:.3f} ms |

The last row increased. It is the median of separately calculated intervals,
not a difference between the two independent endpoint medians. candidate-04
remains at 6.747 s ready and 7.203 s business SEND. Every value, range, pair,
and signed paired delta is retained in the JSON. There is no universal speedup,
percentile, cold-boot, or profiling-disabled claim.

[stage-projection.json](stage-projection.json) is the original independently
audited anonymous stage projection, including the longer business render spans,
repeated frames, diagnostic CPU work, and unresolved gaps. Spans can overlap and
must not be summed into end-to-end latency. The extra pre-ready repeated frame
is present in all five baseline samples and one candidate sample; it is not a
candidate-only mechanism. No dictionary cause is established. The detailed
launcher phase file was not collected, so only the request-to-Rust coarse
boundary and source-defined waits are available.

## Recovery and memory

All 12 cases retain same-process final/lifecycle/performance identity checks,
matching selected input nodes, no recorded sleep/power transitions, launcher
status zero, native service restoration, owned-process exit, and absent
production locks. No emergency recovery command was recorded. This does not
claim that every possible input-discovery fallback was absent.

The controller restored lighting, kept the same boot/input roster, removed its
temporary stage, closed its owned transports, and released only the exact owned
lease. The public release projection retains the explicit owner/token-check
and directory-removal flags without their private identity values. Installation
preservation covers exactly four selected regular-file identities/absences;
it is not a whole-tree census. Native Home framebuffer pixels were privately
reviewed by the primary agent. **Fresh user confirmation of native touch is
pending at export**; a later confirmation must be attached separately.

VmRSS and VmHWM are one sample near first business content. VmHWM is the kernel
high-water value at that reading, not continuously monitored full-run peak
memory. There is no new touch, rotation, sleep/wake, model, or firmware acceptance.

## Six application images

| Software | Early memory projection | Later memory projection | Original logical snapshot |
| --- | --- | --- | --- |
| Baseline-05 | [early](images/baseline-early.png) | [settled](images/baseline-settled.png) | [logical](images/baseline-final-logical.png) |
| Candidate-05 | [early](images/candidate-early.png) | [settled](images/candidate-settled.png) | [logical](images/candidate-final-logical.png) |

The four Gray8 PNGs are lossless projections of all 1860 x 2480 packed visible
memory bytes. The device row stride was 1872; the saved reader payload excludes
padding. No crop, resize, rotation, inversion, masking, or tone correction was
applied. Each sidecar retains the original raw/header/pixel/PNG identities; raw
files stay private. The logical PNGs are original bytes from Slint snapshots
before framebuffer compensation.

The later observation targeted ready + 1,000 ms, but its actual request was
late by 2,782.799 ms for baseline-05 and 2,892.385 ms for candidate-05. These
images therefore do not establish settlement at exactly one second. The
requests occurred near ready + 3.783 s and + 3.892 s, respectively, and their
timestamps are independent of primary SEND endpoints. All early/later visible
memory pixels match in this last pair, as do the final logical pixels. This is
memory evidence, not an optical camera capture or panel-completion proof.

SEND acceptance, driver completion, and panel observation remain separate
fields. The first business updates have accepted/completed matching markers
and recorded driver completion; panel completion was not observed.

[manifest.json](manifest.json) binds the exported bytes. Original private report
and analysis hashes are retained without copying those unfiltered files. Only
the final BiliComics protocol/analysis sources are retained under `tools/`;
reference Aidoku scripts, private configuration, and raw test logs are excluded.
'''
    readme_path = output / "README.md"
    readme_path.write_text(readme, encoding="utf-8", newline="\n")
    add_file(readme_path, "Scoped evidence narrative", "public-exporter", None, False)
    require(len(list((output / "images").glob("*.png"))) == 6, "The public image allowlist is not exactly six PNGs")
    manifest = {"schema_version": 1, "scope": "Saved synthetic application evidence only; no device/lease/application/test execution during export",
                "source_sha256": EXPECTED, "images": 6,
                "excluded": ["Native before/final images and content", "Raw output archives", "Private owner/token/configuration records",
                             "Private logs and databases", "Raw framebuffer payloads", "Reference Aidoku scripts", "Actual account identities"],
                "fresh_user_touch_confirmation": "pending at export; append a separate bound response later",
                "files": sorted(exported, key=lambda row: row["path"])}
    write_json(output / "manifest.json", manifest)
    for path in (output / "images").glob("*.png"):
        path.chmod(0o444)
    print(json.dumps({"summary_sha256": sha256(output / "summary.json"), "manifest_sha256": sha256(output / "manifest.json"),
                      "files": len(exported) + 1, "pngs": 6, "device_or_lease_accessed": False, "application_or_tests_run": False}))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--batch-root", type=Path, required=True)
    parser.add_argument("--software-root", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    main(parser.parse_args())
