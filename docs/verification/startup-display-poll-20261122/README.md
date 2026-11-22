# Is display startup waiting for output or for child exit?

**Decision: stop tuning child-exit detection for startup. Leave the daily build unchanged.** In all three ordinary Scribe launches, most time in `poll` was spent **before the helper observed EOF**: **90.1%, 81.5% and 84.8%** of the poll time in the respective visits. Time after EOF was **20.195, 42.504 and 31.486 ms**. Not one reached the predeclared 100 ms investigation threshold, even without subtracting necessary child runtime. These are measured waits, not savings.

## Physical results

One 2022 Scribe on firmware 5.19.5; the normal native Library card; the same isolated, populated offline Shelf and same instrumented binary in each visit. The first visit is included. All three naturally used five property reads (automatic path); there was no fixed-path physical sample. All three reached the normal Shelf, exited naturally, restored the installed launcher and native navigation, and released their own lease after closing connections. The last release receipt was written at **07:52:28 Asia/Shanghai on November 22**. No manual tap, video or optical observation was needed.

| Visit | Helper wall | Total `poll` | Before observed EOF | After observed EOF | Before EOF / total poll wall |
| --- | ---: | ---: | ---: | ---: | ---: |
| 1 | 294.616 ms | 24 calls / 203.602 ms | 22 / 183.406 ms | 2 / 20.195 ms | 90.1% |
| 2 | 308.328 ms | 27 calls / 229.615 ms | 23 / 187.111 ms | 4 / 42.504 ms | 81.5% |
| 3 | 311.513 ms | 27 calls / 207.601 ms | 24 / 176.115 ms | 3 / 31.486 ms | 84.8% |

At the per-visit median: **207.601 ms** in `poll`, **183.406 ms** before EOF, **31.486 ms** after EOF, and **84.8%** of poll time before EOF. Medians come from different visits and are not additive; the integer counts and nanoseconds sum exactly **within every visit**. The pipe-open interval can include waiting for data, pipe closure, or scheduling; it is not a direct measurement of firmware CPU. The post-EOF interval is not proof the process had already exited. The result does not justify shortening the 10 ms poll slice, busy-waiting, skipping the success status or reducing permission checks.

| Visit | Card release → complete Shelf software render | → accepted SEND | → same-marker driver completion |
| --- | ---: | ---: | ---: |
| 1 | 6.676–6.678 s | 6.721–6.723 s | 7.086–7.088 s |
| 2 | 6.371–6.373 s | 6.410–6.412 s | 6.775–6.777 s |
| 3 (also the median for each endpoint) | **6.401–6.403 s** | 6.470–6.473 s | 6.833–6.835 s |

Bounds enclose the button-release interval rounded outwards to milliseconds. The primary endpoint is the populated Shelf layout, entries and navigation drawn by software, not complete covers, interactivity or optical settling. The [public durations](physical-result.json) keep the exact bounds. Do not subtract a previous run's median from this one: this batch has no candidate arm.

## What to try instead

Run **one offline comparison of when cached covers are attached**, not another experiment on copying pixels or the launcher. Today the startup worker waits for cached covers before delivering its prepared state; the UI also advances through `AttachStage::Covers` before the normal Shelf route. Compare that sequence with delivering the same metadata first and filling placeholders progressively after the normal Shelf's first submission. The [pre-ready investigation](../../STARTUP_PRE_READY_20261121.md) identifies this ordering; it does not establish its benefit. Keep the activity frame, native display barriers, and existing cache and error behavior. On both Scribe and PW6 fixture sizes check identical titles, order, navigation, empty state and eventual covers, and count duplicate decoding, requests and refreshes. Only a clean functional result and a promising first-render difference would justify a separate alternating six-pair device A/B (at least five wins and 100 ms paired median earlier). This change was **not** implemented or deployed in the poll study.

## Why split the wait?

The [first three Scribe measurements](../startup-display-sampling-20261122/README.md) found a median 213.896 ms in `poll` within a 300.502 ms display-capture helper. They did not record the state of the output pipe. A [controlled Linux fixture](../startup-display-sampling-20261122/poll_state/README.md) distinguished a deliberately late output from a deliberately late exit. Neither study determined what the five real getters were waiting for, nor how much time a product change could save.

## Decision rule before device access

- Time the stock `poll` exactly once at each existing call. Classify that same interval from the descriptor passed at entry: **stdout open** (EOF not yet observed) or **after EOF** (the read end has reported EOF but a successful child exit was not yet confirmed). Each child gets the exact same elapsed nanoseconds as the parent for that call. Per visit, both time and count must add back to the parent. A fraction has no value when its denominator is zero.
- Use three independent ordinary-card visits with one instrumented implementation; no warmup exclusion or substitute run. Report the two states for each visit. If all three follow the same query branch and the same state is the strict majority of poll time in each, call that state predominant **in this batch**. Otherwise report the individual results, not a universal winner. A majority is not an improvement or a prerequisite for running the application on a particular model.
- Compare the absolute time with the project's existing **100 ms paired first-Shelf improvement** bar before investing in a change. If no visit spends 100 ms after EOF, do not pursue an exit-wait change for startup on this evidence. If the after-EOF interval is repeatedly substantial, first separate **the child still running** from **late detection of a finished child**; neither EOF nor a `poll` timeout proves exit. A pipe-open majority does not justify shorter poll slices, parallelizing ordered reads, dropping successful-exit checks, or caching authorization.
- The primary measure is ordinary native Library-card release to **the first complete normal Shelf software render**, including its layout, entries or appropriate empty state, and navigation. Covers may be placeholders. Report accepted SEND and matching driver completion separately. Do not substitute a tap, optical measurement, host duration, or differences between single-arm studies for this endpoint. A product claim requires a separate balanced A/B.

This study changes only the instrumentation in a private build. The production capture, command timeouts, periodic permission checks, error semantics and process cleanup stay in force. The shared Kindle is acquired only for a prepared visit and returned before any offline analysis or changes.

See the [frozen physical protocol](PHYSICAL_PLAN.md), [diagnostic overlay](OVERLAY.md), [full build and ARM/QEMU checks](BUILD.md), and [full host product controls](host-product-qualification.json). The [collector](collect_display.py) accepts only closed, bound evidence for timing. An [independent replayer](archive_poll.py) recomputed the partitions, fractions and medians. Only the two sanitized payloads are retained under `artifacts/verification/startup-display-poll-20261122/` outside Git; the repository summary contains no process identifiers or absolute device clocks.
