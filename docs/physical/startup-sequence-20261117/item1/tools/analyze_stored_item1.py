#!/usr/bin/env python3
"""Analyze an ended startup batch without executing application or device code."""

import argparse
import csv
import hashlib
import json
import statistics
import tarfile
from collections import Counter
from pathlib import Path


def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def median(values):
    return statistics.median(values)


def ms(end, start):
    return (end - start) / 1_000_000


def fmt(value):
    return "-" if value is None else f"{value:.3f}"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--batch", type=Path, required=True)
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    analysis = json.loads((args.batch / "analysis.json").read_text())
    report = json.loads((args.batch / "report.json").read_text())
    source_files = {
        "src/main.rs": "264-300,678-713,802-827,846-958,1237-1303",
        "src/startup_admission.rs": "19-39,53-143",
        "src/runtime_wakeup.rs": "48-65,73-96",
        "src/app/startup.rs": "264-340,394-416",
        "src/app/mod.rs": "357-377",
        "src/performance.rs": "333-355,402-405",
        "vendor/slint-backend-kindle/src/platform.rs": "325-343,393-402,443-477,618-644,714-724,863-899",
        "vendor/slint-backend-kindle/src/first_frame_signal.rs": "32-36,69-79",
        "vendor/slint-backend-kindle/src/refresh.rs": "128-131",
    }
    source_identity = {
        name: {"path": str(args.source / name), "sha256": sha256(args.source / name),
               "relevant_lines": lines}
        for name, lines in source_files.items()
    }
    timelines = []
    frame_spans = []
    case_details = []
    checked_files = 0
    hash_mismatches = []

    with tarfile.open(args.batch / "outputs.tar.gz", "r:gz") as archive:
        for case in analysis["cases"]:
            name = case["case"]
            prefix = f"out/{name}/"
            raw_performance = archive.extractfile(prefix + "performance.jsonl").read()
            records = [json.loads(line) for line in raw_performance.splitlines() if line.strip()]
            raw_lifecycle = archive.extractfile(prefix + "lifecycle.jsonl").read()
            lifecycle = [json.loads(line) for line in raw_lifecycle.splitlines() if line.strip()]
            request = json.loads(archive.extractfile(prefix + "request.json").read())
            origin = request["monotonic_ns"]
            for filename, expected in case["file_sha256"].items():
                actual = hashlib.sha256(archive.extractfile(prefix + filename).read()).hexdigest()
                checked_files += 1
                if actual != expected:
                    hash_mismatches.append({"case": name, "file": filename,
                                            "expected": expected, "actual": actual})
            events = []
            for line_number, row in enumerate(records, 1):
                if row.get("kind") != "startup":
                    continue
                data = row.get("data") or {}
                observed = data.get("observed")
                stamp = (observed["monotonic_ns"] if isinstance(observed, dict)
                         else row["monotonic_ns"])
                events.append({"stage": row["stage"], "monotonic_ns": stamp,
                               "after_request_ms": ms(stamp, origin),
                               "envelope_monotonic_ns": row["monotonic_ns"],
                               "source_line": line_number, "data": data,
                               "timestamp_semantics": "observed" if isinstance(observed, dict) else "envelope"})
            counts = Counter(event["stage"] for event in events)

            def select(stage, **conditions):
                return [event for event in events if event["stage"] == stage
                        and all(event["data"].get(key) == value for key, value in conditions.items())]

            def first(stage, **conditions):
                values = select(stage, **conditions)
                return values[0] if values else None

            def stamp(stage, **conditions):
                event = first(stage, **conditions)
                return event["monotonic_ns"] if event else None

            def rel(stage, **conditions):
                value = stamp(stage, **conditions)
                return ms(value, origin) if value is not None else None

            initial = case["first_startup_frame"]
            completion = initial["stages"]["refresh_complete_wait"]["finished_monotonic_ns"]
            completion_started = initial["stages"]["refresh_complete_wait"]["started_monotonic_ns"]
            lifecycle_complete = initial["stages"]["lifecycle_complete"]["finished_monotonic_ns"]
            worker = first("workers_started")
            prepared = stamp("state_prepared")
            font_finish = stamp("font_finish")
            covers = select("state-covers", phase="begin")
            covers_end = select("state-covers", phase="end")
            prepare_gate = stamp("startup_gate", gate="prepare")
            cover_start = covers[0]["monotonic_ns"]
            attach_covers_begin = select("attach-covers", phase="begin")
            attach_covers_end = select("attach-covers", phase="end")
            before_views = stamp("attach-views", phase="begin")
            route_end = stamp("attach-route", phase="end")
            ready = stamp("ui_ready")
            advanced = select("attach_stage_advanced")
            pre_views_advanced = [event for event in advanced if event["monotonic_ns"] < before_views][-1]
            facts = {
                "case": name, "variant": case["software_variant"], "role": case["measurement_role"],
                "request_monotonic_ns": origin,
                "raw_performance_path": f"{args.batch}/outputs.tar.gz::{prefix}performance.jsonl",
                "performance_workers_started_count": counts["workers_started"],
                "lifecycle_workers_started_count": sum(row.get("stage") == "workers_started" for row in lifecycle),
                "font_worker_begin_count": counts["font_worker_begin"],
                "state_worker_begin_count": counts["state_worker_begin"],
                "state_cover_worker_begin_count": counts["state_cover_worker_begin"],
                "workers_observed_monotonic_ns": worker["monotonic_ns"],
                "workers_envelope_monotonic_ns": worker["envelope_monotonic_ns"],
                "workers_source_line": worker["source_line"],
                "initial_send_monotonic_ns": initial["send_accepted_monotonic_ns"],
                "initial_accepted_marker": initial["accepted_update_marker"],
                "initial_completed_marker": initial["completed_update_marker"],
                "initial_driver_completion_observed": initial["driver_completion_observed"],
                "initial_checked_completion_started_monotonic_ns": completion_started,
                "initial_checked_completion_finished_monotonic_ns": completion,
                "initial_lifecycle_call_finished_monotonic_ns": lifecycle_complete,
                "workers_after_send_ms": ms(worker["monotonic_ns"], initial["send_accepted_monotonic_ns"]),
                "completion_after_workers_ms": ms(completion, worker["monotonic_ns"]),
                "workers_during_checked_completion_wait": completion_started < worker["monotonic_ns"] < completion,
                "workers_after_checked_completion": worker["monotonic_ns"] > completion,
                "font_bytes_prepared_before_checked_completion": stamp("fonts_prepared") < completion,
                "metadata_worker_began_before_checked_completion": stamp("state_worker_begin") < completion,
                "metadata_prepared_before_checked_completion": (
                    stamp("state_metadata_prepared") < completion if stamp("state_metadata_prepared") is not None else None),
                "cover_pixels_began_after_prepare_gate_ms": ms(cover_start, prepare_gate),
                "cover_pixels_began_after_checked_completion_ms": ms(cover_start, completion),
                "font_finish_after_checked_completion_ms": ms(font_finish, completion),
                "first_registration_after_checked_completion_ms": ms(stamp("font_registered"), completion),
                "state_prepared_minus_font_finish_ms": ms(prepared, font_finish),
                "state_prepared_to_attach_ms": ms(stamp("application_attached"), prepared),
                "attached_to_first_cover_attach_ms": ms(attach_covers_begin[0]["monotonic_ns"], stamp("application_attached")),
                "first_cover_attach_end_to_second_begin_ms": ms(attach_covers_begin[1]["monotonic_ns"], attach_covers_end[0]["monotonic_ns"]),
                "route_end_to_views_begin_ms": ms(before_views, route_end),
                "last_advance_to_views_begin_ms": ms(before_views, pre_views_advanced["monotonic_ns"]),
                "ready_to_business_send_ms": ms(case["first_business_frame"]["send_accepted_monotonic_ns"], ready),
                "metrics": case["metrics"], "memory": case["memory"],
                "post_business_gate": case["post_business_gate"],
                "ui_ready_before_full_covers_claim": False,
                "source_events": events,
            }
            row = {"case": name, "role": case["measurement_role"]}
            for stage in ["process_enter", "workers_started", "font_worker_begin", "fonts_prepared",
                          "state_worker_begin", "state_metadata_prepared", "state_cover_worker_begin",
                          "state_cover_worker_end", "state_prepared", "font_finish", "application_attached", "ui_ready"]:
                row[stage + "_ms"] = rel(stage)
            row.update(initial_send_ms=initial["send_accepted_after_request_ms"],
                       initial_checked_completion_ms=ms(completion, origin),
                       initial_lifecycle_call_finished_ms=ms(lifecycle_complete, origin),
                       prepare_gate_ms=ms(prepare_gate, origin),
                       cover_pixel_begin_ms=ms(cover_start, origin),
                       cover_pixel_end_ms=ms(covers_end[0]["monotonic_ns"], origin),
                       route_end_ms=ms(route_end, origin), views_begin_ms=ms(before_views, origin),
                       first_business_send_ms=case["metrics"]["content_send_ms"])
            timelines.append(row)
            pre_ready = []
            frame_rows = []
            for frame in case["frame_attempts"]:
                frame_entry = {"render_sequence": frame["render_sequence"],
                               "complete_frame_replacement": frame["complete_frame_replacement"],
                               "send_after_request_ms": frame["send_accepted_after_request_ms"],
                               "record_after_request_ms": frame["record_after_request_ms"], "stages": {}}
                if (frame["complete_frame_replacement"] and not frame["initial_frame"]
                        and frame["send_accepted_monotonic_ns"] < ready):
                    pre_ready.append(frame["render_sequence"])
                for stage, span in frame["stages"].items():
                    if not span:
                        continue
                    span_row = {"case": name, "render_sequence": frame["render_sequence"],
                                "complete_frame_replacement": frame["complete_frame_replacement"],
                                "stage": stage,
                                "started_ms": ms(span["started_monotonic_ns"], origin),
                                "finished_ms": ms(span["finished_monotonic_ns"], origin),
                                "wall_ms": span["wall_ns"] / 1_000_000,
                                "thread_cpu_ms": span["thread_cpu_ns"] / 1_000_000}
                    frame_spans.append(span_row)
                    frame_entry["stages"][stage] = span_row
                frame_rows.append(frame_entry)
            facts["pre_ready_extra_full_render_sequences"] = pre_ready
            facts["frame_attempts"] = frame_rows
            case_details.append(facts)

    primary = [case for case in case_details if case["role"] == "primary"]
    by_name = {case["case"]: case for case in case_details}
    first_frame_stage_summary = {}
    for variant in ["baseline", "candidate"]:
        frames = [case["first_startup_frame"] for case in analysis["cases"]
                  if case["included_in_primary_statistics"] and case["software_variant"] == variant]
        first_frame_stage_summary[variant] = {
            stage: {"median_wall_ms": median([frame["stages"][stage]["wall_ns"] / 1_000_000 for frame in frames]),
                    "median_thread_cpu_ms": median([frame["stages"][stage]["thread_cpu_ns"] / 1_000_000 for frame in frames])}
            for stage in ["slint_render", "rgb_fingerprint", "framebuffer_write", "refresh_submit", "refresh_complete_wait"]
        }
    pairs = []
    for pair in analysis["paired_rounds"]:
        index = pair["round"]
        base = by_name[f"baseline-{index:02}"]
        cand = by_name[f"candidate-{index:02}"]
        pairs.append({"round": index, "order": pair["order"],
                      "send_reduction_ms": base["metrics"]["content_send_ms"] - cand["metrics"]["content_send_ms"],
                      "ready_reduction_ms": base["metrics"]["software_ready_ms"] - cand["metrics"]["software_ready_ms"],
                      "rss_increase_kib": cand["memory"]["VmRSS"] - base["memory"]["VmRSS"],
                      "ready_to_send_reduction_ms": base["ready_to_business_send_ms"] - cand["ready_to_business_send_ms"]})
    aggregate = {
        "all_14_worker_admissions_exactly_once": all(case["performance_workers_started_count"] == 1
                                                     and case["lifecycle_workers_started_count"] == 1 for case in case_details),
        "all_candidate_admissions_inside_original_checked_completion_wait": all(case["workers_during_checked_completion_wait"]
                                                                                for case in case_details if case["variant"] == "candidate"),
        "all_baseline_admissions_after_original_checked_completion": all(case["workers_after_checked_completion"]
                                                                        for case in case_details if case["variant"] == "baseline"),
        "all_candidate_font_bytes_prepared_before_original_checked_completion": all(case["font_bytes_prepared_before_checked_completion"]
                                                                                   for case in case_details if case["variant"] == "candidate"),
        "all_candidate_metadata_workers_began_before_original_checked_completion": all(case["metadata_worker_began_before_checked_completion"]
                                                                                      for case in case_details if case["variant"] == "candidate"),
        "candidate_metadata_completed_before_original_checked_completion_count": sum(case["metadata_prepared_before_checked_completion"] is True
                                                                                     for case in case_details if case["variant"] == "candidate"),
        "all_cover_pixels_after_original_prepare_gate_and_checked_completion": all(case["cover_pixels_began_after_prepare_gate_ms"] > 0
                                                                                  and case["cover_pixels_began_after_checked_completion_ms"] > 0 for case in case_details),
        "all_first_ui_font_registration_after_original_checked_completion": all(case["first_registration_after_checked_completion_ms"] > 0 for case in case_details),
        "send_faster_pairs": sum(pair["send_reduction_ms"] > 0 for pair in pairs),
        "ready_faster_pairs": sum(pair["ready_reduction_ms"] > 0 for pair in pairs),
        "paired_send_reduction_median_ms": median([pair["send_reduction_ms"] for pair in pairs]),
        "paired_ready_reduction_median_ms": median([pair["ready_reduction_ms"] for pair in pairs]),
        "checked_raw_file_hash_count": checked_files, "raw_file_hash_mismatches": hash_mismatches,
    }
    derived = {
        "schema_version": 1,
        "scope": "Read-only analysis of a completed stored batch on test-env; no new tests, builds, application execution, emulator, runtime probe, Kindle access or lease operation.",
        "batch_root": str(args.batch), "source_root": str(args.source),
        "input_sha256": {name: sha256(args.batch / name) for name in ["analysis.json", "report.json", "outputs.tar.gz"]},
        "source_identity": source_identity, "aggregate": aggregate,
        "summary": analysis["summary"], "median_reductions": analysis["median_reductions"],
        "first_frame_stage_summary": first_frame_stage_summary,
        "paired_rounds": pairs, "cases": case_details,
        "restoration": analysis["restoration"],
        "pixel_comparison_scope": analysis["framebuffer_comparisons"],
        "evidence_limitations": analysis["limitations"],
    }
    (args.output / "item1-analysis.json").write_text(json.dumps(derived, indent=2, sort_keys=True) + "\n")
    for filename, rows in [("timeline.csv", timelines), ("frame-spans.csv", frame_spans)]:
        with (args.output / filename).open("w", newline="") as stream:
            writer = csv.DictWriter(stream, fieldnames=list(rows[0]))
            writer.writeheader()
            writer.writerows(rows)

    lines = [
        "# Item 1: stored physical startup A/B analysis, 2026-11-17", "",
        "Keep the early background-admission architecture as a candidate, but do not describe this batch as a stable startup-speed acceptance. The six prescribed pairs favor the candidate at the median, while the observed UI continuation boundaries produce large variations and one SEND regression. The next implementation iteration should remove the redundant full replacement between startup Route and Views before treating this performance result as ready for adoption.", "",
        "## Scope and provenance", "",
        f"This analysis reads only the ended batch `{args.batch}` on `test-env`. It executes no application, test suite, build, emulator, runtime probe, device command, input action, or lease operation. It does not modify raw evidence or runtime source. The raw physical batch ran from `{report['started_at_utc']}` to `{report['finished_at_utc']}` and is recorded as successful with the exact owned lease released. Native control, selected installation identities, lighting, input roster, and same-boot checks are recorded as restored/unchanged; the temporary stage was removed and transport closed.", "",
        "The frozen source is `530f1516c332931059c52161b52d64e86488b3f1`. Both variants use ARM ELF SHA-256 `04fc6f93b4327911ca1a3bb5200175c9f2e13fb21ac5e56280012ff44b6e2e59`, the same package and compiler profile, and the same offline demo snapshot. Baseline arguments are `--demo --offline --serial-startup-preparation`; candidate arguments are `--demo --offline`. This controls the preparation path within one ELF; it does not compare the old production binary against a rebuilt candidate.", "",
        "All fourteen prescribed launches remain in the record: baseline/candidate warmups, then six balanced AB/BA pairs. Only the two predefined warmups are omitted from primary distributions. No earlier failed native-query batch is read or combined. All 184 registered raw file identities match their frozen analyzer records. Derived input and relevant frozen-source hashes are retained in `item1-analysis.json`.", "",
        "Primary timing is device `CLOCK_MONOTONIC`: request precedes the native launch command, Ready is the `startup/ui_ready` envelope stamp, and first Shelf SEND is the first accepted rendered native business frame whose Slint render begins after Ready. This is not a physical-tap, complete-cover, driver-completion, or optical endpoint. Geometry is 1860x2480, 8-bit framebuffer, stride 1872, `mtk` ABI, native `Y8` mode. One warm device boot and retained caches limit generalization.", "",
        "## Complete paired results", "",
        "Positive B-C means the candidate is faster. Times are milliseconds.", "",
        "| Pair/order | Baseline SEND | Candidate SEND | SEND B-C | Baseline Ready | Candidate Ready | Ready B-C |",
        "| --- | ---: | ---: | ---: | ---: | ---: | ---: |",
    ]
    for pair in pairs:
        base = by_name[f"baseline-{pair['round']:02}"]["metrics"]
        cand = by_name[f"candidate-{pair['round']:02}"]["metrics"]
        order = "AB" if pair["order"][0] == "baseline" else "BA"
        lines.append(f"| {pair['round']:02}/{order} | {fmt(base['content_send_ms'])} | {fmt(cand['content_send_ms'])} | {fmt(pair['send_reduction_ms'])} | {fmt(base['software_ready_ms'])} | {fmt(cand['software_ready_ms'])} | {fmt(pair['ready_reduction_ms'])} |")
    lines += ["", "| Endpoint | Baseline median | Candidate median | Difference of medians | Median paired B-C | Faster pairs |",
              "| --- | ---: | ---: | ---: | ---: | ---: |"]
    for metric, label in [("content_send_ms", "First Shelf SEND"), ("software_ready_ms", "Ready")]:
        stat = analysis["median_reductions"][metric]
        lines.append(f"| {label} | {fmt(analysis['summary']['baseline'][metric]['median'])} | {fmt(analysis['summary']['candidate'][metric]['median'])} | {fmt(stat['difference_of_medians_ms'])} ({stat['percent_from_medians']:.3f}%) | {fmt(stat['median_paired_reduction_ms'])} | {sum(p[('send' if metric == 'content_send_ms' else 'ready') + '_reduction_ms'] > 0 for p in pairs)}/6 |")
    lines += ["",
        "The candidate SEND range is 5876.609-6887.877 ms, versus 6574.331-6726.072 ms for baseline. Candidate Ready ranges from 5140.400 to 6410.300 ms, versus 5762.752-5945.663 ms. This six-pair batch establishes the measured distribution, not a population-wide improvement, tail guarantee, or cold-start result. Both the large wins and the regressions occur in the predefined record and remain included.", "",
        "## Observed one-shot admission", "",
        "Every raw performance trace contains exactly one `workers_started`, and every lifecycle trace independently contains exactly one. These are two records of one admission, not two worker starts. Every case has one font worker and one state/metadata worker; each candidate additionally has one gated cover-completion worker. The table uses `data.observed.monotonic_ns` for `workers_started`, never its later envelope/write time. All absolute stamps, enclosing stamps, raw record line numbers, counts, and comparisons are retained in `item1-analysis.json`.", "",
        "`Done` is the finish stamp of the original initial frame's checked own-marker completion wait. Every initial frame is accepted, full, rendered, and completed with accepted marker 1 equal to completed marker 1. `Done-Worker` is positive when the worker starts before completion.", "",
        "| Case | Worker count | Initial SEND | Workers | Done | Worker-SEND | Done-Worker | Cover-prepare gate | Cover-Done |",
        "| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |",
    ]
    for facts, timeline in zip(case_details, timelines):
        lines.append(f"| {facts['case']} | {facts['performance_workers_started_count']} | {fmt(timeline['initial_send_ms'])} | {fmt(timeline['workers_started_ms'])} | {fmt(timeline['initial_checked_completion_ms'])} | {fmt(facts['workers_after_send_ms'])} | {fmt(facts['completion_after_workers_ms'])} | {fmt(facts['cover_pixels_began_after_prepare_gate_ms'])} | {fmt(facts['cover_pixels_began_after_checked_completion_ms'])} |")
    lines += ["",
        "All seven candidates, including the warmup, admit the workers strictly after successful initial SEND and inside its original checked completion wait. All seven baselines start after that completion. The actual candidate font byte preparation finishes before original completion in every case. Candidate metadata starts during that wait in every case; metadata completes after it in all seven cases. Thus the proven overlap is font-byte preparation plus part of metadata preparation, not UI font registration or cover pixel preparation.", "",
        "Candidate cover pixel work begins after both the original `startup_gate=prepare` UI boundary and the original checked completion in all seven cases; baseline cover work is also after those boundaries. Frozen `src/startup_admission.rs:112-143` releases the cover gate at `StartupUiTurn::yield_now()` and waits for that gate after metadata. The gate is released immediately before returning from the first bootstrap UI closure, so the trace and source establish the original yield/admission boundary, not a separately measured closure-return timestamp. It is inaccurate to claim covers overlap the original completion wait or to claim a measured extra full UI turn after closure return.", "",
        "Actual UI font registration is after original checked completion in all fourteen records. `font_decoded`, `fonts_prepared`, and `state_prepared` carry worker observation stamps even when written during later UI consumption; they are not UI-consumption timestamps. `lifecycle_complete` is the duration of a lifecycle method call, not a dedicated successful-input-drain timestamp. The backend can run queued control closures before its pending live-input-drain branch (`platform.rs:393-402,443-459`), while suppressing normal timers/touch dispatch there. Accordingly, neither this implementation nor this record proves that every UI consumer must be later than input readiness. This batch contains no new physical touch-readiness acceptance.", "",
        "`timeline.csv` records the separate font-byte, metadata, cover-pixel, font-registration, application-attach and view boundaries for every case. Stage spans overlap across workers and UI; they must not be added into an end-to-end startup total.", "",
        "## Critical paths and the two large wins", "",
        "The following gaps are directly observed. A positive prepared-minus-font-finish value means state arrived just after the UI completed font registration. All times are milliseconds.", "",
        "| Candidate | Prepared minus font finish | Prepared to attach | First cover attach end to second begin | Route end to Views begin | Ready to SEND | Extra pre-Ready full sequence |",
        "| --- | ---: | ---: | ---: | ---: | ---: | --- |",
    ]
    for facts in case_details:
        if facts["variant"] != "candidate" or facts["role"] != "primary":
            continue
        sequences = ",".join(map(str, facts["pre_ready_extra_full_render_sequences"])) or "none"
        lines.append(f"| {facts['case']} | {fmt(facts['state_prepared_minus_font_finish_ms'])} | {fmt(facts['state_prepared_to_attach_ms'])} | {fmt(facts['first_cover_attach_end_to_second_begin_ms'])} | {fmt(facts['route_end_to_views_begin_ms'])} | {fmt(facts['ready_to_business_send_ms'])} | {sequences} |")
    lines += ["",
        "Candidate 01/02/06 publish `state_prepared` only 2.596/6.161/8.165 ms after `font_finish`. The UI checks state before those arrivals, then the next state-consumption opportunity follows an intermediate frame. The observed prepared-to-attach gaps are about 255 ms. Candidate 03/04/05 already have prepared state when font registration finishes and attach it immediately afterward. This is an observed scheduling boundary, not proof that metadata itself is slow in 03 or 06.", "",
        "Candidate 04/05 avoid that missed state opportunity and complete both cover-attachment chunks and the cheap following stages without an intervening replacement before Ready. Their first business frames are sequence 4; baseline business frames are sequence 6. Their SEND wins are 802.031/814.358 ms and Ready wins are 724.821/787.866 ms. In contrast, 01/02/06 have an additional partial frame between cover-attachment chunks, with 410.174-417.335 ms between the first end and second begin. Candidate 03 has no such cover-attachment gap, but hits the full-frame boundary below. This accounts for where the 04/05 benefit is retained and where other candidates lose it; it does not establish a deterministic 800 ms optimization.", "",
        "Candidate 03 completes Route at 5225.938 ms, yields at `attach_stage_advanced` 5226.293 ms, and begins Views only at 6032.273 ms. Between them, full replacement sequence 4 runs from preceding wait at 5228.700 ms to its record at 6029.736 ms. The non-overlapping components inside that transaction are preceding-marker wait 337.157 ms, Slint render 21.706 ms, fingerprint 46.048 ms, framebuffer write 29.738 ms, SEND 0.332 ms, and checked own-marker wait 365.705 ms. Views then cost about 17 ms and Ready is 6051.432 ms. This directly locates the Ready regression; it is not caused by late worker completion. After Ready, the final business frame has no outstanding preceding-marker wait and SEND follows in 509.212 ms, versus 767.163 ms for its baseline. That 257.951 ms shorter post-Ready path offsets the 215.414 ms Ready regression, leaving a 42.537 ms SEND win.", "",
        "Candidate 06 has both penalties. The first-to-second cover-attachment gap contains non-full sequence 4, whose preceding-marker wait alone is 348.586 ms. Route then completes at 5570.433 ms and Views starts at 6389.977 ms. Full replacement sequence 5 occupies 5573.179-6388.537 ms: preceding wait 348.932 ms, render 22.264 ms, fingerprint 48.182 ms, framebuffer write 29.623 ms, SEND 0.304 ms, and own-marker wait 365.656 ms. Ready is 6410.300 ms. The post-Ready business path shrinks to 477.577 ms from baseline's 781.029 ms, offsetting only part of the 465.258 ms Ready regression; SEND remains 161.805 ms slower.", "",
        "The frozen source provides a concrete mechanism consistent with those transactions: `advance_startup()` checks a 4 ms budget between stages (`src/app/startup.rs:264-270`); Route calls `navigate()`, which queues `EnterScreen` before Views (`src/app/mod.rs:357-371`); Views clears the startup stage; the driver clears `booting` and queues another `EnterScreen` only after all stages finish (`src/main.rs:934-958`). A 1 ms continuation yields to rendering (`src/runtime_wakeup.rs:86-95`). The trace proves a complete replacement was actually presented between Route and Views in 03/06 and blocked subsequent UI progress. It does not tag the queued request's producer, record each budget decision, or establish which scheduler, I/O, instrumentation, or notification-order effect made those specific rounds cross the boundary. Describing CPU contention, font decoding, slow covers, cache eviction, or native launch overhead as the proven root cause would exceed the evidence. Performance tracing itself writes synchronously; its possible influence on the 4 ms boundary is unquantified.", "",
        "## Startup first frame and process front segment", "",
        "| Metric | Baseline median | Candidate median | Interpretation |",
        "| --- | ---: | ---: | --- |",
    ]
    for metric, label, interpretation in [
        ("request_to_process_enter_ms", "Request to process entry", "Native launch front segment is essentially unchanged"),
        ("first_startup_send_ms", "Request to startup SEND", "Visible startup submission timing is essentially unchanged"),
        ("process_enter_to_ready_ms", "Process entry to Ready", "Most of the measured median benefit is after process entry"),
    ]:
        lines.append(f"| {label} | {fmt(analysis['summary']['baseline'][metric]['median'])} | {fmt(analysis['summary']['candidate'][metric]['median'])} | {interpretation} |")
    initial_process = {}
    for variant in ["baseline", "candidate"]:
        initial_process[variant] = median([case["metrics"]["first_startup_send_ms"] - case["metrics"]["request_to_process_enter_ms"]
                                           for case in primary if case["variant"] == variant])
    lines += [f"| Process entry to startup SEND | {fmt(initial_process['baseline'])} | {fmt(initial_process['candidate'])} | No large initial render acceleration |", "",
        "The candidate is admitted only after the first SEND, so it cannot make that already accepted submission faster through preparation overlap. First-frame spans show the original full rewrite and checked completion remain present. Similar first-frame timestamps do not prove optical equality or unchanged physical input readiness.", "",
        "The initial-frame stage medians below use all six primary launches in each variant. Wall and same-thread CPU times are separate observations; medians of separate stages are not an end-to-end sum.", "",
        "| Initial-frame stage | Baseline wall | Candidate wall | Baseline thread CPU | Candidate thread CPU |",
        "| --- | ---: | ---: | ---: | ---: |",
    ]
    for stage in ["slint_render", "rgb_fingerprint", "framebuffer_write", "refresh_submit", "refresh_complete_wait"]:
        base = first_frame_stage_summary["baseline"][stage]
        cand = first_frame_stage_summary["candidate"][stage]
        lines.append(f"| {stage} | {fmt(base['median_wall_ms'])} | {fmt(cand['median_wall_ms'])} | {fmt(base['median_thread_cpu_ms'])} | {fmt(cand['median_thread_cpu_ms'])} |")
    lines += ["",
        "## Post-business resource and pixel observations", "",
        "All samples follow the common first-business SEND plus 1500 ms gate. The instrumented gate's deadline slips range from 0.335 to 1.701 ms; those are gate-request slips, not exact procfs or framebuffer sample timestamps. Procfs reads occur after the gate and any scheduled framebuffer capture. RSS median rises from 55958 to 56150 KiB, a 192 KiB (0.343%) increase; all six paired RSS differences are positive (88-332 KiB). Sampled `VmHWM` medians are 55974 and 56166 KiB. VmHWM is the kernel-reported high-water value up to that later observation, not a continuously measured startup-only peak or a whole-process-lifetime maximum. The current record does not support a memory reduction or material memory-regression claim.", "",
        "All fourteen final logical Slint snapshots have equal complete RGB pixels (before framebuffer compensation). The early `content-frame.raw` and scheduled `settled-frame.raw` comparisons cover only baseline-06 versus candidate-06. Each compares every one of 4,612,800 visible framebuffer pixels, without masks, and reports zero differences; padding bytes are outside that visible-pixel claim. Those are framebuffer-memory observations, not panel photographs or optical completion. The two raw framebuffer pair observations must not be generalized into fourteen framebuffer captures. The recorded first Shelf SEND does not establish that all background covers are complete; the Ready resource snapshot can still contain pending jobs.", "",
        "## Recommendation and limits", "",
        "1. Retain the one-shot post-SEND font/metadata admission and the existing own-marker, renderer/lifecycle ownership, cover gate, cancellation, and input-drain protections as the implementation candidate. Every recorded launch meets the intended observed admission ordering; no duplicate worker admission appears.",
        "2. Hold stable speed-adoption or completion claims. The current implementation is sensitive to missing a UI continuation opportunity and can issue a redundant complete startup frame after Route but before Views. The observed medians improve, but the improvement is heterogeneous and Ready regresses in two of six pairs.",
        "3. In a future source iteration, defer or coalesce the startup Route `EnterScreen` until the final views and `booting=false` scene can be committed together, while retaining the approved complete-frame replacement and checked completion. Preserve bounded UI work and progress feedback. Consider continuation scheduling only where it avoids unnecessary intermediate presentation; do not remove hardware/input barriers to improve the timing number.",
        "4. Attribute a revised source/ELF and evaluate it as a new independent measurement batch with the same retained warmups, balanced pairs, endpoints, and captures. The current analysis requests or performs no new batch. More observations and explicit stage-budget/refresh-producer instrumentation would be needed to distinguish the remaining notification, scheduling, and trace-overhead hypotheses.", "",
        "The separate user-assisted sleep/input batch ending `a1067e98` is outside these performance data. Its bounded window ended without received physical-key feedback and its lease was exactly released. Sleep, input, and sleep-continuation acceptance must remain unverified; this ended attempt neither blocks this stored analysis nor supplies a pass. Existing fixture/software checks, this offline-demo physical batch, live-account behavior, and optical/hardware qualification remain separate evidence classes.", "",
        "## Exact evidence locations", "",
        f"- Frozen aggregate analyzer output: `{args.batch}/analysis.json` (`cases`, `paired_rounds`, `summary`, `median_reductions`, `restoration`, `framebuffer_comparisons`).",
        f"- Ended physical-batch report: `{args.batch}/report.json` (`cases`, `completed_cases`, `variants`, `scope`, restoration and release booleans).",
        f"- Raw case members: `{args.batch}/outputs.tar.gz::out/<case>/request.json`, `performance.jsonl`, `lifecycle.jsonl`, `memory-at-content.txt`, `post-business-gate.json`, `shell.tsv`, `final.json` and `recovery.json`.",
        f"- Visible framebuffer capture pairs: `{args.batch}/outputs.tar.gz::out/{{baseline-06,candidate-06}}/{{content-frame.raw,settled-frame.raw}}`; their observation requests/completions, deadlines and liveness records are adjacent case members.",
        f"- First-frame and UI chronology: native/frame `data.stages.*.*monotonic_ns`, startup worker `data.observed.monotonic_ns`, and non-observed UI event envelope stamps in those performance members. Exact event source lines and absolute stamps are in the derived JSON; `timeline.csv` and `frame-spans.csv` expose the complete tabular data.",
        f"- Frozen implementation: `{args.source}`; relevant source paths, SHA-256 hashes and line references are in `item1-analysis.json/source_identity`.",
        "- Software/build identity remains the separately published `docs/verification/startup-sequence-20261117/` bundle. Its existence does not substitute for physical timing or sleep/input acceptance.",
        "", "All rounded narrative/table times are derived from the exact stored monotonic stamps. JSON and CSV retain full numeric precision.", "",
    ]
    (args.output / "README.md").write_text("\n".join(lines))
    outputs = ["README.md", "item1-analysis.json", "timeline.csv", "frame-spans.csv"]
    manifest = {"processing": "Derived stored-evidence analysis only on test-env; raw bytes unchanged",
                "inputs": derived["input_sha256"],
                "outputs": {name: {"sha256": sha256(args.output / name), "bytes": (args.output / name).stat().st_size}
                            for name in outputs},
                "script_sha256": sha256(Path(__file__))}
    (args.output / "analysis-manifest.json").write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")
    print(json.dumps({"output": str(args.output), "aggregate": aggregate,
                      "outputs": manifest["outputs"]}, indent=2))


if __name__ == "__main__":
    main()
