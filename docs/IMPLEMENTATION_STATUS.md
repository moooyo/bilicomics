# Implementation Status

The user-supplied BiliComics Kindle design has been implemented as an independent Rust and Slint application. The original source provenance is recorded in [Design source](DESIGN_SOURCE.md); the initial handoff remains historical context.

## Live portrait orientation: 0.1.16 scoped verification completed

The [live portrait update](LIVE_ORIENTATION.md) implements automatic Up/Down
holding changes while the application remains active. The user accepted the
tested hardware-event sensitivity and selected two-way behavior after comparing
native Home; four-way expansion was cancelled. Existing manual application
rotation and landscape preferences retain their semantics. No artificial
stability/debounce timer or periodic idle orientation polling was added.

The explicitly calibrated observer shares the existing control/gyro/touch poll
loop. Fresh same-layout GETVAR resolves scanout, while holding controls the
residual pixel and raw-touch offsets. Pointer cancellation, generation changes,
the full rewrite/GC16, and a loss-aware final drain prevent stale contact replay.
A submitted frame awaiting `SYN_REPORT`/snapshot waits on descriptors without
repainting or exiting; control and new direction changes remain available.

R31 passed 474 application library tests, nine reader cases, two startup cases,
and the controlled real-process launcher lifecycle. Its native backend/ARM build
failed on a parser comma, and that original failure is retained. After the parser
and input-loss/waiting corrections, R32 passed 147 backend tests and a 28-entry
ARM package with nine static checks. The R31 application results retain their
original manifest and are not described as a rerun of R32's whole source.
R32's source-input SHA-256 is
`7cdc5e0ad5e7fb9fecca7185e71d87000c4ff18548c46298394340b7b86e3bdd`;
the ARM SHA-256 is
`7e31def888b037f83cabc3caa33d2b097a057c0e275a329fcffd81b98348aa65`,
and production ZIP SHA-256 is
`c0fd20e97abd294fc0a254682fac58b8f38abac49bb7afd86ca8e9feac594a1d`.

The [R33 physical run](../artifacts/verification/live-portrait-20261109/physical-r33/run.json)
used that ARM with its separate five-second test launcher on Scribe2022 / 5.19.5
and fresh offline demo data. The user confirmed active Up/Down artwork rotation,
retained reading, requested controls, X exit, and normal native direction/touch.
Native-live traces retain PID 9454/birth 729057507, holding U/D against scanout D,
and residual/touch 180/0 and 0/180 pairs without sleep/resume. Readiness records
retain page 18; the exact user-reported right-tap 18-to-19 step is not assigned a
numeric regression pass. Exit/recovery and native baselines passed, all owned
processes/connections closed, and the [exact lease was released](../artifacts/verification/live-portrait-20261109/final-physical-summary/public/lease-return-r33.json)
at `2026-11-08T18:34:50.124133+00:00`, verified absent, and returned to WeRead.
Both peer projects received the final two-way scope. The inherited active-turn
work is complete within this exact scope; no broader hardware/account claim is
made and the acceptance allowlist remains empty.

## Wake orientation: 0.1.15 scoped verification completed

The [wake orientation update](WAKE_ORIENTATION.md) now combines an independent
read-only native-portrait evdev holding direction with the same-launch GETVAR scanout
calibration and residual pixel rotation. Startup retains its original calibrated
mapping. Legacy setters and absent/unrecognized-sensor fallback remain intact;
unknown recognized values or multiple sensors fail explicitly. A current kernel
read does not by itself prove physical direction freshness at wake.

The later R29 candidate recognizes the exact measured `bma4xy_feature` sensor
alongside kx132. Public native/sensor pairs associated raw 16 with D/rotation 1
and operator-confirmed raw 15 with U/rotation 3; the independent trace tag is
now `evdev-native-portrait`. R29 passes 120 backend component tests and a
28-entry ARM package with nine static checks. Its ARM source-input manifest is
`c4ab975f072a218ea72e9523b069540ac98e46fca83891412f9fde2dfa757114`,
and its 66,077,248-byte ARM SHA-256 is
`3907fa54fb4d802340d1c4a0cbb866587805d600a350b908ea5163b36bf1fdac`.
The production ZIP SHA-256 is
`88a049d057c07292252a699d54805e4ef123f2e26f18f0e595f20e4b23a7d40a`;
the private five-second test package remains separately identified in
[wake orientation](WAKE_ORIENTATION.md).

