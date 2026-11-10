# Streaming Resize and One Prepared Next Frame

This follow-up implements the first two decisions from the
[KOReader technique review](KOREADER_OPTIMIZATION_AUDIT_20261110.md) in `0.1.22`.
It retains the separately verified `0.1.21` application as its baseline. The
earlier physical acceptance does not apply to this new candidate.

## Selected changes

The Triangle scaler now completes one vertical output row, applies the existing
horizontal coefficients to that row, and reuses its f32 buffer. Each pixel keeps
the original vertical-then-horizontal accumulation order, coefficient arithmetic,
rounding, crop, metadata, and final canvas processing. The 256 KiB horizontal
plan limit remains. An oversized plan or unavailable row reservation falls back
to the previous complete intermediate and column traversal. Cancellation checks
cover planning, weights, rows, output blocks, and exact-size copies.

The application can retain one prepared next page when preloading is enabled.
This opportunity applies to a settled single-page FitPage reader, the same
chapter, and a locally available source. It does not acquire remote content or
prepare spreads, panels, strip views, or a new chapter. The retained allocation
budget is 32 MiB, including owned raster capacity and preparation metadata;
shared source-cache storage has its own existing bound. The actual worker keeps
its separate 192 MiB planned computation reservation until it returns.

Prediction can start only when every compute slot is idle and no regular
compute waiter exists. Regular computation cancels the exact flag owned by the
prediction. The prediction uses one independent result-delivery slot and key,
so it cannot occupy an ordinary job's final delivery slot. Waiting on a network
response alone does not block this local opportunity. Busy admission remains a
normal cache miss, without a periodic polling or retry loop.

The cache key includes account epoch, reader generation, origin version, full
origin and target inputs, viewport, and landscape state. A hit still reads and
hashes the current source, then rechecks its receipt and file object in the
worker. Changed input takes the ordinary rendering path. Prediction completion
does not publish an image, reading anchor, or remote reading history. UI import
and display composition remain on the navigation path.

Sleep, close, account changes, incompatible settings, and relevant source
notifications retire prediction. A completed obsolete worker can offer one new
opportunity after its callback releases the delivery permit. Disabling preload
through the actual account intent releases retained pixels; enabling it offers
a new opportunity without moving the current page. Existing initial-loading
sleep continuation and manual orientation preferences remain intact.

## Preserved kernel-only evidence

The first R52 component run completed on `test-env` before the continuation
session. It used the exact `.21` and `.22` scaler modules in one host executable,
with separate timing, allocator instrumentation, and fresh-process RSS modes.
All 224 timing samples, 224 allocation samples, 56 independent RSS children, and
14 complete reference pixel/metadata comparisons passed. Both the default path
and the real uncancelled `Some(false)` worker path were exercised.

| Actual Fit raster | .21 incremental allocation peak | Streaming incremental allocation peak |
| --- | ---: | ---: |
| 1786 x 2480 | 21,357,560 bytes | 4,500,376 bytes |
| 893 x 1240 | 9,577,912 bytes | 1,152,744 bytes |

These are complete kernel allocation requests above its retained input/reference
baseline, including returned Gray8 output. They are neither total application
RSS nor the RGBA prepared-frame slot. The large cancellation-aware case's
independent `/proc` VmHWM observations were 27,192-27,200 KiB for `.21` and
10,696-10,784 KiB for streaming. Historical `getrusage` high-water floors remain
separate in the raw evidence.

Host CPU samples contain visible outliers. The kernel result supports reduced
working allocation and exact output; it does not establish a stable speedup,
ARM performance, application navigation latency, or panel completion.

The component manifest is
`1f0fdc960abe6424ddaabaa3440cafe05a2cf1cd03aebfedf38d2736e5f7aea7`;
the probe ELF is
`df67ded79840c118ffe6fb9f1b4bc10212a9103b028d34edd90c3a16b3877394`.
Original evidence remains at
`test-env:/root/bilicomics-streaming-prefetch-20261110-r52/evidence/kernel-r1`.

## Integration verification

Selected software verification is complete. The continuation review found and
fixed two integration gaps before freezing application inputs: the account
preload intent had not called the reader invalidation hook, and an obsolete
prediction retried from inside a callback that still held its sole delivery
permit. Tests exercise the actual intent and the normal tick after a delayed
worker exit, rather than repeatedly forcing preparation in the fixture.

Selected gates retain complete library and binary tests, source-proof and
admission regressions, reader/guest/startup/lifecycle cases, diagnostic parity,
real non-demo offline receipt navigation, 40 resource cycles, complete frame and
anchor equivalence, and production ARM packaging. Execution is confined to
`test-env` with one build job, one test thread, one CPU quota, 8 GiB maximum
memory, a 7 GiB high threshold, and no service swap. Historical component reuse,
fresh application results, and physical-device observations must remain separate.

