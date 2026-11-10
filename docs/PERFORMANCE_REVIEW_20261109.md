# Startup, Page-Turn, and Resource Review

Reviewed on 2026-11-09 against application version `0.1.16`, source commit
`3520ef8`. This is an investigation, not an implementation or a new acceptance
decision. Application behavior, timeout limits, refresh policy, and durability
settings were not changed.

## Evidence scope

- Current application, launcher, and native backend sources were inspected.
- Existing physical evidence belongs to its original candidate, private test
  launcher, Scribe2022 / 5.19.5, and offline demonstration workload.
- New observations run only on `ssh test-env`, with isolated fictional data and
  network namespaces. No local verification, account access, or Kindle
  operation is part of this review.
- The reused R31 desktop executable has SHA-256
  `5ad974476774b5378a68a00d482a0deff34480e568ebe44267c103f0c964eefb`.
  Its build source manifest is
  `1f38786c269e196ea9458cecc60c2c1f98bcd211980c7cd46d2d7395bbd740b5`.
  A remote comparison of all 432 current source inputs found only six
  differences, all in the optional Kindle backend. The desktop application
  inputs match. This does not transfer desktop results to that newer backend.
  The [source comparison](../artifacts/verification/performance-review-20261109/source-comparison.json)
  records each difference.
- Desktop timings use the existing development build. They describe controlled
  software workloads, not ARM release speed, flash performance, touch latency,
  or physical e-paper completion.

The [review evidence index](../artifacts/verification/performance-review-20261109/review-summary.json)
binds all four new observations to their results and records the final unchanged
source and executable checks. The experiments were serialized within this review;
the shared host was not reserved and unrelated host load was not controlled.

## Startup: the original limit has not been re-established

The original two-second limit applies to the bounded native-state read group,
not the entire launch. Production still uses that limit in
[launch.sh](../packaging/kindle/launch.sh), lines 212 and 244. The private physical
test copy increased it to five seconds. Passing that copy is not evidence that
the original budget passes.

| Existing observation | Time | Interpretation |
| --- | ---: | --- |
| R20 native sample, older 0.1.13 candidate | 2.53 s | Sample interval is 26.5% longer than the original nominal budget; protected child duration is not isolated |
| R33 `startup_sample_begin` to `startup_sample_ready` | 3.25 s | Phase duration is 62.5% longer than the original nominal budget, under the private physical test wrapper |
| R33 sample ready to native takeover start | 1.14 s | Additional startup work outside the sample interval |
| R33 native takeover start to done | 0.10 s | Separate takeover interval |
| Fixed sleep before sampling in current launcher | 2.00 s | Occurs before the first timestamp above |

The R33 trace covers 4.49 seconds from sample begin through takeover. The fixed
sleep and application initialization are outside that interval. These values
must not be presented as complete launch-to-readable timing. The test wrapper
also adds ownership and executable-identity checks, so this is not a measurement
of an unwrapped production launch. The sample interval includes supervision,
cleanup, and parsing; it does not isolate the watchdog-protected child duration.
No current successful physical run under the original production limit is
established by these records. See the retained
[R33 trace](../artifacts/verification/live-portrait-20261109/physical-r33/launcher-timing.tsv)
and the historical [orientation research](KINDLE_ORIENTATION_RESEARCH.md).

### Current startup work to reduce

1. **Consolidate native sampling and replace the unconditional wait with observed
   readiness.** The launcher runs several bounded LIPC reads, two framebuffer
   helper invocations, exact-response parsing, and publication before the App
   starts. Measure each substage, then reduce process and pipe overhead within
   one owned read group. Keep repeated-consistency checks, process birth and
   generation binding, output limits, and native recovery. The physical wrapper
   already avoids rehashing the entire executable for every helper; that old
   problem must not be reported as still present.
2. **Remove full-library validation from the first interactive screen's critical
   path.** `App::prepare` calls `CatalogState::prepare`, which loads all known
   comics and scans local chapters and receipts. Even an already validated
   receipt reads and hashes its complete encoded file. Prepare the selected
   route and reader first; perform the remaining inventory incrementally in a
   bounded worker. Until verified, represent availability as pending. Do not
   label corrupt or incomplete files as completed downloads.
3. **Load fonts and route-specific assets progressively.** The startup worker
   decompresses all five bundled CJK faces before `App::prepare`. UI attachment
   waits for all registrations, while registration can overlap state
   preparation. Preparation also warms pinyin and prepares covers even
   when resuming directly into a comic. Prioritize the required face, current
   page, and visible assets; retain the existing visual design and font licenses.
