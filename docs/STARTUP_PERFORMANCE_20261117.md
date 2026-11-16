# Startup performance: 2026-11-17

BiliComics moves font inflation to the build, shares immutable font payloads,
coalesces inexpensive attachment stages, and reduces input-discovery and launcher
overhead. The remote application, backend, and launcher tests, ARM builds, and
independent static package checks have passed. Eight desktop cases and ten
exact pixel comparisons also passed. In five matched, instrumented Scribe warm
startup pairs, median software readiness improved from 9.089 to 5.986 seconds
and first Shelf SEND acceptance from 9.407 to 6.850 seconds. These are BiliComics
results from one isolated offline demonstration batch.

## Implementation

The adaptation follows Aidoku commits `6660e32`, `e141d44`, and `4ce1fd1`.
Their complete identities, nine original source snapshots, and original license
are retained in the [startup provenance record](research/aidoku-startup-20261117/README.md).
Aidoku's timings remain Aidoku results.

BiliComics already wrote its initial fill and complete startup scene before the
initial GC16 submission. A missing draw already kept the full-redraw request
pending. Its native-reader and observer ownership checks also already used a
builtin process-stat reader. These existing mechanisms are preserved and are
not counted as newly implemented optimizations.

The five existing canonical gzip faces now expand during the build, retaining
the 64 MiB per-face bound and gzip checksum validation. Runtime registration
shares static ELF slices through Fontique handles, including the 22,040-byte
bootstrap font, without application-owned full-payload copies. Font families,
weights, outlines, ordering, cancellation, and delivery errors retain their
prior contracts. Font parsing and raster caches still incur runtime work.

After the final font registers successfully, startup checks prepared metadata
in the same driver invocation. Activity pulses remain available during actual
font or metadata waits, while final attachment no longer advances the loading
animation. The one-time `attach` phase label remains. Cheap attachment stages
share a 4 ms budget checked between stages; individual stages are not
preempted. A partial 4 MiB cover batch or suspended lifecycle yields without
consuming the next phase.

A follow-up dictionary check found that Shelf synchronization does not call
`all_candidates`. Non-search startup already schedules dictionary warmup with
a 250 ms delay; explicit Search startup prepares it on the worker. No extra
dictionary optimization or Aidoku dictionary timing is claimed in this batch.

Native-reader completion, reader retirement, and observer startup now poll at
20 ms with adjusted counts preserving their nominal 2/1/2-second budgets.
The initial window-manager state and birth query prefers one builtin stat read
and preserves the original fallback on failure. Process names containing spaces
and parentheses are covered. PID birth, nonce, generation, watchdog, and native
recovery checks remain in place.

Input discovery first reads a bounded, complete sysfs roster. Hints bind each
character node's major/minor, filesystem, and inode; opened candidates still
undergo the original ioctl checks and a complete roster recheck. Missing,
malformed, incomplete, changed, or unsuccessful hints use descriptor scanning.
Multiple portrait sources remain ambiguous. Touch transfers its validated
descriptor into the existing grab/state/synchronization sequence, preserving
the legacy symlink fallback. No event number or pose is cached across calls.

## Frozen source and remote checks

| Item | Baseline | Candidate |
| --- | --- | --- |
| Code | `06ad90b02edb28db43abeed3031932a91dc2f8df` | `585e571b422e9586f1e53a417cbaf330a9ec473f` |
| Compiled inputs | 540 | 542 |
| Input inventory SHA-256 | `e9eafe504fcd206aa46dcb257f60329d6d49be146d69915aedd8b031b36833a4` | `33bda43e34f74c7e73058880b4a1ae7a8d4b00d3ec0ae9a1ee22a074b59beb28` |
| ARM application SHA-256 | `ef3f7c352ff39e08ca14a5f040ada158b3c4d895650c451e2bfb99fb9f46261b` | `f5f958b219b8faef1776ba14f50b9d483f628f792ad43503fa46cf0d7879714f` |
| ARM JPEG worker SHA-256 | `e39d7a897f13ce53412f42040d124a106634322899df515b5bc2e96b781914cd` | `12c8988c6a93cade9923d54bef784b774e13cb5e3b042f2e72ea5ecb6b897beb` |

The candidate development ZIP is
`6669b93445c2bc6cfd729d014ea2c3d156f40f307cfb233ae5ca09e7c39fc985`.
The JPEG worker source is unchanged, but its rebuilt ELF identity differs.
Both variants use Rust 1.99.1/Cargo 1.99.1 and the same release profile:
ThinLTO, one codegen unit, and stripping. Verification runs on `test-env` with
six Cargo jobs, incremental compilation disabled, and one test thread.

The corrected verification run passed with both compiled-input inventories
unchanged. Its candidate ARM build completed in 217.898 seconds. The baseline's
own static package checker passed 15 checks for 39 entries; the candidate's
independent package checker passed 16 checks for 42 entries.

The corrected remote run passed 874 library tests, with zero failures and four
ignored tests; eight main-program tests; 236 backend tests; and 129 launcher
tests. Package regressions passed eight startup, five spatial-dither, and three
native-media tests. New coverage includes complete canonical font-byte equality,
static sharing, interrupted delivery, partial attachment, suspended startup,
input-roster integrity, and launcher process-name/budget behavior.

The first application run recorded 866 passes, eight lifecycle failures, and
four ignored tests: its temporary directories failed the private-directory
guard. The verification runner had omitted the standard `umask 077`. The next
run set that mask and a private `TMPDIR`; all tests above passed without changing
product code or weakening the guard. The original failure record is retained.
The source-bound [software evidence](verification/startup-20261117/README.md)
retains the original attempts, compiler profile, package checks, and inventories.

