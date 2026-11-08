# Wake orientation: 0.1.15 development candidate

Recorded on 2026-11-09 (Asia/Shanghai). The application now separates the current
holding direction from the framebuffer's calibrated scanout direction on the
reviewed Scribe / 5.19.5 mapping. Remote application regressions, ARM packaging,
and the corrected backend tests have passed with the separate scopes below.
The later R29 candidate completed supervised wake-direction, retained-reader,
post-wake controls/navigation, and native-return checks on one Scribe2022 /
5.19.5 with isolated offline demonstration data. The inherited sleep-time
direction work is complete within that scope. The precise R28 first-post-wake
19-to-20 page-turn step was not exercised and remains explicitly untested.

## R26 implementation

Startup retains the protected native direction/GETVAR calibration and stages
equal holding and scanout directions. It does not query the new sensor path, so
the previously checked startup and legacy setter contracts remain intact.

At resume, `NativePresentationContext::observe_resume_portrait` encloses a fresh
read-only framebuffer metadata query and an independent evdev pose query. The
context rechecks launch identity, lifecycle request, the current `CALIBRATED`
permission, and presentation records before returning. The framebuffer rotation
is resolved against the same-launch startup calibration; absolute kernel
rotation numbers are not treated as device-independent Up/Down values.

The opt-in pose reader examines read-only evdev capabilities, device name, and
`EVIOCGABS(ABS_PRESSURE)`. A recognized pressure-only kx132 source groups values
15/17 as Up and 16/18 as Down using the reviewed driver contract. It does not
consume events, grab input, change native services, or program display rotation.
An unknown value from a recognized source, an unreadable source, or multiple
recognized sources returns an error. An absent or unrecognized source returns
`None` and retains the calibrated framebuffer direction as the explicit legacy
fallback.

`set_native_framebuffer_portrait_presentation(holding, scanout)` stages the two
directions while display ownership is suspended or before initial native I/O.
Up has offset 0 and Down has offset 180. Pixels use
`(holding_offset + 360 - scanout_offset) % 360`; raw touch uses the holding
offset alongside the existing application rotation and single scale division.
The compatibility framebuffer setter supplies equal directions; the original
software setter retains its pixel compensation and zero native touch offset.
The existing complete scene rewrite, GC16 submission, and input-drain barrier
remain in the display lifecycle.

| Holding | Observed scanout | Residual pixel offset | Native touch offset |
| --- | --- | ---: | ---: |
| Up | Up | 0 | 0 |
| Up | Down | 180 | 0 |
| Down | Up | 180 | 180 |
| Down | Down | 0 | 180 |

The resume trace records `portrait_direction_ready`, `holding_direction`,
`scanout_direction`, and `pose_source`. `evdev-kx132` identifies the independent
source; `framebuffer-calibration` identifies fallback. A newly issued metadata
read does not prove that the driver has already observed the operator's physical
direction change at that point in the wake sequence.

## R26 application and package evidence

The frozen workspace is `/root/bilicomics-wake-orientation-20261108-r26`.
Its [selected pipeline summary](../artifacts/verification/wake-orientation-20261108/candidate-r26/evidence/20261108T155158Z-9475f5d4/summary.json)
records the following stages on `test-env`:

| Stage | Result | Scope |
| --- | --- | --- |
| Application core | 474 passed, 0 failed or ignored; check and desktop build passed | Application library and desktop executable |
| Reader | 9 cases passed | Controlled desktop reader regression |
| Startup | 2 cases passed | Controlled startup frames |
| Launcher lifecycle | Passed | Controlled Linux services with a real Rust process |
| ARM build and package | Passed; 28 entries and 9 static checks | Development archive and ARM ELF metadata |

The source-input manifest SHA-256 is
`afaea670dd00a62cd22e7e1d041c225a92f907bea58c4de6aa88b827f0d9b638`.
The desktop executable SHA-256 is
`6c526eb4d349874b8a74f0cd31f86c14a64c0c91a61dba35785e10b10a67be0f`.
The [package report](../artifacts/verification/wake-orientation-20261108/candidate-r26/evidence/20261108T155158Z-9475f5d4/package.json)
and [archive manifest](../artifacts/verification/wake-orientation-20261108/candidate-r26/bilicomics-kindle-0.1.15-dev.manifest.json)
retain ARM SHA-256
`84371072f66a2c5f84d3ecbdad84e5f5b39e6266da40f7378675146637f5d5d5`
for 66,076,992 bytes and production ZIP SHA-256
`e63fc240a0afa0f6db45c924ae710eef80044907fe219fafb13bbe44feac799b`
for 50,574,349 bytes. The archive contains no accepted device/firmware entry.
Its production launcher retains the unchanged two-second native-read budget.
Any private five-second physical-test copy keeps its own identity.

