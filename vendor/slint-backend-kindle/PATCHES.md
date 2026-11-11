# Project-owned queued refresh patch

## Checked sleep presentation

The sleep-frame completion contract was adapted from the reviewed local WeRead
revision `6a1d070f3b4730146eafeea849fe8b67fc5cf14d` (2026-11-12). Its reference
`src/sleep_presentation.rs` SHA-256 is
`f152a12e9c525c7a09d96ae5f47a717a821ca456e58cc3895cd48d95c873f75a`.
The existing upstream license notices and pinned backend revision remain intact.

`DisplayLifecycleHandle::request_sleep_presentation()` returns a UI-thread token.
The renderer captures the pending token before its complete Slint repaint,
waits for the preceding accepted update before rewriting mapped pixels, requires
acceptance of the full GC16 update, and checks completion of that update's own
marker before acknowledging the captured token. Neither queued properties nor
a redraw request establish presentation. Suspend/resume cancel unfinished
requests; event-loop termination retires all tokens.

Sleep presentation has a separate completion boundary from resumed/live-rotation
input draining. Its completion releases the timer gate without waiting for a
new touch SYN_REPORT, allowing the application's deferred persistence and timeout
to advance while ordinary application intents remain inhibited. The next actual
suspend releases touch, and wake retains the existing fresh-input drain barrier.

Every attempted update consumes a unique nonzero marker across all detected
driver ABIs. Rejected submissions do not replace the most recent accepted marker,
and repeated completion waits reuse only a successfully observed completion.
Ordinary refresh policy, orientation/polarity compensation, and performance
submission records remain in place. Controlled tests cover token cancellation,
termination, timer release, marker ordering, submission/wait failure, and ABI
selection without opening a device framebuffer. These source-level contracts
do not establish physical-device latency or panel quality.

## Provenance

