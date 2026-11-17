# Item 1: stored physical startup A/B analysis, 2026-11-17

Keep the early background-admission architecture as a candidate, but do not describe this batch as a stable startup-speed acceptance. The six prescribed pairs favor the candidate at the median, while the observed UI continuation boundaries produce large variations and one SEND regression. The next implementation iteration should remove the redundant full replacement between startup Route and Views before treating this performance result as ready for adoption.

## Scope and provenance

This analysis reads only the ended batch `/root/bilicomics-physical-startup-sequence-20261117-f028b769` on `test-env`. It executes no application, test suite, build, emulator, runtime probe, device command, input action, or lease operation. It does not modify raw evidence or runtime source. The raw physical batch ran from `2026-11-17T10:12:27.872863+00:00` to `2026-11-17T10:18:37.889312+00:00` and is recorded as successful with the exact owned lease released. Native control, selected installation identities, lighting, input roster, and same-boot checks are recorded as restored/unchanged; the temporary stage was removed and transport closed.

The frozen source is `530f1516c332931059c52161b52d64e86488b3f1`. Both variants use ARM ELF SHA-256 `04fc6f93b4327911ca1a3bb5200175c9f2e13fb21ac5e56280012ff44b6e2e59`, the same package and compiler profile, and the same offline demo snapshot. Baseline arguments are `--demo --offline --serial-startup-preparation`; candidate arguments are `--demo --offline`. This controls the preparation path within one ELF; it does not compare the old production binary against a rebuilt candidate.

All fourteen prescribed launches remain in the record: baseline/candidate warmups, then six balanced AB/BA pairs. Only the two predefined warmups are omitted from primary distributions. No earlier failed native-query batch is read or combined. All 184 registered raw file identities match their frozen analyzer records. Derived input and relevant frozen-source hashes are retained in `item1-analysis.json`.

Primary timing is device `CLOCK_MONOTONIC`: request precedes the native launch command, Ready is the `startup/ui_ready` envelope stamp, and first Shelf SEND is the first accepted rendered native business frame whose Slint render begins after Ready. This is not a physical-tap, complete-cover, driver-completion, or optical endpoint. Geometry is 1860x2480, 8-bit framebuffer, stride 1872, `mtk` ABI, native `Y8` mode. One warm device boot and retained caches limit generalization.

## Complete paired results

Positive B-C means the candidate is faster. Times are milliseconds.

| Pair/order | Baseline SEND | Candidate SEND | SEND B-C | Baseline Ready | Candidate Ready | Ready B-C |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 01/AB | 6721.950 | 6472.051 | 249.899 | 5945.663 | 5699.588 | 246.075 |
| 02/BA | 6574.331 | 6458.672 | 115.660 | 5762.752 | 5682.717 | 80.035 |
| 03/AB | 6603.182 | 6560.644 | 42.537 | 5836.019 | 6051.432 | -215.414 |
| 04/BA | 6702.586 | 5900.555 | 802.031 | 5882.844 | 5158.023 | 724.821 |
| 05/AB | 6690.967 | 5876.609 | 814.358 | 5928.267 | 5140.400 | 787.866 |
| 06/BA | 6726.072 | 6887.877 | -161.805 | 5945.042 | 6410.300 | -465.258 |

| Endpoint | Baseline median | Candidate median | Difference of medians | Median paired B-C | Faster pairs |
| --- | ---: | ---: | ---: | ---: | ---: |
| First Shelf SEND | 6696.777 | 6465.361 | 231.415 (3.456%) | 182.779 | 5/6 |
| Ready | 5905.555 | 5691.153 | 214.402 (3.631%) | 163.055 | 4/6 |

The candidate SEND range is 5876.609-6887.877 ms, versus 6574.331-6726.072 ms for baseline. Candidate Ready ranges from 5140.400 to 6410.300 ms, versus 5762.752-5945.663 ms. This six-pair batch establishes the measured distribution, not a population-wide improvement, tail guarantee, or cold-start result. Both the large wins and the regressions occur in the predefined record and remain included.

