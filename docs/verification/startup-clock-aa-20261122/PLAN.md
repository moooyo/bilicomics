# Native Shelf A/A: startup landmarks and visit position

## Question and fixed schedule

Does a first-versus-second difference recur when every launch uses the **same** stock-equivalent instrumented launcher, ARM application, ordinary Library card, launch arguments and fresh copy of the same closed offline Shelf seed? Where on the device clock does any observed variation occur? A and B are schedule labels, not implementations. This is a descriptive Scribe 5.19.5 study: no performance promotion, duration threshold, deployment or causal inference is possible from it. Do not pool it with earlier A/B data.

Exclude two warmups (pair 0: A, B), then retain exactly four adjacent pairs: **1 AB, 2 BA, 3 AB, 4 BA**, ten visits in total. Analyze `second − first` in actual launch order, including for BA. No retry, substituted visit or extension. A visit starts only after the shared device is available for actual use and ends with native control restored, owned connections closed and exact lease release. Preparation, pauses, waiting for another project and analysis stay outside the lease. Do not reserve the device or manufacture a fixed interval. If a visit cannot complete or release cannot be verified, stop. Preserve any unpaired completed visit but do not call it a measured pair; never skip a visit to complete a later one.

## Clock and frame definitions

Use device `CLOCK_MONOTONIC` throughout. For the ordinary native card button-up bracket `[lo, hi]` and an event at `t`, the elapsed interval is `[t − hi, t − lo]`. The primary landmark remains the software renderer return of the **first qualifying ordinary Shelf** (normal layout, entries or the appropriate empty state, and navigation). Covers may be placeholders. No second tap, optical timing or live-account claim. `SEND` acceptance and driver completion on that same positive marker are separate secondary endpoints and may be unavailable. Do not replace the primary with a later event.

| Landmark | What the timestamp means |
| --- | --- |
| `launch_entry` | Earliest instrumented Bili-owned shim mark **after** its route/ownership checks and entry receipt. Card-to-mark includes native activation and earlier shim work; it is not handler dispatch. |
| `pre_app_invocation` | Marker wrapper immediately before launching the app. Its own receipt writing and executable load precede Rust `main`; this is not a pure `exec` or linker measurement. |
| `rust_main_entry` | The timestamp already captured at the beginning of Rust `main`, recovered from `process_enter` for the same application process. Not the diagnostic header. |
| `trace_started` | The Shelf diagnostic header, emitted after its setup. |
| `state_produced`, `state_consumed` | Optional original preparation timestamp and a **separately witnessed** UI observation for the same result. The retained ELF's lifecycle `state_prepared` may recover the producer, but its file write, the header, Ready, attachment or a frame cannot establish UI consumption. The expected consumer is unavailable. Do not enable the incompatible full performance trace. |
| `shelf_rendered`, `shelf_submitted`, `driver_completed` | Original frame-event timestamps, not the later trace envelope writes; bind accepted submission and completion to the same frame and positive marker when available. |

## Frozen analysis

1. For each observed endpoint report the number of contributing complete pairs; summarize by visit position (first and second), by pair, and by the predeclared AB and BA order groups. Difference intervals are `[second.lo − first.hi, second.hi − first.lo]`. Take medians of the paired lower and upper bounds separately; a negative upper bound is definitively earlier, a positive lower bound definitively later, and touching/overlapping bounds are indeterminate. Do not subtract marginal medians or sum segment medians.
2. Decompose the path per visit into card→entry, entry→pre-invocation, pre-invocation→Rust `main`, `main`→header, and header→renderer. Also show card→header, optional header→producer and producer→UI when both timestamps exist, plus renderer→accepted `SEND`→same-marker driver. Header→renderer and producer→renderer include other work; neither is a measure of UI queueing. Report missing stages and reduced denominators; never impute zero. A valid renderer pair remains valid if a secondary stage is missing.
3. For every adjacent launch after the warmups, report the **release-to-release cadence** using the two card brackets. Keep the four within-pair gaps separate from the three preceding-pair-second→next-pair-first gaps. This includes execution, exit, native restoration, lease turnover and any intervening use. It is **not** a measure of how long another project held the device, a controlled washout, or evidence of why a launch differed. With only four pairs and two launches per nominal-order group, do not fit a cadence model, invent statistical confidence, or select a convenient subset.

## Off-device replay

Private plan and archive commands require `BILICOMICS_STUDY_OWNER` from the original operator approval, even without a live `CODEX_THREAD_ID`; do not use the plan as its own authority. Public projection analysis requires neither ID.

The collector checks private boot, process, run, source/launcher, marker and restoration receipts. The analyzer reads its ordered, closed-prefix projection (`bili-shelf-clock-aa-projection-v1`), with `CLOCK_MONOTONIC` offsets rebased to the first card's release bracket. It does not contact a Kindle, inspect a lease, or export private identifiers. Public results contain elapsed durations and counts. A partial replay can be useful, but is explicitly incomplete and does not authorize more visits.

From this directory on `test-env`, run `python3 -B -m unittest -v test_analyze_aa.py`. Only **after the runner exits and native recovery/release is complete**, run `python3 -B aa_collect.py --plan /private/study/plan.json` and save its successful anonymous projection **before discarding any unattempted slot directories**. That allows the collector to distinguish a verified prefix from a lost or skipped visit. Run `python3 -B analyze_aa.py /path/to/projection.json`, then save and independently replay the projection before cleanup. `python3 -B archive_closed.py /private/study/plan.json /private/study/projection.json /private/study/closed-clocks.tar.gz` creates a private allowlisted clock archive only for the qualified, naturally exited and released prefix. Neither tool releases a lease or authorizes another launch.
