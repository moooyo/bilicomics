# Software dithering verification: 2026-11-16

This record covers BiliComics' own implementation of the
[software grayscale choices](SOFTWARE_DITHERING.md). All execution took place
on `test-env`. Source-project Aidoku/Scribe results remain reference evidence;
this candidate has no new BiliComics physical-device or optical acceptance.

## Candidate identity

| Item | SHA-256 or identity |
| --- | --- |
| Base commit | `217c4c0d11808d1412ca16a81f58c2b57e6c82d2` |
| Worktree | Uncommitted changes; no clean-checkout claim |
| Frozen workspace | `/root/bilicomics-spatial-dither-20261116-01a1d11a-r4` |
| Compiled-input count | 536 |
| Compiled-input fingerprint | `eec20168873c42ef11a8c220e923795b3d74d9ba465730e9fd7a9cfaf8f67c8e` |
| Desktop executable | `26da326e5417ca40811eedc31bf9678c6c804d7a695ca6efd91a6220a1a095f7` |
| ARM application | `137910b4e9f142c53eb031817aaa3c72a2dfe7abfc637885e8db5e591d55908f` |
| ARM JPEG worker | `e39d7a897f13ce53412f42040d124a106634322899df515b5bc2e96b781914cd` |
| Development ZIP | `129baabec1744890e67404e56624fb80fd7fe68a94f09bbc0c04290871ac5d76` |

The compiled-input manifest includes the frozen reference tree used by the PNG
oracle tests and both C codec inputs. CPU pixel processing and final-frame
identity changed; JPEG encoding/geometry admission and DMA ownership did not.
The new binaries have their own identities and do not inherit earlier physical
qualification merely because the JPEG worker source is unchanged.

## Executed software and package checks

| Check | Result | Preserved evidence |
| --- | --- | --- |
| Locked, offline check and desktop build | Passed; executable sealed by digest | [Core/package report](verification/spatial-dither-20261116/core-package.json) |
| Default-feature application library | 825 passed, 0 failed, 3 existing opt-in tests ignored | [Core/package report](verification/spatial-dither-20261116/core-package.json) |
| Kindle-feature spatial tests on Linux host | 27 passed, 0 failed | [Original output](verification/spatial-dither-20261116/kindle-spatial-tests.txt) |
| Kindle-feature refresh routing on Linux host | 7 passed, 0 failed | [Original output](verification/spatial-dither-20261116/kindle-refresh-tests.txt) |
| Package fixtures | 8 passed | [Original output](verification/spatial-dither-20261116/package-fixtures.txt) |
| ARM application and worker build | Passed | [Core/package report](verification/spatial-dither-20261116/core-package.json) |
| Static development ZIP and ARM ELF checks | 15 passed; 39 exact archive entries | [Package report](verification/spatial-dither-20261116/package.json) |

Build concurrency was limited to two Cargo jobs. Library tests ran serially,
and temporary files and build caches belonged to this task. The targeted
Kindle-feature tests execute on the Linux host and are not ARM or driver runs;
they overlap the new default-feature coverage and are not an additional set of
unique product cases. The three opt-in tests were not reported as benchmarks.

The package keeps the three original spatial-dither license/provenance files
and excludes the PNG research fixtures and algorithm research source from the
development ZIP. Missing or replaced notices are rejected. Existing helper
executable, ELF, empty allowlist, and JPEG provenance checks remain in place.

## Actual UI, persistence, and final sampling

The final spatial UI runner passes eight main sessions with twenty-four
settings captures and eight cold-restart/Fast-preservation groups across
Paperwhite 6 and Scribe in portrait and landscape. All choices use actual
XTest input and the real Rust/Slint application. It verifies durable Off and
remembered Floyd, untouched reading anchors, saved clearing intervals, and
availability without Y4. Presented and target version identifiers must match
the selected algorithm. Quiet Normal PNGs stay within `0,17,...,255` after
final physical-size display sampling; they need not contain all sixteen codes.

The separate complete Fast matrix passes twenty captures and eight cold
restarts. Fast stays binary while normal-mode CPU choices are changed and
saved, and restoring Normal reproduces the earlier same-method page PNG
exactly. Idle checks retain pixels and render identity. These are desktop
pixel/state observations, not native DU submission or panel-update proof.

The [reviewed UI supplement](uiux/baselines/2026-11-16-spatial-dither/README.md)
preserves twenty-four settings originals and four complete Fast page originals,
their sidecars, both unmodified capture reports, actual run metadata, and the
536-input source manifest. Every final image was either directly reviewed or
matched byte for byte to an individually reviewed original. Twenty-two PNGs
match the reviewed earlier candidate; six final glyph/scroll-state originals
were inspected directly. No image was resized, thresholded, or reconstructed.
Human review remains separate from the original reports' pending-review fields.
The sealed collection has 65 files and manifest SHA-256
`f9e4abd2eb3ca2b210d00e0149ecdef9b70245712a56ffbfe78f81f10ecf7369`.

The accepted spatial runner SHA-256 is
`ef4ca6da1fcb706123ef69e103ecc2dc180c0a33aacafec9e456ee68cfef8ff6`;
the Fast runner is
`3201e3e3c1f563e69b4864d4e4fcf1f36429553a7e44317f69668ba562afef8b`.
Both use fresh demo data and wait for actual stable X11 paints rather than
treating model readiness alone as displayed pixels. The narrow Paperwhite
landscape selector retains its 12-pixel labels and full touch bounds; its
selected label uses regular weight with black fill/white text to keep the
complete Floyd-Steinberg name visible. Other rows retain their existing weight.

## Pixel and lifecycle coverage

Both the adapted algorithm module and the complete production Reader raster
are compared against the lossless `1860 x 2480` Off, Bayer8, and Q8
Floyd-Steinberg expected images. All comparisons are pixel-exact. The expected
files and original Gray8 input are first bound to their frozen SHA-256 values.

Additional tests cover two-dimensional Bayer phase across odd widths, Q8
signed-error remainder conservation and serpentine direction, cancellation
inside wide rows and after partial quantization, complete-viewport placement,
white margins, four fits, rotations, zoom, source proofs, and shared Gray8
caches. Uniform-patch checks establish 256 strictly increasing regional means
for each spatial algorithm while all output samples remain on the sixteen
native codes. These are mathematical software results, not panel reflectance
measurements.

Application tests cover default Bayer8, legacy software/hardware migration,
unknown methods, explicit Off and remembered Floyd across Store reopening,
independent JPEG settings, retained old pixels during a change, retired
algorithm results, retired viewport results, versioned prepared-frame keys,
thumbnail proof invalidation, and physical-size render targets. Product CPU
choices never request Y4. Explicit low-level raw/Y4 research fixtures retain
their old-frame, sleep, expiry, and exit protections.

Fast remains directly binary and bypasses the selected sixteen-tone method.
The saved grayscale choice survives Fast and normal-mode restoration without
changing the clearing interval or reading anchor. Manual Full is preserved;
quiet Fast still bypasses periodic and delayed grayscale clearing.

No live account, acquired comic, upstream write, native video driver,
firmware emulator, or physical Kindle was used by these checks. ARM binaries
were built and inspected statically. Aidoku's Scribe timings and native pixel
readback remain scoped to its separately recorded candidate.
