# Startup and Page-Turn Cost Review

The subsequent [ARM cause and candidate record](PERFORMANCE_ROOT_CAUSE_20261110.md)
adds R48 App-stage attribution and R49 same-device numerical comparisons. This
earlier investigation retains its original evidence and uncertainty boundaries.

Reviewed on 2026-11-10, Asia/Shanghai, against the frozen `0.1.19` R43 candidate
and its saved R45 physical demonstration run. This is a cost investigation and
an isolated host experiment, not a new product implementation. No Kindle access
or local verification was performed for this review.

The [saved cost decomposition](../artifacts/verification/performance-cost-review-20261110/saved-analysis/cost-breakdown-r46.json)
records input hashes and calculations. The source manifest remains
`f4d0bf08e149e46630a5bb02d56f050cf720a6c7ae9285f2fa927b0473768a3a`.
The [accepted physical result](PERFORMANCE_IMPLEMENTATION_20261109.md) and its
earlier failures retain their original scope.

## What the measured times include

The 4215.079 ms first-image result starts at the application main entry. It ends
at `reader-ready`, after the prepared image has been installed in the UI model.
It excludes the preceding launcher and the new image's subsequent physical
panel completion. It is not a measurement from selecting the normal launcher.

The 1824.137 ms page-turn result starts at the observed UI loading state and
ends at the next `reader-ready`. The request is dispatched before that later
driver snapshot, so admission or worker computation may already have started.
The interval captures whatever work and waiting remain after the snapshot;
it is not a complete request-to-prepared measurement. It ends after UI image
import and model updates, but has no request or finger-contact timestamp.
The new page's Slint composition, framebuffer write and EPDC submission follow
the ready marker. Work on an earlier loading indication can still contend for
CPU or block the UI thread during the observed interval. Earlier descriptions
that implied this interval necessarily includes the entire admission/worker
path were too broad; the recorded timestamps and comparison remain unchanged.

The relevant order is:

`request -> admission -> worker -> result mailbox -> UI image import -> model
publication -> reader-ready -> Slint composition -> framebuffer -> EPDC`

Result delivery already wakes the event loop. The maintenance timer's 80 ms
period is not an unavoidable extra delay on every completed page.

## Startup: measured critical path

These are additive wall-clock partitions from the same R45 App clock. They are
not isolated CPU times.

| Interval | Time |
| --- | ---: |
| Application entry to window shown | 96.773 ms |
| Window shown to startup workers started | 772.847 ms |
| Workers started to final font registered | 1328.836 ms |
| Final font registered to UI ready | 63.364 ms |
| UI ready to first reader-ready | 1953.258 ms |
| Total | 4215.079 ms |

The 772.847 ms interval is a real scheduling/presentation gap, but R45 has no
`first_scene_rendered` event. `window_shown` does not mean the panel has finished.
It is therefore not possible to assign this entire interval to rendering, a
timer, kernel display work or idle waiting from the existing trace. The startup
workers are launched only at the first qualifying driver turn. The native backend
starts with a full redraw pending and skips timer processing until it performs
the initial scene render and display submission. This proves an ordering
dependency, not the CPU or kernel share of the entire gap. Overlapping
preparation with the initial scene is worth testing while preserving visible
startup feedback. The observed overlap window is an upper bound, not a promised
773 ms reduction.

Font preparation spans 1323.329 ms, while `App::prepare` spans 1030.282 ms inside
the same interval. These workers run concurrently. All five faces must arrive
and register before App attachment; fonts currently finish last. Each decoded
face reaches its registered marker within 0.840-3.063 ms, including notification
and scheduling. Font registration itself is not a one-second operation.

If the state branch's timing stayed unchanged, eliminating font preparation
would move the join by at most about 290 ms. Improving only state preparation
would not move the current join. CPU contention can change these branches, so
their real joint benefit needs measurement instead of adding independent times.

The 1030 ms state branch includes offline demonstration materialization and
database seeding. Its trace has no internal spans. It must not be presented as a
measured real-account database bottleneck. Full library image validation is
already deferred from the startup critical path.