These selected stages did not connect to a physical Kindle or firmware emulator,
use an account, or perform remote transactions. Visual review is not claimed by
the automatic reader/startup results. Unselected default-view, pointer, guest,
and launcher-fixture stages are not reassigned from older candidates.

## R26 backend failure and R27 correction

The separate [R26 backend attempt](../artifacts/verification/wake-orientation-20261108/candidate-r26/evidence/backend/summary.json)
failed its test build with `E0432`: the `cfg(test)` orientation fixture imported
`Rgb8Pixel` from `slint::platform::software_renderer` instead of `slint`.
This failure is preserved and did not prevent the production ARM build, which
excludes that test-only module.

R27 corrected only that import. Its [backend summary](../artifacts/verification/wake-orientation-20261108/backend-r27/summary.json)
records 119 passed, 0 failed or ignored, with dependency versions and checksums
aligned to the production lock. The backend source-input manifest SHA-256 is
`523549c314d6e9f24ff2c0762b6be89b2fec6887e4f0ce62504fc1852ba60b79`.
The runtime implementation and the R26 ARM/ZIP identities were unchanged by this
test-only correction. The R27 result is a corrected component-test result; it is
not a rerun of the entire R26 source manifest or application/package pipeline.

The backend suite includes the four independent holding/scanout combinations
using actual Slint pixels and production raw-input corner hits, compatibility
setters, no-op updates, active/reentrant/terminated update rejection, deferred
sleep I/O, retained mappings, and full-rewrite/input barriers. These fixtures do
not establish actual EPDC scanout, physical GC16 completion, or sensor freshness.

## Native-only probes and diagnostic boundaries

R24 used source SHA-256
`3f3a48fe6aa1d9f259534fbf95b56629bc28f3fe96322f1942d6a53527068dee`.
Its [public probe record](../artifacts/verification/wake-orientation-20261108/r24-r25-final-evidence/device-probe/probe-state.json)
and samples retain the USB-up `D` / rotation `1` baseline. The operator performed
sleep, moved the USB port downward, and woke to normally oriented native text
and touch. The probe's WM read gate failed with `wm_changed`, exit 1,
`cleared` recovery, and `unconfirmed` native control at that exact check. Later
observations found the same WM identity and normal native services, but no
accepted new-direction sample. R24 remains inconclusive for wake timing.

R25 used source SHA-256
`4464ff9f22b98db991acaf072dfa8aad0cb42a25cc10a058be97f3b56a0c2920`.
Its [public probe record](../artifacts/verification/wake-orientation-20261108/r24-r25-final-evidence/device-probe-r25/probe-state.json)
completed the actual 80-second native-only window with exit 0 and
`complete / cleared / observed_live`. It observed only the unchanged `U` /
rotation `3` baseline, with no operator sleep/wake cycle; the coordinator sent
the physical sequence instruction too late. R25 validates bounded probe
completion and recovery, not wake orientation or application continuation.

Both runs started no application and opened no framebuffer. Their separate
[R24](../artifacts/verification/wake-orientation-20261108/r24-r25-final-evidence/owned-probe-recovery.json)
and [R25](../artifacts/verification/wake-orientation-20261108/r24-r25-final-evidence/owned-probe-recovery-r25.json)
recovery receipts record no live owned group members, successful guarded
transport completion, and closed device connections. The exact original
`wake-pose-20261108-r24` lease was [released](../artifacts/verification/wake-orientation-20261108/r24-r25-final-evidence/lease-return-r24.json)
at `2026-11-08T16:06:04.726750+00:00`, with owner/token checks and an absent lease
directory. The device was explicitly returned to WeRead.

The separately retrieved [R25 Linux probe fixtures](../artifacts/verification/wake-orientation-20261108/r25/fixture-evidence/summary.json)
passed `normal`, `slow-getter`, and `supervisor-loss` for source `4464ff...`, in
private PID/mount namespaces on `test-env`. The normal case includes a transient
WM stop and restoration. These controlled namespace fixtures are independent
of the native-only physical samples and of the application/backend candidate.

The separate [C sensor diagnostic build](../artifacts/verification/wake-orientation-20261108/sensor-helper-r25/README.md)
passed ARM static checks and three host CLI rejection calls. At that build's
completion it had not queried a physical sensor or been deployed in an
application run. Later R28 read-only sensor pairs are recorded below; the build
and CLI results alone do not establish payload, freshness, or Rust behavior.