The frozen application source manifest is
`bb5078f4900a0c2bb32bc036c1aa4090aeff292532cf8e25e729531581a37cf4`;
its source-plus-pipeline manifest is
`da4ccf4521c0efa0e268da4db40d6ec57a7442c521f3c2274fc5c140fe105bd3`.
All twenty selected inputs from the earlier kernel snapshot remain identical.
The root package notice is additionally bound outside the historical manifest
scope, because the ARM package consumes that file directly.

| Attempt | Observed result and retained boundary |
| --- | --- |
| `app-r1-core` | Rejected before starting a service or compiler: available memory was below the unchanged 12 GiB startup requirement. |
| `app-r1b-core` | Passed 667 library tests, five binary tests, required named coverage, and the diagnostic parser cases. Backend/harness results have explicit unchanged-component reuse proofs. |
| `app-r1-runtime` | Reader, guest, startup, lifecycle, and trace parity passed. The prepared-App probe failed when its before capture was still the old loading scene; resources had not started. |
| `app-r1-final` | Rejected by the same memory requirement before any service or compiler started. |
| `app-r1b-final` | Exhaustive rounding passed. The next compilation admission was rejected by the unchanged memory requirement; complete-frame and ARM stages did not start. |
| `app-r1c-final` | Exhaustive rounding and all 24 complete RGBA/geometry/anchor comparisons passed. ARM packaging stopped before compilation because the transient service did not supply the toolchain's required home environment. |
| `app-r3-remaining` | The corrected six-case prepared-App workload, all original 40 resource cycles, and the production ARM build/package checks passed. |

The original prepared-App failure remains failed. Its after capture and final
current-page capture are exactly equal across all 930 x 1240 RGB pixels; the
before capture contains the loading scene. There is only one foreground image
import/model publication, so model readiness did not establish completed X11
painting. The revised probe first binds all five current-page references from
the completed baseline, then requires each subsequent before capture to match
the selected page's complete RGB, dimensions, and semantic state. Every capture
attempt and its timing is retained. The after capture is compared directly;
the probe does not search for a convenient matching after frame. Eight actual
hits and at least one real before/after-retention pixel-and-anchor pair remain
mandatory. Capturing itself has CPU cost and does not provide an unperturbed
prediction-time measurement.

The separate launcher repair explicitly selects the same caller UID, letting
systemd provide that user's normal home environment. A bounded environment-only
check established that behavior before the next run. Resource limits, source,
application binaries, numerical checks, and acceptance thresholds were not
changed to resolve either driver issue. The selected drivers retain their own
versioned manifests; no failed run is relabeled as successful.

## Actual host application observations

The completed baseline/candidate/candidate/baseline workload uses five fictional
receipt-backed page positions, two encoded objects, no credentials, and an
isolated offline network namespace. The nonzero initial anchor is page index 1.
Each side has eight measured forward turns after warming source bytes and
dwelling for at least 0.5 seconds. The desktop logical raster is 930 x 1240.

| Request to new reader model | .21 baseline | .22 prepared hit |
| --- | ---: | ---: |
| Median of eight observations | 234.094 ms | 11.317 ms |
| Observed range | 229.439-240.984 ms | 10.336-15.118 ms |

Each of the eight hits completes one fresh source read, hash, and receipt/object
recheck inside its worker. It performs no decode, rotation, layout, resize, or
canvas work, and still copies 4,612,800 bytes into the UI image. Ten independent
before/after-retention observations preserve complete RGB, dimensions, and
logical anchors without an image import or model publication during the dwell.
Source-cache growth is separately bounded; it is not silently compared as an
unchanging UI field.

In the same candidate process, the actual preload intent releases a ready slot
at zero, produces no new prediction during the confirmed disabled next/previous
interval, and prepares a new slot after restoring three. Its ordinary disabled
next-page miss takes 215.549 ms to the model, with the full renderer and source
proof present. Matching baseline target-page observations take 229.439-240.984 ms.
Four candidate reverse misses take 194.828-206.238 ms, versus the corresponding
baseline reverse observations of 223.446-245.027 ms. These samples show no miss
regression in the exercised workload; they are not a statistical guarantee.

Two additional close/reopen cycles release the prepared slot, replace the
reader generation, and restore nonzero page index 1. The pre-close retained
state is established by its event chain without a later consumption or
replacement, rather than a direct ready-slot snapshot immediately before close.
The original independent resource cases each complete twenty actual cycles.