After a new user handoff, the R28 reader used that R29 candidate. The operator
confirmed complete page 18, actual finger paging to 19, and physical sleep with
USB-up to USB-down wake directly to correctly oriented page 19 without native
Home. The same PID/full source anchor comparison passed, and the resume trace
used `evdev-native-portrait` with holding U/scanout U and no fallback. The operator
did not perform the exact first-post-wake 19-to-20 step; that case is untested.
The 240-second timer ended R28 normally with exit 0 and completed recovery.

At the user's request, a [new R30 process](../artifacts/verification/wake-orientation-20261108/physical-r30/run.json)
restored page 19 from only the stopped
R28 offline demo SQLite state. The operator confirmed correct same-page reading,
additional sleep/direction changes, and working reader-menu/details/bookshelf/
Account navigation. Three resumes retained PID 11483/birth 728542589 and page 19
with independent U/U, D/D, U/U holding/scanout records. The operator selected X
and confirmed normal native direction and touch. Both runs have exit-0,
completed-watchdog, native-baseline, and absent-launcher-lock recovery results.
The [final return guard](../artifacts/verification/wake-orientation-20261108/final-physical-summary/public/final-return-guard.json)
records closed BiliComics processes/transports. The exact R28 lease was
[released](../artifacts/verification/wake-orientation-20261108/final-physical-summary/public/lease-return-r28.json)
at `2026-11-08T17:01:57.270054+00:00`, verified absent, and explicitly
returned to WeRead. The inherited wake-direction task is complete for the scoped
Scribe2022 / 5.19.5 / offline-demo checks, with the exact page-20 step recorded
as untested rather than assigned to the accepted post-wake controls/navigation.

R26's selected `test-env` pipeline passes 474 application library tests, nine
reader cases, two startup cases, the controlled real-process launcher lifecycle,
and ARM packaging with nine static checks for 28 entries. Its source-input
manifest is `afaea670dd00a62cd22e7e1d041c225a92f907bea58c4de6aa88b827f0d9b638`.
The ARM is 66,076,992 bytes, SHA-256
`84371072f66a2c5f84d3ecbdad84e5f5b39e6266da40f7378675146637f5d5d5`;
the production ZIP is 50,574,349 bytes, SHA-256
`e63fc240a0afa0f6db45c924ae710eef80044907fe219fafb13bbe44feac799b`.
The separate R26 backend test build failed `E0432` in a test-only `Rgb8Pixel`
import. R27 corrected that import and passed 119 backend tests with aligned
dependencies; the runtime and R26 ARM were unchanged. Its corrected component
result is not reassigned to the entire R26 source manifest. Exact evidence and
the retained failure are linked from [wake orientation](WAKE_ORIENTATION.md).

R24's native-only operator cycle was inconclusive after a WM read-gate failure.
R25 completed its 80-second probe but recorded only unchanged U/3 baseline
samples without an operator cycle. Its separate three Linux namespace fixtures
passed. Owned groups and device connections closed; the exact original lease
was released at `2026-11-08T16:06:04.726750+00:00`, verified absent, and Kindle
was returned to WeRead. That historical R24 return is distinct from the newer
R28 claim and completed supervised reader observations above. The R26/R27 results
retain their original source scopes, and the allowlist remains empty.

`test-env` is shared for parallel isolated work with bounded build concurrency;
there is no whole-host completion or loan gate. Physical Kindle ownership alone
requires the coordinated exclusive handoff and shared lease.

## Historical native portrait handoff: 0.1.14 scoped results

The [orientation correction](KINDLE_ORIENTATION_RESEARCH.md) separates opt-in
native scanout pixels/render offset zero from native touch orientation
Up=0/Down=180, preserving legacy setter behavior. The earlier R21 core phase
passed compilation and 474 application library tests. After correcting explicit
positioning in the corner test fixture, the subsequent frozen-source continuation
passed 110 backend tests and built the 0.1.14 ARM package. It retained the earlier
application core result without repeating that phase. The final package passed
nine static checks for 28 entries; the newer candidate does not inherit the
0.1.13 launcher's or physical harness's earlier test counts. The initial R21
formatting-scope failure and the subsequent corner-fixture failure remain
historical failures in the [task state](../artifacts/verification/touch-input-20261108/task-state.json).

