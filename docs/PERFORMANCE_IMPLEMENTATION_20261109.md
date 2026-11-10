# Startup, Reader, and Resource Improvements

This historical implementation record covers `0.1.17` through `0.1.19`. The later
[ARM cause and 0.1.21 result](PERFORMANCE_ROOT_CAUSE_20261110.md) records the current
resize/cancellation implementation and its final physical acceptance.

Latest candidate covered here: `0.1.19` (R43), including a pixel-equivalent
single-channel Triangle resize and row-based canvas conversion. Its selected
remote software gates and ARM package checks passed. Earlier software baselines
are `0.1.17` (R34e) and `0.1.18` (R38); R38 added the single-process native
sampler. Scoped physical startup, finger paging, retained sleep, and manual exit
results for `0.1.18` remain separate from the new candidate.

Status: selected R43 software gates and R45 physical first-panel, real finger
paging, and native-restoration checks are complete for `0.1.19`. The operator
confirmed complete correctly oriented artwork, a real page turn with noticeably
shorter waiting, and normal native dark display and touch after the planned
test stop. R44's missed manual window remains a separate historical result.
No general hardware, account, battery, or panel-latency claim is made.
R36/R37 failures retain their original scope. The historical investigation is in
[Performance review](PERFORMANCE_REVIEW_20261109.md), including its unchanged
0.1.16 observations and original timing limitations.

## Implemented behavior

### Startup and result delivery

Font decompression streams each original bundled face to the UI while application
state is prepared independently. All five original faces are registered before
the business UI switches fonts. Startup registration and attachment continue
on bounded event-loop turns instead of waiting 80 ms between every stage. The
maintenance timer remains; software renderers without a rendering notifier keep
the initial yield rather than inventing a first-frame acknowledgement. The
existing low-frequency startup activity indicator and error state remain intact.

Ready App jobs notify the event loop after releasing mailbox locks. Notifications
are coalesced, partial drains rearm the notification, and shutdown stops further
delivery. Worker callbacks retain weak window references. Optional lifecycle
traces record startup stages, monotonic timestamps, and elapsed process time;
UI readiness is distinct from framebuffer submission and panel completion.

The launcher reduces subprocess and pipe overhead in protected native-state
reads while retaining the original two-second watchdog and its original start
point. Optional `native_read_go`, `native_read_ready`, and
`native_read_reaped` markers distinguish the protected read from supervisor and
cleanup time. The unconditional two-second observer/launcher settling interval
has not yet been removed: pipe connectivity and process existence do not prove
native event subscription readiness. A separate private-event barrier candidate
requires firmware-specific verification before adoption.

The existing startup control barrier drains events already queued in the relay;
it does not acknowledge completion of the native LIPC subscription. The 130 ms
private-event experiment used a separate observer and did not exercise the
production source-to-relay-to-owner path or normal document-launcher/window-
manager handoff. Launcher fixtures deliberately omit the fixed sleep, so their
passing results do not validate shortening it. The frozen `0.1.19` candidate
therefore retains the wait. Reducing the native-read deadline would only make
slow reads fail sooner; it would not accelerate a successful read.

### Inventory and transient metadata

Startup no longer hashes the complete local library before showing the requested
route. Inventory workers inspect at most four source pages and 16 metadata steps
per batch, with a soft 25 ms work budget. A single kernel read may exceed that
soft time budget. A chapter is published only after all of its descriptors have
been checked, and one owned SQLite connection moves with the continuation.

Unverified inventory is displayed as checking, not as zero downloads or proof
that networking is required. Existing local detail and reader validation remain
available during the scan. Complete revalidation has a separate clock so that
repeated incremental refreshes cannot indefinitely postpone corruption checks.
Retired tasks and generation changes cannot publish an obsolete inventory.

Startup prepares only the selected route's visible covers; direct reading
resumption does not predecode the catalog. Pinyin preparation is limited to an
initial search route or one deferred background job.

Ephemeral catalog retention targets are 20 scopes, 512 comics, and 32 details.
These are not limits on a user's library or active query. Bookshelf/history,
favorites, anchors, downloads, indexed content, unresolved transactions, queued
intentions, and the active working set retain their own ownership. Cleanup uses
one transaction and preserves protected records on failure.

