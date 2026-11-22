# What runs during startup display sampling?

**Three Scribe launches identify command waiting, not process creation or parsing, as the largest measured cost inside the display helper.** Its median was **300.502 ms**; it spent **213.896 ms** in `poll` across the five getters. The median from ordinary Library-card release to complete Shelf rendering was **6.430–6.432 s**. All three visits used the same instrumented build. There is no control arm and **no demonstrated startup speedup**. Keep the daily installation unchanged.

## What happened on the Scribe

One Scribe (2022, firmware 5.19.5), the normal native Library card and an isolated populated offline Shelf. All three launches naturally selected the automatic path (five property reads, two hardware metadata observations); none used the fixed path. Each completed the normal Shelf layout, entries and navigation. Covers could be placeholders. The primary endpoint is renderer return; accepted SEND and driver completion refer to the same frame, not optical visibility or clickability. The table rounds card-release brackets outwards to milliseconds. The [public numeric summary](physical-result.json) preserves the exact intervals.

| Visit | Card release → complete Shelf render | → accepted SEND | → same-marker driver completion | Helper wall | Inside `poll` |
| --- | ---: | ---: | ---: | ---: | ---: |
| 1 | 6.221–6.223 s | 6.253–6.255 s | 6.618–6.620 s | 302.295 ms | 219.930 ms |
| 2 | 6.473–6.475 s | 6.536–6.538 s | 6.902–6.904 s | 300.502 ms | 213.896 ms |
| 3 | 6.430–6.432 s | 6.462–6.464 s | 6.828–6.830 s | 296.626 ms | 199.006 ms |

| Inside a successful capture (ms unless specified) | Visit 1 | Visit 2 | Visit 3 | Median |
| --- | ---: | ---: | ---: | ---: |
| Five command calls, inclusive | 248.052 | 250.050 | 234.193 | **248.052** |
| `poll` calls (count) | 27 | 27 | 24 | 27 |
| `poll` wall time | 219.930 | 213.896 | 199.006 | **213.896** |
| Child process creation | 6.882 | 8.388 | 11.740 | **8.388** |
| Initial permit acquisition | 15.842 | 14.180 | 16.556 | **15.842** |
| 31 subsequent checks, each counted once | 49.356 | 53.154 | 57.174 | **53.154** |
| Acquisition plus subsequent checks | 65.198 | 67.335 | 73.730 | **67.335** |
| Both metadata observations, inclusive | 10.023 | 10.800 | 12.101 | **10.800** |

Across these visits `poll` accounted for **67.1–72.8%** of the helper's elapsed time. It includes time waiting for output or for a child to exit, as well as scheduling. It is not LIPC CPU time, a fixed delay per poll, or time we know how to remove. Each visit made 288 process-identity reader calls; these were counted, not individually timed. The automatic path's 31 post-acquisition checks include the checks nested inside the commands and the metadata calls. Do not add the table rows. After subtracting only the direct children of each parent **within each visit**, median unexplained wall times were **1.817 ms** for the helper, **3.426 ms** for its command calls, and **3.252 ms** for the two metadata observations. These are separate accounts, not measures of driver time.

All three apps exited naturally; each visit restored the installed launcher and native navigation, closed the connection and released its own lease. The final release receipt was written at **07:13:23 Asia/Shanghai on November 22**. No recording or manual tap was needed. The helper's start and finish lie on the same device clock between that visit's card release and UI trace header. The launcher's own `go` and `ready` were **not** recorded in these three visits. Do not subtract 300.502 ms from the historical 410 ms to infer launcher overhead or compare this single-arm median against a previous batch as a speedup.

## Decision

