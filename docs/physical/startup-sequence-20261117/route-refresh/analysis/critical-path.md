# Stored route-only physical startup A/B, 2026-11-17

Decision: not adopted. The isolated route-only change removes the measured pre-Ready GC16 complete replacements, but leaves earlier state/attachment presentation boundaries and Route-to-Views renderer work. Both primary endpoints favor the candidate in only three of six prescribed pairs; the marginal medians are slower. The experiment and structural finding are preserved without adopting a standalone performance improvement.

## Frozen scope

This derived report reads the ended `test-env:/root/bilicomics-physical-startup-route-20261117-196c8a73` batch only. It runs no new tests, builds, applications, emulators, runtime probes, device operations, input actions, or leases, and changes no raw records or runtime source. The raw report records `2026-11-17T10:57:15.120226+00:00` through `2026-11-17T11:03:28.437825+00:00`, successful restoration, unchanged selected installation/input identities, same boot, temporary-stage removal, closed transport and exact owned-lease release.

Source `f6208d6f7dd3d8e64a09ef0ceb2ceb6b0c45d0f5` and ELF SHA-256 `3962bc49bb8cd33d21651845fe5de229e1b8458e5d0ba9026485c8bef7224525` are shared. Both variants use default overlapped preparation. Baseline has `--demo --offline --legacy-startup-route-refresh`; candidate has `--demo --offline`. No serial-preparation variant is present. Package, compiler, source and fixture identities are recorded in the ended report. This is a separate experiment from item 1; differences across the two batches are not paired treatment effects and are never pooled here.

All fourteen predefined launches are retained: two warmups and six balanced AB/BA pairs. Only the predefined warmups are omitted from primary statistics. All 184 registered raw file identities match. The frozen analyzer's endpoints remain unchanged: request before native Scriptlet exec, `startup/ui_ready` envelope stamp, and first qualified Shelf SEND whose Slint render begins after Ready, all on device `CLOCK_MONOTONIC`. SEND is acceptance rather than own-marker completion, full-cover completion, physical input or optical completion. Device geometry is 1860x2480, 8-bit, stride 1872, `mtk` ABI and `Y8` native mode. These are warm process launches in one boot with retained caches.

## All six pairs

Positive B-C means the candidate is faster; units are milliseconds.

| Pair/order | Baseline SEND | Candidate SEND | SEND B-C | Baseline Ready | Candidate Ready | Ready B-C |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 01/AB | 6347.143 | 6426.293 | -79.151 | 5578.121 | 5663.803 | -85.682 |
| 02/BA | 6997.369 | 6833.140 | 164.229 | 6494.037 | 6348.265 | 145.773 |
| 03/AB | 6567.124 | 6626.911 | -59.787 | 5740.846 | 6112.279 | -371.433 |
| 04/BA | 6345.365 | 6523.932 | -178.566 | 5569.150 | 5711.640 | -142.490 |
| 05/AB | 6451.448 | 6312.760 | 138.688 | 5684.205 | 5544.881 | 139.324 |
| 06/BA | 6955.340 | 6519.651 | 435.689 | 6481.452 | 6056.388 | 425.064 |

| Endpoint | Baseline median | Candidate median | Difference of medians B-C | Median paired B-C | Faster pairs |
| --- | ---: | ---: | ---: | ---: | ---: |
| First Shelf SEND | 6509.286 | 6521.791 | -12.505 (-0.192%) | 39.451 | 3/6 |
| Ready | 5712.526 | 5884.014 | -171.488 (-3.002%) | 26.821 | 3/6 |

Difference of marginal medians and median of paired differences answer different questions; their different signs are not an error. Neither supports stable improvement with this six-pair, path-dependent result. Ready-to-SEND medians are 768.132 versus 638.561 ms, but its median paired reduction is only 8.578 ms and the 03 pair supplies a 311.646 ms reduction. Moving work before the Ready marker can shrink that interval without an equivalent earlier business SEND.

