# Live portrait orientation: 0.1.16

The user requested automatic upright/upside-down rotation while the application
is already running on 2026-11-09. The completed 0.1.16 candidate received scoped
supervised checks on one Scribe2022 / 5.19.5 using isolated offline demonstration
data. The operator accepted the current two-way behavior and its existing
hardware-event sensitivity, and explicitly cancelled four-way expansion after
comparing native Home. Manual application rotation and landscape preferences
retain their existing semantics. The separate 0.1.15 startup/wake results remain
in [wake orientation](WAKE_ORIENTATION.md).

## Source-derived behavior

The review used KOReader commit
`9040116701c888889bd35233b23b83283be66745` and its actual base dependency
`1c9682506757d5fd30cfa01c5698929dcc35340f`.

- [KindleGyroTransform](https://github.com/koreader/koreader/blob/9040116701c888889bd35233b23b83283be66745/frontend/device/kindle/device.lua#L1540-L1574)
  recognizes trusted accelerometer `EV_ABS / ABS_PRESSURE` values 15/16/17/18.
  The Scribe initialization installs the event adjustment and gyro handler.
- [Input gyro handling](https://github.com/koreader/koreader/blob/9040116701c888889bd35233b23b83283be66745/frontend/device/input.lua#L1437-L1473)
  changes current screen rotation and notifies UI rotation. With orientation
  locked, a matching-parity half turn does not require a different layout.
- [Framebuffer rotation](https://github.com/koreader/koreader-base/blob/1c9682506757d5fd30cfa01c5698929dcc35340f/ffi/framebuffer.lua#L452-L462)
  separates desired application, native, and software-buffer rotations.
- [Linux event protocol](https://github.com/torvalds/linux/blob/v5.10/Documentation/input/event-codes.rst)
  frames changes with `SYN_REPORT`; after `SYN_DROPPED`, consumers discard through
  the next report and use current-state ioctls to recover.

The scoped Scribe / 5.19.5 exposes pressure-only `bma4xy_feature`, already observed
as raw 16 with native D and raw 15 with native U. The implementation recognizes
that exact name alongside kx132 and rejects touchscreen/digitizer capabilities.
This is an independent implementation of the reviewed protocol, not copied Lua.

## Event and display flow

An explicitly calibrated owner supplies holding direction, observed scanout
direction, and the same-run raw GETVAR rotation. This enables a read-only,
non-grabbing accelerometer observer. The backend polls its descriptor alongside
the control pipe and touch; idle operation has no periodic orientation timer.

The observer uses complete `SYN_REPORT` notifications and current snapshots,
coalesces repeated normalized directions, ignores undefined active pose values,
and obtains current state after queue loss. Opening first drains stale events;
suspend and quit close the observer. Descriptor generations prevent old
readiness from replaying into a reopened source. A resumed owner supplies a new
calibration before input is admitted. An unavailable or failed optional observer
is disabled until the next resume while the reading session remains active.

The accepted sensitivity comes from the existing hardware event behavior. No
artificial stability timer, debounce delay, polling timer, or threshold was added
to change it. Complete-frame/current-state handling and duplicate coalescing are
correctness boundaries rather than a new sensitivity setting.

Control callbacks and lifecycle/generation checks run first. A direction event
then precedes touch dispatch. Fresh same-layout GETVAR resolves actual scanout
against the stored anchor, allowing only zero or half-turn relative changes.
The active transition cancels the previous pointer/contact, drains its queue,
invalidates its poll generation, and stages holding/scanout independently.

Pixels use the residual `(holding_offset + 360 - scanout_offset) % 360`; raw touch
uses holding plus the existing app rotation and one logical scale division.
The complete scene rewrite and GC16 submission precede the final input drain.
The native window manager remains under application ownership, and no external
sleep/resume or startup-destination action is generated for a live turn.

The reorientation drain treats `SYN_DROPPED` as an input-state loss: it discards
through the report boundary and reuses the existing protocol-B snapshot logic.
Legacy input without reliable snapshots requires newly observed coordinates.
If the report/snapshot is not ready, the first full frame remains submitted,
input stays gated, and the loop waits on control/gyro/touch descriptors. Touch
readiness retries the final drain without repainting or exiting. New control or
direction changes can submit a new frame. A successful drain admits only a fresh
gesture; closed input or other genuine I/O failures retain their error handling.

## R31 application stages and retained native build failure

The [R31 pipeline](../artifacts/verification/live-portrait-20261109/candidate-r31/evidence/20261108T175027Z-7ce809e7/summary.json)
passed 474 application library tests, its desktop check/build, nine reader cases,
two startup cases, and the controlled real-process launcher lifecycle on
`test-env`. Its source-input manifest SHA-256 is
`1f38786c269e196ea9458cecc60c2c1f98bcd211980c7cd46d2d7395bbd740b5`.

The separate [backend attempt](../artifacts/verification/live-portrait-20261109/candidate-r31/evidence/backend/summary.json)
and ARM stage failed to build because the native event parser contained an extra
comma. The selected R31 pipeline therefore remains failed overall. That original
failure is preserved. The parser correction and necessary input-loss/waiting
boundaries were checked in the fresh R32 native candidate; the passed R31
application stages were not rerun or reassigned to R32's whole-source manifest.

## R32 native candidate and exact artifacts

The [R32 backend result](../artifacts/verification/live-portrait-20261109/candidate-r32/evidence/backend/summary.json)
passes 147 tests, 0 failed or ignored, with dependency versions/checksums aligned
to the production lock. Its component source manifest is
`aa469530b2153c3765acd651337226eb7f840c5e3f1fd8ad8c0f8b7cec2dd4c1`.
Coverage includes evdev ABI/frame/loss handling, optional source errors,
descriptor retirement, actual raw-input contact cancellation and fresh gestures,
GETVAR calibration, preserved mappings/polarity, and submitted-frame waiting
without replaying contacts or drawing repeatedly.

The separate [R32 ARM/package pipeline](../artifacts/verification/live-portrait-20261109/candidate-r32/evidence/20261108T180233Z-46486118/summary.json)
passes ARM compilation and [nine static checks for 28 entries](../artifacts/verification/live-portrait-20261109/candidate-r32/evidence/20261108T180233Z-46486118/package.json).
Its source-input manifest SHA-256 is
`7cdc5e0ad5e7fb9fecca7185e71d87000c4ff18548c46298394340b7b86e3bdd`.
The static ARM executable is 66,093,712 bytes, SHA-256
`7e31def888b037f83cabc3caa33d2b097a057c0e275a329fcffd81b98348aa65`;
the production ZIP is 50,583,537 bytes, SHA-256
`c0fd20e97abd294fc0a254682fac58b8f38abac49bb7afd86ca8e9feac594a1d`.

R33 used the same ARM with a separately identified, user-authorized private
five-second native-read launcher. Its archive SHA-256 is
`6e3a30023eb243c40bccdddec364cdaff0297e1448d9cc04636fbeed6743cd7d`,
with launcher SHA-256
`9858059d65d74596a432dda6663ca041e3d7d287951e835e05b633eb0d29825b`.
The [physical preparation](../artifacts/verification/live-portrait-20261109/final-physical-summary/public/preparation.json)
and [exact test-package check](../artifacts/verification/live-portrait-20261109/final-physical-summary/public/package.json)
retain its candidate binding. The production launcher's two-second budget was
unchanged; physical results retain the private test-launcher scope.

## R33 supervised active rotation and native return

The [R33 run](../artifacts/verification/live-portrait-20261109/physical-r33/run.json)
used fresh offline demo data on the operator's Scribe2022 / 5.19.5. The process
was PID `9454`, birth tick `729057507`. Without sleeping, the operator changed
between upright/upside-down holding and back, confirming automatic artwork
direction and the retained comic page. The operator accepted the requested
operations, selected X, and confirmed native Home direction and touch were normal.

The [active-rotation audit](../artifacts/verification/live-portrait-20261109/final-physical-summary/public/live-rotation-audit-r33.json)
and [original exported lifecycle log](../artifacts/verification/live-portrait-20261109/physical-r33/lifecycle.jsonl)
record holding U/D transitions while native scanout remained D. Residual pixel
offsets were 180/0, and native touch offsets were 0/180. The same PID remained
active, with no suspend/resume event generated for those turns. Reader-ready
records continued to identify page 18. The operator reported a right-side tap
reaching page 19, but an exact numeric 18-to-19 regression result is not assigned
because the retained readiness evidence remained at 18.

The launcher exited 0; the watchdog recorded `launcher-exit` and completed
recovery. Awesome/statusbar baselines matched and the launcher lock was absent.
The [final return guard](../artifacts/verification/live-portrait-20261109/final-physical-summary/public/final-return-guard-r33.json)
records no live BiliComics processes, closed owned device connections, passed
recovery, and operator-confirmed normal native direction/touch.

The exact `live-portrait-20261109-r33` lease for owner
`01a1bba4-bda4-77d1-96ae-aa15eb091ad7` was
[released](../artifacts/verification/live-portrait-20261109/final-physical-summary/public/lease-return-r33.json)
at `2026-11-08T18:34:50.124133+00:00`
(`2026-11-09 02:34:50` Asia/Shanghai). Its token fingerprint is
`8734c6dcb0adf25b4db5c6a3c4ad2586ab26124940475c97d12709bf27c387ac`.
Owner/token checks passed, the shared lease directory was absent, and Kindle was
explicitly returned to WeRead with the coordinated peers notified. Further
device work requires a new exclusive handoff.

The [run export manifest](../artifacts/verification/live-portrait-20261109/physical-r33/export-manifest.json)
and [closure export manifest](../artifacts/verification/live-portrait-20261109/final-physical-summary/public/export-manifest.json)
retain raw/export hashes, sizes, and bounded token redaction. This run has no
PNG/raw framebuffer captures; artwork observations are the operator's direct
reports rather than a derived screenshot acceptance claim.

## Completed scope and limitations

The requested live Up/Down behavior is complete for this scoped development
candidate. The final two-way choice and accepted current sensitivity were
forwarded to WeRead and Aidoku. Each peer owns its implementation and evidence;
the BiliComics results do not assert another project's test pass. KOReader's
four-way behavior remains research context outside the user's selected scope.

The repository hardware acceptance allowlist remains empty. Controlled software
results, this one Scribe/firmware/offline-demo observation, and general hardware
support remain distinct. Real accounts, purchases/downloads, other firmware or
devices, universal sensor freshness, and the exact numeric page-turn step are
outside the physical result. No new stability/debounce timing was introduced.
No local tests, builds, validation suites, or runtime probes were run for this work.
`test-env` supports parallel isolated work with bounded build concurrency;
physical Kindle access alone requires an exclusive owner/run/token lease.
