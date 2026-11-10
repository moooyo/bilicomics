# Measured ARM Resize Cause and Selected Optimization

This record follows the [cost and feasibility review](PERFORMANCE_COST_REVIEW_20261110.md).
R47b verified the opt-in `0.1.20` diagnostics, R48 measured the existing application
on the supervised Kindle, and R49 compared bounded synthetic resize candidates
on that same device. The resulting `0.1.21` integration received its own R50
software and R51 physical verification below; component benchmarks remain
separate from application observations.

## Diagnostic candidate and retained failures

The R47b [software evidence](../artifacts/verification/performance-trace-20261110/r47b-software-safe/README.md)
binds application source manifest
`b9e3f765682b92c293479c27bbeba86ab96e9805be89e5726bee3b50d919b9d5`.
579 library tests, five binary tests, 158 backend tests and the selected reader,
guest, startup and full-state lifecycle cases passed under their recorded source
and runner scopes. The revised pipeline additionally passed 37 supervisor
fixtures, nine diagnostic schema cases, trace-on/off pixel and durable-state
parity, the pre-existing-output isolation case, and the 32-entry ARM package's
ten checks. The old unflagged performance diagnostic was not silently executed.

The original R47 supervisor run failed 25 of 36 cases: an absent optional trace
marker triggered `read_one` failure under `set -e` before UI startup. R47b gives
that marker an explicit default and only reads an existing regular marker.
The new positive test proves only the UI receives `--performance-trace`; native
capture, portrait publication and power relay arguments remain unchanged.
The failed run remains failed. Exact source/runner reuse is distinct from a fresh
whole-suite rerun. An earlier prestart attempt also records the external removal
of the old build cache before Cargo ran; rebuilding used a new private target.

## R48: where ARM work actually goes

The [R48 evidence](../artifacts/verification/performance-trace-20261110/physical-r48-safe/README.md)
uses `0.1.20` ARM SHA-256
`86c2ca554d80e5646ce35af33982909323069a0aa88e27a5728ad0df95fb7fa2`,
PID 26345 / birth 737776883, a 1860x2480 logical raster and the offline demo.
The 200-record diagnostic has a valid header and finished, non-truncated terminal.
Cross-thread records were associated by operation identity and monotonic time,
not assumed to be ordered by append position.

| Work | Wall time | Same-thread CPU time |
| --- | ---: | ---: |
| First image preparation | 2209.297 ms | 2122.829 ms |
| Its resize stage | 1752.112 ms | 1739.403 ms |
| Next new-gray-source preparation, later superseded | 1935.372 ms | 1923.212 ms |
| Its resize stage | 1572.013 ms | 1565.029 ms |
| Reused-gray preparations, four observations | 1380.618-1608.121 ms | 1370.400-1587.725 ms |
| Their resize stages | 1321.472-1550.624 ms | 1315.675-1534.405 ms |
| UI image import | 44.777-66.031 ms | 44.313-65.301 ms |

Resize accounts for about 79-81% of cold-source preparation wall time and 81-82%
of CPU time, and approximately 96% of reused-gray preparation. This is now ARM
evidence rather than an extrapolation from the x86 warm-cache profile. Ordinary
compute admission was below 0.833 ms. The major measured cost is actual resize
CPU work, not the normal queue admission or the image import.

The first corresponding native content frames took 173.692-183.234 ms, including
roughly 143-152 ms of Slint composition and 30-34 ms of framebuffer writing.
Refresh submission was 0.207-0.444 ms. Kernel acceptance is not physical panel
completion. These stages occur after the new image's model publication and are
not subtracted from the worker preparation span.

The operator reported normal completion of the requested next/previous/next
sequence. The recorded application behavior is more specific: five `turn`
requests, six preparations including the initial frame, and five imports/model
publications. Published page numbers were 18, 20, 21, 20 and 19. One earlier
result was superseded after completing 1935 ms of work, and the following request
waited 1114 ms before its job could start. Two extra historical `reader-ready`
records on version 5 reflect menu changes rather than additional prepared pages.
Do not label this a clean three-turn script or infer five physical touches.