4. **Replace fixed timer spacing with completion-driven scheduling.** Startup
   advances on an 80 ms timer, one font or attachment stage at a time. Five font
   registrations, attachment, and the six attachment stages add about a second
   of scheduling opportunity even when individual work is short. Use a bounded
   per-turn work budget and explicit wakeups while yielding for the real first
   frame and the existing 400 ms activity indicator.

Source locations: [main.rs](../src/main.rs), lines 532-534 and 591-705;
[fonts.rs](../src/fonts.rs), lines 35-82;
[startup.rs](../src/app/startup.rs), lines 47-84 and 259-276;
[catalog.rs](../src/app/catalog.rs), lines 184-218, 234-269, and 1316-1333;
[downloads.rs](../src/downloads.rs), lines 2025-2045 and 2071-2077.

The existing `startup_frames.py` intentionally inserts a two-second worker
delay. Its overall duration includes Xvfb, captures, and a post-ready interval;
it tests feedback and lifecycle correctness, not production startup speed.
Existing `reader-ready` records have no timestamp and precede framebuffer
rendering. Neither is an end-to-end startup benchmark.

## Page turns: avoid intermediate work before adding more caching

| Priority | Current finding | Proposed change and required protection |
| --- | --- | --- |
| P1 | Every render request sets `loading=true`, hiding the existing artwork and presenting a full-screen loading view, even for cached pages. If submitted, this creates an extra blank-frame update. | Preserve the displayed frame while preparing the next frame. Separate first load, unavailable content, and navigation in progress. Keep controls responsive and show bounded feedback for slow operations. |
| P1 | Download events for unrelated chapters can invalidate a currently running reader render. `reader_page_ready` increments the version while loading; the completed result is then discarded and rendered again. | Match events to the reader generation, revision, and missing visible source dependencies. Do not invalidate useful work because another chapter or a prefetched page finished. |
| P1/P2 | A page turn synchronously saves its anchor on the UI thread, then saves again after rendering. SQLite uses `synchronous=FULL` and a five-second busy timeout. | Move ordered progress persistence to a bounded worker and coalesce redundant writes. Preserve the latest in-memory anchor, sleep/quit checkpoints, retry behavior, and transaction durability. Do not merely weaken SQLite settings. |
| P2 | `LocalPageResolver` opens SQLite and verifies the whole encoded file while holding the publication lock. The reader subsequently reads and hashes that file again to identify the gray-source cache entry. | Pass verified bytes and an exact content proof through one render operation; shorten lock-held I/O and reuse connections safely. Retain corruption detection and publication/eviction race protection. |
| P2 | Prefetch acquires encoded files but does not prepare the next display frame. The first visit still decodes, resizes, dithers, and creates RGBA on demand. | After the previous work is removed, evaluate one low-priority next-frame slot with an explicit byte budget and generation/layout/content invalidation. Do not multiply full-size frames by the encoded prefetch count. |
| P2 | Worker results enter the UI through the 80 ms timer; publishing a job result does not wake Slint. | Coalesce event-loop wakeups for ready results. Retain the timer for maintenance. This removes up to one idle timer interval, not the decode or panel time. |
| P3 | Installing an RGBA frame copies it into `SharedPixelBuffer`; the native renderer also composes RGB and converts it to grayscale. | Measure copy and conversion stages before changing the buffer interface. A 1860 x 2480 RGBA frame alone is 18,451,200 bytes, about 17.6 MiB. |

Source locations: [reading.rs](../src/app/reading.rs), lines 627-629, 765-818,
824-886, 1062-1070, and 1506-1575;
[reader.slint](../ui/reader.slint), lines 362 and 412-425;
[download_ui.rs](../src/app/download_ui.rs), lines 285-307;
[jobs.rs](../src/app/jobs.rs), lines 276-301;
[reader.rs](../src/reader.rs), lines 618-634 and 702-716;
[store.rs](../src/store.rs), lines 128-130 and 348-360.

Normal smart-mode single-page turns already request AUTO partial refresh.
First frames, spreads, transitions, and dark-fraction threshold crossings can
request a full refresh. Ordinary paging does not explicitly wait for panel
completion in the backend. Removing GC16 globally or changing the accepted
orientation sensitivity is therefore not a justified first optimization.

Catalog/Reader view refreshes can also trigger a full local inventory scan when
it is dirty or the last check is at least 30 seconds old. This is conditional on
view updates, not an unconditional 30-second background poll. It can overlap
reading and should share the incremental inventory work proposed for startup.