- **Keep** the live display permission checks, power/holding stability reads, metadata observations, two-second observer settle, command deadlines and process-group cleanup. An earlier check can see a permission withdrawal that a later one misses. The measured 53.154 ms of rechecks is not redundant work.
- **Do not deploy** the `/proc` parsing prototype or change the 10 ms poll slice on this evidence. Creating the five getter processes accounted for only 8.388 ms at the median. A shorter slice would not necessarily make the firmware return data earlier.
- **Separate waiting for output from waiting for child exit.** The [follow-up host fixture](poll_state/README.md) now distinguishes them in the real command loop. Delaying output by 120 ms put 121.221 ms in pipe-open polls. Closing stdout before a 120 ms delayed exit put 120.973 ms in post-EOF polls. Both parsed the same result; both implementations still rejected a nonzero exit, timeout and revoked fixture permission. These are deliberately injected waits, not estimates for the Scribe. The next device measurement should add only this two-way split. If waiting is mostly before EOF, do not shorten the poll slice on that evidence. If substantial time follows EOF, distinguish necessary child runtime from exit-detection latency before proposing a change. Preserve `try_wait`, the deadline and revocation; only a separate balanced, ordinary-card-to-Shelf A/B could establish a product gain.

## Prior evidence

| Evidence | Measurement | Interpretation |
| --- | --- | --- |
| [Six archived Scribe launches](DEVICE_REPLAY.md) | Display sampling: 480 ms median; permission grant to parent observing success: 410 ms median | A launcher interval, not a timer for LIPC, authorization or Rust. |
| [Stock transaction on Linux](HOST_RESULTS.md) | 68.289 ms median on the automatic Scribe fixture; 68.235 ms on the fixed PW6 fixture | Identical A/A implementations, real process/permission checks, simulated hardware. |
| [Separate Linux diagnostic visits](HOST_RESULTS.md#inside-the-shell-transaction) | 31 rechecks took 3.401 ms on the automatic path; 23 took 2.552 ms on the fixed path, at the within-visit sum median | Rechecks may include more work on a real device. No deduction from the historical 410 ms is valid. |
| [Parsing after the read](stat_parse/README.md) | An iterator saved 113.369 ns per record at the paired median, 12/12 host pairs | At the host fixture's call counts this is about 32.65/24.49 µs (automatic/fixed), not a measured launch improvement. |

## What the existing evidence can establish

- In [six archived Scribe starts](DEVICE_REPLAY.md), the launcher took a median **480 ms** from starting the sample to accepting its display state. The median from granting permission to observing the helper's success was **410 ms** (350–440 ms). The other work took **60 ms** at the within-visit median. The launcher used a 10 ms-resolution clock. It did not record the durations of individual getters or permission checks. The protected two-second power-observer settle was **before** this interval.
- The helper must keep validating a *live* permission. Launch generation, the processes allowed to read, and the power/rotation context can change while the command is running, or between two adjacent checks. A transient withdrawal could be detected by one sample but not the other. The initial, per-command, in-flight and final checks therefore remain intact; the slow wake path also uses this code. No catch-all time-based permission cache, extra retry, shortened timeout, or removal of either stability sample is part of the study.
- The only implementation compared in A/B exchanges a vector for an iterator **after the same process record has already been read**. This [isolated Linux parsing experiment](stat_parse/README.md) preserves both the input and error handling. It is not in the application and is not a device startup result.

## Reproduction boundaries

The [host protocol](HOST_PLAN.md) runs real shell and helper components with controlled responses. It is separate from the [three-visit physical protocol](PHYSICAL_PLAN.md), which uses one instrumented implementation, the normal card, and an isolated populated Shelf on one Scribe. The [source overlay](stage_overlay.py) changes only the private diagnostic build. It counts existing operations and emits a bounded receipt after a successful startup capture; it never sends a second card click or changes the accepted display record. Bucket timings are elapsed times, not CPU or time saved. Nested timings, medians from different runs and observations made on different clocks must not be added or subtracted as though they formed one trace.

In a separate [full-product qualification](host-product-qualification.json), the actual Linux Kindle-feature executable produced a valid record on both simulated display profiles (five or three property reads, two hardware metadata observations). With the diagnostic disabled, capture still succeeded without a record. With permission withdrawn or a getter delayed past the deadline, capture was refused and its process group was reaped. These five cases exercise the code and collector, not the Kindle's display or the speed of its ordinary card.