### Page preparation and persistence

An existing displayed frame remains visible while the next frame is prepared.
Slow work receives a small static indication; first load, missing content, and
errors retain their existing explicit states. Displayed and requested positions
are separate, so a not-yet-presented navigation target is not saved as the
current reading position.

Sleep cancels an unpresented navigation result and clears queued navigation when
a retained frame exists. Resume keeps that source position and reapplies pending
manual display settings. Initial loading without a presented frame remains live.
An already settled frame keeps its complete state, including the diagnostic
render generation; the existing full-state lifecycle comparison remains strict.

Download notifications are matched to the reader's visible content dependencies.
An unrelated completed page does not invalidate useful work, and a valid Ready
result is not discarded merely because a notification arrived during its render.

Receipt-backed rendering shares one immutable encoded read and SHA-256 identity
with decoding and the gray-source cache. Unix reads and decoding happen outside
the global publication lock, followed by receipt and file-object revalidation.
Replacement races permit only one retry; a stale file descriptor cannot revoke
a newly repaired object. Other platforms retain the conservative serialized
boundary. Current complete proofs avoid an unnecessary SQLite write. Source
corruption, resource limits, cancellation, and legacy-original preservation
remain enforced.

Ordinary presented progress uses one actual worker and one replaceable lightweight
pending snapshot. Errors preserve that snapshot and back off from one to 30
seconds. Lifecycle boundaries revoke older writes and synchronously checkpoint
through the existing SQLite transaction fence. Cross-account/episode scopes
must be flushed before replacement; seconds-level timestamps do not determine
write ordering. SQLite durability is unchanged.

### Pixel-equivalent raster preparation in 0.1.19

The reader specializes the pinned `image` 0.25.10 Triangle resize for gray
sources. It retains the immutable crop, target geometry, vertical-then-horizontal
passes, f32 operation order, normalized weights, clamping, and final rounding.
Only the first channel of the reference's four-channel f32 intermediate is
retained. At the same geometry, that allocation changes from
`16 * crop_width * target_height` to `4 * crop_width * target_height` bytes,
a 75% reduction in this intermediate buffer, not total process memory.
Empty, zero-target, and exact-size cases use the pinned generic path; crop color
metadata is preserved. Source validation and reading-anchor policy are unchanged.

Canvas preparation writes opaque RGBA by row and uses exhaustive tone tables
that preserve the existing crop-local Bayer phase and gray quantization. It
does not lower output resolution, change the filter, or alter the refresh policy.
Exact-output coverage includes small exhaustive and pseudorandom crop geometry,
gray values, extreme aspect ratios, metadata, complete frames, and reading
anchors. Optional stage timing retains an independent generic reference in the
explicitly selected diagnostic test.

The [pinned reference snapshot](../vendor/image-triangle-reference-0.25.10/README.md)
records upstream revision `76e57184f22772dad1138e96954e57945406b15e`, source
hashes, derivation notes, and complete MIT/Apache license texts. The ARM archive
includes the required notices and license material. This does not select a
project license.

Generated translations and demo-asset source files are now written only when
their content changes. This avoids causing Slint's imported generated files to
invalidate a subsequent unchanged Cargo check. The observed warm development
check took 0.215 seconds and preserved generated content and timestamps; this
does not claim that a second release/LTO build was measured.

### Resource release and logs

Explicit reader closure clears UI images/models and retires reader work even
when persistence fails. A failed position remains as a lightweight retryable
snapshot rather than an offscreen live reader. Shutdown attempts all independent
cleanup steps and closes job admission before reporting aggregate errors.

Production launcher and application logs each retain at most 1 MiB of current
output and a single backup of at most 1 MiB. Owned FIFO consumers continue to
drain excess output. Natural consumer failures notify the launcher independently
of power events; log I/O failure falls back to discarding diagnostics rather than
blocking the application. Process identity, FIFO type, exact lease cleanup, and
native restoration are covered separately by launcher/supervisor fixtures.
Deliberately killing an entire logging group without subsequent supervisor
activity is outside the exercised automatic-restart scope.

## Verification record

All verification runs on `ssh test-env` with bounded build concurrency and
isolated workspaces. No local verification is authorized. Original failed or
interrupted runs retain their status and input identities.