## Resources: separate defects from bounded retention

Two current ownership/cache paths and one disk-capacity risk require attention:

- **Failed reader close can leave a hidden live reader.** `close_reader` first
  saves the anchor and releases the reader with `?`, before clearing state and
  UI references. `navigate` ignores its error and still changes the route.
  A storage error can therefore bypass teardown, leaving the page, reader state,
  and ownership references alive offscreen. Either complete teardown while
  retaining a separate retryable checkpoint, or reject navigation visibly.
  Silently hiding the failed close is the defect. This is bounded retention of
  one reader, not evidence of linear leakage on every turn.
- **Catalog metadata has no effective retention limit.** Unique searches add
  `search:<digest>` scopes and comics to SQLite, and `merge_comics` accumulates
  known comics in RAM. The visible search history is truncated to 20 entries,
  but old scopes and unreferenced comics are not evicted. Subsequent startup
  loads all of them. Add bounded retention for transient search/category data
  and remove only unreferenced metadata; preserve the bookshelf, history,
  downloads, anchors, active reader, and transaction records.
- **Production logs append without rotation.** The launcher appends
  `launcher.log` and `application.log` across launches without a byte or
  retained-file limit. Add bounded rotation. This is a P3 storage-capacity risk
  established by code, not a measured log-growth rate or an in-memory leak.

Source locations: [reading.rs](../src/app/reading.rs), lines 374-400;
[mod.rs](../src/app/mod.rs), line 310;
[auth.rs](../src/app/auth.rs), lines 286-294;
[catalog.rs](../src/app/catalog.rs), lines 242, 675, 894-917, and 1375-1379;
[store.rs](../src/store.rs), lines 236-245 and 258-269;
[launch.sh](../packaging/kindle/launch.sh), lines 63-65 and 668.

The earlier 0.1.6 protections remain present: explicit normal reader image
release, bounded/coalesced job delivery, request retirement, cover miss
suppression, byte-bounded gray and cover caches, and native descriptor/queue
cleanup. Fonts, shared HTTP pools, allocator high-water memory, and bounded
caches remaining resident are not by themselves leaks. The
[earlier implementation report](RESOURCE_AND_PERFORMANCE_IMPLEMENTATION.md)
retains its own candidate and test scope.

## New remote observations

### Normal reader release

Two receipt-backed fictional cases, with and without a prepared cover, each
completed 20 close/reopen cycles and reached 21 real ready reader generations.
All 40 close observations cleared the active reader, all three image references,
chapter model, and thumbnail model.

| Case | Ready RSS median before / after | All-phase RSS peak | Stable FD count before / after | Peak threads |
| --- | ---: | ---: | ---: | ---: |
| Missing cover | 187.28 / 192.57 MiB | 224.74 MiB | 31 / 32 | 11 |
| Cached cover | 190.00 / 193.84 MiB | 226.01 MiB | 31 / 32 | 11 |

The gray-source cache retained one 4,012,000-byte image within its 32 MiB budget.
This does not require the allocator to return RSS to its original value after a
close. The modest RSS increase and one additional FD are reported rather than
attributed to a specific owner. This bounded same-page workload does not prove
the absence of slow leaks, unique-page churn, or failure-path retention.
The original App, Xvfb, and watchdog process identities were absent at cleanup.
See the [resource results](../artifacts/verification/performance-review-20261109/resources/results.json).

### Startup inventory scale

The two cases used one versus 100 fictional chapters, each with a complete,
already validated receipt. The same fictional encoded page object was reused,
so this measures repeated inventory reads and hashing rather than 100 cold
storage objects. Both cases had a real application warmup to upgrade receipt
proofs, followed by two interleaved cold-process observations. The operating
system page cache was not cleared. Every measured ready route was `shelf`.

| Observation | 1 receipt | 100 receipts |
| --- | ---: | ---: |
| Spawn to externally observed ready, two runs | 1561.13 / 1649.87 ms | 1560.88 / 1731.52 ms |
| Mean process CPU through ready | 0.70 s | 0.93 s |
| Mean `/proc` `rchar` through ready | 1,110,645 bytes | 11,448,902 bytes |
| Physical storage `read_bytes` | 0 | 0 |