The measured retained slot occupies 4,622,379-4,635,135 bytes at this desktop
raster. Candidate process high-water samples are 217,939,968 and 218,361,856
bytes, versus 216,891,392 and 208,670,720 bytes for the two baseline processes.
They do not demonstrate a total App RSS reduction. The isolated scaler working
allocation reduction and the retained prepared page are different quantities.

These are trace-enabled host-debug request-to-model observations. Prediction
work and its dwell precede the measured turn, while display composition follows
model publication. Neither computation nor UI copying has disappeared overall.
The observations do not establish Kindle, panel, power, cold-start, real-account,
or percentile results for `0.1.22`.

This iteration does not remove UI image copying, change JPEG color conversion,
persist rendered frames, replace f32 weights, enable hardware dithering or MDP,
or change font preparation and the launcher's fixed readiness wait.

## Selected evidence and development package

The [safe software evidence](../artifacts/verification/streaming-prefetch-20261110/r52-software-safe/README.md)
retains the selected results, explicit historical reuse, original failed/rejected
attempts, raw quantitative performance records, and copy identities. Its
[aggregate record](../artifacts/verification/streaming-prefetch-20261110/r52-software-safe/evidence/final-selected-summary-r52.json)
does not represent a new single whole-suite execution. No local verification,
real-account operation, physical lease acquisition, or new device run belongs
to R52.

The [0.1.22 development ZIP](../artifacts/verification/streaming-prefetch-20261110/r52-software-safe/app/artifacts/verification/bilicomics-kindle-0.1.22-dev.zip)
is 50,883,370 bytes, SHA-256
`aa62f2dba0a62167df3fac0577169ca3f8179ab6f3f2419386e90d5fb0fbb163`.
Its 66,701,328-byte static ARM executable has SHA-256
`729305ff489a5004731fbda9a8700e6a7a47fe70695c536e198b205083437747`.
All ten package/ELF checks pass for the 32 allowed archive entries. The production
build retains thin LTO; the separate optimized host correctness build does not
establish that profile's performance. Compiler warnings remain in the logs.

The package is a development candidate, not a published release or a general
device-support claim. Any supervised `0.1.22` physical result requires a fresh
exclusive handoff and remains separate from this software record.

## R53: physical observations and a fixture limitation

After a fresh handoff, R53 ran the exact production ARM above on Scribe 2022 /
5.19.5. The operator confirmed the initial picture and one ordinary page 18-to-19
turn were normal. For subsequent rapid tapping, the operator reported that later
pages looked identical and the picture did not visibly change. Rapid visual
acceptance remains false, and sleep/wake was not exercised in this batch.

The saved current state reaches page 29 in the same PID 8660 / birth 745276542.
The bundled design has only page 18 and page 19 templates. Missing page numbers,
including page 20 onward, deliberately reuse page 18 while retaining distinct
page identities. All 4,612,800 visible framebuffer bytes at page 29 equal the
initial page 18 capture; page 19 differs from them in 3,387,951 pixels. This
explains the repeated pictures and establishes that the saved buffer did not
remain at page 19. It cannot turn the operator's rapid observation into a visual
pass or prove that every physical touch produced a panel update.

The [saved R53 analysis](../artifacts/verification/streaming-prefetch-20261110/physical-r53-safe/analysis/r53-v1/README.md)
associates 25 navigation requests, 26 reader jobs including startup, 22 image
imports/publications, and four superseded results without UI import. Fourteen
prepared hits include the ordinary turn and thirteen later operations. There
are 22 predictions, of which sixteen are retained and six are cancelled. These
are application operation counts, not a count of physical touches. This demo
path re-reads and hashes source files on hits; it does not exercise the separate
R52 receipt-backed recheck path.

| R53 observation | To model publication | To first associated accepted refresh return |
| --- | ---: | ---: |
| Process entry to initial page | 3704.063 ms | 3943.379 ms |
| Ordinary 18-to-19 render request | 94.640 ms | 326.294 ms |
| Thirteen later hit render requests | 47.189-75.308 ms | 249.761-398.945 ms |
| Five later miss render requests | 413.203-484.758 ms | 633.856-719.833 ms |

Two completed deferred operations lack their own `render_request` record in
this frozen diagnostic implementation. Their navigation-request intervals are
retained separately rather than pooled with render-request samples or filled
using another operation's fields. Native frames with an old operation identity
or no dirty refresh are not counted as new image publications. Kernel refresh
acceptance is not physical panel completion.

The single startup observation is 500.417 ms longer than R51's 3442.962 ms.
Initial resize falls from 384.604 to 312.931 ms, while font-worker, initial decode,
and native composition intervals increase. The first prediction also overlaps
initial native drawing. These two separate trace-enabled runs establish neither
a stable startup regression nor an isolated cause, and R53 is not presented as
a startup improvement. Their initial framebuffer memories match exactly.