The [desktop startup supplement](uiux/baselines/2026-11-17-startup/README.md)
preserves eight isolated normal/failure cases on PW6 and Scribe at scale two.
Ten comparisons match every RGB pixel, including stable actual X11 shelves.
Startup precedes worker admission, delayed preparation changes the activity
indicator's real pixels, and controlled initialization failure retains the
error page. The repeated runner's eight process groups closed without live
members. Its earlier `/proc` visibility failure remains recorded separately.
A [six-frame assistant inspection](uiux/baselines/2026-11-17-startup/assistant-visual-review.md)
found no missing Chinese characters, clipped controls, or visible regression.
Human acceptance and physical-device results are not inferred from these host
checks. The controlled delay makes this a visual check, not a startup benchmark.

## Physical comparison

Batch `startup-20261117-ee788825` completed twelve normal launches in one device
boot: one warmup per variant, then five alternating AB/BA pairs. Each launch
used a separate copy of the same closed, 37-file offline demo snapshot. Both
variants used the same compiler profile and native Scriptlet/appmgr route,
including the production launcher's ordinary validation and detach path.
The temporary installation and launch instrumentation are separately identified
in the [physical evidence](physical/startup-20261117/README.md).

| Request endpoint | Baseline median (range), seconds | Candidate median (range), seconds |
| --- | --- | --- |
| First startup SEND accepted | 5.279 (5.263-5.414) | 4.432 (4.273-4.504) |
| Software `ui_ready` | 9.089 (9.065-9.197) | 5.986 (5.772-6.747) |
| First Shelf SEND accepted | 9.407 (9.354-9.495) | 6.850 (6.545-7.203) |

The readiness difference of medians is 3.103 seconds (34.138%); the first Shelf
submission difference is 2.557 seconds (27.185%). Median paired reductions are
3.211 and 2.645 seconds respectively. All five pairs improved at both endpoints;
the slower fourth candidate remains included.

Each endpoint uses `CLOCK_MONOTONIC`. The instrumented request precedes the
native launch command's exec; it is not a physical finger tap. The business
endpoint requires a rendered frame whose Slint render begins after `ui_ready`
and uses its successful SEND return timestamp. All twelve business submissions
also have matching accepted/completed markers and observed driver completion,
recorded separately. No optical panel completion was observed, and first Shelf
submission does not establish that all background covers are ready.

Observed stage medians show native initial event-loop setup at
570.482 -> 24.945 ms, font-worker wall time at 1414.446 -> 3.063 ms, and
application attachment to readiness at 1817.990 -> 304.040 ms. These spans can
overlap and must not be added as independent startup savings. At the first
business observation, median VmRSS was 124.76 -> 54.52 MiB and the kernel's
then-reported VmHWM was 124.79 -> 54.54 MiB. These are samples, not continuous
whole-session peak-memory measurements. The ARM ELF grew from 64.42 to 70.83 MiB.

Not every interval shortened. The median of each launch's actual ready-to-SEND
interval increased from 291.541 to 772.638 ms; the first business Slint render
wall median increased from 205.664 to 367.991 ms. The preceding-update wait
within that frame rose from 0.004 to 327.998 ms. The existing traces cannot
isolate dictionary warmup, scheduling, storage, or glyph-cache causes. Candidate
04 crossed the 4 ms attachment budget after Route and yielded across an
825.314 ms GC16 transaction whose recorded FNV64 RGB fingerprint matched the
preceding loading frame; that intermediate image was not independently saved.
Dictionary warmup was queued in the later Views stage and therefore
cannot explain this earlier transaction. This intermediate refresh
occurred in five baseline samples and one candidate sample. It remains a future
optimization opportunity, rather than a removed or excluded observation.

Profiling was enabled equally for both variants. RGB fingerprinting alone
accumulated median CPU time of 545.256 -> 303.786 ms before the first business
SEND, including that frame's own fingerprint. These diagnostic costs are part
of this experiment; the measured
percentages are not uninstrumented-release guarantees. Request to Rust process
entry still takes a median 4.444 -> 4.115 seconds. Detailed production launcher
timing files under the temporary data directories were not collected; this
record cannot subdivide that remaining delay from Aidoku measurements. The
launcher source retains a fixed two-second wait before its startup native
sample; changing that established handoff delay was not evaluated in this batch.

All twelve recorded cases retained the same opened input nodes, one application
PID per lifecycle, and no recorded suspend/resume. Early and scheduled
post-ready framebuffer memory samples match across the last pair, as do all
final logical snapshots. Their lossless memory projections are distinct from
optical photographs or instantaneous snapshots at SEND. The scheduled samples
targeted ready plus one second, but their actual capture requests arrived at
ready plus 3.783 seconds for baseline and 3.892 seconds for candidate. They do
not establish that the scene was stable at the requested one-second target.
The primary assistant inspected the candidate projection
and the private native-return projection without finding a visible blocker.
Native Home content remains private.

Native WM identity/statusbar, process retirement, lighting, the exact four
selected installation-file identities or absences, boot/input roster, temporary
stage removal, owned transport closure, and exact owner/token lease release
all passed before analysis. Whole-tree installation integrity is not claimed.
Fresh human confirmation of native touch remains pending; the earlier
CPU-resize batch's confirmation is not reused. No new touch, rotation, sleep,
cold-boot, other-model qualification, battery result, or published release is
established by this warm-start experiment.