## What complete-frame work actually disappeared

Here 'complete replacement' means an accepted non-initial `complete_frame_replacement=true`, `forced_full=true`, GC16 flashing frame before Ready. It does not mean every update rectangle that spans the screen. Every candidate still has its initial checked GC16 complete frame, sequence 2 full-screen-region DU (`complete_frame_replacement=false`), activity-strip AUTO updates, and the final qualified complete Shelf frame.

| Count | Baseline | Candidate |
| --- | ---: | ---: |
| Extra pre-Ready complete replacements, 6 primary launches | 2 | 0 |
| Accepted complete frames including initial/business, 6 primary launches | 14 | 12 |
| Extra pre-Ready complete replacements including warmup | 3 | 0 |
| Accepted complete frames including warmup | 17 | 14 |
| Pre-Ready rendered attempts with zero SEND ioctls, 6 primary launches | 0 | 3 |

The baseline extra replacements occur only in warmup, 02 and 06, once each, all between Route and Views. None of the seven candidate launches contains another pre-Ready complete replacement from an alternative cause. However, candidate 02/03/06 each contain a rendered sequence 5 with `refresh_ioctl_attempts=0`, null refresh acceptance/marker and no submit span. They wait for preceding marker 4, render an effectively unchanged startup scene, and compute a full RGB fingerprint. These are not accepted partial SENDs or new hardware failures. Separately, both variants retain the accepted partial/activity frames earlier in startup.

## Every observed UI boundary

Times below are milliseconds. Worker stages use their true `data.observed.monotonic_ns` when present; `font_finish`, UI attachment and phase envelopes use their recorded event stamps. A worker's observed timestamp is not the time the UI consumed its message.

| Case | State minus font finish | Font finish to attach | State before seq3 wait end | Cover-attach gap | Route to Views gap | Extra complete / no SEND sequences |
| --- | ---: | ---: | ---: | ---: | ---: | --- |
| baseline-warmup | -1.580 | 257.992 | 191.857 | 413.573 | 820.949 | 5 / none |
| candidate-warmup | -47.912 | 0.679 | 248.813 | 2.706 | 0.378 | none / none |
| baseline-01 | 29.301 | 262.868 | 168.927 | 415.966 | 0.481 | none / none |
| candidate-01 | 32.276 | 260.498 | 158.540 | 411.639 | 0.306 | none / none |
| candidate-02 | 113.110 | 268.725 | 85.256 | 410.015 | 410.680 | none / 5 |
| baseline-02 | 11.840 | 257.183 | 181.609 | 408.460 | 823.233 | 5 / none |
| baseline-03 | 46.913 | 258.164 | 141.783 | 424.949 | 0.989 | none / none |
| candidate-03 | 25.281 | 265.101 | 166.095 | 411.623 | 400.662 | none / 5 |
| candidate-04 | 63.219 | 279.028 | 134.995 | 413.616 | 1.652 | none / none |
| baseline-04 | 28.490 | 270.282 | 168.615 | 414.489 | 0.268 | none / none |
| baseline-05 | 10.930 | 252.073 | 174.485 | 417.012 | 2.078 | none / none |
| candidate-05 | 49.455 | 273.144 | 147.394 | 412.050 | 0.270 | none / none |
| candidate-06 | 33.764 | 263.696 | 158.296 | 411.995 | 404.685 | none / 5 |
| baseline-06 | 38.326 | 258.237 | 150.733 | 420.027 | 821.514 | 5 / none |

All twelve primary launches have state prepared after font finish. Candidate state arrives 25.281-113.110 ms afterward, then waits another 155.615-239.820 ms until application attachment. Font-finish-to-attach is 260.498-279.028 ms (median 266.913 ms) for candidate and median 258.200 ms for baseline. The approximately 255 ms handoff issue therefore remains; it is not fixed by route refresh suppression. Baseline warmup even observes state 1.580 ms before font finish yet attaches 257.992 ms later, demonstrating why an observed worker timestamp alone cannot establish that the current UI closure has consumed it.