The planned operator stop records status 143. The watchdog's active-owned
identity/token cleanup checks, native statusbar/awesome baselines, launcher-lock
absence, and host transport closure pass. Its cleanup predicate is more specific
than PID/birth alone and permits exited zombies; a separate proof that every
recorded PID/birth disappeared from `/proc` was not captured. Final native
Home/touch confirmation by the operator is unknown and remains separate from
automatic restoration checks.

The exact lease was released at `2026-11-10T15:31:41.363514Z`, independently
absent at `15:32:55.489266Z`, and explicitly returned to WeRead. Analysis and
export took place after release. Further device work requires a new immediate
operation batch and fresh handoff/token/run identities. A distinct twelve-page
offline fixture was prepared for the following visual and sleep checks;
its guest data and validation tools do not change the production ARM bytes.

## R54/R55: distinct offline pages and selected physical observations

The next fixture contains twelve locally generated grayscale pages with large
numbers, different whole-page patterns, and independent machine markers. It
uses only isolated guest receipts; the production `0.1.22` ARM and ZIP remain
unchanged. A verification-only socket guard preserves local UNIX IPC while
rejecting other socket families. This follows the kernel's
[seccomp filter contract](https://www.kernel.org/doc/html/v4.19/userspace-api/seccomp_filter.html),
not a claim of a general arbitrary-code sandbox.

R54 passed the device guard and native-IPC checks and reached page 1. Its initial
checkpoint failed because the bounded log sink buffered the short stderr guard
proof until exit. The failed run was restored and released. A separate exclusive
proof file, flushed and closed before UI exec, fixes live observation without
changing the production logger. A real host run with the same `head -c` sink
proved that this file was readable while the ordinary log still held zero bytes.
Marker mapping also now uses the real full viewport canvas and its centered
aspect-preserving image region.

Live Seccomp mode 2 remains mandatory. The guard separately records its pre-exec
`prctl` value; a missing live `NoNewPrivs` field is explicitly unknown, not zero.
The [proc status manual](https://man7.org/linux/man-pages/man5/proc_pid_status.5.html)
identifies that field as a Linux 4.10 addition.

R55's accepted captures bind PID 23688 / birth 746256450, the fixed executable,
live socket-filter state, and markers 1, 2, and 4. The three complete visible
buffers differ. The operator confirmed normal 01-to-02 navigation and two quick
next taps reaching 04 with a normal picture. These observations address R53's
repeated-content limitation.

Three navigation requests produce four new image imports including startup.
Two preparations hit the prepared slot, with complete source read/hash and
receipt recheck and no renderer work. Their render-request-to-model intervals
are 78.136 and 72.239 ms; first accepted refresh returns are 296.554 and
317.652 ms. The rapid miss reaching page 4 takes 423.370 ms to the model and
634.017 ms to accepted refresh return. These few synthetic-content observations
are not percentiles, panel completion, real-account results, or an equivalent
startup comparison with the earlier demo workload.

Two suspend/resume pairs appear in the same-PID trace. The first, on page 1,
has no identified cause or operator rating; subsequent ordinary navigation and
the page 2 capture show that input continued afterward. The requested page 4
sleep/wake preserves generation 3, version 4, page identity and reading state.
The operator confirmed the normal page 4 picture without a native shelf flash.

The original post-wake checkpoint failed: an unchanged retained image can be
reannounced during resume without another `reader-ready` record. The corrected
tool binds publication time to the current resume state already matched to the
retained ready image. Replaying all saved pre-quit records passes that limited
contract, and targeted regressions pass. It does not create a missing post-wake
framebuffer capture or a new saved live-birth observation. Six model announcements
include two resume reannouncements; they are not six image preparations. The
[R55 report](../artifacts/verification/streaming-prefetch-20261110/physical-r55-safe/analysis/r55-v1/README.md)
preserves these limits and the original failure.

Native restoration, launcher-lock absence, all 21 PID/birth identities in the
post-restore snapshot, and host transport closure passed. Final native Home/touch
operator confirmation remains unknown. Exact lease release was
`2026-11-10T18:21:41.254226Z`, independently absent immediately afterward. The
saved recovery record was already complete at `18:17:35.691256Z`: release was
delayed by 245.562970 seconds. This violated the immediate-release rule and is
not relabeled as compliant.

The follow-up [batch finalizer](../scripts/verification/physical/device_batch.py)
now preserves a failed command's exit code while chaining verified recovery,
exact lease-status binding, owned transport closure, and owner-checked release
before returning control. Incomplete evidence blocks release; an already released
run cannot be reused. Twelve mocked cases and isolated host integration with
real helpers and synthetic leases pass, including a live transport that prevents
release. These later checks do not change R55's recorded delay or claim another
physical run.
