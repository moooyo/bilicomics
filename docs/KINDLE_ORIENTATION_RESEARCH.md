# Kindle orientation research

The 0.1.11 physical navigation failure reproduced in inverted portrait. The
0.1.12 diagnostic build preserved the original input mathematics. In upright
portrait, raw `(1660, 2382)` mapped to logical `(830.4465, 1191.4805)` at scale 2,
and the operator successfully used Account navigation and the upper-right X.
In inverted portrait, lower-screen touches mapped to the logical upper edge and
the initial render offset remained zero. The same native orientation query timed
out while awesome was paused, then returned `D` immediately after restoration.

## Official KOReader reference

The source review used KOReader commit
`9040116701c888889bd35233b23b83283be66745` and its actual base submodule commit
`1c9682506757d5fd30cfa01c5698929dcc35340f`. The base master was newer and was not
substituted for the frontend's pinned dependency. Raw research files remain on
the designated host outside this repository.

The [Scribe initialization](https://github.com/koreader/koreader/blob/9040116701c888889bd35233b23b83283be66745/frontend/device/kindle/device.lua#L1841-L1897)
temporarily resumes an already paused awesome, reads the native accelerometer
property, normalizes `U/L` to upright and `D/R` to upside-down, sets the native
and current screen rotation, and only then pauses awesome again. This explicitly
addresses inverted-start touch behavior. The Bili implementation instead obtains
the required observation before it pauses its individually identified native
processes; it does not import process-name signalling.

KOReader also translates trusted accelerometer events and handles
[runtime direction changes](https://github.com/koreader/koreader/blob/9040116701c888889bd35233b23b83283be66745/frontend/device/input.lua#L1437-L1473).
Its [framebuffer abstraction](https://github.com/koreader/koreader-base/blob/1c9682506757d5fd30cfa01c5698929dcc35340f/ffi/framebuffer.lua#L452-L462)
separates current application rotation, native rotation, and software buffer
rotation. The
[MXCFB refresh path](https://github.com/koreader/koreader-base/blob/1c9682506757d5fd30cfa01c5698929dcc35340f/ffi/framebuffer_mxcfb.lua#L318-L330)
uses the same buffer transform for update rectangles. Optional hardware
`forced_rotation` paths are not a claim about Scribe's normal configuration.

## Bili correction and limits

The fixed implementation must authenticate the pre-takeover native orientation
and a same-transaction framebuffer baseline, stage the rendering correction
before first input, and refresh it on wake. Display pixels, window dimensions,
and refresh rectangles must use one effective rendering rotation. Slint 1.17.1
accepts logical pointer coordinates without another scale conversion, so the
existing single physical-to-logical division remains necessary.

The wake fast path must retain its confirmed native dismissal and frame barriers.
Adding another synchronous window-manager query would delay display takeover.
Relative fresh GETVAR calibration is restricted to validated same-run anchors,
unchanged geometry, and zero or half-turn changes; unknown or unsupported
changes are rejected rather than inferred. The sleep-time direction-change path
still needs a new correction and scoped physical verification; later inverted
startup and touch observations are recorded separately below.

The upright X result and the inverted failure remain separate. Neither the
diagnostic-only build nor the software regression results establish general
hardware compatibility. The research did not modify native themes, inject
touches, access accounts, or copy KOReader source into the application.

## Historical 0.1.13 candidate

The candidate captures native portrait direction and framebuffer rotation before
pausing the native window manager. Private, launch-bound sidecars authenticate
the calibration and each resume permission. The backend stages the effective
rendering rotation before its first input acquisition; wake uses a fresh,
read-only GETVAR observation against that calibration without adding a fast-wake
window-manager query. Unknown rotation, odd relative turns, and changed memory
layouts are rejected. Touch mathematical conversion remains unchanged, including
its single scale division; full scene drawing and GC16 submission precede input
admission.

The frozen source at
`/root/bilicomics-native-orientation-20261108-r14-e14f1e35` passed 473 library,
106 backend, 90 launcher, and 21 canonical physical-harness tests on `test-env`.
The ARM archive passed nine static checks for 28 entries. A later test-only
supervisor revision passed three receipt fixtures and one alias fixture in
46.359 seconds, exit 0, with no background fixture processes; the initial
zero-test invocation, exit 5, is retained alongside the
[retry result](../artifacts/verification/touch-input-20261108/receipt-builtins-r19/summary-retry.json).
These are controlled software and package results with separate supervisor
revision scope. The historical 0.1.13 ARM
SHA-256 is `20ee9293198a45cf436e6e4ff22b3276c49d7e6b0461f162a0eec99c275306dd`;
the archive SHA-256 is
`29499570a47d98c5488e553c30d072146f26384e52369f4186924128bcddd929`.

R15-R19 remain failed physical attempts. The user then authorized a private
[test copy](../artifacts/verification/touch-input-20261108/test-native-read-5s.json)
changing only the native-read watchdog sleep from two to five seconds and its
outer polling count from 20 to 50. The production source, ARM and archive were
unchanged. R20 reached startup readiness: its native sample ran from uptime
7257141.11 to 7257143.64, or 2.53 seconds, exceeding the old two-second budget.

R20 orientation acceptance failed: the user held the device USB-up and reported
the BiliComics scene inverted. The user actively selected X; the launcher exited
naturally with status 0 before the 300-second application deadline. Native
direction and touch were then confirmed normal. Recovery guard exit 0, closed
connections, owner/token-checked lease release, an absent lease directory, and
explicit return to WeRead are recorded in the
[run](../artifacts/verification/touch-input-20261108/portable-physical-orientation-r20-5s/run.json)
and [release receipt](../artifacts/verification/touch-input-20261108/portable-physical-orientation-r20-5s/lease-return.json).
The X/restoration observations do not accept global touch navigation; reader and
sleep acceptance were not tested. The earlier failures remain in their separate
portable evidence directories.

## 0.1.14 completed stages and scoped physical observations

The 0.1.14 correction separates an opt-in native scanout pixel/render offset of
zero from native touch orientation, Up=0 and Down=180, while retaining the old
setter's legacy behavior. The earlier R21 core phase passed compilation and
474 application library tests. Its first formatting pass touched unrelated
modules and stopped before compilation; those changes were restored. The
following core pass succeeded, then the backend stopped at a corner fixture
whose top-left target lacked explicit x/y positioning. Those failed attempts
remain preserved rather than reassigned to the successful retry.

The [final R21 continuation](../artifacts/verification/wake-orientation-20261108/recovered-r21/portable-host-r21-final/driver-summary.json)
performed selected-file formatting, [110 backend tests](../artifacts/verification/wake-orientation-20261108/recovered-r21/portable-host-r21-final/backend/summary.json),
and ARM packaging. It retained the earlier 474-test core result without rerunning
the application core phase. The [package report](../artifacts/verification/wake-orientation-20261108/recovered-r21/portable-host-r21-final/arm/package.json)
passed nine static checks for 28 entries, with an empty device acceptance
allowlist. The ARM executable is 66,073,152 bytes, SHA-256
`ba47f3c25bb676900709d0cbaf32d3fb3cce345d57cd8fd025075320a5f09628`;
the production archive is 50,572,397 bytes, SHA-256
`7871cb0746f54314165c452795710fc525910243db59cab3fd3f4ecf19a99d79`.
No current launcher or physical-harness result is inferred from the earlier
0.1.13 counts.

[R22](../artifacts/verification/touch-input-20261108/portable-physical-touch-split-r22/run.json)
used this ARM executable with the separately recorded private five-second
native-read launcher. Its deployed archive SHA-256 is
`2b44f68b42e1f21b302cdc9789a7b0ef00798e12de64dd5337ac928f52efe969`;
its launcher SHA-256 is
`9858059d65d74596a432dda6663ca041e3d7d287951e835e05b633eb0d29825b`.
The production launcher's two-second budget was unchanged. On Scribe2022 /
5.19.5, the user held USB-up and confirmed readable Bili direction, working
My / Screen and System navigation, the upper-right X exit, and normal native
direction and touch after exit. This is a scoped physical observation, not
general touch or firmware certification.

[R23](../artifacts/verification/touch-input-20261108/portable-physical-touch-split-r23/run.json)
used the same ARM and private test package for immersive reading. The user
confirmed correctly oriented complete artwork and finger paging 18 to 19.
Process `20990` and the full page-19 source anchor were retained at both suspend
and resume, as recorded in the [anchor audit](../artifacts/verification/touch-input-20261108/portable-physical-touch-split-r23/reader-anchor-audit.json)
and original lifecycle log. Later menu actions and page 20 were explicitly
operator initiated. The coordinator's later live-anchor comparison stopped the
run; that later difference is not an unintended page turn or a failed
suspend/resume anchor match.

Sleep-time direction change did fail: the user changed from USB-up to USB-down
while sleeping and woke to page 19 with the old display direction. Native
direction and touch were subsequently confirmed normal. The
[post-restoration read](../artifacts/verification/touch-input-20261108/portable-physical-touch-split-r23/native-orientation-after-stop.json)
returned rotation `3` and direction `U`; it does not establish when those
values became available before takeover. Recovery, closed connections, the
original owner/token-checked [lease release](../artifacts/verification/touch-input-20261108/portable-physical-touch-split-r23/lease-return.json),
and explicit return to WeRead remain tied to that original run.

## Current continuation: 0.1.15 automated candidate

The 0.1.14 fast-wake path could retain an old but valid GETVAR rotation after
pausing awesome. Its zero calibration delta preserved the old direction.
The exact native update time remains unmeasured by the available physical
probes. The [0.1.15 implementation and evidence](WAKE_ORIENTATION.md) now treat
current holding direction and actual scanout independently rather than assuming
they match at takeover.

`set_native_framebuffer_portrait_presentation(holding, scanout)` computes only
the residual pixel rotation and keeps raw touch relative to holding. The two
older setters preserve their contracts. The application encloses GETVAR and the
independent read-only evdev-kx132 pose query in the current protected resume
observation, then stages both directions before reacquiring input. Initial
startup retains the prior calibration and does not query the new sensor path.
Unknown recognized values and ambiguous sources return errors; absent or
unrecognized sensors retain the labeled calibration fallback. No metadata
sample alone proves the operator's direction was freshly observed during wake.

The R26 selected pipeline passed 474 library tests, nine reader cases, two
startup cases, the real-process controlled launcher lifecycle, and a 28-entry
ARM package with nine static checks. Its production ARM SHA-256 is
`84371072f66a2c5f84d3ecbdad84e5f5b39e6266da40f7378675146637f5d5d5`.
A separate backend test build failed `E0432` in a `cfg(test)` `Rgb8Pixel` import.
R27 corrected only that import and passed 119 backend tests, including actual
Slint pixel/raw-input corner regressions and the suspended/full-rewrite
barriers. This corrected component result does not replace the R26 whole-source
identity or claim a repeated application/ARM run. Full manifests, package
identities, original failure, and scope boundaries are in
[wake orientation](WAKE_ORIENTATION.md).

The continuation owner is `01a1bba4-bda4-77d1-96ae-aa15eb091ad7`. WeRead's
historical `wake-pose-20261108-r24` handoff used token fingerprint
`666ac047db5ac68df46def735ef1cec95ac6c9ddb11e6212cfb187f6b2e63614`.
R24's source `3f3a48fe...` observed D/1 baseline and the user completed the
USB-up sleep to USB-down native wake, but the WM read gate failed before any
accepted new-direction sample. R25's revised source `4464ff...` completed an
80-second native-only run with `complete / cleared / observed_live`; it saw only
U/3 baseline without an operator cycle because the coordinator instruction was
late. Neither run establishes new-direction timing or application wake success.
Their separate recovery receipts have no live owned groups or open transports.
The exact R24 lease was released at `2026-11-08T16:06:04.726750+00:00`, verified
absent, and the device explicitly returned to WeRead.

The independently retrieved R25 Linux probe fixtures passed all three cases in
private PID/mount namespaces, including transient WM stop/restoration. The C
sensor helper's separate static ARM/CLI checks passed; physical sensor reading
had not occurred at that build stage. The later R28 readings are described
below. The [wake orientation report](WAKE_ORIENTATION.md)
links these public records separately from the application candidate.

`test-env` is shared for parallel isolated work with bounded build concurrency;
the earlier whole-host gate no longer applies. The earlier R28 request-only
state has now advanced to the separate claim and supervised observations below.

## R29 sensor correction and completed R28/R30 scoped verification

The user handed over the new `wake-reader-20261109-r28` physical window, and
owner `01a1bba4-bda4-77d1-96ae-aa15eb091ad7` atomically claimed its new lease.
The public token fingerprint is
`d68e5dccc541e7902d68f3bfaba061c6bdc5bc432aedd4605f73638afa19d82e`.
The C diagnostic identified the actual pressure-only source as
`/dev/input/event2`, exact name `bma4xy_feature`. Raw 16 paired with native
D/rotation 1; after operator USB-down/upright confirmation, raw 15 paired with
U/rotation 3. The diagnostic retained unknown normalization for this name.
These raw/native pairs establish the scoped association, not exact wake
freshness. R26's kx132-only reader would fall back on this source and is not
reassigned as the physical fix.

R29 added exact-name support alongside kx132 and the measured 15/16 association,
with a new unit case and `evdev-native-portrait` independent-source trace tag.
Protected GETVAR calibration and initial startup behavior were retained. Its
separate backend component passed 120 tests, and ARM packaging passed 28-entry
and nine static checks. The ARM source-input manifest is
`c4ab975f072a218ea72e9523b069540ac98e46fca83891412f9fde2dfa757114`;
ARM SHA-256 is
`3907fa54fb4d802340d1c4a0cbb866587805d600a350b908ea5163b36bf1fdac`.
The [wake orientation report](WAKE_ORIENTATION.md) links the separate R29
component/build records and preserves the R26/R27 and original failure scopes.

Using the private five-second test package, the R28 reader ran as PID 28038,
birth tick 728437810. The operator confirmed complete correctly oriented
page 18, finger paging to 19, then physical sleep, a USB-up to USB-down change,
and direct correctly oriented page-19 wake without native Home. The same
PID/full reader anchor comparison passed; the resume trace recorded holding U,
scanout U, and `pose_source=evdev-native-portrait`, with no fallback.

R28 post-wake menu open/close was recorded. The operator did not perform the
requested first-post-wake 19-to-20 step before leaving the device; that exact
case remains untested. The 240-second timer ended the bounded application
normally with exit 0 and completed watchdog recovery.

The user requested reading again. Only the stopped R28 offline
`demo/state.sqlite3` was copied into a fresh R30 `data/demo` directory, preserving
page 19 without transferring an account. The new R30 process was PID 11483,
birth tick 728542589. The operator confirmed same-page correctly oriented
reading, additional physical sleep/direction changes, and correct reader-menu,
details, bookshelf, and Account navigation. Three resume records retained the
same R30 PID and page 19 with independent holding/scanout U/U, D/D, U/U pairs.
Actual post-wake controls/navigation passed in this scope. No page-20 result is
inferred from those actions.

The [R30 run](../artifacts/verification/wake-orientation-20261108/physical-r30/run.json)
records the operator's X exit and normal native direction/touch confirmation. Both
R28/R30 watchdogs recorded `launcher-exit`, launcher status 0, complete recovery,
matching native baselines, and an absent launcher lock. All BiliComics processes
and owned SSH transports had exited, as retained by the
[final return guard](../artifacts/verification/wake-orientation-20261108/final-physical-summary/public/final-return-guard.json).
The exact R28 lease was
[released](../artifacts/verification/wake-orientation-20261108/final-physical-summary/public/lease-return-r28.json) at
`2026-11-08T17:01:57.270054+00:00` (`2026-11-09 01:01:57` Asia/Shanghai), with
owner/token verification and an absent shared directory, and the device was
explicitly returned to WeRead. The [wake orientation report](WAKE_ORIENTATION.md)
retains the independent R26/R27/R29 identities and detailed physical scope.

The inherited wake-direction work is complete for this Scribe2022 / 5.19.5 /
offline-demo observation. The precise R28 first-post-wake 19-to-20 step remains
explicitly untested, while additional wake controls/navigation and native return
are accepted. General hardware support, accounts, and universal sensor freshness
remain outside that scoped result; the repository allowlist remains empty.