### Latest R43 identity and checks

The [R43 software summary](../artifacts/verification/performance-implementation-20261109/r43-final-safe/evidence/final-software-summary.json)
and export manifest bind the frozen `0.1.19` sources to these results. The source
inputs remained unchanged during the final run.

| Item | Result |
| --- | --- |
| Source input manifest | `f4d0bf08e149e46630a5bb02d56f050cf720a6c7ae9285f2fa927b0473768a3a` |
| Desktop development executable | 235,103,944 bytes; `63e606e32bdc8c2a91e3aaa02f302950301bce26bdb1cdf60ac8ebfcde10dbb6` |
| Library / binary tests | 558 / 5 passed; zero failed |
| Explicit release diagnostic | One normally ignored performance diagnostic selected separately and passed; zero ignored in that run |
| Guest / reader / startup cases | 5 / 9 / 2 passed |
| Lifecycle | Full reader-state comparison passed |
| Resource cycles | 40 complete close/reopen cycles, 42 actual ready generations; UI image/model release passed |
| Real App page observations | Five forward and five return turns, all retaining previous artwork without a full-screen loading frame; final RGB pixels equal the reference |
| ARM development package | 32 entries and all 10 static checks passed |
| ARM executable | 66,349,816 bytes; `4ac9decde9b28eb42572f5035c311d4d6af2badbec8d5d63e5117092c301cd4b` |
| ZIP | 50,705,707 bytes; `848273e2a42fb1f68f583aff13c37a657847d5050cad57625ccc6e81a5baa331` |

The default library run records one ignored diagnostic rather than describing
all tests as unconditionally executed. Its separate release execution validates
pixel and anchor equality while collecting the observations below.

The [component proof](../artifacts/verification/performance-implementation-20261109/r43-final-safe/evidence/component-reuse.json)
permits scoped reuse of the historical 147 backend, 104 launcher, and 36
fake-service supervisor checks based on their exact component inputs and
normalized dependency graph. These are historical component results, not fresh
whole-application tests. The complete application manifest and package version
differ from R38. The 13 native-capture and three geometry results are not directly
reused after the `build.rs` change, and the old 36 supervisor checks do not cover
the external revised physical harness.

The first R43 runtime attempt stopped at its unchanged-build diagnostic after
a build-profile transition: the generated content and timestamps were unchanged,
but Cargo still rebuilt the App. Its failed record is retained. The next
consecutive development check reported the App as Fresh in 0.215 seconds, then
the remaining runtime and package stages completed. The failure is not relabeled
as a passing check or removed from the evidence.

In the missing-cover and cached-cover resource cases, stable-window RSS medians
changed from approximately 187.7 to 196.9 MiB and 189.7 to 199.0 MiB,
respectively. Stable FD counts remained 33 to 33 and peak threads were 11.
The source cache retained one 4,012,000-byte image within its 32 MiB budget.
These bounded workloads verify the exercised release paths; they do not prove
that every resource path is leak-free or that the allocator returns all memory.

### R43 host performance observations

The real App development-build observations use the same fictional receipt-backed
page fixture and final RGB reference as the historical development observations.
They are separate warm-cache host observations, not an interleaved comparison
of all candidate versions. Shared-host load and the operating-system cache were
not controlled.

| Observation | R43 median of five |
| --- | ---: |
| First forward turn | 460.8 ms |
| Return to the previous page | 202.0 ms |

All ten turns retained the old artwork while preparing the requested frame,
and all final RGB pixels matched the reference. Forward and return turns use
different content and directions, so their difference does not isolate cache
benefit. These observations exclude the Kindle launcher and physical panel and
do not establish cold-device performance or P95 latency.

The separately executed same-candidate release-library diagnostic compares the
generic raster path with the new path using identical sources and settings:

| Fictional input / viewport | Generic median | New median |
| --- | ---: | ---: |
| Input 0 / 1860 x 2480 | 59.287 ms | 25.564 ms |
| Input 0 / 930 x 1240 | 23.724 ms | 9.723 ms |
| Input 1 / 1860 x 2480 | 57.415 ms | 24.599 ms |
| Input 1 / 930 x 1240 | 23.452 ms | 9.410 ms |