The [font storage analysis](../artifacts/verification/performance-cost-review-20261110/saved-analysis/font-storage-review-r46.json)
validated all five gzip streams on `test-env`: compressed payload totals
36,806,470 bytes (35.10 MiB), and identical expanded font data totals 43,523,552
bytes (41.51 MiB). Runtime compression saves only 6,717,082 payload bytes
(6.41 MiB). Pre-expanding the original fonts at build/install time or safely
reusing expanded data is a concrete storage-versus-CPU option. These are payload
sizes, not measured final package, ELF or RSS changes. Font identity, all five
styles, licenses, and the startup scene must remain intact.

The launcher has additional sequential work before the App clock:

| Launcher observation | Time |
| --- | ---: |
| Fixed observer/launcher settling wait, before the first marker | 2000 ms |
| Sample phase begin to snapshot ready | 1440 ms |
| Sequence 1 protected command: go to ready / reaped | 1170 / 1290 ms |
| Sequence 2 protected command: go to ready / reaped | 860 / 860 ms |
| Native display takeover begin to done | 100 ms |
| Sample phase begin through native takeover done | 2560 ms |

The nested rows must not be added again to the 2560 ms total. These observations
include the supervised physical wrapper and omit other uninstrumented launch
work. Do not combine launcher uptime with App timestamps into an invented
end-to-end duration.

The shared trace label `native_read` is broader than its name: sequence 1
captures native state, while sequence 2 invokes
`publish_native_portrait_orientation` to publish the protected typed orientation
sidecar. It is not another full native-state sample. Earlier generic references
to two native reads should be interpreted with this distinction; raw evidence
and its numerical times have not been rewritten.

Both helpers meet their unchanged two-second deadlines. Consolidating repeated
helper/custody work could reduce startup overhead, but the measured 860 ms is
not merely disk-write time. The fixed two-second wait needs a positive readiness
protocol through the production observer and relay plus normal launcher/window-
manager handoff evidence before replacement. The earlier separate 130 ms private
event experiment does not establish those conditions.

## Reader: measured library stages and remaining unknowns

R43 already contains a release-library profile using two fictional 1700x2360
PNG inputs. Their encoded sizes are 103,979 and 86,296 bytes. The table below
uses the 1860x2480 viewport and reports milliseconds on the x86_64 verification
host, not ARM stage times.

| Input / grayscale cache | Samples | Total | Integrity + decode | Gray conversion | Resize | Canvas |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Page 18 / decoded | 1 | 67.223 | 24.284 | 13.898 | 25.848 | 3.026 |
| Page 19 / decoded | 1 | 45.792 | 7.214 | 13.371 | 22.483 | 2.666 |
| Page 18 / reused, medians | 6 | 25.831 | 0.049 | 0 | 22.558 | 3.212 |
| Page 19 / reused, medians | 6 | 24.818 | 0.042 | 0 | 21.892 | 2.912 |

In the reused cases, resize is about 87-88% of this library path and canvas
preparation about 12%. File reading is approximately 0.05 ms for these small
warm-cache files, rotation is zero, and layout is approximately 0.001-0.002 ms.
Independent stage medians need not sum to the total median.

The decoded groups have only one sample each. They do not prove a cold OS cache
or cold integrity memo. The integrity/decode span combines hashing, validation,
cache lookup and necessary decoding. It must not all be labeled decoder time.
Real receipt-backed files also have SQLite, publication and file-object checks
outside this path API; larger JPEG or other sources can have a different mix.
Applying these x86 percentages to the physical 1824 ms would be unjustified.
R45's next source page also grows the grayscale cache from one entry to two;
it is a first preparation of that page. The 87-88% warm-cache resize proportion
therefore belongs to a different workload as well as a different architecture.
The corresponding host first-use samples spend about 38-49% on resize.

The current 32 MiB cache holds validated unrotated grayscale sources. A hit saves
decoding and gray conversion, but still performs the requested resize and canvas
generation. The layout cache is not a rendered-frame cache. Existing prefetch
acquires encoded files; it does not prepare the next display frame and is skipped
in the demonstration/offline path. Raising the encoded prefetch count alone
therefore does not remove on-demand raster work.

