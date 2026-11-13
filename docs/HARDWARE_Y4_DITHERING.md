# Hardware Y4 dithering

Current scoped result: R68 on Kindle Scribe 2022 / firmware 5.19.5 verifies an
accepted GC16 update carrying hardware Y4, no software fallback, and a live-bound
framebuffer ramp containing all 256 input luminance values. Native services and
owned processes were restored before exact lease release. This is driver/input
acceptance, not a measurement of 256 physical panel reflectance levels. The
complete scoped evidence is linked in the final physical-result section below.

## Scope and input format

Hardware Y4 is a display-processing option for reviewed monochrome MediaTek
Kindles. It converts an eight-bit luminance input into a four-bit output using
the display processor's dithering implementation. This is not a claim that the
panel exposes 256 independently addressable, stable grayscale levels.

The reader keeps its existing software Bayer default. Its display setting can
select software dithering, hardware dithering, or no dithering. Older saved
`dither` booleans retain their meaning. The optional `dither_method` preference
defaults to `software`; unknown methods use software processing.

Software mode retains the existing four-by-four Bayer conversion to the values
`0, 17, ..., 255`. Disabling dithering retains sixteen-level quantization without
the Bayer offset. Hardware mode preserves the full Gray8 image until native
display processing. The reader must not software-dither or quantize that image
before requesting hardware Y4.

## Native interface and device policy

The MediaTek update structure uses the Y4 flag `0x4000`. Its `dither_mode` and
`quant_bit` fields remain zero. The older i.MX EPDC-v2 dithering parameters are
not interchangeable with this interface: `0x8000` is the MediaTek REGAL flag.

The initial supported policy is deliberately limited to recognized Kindle
Scribe 2022 and 2024 hardware identities with an eight-bit grayscale
framebuffer. A UI design profile, display resolution, or successful ioctl is
not a sufficient hardware identity. Unknown hardware, color framebuffers, and
Paperwhite 5 use the software path. The support decision is read-only and does
not change the system's native display mode.

While a hardware-input scene is presented, grayscale-safe updates preserve its
pixels. Fast binary requests must not send a Gray8 comic through a binary
waveform. Dithered refresh regions follow the reviewed outward alignment and
screen-boundary rules. Existing full-cleaning, marker, completion, native
polarity, and sleep-ownership rules still apply.

## Presentation and fallback

The processing request belongs to the displayed frame. A pending setting can
start preparing a replacement image while the old frame remains visible; it
must not change the old frame's processing. Importing the replacement frame
switches the processing mode and requests a complete redraw and GC16 refresh.
Prepared-image identities include the effective processing choice. Thumbnail
processing follows the displayed reader scene, including during pending work.

Leaving the reader disables its hardware-input request. A temporary sleep
cover suspends that request and restores the retained reader frame's request
when the cover leaves. Reflow after wake continues to bind processing to the
actually displayed image, including a retained frame with older preferences.

A definite unsupported-parameter rejection disables hardware Y4 for the
session. The fallback must first settle any previously accepted update, then
rewrite the raw input using software dithering and submit a fresh update with
its own marker. Clearing the Y4 flag alone would leave unprocessed Gray8 data
and is insufficient. An uncertain I/O failure is not permission to rewrite
possibly in-flight memory or blindly resubmit the same update.
An uncertain hardware-path delivery latches the framebuffer against further
writes and submissions and stops the display loop through the existing native
restoration path. A later mode revision cannot silently retry that operation.

Diagnostics distinguish requested processing, available processing, an accepted
submission carrying the Y4 flag, and software fallback. An accepted ioctl is
not proof that the panel completed the update or that a firmware implementation
honored the requested image treatment.

## Reviewed upstream references