The recovered [final driver record](../artifacts/verification/wake-orientation-20261108/recovered-r21/portable-host-r21-final/driver-summary.json)
and [package report](../artifacts/verification/wake-orientation-20261108/recovered-r21/portable-host-r21-final/arm/package.json)
identify ARM SHA-256
`ba47f3c25bb676900709d0cbaf32d3fb3cce345d57cd8fd025075320a5f09628`
and archive SHA-256
`7871cb0746f54314165c452795710fc525910243db59cab3fd3f4ecf19a99d79`.
The archive is 50,572,397 bytes and its ARM executable is 66,073,152 bytes. The
physical runs used a separately identified private launcher copy with the
user-selected five-second native-read budget, not the production archive's
unchanged two-second budget.

On one Scribe2022 / 5.19.5, the user confirmed [R22](../artifacts/verification/touch-input-20261108/portable-physical-touch-split-r22/run.json)
USB-up startup direction, My / Screen and System navigation, the upper-right X
exit, and normal native Home direction and touch. [R23](../artifacts/verification/touch-input-20261108/portable-physical-touch-split-r23/run.json)
confirmed correctly oriented immersive artwork and actual finger paging from
18 to 19. The same process retained page 19 and its complete source anchor at
both suspend and resume. The subsequent menu actions and page 20 were explicitly
operator initiated, not unintended input. Sleep-time direction change remains
unaccepted: after changing to USB-down, the user returned to the same page but
the display direction stayed unchanged. The later live-anchor comparison stopped
that run; it does not invalidate the separate suspend/resume anchor match.

Native control and touch were confirmed restored after R23. Its exact
[lease release](../artifacts/verification/touch-input-20261108/portable-physical-touch-split-r23/lease-return.json)
and [closed-transport receipt](../artifacts/verification/touch-input-20261108/portable-physical-touch-split-r23/transport-closure.json)
preserve the original owner and run. The historical device was returned to
WeRead. These scoped offline observations do not establish general hardware
support or live-account behavior; the repository acceptance allowlist remains
empty.

The newer continuation, R24/R25 probes, corrected backend component tests, and
0.1.15 automated candidate are recorded in [wake orientation](WAKE_ORIENTATION.md).
The historical 0.1.14 observations above retain their original source, package,
owner, and behavior scope; they are not reassigned to that newer implementation.

The historical 0.1.13 candidate and R20 remain distinct. R20's five-second test
copy reached startup after a 2.53-second native sample but displayed an inverted
Bili scene while held USB-up. Its user-selected X exit, native recovery, exact
lease release, and return to WeRead passed; its orientation failed. That result
is not relabeled by the later 0.1.14 checks. Earlier 473 library, 106 backend,
90 launcher, 21 canonical harness, and receipt-fixture results retain their
0.1.13 or separate supervisor scope.

## Wake continuity and settings: 0.1.9

The subsequent [0.1.11 close navigation](EXIT_NAVIGATION.md) places an X in the
upper-right Account header on home and child settings while preserving local
child-page back navigation. Existing IconButton semantics and safe quit remain
in use, and the background X is disabled while the recharge sheet is visible.
The candidate passes 458 library tests, five real XTest toolbar cases, six
reviewed Account captures, and nine static checks on its 28-entry ARM archive.
Earlier controlled matrices retain their candidate scope; physical-device
acceptance was not repeated for this UI change.

The inversion setting, its action/model bindings, and the saved preference have
been removed. Existing JSON containing `invert=true` still loads other
preferences and no longer writes that key. Native display-mode compensation
remains internal so comic pixels retain their original meaning under `Y8INV`.

The 0.1.8 native wake implementation uses a sequence-bound capability request
and a fresh read-only kernel format observation after confirmed native
dismissal. A launcher correction accepts inherited SGID on an otherwise private
0700 directory. On the scoped Scribe2022 / 5.19.5 run, the user confirmed direct
same-page wake without native Home, normal colors, and intentional subsequent
menu and page input. Native dark Home and touch were restored, connections
closed, and the exact device lease released.

The final 0.1.9 candidate passes 458 library tests, 23 design captures, eight
pointer cases, nine reader cases, two startup cases, the real-process lifecycle
case, and three targeted settings captures. Its new ARM package passes nine
static checks. Component mappings retain the separately executed 93-backend and
74-launcher results; the newer whole binary was not used for a second physical
run. See [Wake continuity and settings](WAKE_CONTINUITY_AND_SETTINGS.md) for
exact identities, original failures, direct observations, and evidence limits.

## Resource lifetime and performance: 0.1.6

