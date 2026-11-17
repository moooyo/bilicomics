# Scoped startup-sequence results: 2026-11-17

## Decision

Adopt only the initial-SEND background preparation in source
`530f1516c332931059c52161b52d64e86488b3f1`. The measured improvement is modest:
first Shelf SEND acceptance improved by 231.415 ms at the median, with five of
six matched pairs faster. This is a descriptive result for the tested offline
fixture and device, not a guarantee of stable gains for every launch.

Do not adopt the separate hidden-route refresh prototype
`f6208d6f7dd3d8e64a09ef0ceb2ceb6b0c45d0f5`. It reduced redundant complete
replacement frames but did not improve the overall startup endpoint reliably.
The prototype remains on `codex/startup-sequence`; the adopted branch returns
to the measured item 1 runtime. All samples and both experiments remain in the
repository evidence.

## Adopted implementation

The Kindle backend sends one fixed channel message after the real initial full
draw has an accepted, current, nonzero SEND marker and known delivery. The
notification precedes the existing checked wait for that marker. It invokes no
application observer inside the framebuffer borrow.

An independently cancelable receiver admits font-byte and application-metadata
preparation exactly once. Font registration, family activation, Slint attachment,
and the first UI yield keep their existing application boundaries. Startup cover
pixel preparation waits for the original post-completion prepare gate. Demo
metadata still performs its existing asset materialization and header reads.
Suspend prevents UI consumption; cancellation and errors cannot publish Ready.
The waiter is retired before runtime shutdown, including early-return paths.

Desktop and unavailable-hook paths retain serial preparation. The hidden
`--serial-startup-preparation` control requires both `--demo` and `--offline`.
The comparison uses that control in the same ELF; it does not assert binary
equivalence with the earlier main executable.

The existing input, fresh-portrait, framebuffer-write and checked-completion
barriers remain. Persistent binary DU reading, manual clearing, saved grayscale
preferences and normal-mode restoration are unchanged. The release profile
remains ThinLTO, one codegen unit and symbol stripping. No GNU native-query
helper or shared Scriptlet handler/scanner change is adopted.

Selected Aidoku reference blobs, complete licenses and adaptation boundaries are
retained in [the source record](research/aidoku-startup-sequence-20261117/README.md).
Aidoku measurements are not used as BiliComics results.

## Two independent physical comparisons

Each comparison kept two warmups followed by six balanced AB/BA pairs. Every
launch used a fresh application process, with retained caches on one device
boot. Both variants within each comparison shared the exact source, ELF, ZIP,
compiler and build configuration. Only the declared fixture flag differed.
The two experiments have different source/package identities; their timing
reductions must not be added or treated as a direct cross-batch comparison.

| Trial and endpoint | Control median, ms | Candidate median, ms | Candidate faster |
| --- | ---: | ---: | ---: |
| Initial-SEND preparation: first Shelf SEND | 6696.777 | 6465.361 | 5/6 |
| Initial-SEND preparation: software Ready | 5905.555 | 5691.153 | 4/6 |
| Hidden-route prototype: first Shelf SEND | 6509.286 | 6521.791 | 3/6 |
| Hidden-route prototype: software Ready | 5712.526 | 5884.014 | 3/6 |

The first trial's median paired SEND reduction is 182.779 ms. In the route
trial, median paired reductions remain positive (39.451 ms SEND and 26.821 ms
Ready), while the differences of distribution medians favor the control. This
disagreement and the three-to-three split do not establish a reliable benefit;
the favorable paired summary is retained alongside the unfavorable medians.

The clock starts in the device request helper before its marker write/close and
the LIPC/appmgr Scriptlet dispatch exec. Ready uses the real `startup/ui_ready`
timestamp. The business endpoint is the first accepted Shelf SEND whose Slint
render starts after Ready. Neither endpoint proves optical panel completion or
the completion of every background cover.

In the first trial, all seven candidate admissions occurred after initial SEND
acceptance and before the original checked completion finished; all seven
controls started afterward. Font-byte preparation completed within that wait;
metadata overlapped only partly. Cover pixels began after the prepare gate and
completion in all cases. Post-business RSS median increased by 192 KiB; the
recorded sampling is not a continuous startup-peak measurement.

The first trial's slow candidate 03 and 06 each incurred an additional complete
replacement frame between Route and Views before Ready, occupying about
801/815 ms. The route-only prototype removed these candidate replacements, but
other work still occupied the display loop. Its candidate 02/03/06 had
approximately 400–411 ms rendered attempts with no SEND, dominated by waiting
for a previous marker and diagnostic fingerprint work. Candidate font-finish to
application attachment still took about 260–279 ms. Removing a refresh request
alone therefore did not establish an end-to-end speed improvement.

The records show an opportunity to examine bounded control/continuation
coalescing after an existing wait. That is an unimplemented design hypothesis;
this change does not move callbacks into framebuffer borrows, alter the 4 ms UI
budget, add a 300 ms delay, or claim a verified benefit from such a design.

