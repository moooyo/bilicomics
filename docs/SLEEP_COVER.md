# Comic-cover sleep

Recorded: 2026-11-12 (Asia/Shanghai). Application version: 0.1.23 development.

## Accepted behavior

The operator requested the corresponding sleep-screen optimization from the
WeRead sleep-cover chat. The reference
implementation is the local WeRead revision
`6a1d070f3b4730146eafeea849fe8b67fc5cf14d`. The operator accepted its physical
text-cover demonstration; that acceptance belongs to WeRead and is not a
BiliComics hardware result.

BiliComics prepares its current comic cover while awake, displays it before
synchronous sleep checkpoint work, and retains the live Reader behind a
temporary overlay. Wake hides the overlay before rendering the retained route
and page. Missing images use the prepared comic title and author. A route
outside the active Reader uses a neutral application sleep screen. The complete
cover is fitted proportionally without animation, persistent progress badges,
or a reserved Reader margin.

The existing `sleep_cover` preference is now implemented for Kindle operation
and defaults to enabled for default preferences. Explicit saved preferences are
preserved. The system-screensaver selection remains available. A device profile
alone does not authorize custom sleep or claim hardware support.

## Preparation and resource boundaries

Only the active Reader identifies a comic. Residual reading state after leaving
Reader never selects its old cover. The selection binds account, account epoch,
comic identity, and source URL digest. Late results must match that selection
before they can become the current sleep image. Cache filenames contain only a
digest, not an account, comic title, credential, or source URL.

Ordinary awake work decodes a bounded derivative, at most 1536 by 2048 pixels,
composites transparency on white, and quantizes grayscale to sixteen levels.
The original aspect ratio is preserved. The currently cached list thumbnail is
an immediate fallback while the derivative is being prepared. Only one decoded
sleep image is retained. The source adapter's existing image transport,
inspection, concurrency and worker admission limits remain in force.

Derivative PNGs share the existing managed `covers` directory, atomic publisher,
128 MiB disk budget and 1,024-object limit. Regular-file, no-symlink/no-hard-link,
owner, reparse-point and encoded-byte checks remain applicable. Corrupt cached
derivatives can be refilled while awake. Optional failures use bounded retries;
capacity deferral does not turn into a persistent negative result. Logout and
account-context retirement clear the prepared image and overlay.

The sleep event itself does not read SQL, read an image file, decode an image,
start a download, or acquire a new source capability. It consumes already
prepared image/text state. Checkpoint and payment-receipt persistence retain
their existing ordering and uncertainty semantics.

## Checked presentation and supervisor protocol

1. The supervisor receives `goingToScreenSaver`. Only controlled, still-stopped
   native window-manager identities, a known non-advertisement configuration
   and absence of native passcode configuration permit a custom-cover request.
2. `sleep-cover-request` contains the exact current lifecycle record:
   `generation:token:sequence:suspend`. The application gates ordinary input and
   transport, shows prepared state, requests a renderer token and yields.
3. The native renderer waits for the preceding update before rewriting mapped
   pixels, renders the complete overlay, submits a checked full update and waits
   for that independent update marker. A property change, redraw request, queued
   refresh or failed ioctl cannot acknowledge presentation.
4. The existing maintenance turn observes the matching completed token. Only
   then does synchronous display/input suspension and checkpoint work begin.
   The cover remains visible while that work completes.
5. Only successful presentation, safe display suspension and a saved checkpoint
   can publish matching `sleep-cover-ready`, followed by the lifecycle `ok`
   acknowledgement. A checkpoint error retains its original explicit outcome.
6. With current capability and exact matching records, the supervisor retains
   native display ownership. It does not resume the native window manager,
   restart the status bar or call `xrefresh` over the cover. Other outcomes use
   the stock handoff. No new `powerd` policy or keep-awake mechanism is added.
7. A current user-wake event and bounded confirmed active power state authorize
   custom wake. Fresh native polarity and portrait/scanout calibration are
   observed before the first full Reader frame; input draining still precedes
   normal work. A CPU/RTC wake never acquires the display or input.

