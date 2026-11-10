# KOReader Technique Adoption Audit

This is a read-only comparison against BiliComics `0.1.21`. It does not claim
that every suggested technique has been implemented. No new application test,
build or device operation was performed for this audit. Public upstream sources
were read at the versions cited below; research archives were not imported.

Reference versions: KOReader `01ccbd790d6269301ad523e75c45f62d022a6d7b`,
koreader-base `b153a4a252f6e4295af7a569bcfdd59ffa688fa6`, and MuPDF `1.27.2`.
Adopting a mechanism is distinct from copying that project's implementation.

The later `0.1.22` streaming-row and single prepared-page implementation is
tracked in the [follow-up record](PERFORMANCE_STREAMING_PREFETCH_20261110.md).
The table below retains this audit's original `0.1.21` scope.

## Status at a glance

| Technique | Current status | Remaining work or reason for deferral |
| --- | --- | --- |
| Bounded grayscale-source cache | Implemented | It saves decode/gray work, not final raster generation |
| Final rendered-frame cache and next-frame preparation | Not implemented | Requires a separate byte budget, actual foreground priority and source/state invalidation; current encoded prefetch is a different layer |
| Compressed persistence of the currently displayed frame | Not implemented | Compression, flash I/O and reopen benefit have not been measured; it needs a versioned cache contract separate from reading position and authorization |
| Single-channel image preparation | Partly implemented | Gray8 source cache and one-channel float resize exist, but decoder/UI/native format conversions remain |
| Direct grayscale JPEG output | Not implemented | The current decoder is explicitly configured for RGB; JPEG Y and the application's RGB-weighted gray conversion are not universally equivalent |
| Eliminate UI whole-image copy | Not implemented | Reader still produces a vector and clones it into a Slint buffer; a render-into interface remains separate work |
| Precomputed weights and row traversal | Implemented | Bounded to 256 KiB for the horizontal plan, with column fallback |
| Fixed-point resampling arithmetic | Not implemented | Weights and sums remain f32; only the final byte conversion is exact integer bit arithmetic |
| Reused intermediate row buffer / streaming resize | Not implemented | The full vertical f32 intermediate is still retained; row traversal alone is not streaming |
| Semantic refresh policy and per-frame dirty merging | Implemented | It is the existing Bili policy, not full parity with every KOReader device rule |
| Complete per-device display-capability matrix | Partial foundation | Geometry, allowlists, live framebuffer checks and ABI probing exist; per-model waveform/dither capabilities are not fully represented |
| Explicit MTK hardware Y4 dithering | Not implemented | Requires separate pixel-quality, polarity, overlay and firmware validation |
| General GPU or MDP resize offload | Not implemented | No usable resize submission path has been implemented or verified in this application |

## Final-frame caching and persistence