Complete rendered pixels, gray statistics, geometry, and reading anchors matched.
The observed reduction is 56.9-59.9% in these four host-library cases. This is
neither an ARM measurement nor end-to-end App, finger, or panel latency.

### Earlier implementation attempts

- Launcher component R1: 97 cases, with one fixture log-collection error when
  opening a directory through `os.fdopen`; 26 physical-supervisor Linux fixture
  cases and both shell syntax checks passed. This is not physical-device testing.
- The accidentally repeated unfixed R2 was stopped by exact process identity;
  it is not counted as a fix or a passing result.
- The fixture helper now examines file type before `fdopen` and closes rejected
  descriptors. The fresh R3 launcher suite has one FIFO-pressure readiness
  failure requiring diagnosis; it is not being dismissed as timing noise.
- Frozen R34b passed 531 application library tests and all five binary tests.
  The driver then stopped at its disk-space boundary before backend/runtime
  continuation. That stop does not invalidate the completed checks, and it does
  not authorize a local fallback.

Later candidate failures remain separate: R34c had a test-only image-size type
mismatch, and R34d's complete lifecycle comparison caught an unnecessary render
generation change on an already settled page. The final implementation avoids
that state change rather than excluding the field from the comparison. The
first fault probe's ten-second recovery window did not cover the documented
30-second retry ceiling; the revised probe observes recovery without triggering
a resume or shutdown write after removing the fault. None of these adjustments
changes the production native-read deadline or the original guest writer-lock
window.

### Final R34e identity and checks

The [software summary](../artifacts/verification/performance-implementation-20261109/r34e-final-safe/evidence/final-software-summary.json)
and safe-export manifest retain the exact inputs, original failures, and scoped
component reuse.

| Item | Result |
| --- | --- |
| Source input manifest | `c19ce668a08f3ec18f85a11243ae9a8abc2cd92c71bf773e0ffedb266479d74e` |
| Desktop executable | `342d04f93dfc61c20a7fc44aeaaf67a1e488641264f9b6dc957b433c02f7a8bb` |
| Library / binary tests | 537 / 5 passed; zero failed or ignored |
| Backend component | 147 passed; exact 74-file component/dependency reuse proof retained |
| Launcher / physical-supervisor Linux fixtures | 98 / 26 passed with exact dependency reuse; no physical result implied |
| Guest / reader / startup cases | 5 / 9 / 2 passed |
| Lifecycle | Full reader-state comparison passed without excluding diagnostic fields |
| Resource cycles | 40 complete close/reopen cycles, 42 actual ready generations; UI image/model release passed |
| Failed progress checkpoint | Genuine new-page write failure, close-time release, and active background recovery passed |
| ARM development package | 28 entries and all 9 static checks passed |
| ARM executable | 66,302,192 bytes; `8fa3107139194a897dc70a806b944b4538f88ff0fcb1e49cec775c8271dcb161` |
| ZIP | 50,677,365 bytes; `30be11cb64825400c66a7189c8fd172c488c9571a89338fdbe11bae313dcaa24` |

Resource FD counts remained 33 to 33, with a peak of 11 threads. Stable-window
RSS increased from approximately 188 to 194 MiB in the bounded workload; this
is not proof that every resource path is leak-free. After storage recovery, the
dirty second-page anchor was saved automatically after approximately 30.05
seconds, and both pending and in-flight flags cleared before planned shutdown.
No additional resume or explicit close was used to make that write pass.

### Same-profile software observations

The [paired observations](../artifacts/verification/performance-implementation-20261109/r34e-final-safe/evidence/candidate-r1-runtime/observation-comparison.json)
use the same development build profile, fictional receipt-backed pages, and
sampling method. Operating-system caches were warm and unrelated shared-host
load was not reserved.

| Observation | Baseline | R34e |
| --- | ---: | ---: |
| Process start to interactive ready, 1 receipt, mean of 2 | 1568.7 ms | 633.9 ms |
| Process start to interactive ready, 100 receipts, mean of 2 | 1567.8 ms | 652.8 ms |
| First next-page gray-source use, median of 5 | 1068.8 ms | 974.5 ms |
| Return to an already viewed page, median of 5 | 762.3 ms | 720.7 ms |