Receipt-backed rendering already shares one immutable encoded read and hash
with the reader. A second reader hash is not an outstanding normal-path problem.
Presented progress is already asynchronously coalesced, and result delivery is
already event-driven. These earlier fixes should not be proposed as new savings.

## Root-cause and feasibility reassessment

The implemented single-channel resize removes unused channel work and storage;
the earlier progress and source-loading changes remove identified redundant
work. Those are concrete improvements with their own verification. Remaining
cost need not have the same dominant stage after those improvements. The saved
physical observations support investigating the image pipeline, but do not
identify which remaining ARM stage is now the primary bottleneck.

Three different mechanisms must be assessed separately:

| Mechanism | What is established | Consequence |
| --- | --- | --- |
| Unnecessary startup dependency | All fonts gate App attachment, which gates the first reader request, although image preparation itself does not use fonts | Separating preparation from UI reveal addresses a real dependency; available overlap and real-account readiness still need measurement |
| Expensive work begins only after navigation | There is no prepared next-frame slot; each actual navigation requests raster preparation | Speculation can address predictable navigation wait, but does not reduce required work, first-image cost or arbitrary-seek cost |
| Repeated computation or data movement | Static fonts are inflated every start, RGB/RGBA conversion uses generic iteration, and RGBA is copied into Slint | These are concrete work/ownership candidates; their ARM share and overall return are not yet established |

There are at least two competing explanations for the remaining observed turn:
the worker might consume most of it, or it might finish early and wait for UI
delivery/composition contention. Further standalone resize benchmarks cannot
distinguish them. Correlate request, admission, worker start/end, publication,
UI apply and display submission with one operation identity. Record wall time
and worker CPU time where practical so scheduling/I/O is not mislabeled as
computation. A result should improve the actual request/display path, not merely
move the readiness marker or work across a measurement boundary.

### Startup feasibility

The smallest scheduling experiment is to start the existing immutable font/state
workers after backend and signal setup and `ui.show()`, while retaining UI-thread
registration and the existing reveal gate. It can overlap initial display work,
but it may compete with that work for CPU and memory bandwidth. Measure both the
first visible startup feedback and readable first image.

Separating the font gate from reader preparation is feasible but is not a single
`return` deletion. In the demo path, a prepared App can advance through Route
while `Shell.booting` and `App.is_starting` remain set; Views/final readiness and
result application can still wait for fonts. Reader preparation uses pure core,
settings, viewport and source data; `ReadingState` contains UI objects and must
remain on the UI thread. Reuse the existing worker result and epoch/generation
checks rather than introducing a second renderer.

For a real account, `startup-download-manager` must first yield a manager and
then download recovery must return it. That restores the local resolver/reader
lease and permits page preparation. Normal mailbox draining is currently gated
during startup. Simply moving Route ahead of fonts can improve the demo without
unblocking actual local continuation. A restricted startup-result drain or an
equivalent early reader context must preserve account namespace, permissions,
source proof, reader pinning, recovery, offline and sleep behavior. A standalone
resolver cannot silently create a separate publication/ownership domain.

For fonts, build-time expansion of the identical five faces is a smaller change
than a new persistent runtime cache with repair and publication rules. Keeping
`Arc<Vec<u8>>` still copies the expanded bytes. Increasing ELF size can also add
cost to the supervised wrapper's existing relay/UI hashes and watchdog receipt
validation outside the App clock. The known payload difference is not a net
startup prediction; evaluate the complete supervised and normal launch paths
separately. Native capture/portrait helpers already reuse their verified receipt
and should not be accused of the previously removed per-helper whole-ELF hash.

### Prepared-frame feasibility

A first implementation can be limited to a stable `Page + FitPage` reader,
non-spread layout, the same chapter and a locally available next source. Clone
the presented core and call its real `step(true)`; do not assume every mode's
next navigation is `page_index + 1`. Skip chapter-end transitions. This reuses
`prepare_reader_frame` instead of introducing a cache framework for all modes.

