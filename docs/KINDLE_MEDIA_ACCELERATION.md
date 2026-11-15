# Qualified JPEG hardware acceleration

BiliComics provides an optional, off-by-default JPEG hardware acceleration
setting in default reading settings and per-comic reading settings. A disabled
control explains why the current platform, device, helper, driver or
diagnostic access is unavailable. A stored preference never grants capability.
The command handler applies the same capability check as the control.

This setting controls image decoding and resizing. Hardware Y4 dithering and
the display waveform remain independent. It does not increase the panel's
native number of gray levels.

## Qualification and fallback

The admission policy follows the
[Aidoku research handoff](KINDLE_MEDIA_ACCELERATION_AIDOKU_20261115.md).
Its physical evidence belongs to Aidoku's candidate and the tested Scribe;
it is not physical acceptance or a performance measurement of BiliComics.

The worker checks real ARM hardware identity from the first 16 bytes of
`/proc/usid` and one of the supported Scribe 2022 model codes: `27J`, `2BL`,
`263`, `227`, `2BM`, `23L`, `23M` or `270`. Firmware, software-build and kernel
versions are evidence provenance, not runtime allowlist keys. This follows the
operator's 2026-11-15 clarification to use hardware-model eligibility.

It also checks the actual JDEC driver and access to independent kernel
diagnostics. A UI design profile and Y4 capability do not establish media
support. Other Scribe generations, Paperwhite models and unknown identities
remain outside the supported model list; this does not assert that their
silicon lacks a decoder. A different system version alone cannot disable the
option on a supported model. Driver negotiation and runtime failures still
retain the software fallback.

Only a combined JPEG decode and IMGRZ downscale is eligible. The input must be
complete baseline 8-bit, single-component grayscale JPEG with qualified tables,
scan structure, color semantics and entropy data. Color JPEG remains software:
the BiliComics physical run recorded IOMMU read faults for its 4:2:2 workload,
so the earlier reference project's color admission is not used in production.
Parent metadata/decode, worker and C entry points reject color before hardware
submission. Limits are
2 MiB encoded, 16–2048 source width, 16–4096 source height, at most 6 Mi pixels,
and at least 256 x 256 equivalent source pixels. Each target axis must be at
least 16 pixels and no larger than the corresponding source axis. At least one
axis must shrink. Production additionally requires an exact measured shape:

| Source pixels | Target pixels |
| --- | --- |
| 1920 x 2560 | 1860 x 2480 |
| 1920 x 1280 | 1860 x 1240 |

All other source/target combinations retain software processing, including
1280 x 2560 to 1240 x 2480: a subsequent Gray-only physical trial recorded the
same class of IOMMU read fault for that combination. These are explicit measured
tuples, not an inferred scale ratio or width threshold. The faulting color and
Gray combinations are not retried. Device-model eligibility and default-off
selection remain independent of this per-image submission policy.

The ordinary software pipeline handles unsupported formats and geometry,
including progressive JPEG, all color JPEG, all PNG,
unchanged dimensions, upscaling, panel/strip modes, zoom, rotation and crops.
Reference-project full-page 4:2:0 trials also produced IOMMU faults despite
apparently correct pixels. PNG is excluded both from production submission and
from this setting. In particular, non-palette PNG tRNS must never reach the
hardware: the qualified kernel has an unsafe user-pointer access in that path.
The standalone MDP scaler is not used.

## Reader integration

The fast path runs before CPU image decoding, within the existing immutable
hashed-input and receipt-verification callback. The existing strict container
and pinned decoder header checks preserve the software validation policy,
including metadata, segment budgets and color-component semantics. An
entropy-only JPEG check creates a sealed proof bound to the encoded digest,
length, validation version and **original** dimensions. No CPU IDCT is needed
for this proof. The resolver still checks the receipt and source object before
presenting a frame. A hardware failure requests software
fallback; it does not mark a valid download corrupt.

The Reader's existing `fit_geometry` remains the geometry authority. The four
accepted fit definitions remain unchanged: `original` contains large images and keeps
small images at their native size, `page` fills both viewport axes, and
`height`/`width` retain their named-axis scale with centered overflow. Hardware
is considered only for a complete source, without a crop or sampling window.
A portrait screen itself is eligible; a strip-mode viewport is not.

Processed Gray8 images carry the original proof and the complete preparation
request, geometry, effective media policy and processing version (currently 3). A separate
16 MiB / 16-entry LRU retains them. The canonical source cache continues to
contain only original-size, unrotated gray pixels. A policy change invalidates prepared
frames and preserves the current reading anchor, rotation and zoom.

Hardware and software results share the existing canvas, quantization/Y4 and
complete-frame presentation stages. Software color decoding preserves the
pinned zune-jpeg scalar RGB clipping and BiliComics' 54/183/19 grayscale weights.
It does not silently substitute JPEG's native Y channel. The retained NV16
research code is not a production color path. Hardware resizing uses a different filter,
so general pixel equivalence with software resizing is not promised.

## Worker ownership

`kindle-jpeg-worker` is installed next to the application. Each request uses a
separate process session and the shared `/tmp/kindle-jpeg-hardware.lock` flock.
Both parent and worker validate the input and target. The parent deadline
allows software fallback without killing a worker that may still own DMA
memory. Reader-generation cancellation never kills the worker.

The driver transaction queues OUTPUT first with initialized, page-aligned,
prefaulted USERPTR memory and an advertised zero tail. After SOURCE_CHANGE it
reads and validates the real CAPTURE layout before allocating planes. Gray
uses GREY. The production entry refuses multi-component sources before opening
the video device. Returned formats, dimensions, pitches, capacities, payloads and
error flags are checked.

The worker keeps context, storage and lock until both STREAMOFF calls and
context close complete. It retries interrupted cleanup instead of freeing
active buffers. An unresolved transaction blocks another cooperating worker,
including after an application restart. Independent kernel diagnostics check
IOMMU `fault type=`, DMA faults, timeout and kernel error records. A hardware or
transport failure disables acceleration for the current application run.

## Source and verification scope

Copied worker, JPEG admission and C transaction code retain the Aidoku
GPL-3.0-or-later source provenance and license in
[`vendor/aidoku-native-media`](../vendor/aidoku-native-media/).
See [third-party notices](../THIRD_PARTY_NOTICES.md). This scoped provenance does
not select a project-wide license or declare a public release.

Builds, unit tests, simulated-driver/IPC checks, package verification and actual
desktop XTest captures run on `test-env`. Fixture results are separate from
physical-device results. A subsequent Scribe physical batch published a Gray
hardware frame, then detected IOMMU read faults on the color 4:2:2 candidate.
The run disabled hardware, published its software replacement and restored
native control without rebooting. That color input was not retried. The current
production policy therefore permits Gray only. The Aidoku speedups do not
describe BiliComics page-turn latency.

The [implementation verification report](KINDLE_MEDIA_ACCELERATION_VERIFICATION_20261115.md)
records the tested source, application and worker identities, executed checks
and reviewed UI baseline.

The subsequent [physical verification report](KINDLE_MEDIA_ACCELERATION_PHYSICAL_20261116.md)
records both rejected hardware candidates and the final exact-tuple policy.
The final scoped Scribe runs completed 18 reading phases, five hardware
operations, and a separate cold start using the application's actual saved
SQLite bytes. Unsupported inputs retained software rendering; both final runs
had no new critical kernel diagnostics and restored native control. These
results remain distinct from optical panel measurements and from support for
other devices, firmware builds, JPEG encodings or dimensions.