## Observed one-shot admission

Every raw performance trace contains exactly one `workers_started`, and every lifecycle trace independently contains exactly one. These are two records of one admission, not two worker starts. Every case has one font worker and one state/metadata worker; each candidate additionally has one gated cover-completion worker. The table uses `data.observed.monotonic_ns` for `workers_started`, never its later envelope/write time. All absolute stamps, enclosing stamps, raw record line numbers, counts, and comparisons are retained in `item1-analysis.json`.

`Done` is the finish stamp of the original initial frame's checked own-marker completion wait. Every initial frame is accepted, full, rendered, and completed with accepted marker 1 equal to completed marker 1. `Done-Worker` is positive when the worker starts before completion.

| Case | Worker count | Initial SEND | Workers | Done | Worker-SEND | Done-Worker | Cover-prepare gate | Cover-Done |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| baseline-warmup | 1 | 4633.092 | 5002.421 | 4998.209 | 369.329 | -4.212 | 457.129 | 460.506 |
| candidate-warmup | 1 | 4509.426 | 4512.636 | 4875.064 | 3.211 | 362.428 | 57.089 | 59.292 |
| baseline-01 | 1 | 4445.604 | 4818.247 | 4811.830 | 372.643 | -6.417 | 461.391 | 466.991 |
| candidate-01 | 1 | 4469.046 | 4471.634 | 4834.701 | 2.588 | 363.067 | 33.483 | 35.948 |
| candidate-02 | 1 | 4448.765 | 4449.917 | 4814.879 | 1.152 | 364.962 | 27.177 | 30.850 |
| baseline-02 | 1 | 4270.929 | 4641.690 | 4636.470 | 370.761 | -5.220 | 467.586 | 472.287 |
| baseline-03 | 1 | 4344.963 | 4716.692 | 4710.619 | 371.728 | -6.073 | 463.134 | 468.362 |
| candidate-03 | 1 | 4430.607 | 4431.697 | 4796.302 | 1.090 | 364.605 | 22.239 | 24.733 |
| candidate-04 | 1 | 4342.571 | 4343.831 | 4706.972 | 1.260 | 363.141 | 11.039 | 13.120 |
| baseline-04 | 1 | 4363.234 | 4736.589 | 4728.923 | 373.355 | -7.666 | 496.134 | 502.631 |
| baseline-05 | 1 | 4432.265 | 4805.952 | 4798.148 | 373.687 | -7.804 | 468.909 | 476.017 |
| candidate-05 | 1 | 4329.798 | 4332.406 | 4695.436 | 2.609 | 363.030 | 39.571 | 41.371 |
| candidate-06 | 1 | 4365.187 | 4368.066 | 4730.833 | 2.879 | 362.767 | 42.923 | 44.530 |
| baseline-06 | 1 | 4458.822 | 4830.952 | 4824.395 | 372.130 | -6.557 | 497.202 | 503.223 |

All seven candidates, including the warmup, admit the workers strictly after successful initial SEND and inside its original checked completion wait. All seven baselines start after that completion. The actual candidate font byte preparation finishes before original completion in every case. Candidate metadata starts during that wait in every case; metadata completes after it in all seven cases. Thus the proven overlap is font-byte preparation plus part of metadata preparation, not UI font registration or cover pixel preparation.

Candidate cover pixel work begins after both the original `startup_gate=prepare` UI boundary and the original checked completion in all seven cases; baseline cover work is also after those boundaries. Frozen `src/startup_admission.rs:112-143` releases the cover gate at `StartupUiTurn::yield_now()` and waits for that gate after metadata. The gate is released immediately before returning from the first bootstrap UI closure, so the trace and source establish the original yield/admission boundary, not a separately measured closure-return timestamp. It is inaccurate to claim covers overlap the original completion wait or to claim a measured extra full UI turn after closure return.