All twelve primary state results are ready before the existing sequence 3 preceding-marker wait finishes, by 85.256-181.609 ms (candidate 85.256-166.095 ms). The marker wait is already mandatory for the currently planned frame; no extra 300 ms delay is needed to explain the opportunity. The current pipeline renders and fingerprints that intermediate scene before another UI control dispatch attaches the application. This is evidence for studying safe coalescing of already-ready work at an existing barrier, not proof that a particular reorder is correct.

All twelve primary launches also have an accepted partial frame between the two cover-attachment chunks. The first-end-to-second-begin gaps are 408.460-424.949 ms for baseline and 410.015-413.616 ms for candidate. Sequence 4 preceding-marker waits are about 347-351 ms. Route-only suppression leaves these earlier presentation/continuation boundaries intact.

## Actual Route-to-Views critical paths

| Case/sequence | Route end to Views begin | Preceding-marker wait | Slint render | RGB fingerprint | Framebuffer write | Own completion wait | New SEND |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| baseline-02/5 | 823.233 | 351.734 | 22.399 | 49.490 | 29.815 | 365.616 | accepted GC16 |
| candidate-02/5 | 410.680 | 350.247 | 0.196 | 57.078 | 0.003 | - | none; 0 ioctls |
| candidate-03/5 | 400.662 | 347.980 | 0.243 | 49.238 | 0.002 | - | none; 0 ioctls |
| baseline-06/5 | 821.514 | 348.522 | 26.543 | 46.120 | 29.399 | 366.253 | accepted GC16 |
| candidate-06/5 | 404.685 | 343.847 | 0.343 | 56.853 | 0.003 | - | none; 0 ioctls |

In pair 06, suppressing the replacement corresponds to a Route-to-Views gap reduction from 821.514 to 404.685 ms; Ready is 425.064 ms earlier and business SEND 435.689 ms earlier. Pair 02 similarly reduces the gap from 823.233 to 410.680 ms, but candidate request-to-process-entry is 295.686 ms longer, limiting its final endpoint improvement. That front-segment variation remains a valid observation without a proven scheduling/cache/power cause.

Pair 03 shows why the change is insufficient: baseline already coalesces Route and Views (0.989 ms gap), whereas candidate crosses another presentation boundary and spends 400.662 ms there despite issuing no SEND. Its old-marker wait is 347.980 ms and diagnostic fingerprint costs 49.238 ms; Ready regresses by 371.433 ms. The post-Ready path is 311.646 ms shorter because the old marker has already been waited, leaving a 59.787 ms SEND regression. The large improvement in a Ready-to-SEND interval must not be presented as independently gained time.

Pairs 01/04/05 have no extra Route replacement or no-SEND attempt in either variant. Their differences therefore cannot be attributed to removing an observed route replacement in those launches. All pairs remain included; 06 alone does not establish a general gain, and 03 alone does not establish a scheduler defect.

The frozen source still sets the hidden Shell route in `navigate()` but suppresses its `EnterScreen` unless the legacy flag is enabled (`src/app/mod.rs:370-375`). The 4 ms between-stage budget can yield between Route and Views (`src/app/startup.rs:286-295`), and the continuation remains a 1 ms timer (`src/runtime_wakeup.rs:86-95`). The backend takes a control-queue snapshot before rendering (`platform.rs:391-402`) and waits for settled mapping inside the renderer callback before it discovers that an attempted scene has no dirty pixels (`platform.rs:527-588`). Consequently, removing only the refresh request can remove a submission and its own completion wait while preserving the previous-marker wait and diagnostic fingerprint work. The exact trace confirms those spans; the producer identity, individual budget decisions and notification scheduling decisions are not logged.

## Unimplemented design implication