## Handoff state before the R28 claim

The continuation owner is `01a1bba4-bda4-77d1-96ae-aa15eb091ad7`. A new
`wake-reader-20261109-r28` physical handoff has been requested from WeRead.
At that earlier update no new lease was claimed, and neither the 0.1.15 application nor
the C helper had been deployed for that run. The historical R22/R23 startup,
touch, and retained-page results remain described in
[orientation research](KINDLE_ORIENTATION_RESEARCH.md); they do not accept the
new wake direction implementation.

The R26-specific acceptance target required `pose_source=evdev-kx132`
and an actual operator sleep-time direction change, correctly oriented artwork,
the same process/page/source anchor, and intentional first post-wake input.
Fallback or unchanged baseline observations cannot satisfy that acceptance.
The native dismissal and input/repaint barriers must remain effective. After
the scoped run, restore native control, confirm native direction and touch,
close owned transports, release the exact lease, and explicitly return Kindle.

`test-env` supports parallel work in isolated workspaces with bounded build
concurrency. No whole-host loan or completion gate applies. The physical Kindle
alone requires the exclusive handoff and shared device lease. No local tests,
builds, validation suites, or runtime probes were run for this continuation.

## R29 actual sensor correction

The user subsequently handed over the R28 physical window. The coordinator
atomically claimed the distinct `wake-reader-20261109-r28` lease for owner
`01a1bba4-bda4-77d1-96ae-aa15eb091ad7`, with token fingerprint
`d68e5dccc541e7902d68f3bfaba061c6bdc5bc432aedd4605f73638afa19d82e`.
The earlier R24 lease return is a separate completed handoff.

The read-only [initial native/sensor pair](../artifacts/verification/wake-orientation-20261108/final-physical-summary/public/sensor-native-initial-r28.json)
identified `/dev/input/event2` as pressure-only `bma4xy_feature`, with raw value
16 alongside native `D` and rotation `1`. After the operator confirmed USB-down
and upright native text, the [second pair](../artifacts/verification/wake-orientation-20261108/final-physical-summary/public/sensor-native-upright-r28.json)
returned raw 15 alongside native `U` and rotation `3`. The independent C helper
retained unknown normalization for this previously unrecognized name; the
association comes from these declared raw/native pairs. Neither pair measures
the driver's exact update time during a wake transition.

R26 recognized only kx132 and would use calibration fallback on this device, so
its candidate is not accepted as the physical direction fix. R29 added the
exact `bma4xy_feature` name alongside the existing kx132 contract and the
observed 15/16 mapping, plus a corresponding unit case. The application's
independent-source trace tag became `evdev-native-portrait`. Protected GETVAR
calibration, startup behavior, residual rendering, legacy paths, and ownership
barriers retained their existing contracts.

The R29 [backend component report](../artifacts/verification/wake-orientation-20261108/candidate-r29/evidence/backend/summary.json)
passes 120 tests, 0 failed or ignored, with aligned production dependencies.
Its component source manifest is
`da96940a17f22fa28bfa7b3cd1874c4cab81894f3ff26ee6e6cabf0885de225d`.
The separate [ARM/package report](../artifacts/verification/wake-orientation-20261108/candidate-r29/evidence/arm/summary.json)
records 28 entries and nine passed static checks. The ARM build's source-input
manifest SHA-256 is
`c4ab975f072a218ea72e9523b069540ac98e46fca83891412f9fde2dfa757114`;
its executable is 66,077,248 bytes, SHA-256
`3907fa54fb4d802340d1c4a0cbb866587805d600a350b908ea5163b36bf1fdac`.
The production ZIP is 50,574,690 bytes, SHA-256
`88a049d057c07292252a699d54805e4ef123f2e26f18f0e595f20e4b23a7d40a`.
The R26 application/desktop counts and R27 corrected component result retain
their original manifests; they are not reassigned to this new ARM candidate.

The [supervised reader package record](../artifacts/verification/wake-orientation-20261108/final-physical-summary/public/bma-package-r29.json)
and its [static package checks](../artifacts/verification/wake-orientation-20261108/final-physical-summary/public/bma-package-verification-r29.json)
identify the private five-second
native-read test package, SHA-256
`a12248c76f288968c6a28bab9a7433b5ddfe512f33a9cd23e092e8f7d3c7e8c8`,
with launcher SHA-256
`9858059d65d74596a432dda6663ca041e3d7d287951e835e05b633eb0d29825b`.
This retains the user's earlier test-only budget choice. The production
launcher's two-second budget and production package identity remain separate.