Requests, deferred callbacks and acknowledgements bind the entire current
identity. Replacement requests, stale markers, multi-line records, timeout,
presentation errors, disabled preferences and missing capabilities cannot
manufacture cover success. Native passcodes and Special Offers retain stock
handling. Normal exit, USB takeover and failed supervision retain native cleanup.

## Verification

Verification runs on `test-env` in the isolated workspace
`/tmp/bilicomics-sleep-cover-20261112-r1`. Its root disk was full, so a private
mount namespace uses historical Cargo and Zig caches as read-only overlay lower
layers, with every write confined to this run's temporary upper layers. The
same run's overlay mounts are serialized; unrelated remote work is not locked.
The physical Kindle lease is not acquired for source preparation or these
software checks.

The final source manifest is
`6b5c071814fc0b65bc239054cf8f4933afe49fb2f9df2d78da4e84d62ab401d8`;
the pipeline manifest is
`fa73851b52f18a3a9f391d470f45ade8935dd512ab70dc339fe37388925361a0`.
Inputs and the sealed desktop binary remained unchanged throughout the final
runtime/package run. No local application verification was performed.

| Check | Result | Scope |
| --- | --- | --- |
| Application with `kindle`, all targets | 696 library tests, eight Main tests and four integration tests passed | Three library timing benchmarks, five provisioned-WASM cases and one public-network case retain their existing ignored status. No account or public-network test was substituted. |
| Native backend | 170 passed, none ignored | Isolated backend crate; all 465 dependency versions/checksums matched the production lockfile; no framebuffer opened. |
| Launcher | 124 passed; six focused wake regressions also passed | Isolated Linux service/process fixtures, exact final launcher bytes. |
| Sleep-screen rendering | 20 cases and 24 PNGs across four layouts passed | PW5/Scribe portrait and landscape, complete covers, leading text glyphs, neutral fallback and exact retained Reader pixels/source anchors. All final PNGs were visually reviewed. |
| Reader, unavailable-page and startup regressions | Nine Reader, five guest-page and two startup cases passed | Sealed real desktop application, fictional/offline data. |
| Process sleep/wake | Passed | Production launcher, simulated services and real Rust process; same live reading session and checkpoint-error recovery. |
| ARM development package | Built; ten static checks passed for 32 allowed entries | Static ARM ELF32 EABI5 hard-float binary, CRC/digests/modes/notices checked; not device execution. |

The first launcher run exposed a retained-cover wake retry being rejected by a
stock-only display-ownership guard. The guard was corrected; the original active
power-state assertions and the complete 124-test suite then passed. Initial Rust
checks also caught a missing qualified domain type and a UI-test accumulator type;
both were corrected. The one-off remote wrapper initially omitted the standard
runner's private `077` umask, so lifecycle fixtures rejected their directories.
The wrapper was aligned with `run_suite.py`; production permission checks were
not weakened. Screenshot review caught leading text being clipped when long
multi-line labels were vertically centered. Bounded preferred-height positioning
and explicit leading-glyph checks now preserve both the prefix and short-label
centering. Initial failures and their corrected runs remain in the evidence.

The final host build and ARM build retain existing compiler/vendor warnings and
the Zig `-nostartfiles` warning; no warning-free build is claimed.

The [development ZIP](../artifacts/verification/sleep-cover-20261112/bilicomics-kindle-0.1.23-dev.zip)
is 50,961,428 bytes, SHA-256
`962053cb0d0402aa2748865d1d78d41320f87e2ec8101df6d13a4af9587b3758`.
Its 66,911,216-byte ARM executable has SHA-256
`92736c132ba26dbf6a4b35961d41b178005bec21a28df3eff78e55ee1d39206f`.
The sealed desktop executable has SHA-256
`7da137a2a9dd73eb5b74f6ead1bb1d1b3228ee1d71f67273d761740ffa859828`.
The exact launcher SHA-256 is
`917c48ac3838681975834e11023a71579d985df5c38538fa708e1069a754d111`.

Local copies of the [runtime/package report](../artifacts/verification/sleep-cover-20261112/evidence/final/summary.json),
[backend report](../artifacts/verification/sleep-cover-20261112/backend-summary.json),
[sleep UI report](../artifacts/verification/sleep-cover-20261112/sleep-ui-final/report.json)
and raw logs are under `artifacts/verification/sleep-cover-20261112/`. The public
source snapshot, reports, logs and sealed outputs remain in the isolated remote
workspace; this run's writable build-cache layers are disposable.