The existing wait contains a real opportunity to study bounded servicing of already-ready startup controls/continuations before choosing a render scene. A possible design would separate the mandatory wait from renderer/UI borrows, then recheck lifecycle and refresh ownership before a bounded dispatch. This must preserve the 4 ms startup-stage budget, first-frame feedback, cancellation and own-marker/input protections; neither a new 300 ms delay nor a larger budget is justified. No such coalescing or backend-loop change was implemented or evaluated, and no benefit is promised.

An immediate second message drain at font finish alone would not solve these twelve launches because state arrives later. The evidence supports only the above design hypothesis at an existing barrier. Instrumented RGB fingerprint and synchronous trace writes contribute measured overhead; their influence on timing thresholds or normal untraced behavior remains unquantified.

## Front segment, memory and pixels

| Primary metric | Baseline median | Candidate median |
| --- | ---: | ---: |
| Request to process entry (ms) | 4131.752 | 4134.739 |
| Request to startup SEND (ms) | 4437.426 | 4457.605 |
| Process entry to Ready (ms) | 1558.433 | 1751.744 |
| Post-business sampled VmRSS (KiB) | 55964.000 | 55914.000 |
| Post-business sampled VmHWM (KiB) | 55980.000 | 55930.000 |

Process-entry-to-startup-SEND medians are 307.069 and 307.944 ms. The front segment is not improved. The candidate 02 native front segment is retained and reported without assigning an unobserved cause. Sampled RSS differs by only 50 KiB (0.089%) at the marginal medians, with mixed paired directions. Memory/input procfs observations follow the common first-business SEND plus 1500 ms gate and any scheduled framebuffer capture. They are not exact SEND samples or a continuously measured startup peak; sampled VmHWM is the kernel high-water field up to that later observation, not a whole-lifetime peak measurement.

All fourteen final logical Slint RGB snapshots match before framebuffer compensation. Early and settled raw framebuffer comparisons cover only baseline-06 versus candidate-06; each compares all 4,612,800 visible 8-bit pixels without masks and reports zero differences. This does not claim fourteen raw framebuffer capture pairs, optical panel equality/completion, physical touch readiness, sleep/wake acceptance, live-account behavior, or a full-cover completion endpoint. Queued controls can run before a pending input-drain branch; do not infer that every UI consumer is later than physical input readiness.

## Exact evidence and frozen deliverables

- Ended raw scope and identities: `/root/bilicomics-physical-startup-route-20261117-196c8a73/report.json`.
- Unchanged frozen analyzer results: `/root/bilicomics-physical-startup-route-20261117-196c8a73/analysis.json` (`cases`, `summary`, `paired_rounds`, `median_reductions`, `restoration`, `framebuffer_comparisons`).
- Raw cases: `/root/bilicomics-physical-startup-route-20261117-196c8a73/outputs.tar.gz::out/<case>/{request.json,performance.jsonl,lifecycle.jsonl,memory-at-content.txt,post-business-gate.json,shell.tsv,final.json,recovery.json}`.
- Raw framebuffer scope: the same archive's `out/{baseline-06,candidate-06}/{content-frame.raw,settled-frame.raw}` and adjacent capture request/completion records.
- Source read for interpretation: `/root/bilicomics-startup-sequence-20261117-01a1d11a-r1/candidate-r2/app`. Exact path/hash/line references are in `route-analysis.json/source_identity`.
- `route-analysis.json` records exact absolute and observed/envelope stamps, original record line numbers, all frames, count summaries, stage-gap distributions and input hashes. `timeline.csv` and `frame-spans.csv` expose all fourteen cases. Stage spans can overlap; do not sum overlapping worker/UI spans into elapsed time.
- `analysis-manifest.json` binds the final four output files and analysis script. The script runs only against stored data on test-env. These deliverables are frozen together and synchronized byte-for-byte; the previously frozen item1 directory is unchanged.

All narrative/table times are rounded to three decimals; exact stored stamp-derived precision remains in JSON/CSV. No sample is excluded because of its speed or route path.