The baseline is [`KindleTweaks/slint-kindle-backend-patch` at `03aa48d3531e9eae7b360bdc24e90a72e9f307a4`](https://github.com/KindleTweaks/slint-kindle-backend-patch/tree/03aa48d3531e9eae7b360bdc24e90a72e9f307a4). `UPSTREAM_REVISION` records the same identity. The original `LICENSE-MIT` and `LICENSE-APACHE` files are preserved without changes. This dependency's license metadata does not select the application's project license.

Only the library sources, manifest, README, and licenses are vendored. Example applications, screenshots, and unrelated repository material are omitted. The manifest's example workspace stanza is removed because those examples are not part of this source distribution. The upstream Slint `=1.17.1` requirement and Rust 1.92 minimum are retained.

The upstream manifest declares `fontique 0.11` and `fontdb 0.24`, whereas Slint 1.17.1 actually resolves `fontique 0.10` and `fontdb 0.23`. The compatibility patch changes the direct declarations to those actual versions. `fontique`'s `fontconfig-dlopen` feature then applies to the same instance used by `i-slint-common`, enabling `yeslogic-fontconfig-sys/dlopen` and avoiding a required native `libfontconfig` link. The application supplies its fonts explicitly. Direct dependencies with `default-features = false` do not turn off transitive default features; `fontdb`'s fontconfig parser is Rust code rather than a native fontconfig link.

## Public extension

`KindleBackend::refresh_handle()` returns a cloned `RefreshHandle` that is `Send + Sync`. `RefreshHandle::request()` accepts:

- `RefreshWaveform::{Auto, Gc16, Du, A2}`;
- a `flashing` flag;
- `RefreshArea::{Dirty, Full, Rect(PhysicalRect)}`.

Rectangles use physical window pixels before render rotation. Empty rectangles are rejected immediately. Other rectangles are intersected with the actual window geometry and rotated into panel coordinates before an update is submitted. Signed positions and unsigned dimensions use widened arithmetic, preventing wraparound when a caller supplies a negative origin or a very large size.

A successful call means that the request entered the producer queue. It is not a completion receipt or evidence of panel quality. Requests after event-loop termination are rejected. The queue holds at most 32 requests; overload collapses to one conservative GC16 full-screen request rather than growing memory without a bound.

## Rendering order

The producer only appends to a mutex-protected queue and nudges the existing wakeup pipe. It does not open a framebuffer or call an ioctl.

On the event-loop thread, application closures, touch events, and Slint timers run first. The backend snapshots the pending requests, renders the current scene, and writes all dirty pixels through its existing framebuffer object. Only after `draw_if_needed()` returns does it resolve pending regions and call the existing detected Legacy, Rex, Zelda, or MTK driver implementation. Requests arriving during rendering stay queued for the next awakened iteration.

The submitted region includes both valid requested rectangles and any dirty pixels written in the same frame. Conflicting requests choose the strongest requested waveform, and a flashing request uses GC16. A rotation change retains the original full-screen GC16 path. Without any explicit request, rendering retains the original AUTO partial update for the dirty region.

## ABI constants

The baseline already defines GC16 as 2, AUTO as 257, and the four update ioctl structures. The additional DU and A2 values are reviewed against [`koreader-base` at `6e4bc81af4e04f78d81677a323a780a71b29702a`, `ffi/mxcfb_kindle_h.lua`](https://github.com/koreader/koreader-base/blob/6e4bc81af4e04f78d81677a323a780a71b29702a/ffi/mxcfb_kindle_h.lua):

| Waveform | Legacy | Rex / Zelda / MTK |
| --- | --- | --- |
| DU | 1 | 1 |
| GC16 | 2 | 2 |
| A2 | 4 | 6 |

DU requests set the reviewed `EPDC_FLAG_FORCE_MONOCHROME` value of 2. A2 falls back to AUTO when the driver family is not yet known or is unsupported. No new hardware inversion or pen support is introduced.

The application adapter maps UI and reading partial requests to AUTO, press feedback to DU, and flashing/full requests to GC16. A2 is exposed for a future measured policy but is not selected by the application's initial adapter.

## Changed files

- `src/refresh.rs`: producer handle, bounded queue, region resolution, and controlled tests.
- `src/clipboard.rs`: two application-local clipboard slots and pure standard-library tests.
- `src/touch.rs`: convert normalized panel pixels to Slint logical coordinates using the current window scale after undoing rotation; provide bounded opt-in press/release diagnostics without changing input selection or conversion.
- `src/lib.rs`: public refresh API and handle preservation across backend typestates.
- `src/platform.rs`: queue wiring, suspend eligibility, post-render execution, and termination state.
- `src/wakeup.rs`: make the existing pipe nudge usable inside this crate and retry interrupted writes.
- `src/framebuffer/mod.rs`: execute resolved plans through the existing driver and map driver-specific waveforms.
- `src/framebuffer/ffi.rs`: reviewed DU/A2 and monochrome constants.
- `Cargo.toml`: omit the unavailable example workspace and align font dependencies with Slint 1.17.1's actual resolved versions and dlopen requirements.

## Scoped input diagnostics

`SLINT_KINDLE_INPUT_DIAGNOSTICS=1` enables at most sixteen press/release records
on stderr for one process. The default is disabled. Records describe raw
multitouch and legacy axis values and advertised bounds, framebuffer dimensions,
window size and scale, render offset, application rotation, and transformed
pointer coordinates. They omit credentials, account content, device serials,
and environment dumps. The environment flag is an internal supervised diagnostic
input, not an application setting.

The render loop supplies its existing launch render offset to the diagnostic
state. This adds observation only; the original coordinate selection, rotation,
and physical-to-logical conversion remain in use until a measured correction is
identified. Input observations must still be attributed to actual operator
touches separately from emitted coordinates.

## Application-local clipboard

The native platform implements [Slint 1.17.1's public platform trait](https://github.com/slint-ui/slint/blob/v1.17.1/internal/core/platform.rs): `Platform::set_clipboard_text(&self, text: &str, clipboard: Clipboard)` and `Platform::clipboard_text(&self, clipboard: Clipboard) -> Option<String>`. `DefaultClipboard` and `SelectionClipboard` use separate owned text slots. Unknown future clipboard kinds are ignored or return `None`.

The store lives only in this application process's RAM and is dropped with the platform. It does not read or write files, call a Kindle service, or provide a system-wide clipboard across applications. Public Slint `TextInput.copy()` and `TextInput.paste()` use these platform methods without a Rust private API or a firmware-specific contract.

## Native touch coordinates

Touch controller axes remain normalized to the physical framebuffer dimensions. Press, move, and release events use one conversion that first undoes the existing panel rotation and then divides both coordinates by the current Slint window scale factor. This preserves the upstream rotation mapping and contact-slot tracking while aligning native touch events with a scaled logical window. Logical pointer events dispatched directly by the application already use logical coordinates and do not pass through this conversion.

The added regression vectors cover a noncentral point and the logical origin at scale 2 for rotations 0, 90, 180, and 270 degrees. They exercise the pure coordinate conversion without opening an input device or framebuffer. Controlled-test and firmware-emulator results are recorded below; these vectors alone do not establish physical-device support.

## Verification boundary

Controlled tests exercise empty/outside rectangles, integer bounds, rotation, region merging, full requests, default AUTO behavior, queue saturation, cross-thread producers, termination, and driver-specific waveform mapping without opening a Kindle framebuffer. Remote ARM cross-compilation checks the code and ABI layout assertions for the intended build target. Neither test class establishes a physical model/firmware support claim, ghosting quality, or panel timing.

On 2026-11-03, `cargo test --lib --manifest-path vendor/slint-backend-kindle/Cargo.toml` passed all 11 dependency tests on `test-env`, including nine refresh/driver tests and two retained upstream touch tests. The queue tests used the actual nonblocking wakeup pipe. No framebuffer was opened by these tests.

After adding the clipboard extension, five additional pure standard-library tests passed remotely with `rustc --test src/clipboard.rs`. They cover initial emptiness, separation of selection and explicit copy, owned copy/paste values, replacing old text with empty text, and preserving Unicode and paragraph boundaries. These tests use no display or firmware service.

After the native touch-scale fix, the complete standalone dependency suite passed 17 tests on `test-env`, including the new four-rotation/scale-2 coordinate vectors. The pinned PW6 firmware emulator independently reproduced the old physical-coordinate mismatch and exercised corrected QEMU-injected taps through the guest input driver. These are separate controlled-test and emulator observations; neither establishes physical-device input acceptance.

## In-process system screensaver lifecycle

`KindleBackend::lifecycle_handle()` returns a cloned `DisplayLifecycleHandle`
for the UI thread. It does not implement `Send` or `Sync`. Its `suspend()` and
`resume()` operations are independent of the upstream optional `WakeSchedule`;
this extension does not write `/sys/power/state`, set an RTC alarm, or interpret
firmware power events. The application and launcher own that protocol.

The application inhibits its intents before calling `suspend()` and releases
any application `RefCell` borrow before pointer cancellation callbacks. The
backend publishes its display gate, cancels the existing Slint pointer with
`PointerExited`, drops the exclusive touch input lease, clears queued refreshes,
and waits for the existing best-effort panel-update completion call. A successful
return acknowledges input release and the no-further-write gate, not physical
panel timing. Repeated suspend calls do not issue another wait ioctl after the
first completed handoff. Failure still leaves the gate in place. A UI-thread
RAII transition guard rejects reentrant suspend/resume callbacks, preventing a
nested cancellation from acknowledging release before the old input is dropped.

Sleeping event-loop iterations poll only the existing wakeup pipe indefinitely.
They continue to execute posted control closures and accept quit requests, but
do not dispatch touch, run Slint timers or animations, draw, write framebuffer
pixels, or issue display updates. `RefreshHandle::request()` rejects sleeping
requests with `RefreshError::DisplaySuspended`. Device initialization is lazy,
so a process suspended before its first loop never opens or grabs input, fills
the display, or flashes the system screensaver.

`resume()` opens a fresh input lease when native geometry is available and
requests a complete frame. Before ordinary input and timers resume, the event
loop switches the renderer from `NewBuffer` back to `ReusedBuffer`, writes the
entire scene through the existing framebuffer, and submits a GC16 full update.
It then discards input accumulated during this redraw. This is necessary because
the system screensaver may have modified the shared framebuffer while the
software renderer still retained its old pixels. A failed input reacquisition
keeps the backend suspended. No application route or reader object is destroyed.

Touch queue draining preserves Protocol-B slot selection. Opening input reads
the current slot through the already used `EVIOCGABS` interface when supported;
unsupported slot queries fall back to slot zero, while other failures are
reported. Cancellation never synthesizes `PointerReleased` or a click. These
changes do not introduce multi-pointer or pen support.

The extension's mock tests use fake input leases, a headless software window,
and ordinary pipes. They do not open a framebuffer or real input device.
Physical screen handoff, lock-screen interaction, panel completion, driver
behavior, suspend power consumption, and wake timing still require acceptance
on an explicitly identified device and firmware.

Targeted checks on `ssh test-env` passed all ten added lifecycle regressions:
six transition/queue tests, two headless touch/pipe tests, one suspended producer
test, and one real paused control-loop/quit test that opens no native devices.
The other dependency tests were compiled but were not rerun by these filters.
These are controlled software results, not a physical suspend/resume result.

## Resumed evdev position snapshots

Reopening an evdev client does not reset the input device's absolute positions.
Linux filters unchanged Protocol-B slot coordinates, so a fresh tracking ID or
`BTN_TOUCH` press can arrive without either position axis. Initializing a new
lease at `(0, 0)` would turn a repeated right-side tap into a left-side tap.

The input lease now checks the `ABS_MT_SLOT` capability bit, reads the axis
ranges and current slot, then reads both position arrays with `EVIOCGMTSLOTS`.
`EVIOCGABS` succeeding with a zero-filled entry does not prove that the slot axis
exists. The slot-position buffer contains one 32-bit axis
code followed by one signed 32-bit value per slot. The slot range must start at
zero, contain its current selection, and fit the bounded 256-slot allocation.
Incomplete snapshots and unsupported or failed Protocol-B position queries are
reported instead of silently substituting an origin or a guessed axis range.
An owned descriptor closes and releases its grab if these initial queries fail.
Initialization discards the fd's older queue without interpreting its implicit
slot prefix, reads a complete snapshot, and checks for visible evdev events queued
during the queries. A visible race causes a bounded discard-and-snapshot retry;
only a snapshot with an empty visible queue is published to the position cache.
Persistent activity fails initialization explicitly rather than mixing two slot
timelines. This is not an atomic snapshot across input frames: the input core can
retain an incomplete frame until `SYN_REPORT` even while evdev reports `EAGAIN`.

`EVIOCGABS(ABS_MT_POSITION_X/Y).value` is deliberately not used as a position:
Protocol B stores coordinates in its slot array, and Protocol A does not keep
these MT absinfo values current. Legacy MT devices whose bitmap lacks a slot
axis start with unknown coordinates and cannot dispatch a press until both X
and Y have actually been observed. The existing MT device selection and
`BTN_TOUCH` handling remain in place; this does not add classic single-axis,
pen, or multi-pointer device support.

After initialization, post-redraw drains retain slot selection and all position
deltas while discarding contact transitions. Ordinary input also maintains
each slot's position, including secondary slots, without moving the tracked
pointer to another contact. Neither a snapshot nor a drain restores a pressed
pointer: a later fresh tracking ID or `BTN_TOUCH` press is still required. A
release reads its tracked slot before clearing pointer ownership. No per-contact
ioctl is introduced, because a device snapshot during queued event playback
could read a future position out of order.

Protocol-B devices advertising both `ABS_MT_SLOT` and `ABS_MT_TRACKING_ID`
establish and release pointer ownership only through the slot-specific tracking
event. Their global `BTN_TOUCH` reports are ignored, including reports that
precede a slot change. Non-slot legacy devices retain their combined BTN/ID
stream, including BTN-only release. Snow-protocol devices without tracking IDs
continue to use `BTN_TOUCH`.

The kernel behavior and ABI were reviewed against Linux v4.9 primary sources:

- [`drivers/input/input.c`](https://github.com/torvalds/linux/blob/v4.9/drivers/input/input.c):
  absolute-event filtering and per-slot position storage.
- [`include/uapi/linux/input.h`](https://github.com/torvalds/linux/blob/v4.9/include/uapi/linux/input.h):
  `EVIOCGMTSLOTS` request and buffer layout.
- [`drivers/input/evdev.c`](https://github.com/torvalds/linux/blob/v4.9/drivers/input/evdev.c):
  slot-zero-based position array copying.

Seven added regressions use fake kernel-query results, ordinary pipes, and two
actual headless Slint touch areas. They cover an unchanged tap after a nonzero
current-slot reopen, a single changed axis, secondary-slot isolation and later
reuse, discarded wake coordinates without replaying its contact, unknown legacy
positions despite a successful zero-filled slot query, malformed slots,
incomplete snapshots, failed ioctls, older implicit-slot queue prefixes, and
bounded snapshot retries, and global BTN ordering before a new slot's tracking
ID and release, plus legacy non-slot tracking with BTN-only release. No test
opens a native input device or framebuffer. Remote software-suite execution and
firmware-emulator replay of the same physical point are separate checks; neither
establishes physical-device touch or sleep acceptance. No local verification was
run for this change.

## Native output-polarity compensation

`DisplayLifecycleHandle::set_output_inverted(bool)` configures only the
logical-gray-to-framebuffer-memory conversion. It does not read or write a
system EPDC mode, change a persistent native color preference, or change
application UI colors or reader pixels. The application and launcher must
establish the actual native output mode before supplying this value. The
library default remains `false` for existing consumers.

The stored flag can be set before the event loop starts or while suspended.
The setter never opens a framebuffer or input device, writes pixels, issues a
refresh ioctl, acquires touch, or removes the suspended gate. A new framebuffer
inherits the flag before its initial logical-white clear or first scene write.
Both `fill()` and `write_line()` apply one XOR mask at the same output boundary.
The 8-bit path writes the complemented gray byte when enabled; the 32-bit path
replicates that gray into RGB and keeps alpha opaque. Row padding is untouched.
Slint's reused RGB buffer and application image caches retain logical pixels.

A changed value schedules the existing full scene rewrite and GC16 request.
The event loop uses `NewBuffer` and then restores `ReusedBuffer`, writes the
entire scene under the new polarity, and only then submits its full update.
This avoids leaving unchanged regions encoded with the previous polarity.
While asleep, the rewrite remains deferred until the next successful resume.
Repeating the current value schedules no extra redraw or wakeup. Calls after
quit fail even when the value is unchanged. Borrow and reentrant-transition
failures leave the configured flags and redraw state unchanged.

The added tests call the production fill and span-write methods on anonymous
memory mappings with ordinary owned pipe descriptors. They cover the complete
gray gradient, logical-white initial clears, both polarity directions,
segmented and repeated dirty spans, opaque alpha, untouched padding, and the
composition of upstream software pixel intent with native output inversion.
Headless lifecycle tests cover pre-start staging, lazy framebuffer inheritance,
initial sleep, a sleeping retained mapping, full-rewrite scheduling, same-value
no-ops, conflicting borrows, reentrant transitions, and quit rejection. They
do not open `/dev/fb0` or a native input device. These tests have been added but
not executed in the editing environment; remote execution and physical
acceptance are recorded separately by the application task. Scheduling a GC16
request does not establish that a physical panel completed an update.

## Read-only framebuffer observation and optional resume timing

`DisplayLifecycleHandle::probe_output_inversion()` opens `/dev/fb0` read-only
and issues only `FBIOGET_VSCREENINFO` and `FBIOGET_FSCREENINFO`. It never maps
framebuffer memory, writes pixels, requests refresh, opens input, or changes
the display gate. Each call observes fresh metadata rather than retaining the
startup polarity. The existing profile boundary validates the approved
geometry before backend startup. This probe additionally checks nonzero
visible geometry and stride, zero panning offsets, packed-row/visible-length
arithmetic, and exact geometry, stride, byte size, and pixel depth against an
already retained framebuffer. A fresh metadata change with unchanged visible
dimensions is still rejected before input can be reclaimed. The probe copies
the retained layout and does not remap or modify its pixels.

Only 8-bit grayscale codes 1 and 2 are decoded as normal and inverted output.
The reviewed primary-code references are FBInk's
[`eink/mxcfb-kindle.h`](https://github.com/NiLuJe/FBInk/blob/master/eink/mxcfb-kindle.h)
lines 125-126, content SHA-256
`6e657f7c201146326f8b9bcae8a9c8308549baee73b1aefe7d6674fd341afbdd`,
and [`fbink.c`](https://github.com/NiLuJe/FBInk/blob/master/fbink.c) line 5953,
content SHA-256
`8e498e07d78a9f744a9c429457ab46b2709a5685410e33d645527d9b6ddcf962`.
These are reviewed content identities, not a claim that the moving branch
links remain unchanged. Other valid depths or grayscale codes return `None`
for the consumer's legacy mode source. Metadata/I/O errors return an error.
The existing 32-bit rendering and output-compensation paths are unchanged.

`DisplayLifecycleHandle::set_resume_trace_path(Option<PathBuf>)` stores only an
optional path. It performs no file I/O and retains no application callback or
UI object. The default is disabled. A successful suspended-to-active transition
arms one receipt; repeated active resumes do not arm another. After the first
resumed complete scene has been written and its full refresh requested, the
existing `complete_full_redraw()` boundary consumes the receipt before input
draining. Re-suspension, a changed/disabled path, and quit cancel an unrendered
receipt. Event-loop cleanup also drops the configured path.

The single JSONL event is named `native-resume-frame-rendered`. It records the
process ID, a finite `/proc/uptime` sample when available, current output
compensation, `framebuffer_pixels_written=true`, `refresh_requested=true`, and
`physical_panel_update_confirmed=false`. Existing refresh submission does not
return a physical-completion or ioctl-success receipt. Diagnostics add no
acknowledgment wait, callback into the application, retry, flush, or intentional
delay. The destination is opened append-only with nonblocking/no-follow flags
and must be a regular file; a diagnostic failure only warns and cannot fail
the display transition. A failed write is consumed rather than being relabeled
as a later frame. Optional file I/O is not a storage-latency guarantee.

Added controlled tests use fake metadata/query failures, ordinary wake pipes,
headless lifecycle handles, and injected trace writers. They cover normal and
inverted grayscale, unknown formats including 32-bit fallback, invalid layout
and expected geometry, same-size retained stride/depth drift without mapping
or sleep-state mutation, arithmetic bounds, read errors, default-disabled
tracing, first/repeated/failed resumes, re-sleep/path/quit cancellation, writer
failure, and safe uptime/record formatting. They do not call the live probe or
open a real framebuffer/input device. No local or remote verification was run
while editing this change; the application task records remote and physical
verification separately.

## Owner-calibrated native portrait orientation

`NativePortraitOrientation::{Up, Down}` is distinct from app rotation and output
polarity. `DisplayLifecycleHandle::set_native_portrait_orientation()` stages the
owning application's authenticated direction before initial native I/O or while
suspended. It opens no device and writes no pixels. Changed values on an already
active, initialized display fail so an existing pointer grab cannot cross a
direction change; repeated values are no-ops, and calls after quit fail. Staging
requests the existing full redraw, GC16 submission, and stale-input drain.

The event loop reads the staged offset after control dispatch on every active
frame. Renderer rotation, window geometry, and refresh-area transformation use
the same combined app rotation plus native offset. A configured owner bypasses
the old late startup LIPC query; unconfigured library consumers retain that
bounded legacy fallback. Touch in this compatibility mapping reverses only app
rotation and applies the logical scale once. It assumes native touch coordinates
already match the display; owners with raw evdev coordinates use the calibrated
framebuffer-scanout mapping below.

`probe_native_framebuffer()` and its lifecycle equivalent return one fresh,
read-only `NativeFramebufferObservation` with output polarity and raw kernel
rotation. They reuse framebuffer GET information and retained-layout validation
without mapping pixels, opening input, modifying display state, or spawning a
firmware command. Raw rotations outside 0 through 3 fail. Absolute rotation
codes do not establish a universal portrait mapping. The application's owning
launcher supplies a stable pre-takeover direction/rotation calibration; each
sequence-bound wake observes a fresh relative zero or half-turn change. No LIPC
query is added to the application's paused-window-manager wake path.

The polarity-only API preserves its older behavior: unknown valid pixel formats
return `None` and unrelated rotation codes do not affect polarity decoding.
`native_accelerometer_available()` reuses the existing input-inventory detection
and preserves read errors instead of silently choosing fixed portrait mode.

Added headless lifecycle and injected-metadata tests cover pre-I/O/sleep staging,
same-value behavior, retained geometry, complete redraw, reentry/quit rejection,
explicit-owner bypass of the late query, raw-rotation validation, and layout
drift. They have not been executed while editing this change. Remote software
checks and physical orientation/touch/sleep acceptance remain separate evidence.

## Calibrated framebuffer scanout and raw touch

`DisplayLifecycleHandle::set_native_framebuffer_portrait_orientation()` is an
owner-selected alternative for a driver that already rotates framebuffer
scanout. It stages zero native pixel offset and a separate raw-touch correction:
zero for `Up`, 180 degrees for `Down`. Renderer rotation, window dimensions, and
refresh rectangles use the display path; touch reverses app rotation plus the
native touch correction before applying the logical scale once. The existing
portrait setter and unconfigured consumers retain their compatibility behavior.

The owner must establish the device mapping independently of raw `fb.rotate`
codes. BiliComics selects this path only for its observed, approved
`scribe/5.19.5` pair after authenticated startup or resume calibration. Staging
still requires pre-I/O or suspended ownership, preserves output polarity and
retained mappings, and uses the existing complete redraw and input-drain barrier.
Input diagnostics distinguish app, display, and touch rotation and retain the
process-wide sixteen-record limit.

Added controlled tests cover both directions with four app rotations and actual
Slint corner controls at scale two, legacy behavior, rejected partial staging,
and resume geometry, polarity, and input barriers. These source tests do not
establish physical display or touch acceptance.

## Independent holding and scanout portrait directions

The owner-selected `set_native_framebuffer_portrait_presentation(holding,
scanout)` API stages the desired holding direction independently of calibrated
native scanout. Pixel compensation is the residual between those directions;
raw touch uses holding direction with the existing single scale division. The
two older setters preserve their compatibility contracts, and staging keeps the
pre-I/O/suspended transition checks, full rewrite, GC16, and stale-input drain.

The independent `portrait_pose` module queries read-only evdev capabilities,
device name, and `EVIOCGABS(ABS_PRESSURE)` without consuming events or grabbing
input. Eligibility requires pressure-only axes and no digitizer keys. The
reviewed kx132 contract follows KOReader commit
`9040116701c888889bd35233b23b83283be66745`; no Lua implementation was copied.
The supervised Scribe / 5.19.5 device actually reported `bma4xy_feature`, with
raw 16/native D and raw 15/native U confirmed through read-only observations.
That exact name is recognized alongside kx132. Other names remain unrecognized;
unknown values and multiple recognized sources fail explicitly.

Controlled coverage includes four actual Slint pixel/raw-input combinations,
retained geometry and input barriers, capability/name filtering, driver values,
and ambiguous-source rejection. The matching application encloses framebuffer
and pose queries in its existing protected resume transaction. Software results,
scoped physical observations, and general hardware support remain distinct.

## Live two-way portrait observation: BiliComics 0.1.16

The explicit `set_native_framebuffer_portrait_calibration(holding, scanout,
raw_rotation)` API supplies a same-run scanout/GETVAR anchor and opts into active
orientation observation. Compatibility setters alone do not open a sensor.
Calibration changes remain restricted to pre-I/O or suspended ownership;
same-value compatibility calls retain their no-op behavior.

The independent `portrait_events` observer keeps one validated, read-only,
non-grabbing evdev descriptor. Its wire layout uses native unsigned-long
timestamps for ARM32/Linux x86_64 ABI compatibility. Complete `SYN_REPORT`
notifications/current snapshots, duplicate coalescing, undefined-active-value
filtering, and `SYN_DROPPED` recovery prevent stale direction replay. Suspend,
quit, and retirement close the source; descriptor generations reject old
readiness. Opening is attempted once per active cycle, and optional failures
disable that source until resume. Idle orientation has no polling/stability or
debounce timer. The final user scope preserves the tested hardware sensitivity
and accepts only Up/Down automatic behavior; manual application rotation and
landscape preferences remain independent.

Control/lifecycle checks run before gyro, which runs before ordinary touch.
An active turn probes fresh retained-layout GETVAR and resolves only zero or
half-turn relative scanout changes. It preserves the touch grab, geometry,
framebuffer mapping, polarity, and reader/application rotation state while
cancelling the pointer, draining old input, and invalidating the touch generation.
The holding/scanout residual drives complete scene rendering and refresh areas.

The dedicated reorientation drain handles input queue loss using the existing
protocol-B snapshot logic; legacy input without reliable snapshots waits for
fresh observed coordinates. The full rewrite and GC16 submission precede the
second drain. Missing reports/snapshots keep input gated with an already
submitted frame; descriptor readiness retries the drain without another draw,
busy timer, or forced application exit. New lifecycle/direction/frame changes
reset that wait. Other input failures retain their genuine error handling.
Optional native-live traces use nonblocking, no-follow, close-on-exec opens and
regular-file checks, preserving the separate resume-trace semantics.

The frozen 0.1.16 R32 backend component passed 147 controlled tests and its ARM
archive passed nine static checks for 28 entries. The R31 application stages
passed separately; its initial native parser build failure remains preserved.
The scoped R33 offline Scribe2022 / 5.19.5 run received operator-confirmed active
two-way artwork rotation and native X-return/direction/touch checks. Exact
numeric page advancement is not inferred from the rotation result. The private
five-second test launcher, production two-second launcher, component/source
manifests, raw/export records, and closed device lease retain separate identities
in [live portrait evidence](../../docs/LIVE_ORIENTATION.md). General hardware
support and other projects' results are not established by that scope.

## Optional rendering performance observations

The project-owned `performance` module exposes a cloneable callback handle that
is disabled until configured. Disabled sampling reads no diagnostic clocks and
performs no per-frame allocation or callback locking. A Reader operation marker
identifies the image already published to the UI; changing it does not request
a redraw. Repeated actual draws retain that marker and have distinct render
sequence numbers. Callback retirement rejects old generations at the dispatch
check without waiting for a callback already selected for dispatch. Callbacks
execute outside framebuffer and lifecycle borrows.

Initial device setup and actual frame work record monotonic wall time and
same-thread CPU time separately. The stages cover initial I/O, allocation,
window setup, observer setup, initial fill, Slint rendering, grayscale conversion
and framebuffer writes, refresh submission, and existing lifecycle completion.
Clock failures remain absent values. The callback data contains only numeric
timing, display geometry, software modes, and operation/sequence identifiers.
The application owns the optional JSONL sink; this backend does not write files.

Existing refresh functions now return an observational submission result with
the exact ioctl attempt count, acceptance status, and final numeric errno.
The ABI probe order, cached variant, waveforms, markers, and original error log
remain unchanged. Empty regions and a cached unsupported ABI issue no ioctl.
No additional refresh or wait is introduced. An ioctl return or acceptance is
explicitly distinct from physical panel completion, which is not measured by
these samples.