The same trace resolves the startup gap for this run. Window-to-workers was
703.309 ms: native initialization took 582.014 ms, the initial frame 107.113 ms,
and the remaining gap about 14.182 ms. Initialization CPU was only 17.728 ms;
its active-I/O and portrait-observer stages still require separate attribution
before changing their behavior. The earlier approximately 773 ms gap cannot be
treated as free idle time to delete.

Font preparation was 1360.673 ms wall / 1196.941 ms CPU and overlapped state
preparation of 1065.500 ms wall / 243.472 ms CPU. The demo state includes 382.445 ms
of materialization and 516.995 ms of seeding. All font registrations together
took only 0.604 ms. Holding the state branch fixed leaves about 296 ms of join
headroom for a font-only change in this workload.

R48 has no same-device trace-off pair. Inner samples finish before their record
is written; outer worker and UI spans can include nested diagnostic overhead.
These observations are not P95, cold-storage or live-account performance claims.
The independent x86 receipt-fixture owner-sync wait remains a separate lock/
SQLite hypothesis; the physical demo does not exercise that branch.

After the planned supervised stop (`operator-stop` / 143, not X exit), native
baselines, the launcher lock, all 17 recorded process identities and owned
transports were checked. The operator confirmed normal native dark display and
touch. Exact lease release was `2026-11-09T18:45:41.963429Z`, followed by independent
absence at `18:46:00.470689Z` and explicit return to WeRead.

## R49: controlled numerical candidate comparison

The [host prototype](../artifacts/verification/resize-rootcause-20261110/prototype-safe/README.md)
first proved exact final-byte rounding with 4,835 boundary values and every
75,431,936 finite f32 value in the nontrivial interval. Complete resize pixels
and metadata were also compared. A same-expression ARMv7 code-generation probe
shows a `roundf` call for the old conversion and none for the integer helper;
that probe alone does not establish production runtime attribution.

The subsequent [physical component evidence](../artifacts/verification/resize-rootcause-20261110/physical-r49-safe/README.md)
compares four variants inside one static ARM executable, SHA-256
`10f530d200ae15d98ef111f0ddbadf257ba7b9d3e7d9ae22dbe9aeb2890f7116`.
It generates two synthetic 1700x2360 grayscale inputs in RAM, with no display,
input, service or account operation. Each case has four timed samples per variant,
with every variant occupying every execution-order position once. Full output
and metadata comparisons pass for every sample.

| Input / viewport | Baseline | Exact rounding only | Row traversal / coefficients | Combined |
| --- | ---: | ---: | ---: | ---: |
| 0 / 1860x2480 | 1419.104 ms | 1371.779 ms | 338.762 ms | 298.276 ms |
| 1 / 1860x2480 | 1408.374 ms | 1371.562 ms | 338.919 ms | 297.838 ms |
| 0 / 930x1240 | 210.242 ms | 192.706 ms | 159.562 ms | 141.042 ms |
| 1 / 930x1240 | 211.267 ms | 191.817 ms | 158.776 ms | 143.102 ms |

The main observed gain comes from row traversal with precomputed coefficient
layout, not removal of `roundf` alone. Combined resize time is approximately 79%
shorter for the large target and 32-33% shorter for the smaller target. This is
consistent with improving the original cross-row buffer access, but no cache/TLB
hardware counters were measured. The x86 row-only regression is still retained
in the earlier cost review.

The first large case retains a contemporaneous user-authorized read-only hardware
metadata SSH activity limitation; its end had not yet been confirmed. The other
three cases followed the completion notice. Device CPU samples have approximately
10 ms `getrusage` quantization. No governor or affinity change was made on the
device, and no stable latency, App, cancellable-path, input or panel speedup is
claimed from this component run.

