# Where the ordinary desktop launch spent time

This is a read-only projection of the closed `oKSvppcT` [ordinary direct-PNG trial](PROTOCOL.md), not a new timing run. It uses `performance.jsonl`, `lifecycle.jsonl` and the [per-case X11 observations](desktop-observations.json). A and B have six measured launches each; `a00` and `b00` are excluded. Both run the same synthetic offline Scribe-sized desktop Shelf with ten visible covers, not `--demo`, a real account or Kindle hardware. No original session, paths or images are needed below.

The origin is the process-entry observation (`CLOCK_MONOTONIC`), corroborated by the same PID's lifecycle trace. The first state-worker queue and begin are actual `state_worker_queued` and `state_worker_begin` events, not the earlier `workers_started` announcement. The producer stamp comes from `state_prepared.data.observed.monotonic_ns`, not its UI-side record time. `ui_ready` is software readiness. Each table cell is a median of **within-case** intervals, in milliseconds. Bracketed endpoints are medians of the respective lower and upper bounds, not point estimates or confidence intervals. Do not add these overlapping rows or subtract their marginal medians.

| Interval | A | B |
| --- | ---: | ---: |
| Entry to `window_shown` | 11.761 | 12.135 |
| Entry to first observed feedback (X11 bracket) | [0, 28.740] | [0, 28.533] |
| `window_shown` to `driver_first_entry` | 80.726 | 80.436 |
| Entry to state worker queued | 92.828 | 93.107 |
| End of first positive feedback sample to state worker queued | 64.195 | 64.586 |
| First UI driver entry to state worker queued | 0.258 | 0.385 |
| State worker queued to actual begin | 0.212 | 0.235 |
| State worker begin to `state-covers` begin | 9.039 | 9.307 |
| `state-cache-maintenance` measured wall time (within the preceding interval) | 7.512 | 7.695 |
| `state-catalog` measured wall time | 0.415 | 0.444 |
| `state-covers` measured wall time | 3.808 | 4.030 |
| State worker begin to prepared result produced | 13.122 | 13.444 |
| Producer to UI record | 0.047 | 0.080 |
| UI record to `ui_ready` | 3.622 | 2.887 |
| Entry to `ui_ready` | 110.744 | 109.411 |
| `ui_ready` to first exact complete Shelf (X11 bracket) | [14.524, 24.159] | [17.053, 26.400] |

The first positive feedback sample ended **62.615–66.170 ms before state-worker queueing in every launch**. The pre-feedback lower bound is zero because the last negative sample started at or before process entry, not because feedback appeared immediately. The complete-Shelf bracket uses the start of the last nonmatching sample and the end of the first matching full-window read, each relative to that case's `ui_ready`; it includes sampling uncertainty and is not pure rendering cost.

**Next measurement:** discriminate a render-notification wake from the desktop maintenance-timer fallback at first UI-driver entry. Feedback was already present, and once the first driver entered it queued state work in under 0.5 ms in all twelve cases. The observed `window_shown` to driver interval (79.895–80.880 ms across both arms) is consistent with the 80 ms timer but the trace does not identify which wake fired or what an earlier safe wake would save. Preserve actual changing feedback and the first UI yield. The Kindle backend already has a separate accepted-full-SEND admission; its timing cannot be inferred here. For a common state-worker follow-up, first break down the `state-cache-maintenance` span (6.050–8.915 ms across both arms); its measured cost exceeds cover decode in this fixture, but neither an internal scan nor recoverable latency has been demonstrated.

## Control clock anchors

The entry column is absolute monotonic nanoseconds. All subsequent columns are `event - entry` in nanoseconds (including cover *marker* begin/end, distinct from the enclosed operation's own `wall_us`). These additional stamps complement the [published per-case endpoints for both arms](paired-timings.md); they are not sample timestamps for other processes or devices.

| Case / PID | Entry | Window shown | First UI driver | State queued | State began | Covers begin | Covers end | Software ready |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| a01 / 134 | 63453502961708 | 14254540 | 94837156 | 95089530 | 95343023 | 104438113 | 108347275 | 112009673 |
| a02 / 245 | 63457471394693 | 12207032 | 93087251 | 93444612 | 93877542 | 103241815 | 107095878 | 111582554 |
| a03 / 283 | 63458771335849 | 11163849 | 91689396 | 91885541 | 91984318 | 99350333 | 102844164 | 106381526 |
| a04 / 395 | 63462729950542 | 11300906 | 92011388 | 92219903 | 92528806 | 100788337 | 104300099 | 107891788 |
| a05 / 432 | 63464055303797 | 11377293 | 92119035 | 92381669 | 92552414 | 101535378 | 105958536 | 110440033 |
| a06 / 544 | 63468000977037 | 12145653 | 92894835 | 93274226 | 93391933 | 103577506 | 107349261 | 111047507 |
