# Local strict decoding changes

The upstream version remains `0.5.15`. The application selects this directory
through its Cargo patch configuration and explicitly enables strict decoding for
JPEG integrity validation. The normal upstream decoding path retains its
existing output and recovery behavior.

## Scope

Only `src/bitstream.rs`, `src/mcu.rs`, and `src/mcu_prog.rs` differ from the pinned
upstream Rust source. The unified patch is `patches/0001-strict-entropy-completeness.patch`.

The patch does not add a separate JPEG entropy interpreter. It keeps the upstream
baseline and progressive decoders, Huffman lookup tables, coefficient handling,
inverse transforms, color conversion, and SIMD implementations.

## Strict behavior

- Bitstream bookkeeping distinguishes real encoded entropy bits from synthetic
  bits used by fast Huffman lookahead. Peeking may still use the upstream padded
  window. A coefficient or refinement bit may not consume synthetic bits. The
  resulting exhaustion flag survives scan and restart resets.
- Strict scan and restart boundaries allow only the current byte's remaining
  one-valued alignment bits. Strict marker reading accepts marker fill bytes but
  does not skip arbitrary entropy bytes or stuffed bytes to find a marker.
- Sequential decoding must finish its expected blocks and component scans before
  successful completion. A marker discovered by lookahead is deferred until the
  current scan's buffered coefficients have been consumed. Strict restart
  markers must be ordered; the final MCU does not require an extra restart.
- Progressive decoding validates the declared coefficient scan history, completes
  every declared scan, rejects EOB runs that exceed a scan or restart interval,
  and requires each component's initial DC scan before EOI. It does not require
  optional AC bands or all refinement bit planes to be sent.
- Strict coefficient decoding rejects out-of-range DC categories, AC runs and
  sizes, and invalid refinement runs instead of wrapping or clamping them.

The existing upstream canonical Huffman construction already rejects
oversubscribed trees and all-ones codes; it is retained unchanged. This is
necessary so alignment padding cannot stand in for missing Huffman symbols.

## Application boundary

`src/image_integrity/jpeg.rs` separately checks SOI, segment lengths, byte
stuffing, the first terminal EOI, and trailing data. It preflights the RGB output,
buffered coefficient planes, metadata reserve, and conservative row/scratch
storage against the shared decode allocation budget. The encoded-size and pixel
limits remain shared with the other image formats.

The application's sequential JPEG capability is deliberately narrower than the
upstream decoder's feature set. It accepts a single SOS containing every frame
component, and rejects sequential component-separated or multiple-SOS images as
unsupported before pixel decoding. A single-component frame must use 1x1
sampling. This excludes the unestablished non-interleaved sampled-row and
cross-scan restart-accounting paths without changing the vendored algorithms.
Within the accepted sequential path, SOS parameters must be exactly `Ss=0`,
`Se=63`, `Ah=0`, `Al=0`; violations are invalid encoding rather than an
unsupported capability. Excluded scan modes do not produce corruption evidence.
Ordinary full-component single-scan baseline and supported progressive images
remain enabled. The strict branches retained in other upstream sequential paths
do not establish application support for those paths.

Known resource limits and unsupported coding modes do not produce corruption
evidence. Ambiguous upstream format errors are conservatively reported as
unsupported. A success proof is created only after the strict decoder returns a
complete pixel buffer.

These source changes and the synthetic regression fixtures are not a live-service
or hardware verification result. Validation is performed by the project's
designated remote verification workflow; no local test result is asserted here.