Actual navigation always increments `state.version`, so a prediction stamped
only with the old version would inevitably fail the normal result check. Keep
the prediction in a separate slot with its origin and expected target. Only
after a matching real navigation may it be submitted using the current version.
Do not invoke `apply_prepared_reader` when speculation completes: it updates the
presented core, progress and refresh state.

The candidate must carry the validated source proof, which `PreparedReader`
currently does not retain. Existing thumbnail code demonstrates a reusable
contract: `files.source(..., Some(saved_proof), cancellation)` returns an
unchanged-content result after a fresh read/hash and receipt/file-object checks.
A matching frame can then skip decoding and raster computation. Perform that
check on a worker; using only an old receipt or file metadata would weaken current
integrity guarantees. A changed source takes the ordinary preparation path.

The existing two-slot compute semaphore has no speculative priority. Triangle
has no cancellation points inside its long passes, so dropping a future does
not immediately free an occupied slot. Speculation requires immediate-only
admission when foreground resources are free, bounded cancellation within long
pixel loops, and an independent retained-frame byte limit. Task naming also
matters: the current memory classifier reserves heavy work for `reader-render`
and `reader-thumbnails`; a new plain `reader-prefetch` name would otherwise fall
into the 4 MiB metadata category. A safe miss must not be delayed by unrelated
speculative computation. Reusing an in-flight matching prediction is a later
optimization, not a prerequisite for implementing every reader mode at once.

### Revised implementation order

1. Establish an ARM causal trace through request and display submission while
   preserving the existing observation timestamps. Independently validate the
   startup dependency design for real-account local continuation.
2. Select the smallest change for the measured dominant stage: actual pixel work
   if worker CPU dominates, admission/delivery if waiting dominates, or the
   presentation path if display submission dominates. An 18 MiB copy being
   visible in source does not establish a seconds-scale bottleneck.
3. Add the bounded single-page prediction only as a sequential-reading latency
   improvement, with hit/miss, rapid-turn, reverse-turn and invalidation cases.
   Preserve foreground miss performance, not merely a fast cache-hit result.
4. Evaluate font expansion and initial preparation overlap as separate controlled
   changes. Do not accumulate their overlapping theoretical upper bounds.

The generic row-major replacement is deferred because the host experiment has
a target-size regression. Removing the fixed two-second settle is an independent
readiness-protocol project: no proven production subscription plus native
window-manager handoff condition currently replaces it. A full Slint/grayscale
backend rewrite is also not justified by the present measurements.

## Candidate mechanisms and constraints

These entries describe implementation options; select them using the measured-
stage order above rather than treating every candidate as an established cause.

1. **One low-priority prepared next frame.** After presenting the current frame,
   prepare only the predicted next page. A hit removes most on-demand image
   preparation from the user's wait, while keeping total work and power cost
   visible. One 1860x2480 RGBA frame retains 18,451,200 bytes (17.60 MiB), plus
   metadata and preparation temporaries. Use a separate byte limit and account
   for actual worker memory. The two existing compute slots do not provide
   foreground priority automatically. New requests must take precedence, with
   prompt cancellation and no speculative UI publication or progress update.
   Bind the entry to account/episode/revision/content, viewport, crop and display
   settings; retire it on invalidation, closure and sleep. A cached frame cannot
   replace current source-integrity checks or source-replacement protection.

2. **Direct RGB8/RGBA8 grayscale loops.** These types currently take the generic
   per-pixel image iterator; only Luma8 has a dedicated fast path. Direct row
   access could reduce the observed first-use conversion work. Preserve the
   exact integer luma coefficients, alpha-on-white composition and cancellation.
   Keep other formats on their existing path until separately proven equivalent.
   No speedup has yet been measured for this candidate.

3. **Render directly into the UI-owned output buffer.** The worker produces an
   18.45 MB RGBA vector and `clone_from_slice` allocates and copies it again on
   the UI thread. Later `Image` clones/setters share ownership and should not be
   counted as more whole-image copies. Slint 1.17.1 permits constructing a
   `SharedPixelBuffer` on a worker and handing it to the UI, but offers no public
   zero-copy adoption of the existing vector. A backend-neutral render-into-slice
   interface can fill the single shared output buffer while keeping ReaderCore
   separate from Slint. Merely moving the current copy onto a worker does not
   remove the total copy. Its current ARM time remains unmeasured.