All ten page turns avoided the full-screen loading frame and retained the old
artwork; final RGB pixels matched the baseline exactly. The latency reductions
are approximately 8.8% and 5.5%, respectively, not a large rendering speedup.
First and return turns use different artwork and directions, so their difference
does not isolate cache benefit. Startup now reports inventory as checking until
the background validation completes; it does not claim that unchecked downloads
are complete. These measurements end at software readiness and exclude the
Kindle launcher and physical panel. They do not establish P95 or cold-device
performance.

### Physical handoff status

#### Accepted R45 first-panel and finger check

After the actual WeRead R9 return and explicit operator wake confirmation, the
continuation thread acquired a fresh `performance-20261109-r45` lease at
`2026-11-09T15:40:42.797143Z`. It deployed the exact frozen R43 `0.1.19` package,
kept the production two-second native-read deadline and fixed pre-sampling wait,
and preserved the current native dark mode. The same frozen host harness waited
twelve seconds on the host before one fully verified initial checkpoint; no
device sampling occurred while the operator performed the requested finger turn.

The operator confirmed complete correctly oriented artwork and one real right-
side finger turn, with noticeably shorter waiting. Full live identity checks
bound both checkpoints to PID 24764 / birth 736740600 and the exact R43 ARM. Page
18 advanced to 19 within the original comic, episode, source revision, reader
generation, viewport and settings. The complete corresponding before and after
reader states each match R40's states. The initial checkpoint includes the demo
framebuffer; the turned checkpoint has no new framebuffer capture and does not
establish physical-device pixel equality for that next page.

The [R45 summary](../artifacts/verification/performance-implementation-20261109/physical-r45-safe/r45/physical-performance-summary-r45.json)
records these application-clock observations. An independent host-only review
recomputed the intervals from the saved raw trace and corresponding checkpoints.

| Application-clock interval | R40 / 0.1.18 | R45 / 0.1.19 |
| --- | ---: | ---: |
| Process start to UI ready | 2237.408 ms | 2261.821 ms |
| Process start to first reader-ready | 6492.339 ms | 4215.079 ms |
| UI ready to first reader-ready | 4254.931 ms | 1953.258 ms |
| Page-turn loading to next reader-ready | 4020.949 ms | 1824.137 ms |

The last two intervals were 54.09% and 54.63% shorter in this one pair. They are
single-run software observations, not stable causal estimates, cold-start/P95
measurements, complete launcher timing, finger-contact latency, or panel
completion. Operating-system cache conditions were not measured and the host
harness differs from R40. Both runs use Y8INV, but framebuffer rotate changed
from 1 to 3; hardware scanout/image equivalence is not directly comparable and
no synthetic rotation was introduced to match it. Manual panel and touch
acceptance remains the operator's separate explicit observation.

The two protected helper commands reached ready in 1170 ms and 860 ms, and were
reaped in 1290 ms and 860 ms respectively, within the unchanged original deadline.
Sequence 1 captures native state; sequence 2 publishes the protected orientation
sidecar through the same bounded wrapper. They share the `native_read` trace
labels but are not two full native-state samples. The later
[cost breakdown](PERFORMANCE_COST_REVIEW_20261110.md) explains the distinction.
After
the verified turn, the coordinator deliberately stopped the test. Its
`operator-stop` / 143 result is expected and is not another X-exit or sleep
acceptance result. The operator separately confirmed normal native picture,
dark mode, and touch after restoration.

Native statusbar/awesome baselines and launcher-lock checks passed; all 17
recorded PID/birth identities, including the independent watchdog, were absent
with zero live, zombie, or unreadable identities. Owned host transports were
closed. The exact owner/token lease release at
`2026-11-09T15:45:00.774211Z`, independent absence at `15:45:07.618908Z`, and
explicit return to WeRead are recorded in the
[R45 safe evidence](../artifacts/verification/performance-implementation-20261109/physical-r45-safe/README.md).
No further device operation belongs to this returned run.

#### Earlier physical runs

