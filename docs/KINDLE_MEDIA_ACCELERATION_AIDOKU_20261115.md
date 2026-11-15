# Aidoku Kindle media research handoff

Date: 2026-11-15. The operator requested that BiliComics begin corresponding
implementation after the Aidoku research and integration were completed.
This note transfers conclusions and sanitized evidence, not device credentials
or a new publishing authorization.

## Recommended implementation

Provide an off-by-default **JPEG hardware acceleration** option. Enable it
for supported hardware models, independently of firmware or kernel version,
and retain actual driver-capability and runtime-failure checks. Show a disabled
control with an explanatory reason on unsupported models or unavailable drivers.
This 2026-11-15 user clarification supersedes the original exact-firmware gate.
Use hardware only when a combined
JPEG decode and IMGRZ downscale is applicable. Border trimming, centered region
crops, vertical viewports, unchanged dimensions and upscaling should keep the
existing software pipeline. Forced hardware decoding followed by CPU resizing
was slower; do not ship it as the automatic fast path.

Preserve BiliComics' accepted fitting geometry, grayscale conversion, Y4 and
display ownership. Image decoding/scaling and display waveform/dithering are
independent policies. Include the hardware policy in processed-image and
retained-image identities so setting changes cannot reuse an incompatible
prepared frame.

## Admission matrix

The physical evidence is for Scribe 2022, firmware 5.19.5, software build
`058-juno_1905_barolo_bellatrix3-479432`, ARMv7 Linux `4.9.77-lab126`.
An application design profile is not sufficient capability evidence.
Firmware, software-build and kernel versions are measurement provenance, not
runtime allowlist keys. The current supported Scribe 2022 model codes are
`27J`, `2BL`, `263`, `227`, `2BM`, `23L`, `23M` and `270`; a different firmware
on a matching model must not by itself disable the option. Keep the option off
by default. Paperwhite 5/6, later Scribes and unknown model identities remain
outside this media path's model support list. Retain software and a disabled
option without claiming that their silicon has no decoder. Colorsoft was excluded
from scope. Existing physical results do not establish performance on every firmware.

| Input/path | Decision |
| --- | --- |
| Baseline 8-bit grayscale JPEG, one 1x1 component | Qualified combined decode/resize candidate |
| Baseline 8-bit YCbCr 4:2:2 or 4:4:4, even width | Qualified combined decode/resize candidate, with documented filter differences |
| Color JPEG 4:2:0 | Reject before hardware: two full-size trials produced IOMMU read faults despite correct visible pixels |
| JPEG 4:1:1, 4:4:0, 4:1:0 | Small GREY trials passed; not promoted to full-page/color production admission |
| Odd-width color JPEG | Reject before hardware: final-column Y/chroma errors |
| Progressive and other unqualified JPEG coding | Reject hardware submission; retain ordinary CPU behavior |
| PNG | Keep CPU: the measured hardware-plus-CPU-resize pipeline was 37-72% slower |
| Non-palette PNG tRNS color key | Never submit: confirmed kernel access fault and reboot |
| Standalone full-page MDP scaling | Not qualified: returned pixels were incorrect |

Aidoku's conservative limits are 2 MiB encoded, 16-2048 source width,
16-4096 height, at most 6 Mi pixels, and no small-input acceleration below
256 x 256 equivalent pixels. Targets stay within source dimensions, differ
from source size, have at least 16 pixels per axis, and have even width for
color. Fallback preserves the project's broader software limits.

## Required caller contract

