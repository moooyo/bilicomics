# What still precedes the first Shelf render

The November 26 launcher A/B changed where the sleep-cover query runs, not
application code, cover policy, fonts, or renderer. This replay treats its
**12 measured offline demo launches** as observations of the current app-side
path, not as 12 independent comparisons of that path. The two warmups are
excluded. It does not launch a new process on Kindle, change the product, or
measure optical visibility.

## Observed in the November 26 series

| Interval | Median | Minimum–maximum | What the clocks do not establish |
| --- | ---: | ---: | --- |
| Rust `main` → first normal Shelf renderer return | 2,491 ms | 2,307–2,691 ms | Not all synchronous work; workers and display transactions can overlap. |
| Rust `main` → startup window shown → workers admitted | 102 / 210 ms | 71–194 / 108–254 ms | Window shown is not loading SEND. The second span includes native I/O setup, initial rendering/submission and notification, so it is not just scheduler idle. |
| Worker admission → state worker starts | 6.46 ms | 2.38–12.26 ms | Not the worker's subsequent work. |
| State worker start → metadata ready | 1,255 ms | 1,147–1,429 ms | Includes native JPEG capability initialization and demo creation/seeding. This is not a measured database or JPEG wait. |
| Metadata ready → prepared state, just before publication | 158 ms | 112–273 ms | Includes joining metadata, the first UI yield gate, cover decode, and the worker handoff. This does not isolate any one of them. |
| Prepared-state timestamp → app attached | 1.15 ms | 1.04–3.59 ms | Includes publication, receiving, any remaining font/driver work on this path, and initial attachment. It is not a precise send-to-receive clock. |
| App attached → first Shelf renderer return | 739 ms | 669–932 ms | Includes bounded UI stages, inter-turn waits and the full renderer call. The archived clocks do not split that span. |
| Renderer return → accepted SEND → same-marker driver completion | 44 / 366 ms | 32–70 / 364–373 ms | Both are downstream of the primary endpoint, not pre-Shelf wait or optical timing. |

Concatenated spans are calculated **within each launch**, then summarized;
individual stage medians need not add to a total. All 12 qualifying frames
recorded `render_then_wait` in the same-visit Shelf audit. Therefore the
preceding marker wait for the **qualifying frame's write** took place *after*
its private RGB draw, and cannot explain the earlier renderer-return endpoint.
Earlier **loading** transactions can still block a UI turn. Do not equate
these two waits or remove either mapping barrier.

The [archived projection](physical-result.json) and
[replay](pre_shelf_replay.py) are sufficient to recompute the table on `test-env`:

```sh
python3 -IB pre_shelf_replay.py physical-result.json
```

The recombined per-launch totals matched the original monotonic application
and renderer observations for all 12 samples on `test-env`. Negative controls
for a missing visit, reordered visits, a visit without confirmed restoration,
and a missing phase were refused. There are no new device measurements.

## What the sources and older runs imply

- **Do not tune the demo database numbers into a product optimization.** In
  three November 22 demo starts, named materialization/load and demo seeding
  together used **77–84% of the metadata span**. Three older initialized
  ordinary-account starts used **93, 221 and 116 ms** total in the comparable
  metadata span. These were synthetic offline accounts, with a temporary
  private session placement and SQLite/covers on the original user storage;
  they were not a same-build A/B against today's demo or a live-account
  benchmark. Keep session validation, SQLite schema/durability and durable
  interrupted-purchase/recharge reconciliation. The product-written cache
  maintenance control found a 0.49 ms median on Linux, not a basis for
  changing SQLite's transaction policy. The redundant `queued_download_intents`
  warm read was only **0.04–0.09 ms** in those older ordinary starts; the later
  typed read and download badge cannot simply be dropped.
- **The main unresolved part is the 739 ms attachment-plus-draw span.** An
  older three-visit ordinary-account breakdown found attached → Ready of
  **406–430 ms**, with one **357–369 ms** gap between successive attachment
  yields; separately observed loading transactions previously accounted for
  much of that interval. Ready → draw start was just **1.5–2.5 ms**, while
  the qualifying draw itself took **391–593 ms**. A separate glyph study
  found **198–213 ms** constructing glyph images on cache miss versus
  **11–13 ms** painting the masks. These figures explain where to instrument,
  not how to split 739 ms in the current UI. Full font files are already
  expanded at build time; glyph rasterization is a different cost from
  registration or decompression. Prior scaler reuse did not meet its gate.
- **Cover ordering is a real dependency, not yet a proven win.** The current
  path awaits the first UI yield and worker cover preparation before publishing
  state; UI attachment consumes its cover batches before committing the route.
  The existing per-cover path already shows a placeholder while reading cached
  pixels on a background worker, so an ordinary Shelf can legally draw with
  metadata alone. However, the previous deferred-cover experiment was clearly
  earlier in only **2/6** desktop pairs. Removing an earlier wait may merely
  add an EPDC submission before the covers arrive. Measure its timing instead
  of assuming the entire 158 ms or 739 ms could be removed; keep initial
  feedback and bounded, account-bound eventual cover delivery.
- **One clean synchronous dependency to measure on its own:** the state worker
  calls `native_media::initialize` before opening the session and database.
  On qualified hardware it waits for an operation-0 worker exchange (two-second
  receive deadline) covering worker startup, a nonblocking hardware lock and
  `VIDIOC_QUERYCAP`. No decoded page or DMA buffer is requested by this probe.
  The first Shelf does not use this result, and hardware JPEG is off by default.
  Two seconds is a timeout, not an observed cost; older ordinary-account
  metadata left only **21–83 ms** outside all its named probes, which also
  included overhead and other work. If the measured cost warrants detaching
  the probe, start with plain Shelf only: maintain one bounded worker and the
  existing ownership, use software while pending, refresh the capability
  state when known, and avoid re-rendering an already stable Reader page. Do
  not alter direct Reader resume, sleep, settings or framebuffer behavior.

The [earlier startup notes](../../STARTUP_AND_SLEEP.md#primary-startup-metric-first-shelf-render),
[performance decision summary](../../PERFORMANCE_OPPORTUNITIES_20261122.md#scribe-startup-studies)
and [cache maintenance control](../startup-cache-maintenance-20261121/README.md#measured-result)
record the distinct evidence boundaries. Historical substeps here were checked
against the closed November 22 witness and initialized ordinary-account numeric
projections retained on `test-env`; no account state was opened for this replay.

## Recommended next measurement

First profile the current **initialized, ordinary offline Shelf** with local
synthetic data, keeping the launcher, UI and refresh policy fixed. Use both a
normal populated fixture and an appropriate empty-state control. Record the
actual first loading SEND and worker admission; separate the JPEG query from
session/SQLite and the remaining metadata; record the cover gate release and
worker begin/end; split the UI attachment spans from intervening loading-marker
waits and from start/end of the qualifying Shelf draw. Capture all those
boundaries on the same monotonic clock, with thread CPU or explicit wait type
where useful. Retain the narrow qualifying frame/SEND/driver witness. Do not
perform synchronous diagnostic file writes inside the hot frame or worker
measurements: the current opt-in lifecycle trace does append records before
returning to the event loop, and its cost has not been separately measured.

Prioritize the attachment/draw split for finding the larger remaining cost,
and measure the optional JPEG probe at the same time because its dependency is
clear. Change one mechanism only if the new trace demonstrates a recurring,
safe opportunity. Keep the two-second power settle, live native-state checks,
framebuffer-write and input barriers, startup feedback, purchase uncertainty,
and Reader anchors unchanged. No new performance or hardware acceptance claim
is made by this source audit.