R44 deployed the exact frozen R43 `0.1.19` ARM and ZIP in an isolated offline
physical-device run, retaining the production two-second native-read deadline.
Its [first-image summary](../artifacts/verification/performance-implementation-20261109/physical-r44-safe/r44/physical-performance-summary-r44.json)
binds the elapsed App clock to the live PID/birth and source checkpoint. The
complete initial reader state matches R40. Process-to-UI-ready was 2252.689 ms,
and process-to-first-reader-ready was 4546.472 ms, compared with R40's
2237.408 ms and 6492.339 ms. The UI-ready-to-reader-ready interval was
2293.783 ms versus 4254.931 ms, a 46.09% shorter single-run observation.

The initial source-cache conditions match, but operating-system cache conditions
were not measured and the host harness differs. R40 used Y8INV/grayscale 2;
R44 used Y8/grayscale 1. Both saved projections obey their own documented
memory-to-EPDC transform and have identical decoded pixels, but hardware scanout
and physical-panel equivalence are not comparable. There is no panel-completion
timestamp, no finger-contact timestamp, and no stable or purely causal speedup
claim from this pair.

R44 has no next-page event or live turned checkpoint. The operator subsequently
reported that they had not seen the page and had not had time to test, so no
new-candidate first-panel, finger-turn, or native-touch acceptance is recorded.
The unchanged 480-second application lifetime ended normally with launcher exit
0. Technical native-baseline and launcher-lock checks passed, all 16 recorded
owned PID/birth identities were absent, and host transports closed. The exact
lease release at `2026-11-09T14:53:54.879530Z` and independent absence at
`14:54:09.147796Z` are preserved in the
[R44 safe evidence](../artifacts/verification/performance-implementation-20261109/physical-r44-safe/README.md).
Technical restoration did not substitute for that run's missing operator
observations. The later R45 acceptance used its own fresh handoff and lease;
it does not retroactively complete R44's manual observations.

R39 successfully started the frozen `0.1.18` ARM under the unchanged two-second
native-read budget. The two native read intervals were 1170 ms and 750 ms from
`go` to `ready`. Its application elapsed times were 2760.582 ms to UI ready and
7929.427 ms to reader-ready. This run used the original expensive readiness
polling and timed out normally before operator interaction was obtained.

R40 used the same application and frozen supervisor but waited twelve seconds
on the host before taking its first fully verified checkpoint. This did not
change the App clock or relax accepted-checkpoint validation. The operator
confirmed complete, correctly oriented artwork and a real finger turn from
page 18 to 19. The live PID/birth, episode, source revision, settings, and frame
capture matched the intended transition. UI-ready was 2237.408 ms and first
reader-ready was 6492.339 ms; the observed UI loading-to-next-reader-ready
interval was 4020.949 ms. These are software states, not measured physical panel
completion. A single pair does not isolate a causal effect of the polling change.

R41 separately verified an actual short sleep/wake at page 18 with the same
PID, birth, source anchor, geometry, settings, and Y8INV state before and after.
The operator confirmed the retained page, direction, touch, and absence of an
application restart. The successful resumed checkpoint did not include a PNG.
A subsequent erroneous `confirm --panel` request required such a PNG and was
rejected; the old harness then generated an operator-stop request. Its exit 143
therefore does not prove that the operator's later X action caused termination.
That tool failure is preserved, independently of the already successful sleep
comparison. See the [R40/R41 scoped evidence](../artifacts/verification/performance-implementation-20261109/physical-r40-r41-safe/scope-summary.json).

The harness now uses untrusted readiness hints only to avoid repeated full ELF
hashing before a candidate state exists. Every accepted checkpoint still runs
the original full identity proof, and framebuffer capture gates are unchanged.
Pure host-side confirmation failures no longer send a device stop request;
actual device-operation failures retain the fail-safe. The focused remote
confirmation, polling, framebuffer, and relevant supervisor checks passed.

R42 then verified X exit in a dedicated shelf-to-Account run with that revised
harness. The operator explicitly selected X and confirmed normal native display
and touch. The launcher exited naturally with status 0 before its 480-second
duration, no `stop.request` existed, and there was no failure-recovery stop.
The trace records shelf and then Account; no exact X timestamp or `quit` trace
is invented. The [R42 evidence](../artifacts/verification/performance-implementation-20261109/physical-r42-safe/README.md)
records native restoration, empty owned-process/transport checks, exact lease
release at `2026-11-09T13:34:57.422906Z`, and independent absence at
`13:35:12.267138Z`. R40/R41 were previously returned with their own exact lease
and absence evidence. Every subsequent device run requires a fresh handoff.