Actual UI font registration is after original checked completion in all fourteen records. `font_decoded`, `fonts_prepared`, and `state_prepared` carry worker observation stamps even when written during later UI consumption; they are not UI-consumption timestamps. `lifecycle_complete` is the duration of a lifecycle method call, not a dedicated successful-input-drain timestamp. The backend can run queued control closures before its pending live-input-drain branch (`platform.rs:393-402,443-459`), while suppressing normal timers/touch dispatch there. Accordingly, neither this implementation nor this record proves that every UI consumer must be later than input readiness. This batch contains no new physical touch-readiness acceptance.

`timeline.csv` records the separate font-byte, metadata, cover-pixel, font-registration, application-attach and view boundaries for every case. Stage spans overlap across workers and UI; they must not be added into an end-to-end startup total.

## Critical paths and the two large wins

The following gaps are directly observed. A positive prepared-minus-font-finish value means state arrived just after the UI completed font registration. All times are milliseconds.

| Candidate | Prepared minus font finish | Prepared to attach | First cover attach end to second begin | Route end to Views begin | Ready to SEND | Extra pre-Ready full sequence |
| --- | ---: | ---: | ---: | ---: | ---: | --- |
| candidate-01 | 2.596 | 255.193 | 417.335 | 0.618 | 772.463 | none |
| candidate-02 | 6.161 | 254.690 | 412.275 | 1.082 | 775.954 | none |
| candidate-03 | -20.985 | 22.191 | 1.327 | 806.335 | 509.212 | 4 |
| candidate-04 | -24.806 | 25.554 | 1.568 | 0.386 | 742.532 | none |
| candidate-05 | -4.053 | 4.819 | 1.267 | 0.412 | 736.209 | none |
| candidate-06 | 8.165 | 254.320 | 410.174 | 819.544 | 477.577 | 5 |

Candidate 01/02/06 publish `state_prepared` only 2.596/6.161/8.165 ms after `font_finish`. The UI checks state before those arrivals, then the next state-consumption opportunity follows an intermediate frame. The observed prepared-to-attach gaps are about 255 ms. Candidate 03/04/05 already have prepared state when font registration finishes and attach it immediately afterward. This is an observed scheduling boundary, not proof that metadata itself is slow in 03 or 06.

Candidate 04/05 avoid that missed state opportunity and complete both cover-attachment chunks and the cheap following stages without an intervening replacement before Ready. Their first business frames are sequence 4; baseline business frames are sequence 6. Their SEND wins are 802.031/814.358 ms and Ready wins are 724.821/787.866 ms. In contrast, 01/02/06 have an additional partial frame between cover-attachment chunks, with 410.174-417.335 ms between the first end and second begin. Candidate 03 has no such cover-attachment gap, but hits the full-frame boundary below. This accounts for where the 04/05 benefit is retained and where other candidates lose it; it does not establish a deterministic 800 ms optimization.

Candidate 03 completes Route at 5225.938 ms, yields at `attach_stage_advanced` 5226.293 ms, and begins Views only at 6032.273 ms. Between them, full replacement sequence 4 runs from preceding wait at 5228.700 ms to its record at 6029.736 ms. The non-overlapping components inside that transaction are preceding-marker wait 337.157 ms, Slint render 21.706 ms, fingerprint 46.048 ms, framebuffer write 29.738 ms, SEND 0.332 ms, and checked own-marker wait 365.705 ms. Views then cost about 17 ms and Ready is 6051.432 ms. This directly locates the Ready regression; it is not caused by late worker completion. After Ready, the final business frame has no outstanding preceding-marker wait and SEND follows in 509.212 ms, versus 767.163 ms for its baseline. That 257.951 ms shorter post-Ready path offsets the 215.414 ms Ready regression, leaving a 42.537 ms SEND win.

