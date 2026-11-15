# Aidoku native JPEG transaction provenance

Recorded on 2026-11-15. This directory retains the source identity and terms of
the isolated JPEG path adapted for BiliComics. It does not choose an overall
license for BiliComics or establish BiliComics physical-device acceptance.

## Source identity and terms

- Source project: the operator's sibling `AidokuForKindle` working tree.
- Source base commit: `a53b9716d64fe5521fac38431b24732c75cb8ff7`.
- The copied implementation was an **uncommitted candidate**, not the contents
  of that commit alone or an assertion about a published remote revision.
- Handoff-declared complete candidate fingerprint:
  `e9f8d08b6599ef657ecb572d8c343b16050fa87d1fa35579c5e518c9d14108e8`.
  This value identifies the source project's declared candidate; it is not the
  fingerprint of this smaller retained snapshot or of BiliComics.
- Applicable source license: `GPL-3.0-or-later`, as declared by the retained
  upstream `Cargo.toml`. The complete, unchanged GPLv3 text is retained in
  `LICENSE-GPL-3.0-or-later`. The adapted source files retain file-level SPDX
  and provenance comments. No MIT or Apache license grant is inferred for
  these application-owned modules from the separately licensed Kindle backend.

`upstream/` holds unchanged snapshots of the nine selected source inputs.
`UPSTREAM_FILES.json` records their original paths, byte counts and SHA-256
values. Recording source-file fingerprints reads source bytes; it does not
execute or validate the implementation. The nested `.gitattributes` preserves
the original snapshot and license bytes, including their line endings.

## Adapted units and changes

| Source input | BiliComics destination and adaptation |
| --- | --- |
| `src/native_media/mod.rs` | `src/native_media/mod.rs`: BiliComics diagnostics; `Off` and `DecodeAndResize` policies only; complete entropy-only Gray metadata in `information`; target-aware production admission restricted to two complete measured geometry pairs; bounded IPC response validation; transient in-process contention keeps the healthy owner alive. |
| `src/native_media/jpeg.rs` | `src/native_media/jpeg.rs`: conservative baseline marker, table, sampling, color, size and entropy admission; tightened to BiliComics' marker budget, marker whitelist, exact terminal EOI and one-valued entropy alignment; pinned strict headers-only metadata validation and explicit Luma/YCbCr semantics. |
| `src/native_media/worker.rs` | `src/native_media/worker.rs`: retained real Scribe 2022 model, lock, driver, diagnostics, fixed C ABI and cleanup checks; narrowed production target admission; removed the source candidate's exact kernel and firmware gate under the operator's 2026-11-15 hardware-model policy. |
| `src/native_media/codec.c` | `src/native_media/codec.c`: compile the shared transaction with only the JPEG worker entry points. |
| `src/bin/jpeg_worker.rs` | `src/bin/kindle-jpeg-worker.rs`: BiliComics library entry point. |
| `scripts/native-codec-probe.c` | Same relative path: production is restricted to baseline single-component 1x1 JPEG and GREY combined downscaling for the two exact measured geometry pairs; other geometry and every color encoding are rejected before video open. PNG and AYUV research branches are excluded from the worker build; the initial default NV12 layout is observation-only before explicit GREY negotiation. |
| `scripts/native-codec-bench.h` | Same relative path: retain the 112-byte C/Rust report ABI and the original symbol names. |
| `Cargo.toml` | Retained as source-license and worker-build provenance; BiliComics uses its own manifest, features and dependency graph. |
| `docs/review/media-qualification-20261115/shutdown-contract-mock.c` | `scripts/verification/native_media_codec_mock.c` and `test_native_media_codec.py`: rewritten as a process-local host simulation of the current production C transaction, including strict downscale admission, negotiated layouts, injected errors/timeouts, STREAMOFF retries and allocation lifetime. |

The build integration compiles the C transaction only for Linux Kindle builds.
The development ZIP includes the application and its executable
`kindle-jpeg-worker` sibling, together with this notice, the source-input
manifest and complete GPL text. Static package verification binds both selected
ARM binaries, their executable modes and the retained notices. Build-source
identity includes both C inputs even though they live under `scripts/`.

## Safety and verification boundary