All nine authorized resource-review projects are implemented: shared actual-work
admission, recoverable cover misses, explicit reader UI-image release, verified
gray-source reuse, safe owner/task retirement, bounded and durable event delivery,
cover disk eviction, visible-view/SQL/idle maintenance, and native process/asset
lifetime improvements. Sleep retry work uses scoped background checkpoints and
the native control queue while retaining the live reading frame.

The frozen 419-input candidate passes 437 library tests, 62 standalone native
backend tests with matching final component inputs, three Kindle-feature host
tests, the existing desktop matrices, and a real SignalBridge storage-failure
recovery case. Two isolated resource cases each complete 20 close/reopen cycles
with 21 actual ready generations and empty global reader-image references after
every close. Official-module signature comparisons and both signers' controlled
45-page production-adapter acquisition pass. The 28-entry ARM development package
passes all nine static checks; it was not run on a Kindle.

See [Resource and performance implementation](RESOURCE_AND_PERFORMANCE_IMPLEMENTATION.md)
for exact identities, cache/budget semantics, evidence, retained diagnostics, and
limits. The older zero-ready cancellation sample, live-service uncertainty, and
physical-device results keep their original scopes.

## Download completeness: 0.1.5

The client now expands an active retained subset when a whole-index request is
submitted, rejects malformed chapter directories, and preserves selections
larger than 256 chapters across authentication return. Completed tasks require
every bound page receipt and file to pass the final whole-index check. Encoded
images receive bounded complete validation before a sealed proof can enter the
durable receipt/history path. Old malformed originals can be repaired only with
matching byte evidence; unsupported or over-budget originals remain retained
without being advertised as complete.

An explicit whole-chapter acquisition rechecks current permissions and the
source index even when an older cache exists. New revisions preserve old
objects and explicit pinned tasks. The frozen 390-input R2 candidate passes 304
library tests, nine reader cases, two startup cases, a controlled live-reader
lifecycle case, five corrected guest fixtures, two subprocess-service/startup
cases, and the controlled production adapter's complete 45-page acquisition and
readback. The original failed runs and the loading fixture's schema-lock repair
retain separate evidence.

The 0.1.5 ARM development package passes 28-entry static verification and retains
three source warning diagnostics. It has not been run on a Kindle. Real-service
full-versus-preview coverage requires separate same-account official-client
comparison. See [Download completeness](DOWNLOAD_COMPLETENESS.md) for precise
source, file, selection, permission, and verification boundaries.

## Immersive reader: 0.1.4

The user's subsequent full-page request now uses the entire reading window.
Whole-page fit preserves the complete image and its aspect ratio. Quiet reading
has no persistent progress footer, page badge, panel locator, strip overlap rule,
or spread annotation. Center tapping reveals contextual header and progress/tool
overlays without changing the image raster or source anchor. Thumbnail and
chapter model updates retain existing input targets across background loading.

The final R5 source passes 231 library tests, nine reader cases, five offline
guest cases, 16 immersive pointer cases, two startup cases, and the controlled
real-process retained-reader fixture. Selected final screenshots received manual
visual review. The fresh 0.1.4 ARM development package passes all 22 allowed-entry
and static ELF checks, retaining two compiler-wrapper warnings. The same candidate
subsequently passed [scoped physical checks](PHYSICAL_IMMERSIVE_VERIFICATION.md)
on one first-generation Scribe with firmware 5.19.5. User observations, live
process/anchor comparisons and framebuffer evidence cover startup feedback,
unobstructed artwork, contextual controls, physical sleep/wake and the first
post-wake finger page turn. Earlier sampling and annotation interruptions remain
recorded separately. Native Home and touch were restored and the device was
explicitly returned to WeRead. Exact source, binary, package and remote evidence
identities are in [Immersive reader](IMMERSIVE_READER.md).

## Supervised physical check: 2026-11-05

The user authorized and assisted an isolated offline run on a first-generation
Scribe with firmware 5.19.5. The fresh 0.1.3 candidate retained application source
manifest `19e4eeadb21394d5481487e3391a2a0dc46a3dd678525e2d01c6dafc55d44bdb`,
ARM executable `fa819c514400c84be113a2e6d4feeeb2931bf41014bc63a16098736e5f87fbfb`,
and ZIP `f5ba2203c7342b00c5afd639695dab48bd5476fc849197b0c8b3d80ece4136d1`.
The [physical report](PHYSICAL_VERIFICATION.md) records bounded startup and exit,
real finger input, physical sleep/wake retaining PID and source anchor, and the
first post-wake page turn. The first reader run's timed-exit interference remains
inconclusive; the longer retry passed. Native control and touch were restored and
the exclusive lease was explicitly returned to WeRead. Real-account and download
behavior retain separate acceptance. The later 0.1.4 immersive UI does not inherit
this older candidate's physical result.

