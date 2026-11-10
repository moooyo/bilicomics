# image 0.25.10 Triangle reference

This directory retains complete, unmodified upstream files used to derive the
single-channel Triangle resize specialization in `src/reader/triangle.rs`. It is a
reference snapshot, not a complete crate checkout, dependency replacement, or
Cargo patch. The archived `Cargo.toml` is not used to build the application.

## Provenance

- Crate: `image`, version `0.25.10`.
- Upstream repository: <https://github.com/image-rs/image>.
- Crate source: `registry+https://github.com/rust-lang/crates.io-index`.
- Package checksum pinned in the application's `Cargo.lock`:
  `85ab80394333c02fe689eaf900ab500fbd0c2213da414687ebf995a65d5a6104`.
- Upstream Git revision recorded in `.cargo_vcs_info.json`:
  `76e57184f22772dad1138e96954e57945406b15e`.
- Snapshot date: `2026-11-09`.
- Acquisition: the exact unpacked Cargo registry files on the designated remote
  verification environment were copied unchanged. Their SHA-256 values were
  computed there before copying and are recorded in `SHA256SUMS`.

The upstream crate declares `MIT OR Apache-2.0`. Both complete license texts are
retained as `LICENSE-MIT` and `LICENSE-APACHE`. Existing source notices remain
unchanged. This reference does not select a license for the BiliComics project.

## Source map

Line numbers refer to the unmodified files retained here.

| File and lines | Relevant behavior |
| --- | --- |
| `src/imageops/sample.rs:214-219` | Triangle kernel and its strict support boundary. |
| `src/imageops/sample.rs:236-327` | Horizontal coordinates, normalization, accumulation, clamping, and integer conversion. |
| `src/imageops/sample.rs:490-572` | Vertical coordinates, normalization, four-channel f32 intermediate, and accumulation. |
| `src/imageops/sample.rs:118-146` | `FloatNearest`, including `round()` before conversion to `u8`. |
| `src/imageops/sample.rs:974-1016` | Empty and same-size paths, Triangle support, and vertical-then-horizontal order. |
| `src/imageops/mod.rs:54-82` | The crop view clamps its origin and dimensions to the source. |
| `src/color.rs:362-371,491` | `Luma<u8>` supplies `(Y,255,255,255)` and keeps only the first output channel. |
| `src/images/buffer.rs:1800,1818` | The concrete `GrayImage` and `Rgba32FImage` aliases. |
| `src/images/buffer.rs:1249-1252,1460-1462` | New buffers retain color metadata without transforming pixels. |
| `src/images/sub_image.rs:185-209` | Crop reads apply source offsets and metadata allocation delegates to the underlying image. |
| `src/images/generic_image.rs:116-141` | Buffer construction and metadata contract. |
| `src/traits.rs:38-69` | Primitive channel ranges, including `u8` values from zero to 255. |
| `src/utils/mod.rs:129-141` | The reference clamp operation. |

## Specialization scope and numerical contract

The specialization retains the same immutable crop, target dimensions, Triangle
kernel, f32 coordinate calculations, edge clipping, weight normalization, and
vertical-then-horizontal ordering. For each output component, weights are summed
in increasing input order, each weight is divided by that sum, and samples are
accumulated with separate multiplication and addition in the same order. No
f64 substitution, reciprocal multiplication, reordered reduction, FMA, or
fast-math operation is introduced. There is no rounding between passes. The
final byte remains exactly equal to clamping to zero through 255, applying
`f32::round()`, and performing Rust's saturating conversion to `u8`.

The final conversion now uses an exact IEEE-754 bit calculation. Negative
values, negative zero and every NaN payload produce zero; positive infinity and
values at least 255 produce 255. In the remaining `[0.5, 255)` interval, the
significand `M` and biased exponent `E` represent the value as `M / 2^(150-E)`.
Adding the integer half-unit bias and shifting produces precisely the same
halfway-away result without a per-pixel `roundf` call. The intermediate sum is
below `2^25`, and the shift is between 16 and 24. This equivalence concerns the
output byte, not floating-point exception flags. Adding `0.5` in floating point
before casting is not equivalent and is not used.