R38 (`0.1.18`) passes 550 default library and five binary tests, 13 Kindle-feature
capture tests including four actual command-lifetime cases, three existing
native tests, 147 freshly tested backend cases, 104 launcher cases, and 36
physical-supervisor Linux fixture cases. Guest 5, reader 9, startup 2, and the
real-process lifecycle case passed. The lifecycle OS-service shim needed a new
capture CLI branch; its old failure and the separately recorded fixture-only
revision remain preserved. The production two-second deadline and full reader
state comparison were not relaxed.

The R38 [software result](../artifacts/verification/performance-implementation-20261109/r38-final-safe/evidence/final-software-summary.json)
binds source manifest
`37bfa3cc441e75e254f520093597098fe6c10e35649ccabf9eb7bcd46ffa9d2e`
to ARM SHA-256
`10ed4069fcec269a9a3c8671c0b3620f200ed17b0d52c800792755aa5e8b2819`
and ZIP SHA-256
`a3afb7e5113ba266f77336e0c48d4f20860a77725076aaee0638bca1037dc808`.
The 66,342,680-byte ARM and 50,694,399-byte ZIP pass all nine static checks for
28 package entries. Earlier desktop timing samples are not reassigned to R38.

After the operator restored connectivity, R36 and R37 deployed the frozen R34e
ARM and unchanged production launcher on Scribe2022 / 5.19.5. Both failed the
original two-second native-read window before application readiness or display
takeover. There was no `native_read_ready` event; `go` to `reaped` was 2000 ms
and 2030 ms respectively. The second run used the independently verified
equivalent wrapper read optimization. Neither failure is counted as a pass.

Direct read-only native getters measured 20-60 ms, while one whole executable
hash took 1460 ms. Those separate observations do not identify the cause of the
bounded-read failure. A private-event experiment on the actual firmware received
its marker in 130 ms, with no native power events and independently confirmed
process-group cleanup. It proves only that private round trip, not readiness of
the production relay or native window-manager handoff.

Both watchdogs restored their recorded native baselines, and the operator
explicitly confirmed normal native display and touch after both attempts. The
R36 window was released with exact owner/token checks at
`2026-11-09T05:47:45.265658Z`, the directory was independently confirmed absent,
and the device was explicitly returned to WeRead. The
[safe diagnostic export](../artifacts/verification/performance-implementation-20261109/physical-r36-r37-safe/summary.json)
retains these failed runs and their scope without raw tokens or firmware files.

R38 subsequently consolidated the same ordered native reads and consistency
checks into one launch-owned Rust helper. It retains the outer two-second
deadline, validates the read context before native I/O, and preserves the
physical-wrapper custody checks. Its separate results are recorded above.

The earlier R35 connectivity-only attempt is retained below as historical context.

After WeRead released its window, the coordinator acquired the fresh
`performance-20261109-r35` lease for thread
`01a1bc89-76b7-7d72-8346-4d2d24a8f94a` at
`2026-11-08T22:38:56.378784Z`. The guarded preflight did not succeed; a separate
bounded connectivity check reported that the device was unreachable. No package
was deployed, no application was launched, and no display/input ownership was
taken. The operator was asked to wake and unlock the Kindle and keep Wi-Fi
connected. Fresh native, touch, panel, startup, and page-turn acceptance must not
be inferred from software results or the previous application's observations.

The unused window was returned with exact owner/token checks at
`2026-11-08T22:57:42.766907Z`, followed by an independent absent-directory check
and explicit notification to WeRead. The
[return context](../artifacts/verification/performance-implementation-20261109/physical-r35-unused/unused-return-context.json)
states why no restoration action was needed: this run never launched an App or
took display/input ownership, and its non-multiplexed device transport commands
had ended. The
[release record](../artifacts/verification/performance-implementation-20261109/physical-r35-unused/lease-return-unused.json)
does not represent a new human panel/touch confirmation. A later physical run
requires a fresh handoff and token. Transient test-env SSH banner delays during
the return did not change the final successful release result.