## Subsequent client download update: 2026-11-05

The [download performance update](DOWNLOAD_PERFORMANCE.md) prepares up to 20
missing image grants per request and reuses their independent transient keys for
bounded parallel image acquisition. It preserves per-page receipts and verifies
cache reuse, cancellation, account boundaries, and task priority. Metadata
preparation now shares admission capacity with page work, and blocking page
validation/commit retains its permit through cancellation.

The final remote snapshot passes **222 library tests**, **two controlled recovery
integration tests**, and both fixture-signing and official-WASM batch benchmarks.
The controlled 45-page native-signing comparison reduces ImageToken requests
from 45 to three and median elapsed time from 1,268.405 ms to 742.208 ms, with a
fixed synthetic 5 ms CDN delay and no added token delay. It compares two source
contracts under the same manager. It does not establish physical-device or live
service throughput. Exact source, executable, and timing records remain in
`artifacts/verification/download-batching-20261105/`.

At that update stage no replacement ARM package had been built for the cookie
and download changes. The later combined 0.1.3 candidate and supervised physical
checks are recorded above. Earlier archives retain their own source identities.

## Pre-device verification: 0.1.3

The 2026-11-04 development candidate adds signing-query NUL rejection, persistent touch-coordinate restoration after sleep, and reproducible isolated remote verification. The supplied design and UI sources are unchanged. The user required testing to stop before any physical Kindle connection or deployment.

The desktop regression passed **149 library tests**, **23 default/paused captures**, **8 pointer cases**, **9 Reader cases**, **5 synthetic guest cases**, **2 startup cases**, the real-process retained-Reader case, and **28 launcher fixtures**. All 23 default captures also received manual visual review. The [suite summary](../artifacts/verification/predevice-20261104/final-regression/evidence/20261104T060126Z-b39e9f39/summary.json) retains the source manifest `899e85f81f219a371ca34778994ae4d0f1a8b734599223299a52df33a2733083` and desktop executable `48a4de7b9201dc4cdeb43f90297c32d7cfac20f5f8b96bd47b820f53311b7370`. Those results are not reassigned to the corrected ARM build.

Additional checks passed the actual official signing WASM, seven independent Go/JS comparisons, controlled download-process interruption and damaged-page repair, and non-demo offline App continuation into a new process. A separate three-request anonymous check successfully read labels, obtained an anonymous device context, and read/decrypted a signed public category page. It used no account session, comic page, history mutation, purchase, or recharge. Unknown transaction startup checks use synthetic durable intentions and do not establish complete UI reconciliation or real delivery.

The corrected Kindle dependency passed **34 tests**, including all seven new input regressions. The independent Scribe 5.20.1 guest passed **seven native UI cases** and **five ARM signing cases**, with all seven official signature comparisons matching. The same process retained page 19 across sleep/wake and the original first tap advanced to page 20. The [corrected candidate scope](../artifacts/verification/predevice-20261104/corrected-candidate/evidence/candidate-scope.json) changes only the optional Kindle dependency's touch source and patch record; the application, protocol, and desktop runtime inputs remain unchanged.

The selected [corrected 0.1.3 development archive](../artifacts/verification/predevice-20261104/corrected-candidate/bilicomics-kindle-0.1.3-dev.zip) is **49,927,231 bytes**, SHA-256 `58d94bad279ee3a4bb26abbaa102017d39753a8caa9f6391466e6ab4f39db0eb`. Its static ARM EABI5 hard-float executable is **64,632,960 bytes**, SHA-256 `6f178d7b0c4d2d4334fe7d0136b55ad65334388fed5beb271f599086bf2f3fff`, built from source manifest `a589d72383a8d9bfd01a0a2f2f27b1e9eacf2f1c096533c4129da89338c009af`. All 22 allowed archive entries and static ELF checks passed. Physical testing remains held for the user, and the repository allowlist remains empty.