Candidate 06 has both penalties. The first-to-second cover-attachment gap contains non-full sequence 4, whose preceding-marker wait alone is 348.586 ms. Route then completes at 5570.433 ms and Views starts at 6389.977 ms. Full replacement sequence 5 occupies 5573.179-6388.537 ms: preceding wait 348.932 ms, render 22.264 ms, fingerprint 48.182 ms, framebuffer write 29.623 ms, SEND 0.304 ms, and own-marker wait 365.656 ms. Ready is 6410.300 ms. The post-Ready business path shrinks to 477.577 ms from baseline's 781.029 ms, offsetting only part of the 465.258 ms Ready regression; SEND remains 161.805 ms slower.

The frozen source provides a concrete mechanism consistent with those transactions: `advance_startup()` checks a 4 ms budget between stages (`src/app/startup.rs:264-270`); Route calls `navigate()`, which queues `EnterScreen` before Views (`src/app/mod.rs:357-371`); Views clears the startup stage; the driver clears `booting` and queues another `EnterScreen` only after all stages finish (`src/main.rs:934-958`). A 1 ms continuation yields to rendering (`src/runtime_wakeup.rs:86-95`). The trace proves a complete replacement was actually presented between Route and Views in 03/06 and blocked subsequent UI progress. It does not tag the queued request's producer, record each budget decision, or establish which scheduler, I/O, instrumentation, or notification-order effect made those specific rounds cross the boundary. Describing CPU contention, font decoding, slow covers, cache eviction, or native launch overhead as the proven root cause would exceed the evidence. Performance tracing itself writes synchronously; its possible influence on the 4 ms boundary is unquantified.

## Startup first frame and process front segment

| Metric | Baseline median | Candidate median | Interpretation |
| --- | ---: | ---: | --- |
| Request to process entry | 4088.163 | 4098.798 | Native launch front segment is essentially unchanged |
| Request to startup SEND | 4397.749 | 4397.897 | Visible startup submission timing is essentially unchanged |
| Process entry to Ready | 1798.950 | 1542.523 | Most of the measured median benefit is after process entry |
| Process entry to startup SEND | 307.090 | 302.588 | No large initial render acceleration |

The candidate is admitted only after the first SEND, so it cannot make that already accepted submission faster through preparation overlap. First-frame spans show the original full rewrite and checked completion remain present. Similar first-frame timestamps do not prove optical equality or unchanged physical input readiness.

The initial-frame stage medians below use all six primary launches in each variant. Wall and same-thread CPU times are separate observations; medians of separate stages are not an end-to-end sum.

| Initial-frame stage | Baseline wall | Candidate wall | Baseline thread CPU | Candidate thread CPU |
| --- | ---: | ---: | ---: | ---: |
| slint_render | 59.616 | 57.315 | 37.420 | 35.552 |
| rgb_fingerprint | 48.276 | 50.559 | 47.581 | 49.909 |
| framebuffer_write | 29.807 | 30.748 | 29.633 | 30.615 |
| refresh_submit | 0.309 | 0.296 | 0.226 | 0.203 |
| refresh_complete_wait | 365.661 | 365.576 | 0.065 | 0.091 |

## Post-business resource and pixel observations

All samples follow the common first-business SEND plus 1500 ms gate. The instrumented gate's deadline slips range from 0.335 to 1.701 ms; those are gate-request slips, not exact procfs or framebuffer sample timestamps. Procfs reads occur after the gate and any scheduled framebuffer capture. RSS median rises from 55958 to 56150 KiB, a 192 KiB (0.343%) increase; all six paired RSS differences are positive (88-332 KiB). Sampled `VmHWM` medians are 55974 and 56166 KiB. VmHWM is the kernel-reported high-water value up to that later observation, not a continuously measured startup-only peak or a whole-process-lifetime maximum. The current record does not support a memory reduction or material memory-regression claim.

