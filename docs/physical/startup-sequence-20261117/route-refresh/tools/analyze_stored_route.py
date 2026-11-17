#!/usr/bin/env python3
"""Derive a route-only report from an ended stored batch on test-env."""

import argparse
import csv
import hashlib
import json
import statistics
import tarfile
from collections import Counter
from pathlib import Path


def sha(path):
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def delta(end, start):
    return (end - start) / 1_000_000


def med(values):
    return statistics.median(values)


def fmt(value):
    return "-" if value is None else f"{value:.3f}"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--batch", type=Path, required=True)
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    a = json.loads((args.batch / "analysis.json").read_text())
    r = json.loads((args.batch / "report.json").read_text())
    source_refs = {
        "src/app/startup.rs": "263-295,297-359",
        "src/app/mod.rs": "358-381",
        "src/main.rs": "802-847,900-970",
        "src/startup_admission.rs": "19-39,99-143",
        "src/runtime_wakeup.rs": "48-65,73-96",
        "src/performance.rs": "333-355,402-405",
        "vendor/slint-backend-kindle/src/platform.rs": "391-402,443-477,527-588,618-644,863-899",
    }
    source_identity = {name: {"path": str(args.source / name), "sha256": sha(args.source / name),
                              "relevant_lines": lines} for name, lines in source_refs.items()}
    cases = []
    timeline = []
    spans = []
    checked = 0
    mismatches = []
    with tarfile.open(args.batch / "outputs.tar.gz", "r:gz") as archive:
        for c in a["cases"]:
            name = c["case"]
            prefix = f"out/{name}/"
            records = [json.loads(line) for line in archive.extractfile(prefix + "performance.jsonl").read().splitlines() if line.strip()]
            life = [json.loads(line) for line in archive.extractfile(prefix + "lifecycle.jsonl").read().splitlines() if line.strip()]
            origin = c["request"]["monotonic_ns"]
            for filename, expected in c["file_sha256"].items():
                actual = hashlib.sha256(archive.extractfile(prefix + filename).read()).hexdigest()
                checked += 1
                if actual != expected:
                    mismatches.append({"case": name, "file": filename, "expected": expected, "actual": actual})
            events = []
            for line_number, row in enumerate(records, 1):
                if row.get("kind") != "startup":
                    continue
                data = row.get("data") or {}
                observed = data.get("observed")
                stamp = observed["monotonic_ns"] if isinstance(observed, dict) else row["monotonic_ns"]
                events.append({"stage": row["stage"], "monotonic_ns": stamp,
                               "after_request_ms": delta(stamp, origin), "data": data,
                               "source_line": line_number, "envelope_monotonic_ns": row["monotonic_ns"],
                               "timestamp_semantics": "observed" if isinstance(observed, dict) else "envelope"})

            def choose(stage, **conditions):
                return [event for event in events if event["stage"] == stage and
                        all(event["data"].get(key) == value for key, value in conditions.items())]

            def stamp(stage, **conditions):
                values = choose(stage, **conditions)
                return values[0]["monotonic_ns"] if values else None

            ready = stamp("ui_ready")
            initial = c["first_startup_frame"]
            completion = initial["stages"]["refresh_complete_wait"]["finished_monotonic_ns"]
            worker = stamp("workers_started")
            font_finish = stamp("font_finish")
            prepared = stamp("state_prepared")
            attached = stamp("application_attached")
            route_end = stamp("attach-route", phase="end")
            views_begin = stamp("attach-views", phase="begin")
            covers_begin = choose("attach-covers", phase="begin")
            covers_end = choose("attach-covers", phase="end")
            frame3 = next(frame for frame in c["frame_attempts"] if frame["render_sequence"] == 3)
            frame3_wait = frame3["stages"]["preceding_update_wait"]
            pre_ready = [frame for frame in c["frame_attempts"] if frame["record_kind"] == "native" and
                         frame["record_monotonic_ns"] < ready and not frame["initial_frame"]]
            full = [frame for frame in pre_ready if frame["complete_frame_replacement"] and frame["refresh_accepted"]]
            no_send = [frame for frame in pre_ready if frame["rendered"] and frame["refresh_ioctl_attempts"] == 0]
            route_frames = [frame for frame in pre_ready if frame["record_monotonic_ns"] > route_end and
                            frame["record_monotonic_ns"] < views_begin]
            counts = Counter(event["stage"] for event in events)
            facts = {
                "case": name, "variant": c["software_variant"], "role": c["measurement_role"],
                "request_monotonic_ns": origin, "metrics": c["metrics"], "memory": c["memory"],
                "worker_count": counts["workers_started"],
                "lifecycle_worker_count": sum(event.get("stage") == "workers_started" for event in life),
                "worker_after_initial_send_ms": delta(worker, initial["send_accepted_monotonic_ns"]),
                "initial_completion_after_worker_ms": delta(completion, worker),
                "font_finish_to_state_prepared_ms": delta(prepared, font_finish),
                "state_prepared_to_attached_ms": delta(attached, prepared),
                "font_finish_to_attached_ms": delta(attached, font_finish),
                "state_prepared_before_frame3_wait_finish_ms": delta(frame3_wait["finished_monotonic_ns"], prepared),
                "frame3_wait_started_ms": delta(frame3_wait["started_monotonic_ns"], origin),
                "frame3_wait_finished_ms": delta(frame3_wait["finished_monotonic_ns"], origin),
                "cover_attach_gap_ms": delta(covers_begin[1]["monotonic_ns"], covers_end[0]["monotonic_ns"]),
                "route_to_views_gap_ms": delta(views_begin, route_end),
                "ready_to_send_ms": delta(c["first_business_frame"]["send_accepted_monotonic_ns"], ready),
                "pre_ready_extra_complete_sequences": [frame["render_sequence"] for frame in full],
                "pre_ready_no_send_sequences": [frame["render_sequence"] for frame in no_send],
                "route_window_sequences": [frame["render_sequence"] for frame in route_frames],
                "all_accepted_complete_frames": sum(frame["complete_frame_replacement"] and frame["refresh_accepted"] is True for frame in c["frame_attempts"]),
                "post_business_gate": c["post_business_gate"], "events": events,
                "raw_performance_path": f"{args.batch}/outputs.tar.gz::{prefix}performance.jsonl",
                "frames": c["frame_attempts"],
            }
            cases.append(facts)
            row = {"case": name, "role": c["measurement_role"]}
            for stage in ["process_enter", "workers_started", "font_finish", "state_prepared", "application_attached", "ui_ready"]:
                row[stage + "_ms"] = delta(stamp(stage), origin)
            row.update(route_end_ms=delta(route_end, origin), views_begin_ms=delta(views_begin, origin),
                       font_to_state_ms=facts["font_finish_to_state_prepared_ms"],
                       font_to_attached_ms=facts["font_finish_to_attached_ms"],
                       state_before_seq3_wait_finish_ms=facts["state_prepared_before_frame3_wait_finish_ms"],
                       cover_gap_ms=facts["cover_attach_gap_ms"], route_views_gap_ms=facts["route_to_views_gap_ms"],
                       ready_to_send_ms=facts["ready_to_send_ms"],
                       extra_complete_sequences=",".join(map(str, facts["pre_ready_extra_complete_sequences"])),
                       no_send_sequences=",".join(map(str, facts["pre_ready_no_send_sequences"])),
                       first_business_send_ms=c["metrics"]["content_send_ms"])
            timeline.append(row)
            for frame in c["frame_attempts"]:
                for stage, span in frame["stages"].items():
                    if span:
                        spans.append({"case": name, "render_sequence": frame["render_sequence"],
                                      "complete_frame_replacement": frame["complete_frame_replacement"],
                                      "refresh_ioctl_attempts": frame["refresh_ioctl_attempts"],
                                      "stage": stage,
                                      "started_ms": delta(span["started_monotonic_ns"], origin),
                                      "finished_ms": delta(span["finished_monotonic_ns"], origin),
                                      "wall_ms": span["wall_ns"] / 1_000_000,
                                      "thread_cpu_ms": span["thread_cpu_ns"] / 1_000_000})

    primary = [case for case in cases if case["role"] == "primary"]
    by_name = {case["case"]: case for case in cases}
    groups = {variant: [case for case in primary if case["variant"] == variant] for variant in ["baseline", "candidate"]}
    phase_keys = ["font_finish_to_state_prepared_ms", "font_finish_to_attached_ms", "state_prepared_to_attached_ms",
                  "state_prepared_before_frame3_wait_finish_ms", "cover_attach_gap_ms", "route_to_views_gap_ms", "ready_to_send_ms"]
    phases = {variant: {key: {"median": med([case[key] for case in values]),
                               "minimum": min(case[key] for case in values),
                               "maximum": max(case[key] for case in values),
                               "values": [case[key] for case in values]} for key in phase_keys}
              for variant, values in groups.items()}
    frame_counts = {variant: {
        "primary_extra_pre_ready_complete_frames": sum(len(case["pre_ready_extra_complete_sequences"]) for case in values),
        "primary_pre_ready_no_send_attempts": sum(len(case["pre_ready_no_send_sequences"]) for case in values),
        "primary_accepted_complete_frames": sum(case["all_accepted_complete_frames"] for case in values),
        "all_7_extra_pre_ready_complete_frames": sum(len(case["pre_ready_extra_complete_sequences"]) for case in cases if case["variant"] == variant),
        "all_7_accepted_complete_frames": sum(case["all_accepted_complete_frames"] for case in cases if case["variant"] == variant),
    } for variant, values in groups.items()}
    derived = {
        "schema_version": 1,
        "decision": "not_adopted",
        "scope": "Read-only derived analysis of the ended route-only A/B; computations only on test-env, no tests/build/application/device/lease execution.",
        "batch_root": str(args.batch), "source_root": str(args.source),
        "input_sha256": {name: sha(args.batch / name) for name in ["analysis.json", "report.json", "outputs.tar.gz"]},
        "source_identity": source_identity, "cases": cases,
        "raw_registered_file_hash_count": checked, "raw_registered_file_hash_mismatches": mismatches,
        "summary": a["summary"], "median_reductions": a["median_reductions"], "paired_rounds": a["paired_rounds"],
        "phase_summary": phases, "frame_counts": frame_counts,
        "restoration": a["restoration"], "framebuffer_comparisons": a["framebuffer_comparisons"],
        "limitations": a["limitations"],
    }
    (args.output / "route-analysis.json").write_text(json.dumps(derived, indent=2, sort_keys=True) + "\n")
    for filename, rows in [("timeline.csv", timeline), ("frame-spans.csv", spans)]:
        with (args.output / filename).open("w", newline="") as stream:
            writer = csv.DictWriter(stream, fieldnames=list(rows[0])); writer.writeheader(); writer.writerows(rows)

    lines = [
        "# Stored route-only physical startup A/B, 2026-11-17", "",
        "Decision: not adopted. The isolated route-only change removes the measured pre-Ready GC16 complete replacements, but leaves earlier state/attachment presentation boundaries and Route-to-Views renderer work. Both primary endpoints favor the candidate in only three of six prescribed pairs; the marginal medians are slower. The experiment and structural finding are preserved without adopting a standalone performance improvement.", "",
        "## Frozen scope", "",
        f"This derived report reads the ended `test-env:{args.batch}` batch only. It runs no new tests, builds, applications, emulators, runtime probes, device operations, input actions, or leases, and changes no raw records or runtime source. The raw report records `{r['started_at_utc']}` through `{r['finished_at_utc']}`, successful restoration, unchanged selected installation/input identities, same boot, temporary-stage removal, closed transport and exact owned-lease release.", "",
        "Source `f6208d6f7dd3d8e64a09ef0ceb2ceb6b0c45d0f5` and ELF SHA-256 `3962bc49bb8cd33d21651845fe5de229e1b8458e5d0ba9026485c8bef7224525` are shared. Both variants use default overlapped preparation. Baseline has `--demo --offline --legacy-startup-route-refresh`; candidate has `--demo --offline`. No serial-preparation variant is present. Package, compiler, source and fixture identities are recorded in the ended report. This is a separate experiment from item 1; differences across the two batches are not paired treatment effects and are never pooled here.", "",
        "All fourteen predefined launches are retained: two warmups and six balanced AB/BA pairs. Only the predefined warmups are omitted from primary statistics. All 184 registered raw file identities match. The frozen analyzer's endpoints remain unchanged: request before native Scriptlet exec, `startup/ui_ready` envelope stamp, and first qualified Shelf SEND whose Slint render begins after Ready, all on device `CLOCK_MONOTONIC`. SEND is acceptance rather than own-marker completion, full-cover completion, physical input or optical completion. Device geometry is 1860x2480, 8-bit, stride 1872, `mtk` ABI and `Y8` native mode. These are warm process launches in one boot with retained caches.", "",
        "## All six pairs", "",
        "Positive B-C means the candidate is faster; units are milliseconds.", "",
        "| Pair/order | Baseline SEND | Candidate SEND | SEND B-C | Baseline Ready | Candidate Ready | Ready B-C |",
        "| --- | ---: | ---: | ---: | ---: | ---: | ---: |",
    ]
    for pair in a["paired_rounds"]:
        n = pair["round"]; b = by_name[f"baseline-{n:02}"]; c = by_name[f"candidate-{n:02}"]
        order = "AB" if pair["order"][0] == "baseline" else "BA"
        lines.append(f"| {n:02}/{order} | {fmt(b['metrics']['content_send_ms'])} | {fmt(c['metrics']['content_send_ms'])} | {fmt(pair['metrics']['content_send_ms']['reduction_ms'])} | {fmt(b['metrics']['software_ready_ms'])} | {fmt(c['metrics']['software_ready_ms'])} | {fmt(pair['metrics']['software_ready_ms']['reduction_ms'])} |")
    lines += ["", "| Endpoint | Baseline median | Candidate median | Difference of medians B-C | Median paired B-C | Faster pairs |",
              "| --- | ---: | ---: | ---: | ---: | ---: |"]
    for metric, label in [("content_send_ms", "First Shelf SEND"), ("software_ready_ms", "Ready")]:
        reduction = a["median_reductions"][metric]
        lines.append(f"| {label} | {fmt(a['summary']['baseline'][metric]['median'])} | {fmt(a['summary']['candidate'][metric]['median'])} | {fmt(reduction['difference_of_medians_ms'])} ({reduction['percent_from_medians']:.3f}%) | {fmt(reduction['median_paired_reduction_ms'])} | {sum(pair['metrics'][metric]['reduction_ms'] > 0 for pair in a['paired_rounds'])}/6 |")
    lines += ["",
        "Difference of marginal medians and median of paired differences answer different questions; their different signs are not an error. Neither supports stable improvement with this six-pair, path-dependent result. Ready-to-SEND medians are 768.132 versus 638.561 ms, but its median paired reduction is only 8.578 ms and the 03 pair supplies a 311.646 ms reduction. Moving work before the Ready marker can shrink that interval without an equivalent earlier business SEND.", "",
        "## What complete-frame work actually disappeared", "",
        "Here 'complete replacement' means an accepted non-initial `complete_frame_replacement=true`, `forced_full=true`, GC16 flashing frame before Ready. It does not mean every update rectangle that spans the screen. Every candidate still has its initial checked GC16 complete frame, sequence 2 full-screen-region DU (`complete_frame_replacement=false`), activity-strip AUTO updates, and the final qualified complete Shelf frame.", "",
        "| Count | Baseline | Candidate |",
        "| --- | ---: | ---: |",
        f"| Extra pre-Ready complete replacements, 6 primary launches | {frame_counts['baseline']['primary_extra_pre_ready_complete_frames']} | {frame_counts['candidate']['primary_extra_pre_ready_complete_frames']} |",
        f"| Accepted complete frames including initial/business, 6 primary launches | {frame_counts['baseline']['primary_accepted_complete_frames']} | {frame_counts['candidate']['primary_accepted_complete_frames']} |",
        f"| Extra pre-Ready complete replacements including warmup | {frame_counts['baseline']['all_7_extra_pre_ready_complete_frames']} | {frame_counts['candidate']['all_7_extra_pre_ready_complete_frames']} |",
        f"| Accepted complete frames including warmup | {frame_counts['baseline']['all_7_accepted_complete_frames']} | {frame_counts['candidate']['all_7_accepted_complete_frames']} |",
        f"| Pre-Ready rendered attempts with zero SEND ioctls, 6 primary launches | {frame_counts['baseline']['primary_pre_ready_no_send_attempts']} | {frame_counts['candidate']['primary_pre_ready_no_send_attempts']} |", "",
        "The baseline extra replacements occur only in warmup, 02 and 06, once each, all between Route and Views. None of the seven candidate launches contains another pre-Ready complete replacement from an alternative cause. However, candidate 02/03/06 each contain a rendered sequence 5 with `refresh_ioctl_attempts=0`, null refresh acceptance/marker and no submit span. They wait for preceding marker 4, render an effectively unchanged startup scene, and compute a full RGB fingerprint. These are not accepted partial SENDs or new hardware failures. Separately, both variants retain the accepted partial/activity frames earlier in startup.", "",
        "## Every observed UI boundary", "",
        "Times below are milliseconds. Worker stages use their true `data.observed.monotonic_ns` when present; `font_finish`, UI attachment and phase envelopes use their recorded event stamps. A worker's observed timestamp is not the time the UI consumed its message.", "",
        "| Case | State minus font finish | Font finish to attach | State before seq3 wait end | Cover-attach gap | Route to Views gap | Extra complete / no SEND sequences |",
        "| --- | ---: | ---: | ---: | ---: | ---: | --- |",
    ]
    for c in cases:
        full = ",".join(map(str, c["pre_ready_extra_complete_sequences"])) or "none"
        no_send = ",".join(map(str, c["pre_ready_no_send_sequences"])) or "none"
        lines.append(f"| {c['case']} | {fmt(c['font_finish_to_state_prepared_ms'])} | {fmt(c['font_finish_to_attached_ms'])} | {fmt(c['state_prepared_before_frame3_wait_finish_ms'])} | {fmt(c['cover_attach_gap_ms'])} | {fmt(c['route_to_views_gap_ms'])} | {full} / {no_send} |")
    lines += ["",
        "All twelve primary launches have state prepared after font finish. Candidate state arrives 25.281-113.110 ms afterward, then waits another 155.615-239.820 ms until application attachment. Font-finish-to-attach is 260.498-279.028 ms (median 266.913 ms) for candidate and median 258.200 ms for baseline. The approximately 255 ms handoff issue therefore remains; it is not fixed by route refresh suppression. Baseline warmup even observes state 1.580 ms before font finish yet attaches 257.992 ms later, demonstrating why an observed worker timestamp alone cannot establish that the current UI closure has consumed it.", "",
        "All twelve primary state results are ready before the existing sequence 3 preceding-marker wait finishes, by 85.256-181.609 ms (candidate 85.256-166.095 ms). The marker wait is already mandatory for the currently planned frame; no extra 300 ms delay is needed to explain the opportunity. The current pipeline renders and fingerprints that intermediate scene before another UI control dispatch attaches the application. This is evidence for studying safe coalescing of already-ready work at an existing barrier, not proof that a particular reorder is correct.", "",
        "All twelve primary launches also have an accepted partial frame between the two cover-attachment chunks. The first-end-to-second-begin gaps are 408.460-424.949 ms for baseline and 410.015-413.616 ms for candidate. Sequence 4 preceding-marker waits are about 347-351 ms. Route-only suppression leaves these earlier presentation/continuation boundaries intact.", "",
        "## Actual Route-to-Views critical paths", "",
        "| Case/sequence | Route end to Views begin | Preceding-marker wait | Slint render | RGB fingerprint | Framebuffer write | Own completion wait | New SEND |",
        "| --- | ---: | ---: | ---: | ---: | ---: | ---: | --- |",
    ]
    for name in ["baseline-02", "candidate-02", "candidate-03", "baseline-06", "candidate-06"]:
        c = by_name[name]; frame = next(frame for frame in c["frames"] if frame["render_sequence"] == 5)
        stage = frame["stages"]
        value = lambda key: stage[key]["wall_ns"] / 1_000_000 if stage.get(key) else None
        lines.append(f"| {name}/5 | {fmt(c['route_to_views_gap_ms'])} | {fmt(value('preceding_update_wait'))} | {fmt(value('slint_render'))} | {fmt(value('rgb_fingerprint'))} | {fmt(value('framebuffer_write'))} | {fmt(value('refresh_complete_wait'))} | {'accepted GC16' if frame['refresh_accepted'] else 'none; 0 ioctls'} |")
    lines += ["",
        "In pair 06, suppressing the replacement corresponds to a Route-to-Views gap reduction from 821.514 to 404.685 ms; Ready is 425.064 ms earlier and business SEND 435.689 ms earlier. Pair 02 similarly reduces the gap from 823.233 to 410.680 ms, but candidate request-to-process-entry is 295.686 ms longer, limiting its final endpoint improvement. That front-segment variation remains a valid observation without a proven scheduling/cache/power cause.", "",
        "Pair 03 shows why the change is insufficient: baseline already coalesces Route and Views (0.989 ms gap), whereas candidate crosses another presentation boundary and spends 400.662 ms there despite issuing no SEND. Its old-marker wait is 347.980 ms and diagnostic fingerprint costs 49.238 ms; Ready regresses by 371.433 ms. The post-Ready path is 311.646 ms shorter because the old marker has already been waited, leaving a 59.787 ms SEND regression. The large improvement in a Ready-to-SEND interval must not be presented as independently gained time.", "",
        "Pairs 01/04/05 have no extra Route replacement or no-SEND attempt in either variant. Their differences therefore cannot be attributed to removing an observed route replacement in those launches. All pairs remain included; 06 alone does not establish a general gain, and 03 alone does not establish a scheduler defect.", "",
        "The frozen source still sets the hidden Shell route in `navigate()` but suppresses its `EnterScreen` unless the legacy flag is enabled (`src/app/mod.rs:370-375`). The 4 ms between-stage budget can yield between Route and Views (`src/app/startup.rs:286-295`), and the continuation remains a 1 ms timer (`src/runtime_wakeup.rs:86-95`). The backend takes a control-queue snapshot before rendering (`platform.rs:391-402`) and waits for settled mapping inside the renderer callback before it discovers that an attempted scene has no dirty pixels (`platform.rs:527-588`). Consequently, removing only the refresh request can remove a submission and its own completion wait while preserving the previous-marker wait and diagnostic fingerprint work. The exact trace confirms those spans; the producer identity, individual budget decisions and notification scheduling decisions are not logged.", "",
        "## Unimplemented design implication", "",
        "The existing wait contains a real opportunity to study bounded servicing of already-ready startup controls/continuations before choosing a render scene. A possible design would separate the mandatory wait from renderer/UI borrows, then recheck lifecycle and refresh ownership before a bounded dispatch. This must preserve the 4 ms startup-stage budget, first-frame feedback, cancellation and own-marker/input protections; neither a new 300 ms delay nor a larger budget is justified. No such coalescing or backend-loop change was implemented or evaluated, and no benefit is promised.", "",
        "An immediate second message drain at font finish alone would not solve these twelve launches because state arrives later. The evidence supports only the above design hypothesis at an existing barrier. Instrumented RGB fingerprint and synchronous trace writes contribute measured overhead; their influence on timing thresholds or normal untraced behavior remains unquantified.", "",
        "## Front segment, memory and pixels", "",
        "| Primary metric | Baseline median | Candidate median |",
        "| --- | ---: | ---: |",
    ]
    for key, label in [("request_to_process_enter_ms", "Request to process entry (ms)"),
                       ("first_startup_send_ms", "Request to startup SEND (ms)"),
                       ("process_enter_to_ready_ms", "Process entry to Ready (ms)"),
                       ("rss_kib", "Post-business sampled VmRSS (KiB)"),
                       ("hwm_kib", "Post-business sampled VmHWM (KiB)")]:
        lines.append(f"| {label} | {fmt(a['summary']['baseline'][key]['median'])} | {fmt(a['summary']['candidate'][key]['median'])} |")
    lines += ["",
        "Process-entry-to-startup-SEND medians are 307.069 and 307.944 ms. The front segment is not improved. The candidate 02 native front segment is retained and reported without assigning an unobserved cause. Sampled RSS differs by only 50 KiB (0.089%) at the marginal medians, with mixed paired directions. Memory/input procfs observations follow the common first-business SEND plus 1500 ms gate and any scheduled framebuffer capture. They are not exact SEND samples or a continuously measured startup peak; sampled VmHWM is the kernel high-water field up to that later observation, not a whole-lifetime peak measurement.", "",
        "All fourteen final logical Slint RGB snapshots match before framebuffer compensation. Early and settled raw framebuffer comparisons cover only baseline-06 versus candidate-06; each compares all 4,612,800 visible 8-bit pixels without masks and reports zero differences. This does not claim fourteen raw framebuffer capture pairs, optical panel equality/completion, physical touch readiness, sleep/wake acceptance, live-account behavior, or a full-cover completion endpoint. Queued controls can run before a pending input-drain branch; do not infer that every UI consumer is later than physical input readiness.", "",
        "## Exact evidence and frozen deliverables", "",
        f"- Ended raw scope and identities: `{args.batch}/report.json`.",
        f"- Unchanged frozen analyzer results: `{args.batch}/analysis.json` (`cases`, `summary`, `paired_rounds`, `median_reductions`, `restoration`, `framebuffer_comparisons`).",
        f"- Raw cases: `{args.batch}/outputs.tar.gz::out/<case>/{{request.json,performance.jsonl,lifecycle.jsonl,memory-at-content.txt,post-business-gate.json,shell.tsv,final.json,recovery.json}}`.",
        f"- Raw framebuffer scope: the same archive's `out/{{baseline-06,candidate-06}}/{{content-frame.raw,settled-frame.raw}}` and adjacent capture request/completion records.",
        f"- Source read for interpretation: `{args.source}`. Exact path/hash/line references are in `route-analysis.json/source_identity`.",
        "- `route-analysis.json` records exact absolute and observed/envelope stamps, original record line numbers, all frames, count summaries, stage-gap distributions and input hashes. `timeline.csv` and `frame-spans.csv` expose all fourteen cases. Stage spans can overlap; do not sum overlapping worker/UI spans into elapsed time.",
        "- `analysis-manifest.json` binds the final four output files and analysis script. The script runs only against stored data on test-env. These deliverables are frozen together and synchronized byte-for-byte; the previously frozen item1 directory is unchanged.", "",
        "All narrative/table times are rounded to three decimals; exact stored stamp-derived precision remains in JSON/CSV. No sample is excluded because of its speed or route path.", "",
    ]
    (args.output / "README.md").write_text("\n".join(lines))
    outputs = ["README.md", "route-analysis.json", "timeline.csv", "frame-spans.csv"]
    manifest = {"schema_version": 1,
                "processing": "Derived stored-evidence analysis only on test-env; no new runtime/device/test/build work and no raw-byte changes",
                "inputs": derived["input_sha256"], "script_sha256": sha(Path(__file__)),
                "outputs": {name: {"bytes": (args.output / name).stat().st_size, "sha256": sha(args.output / name)} for name in outputs}}
    (args.output / "analysis-manifest.json").write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")
    files = [Path(__file__).name] + outputs + ["analysis-manifest.json"]
    print(json.dumps({"output": str(args.output), "frame_counts": frame_counts,
                      "registered_raw_files": checked, "raw_hash_mismatches": mismatches,
                      "deliverables": {name: {"bytes": (args.output / name).stat().st_size,
                                              "sha256": sha(args.output / name)} for name in files}}, indent=2))


if __name__ == "__main__":
    main()