The worker alone owns the video descriptor, USERPTR pages and cooperating
`/tmp/kindle-jpeg-hardware.lock`. It runs in its own session. A parent deadline
does not kill it, release its lock, close its context or free its memory. It
keeps every allocation until both STREAMOFF calls complete and the context
closes. The shared lock file must not be deleted to bypass a draining owner.

The production C interface must not be called directly from Reader threads.
The Rust worker independently validates the complete admitted encoding before
opening the engine. A new kernel fault, transport error or failed hardware
transaction disables acceleration for the current application run. Rejected
formats and operations retain software processing.

The source encoding must end at EOI, without even a zero-valued trailing byte.
The advertised DMA zero tail is generated in the separate initialized staging
allocation and never changes that source-container rule. Grayscale Adobe APP14
tags use software conservatively. Metadata is also checked with BiliComics'
already pinned strict zune-jpeg decoder in headers-only mode; this does not run
IDCT or allocate a decoded RGB image. Differential fixtures compare malformed
native inputs against the existing full software integrity validator.

Host simulations never open a real video device or use a firmware substitute.
The codec fixture replaces every external device operation with an in-process
mock and has no real-device fallback. IPC tests use synthetic Python helpers
and private temporary locks to test response rejection, detached-owner
survival, cross-process exclusion and release. Package tampering fixtures
exercise missing, non-executable and replaced helper rejection. These are
source-bound software checks, not optical or physical-driver acceptance.

The current policy exposes the off-by-default option by positively identified
Scribe 2022 hardware model, together with matching live JDEC QUERYCAP and
readable per-worker kernel diagnostics. It does not read the kernel release or
system software version to decide whether to offer the option. This adapts the
source candidate's exact-version gate to the operator's 2026-11-15 policy;
original snapshots and historical qualification reports retain their original
device and software identities. Driver-layout checks and fault handling remain
mandatory at runtime. Y4 capability, display profiles and screen geometry do
not establish JPEG hardware identity. Only baseline 8-bit one-component 1x1
grayscale may reach the combined decoder/IMGRZ path. All color JPEG sampling,
including 4:2:2 and 4:4:4, now retains software processing. PNG,
unchanged dimensions, upscaling, region crops and unqualified devices remain
software paths. The original source's PNG transparency incident must not be
retried as part of this import.

The BiliComics R4 physical run on 2026-11-16 completed the grayscale
1920x2560 to 1860x2480 transaction, then recorded ten new IOMMU read-fault
messages during the corresponding 4:2:2 color transaction. The worker's kernel
diagnostic check rejected that result and disabled acceleration for the run.
This invalidates broad color production admission; apparently correct pixels
or successful ioctls do not establish DMA safety. Public planning, the isolated
worker and the production C FFI independently reject color before video open.
The broader JPEG parser and unchanged upstream snapshots remain research
provenance and do not authorize a color hardware retry. The precise fault
root cause has not been established by the retained evidence.

The subsequent R6 physical run completed five grayscale hardware operations:
1920x2560 to 1860x2480 for four named fitting modes, and 1920x1280 to
1860x1240 once. It then recorded another IOMMU read-fault burst for grayscale
1280x2560 to 1240x2480. Grayscale coding alone therefore does not qualify a
transaction. The current production whitelist is exactly:

| Source width x height | Target width x height |
| --- | --- |
| 1920 x 2560 | 1860 x 2480 |
| 1920 x 1280 | 1860 x 1240 |

Every other grayscale source/target pair uses software, including the former
930-pixel reference targets. `information` may still return complete, strictly
validated grayscale metadata; metadata success does not authorize submission.
Reader planning, the isolated worker and the production C FFI apply the exact
geometry check. These are measured points, not a width threshold, scaling-ratio
rule or proof of the unique driver root cause. Neither faulting input is
authorized for another hardware trial by this import.

The source project reviewed the matching [Amazon kernel archive](https://s3.amazonaws.com/kindledownloads/Kindle_src_5.19.5_4794320058.tar.gz).
No kernel implementation, firmware binary, private device identity, account,
comic, signed URL or research archive is copied here. Aidoku timing and device
results remain reference-project evidence; changed admission and IPC costs
require BiliComics verification on `test-env` and, separately, physical hardware.