Use the exact [Amazon kernel source](https://s3.amazonaws.com/kindledownloads/Kindle_src_5.19.5_4794320058.tar.gz).
The safe sequence is:

1. Validate JPEG marker bounds, SOF0 precision/sampling/color semantics,
   quantization/Huffman tables, scan layout and complete entropy blocks.
   Aidoku uses an entropy-only validator before hardware, avoiding CPU IDCT.
2. Identify JDEC through QUERYCAP. Its JPEG/PNG queues support USERPTR, not
   MMAP queue allocation. Use initialized, page-aligned, prefaulted storage.
3. Prepare only OUTPUT first. Include an advertised zero tail in bytesused:
   `round_up(encoded_bytes + 64, 64)`.
4. Queue/start OUTPUT, receive SOURCE_CHANGE, and inspect the parsed CAPTURE
   layout before selecting the desired format or allocating CAPTURE.
5. Use GREY for monochrome and NV16 for combined color processing. This MTK
   driver uses NV16 with **two separately backed planes**. Both byte pitches
   cover the full output width, and both planes cover the full output height.
   Read back and verify format, dimensions, plane count, pitches and capacities.
6. Dequeue both buffers and inspect flags/payloads. Preserve every allocation
   until both streams have actually stopped and the context has closed.
7. Check kernel diagnostics, including `fault type=` IOMMU messages, not only
   ioctl results and decoder timeout strings.

The original JPEG reboot was caused by allocating one-plane GREY before the
first header changed CAPTURE to two-plane NV12. USERPTR itself was not the
problem. Explicit full-width UV pitch fixed the small NV12 luminance corruption,
but full-size color 4:2:0 subsequently failed the broader IOMMU audit.

The PNG tRNS fault is separate. `mtk_png_device_run` passes the raw userspace
pointer to `mtk_png_set_trns`, which directly dereferences it without protected
access and without the chunk offset. The retained kernel log shows a page-domain
fault at that pointer during the byte load. Palette tRNS and intrinsic alpha
take different branches and had successful pixel tests; that does not qualify
non-palette color-key transparency. No faulting RGB color-key retry was made.

## Preserve color-to-gray behavior

Use the pinned zune-jpeg 0.5.15 scalar full-range coefficients and clip RGB
before applying BiliComics' existing 54/183/19 weights:

```text
y0 = Y * 16384 + 8191
cb = Cb - 128; cr = Cr - 128
R = clamp((y0 + 22970 * cr) >> 14, 0, 255)
G = clamp((y0 - 11700 * cr - 5638 * cb) >> 14, 0, 255)
B = clamp((y0 + 29032 * cb) >> 14, 0, 255)
gray = (54 * R + 183 * G + 19 * B + 128) >> 8
```

NV16 chroma uses centered horizontal 3:1 expansion with rounding 2 before
shifting by 2. Full-size 4:2:2 identity conversion matched the CPU exactly.
Do not silently replace the current color policy with JPEG's native Y plane.

Conventional AYUV was wrong. This firmware has a measurable split-luma quirk:
Y at `row*stride+2*x`, U at `row*stride+4*x+1`, and V at `row*stride+4*x+3`.
Even-width 4:4:4 can be reconstructed exactly, but odd width fails. The final
automatic Aidoku Reader does not select that unscaled, slower route.

## Worker lifetime

The final Aidoku implementation uses a separate one-shot `kindle-jpeg-worker`,
an independent process session, and a cooperating `/tmp/kindle-jpeg-hardware.lock`.
The parent deadline never kills the DMA owner. A timed-out worker retains its
context and memory until STREAMOFF/close actually complete; a pending worker
blocks another cooperating process, including across application restarts.
New kernel faults disable acceleration for the current application run.

The driver's empty job_abort is not bounded cancellation. STREAMOFF waits for
M2M completion. JPEG release frees temporary chroma before context cancellation,
so closing an active GREY context is unsafe. Do not turn process cancellation,
request-generation cancellation or a timeout into early buffer release.

## Measured results and verification

The final production Reader API measured the following seven-sample medians
after one warmup, with identical resident input and native UI parked:

| Workload | Software | Combined hardware path | Speedup |
| --- | ---: | ---: | ---: |
| Gray 1920 x 2560 to 930 x 1240 | 279.65 ms | 85.91 ms | 3.25x |
| Gray 1920 x 2560 to 1860 x 2480 | 466.31 ms | 150.94 ms | 3.09x |
| Color 4:4:4 1860 x 2480 to 930 x 1240 | 353.98 ms | 139.27 ms | 2.54x |

These include entropy validation, process startup, IPC, allocations, driver
operations, copying and postprocessing. They exclude file I/O, hashing and
display-waveform time. The earlier 27.68/53.37 ms probe numbers are not complete
application costs. Filters differ; a small high-frequency color fixture failed
the declared quality comparison and remains documented. Do not generalize the
large-scene quality/performance figures to all content or full page turns.

The exact final application passed real Scribe switch activation, persisted
restart and two-page imported JPEG Reader use. The final fallback cases made
zero decoder-worker attempts. Native services and lighting were restored, and
all final batches ended without new critical kernel errors. Worker-side tests
included 780 Kindle-library tests (five existing ignored), 767 default-library
tests, 20 binary tests, ten desktop UI cases and 162 package checks.

Source provenance: AidokuForKindle base `a53b9716d64fe5521fac38431b24732c75cb8ff7`
plus an uncommitted candidate, source fingerprint
`e9f8d08b6599ef657ecb572d8c343b16050fa87d1fa35579c5e518c9d14108e8`.
Final worker SHA-256:
`6f45e6b977cc94238cf22a1885dbb7298c7834abfc072aacbc19b748b8b277e2`.
See [sanitized final Reader evidence](research/aidoku-media-20261115/reader-final-results.json)
and [final native UI evidence](research/aidoku-media-20261115/native-media-ui-final-verification.json).

Implement within BiliComics' own architecture and verification rules. Reuse
research facts freely; any copied code must retain its applicable licensing
and provenance. This handoff does not choose a project-wide license or extend
prior commit/push instructions to this new work.