All fourteen final logical Slint snapshots have equal complete RGB pixels (before framebuffer compensation). The early `content-frame.raw` and scheduled `settled-frame.raw` comparisons cover only baseline-06 versus candidate-06. Each compares every one of 4,612,800 visible framebuffer pixels, without masks, and reports zero differences; padding bytes are outside that visible-pixel claim. Those are framebuffer-memory observations, not panel photographs or optical completion. The two raw framebuffer pair observations must not be generalized into fourteen framebuffer captures. The recorded first Shelf SEND does not establish that all background covers are complete; the Ready resource snapshot can still contain pending jobs.

## Recommendation and limits

1. Retain the one-shot post-SEND font/metadata admission and the existing own-marker, renderer/lifecycle ownership, cover gate, cancellation, and input-drain protections as the implementation candidate. Every recorded launch meets the intended observed admission ordering; no duplicate worker admission appears.
2. Hold stable speed-adoption or completion claims. The current implementation is sensitive to missing a UI continuation opportunity and can issue a redundant complete startup frame after Route but before Views. The observed medians improve, but the improvement is heterogeneous and Ready regresses in two of six pairs.
3. In a future source iteration, defer or coalesce the startup Route `EnterScreen` until the final views and `booting=false` scene can be committed together, while retaining the approved complete-frame replacement and checked completion. Preserve bounded UI work and progress feedback. Consider continuation scheduling only where it avoids unnecessary intermediate presentation; do not remove hardware/input barriers to improve the timing number.
4. Attribute a revised source/ELF and evaluate it as a new independent measurement batch with the same retained warmups, balanced pairs, endpoints, and captures. The current analysis requests or performs no new batch. More observations and explicit stage-budget/refresh-producer instrumentation would be needed to distinguish the remaining notification, scheduling, and trace-overhead hypotheses.

The separate user-assisted sleep/input batch ending `a1067e98` is outside these performance data. Its bounded window ended without received physical-key feedback and its lease was exactly released. Sleep, input, and sleep-continuation acceptance must remain unverified; this ended attempt neither blocks this stored analysis nor supplies a pass. Existing fixture/software checks, this offline-demo physical batch, live-account behavior, and optical/hardware qualification remain separate evidence classes.

## Exact evidence locations

- Frozen aggregate analyzer output: `/root/bilicomics-physical-startup-sequence-20261117-f028b769/analysis.json` (`cases`, `paired_rounds`, `summary`, `median_reductions`, `restoration`, `framebuffer_comparisons`).
- Ended physical-batch report: `/root/bilicomics-physical-startup-sequence-20261117-f028b769/report.json` (`cases`, `completed_cases`, `variants`, `scope`, restoration and release booleans).
- Raw case members: `/root/bilicomics-physical-startup-sequence-20261117-f028b769/outputs.tar.gz::out/<case>/request.json`, `performance.jsonl`, `lifecycle.jsonl`, `memory-at-content.txt`, `post-business-gate.json`, `shell.tsv`, `final.json` and `recovery.json`.
- Visible framebuffer capture pairs: `/root/bilicomics-physical-startup-sequence-20261117-f028b769/outputs.tar.gz::out/{baseline-06,candidate-06}/{content-frame.raw,settled-frame.raw}`; their observation requests/completions, deadlines and liveness records are adjacent case members.
- First-frame and UI chronology: native/frame `data.stages.*.*monotonic_ns`, startup worker `data.observed.monotonic_ns`, and non-observed UI event envelope stamps in those performance members. Exact event source lines and absolute stamps are in the derived JSON; `timeline.csv` and `frame-spans.csv` expose the complete tabular data.
- Frozen implementation: `/root/bilicomics-startup-sequence-20261117-01a1d11a-r1/candidate-r1/app`; relevant source paths, SHA-256 hashes and line references are in `item1-analysis.json/source_identity`.
- Software/build identity remains the separately published `docs/verification/startup-sequence-20261117/` bundle. Its existence does not substitute for physical timing or sleep/input acceptance.

All rounded narrative/table times are derived from the exact stored monotonic stamps. JSON and CSV retain full numeric precision.