Only four timeout-supervisor PID/birth identities were captured; the child
identity files were empty and are preserved as such. Their absence checks,
the exact benchmark executable scan, bounded command status, before/after SHA,
native baseline and transport closure are recorded without inventing child PID
evidence. The benchmark never took native display/input control. Native mode,
statusbar identity and awesome identity/state matched. Exact lease release at
`2026-11-09T19:47:21.488972Z` was independently confirmed absent at
`19:48:09.873895Z`, then explicitly returned.

## Selected product change and acceptance boundary

The selected `0.1.21` change retains the existing filter, crop, per-pixel f32
coefficient/accumulation order, quantization and frame geometry. It combines:

1. A horizontal coefficient plan and row traversal, bounded to 256 KiB of the
   two plan vectors' element capacities. Count/size arithmetic is checked before
   allocation, each vector reserves once, and actual capacities are checked.
   An oversized or unavailable plan falls back to column traversal. Existing
   source/intermediate/output buffers and allocator overhead are not this budget.
2. One shared exact integer implementation of the old clamp, round-away and u8
   cast, used in both row and fallback paths. It is not the inexact `x + 0.5`
   shortcut and does not change intermediate arithmetic.
3. Cooperative cancellation of a genuinely superseded reader render, with
   bounded plan/weight/pixel/copy checks. A same-version notification does not
   cancel useful work. Real workers retain their resource permits until returning;
   the existing stale callback then starts the latest target. No partial frame or
   unpresented reading anchor is published, and initial-loading sleep continuation
   remains intact.

The plan adds a small bounded working allocation; it does not implement a
prepared-frame cache or a streaming single-row intermediate. The full existing
f32 intermediate remains. Font storage, startup readiness protocols, refresh
policy, GPU/MDP acceleration and new speculative scheduling are not changed.

R50 software verification passed the integrated application's actual
`AtomicBool(false)` worker path, full RGBA/frame metadata/anchor equivalence,
forced fallback, cancellation, 40 resource cycles, lifecycle and ARM packaging.
It also passed actual-App RGB/frame/durable-state comparisons with the sealed
`0.1.20` binary. The two explicit mathematical/full-frame checks completed in
the separately scoped recovery below. R51 then supplied the independent final
App measurements and interaction acceptance. R49's percentage is not promoted
to an application result.

## R50 verification environment interruption

The initial R50 run completed its ordinary tests, runtime/resource/pixel checks
and production ARM package before VM 101 became unresponsive during the first
host release libtest build. There was no result for the exhaustive rounding test
and the full-frame oracle had not started. The original run is retained as
interrupted; its historical running status is not a successful final result.

After the separately authorized VM recovery, source and pipeline hashes and the
completed executable/archive bytes were checked again. The two unfinished
correctness checks retain the same frozen source, inputs, filters and assertions.
Their resumed host target uses optimization level 3, disabled debug assertions,
one codegen unit and explicit LTO off. It is not a production performance build
and supplies no timing claim. The already-built production ARM remains thin LTO.

The resumed transient service verifies effective limits before Cargo starts:
8 GiB maximum memory, a 7 GiB high threshold, no swap, one CPU quota, one Cargo
job and one test thread. A parent outside the service records memory events,
peak usage and completion. VM-level memory pressure was observed during the
incident, but an attributable per-process peak was not captured for the old run.
Resource limits are intended to contain this verification task, not reserve or
lock the shared host.

The resumed build and both unchanged tests completed successfully on one host
libtest ELF, SHA-256
`45efc0c4f208c106134fcb0fac75fd5e35cc9f91d7451e48b9ab002ad948c90c`.
The exhaustive core scan and full-frame oracle passed; the latter compares two
inputs across eight configurations and three current execution paths, for 24
complete RGBA, frame metadata and anchor comparisons. Recorded cgroup peak was
7,518,416,896 bytes, with high-threshold reclaim/throttling events but zero max,
OOM or OOM-kill events and zero swap. The service exited successfully and became
inactive. Historical anonymous/file memory breakdown was not captured and is
not reconstructed. The original interrupted run remains separate.

## R51: final application acceptance