The only intermediate pixel data retained by the specialization is the first f32
channel. The upstream implementation stores four f32 channels at this point even
when its input and final output are `Luma<u8>`; the other three channels do not
contribute to the final gray output. With a ready horizontal plan, the specialized
path reserves one `crop_width`-element f32 row. It calculates that row's vertical
results in the original sample order, stores them as f32, then calculates and
writes the complete horizontal output row before reusing the intermediate.
Neither axis is exchanged or fused, and no intermediate value is quantized.

The intermediate row's pixel payload is `4 * crop_width` bytes, compared with
`4 * crop_width * target_height` for the previous full single-channel matrix and
`16 * crop_width * target_height` for the generic four-channel matrix. For a
1700-pixel crop width and 2480-pixel output height, the single-channel pixel
payload changes from 16,864,000 bytes to 6,800 bytes. These values describe that
buffer only; they are not total RSS or a measured speedup. Actual vector
capacities, including the row, are observed by test-only allocation hooks.

Horizontal coefficients may be retained in a bounded plan so independent output
pixels are visited row by row. Each column's original weights and each pixel's
accumulation order are unchanged. Plan preparation precedes vertical pixel work
and stays inside the existing resize timing span. A first pass uses the same
axis-range calculation to count the exact coefficient requirement, with checked
multiplication and addition before any
reservation. Each vector is reserved once; the first actual capacity is checked
before reserving the second vector, and their combined actual element capacity
is checked again before the plan is accepted. A failed reservation, overflow or
budget excess selects the original full single-channel matrix and column
traversal with the same exact byte conversion. A failed row reservation releases
the ready plan and takes that same fallback. This fallback still requires
`4 * crop_width * target_height` bytes of intermediate pixel payload; streaming
does not make the pre-existing fallback allocation infallible.

The plan limit is 256 KiB for
`columns.capacity() * size_of::<HorizontalColumn>() + coefficients.capacity() * 4`.
The plan appends and normalizes directly into its one coefficient vector, with
no additional per-column scratch allocation. Its capacity is checked before
each append, so filling cannot cause an implicit reallocation. This is a bound
on the accepted plan's element capacities and reservation requests, not allocator
bookkeeping, transient allocator rounding, total RSS, the vertical row/fallback
matrix or output image. The old reusable per-column weight vector remains the
fallback; it and the vertical stage's weight vector are outside this plan budget.
The streamed path retains one reusable vertical weight vector alongside the plan
and row. The fallback releases vertical weights before computing its horizontal
weights.

Empty crops and zero target dimensions delegate to the original
`imageops::resize` on the same crop view. Default exact-size copies also use that
helper; the cancellable path performs the equivalent row copy in bounded chunks.
For all specialized output, `view.buffer_with_dimensions` preserves the
underlying source's color metadata, matching its propagation through the
reference's intermediate buffer.

The application-worker path cooperatively checks cancellation every 64 plan
columns, every 64 coefficients in long generation/normalization/accumulation,
every 256 pixel visits, and every 64 KiB of exact-copy work. Plan passes report
completed columns and restart at zero for filling; coefficient checks report
progress within the current column. Vertical pixel counters retain their full
logical matrix positions despite row-buffer reuse. Horizontal pixel counters
count visits in the selected row or fallback-column traversal order. Cancellation
returns no image, including during plan construction or after partial local output work.
Allocation and kernel operations are not interruptible work blocks. The default
path supplies no cancellation source and adds no atomic loads to its pixel loops.

The application still owns source validation, immutable content identity,
rotation, crop selection, target-size calculation, Bayer quantization, canvas
placement, and reading-anchor updates. This module does not replace those
operations or alter their policies.

The reader uses this specialization after source validation and crop selection.
Regression coverage compares exact gray pixels against the pinned dependency
and a test-only pre-streaming `0.1.21` full-matrix reference across boundary and
pseudorandom cases, forced plan fallback, real uncancelled worker paths, and
nondefault color metadata, as well as complete rendered frames and reading
anchors. Allocation observations cover successful row use, plan fallback and
injected row-reservation failure. Cancellation tests cover interleaved rows,
global logical progress, long vertical/horizontal accumulation and construction
before any intermediate pixel buffer exists. Plan and pixel cancellation cases
must return no partial image. Threshold neighbors, special float values and
integer-byte boundaries cover the conversion; an explicitly ignored release diagnostic can scan all
75,431,936 finite `f32` values in its nontrivial interval. The diagnostic release
probe retains the generic reference for same-build comparisons. Host measurements
and physical-device observations must remain separate; no hardware result is
asserted by this reference snapshot.