These results do not establish a numeric
power-button-to-cover latency, kernel-suspend timing, physical panel quality,
real-account cover acquisition, or a new firmware compatibility claim. The
optional application diagnostic measures application sleep handling to observed
update completion; it does not start at the physical power-button press.
That software run did not connect or deploy a physical Kindle and did not acquire
a device lease. The separately authorized device follow-up is recorded below.

## Scoped physical follow-up

On 2026-11-12 (Asia/Shanghai), the operator requested an immediate Kindle test.
The previously identified Scribe 2022 and exact firmware 5.19.5 were selected;
preflight and the read-only framebuffer helper checked the actual firmware and
display geometry before starting the application. WeRead confirmed a fresh
handoff, and Aidoku was notified of the bounded batch and subsequent return.

The coordinator acquired a fresh owner/token-bound lease immediately before
actual device access. Run `cover-20261112-r56-3fb6dbf1`, owned by chat
`01a1cbf1-1191-7730-a4ee-aaa5d7600f7d`, used a new host evidence directory and
`/mnt/us/bilicomics-physical-cover-20261112-r56-3fb6dbf1` on the device. It deployed
the exact frozen 0.1.23 ZIP, main executable and production launcher identified
above. Existing application installations, credentials, comics and progress were
not replaced. The Reader ran the built-in isolated offline demonstration data.

Two physical-harness adaptations were prepared and tested before acquiring the
lease: the independent watchdog recognizes the four exact cover protocol leaves
without weakening foreign/unknown-entry checks, and demo recovery performs the
same real PID/birth snapshot used for distinct fixtures. All 23 selected host
fixtures passed. These harness changes do not change the frozen ARM application
or production launcher. Their source hashes and separate software evidence are
under `artifacts/verification/sleep-cover-20261112/harness-prep/`.

The controlled stock `powerd_test -p` cycle displayed the prepared bitmap comic
cover. While `powerd` reported `screenSaver`, current cover request/ready records
matched the lifecycle request and successful checkpoint acknowledgement; the
native window manager and statusbar remained stopped. The cover was retained
for approximately four seconds. This was an actual-device power-daemon event
test, not a physical-button latency measurement. The harness deliberately did
not capture framebuffer memory while suspended.

Wake returned to the same application PID/birth and complete reading source
anchor. The before, suspended and resumed checkpoints retained page 18. An awake
capture was taken before the cycle and after wake. The operator
then reported normal cover display and a successful real finger tap to the next
page. A separate captured checkpoint verified exactly one source-page advance
to page 19. Collected application diagnostics contain presentation observations
of 1805 ms and 1792 ms; these include application handling, checked EPDC completion
and the observation interval, and do not start at a physical-button press. The
single coordinator-requested power-event cycle is distinct from other observed
native events whose source was not inferred: the saved trace contains two
`suspend -> resume` pairs, with two matching retained-cover/wake-barrier events.
This establishes the scoped bitmap-cover experience
and post-wake application touch observation; it is separate from the WeRead
title/author-only physical result.

Normal shutdown restored the saved statusbar/window-manager baseline, removed
the owned launcher lock and passed the actual owned-process snapshot: all 32
recorded PID/birth identities were gone. All
nonmultiplexed device transports ended before exact owner/token-checked lease
release at 2026-11-12 03:54:57 +08:00. A host-only check confirmed lease absence;
peers were notified immediately, and later evidence analysis used host files
only. Native Home and native touch did not receive a separate operator visual
confirmation in this batch.

The isolated test deployment is an audit artifact, not an update to the user's
normal application entry. No account login, download, purchase, long sleep,
kernel low-power timing, battery measurement, rapid reversal, password/advertising
fallback, orientation stress or other model/firmware claim is made by this run.
The production acceptance allowlist remains unchanged. The public, sanitized
[device summary](../artifacts/verification/sleep-cover-20261112/physical-r56/summary.json)
and its 14-file export are under `artifacts/verification/sleep-cover-20261112/physical-r56/`;
private transport settings and raw run tokens remain outside the repository on
the verification host.