- [FBInk MediaTek ABI](https://github.com/NiLuJe/FBInk/blob/ded92fa510c957e0157c92ae83bd5c51af710c05/eink/mtk-kindle.h#L47)
  and [MTK refresh implementation](https://github.com/NiLuJe/FBInk/blob/ded92fa510c957e0157c92ae83bd5c51af710c05/fbink.c#L2565).
  The header identifies its Kindle kernel source and Amazon GPL download entry.
- [KOReader MTK submission](https://github.com/koreader/koreader-base/blob/f64c3eb11049e9a75189f186d7345d97c07d87b9/ffi/framebuffer_mxcfb.lua#L635)
  and [region alignment](https://github.com/koreader/koreader-base/blob/f64c3eb11049e9a75189f186d7345d97c07d87b9/ffi/framebuffer_mxcfb.lua#L321).
- [KOReader Scribe capability](https://github.com/koreader/koreader/blob/7c38cf104cb9ea89df243e47ec820ef4aa97cab8/frontend/device/kindle/device.lua#L1174)
  and [Paperwhite 5 policy](https://github.com/koreader/koreader/blob/7c38cf104cb9ea89df243e47ec820ef4aa97cab8/frontend/device/kindle/device.lua#L1105).
- [Scribe hardware-dithering report](https://github.com/koreader/koreader/issues/11286)
  provides an upstream physical-device precedent. It is not verification of
  this application's binary or the locally used firmware.

## Verification

The 2026-11-13 source snapshot was verified on `test-env`, with synthetic input
and isolated build state. That run did not perform local verification. Compilation used
two Cargo jobs, and a private, bounded cache overlay preserved historical build
caches. The source and pipeline inputs remained unchanged during execution.

- 713 application-library, eight Main, and four integration tests passed. Nine
  opt-in tests remained explicitly ignored.
- All 185 standalone native-backend tests and 21 strict physical-fixture tests
  passed. The fixture retains its original default contract and adds a separately
  declared, reconstructible hardware/Gray8-ramp variant.
- The real offline desktop application passed software/hardware/off selection,
  software fallback, unchanged anchors, actual page turns, and cold-restart
  persistence across six processes on PW5 and Scribe design geometries. Quiet
  fallback pixels matched software pixels. Selected captures were visually
  inspected for complete controls and readable fallback copy.
- Existing Reader, guest, startup, and real-process lifecycle regressions passed.
- The static ARM development package passed all ten archive/ELF checks. These
  results do not establish native-driver or physical-panel acceptance.

Exact evidence is retained in the [software summary](../artifacts/verification/y4-20261113-22254432/software/verification-summary.json),
[desktop UI report](../artifacts/verification/y4-20261113-22254432/hardware-y4/results.json),
and [runtime/package report](../artifacts/verification/y4-20261113-22254432/software/runtime/summary.json).
The development package retains application version `0.1.23`; use these exact
identities to distinguish this follow-up from the earlier cover-sleep build:

| Artifact | SHA-256 |
| --- | --- |
| Source-input manifest | `551f29eb36d52d7bad37ebc34c45af9c268bfffafc18b4f263deacd655335abc` |
| Pipeline-input manifest | `d6d983ff9b68fd04c419e0e75192a6a3b95fd173fa9b0b4699da1b4221740cad` |
| ARM executable | `0d84396f2189ac0e6abfbc8e1df01c484d5a5fd145ca24ca0c2b4a1148102b0f` |
| Development ZIP | `6e66051bc682a64517812e1c3b9ba4ba019c88f489a5bcef2e244fc8e92dc3f9` |

### Physical preflight: blocked before application startup

Fresh WeRead coordination and run `y4-20261113-r57-c91905dc` preceded a separate
physical batch on the previously identified Scribe 2022 / firmware 5.19.5. Its
preflight observed an eight-bit framebuffer, a stopped statusbar service, and an
unavailable normal-native EPDC observation. Deployment then failed with status
15 in the guarded native-IPC gate. The isolated application did not start, and
this batch did not take display or input ownership.

The controller closed its transports and released the lease with exact owner
and token checks at `2026-11-12T20:51:56Z` (`2026-11-13 04:51:56 +08:00`), then
notified the coordinator before host-side analysis. The
[public physical result](../artifacts/verification/y4-20261113-22254432/physical-r57/public-preflight-result.json)
and [release receipt](../artifacts/verification/y4-20261113-22254432/physical-r57/unstarted-release.json)
retain that boundary. Existing production installations were not replaced.
The isolated staging path is recorded in the result; its removal is not claimed.

No accepted Y4 submission, panel appearance, or physical grayscale count is
claimed for this application's binary. Any later physical attempt requires the
device to return to a normal native baseline, a fresh handoff, and independent
run, token, and evidence identities.

### Authorized local WSL verification

The user subsequently authorized local verification and required compilation
to remain on `test-env`. The unchanged sealed desktop executable
`38e5cbe4c3bac8318d358bad29702011f3df008faeee1259fa9ea0bd56bcf0a7`
was downloaded and its digest checked before running on the workstation's
Debian 13 WSL/WSLg runtime. No application compilation took place locally.

Both PW5 and Scribe cases passed across six real local application processes:
software/hardware/off selection, software fallback, unchanged reading anchors,
actual page turns, and cold-restart persistence. Hardware-fallback pixels match
software pixels. The [local summary](../artifacts/verification/y4-local-wsl-20261113-3e1c5754/local-verification-summary.json)
and [detailed report](../artifacts/verification/y4-local-wsl-20261113-3e1c5754/workspace/evidence/hardware-y4-r5/results.json)
record `execution_environment=local-wsl` and `native_windows=false`.

Initial local attempts exposed missing image/X11 runtime dependencies and WSL's
abstract-only private X server connection. Their failure reports remain in the
same evidence workspace. The passing run uses existing `test-env` runtime
binaries with their license notices in an isolated directory and a private
mount namespace for Xvfb's filesystem socket. It does not globally install
dependencies or change WSLg's normal display socket.

The verifier now supports an explicit `--local` option together with
`BILICOMICS_LOCAL_VERIFICATION=1`; local runs cannot masquerade as remote runs.
Current-task user authorization is still required by the repository's testing
policy. Optional X11/XTest library paths support isolated runtime bundles while
the default remote behavior remains unchanged.

A separate WSLg window was also inspected with Computer Use. It shows the
hardware choice and the accurate software-fallback note, with page two retained.
This verifies the local desktop behavior; it does not verify a native Windows
PE executable, the Kindle driver, or physical panel grayscale levels.

### Second physical attempt: network unavailable

On the user's explicit physical-test request, fresh run
`y4-20261113-r58-0e55587a` first compared current source inputs with the sealed
ARM build. Both manifests remained
`551f29eb36d52d7bad37ebc34c45af9c268bfffafc18b4f263deacd655335abc`.
The host tools were syntax-checked, the known normal-reader exit path was
independently reviewed, and the negative native-ready gate passed on `test-env`.

After fresh coordination and acquisition, the first guarded SSH failed during
TCP connection with a timeout. The recovery observation also failed before
connection with `No route to host`. No device command was dispatched, no
application was deployed or started, and this batch did not take display or
input ownership. The controller's original `needs_recovery` result is preserved.
The coordinator subsequently verified the two explicit pre-dispatch failures,
closed the owned transports, and released with exact owner/token checks at
`2026-11-12T22:46:37Z` (`2026-11-13 06:46:37 +08:00`), then notified the peer.
Native baseline confirmation remains false. See the
[connection finalization](../artifacts/verification/y4-physical-20261113-r58-0e55587a/unconnected-finalization.json)
and [release receipt](../artifacts/verification/y4-physical-20261113-r58-0e55587a/unconnected-release.json).

Independent run `y4-20261113-r59-10a6c9ac` was initially prepared without a lease or device
access. Its ten host-only connection-failure tests passed. A narrowly proven first
SSH connection failure now closes transports and releases on the host without
issuing a second device request; possible command delivery still requires the
real native-restoration gate. This preparation is not a new physical pass or
authorization to retry while the device is unreachable. The device must be
awake and connected, and a new handoff must precede any R59 access.

### Physical diagnosis and bounded identity-read correction

R59's inline native-return command was closed by the device. Bounded short
recovery queries confirmed running native services, active power, `Y8INV`, and
absent reader locks before transport closure and exact release. R60 switched the
reviewed native-return script to stdin. It deployed and started the isolated
application, then stopped and recovered before release when its checkpoint did
not complete. Its retained trace proves the hardware preference fell back to
software: hardware availability was false from the first native frame. This is
not an accepted Y4 result.

A separately handed-off, read-only R61 batch diagnosed the identity predicate
without exporting a complete serial number. This device's `/proc/usid` contains
17 bytes: a valid sixteen-byte identity followed by one NUL. The public device
code is `23L`, already in the reviewed Scribe whitelist. The prior whole-file
read plus exact-length predicate rejected that valid proc-file representation.
Read-only diagnostics and exact release are recorded in the
[R61 result](../artifacts/verification/y4-physical-20261113-r61-8813d2f3/readonly-result.json).

The production detector now follows the upstream fixed-width field: open the
file and read exactly its first sixteen bytes, then apply the same strict
identity, device-code, and framebuffer checks. It does not read or interpret
trailing proc padding. Short reads, embedded NUL, unknown device codes, and I/O
errors remain rejected. Synthetic tests cover padding, fragmented bounded reads,
and failures; no complete real serial was copied into code, logs, or fixtures.

The corrected snapshot passes 15 application dithering checks, all 189 backend
tests, 14 checkpoint-host tests, and ARM archive/ELF verification on `test-env`.
See the [correction build summary](../artifacts/verification/usid-fix-20261113-2ca0bade/evidence/summary.json).
Its source-input manifest is
`f86a5938d43117f019926d1005f81b864fe44eb964465084a902163baa035567`;
its ARM executable is
`4dabdd791462ceb0bf2d18fab96ccb5a42d7d3f41c62baaa0b2e7abe9cd2d5f5`.
The application version remains `0.1.23`; the earlier source and binary identities
above remain historical evidence.

R62's first SSH connection timed out before command dispatch. The strict
connection-failure finalizer closed transports and released automatically within
approximately ten seconds; no corrected application was deployed or started.
Its [batch result](../artifacts/verification/y4-physical-20261113-r62-15c60366/batch-result.json)
does not claim native baseline or panel confirmation. At that stage, R63 was prepared with the
corrected artifact and no lease/token, pending a fresh device-online confirmation.
No accepted Y4 or panel-quality pass had been claimed for the corrected artifact
at that point; the completed result below supersedes that pending status.

### Completed physical driver and Gray8-input verification

The subsequent direct user instruction granted sole device use, removed peer
coordination for this task, and authorized a longer continuous test window.
Fresh owner/run/token lease identities were retained for recovery and audit;
no peer handoff was invented for that exclusive authorization.

R65's retained first proof error identified a separate harness limitation: it
accepted only `U` startup calibration, while the production Rust contract also
accepts authenticated `D` and `FIXED` records. The harness now retains those
semantics, keeps the exact generation/nonce/live-process/argv/credential/socket
guard checks, and requires one current, evidenced framebuffer mapping. A Down
holding/Down scanout pair has zero software-rendering residual; it must not
rotate the logical framebuffer a second time. No rotation or polarity search
was introduced. Six real-Shell identity contracts and 19 offline fixture cases
pass, along with 15 checkpoint polling cases. A controller wait parameter above
the existing CLI range was rejected in R67; its original failure and recovery
remain retained. R68 validates the actual five planned CLI command vectors on
the host before acquisition and uses an allowed 120-second checkpoint wait.

Run `y4-20261113-r68-4c9400f9` then completed the full physical fixture capture
using the corrected ARM executable
`4dabdd791462ceb0bf2d18fab96ccb5a42d7d3f41c62baaa0b2e7abe9cd2d5f5`.
The live process, executable digest, complete offline arguments, socket guard,
credentials, generation/nonce, calibration and source-page marker all passed.
The actual startup calibration was `D` at raw rotation `1`, with native software
rendering rotation `0`. The fixture stayed on its original page and full
`1860 x 2480` canvas.

The accepted native submission records `hardware_y4_available=true`,
`hardware_y4_requested=true`, `hardware_y4_applied=true`, GC16, and
`software_dither_fallback=false`. An independently captured ramp row at
`[126, 336, 1608, 1]`, normalized with the evidenced output polarity, contains
exactly 256 distinct byte luminances from 0 through 255. Of these, 240 are
outside the sixteen-value software palette. This confirms the image was not
quantized to sixteen levels before native Y4 processing.

The controller stopped immediately after capture, settled the existing native
recovery checks, closed its owned transports, and released with exact owner and
token checks at `2026-11-13T10:28:03Z` (`2026-11-13 18:28:03 +08:00`). Statusbar,
awesome baseline identity/state, launcher-lock absence, and the complete owned
process snapshot passed. The application session had a 300-second ceiling, but
did not idle until that ceiling. Existing production installations and account
data were not replaced.

- [Complete driver/input result](../artifacts/verification/y4-physical-20261113-r68-4c9400f9/hardware-y4-result.json)
- [Public live-context and marker proof](../artifacts/verification/y4-physical-20261113-r68-4c9400f9/public-checkpoint.json)
- [Exact release receipt](../artifacts/verification/y4-physical-20261113-r68-4c9400f9/public-release.json)
- [Framebuffer-derived projection](../artifacts/verification/y4-physical-20261113-r68-4c9400f9/hardware-y4.projected-panel.png)

The PNG and byte counts describe framebuffer input. They are not a panel
photograph, post-MDP readback, physical-button/touch observation, or a
256-reflectance-level panel measurement. This run establishes the scoped Y4
driver path and unquantized input on this device/firmware, not every Kindle.