The [initial 0.1.3 development archive](../artifacts/verification/predevice-20261104/final-regression/bilicomics-kindle-0.1.3-dev.zip) is **49,924,263 bytes**, SHA-256 `cf996524c978118c598615268c611a9270312edaea80019edb8e8b22973089cb`. Its static ARM EABI5 hard-float executable is **64,629,504 bytes**, SHA-256 `f6a88d9d14c34a483ac3c2fcf020e817b72ce3b08684cf409c45ae9ec19dc49d`. All 22 allowed entries, CRC, content digests, modes, timestamps, and runtime-data exclusion passed. Two recoverable compiler-wrapper probe warnings remain in the preserved ARM build log; this is not a warning-free build claim. Firmware testing subsequently reproduced incorrect first-touch coordinates after wake, so this initial archive is retained as historical evidence and is not the physical handoff candidate.

The independent firmware run and the exact physical handoff gate are recorded in [pre-device verification](PRE_DEVICE_VERIFICATION.md). Firmware-emulator and physical-device results remain separate. Earlier failed, interrupted, and resource-exhausted attempts are preserved; they are not relabeled as the successful candidate.

## V2 design review: 0.1.2

The 2026-11-04 review follows the latest Downloads reference, SHA-256 `62425b5a36df75457fd1769667b5fe88ef06a9d7c6c566a9608a12549aa88414`. It uses 57 reference renders across Paperwhite and Scribe, measured layout data, actual application images, and real pointer input. The [v2 review record](DESIGN_REVIEW_V2.md) describes the confirmed discrepancies and evidence boundaries.

Corrections cover navigation and footer alignment, chapter-selection states, committed text versus composition, candidate paging, search empty states, reader sheets and tap zones, download drawers and retained pause progress, account storage, and authentication/purchase presentation. Queue priority now changes persisted acquisition order. Authentication return rechecks the new account's permissions, blocked downloads retain their selection, and a refreshed wallet requires a fresh complete quote.

Startup feedback and same-process reading continuation remain intact. Fictional previews, view-model fixtures, live-account behavior, simulated operating-system services, and physical-device acceptance remain separate evidence.

The final snapshot passes **148 application tests**, its desktop build, **8 actual pointer cases with 54 clicks**, and **23 default/paused application captures**. Six authentication views and two Guide projections are explicitly view-model fixtures. The earlier snapshot 8 retains its full 44-view matrix, 11 affected-state images, 9 reader cases, 5 guest-reader cases, and 3 startup/sleep protection checks. The final four-file presentation delta and each result's executable identity are recorded in the [verification summary](../artifacts/verification/design-v2-0.1.2/verification-summary.json) and [review record](DESIGN_REVIEW_V2.md); earlier results are not reassigned to a newer binary.

The final [source manifest](../artifacts/verification/design-v2-0.1.2/source-inputs-round9.sha256) has SHA-256 `0aaead665f56074d7c6709f428b0b4c68d38d49634efa685d8624d70322db0b3`. The desktop executable has SHA-256 `dc13dc374525fe98e3bb43d2deed9526f560f0839c47b22f37701cfd471048c9`.

The [0.1.2 ARM development package](../artifacts/verification/design-v2-0.1.2/bilicomics-kindle-0.1.2-dev.zip) is **49,923,219 bytes**, SHA-256 `02a85557728b3bb612929afa6e96c89a9d0cd5434ca98bd0e6514d54bb057038`. The static ARM EABI5 hard-float executable has SHA-256 `76a8c9969120903db6b6df532eb8cf469932a6f169d0593134e5d0ef0ead7842`. All 22 allowed archive entries, content digests, CRC, executable modes, fixed timestamps, and runtime-account-data exclusion passed [remote package checks](../artifacts/verification/design-v2-0.1.2/package-verification.json). The archive contains no accepted device/firmware entry. Existing installations must preserve their `data/` directory when updating.

[Paperwhite preview](../artifacts/verification/design-v2-0.1.2/overview-pw-final9.png) and [Scribe preview](../artifacts/verification/design-v2-0.1.2/overview-sc-final9.png) use only the final executable's actual default views.

## Startup and sleep update: 0.1.1

The 2026-11-04 update adds a visible first startup scene and a low-frequency activity indicator before full font and state preparation. It also keeps the same application and reader alive across the system screensaver lifecycle. Ordinary `launch=shelf` settings do not redirect this retained reading session.