4. **Overlap startup preparation and reduce repeated font work.** Investigate
   the 773 ms pre-worker gap, pre-expanded identical fonts, and preparing the
   selected reader as soon as its minimum state is available. Preserve early
   visible feedback, error handling and layout. Optimize the parallel branches
   together; the standalone font span is not directly recoverable startup time.

5. **Optimize composition after measuring it.** Reader RGBA pixels have alpha
   255. Submitting them through the premultiplied path can avoid unnecessary
   premultiplication with the same bytes; RGB output could reduce image payload
   by 25% but needs broader interface changes. The native backend has one reused
   RGB composition buffer and only a reused grayscale row, not another full-size
   grayscale allocation per turn. Its RGB-to-gray conversion, inversion and
   framebuffer stores are further candidates. These mainly affect the display
   work after `reader-ready`, so they cannot be credited against the recorded
   1824 ms without new timing. Ordinary turns have no explicit panel-completion
   wait to simply remove; suspend/lifecycle waits have a different purpose.

PNG first-use validation also inflates the complete stream before the pixel
decoder processes it. Sharing one decode with the integrity proof may eventually
help, but only with equivalent CRC, complete-stream/trailing-data, scanline,
Adam7, animation and resource checks. The present combined span does not isolate
its cost, so it ranks behind simpler loop and ownership changes.

## Isolated horizontal-traversal experiment

The current Triangle horizontal pass walks columns across all rows of a large
f32 intermediate. An isolated candidate precomputes the same column coefficients
and walks rows. Per-pixel f32 operation order, clamp, round and color metadata
remain unchanged. The production files were not edited.

The [complete experiment](../artifacts/verification/performance-cost-review-20261110/triangle-locality/README.md)
uses one small `rustc` binary and the existing pinned image 0.25.10 dependency,
read-only. Each case has eleven alternating-order pairs. The two fixed-seed
synthetic gray inputs have the same dimensions as R43 but different pixel bytes.

| Viewport / input | Current median | Row-major median | Observation |
| --- | ---: | ---: | --- |
| 1860x2480 / 0 | 20.931 ms | 18.725 ms | 10.54% shorter |
| 1860x2480 / 1 | 20.739 ms | 18.637 ms | 10.14% shorter |
| 930x1240 / 0 | 7.493 ms | 8.165 ms | 8.97% longer |
| 930x1240 / 1 | 7.382 ms | 8.079 ms | 9.43% longer |

All 1,995 boundary/crop/zero/exact-size/metadata comparisons and every sampled
large-image comparison matched completely. Timing includes both resize passes
and candidate coefficient preparation; it excludes decoding, UI work and output
comparison. Shared-host load and cache/frequency conditions were not controlled.

The outcome supports investigating bounded tiling or a measured size-dependent
strategy, not blindly replacing all horizontal traversal. ARM results are
unknown. The candidate's coefficient/column storage also needs an explicit byte
budget; peak RSS was not measured. Sources, all samples, hashes, independent
review and complete third-party licenses are retained with the experiment.

## Next measurement before implementation selection

Add a small opt-in set of monotonic stage records, emitted once per operation:

- Startup: first driver entry, scene render begin/end and submission, worker
  start, font preparation, state substeps, initial reader request.
- Reader: request, compute admission, worker start, receipt/read/hash/validation,
  decode, grayscale, resize, canvas, result publication, UI apply and image copy.
- Display: Slint render, RGB-to-gray/write, ioctl begin/end. Panel completion
  needs separate observation; inserting a blocking wait would change behavior.

Compare identical fictional PNG and JPEG sources through both the path API and
receipt-backed App path, with grayscale misses/hits separate and the same output
pixels/settings/build profile. Then use a fresh coordinated device window for
ARM attribution. No hardware run is authorized by the already released R45
lease. This review supplies priorities and a mixed host prototype result; it
does not claim those candidates are implemented or physically accepted.