KOReader's cited [document renderer](https://github.com/koreader/koreader/blob/01ccbd790d6269301ad523e75c45f62d022a6d7b/frontend/document/document.lua#L410)
checks a render-keyed tile cache. Its `hintPage` calls the renderer synchronously;
the cited [ReaderView path](https://github.com/koreader/koreader/blob/01ccbd790d6269301ad523e75c45f62d022a6d7b/frontend/apps/reader/modules/readerview.lua#L455)
schedules the hint for a later UI tick. This is not proof of an independent
background rendering thread.

Bili currently retains a 32 MiB / 128-entry unrotated grayscale-source LRU in
[resources.rs](../src/reader/resources.rs). Its small layout cache has no pixels.
[reading.rs](../src/app/reading.rs) prefetches missing encoded files, capped by
the preload preference; demonstration, offline and suspended routes skip that
download path. The current Slint page, spread leaf and overview are active images,
not an LRU of final frames. A requested frame still runs resize and canvas work.

One Scribe final RGBA frame is 18,451,200 bytes (17.60 MiB), before preparation
temporaries. Next-frame preparation could reduce predictable sequential waits,
but it can waste work on a reverse turn or changed setting. It needs actual
foreground priority and bounded ownership, not merely another ordinary worker.
The new cooperative cancellation is a useful prerequisite, not implementation
of prediction or a final-frame cache. The measured mandatory CPU resize cost
was addressed first because that benefits first images and arbitrary navigation.

KOReader's [DocCache persistence selection](https://github.com/koreader/koreader/blob/01ccbd790d6269301ad523e75c45f62d022a6d7b/frontend/document/doccache.lua#L60)
aims to save the displayed page rather than the newest hinted page. The cited
[tile item](https://github.com/koreader/koreader/blob/01ccbd790d6269301ad523e75c45f62d022a6d7b/frontend/document/tilecacheitem.lua)
uses zstd-backed persistence and frees its buffer on eviction.

Bili's [saved anchor and receipts](../src/store.rs) contain position and source
proof, not compressed rendered pixels. Covers are not a current-comic-frame
cache either. A persistent frame would need account/content identity, viewport,
crop, rendering preferences and algorithm-version binding. Current permission
and source-integrity checks must remain. Its compression ratio, CPU cost and
flash read/write benefit are not yet measured. This is unfinished engineering,
not a blanket claim that persistence is unsuitable.

## Single-channel processing and copies

The cited [JPEG bridge](https://github.com/koreader/koreader-base/blob/b153a4a252f6e4295af7a569bcfdd59ffa688fa6/ffi/jpeg.lua)
can request one-component gray output. The [MuPDF document draw bridge](https://github.com/koreader/koreader-base/blob/b153a4a252f6e4295af7a569bcfdd59ffa688fa6/ffi/mupdf.lua#L716)
also supports drawing grayscale document pixels into a BB8 allocation.

Bili already keeps Gray8 sources and only one f32 channel in Triangle. A decoder
result that is already `ImageLuma8` can be reused directly. However,
[jpeg.rs](../src/image_integrity/jpeg.rs) explicitly requests RGB, and
[reader.rs](../src/reader.rs) then applies its existing integer RGB-to-gray and
alpha-on-white formula. In the vendored decoder, Luma output uses JPEG Y; that
does not generally equal the application's weighted RGB result for colored JPEG.
True single-component grayscale JPEG is a narrower candidate for equivalent
optimization, while retaining the existing completeness checks.

The current image path still generates RGBA, then
`SharedPixelBuffer::clone_from_slice` copies it into Slint. Later Image clones
share ownership and are not more full-frame copies. Premultiplied/RGB submission
and direct render-into remain proposals, not active code. Native composition
still uses a full RGB buffer and converts dirty rows back to gray; its temporary
gray buffer is one row, which does not make the entire renderer grayscale.
These are real remaining boundaries. The measured UI import and composition
costs are smaller than the resize hotspot that was optimized in this iteration.

## Weights, traversal and working set

The cited [MuPDF scaler](https://github.com/ArtifexSoftware/mupdf/blob/1.27.2/source/fitz/draw-scale-simple.c#L1810)
uses precomputed contributions and a reused intermediate row window. Its sample
arithmetic also uses integer weights and shifts. Replacing Bili's numerical
algorithm with that implementation would not establish pixel equivalence.

Bili's [Triangle specialization](../src/reader/triangle.rs) now precomputes a
bounded horizontal plan and visits output pixels by row. The filter weights,
normalization and each pixel's accumulation still use the original f32 order.
The exact final-byte conversion is integer arithmetic; it is not fixed-point
weight resampling. The implementation and device comparisons are documented in
the [ARM cause record](PERFORMANCE_ROOT_CAUSE_20261110.md).

The code still allocates `crop_width * target_height` floats and completes the
vertical pass before the horizontal pass. For the measured example, that is
16,864,000 bytes (16.08 MiB), whereas one 1700-element float row is 6,800 bytes
(6.64 KiB). Only this intermediate is being compared, not total process RSS.

A future streaming path could compute one vertical output row, finish that row's
horizontal sampling and reuse its temporary storage. It can preserve per-pixel
axis and accumulation order without swapping the two axes. It still needs its
own budget/fallback/cancellation and full-output proof; it has not been written
or measured. This iteration intentionally validated the already-measured
traversal/rounding change rather than bundling another memory-layout redesign.

There is no explicit NEON, hand-written ARM or MDP implementation in Bili's
resize kernel. The cited KOReader [no_arm_asm patch](https://github.com/koreader/koreader-base/blob/b153a4a252f6e4295af7a569bcfdd59ffa688fa6/thirdparty/mupdf/no_arm_asm.patch)
disables MuPDF's older 32-bit ARM assembly path; it does not support a blanket
claim that hand-written ARM assembly explains all KOReader speed.

## Refresh semantics and hardware paths

KOReader's [UI manager](https://github.com/koreader/koreader/blob/01ccbd790d6269301ad523e75c45f62d022a6d7b/frontend/ui/uimanager.lua#L543)
queues semantic paint/refresh requests and reduces their submissions. Bili
already has its own [semantic scheduler](../src/platform/refresh.rs) and
[backend merge](../vendor/slint-backend-kindle/src/refresh.rs): current-frame
requests are merged with dirty bounds, rotated/clipped, and assigned an existing
waveform priority. This is not cross-frame semantic deduplication or complete
device-policy parity.

KOReader's [MTK display path](https://github.com/koreader/koreader-base/blob/b153a4a252f6e4295af7a569bcfdd59ffa688fa6/ffi/framebuffer_mxcfb.lua#L669)
conditionally enables Y4 dithering for eligible device/rendering combinations.
Bili recognizes the MTK update structure, but leaves its dither/quantization
fields at zero and does not explicitly set the Y4 enable flag. Its existing
software Bayer output, DU monochrome flag and optional binary threshold are
different mechanisms. Even `dither=false` currently retains 16-level quantization;
it is not raw Gray8 passthrough. Hardware dithering affects the final framebuffer,
including text/overlays, rotation and inversion, so it needs an independently
verified output path rather than a flag-only substitution.

The cited [image scaling route](https://github.com/koreader/koreader/blob/01ccbd790d6269301ad523e75c45f62d022a6d7b/frontend/ui/renderimage.lua#L303)
uses the [CPU MuPDF scaler bridge](https://github.com/koreader/koreader-base/blob/b153a4a252f6e4295af7a569bcfdd59ffa688fa6/ffi/mupdf.lua#L1082).
The inspected MDP completion handling is display-buffer processing, not a
ready-made hardware comic-resize implementation. The separate device survey
reported dedicated MDP interfaces, but this audit neither submitted a task nor
validated formats, quality, concurrency or gain. Hardware offload remains an
independent candidate, not an adopted or accepted feature.

## Follow-up priorities

The completed scope is the measured resize/cancellation improvement, not the
entire KOReader-inspired list. Deferral was primarily an implementation-scope
choice: first remove mandatory work with demonstrated ARM cost and exact-output
proof. That is not a reason to reject the remaining useful mechanisms forever.

The following decisions use the final R51 observations: reused-gray preparation
takes approximately 438-463 ms, UI import roughly 45-66 ms, and a new native frame
roughly 180 ms. These are sample-specific software stages, not panel latency.

| Candidate | Decision | Reason and acceptance condition |
| --- | --- | --- |
| Streaming intermediate rows | Adopt next, after a separate equivalence trial | It removes a known large working buffer and can preserve per-pixel arithmetic. Prove complete frames, metadata, fallback and cancellation; measure real peak memory and speed rather than promising a total-RSS reduction |
| One prepared next-frame slot | Adopt as a bounded sequential-reading feature | A hit can move approximately 438-463 ms of observed preparation out of the click path. Source validation, UI import and presentation still remain. Require foreground miss performance not to regress, with one slot, explicit bytes and cancellation |
| Opaque/premultiplied submission and eliminating UI copy | Adopt in a separate small step when equivalence and measurements pass | This is known extra work, but the measured import span is only tens of milliseconds. Prefer a narrowly proven opaque Reader path, then a backend-neutral render-into API; do not rewrite the entire compositor first |
| Direct grayscale JPEG | Adopt selectively, starting with genuinely one-component JPEG | Avoiding gray-to-RGB-to-gray is an equivalent opportunity. Colored JPEG's Y output does not equal the current weighted RGB conversion; broader adoption requires a deliberate image-quality decision and representative source data |
| Compressed persistence of the current rendered frame | Second-stage experiment, not yet a default | It helps process restart or reopening, not existing same-process sleep continuation. Measure compression/readback and flash-write costs; it does not remove font preparation or native launcher work |
| Fixed-point weight resampling | Defer | It changes numerical behavior and potentially final dither dots. Equivalent streaming and caching remain available without that quality tradeoff; establish a need before changing the numeric algorithm |
| Hardware Y4 / MDP resize | Separate exploratory work, not a mainline dependency yet | Driver structure/interface availability does not prove appropriate formats, quality, concurrency or gain. Y4 changes final-frame treatment; the inspected KOReader image path is not a ready-made MDP resize implementation |

For prepared-frame caching, the existing RGBA interface needs about 17.60 MiB
per full Scribe frame. A final Gray8 cache would need about 4.40 MiB of pixel
payload but still require expansion/import at consumption. Choose an explicit
representation and byte budget rather than copying KOReader's slot count.
Begin with a stable single-page, same-chapter, locally available next target;
existing generation/proof rules and actual priority admission remain necessary.
Rapid navigation, reverse turns, settings changes and expired access must fall
back correctly. This feature improves a predictable wait; it does not make the
underlying per-page computation disappear.

The recommended sequence is a standalone streaming-row equivalence experiment,
then the smallest prepared-frame slot with foreground-priority proof. Image-copy
and true-gray-JPEG changes can be measured independently. Persistent-frame and
hardware work should follow demonstrated workload benefit rather than checklist
completion. None of these remaining items is marked implemented by this audit.