The remote integrated library suite passes **129 tests**; the native backend passes **27 tests**, and the isolated production launcher passes **28 lifecycle fixtures**. Actual Rust/Slint process checks additionally demonstrate two changing startup frames before readiness, the same non-default comic/episode/revision/page after sleep, ineffective input while suspended, and continued page turning after wake. A SQLite failure case keeps page 27 in RAM while the durable anchor is deliberately stale at 18, then catches the database up after the failure is removed; unknown purchase/recharge intentions remain unchanged.

These are remote software and simulated-OS-service results. They do not establish physical Kindle panel, power-daemon, or touch acceptance. The implementation and evidence boundaries are described in [Startup and sleep lifecycle](STARTUP_AND_SLEEP.md). The earlier candidate3 results below are retained as the 0.1.0 baseline.

The new [0.1.1 ARM development package](../artifacts/verification/startup-sleep-0.1.1/bilicomics-kindle-0.1.1-dev.zip) is 49,618,562 bytes, SHA-256 `c8ccfecfd073a46b3f2f6caf4e996bfc9062738a7086bf4739db57cf17117d32`. The release executable is static ARM EABI5 hard-float, with no interpreter or required shared libraries. All 22 allowed archive entries, content digests, executable modes, fixed timestamps, startup-font provenance, and runtime-data exclusion passed remote checks.

| 0.1.1 evidence | Result |
| --- | --- |
| [Runtime summary](../artifacts/verification/startup-sleep-0.1.1/runtime-verification-summary.json) | PW5/Scribe animated first frames, startup error exit, early-boot signals, same-process reading continuation, and checkpoint failure all passed |
| [ARM and archive report](../artifacts/verification/startup-sleep-0.1.1/startup-sleep-0.1.1-arm-package-verification.json) | Release build and all archive/ELF checks passed |
| `artifacts/verification/startup-sleep-0.1.1/logs/` | Integrated 129-test suite, 27 backend tests, build and lifecycle logs |
| `artifacts/verification/splash-sleep-launcher-tests.log` | 28 isolated supervisor fixtures passed |

Updating an installed development build must preserve its `data/` directory, including device configuration, credentials, downloads, and reading state.

## Implemented application

- Paperwhite and Scribe layouts, shared grayscale components, bundled Chinese fonts, and independent application navigation.
- Bookshelf grid/list, resume card, filters, sorting, cached catalog browsing, category selection, search history, and offline pinyin input.
- Comic details, separate reading/access/storage states, chapter filtering, selection, and explicit downloads.
- Background comic-image preparation, exact source anchors, single-page, panel and strip modes, overview/chapters/settings, chapter transitions, and configurable reading direction. Scribe landscape spreads are available through the desktop preview.
- Durable downloads, retained versus reader-owned work, pause/resume/cancel/retry, account namespaces, immutable index revisions, receipt-backed files, and interruption recovery.
- QR sign-in, bounded session import, manga device initialization, account validation, private session persistence, and session-bound renewal stages.
- Wallet and quote reads, explicit purchase and recharge flows, durable transaction intentions/receipts, and local receipt retry without retransmitting an uncertain transaction.
- Account preferences, storage statistics, protected cache removal, and diagnostic export. Unsupported hardware-dependent operations are disabled with an explanation.

UI components live in `ui/`. Rust controllers are separated under `src/app/`; protocol, local reader, download services, SQLite persistence, and platform code have their own modules. Chinese text lives in localization and fictional preview data rather than code comments.

## Verification

All compilation, tests, application execution, and rendering for this task run through `ssh test-env`. The operator's Windows machine is used for source editing, static design extraction, file transfer, and viewing saved images.

The original 0.1.0 integration candidate passed `cargo check --offline`, `cargo test --offline --lib`, and the desktop build without warnings. Its application suite reports **115 passed, 0 failed, 0 ignored**, covering native C integration, protocol vectors, account binding, authentication stages, transaction persistence, reader positioning, download recovery, file receipts, and demo isolation. Later update results are recorded separately above rather than reassigned to that binary.

Runtime evidence is generated under the ignored `artifacts/verification/` directory. Earlier candidates are retained with separate names and binary/source identities; their screenshots or results are not silently reassigned to a newer binary. Final default-view, pointer, state-transition, restart, and ARM package results are recorded in the accompanying final verification artifacts.

### Previous 0.1.0 candidate: candidate3

Recorded on 2026-11-04 (Asia/Shanghai). The final revision corrects icon alignment; its Rust application and protocol inputs are unchanged from candidate2. The complete verification was repeated against the final binary.