The workload increased observed read traffic by about 10.3 times. Mean total
ready time differed by only 40.70 ms in two samples per case; this is not a
statistically established slowdown or a Kindle estimate. The controlled result
supports the repeated-read concern without establishing its device-level cost.
All receipts retained validation version 1 after the run. All six App processes,
Xvfb, and the watchdog exited and their original identities were absent.
See the [startup scale results](../artifacts/verification/performance-review-20261109/startup-scale/results.json).

### Observed page turns

Five independent processes each displayed fictional source artwork 18, turned
forward to artwork 19, and returned to artwork 18 through actual XTest pointer
input. These were non-demo, receipt-backed offline pages. The desktop preview
rendered 930 x 1240 images with dithering enabled.

| Direction and source-cache state | Samples | Release to externally observed UI-ready median | Observed range |
| --- | ---: | ---: | ---: |
| Next page, first gray-source use in the process | 5 | 1085.1 ms | 1065.4-1089.6 ms |
| Previous page, already decoded gray source | 5 | 848.6 ms | 823.7-857.8 ms |

All ten turns visibly replaced the old comic with the full-screen loading view.
Each direction's five intermediate PNGs had identical digests. Both
representative images were directly inspected, including the
[already-viewed page](../artifacts/verification/performance-review-20261109/page-turn/run-1/already-viewed-gray-source-intermediate.png).
This verifies the avoidable intermediate frame in the controlled desktop path.

The endpoint is a UI-ready trace, before final framebuffer/panel completion.
This development build, X11 sampling overhead, warm operating-system cache,
small sample size, and different artwork in the two directions prevent a
hardware latency or isolated cache-speedup claim. Five observations are also
insufficient for a reliable P95. No product optimization was applied in this run.

Post-turn FD counts stayed at 32, peak threads were 11, and sampled App RSS peaked
at 232.23 MiB. Ready RSS was approximately 198.23-200.07 MiB, unchanged between
the two measured ready states in each process. The gray cache increased from
4,012,000 to 8,024,000 bytes for the second source and remained within its 32 MiB
budget; ready pending-job counts were zero. These two-turn runs are not a
long-session leak test. All five Apps, Xvfb, and the watchdog exited, and their
original process identities were absent. See the
[page-turn observations](../artifacts/verification/performance-review-20261109/page-turn/observation-summary.json)
and [raw results](../artifacts/verification/performance-review-20261109/page-turn/result.json).

### Failed-close reproduction

After a real receipt-backed reader became ready, a controlled SQLite trigger
rejected anchor writes. The `auth/net-downloads` intent moved the route to
`downloads`; one failed close-time save was observed and no reader-close event
occurred. A subsequent controlled lifecycle snapshot still contained the same
reader generation 8, its 618 x 824 image, and its chapter model on that route.

After removing the storage fault, an explicit close produced
`reader_active=false`, zero image dimensions, and empty chapter/thumbnail
models. The ordinary bounded gray cache remained. This causal comparison
confirms the failed-close ownership defect; it is not a product fix or a
successful post-fix test. The App exited 0, and the owned App, Xvfb, and watchdog
identities were absent. See the
[fault reproduction](../artifacts/verification/performance-review-20261109/fault-close/results.json).

## Implementation and verification order

1. Fix failed-close handling and dependency-specific render invalidation; keep
   the old frame visible while paging. These have concrete correctness or wasted
   work triggers and do not require a new rendering architecture.
2. Add monotonic stage timing, then consolidate launcher sampling and remove
   fixed startup waits only where an explicit readiness condition replaces them.
   Re-establish the original two-second native-read budget on the real device.
3. Move full inventory work off the startup/interaction path, share per-operation
   content validation, and queue progress writes without weakening durability.
4. Introduce completion-driven UI delivery and progressive startup assets.
   Evaluate a one-frame lookahead only after measuring the remaining cost.
5. Apply metadata retention limits and repeat representative long-session and
   failure-path observations.

Keep launch-to-first-visible-feedback, launch-to-interactive, and
launch-to-readable as separate measurements. For paging, record touch release,
dispatch, worker start, validation, decode, transform, enqueue, UI installation,
framebuffer submission, and physical completion separately. Compare the same
candidate, data, orientation, page geometry, refresh waveform, and concurrent
download workload; report P50/P95 and sample counts without treating a desktop
ready event as a panel timing.

Remote coverage should include receipt-backed cached pages as well as demo,
first/repeated page visits, panel/strip/spread modes, chapter boundaries,
unrelated download completion, database failure, and interrupted workers.
Physical timing remains pending a fresh exclusive device handoff and lease;
this review did not reacquire the Kindle.