The [R51 safe evidence](../artifacts/verification/resize-optimization-20261110/physical-r51-safe/README.md)
binds the production `0.1.21` ARM
`bda8dd027e7dd3d6a38d956ddffadde6561639e73cad61929538793f8ff5fbb8`
and development ZIP
`49b1133413ee44a5878dec275c831368b113e886cad1d4affa88954308c542e3`
to the final supervised Scribe2022 / 5.19.5 run. Both live checkpoints verify
PID 32479 / birth 742344981, the exact executable, source revision and settings.
The 1860x2480 raster, Y8INV mode, application rotation 0 and framebuffer rotate 3
match the compared R48 first-image conditions.

The operator confirmed complete correctly oriented artwork, normal ordinary and
consecutive interaction, and noticeably faster page turning. The actual trace
contains eleven navigation requests, twelve preparations (eleven ready and one
cancelled), eleven imports/model publications and 38 native frames. Published
pages are 18, 19, 20, 19, 18, 20, 21, 22, 23, 24 and 25. This is not a clean
three-touch sequence despite the requested interaction. Menu changes and old-
model redraws are not counted as new image preparations.

| Application observation | R48 / 0.1.20 | R51 / 0.1.21 |
| --- | ---: | ---: |
| Main entry to first accepted image submission | 4714.814 ms | 3442.962 ms |
| First-image preparation | 2209.297 ms | 875.669 ms |
| First-image resize | 1752.112 ms | 384.604 ms |
| Direct gray-reuse navigation to accepted submission, menus closed | 1637.161-1858.732 ms, n=3 | 674.584-719.853 ms, n=7 |
| Median of those gray-reuse observations | 1638.435 ms | 704.600 ms |

The single new-gray-source navigation in R51 reaches accepted submission in
948.429 ms. Its preparation falls from the completed-but-unpresented R48 cold
operation's 1935.372 ms to 727.957 ms. Of that 1207.415 ms preparation difference,
1202.107 ms is in resize (1572.013 to 369.906 ms). This directly supports the
selected resize change within the tested workload. R48's old cold operation was
superseded before presentation, so no old cold navigation-to-display interval
is invented for comparison.

The first framebuffer captures are exactly equal across runs: 4,612,800 bytes,
SHA-256 `e4e0afb87e540bf0f9c21d519aafdf896327d447311f4673aade44253088f96e`.
This compares saved framebuffer memory, not panel photographs. Kernel acceptance
is still not panel completion. Main-entry timing excludes the launcher, and
navigation-request timing starts at the application handler rather than hardware
finger contact. The two human sessions have unequal sample counts and cadence;
these ranges and medians are descriptive, not controlled P95 or stable speedup
guarantees. A menu-open first frame and a deferred request are excluded from the
ordinary gray-reuse group and retained separately in the detailed analysis.

The cancelled R51 operation completes its 383.301 ms resize before cancellation
is observed in a partial canvas stage. It never imports or publishes a partial
image, and the replacement operation is published correctly. This demonstrates
correct cooperative retirement in the observed phase; it does not demonstrate
physical cancellation inside resize or establish how much CPU that cancellation
saved. Deterministic software tests cover cancellation inside the other long
stages. The earlier R48 superseded operation used different timing and cannot be
subtracted to manufacture a cancellation speedup.

The planned test stop records `operator-stop` / 143, not an X-exit test. The
operator explicitly confirmed normal native picture, dark mode and touch after
recovery. All 17 recorded PID/birth identities, including the independent
watchdog, were gone; native baselines and launcher-lock checks passed, and owned
transports were closed. Exact release was `2026-11-10T07:24:52.406756Z`, followed
by independent absence at `07:25:03.652651Z` and explicit return to the active
WeRead successor chat. No further device operation belongs to this released run.

The final scope is complete for this version, selected software cases and the
supervised offline device workload. Real-account performance, broader firmware
support, battery impact and physical panel completion remain outside this result.
The fixed launcher wait is retained pending a proven readiness replacement.