| Check | Final result | Evidence |
| --- | --- | --- |
| Integrated application tests | 115 passed; no failures or ignored tests | `artifacts/verification/candidate3-lib-tests.log` |
| Default views | 22 passed: all 21 design views plus the PW6 bookshelf | [UI summary](../artifacts/verification/candidate3-ui-summary.json) |
| Actual pointer interaction | 16 passed, using XTEST rather than direct controller calls | Final pointer manifest in `artifacts/verification/` |
| State transitions | 14 passed, including unavailable-network resume/retry | Final named-state manifest in `artifacts/verification/` |
| Process restart | Reader page 19 restored as page 19 in a separate process | Final reader-restart record in `artifacts/verification/` |
| Kindle release build | ARM EABI5 hard-float; no interpreter or required shared libraries | [ARM report](../artifacts/verification/candidate3-arm-package-verification.json) |
| Development archive | CRC, all 21 allowed files, content digests, executable modes, timestamps, and runtime-data exclusion passed | [Package manifest](../artifacts/verification/bilicomics-kindle-0.1.0-dev.manifest.json) |

The previous [0.1.0 development ZIP](../artifacts/verification/bilicomics-kindle-0.1.0-dev.zip) is 49,387,985 bytes. Its SHA-256 is `79f66fdd57cf911aa8f7b79410cb22670aef1a3d78bda34672b8ffd9f9b26c08`. The ARM executable digest is `911002480a84164fdf7aa73209e2bd4f22e71e601ef0b87f841db6d4f8b7d4b6`. Its exact binary inputs are recorded in `artifacts/verification/candidate3-final-source.sha256`.

The package includes the launcher, an empty device/firmware acceptance allowlist, and original third-party notices. It contains no account runtime data. The ARM build's compiler-probe messages are preserved in `kindle-build.log`; the completed release and static ELF checks passed.

![Application preview](../artifacts/verification/overview-candidate3.png)

The remote development combination uses Rust/Cargo 1.99.1 and Slint 1.17.1. The declared Rust minimum and the reviewed backend candidate originate from the WeRead reference; a separate minimum-toolchain acceptance run has not been performed.

## Preview and real data

`--demo` uses the embedded fictional design data and a source client that rejects all network and cryptographic acquisition operations. It uses a separate `demo/` data directory, never loads the real credential file, and never performs a real purchase. UI captures identify the profile, route, binary state, and demo status.

Only explicitly known current chapters are synthesized for other preview comics. Missing pages reuse identified artwork as fixture substitutions; unknown chapters and page counts are not represented as acquired upstream content. The separate strip sample does not overwrite the 45-page example's source anchor.

Normal operation uses the reviewed source adapter. Signed URLs and ephemeral image keys are not persisted as stable page identity. Download clients retain a fixed account constraint while accepting legitimate same-account credential updates. Old results cannot publish to a different account namespace.

## Acceptance boundaries

- No real Bilibili account login, account content request, purchase, coupon consumption, recharge, or credential rotation has been exercised by this application in this task. The anonymous public category check is limited to metadata; source fixtures and upstream reports remain separate evidence.
- No general physical Kindle compatibility is claimed. The supervised observations above retain their exact candidate and behavior scopes; sleep-time direction change in 0.1.14 remains unaccepted. Device/firmware allowlists remain empty. Cross-compilation alone does not establish panel, touch, battery, sleep, TLS, or firmware compatibility.
- Native rotation, hardware dithering selection, Wi-Fi management, sleep-cover customization, hardware inversion, and an update feed are not established by the current platform boundary. Their corresponding production controls are unavailable. The current native command rejects landscape; the desktop Scribe spread preview is implemented.
- Automatic future-chapter downloads depend on charging/connectivity integration that is not available here. The production setting is disabled; manual retained downloads and reader-owned page prefetch are implemented.
- Panel discovery for real pages is heuristic, with deterministic viewport fallback. Design fixtures have explicit panel maps. The reader performs bounded raster decoding with a 32-million-source-pixel limit; oversized or unsupported images fail explicitly.
- The native protocol host targets Linux, including the Kindle musl target. Other platforms expose a typed capability error for that native protocol path. Desktop UI and local content are separate from that capability.
- FAT/exFAT does not provide an encrypted credential vault or enforce Unix private-file modes. Recognized filesystem limitations are distinguished from other permission and storage failures.
- Project licensing and public distribution remain undecided. Third-party notices and reviewed source revisions are preserved; the output is a development candidate rather than a published release.