Full records, slow samples, exact timing windows and analyses are retained in
[item 1 evidence](physical/startup-sequence-20261117/item1/README.md) and
[the rejected route experiment](physical/startup-sequence-20261117/route-refresh/README.md).

## Other reference items

| Reference item | BiliComics assessment and disposition |
| --- | --- |
| Covers after first business SEND | Not adopted. Prepared covers currently precede application attachment; simply making their gate wait for that SEND creates a dependency cycle. A streaming split changes the first visible Shelf and needs an actual projected-slot/accepted-frame endpoint. Existing resident or queue counts do not supply that proof. |
| Warm derivative reuse | Bili's managed warm cover cache already contains small prepared PNGs, rather than a separately decoded local original followed by a derivative lookup. No equivalent warm duplicate decode was found. Cold network validation/decode reuse is a separate path. |
| Cover publication coalescing | Mailbox and runtime wake coalescing already exist. Bili has an 80 ms maintenance cadence, not Aidoku's existing 300 ms publication window. No new delay or cache layer is introduced without a separately qualified benefit. |
| Combined sleep-capability observation | Not adopted as a behavior-equivalent optimization. Clean property-unavailable results can be typed separately, but a hung/continuously overflowing command or unclosed descendant stdout cannot consume the existing shared two-second transaction budget and still preserve the old independent optional-query failure behavior. Ownership, deadline and uncertain cleanup failures must remain fatal. The standalone optional observation remains. |
| Batched startup lighting | No startup brightness/warmth setter exists to consolidate. Manual controls remain; automatic brightness is outside requirements. |
| Direct shared dispatch | The reference direct entry is a diagnostic with a different framework path. Shared handler, scanner and registration behavior remain unchanged. |

These are source assessments, not unperformed benchmark results. The capability
combination was not implemented or tested on hardware. Its failure-policy
boundary is visible in the existing
[capture transaction](../src/platform/native_capture.rs) and
[optional launcher observation](../packaging/kindle/launch.sh). The previously rejected
GNU-helper experiment remains separate in
[its original report](NATIVE_QUERY_STARTUP_20261117.md).

## Verification and limits

All verification ran on `test-env`, with Cargo jobs 6, incremental compilation
disabled and one Rust test thread. No Windows-local test or build ran.

- Adopted source: 891 library and 18 main tests passed; four existing benchmarks
  stayed ignored. The complete backend passed 244 tests, four normal/error
  desktop startup fixtures passed, and 26 package regressions passed. The new
  ARM package passed 17 static checks with 45 entries.
- Rejected prototype: 897 library and 19 main tests passed, including seven new
  regressions, plus four startup fixtures and an independent new ZIP check.
  Unchanged backend/package regression results were inherited through exact
  scoped input hashes, with their original execution source retained.
- Each actual ARM build cleaned only this task's app/backend outputs, retained
  dependency caches, and saved fresh app/backend compile announcements and
  actual rustc invocations. Selected ELF/ZIP identities and forbidden-diagnostic
  checks are bound in [software evidence](verification/startup-sequence-20261117/README.md)
  and [prototype software evidence](verification/startup-sequence-20261117/route-refresh/README.md).
- All 28 A/B launches passed their frozen contracts. Each batch restored native
  control, closed owned transports and released its exact fresh lease. Final
  logical pixels matched across all 14 cases within each trial. Raw framebuffer
  comparisons cover only pair 06 in each trial, with every visible pixel
  compared; capture brackets and delayed resource sampling remain explicit.
- The observed device was Scribe, firmware 5.19.5, 1860 × 2480 visible pixels,
  1872-byte stride and eight-bit framebuffer. No live-account, cold-boot,
  optical-quality or broader-device claim follows.
- A separate six-minute assisted reader attempt reached page 18 and exited
  normally with recovery/release, but had no within-window operator feedback or
  corresponding follow-up checkpoints. It remains **incomplete /
  no_operator_observation**; a later reply is not rebound to its retired run.
- A [fresh assisted run](physical/startup-sequence-20261117/assisted-final/README.md)
  tested the adopted ELF without `--performance-trace`. The operator performed
  a physical sleep/wake cycle and observed page 18 retained. Stored trace
  records 29/30 contain a new suspend/resume pair strictly after the before
  checkpoint's 28 records and before the resumed checkpoint's 35 records.
  PID, process birth, ELF identity, reading settings and source anchor remained
  identical. One subsequent finger tap advanced exactly to page 19. The
  operator confirmed native Home and touch after controlled shutdown and exact
  release at `2026-11-17T11:28:12.165059+00:00`. This qualifies that normal-mode
  Bayer8 continuation/input cycle on the observed device, without measuring
  deep-sleep duration, button-to-panel latency or Fast-mode continuation.

Evidence exports use fixed allowlists with source hashes and transformations.
The nested prototype software export has its own manifest; each manifest covers
its declared payloads rather than a recursive census of unrelated sub-bundles.
They exclude credentials, raw ownership tokens, private transport, databases,
native Home images, real account content, ELF/ZIP payloads and full archives.
