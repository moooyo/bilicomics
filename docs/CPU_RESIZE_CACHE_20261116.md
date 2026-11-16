# CPU resize cache: 2026-11-16

The Reader caches resized CPU Gray8 before dithering/RGBA, allowing Normal and
Fast to reuse geometry while retaining their separate final pixel processing.

## Implementation and verification

Eligibility requires Page/FitPage, rotation 0, zoom 1, full-source crop, no
sampling window, and the target viewport. Keys bind sealed source proof and
preparation identity. CPU/Native tags prevent aliasing within the original
16 MiB LRU; hardware JPEG admission remains unchanged. Source/next-frame/compute budgets
remain 32/32/384 MiB. Useful retention increases actual residency; unchanged
budgets do not promise unchanged RSS. Reference rendering bypasses this cache;
cold/warm tests use independent zero-budget resources for their comparison.
The opt-in complete-frame oracle was updated to remain cold but was not run
as an additional benchmark in this verification.

| Frozen item | Identity |
| --- | --- |
| Code | `c49d711c67941d762d984c34df008348f27410c8`; production changes from `da5a34463aaee53f339a604b78b9c9b9728ac5ee`, plus a test-probe correction |
| Source inventory/fingerprint | 540 inputs; `d73bc945a92f2c9db8f4fe6a28b8d4dd1efe46a3e9cf2d5cf55bcb1c29ca7989` |
| ARM application | `d58ae1860a2726f5663b104d9bf48ae39a89af73bc72426b93a8a9b17b142742` |
| ARM JPEG worker | `e39d7a897f13ce53412f42040d124a106634322899df515b5bc2e96b781914cd` |
| Development ZIP | `2f5aa506a5e366a65372ef05384d5260d709b1eccaeb7e54b7039fbe3d4ee00d` |

The [software summary](verification/cpu-resize-cache-20261116/summary.json),
SHA-256 `d73a10a72b9239332832b1eae58b3c7e99638d15a359aa42671f0fae1c02c7c5`,
records 868 passes, zero failures, four ignored tests, including seven new CPU
and four LRU tests. ARM compilation and 15 static package checks/39 entries
passed. Seven host pairs preserved 91 pixel and 84 anchor checkpoints exactly.
The
[source inventory](verification/cpu-resize-cache-20261116/source-inputs.sha256)
and [package record](verification/cpu-resize-cache-20261116/package.json) bind
the R2 artifacts.

## Physical observation

The manual batch reused twelve `1700 x 2360` synthetic PNG guest pages
byte-for-byte, manifest
`0833086bd80c98cea3bdaf7cd4718d33487104b8716c561a63cce292eb462ec0`.
These are not the original Perf05 images.

Fast forward hits were 4/4: initial operation 13 took **205.595 ms**;
operations 17/21/25 took 30.620/21.885/34.031 ms. Three backward CPU-cache hits
avoided resize and took 140.076/134.015/135.739 ms. Canvas/shared preparation
and one fresh read/hash/receipt recheck each remained. These are request-to-model
times, not touch/panel latency.

Nine ready predictions comprised seven CPU hits/no resize and two cold resizes.
Normal/Off-to-Fast also reused CPU Gray8. Processed residency was 13,838,400 bytes
in three entries; source residency was 12,036,000 bytes, both within budget.
UI copying and JPEG calls were zero.

The [previous physical batch](PHYSICAL_PREFETCH_DELIVERY_20261116.md) had one
Fast forward hit at 26.541 ms, two misses at 407.784/437.041 ms, and four back
turns at 406.867-463.908 ms. Manual timing/cache-history differences permit only
observational comparison. Profiles prove avoided resize, not general speedup
or p95. The user reported fewer pauses.

Four captures passed: initial 1/v1, Fast-rounds 2/v9, Fast-idle 2/v9, manual-clear
2/v9. Both retained comparisons matched pixel hashes. Seven Fast frames used
accepted DU/nonflash/no errno; turn completion was unobserved. Manual Full used
accepted GC16/flashing with reported driver completion; optical panel state
remains unknown.

The batch completed with operator stop/status 143, not natural exit 0. All 31
PID/birth identities were gone; native recovery, empty owned transports, and
exact lease release passed. The user confirmed native Home/touch restoration.
The [physical summary](physical/cpu-resize-cache-20261116/summary.json) retains
four reviewed captures. This does not expand device qualification.
Physical summary SHA-256:
`fc591e1d0238125bb4a2318a88bb34250eb4a08aa5de8123fa883a4bbdf26259`.
These are framebuffer-derived projections, not photographs:
[initial](physical/cpu-resize-cache-20261116/initial.projected-panel.png),
[Fast](physical/cpu-resize-cache-20261116/fast-rounds.projected-panel.png),
[idle](physical/cpu-resize-cache-20261116/fast-idle.projected-panel.png), and
[manual clear](physical/cpu-resize-cache-20261116/manual-clear.projected-panel.png).