## R28 supervised reader observations

The [run record](../artifacts/verification/wake-orientation-20261108/physical-r28/run.json)
and [suspend/resume trace](../artifacts/verification/wake-orientation-20261108/final-physical-summary/public/reader-wake-trace-r28.json)
retain the exact candidate, process, and reader anchor.
On the scoped Scribe2022 / 5.19.5, the R29 reader ran as PID `28038`, birth tick
`728437810`. The operator confirmed complete correctly oriented page 18 while
holding USB-up and used actual finger input to reach page 19.

The operator then used the physical power key, waited about ten seconds in
sleep, changed to USB-down, and woke directly to the same correctly oriented
page 19 without native Home. The coordinator's comparison retained the same
PID and full reader source anchor. The resume trace recorded holding `U`,
scanout `U`, and `pose_source=evdev-native-portrait`, with no calibration
fallback. These observations accept this scoped wake direction and page
continuation; they do not establish universal sensor freshness or another
device/firmware mapping.

Post-wake menu opening and closing were recorded. The operator did not perform
the requested first-post-wake 19-to-20 page turn before leaving the device;
page 20 was not recorded. That exact step is untested. The 240-second bounded
application exited normally through its timer with launcher status 0. The
watchdog recorded `launcher-exit` and complete recovery. Recorded recovery
checks matched the awesome and statusbar baselines and found the launcher lock
absent. The user's subsequent request to reopen reading led to the separate
R30 process below.

## R30 retained reading and post-wake controls

The separate [R30 run](../artifacts/verification/wake-orientation-20261108/physical-r30/run.json)
and [retained-reader trace](../artifacts/verification/wake-orientation-20261108/final-physical-summary/public/reader-last-trace-r30.json)
record the reopening, additional wakes, and native-return observations.
After R28 had stopped, only its isolated `demo/state.sqlite3` was copied into a
new R30 `data/demo` directory to retain page 19. No account state was transferred.
R30 used the same R29 ARM/private five-second test package and ran as PID
`11483`, birth tick `728542589`. The operator confirmed the same page and correct
direction after reopening. This is a new process restoring an offline anchor;
the R28 same-process wake result remains separately identified.

The operator completed additional physical sleep/direction changes and confirmed
the reader menu, details, bookshelf, and Account navigation behaved correctly.
Three resume records show `U/U`, `D/D`, then `U/U` holding/scanout pairs through
the independent `evdev-native-portrait` source. The same R30 PID retained page 19
until the reader was closed. Actual post-wake controls/navigation are accepted
for this supervised scope. Neither run records the exact page-20 step, so that
specific assertion is not reported as exercised.

The operator selected X and confirmed native direction and touch were normal.
The R30 launcher exited 0 through `launcher-exit`; watchdog recovery completed,
the awesome/statusbar baselines matched, and the launcher lock was absent.

## Final physical closure and acceptance scope

The [final return guard](../artifacts/verification/wake-orientation-20261108/final-physical-summary/public/final-return-guard.json)
records closed BiliComics processes/transports and operator-confirmed native
direction/touch. The exact
`wake-reader-20261109-r28` lease, with the public fingerprint recorded above,
was [released](../artifacts/verification/wake-orientation-20261108/final-physical-summary/public/lease-return-r28.json)
at `2026-11-08T17:01:57.270054+00:00`
(`2026-11-09 01:01:57` Asia/Shanghai). Owner/token checks passed and the shared
lease directory was absent after release. The device was explicitly returned to
WeRead, and the coordinated device users were notified of that return.

The public [R28 export manifest](../artifacts/verification/wake-orientation-20261108/physical-r28/export-manifest.json),
[R30 export manifest](../artifacts/verification/wake-orientation-20261108/physical-r30/export-manifest.json),
and [closure export manifest](../artifacts/verification/wake-orientation-20261108/final-physical-summary/public/export-manifest.json)
retain raw/export digests and sizes, bounded token redaction, and exact included
files. Demo framebuffer captures remain separate from operator panel observations.

The remaining wake-direction issue inherited from the preceding chat is resolved
within the observed Scribe2022 / 5.19.5 / offline-demo scope: correctly oriented
retained reading, independent pose rather than calibration fallback, functional
post-wake controls/navigation, and confirmed native return. The exact R28
first-post-wake 19-to-20 step remains an untested stage-specific case. The
repository acceptance allowlist stays empty; these observations do not establish
general device support, real-account behavior, or universal sensor freshness.
