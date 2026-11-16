# Software grayscale settings supplement: 2026-11-16

This twenty-eight-image implementation record supplements the immutable
[complete baseline](../2026-11-14-v0.1.25/README.md) and
[persistent Fast baseline](../2026-11-16-fast-mode/README.md).
It records twenty-four settings captures and four complete Fast pages at the
updated physical raster resolution. It retains the existing
component and interaction conventions. It does not replace the approved design
or establish physical Kindle acceptance.

| Identity | Value |
| --- | --- |
| Application | BiliComics 0.1.25 |
| Base commit | `217c4c0d11808d1412ca16a81f58c2b57e6c82d2` |
| Worktree | Uncommitted changes; no clean-checkout claim |
| Compiled-input fingerprint / manifest SHA-256 | `eec20168873c42ef11a8c220e923795b3d74d9ba465730e9fd7a9cfaf8f67c8e` |
| Compiled-input files | 536 |
| Desktop executable SHA-256 | `26da326e5417ca40811eedc31bf9678c6c804d7a695ca6efd91a6220a1a095f7` |
| Capture run | `bilicomics-spatial-dither-20261116-01a1d11a-r4 / spatial-ui-r1` |
| Capture harness SHA-256 | `ef4ca6da1fcb706123ef69e103ecc2dc180c0a33aacafec9e456ee68cfef8ff6` |
| Host, renderer, and scale | test-env, Slint winit software renderer, 2 |
| Input | Actual XTest pointer and wheel events |
| Paperwhite 6 profile | 1264 x 1680 portrait pixels, 14 captures |
| Scribe profile | 1860 x 2480 portrait pixels, 14 captures |
| Landscape | Portrait width and height are swapped |
| User date and timezone | 2026-11-16 / Asia/Shanghai |

## Original captures

| Scene | Paperwhite 6 | Scribe |
| --- | --- | --- |
| Account defaults, portrait, Off | [PNG](raw/pw6/10-account-cpu-off-portrait.png) | [PNG](raw/scribe/10-account-cpu-off-portrait.png) |
| Account defaults, portrait, 8x8 Bayer | [PNG](raw/pw6/10-account-cpu-bayer8-portrait.png) | [PNG](raw/scribe/10-account-cpu-bayer8-portrait.png) |
| Account defaults, portrait, High-precision Floyd-Steinberg | [PNG](raw/pw6/10-account-cpu-floyd-steinberg-portrait.png) | [PNG](raw/scribe/10-account-cpu-floyd-steinberg-portrait.png) |
| Account defaults, landscape, Off | [PNG](raw/pw6/10-account-cpu-off-landscape.png) | [PNG](raw/scribe/10-account-cpu-off-landscape.png) |
| Account defaults, landscape, 8x8 Bayer | [PNG](raw/pw6/10-account-cpu-bayer8-landscape.png) | [PNG](raw/scribe/10-account-cpu-bayer8-landscape.png) |
| Account defaults, landscape, High-precision Floyd-Steinberg | [PNG](raw/pw6/10-account-cpu-floyd-steinberg-landscape.png) | [PNG](raw/scribe/10-account-cpu-floyd-steinberg-landscape.png) |
| Per-comic Reader, portrait, Off | [PNG](raw/pw6/20-reader-cpu-off-portrait.png) | [PNG](raw/scribe/20-reader-cpu-off-portrait.png) |
| Per-comic Reader, portrait, 8x8 Bayer | [PNG](raw/pw6/20-reader-cpu-bayer8-portrait.png) | [PNG](raw/scribe/20-reader-cpu-bayer8-portrait.png) |
| Per-comic Reader, portrait, High-precision Floyd-Steinberg | [PNG](raw/pw6/20-reader-cpu-floyd-steinberg-portrait.png) | [PNG](raw/scribe/20-reader-cpu-floyd-steinberg-portrait.png) |
| Per-comic Reader, landscape, Off | [PNG](raw/pw6/20-reader-cpu-off-landscape.png) | [PNG](raw/scribe/20-reader-cpu-off-landscape.png) |
| Per-comic Reader, landscape, 8x8 Bayer | [PNG](raw/pw6/20-reader-cpu-bayer8-landscape.png) | [PNG](raw/scribe/20-reader-cpu-bayer8-landscape.png) |
| Per-comic Reader, landscape, High-precision Floyd-Steinberg | [PNG](raw/pw6/20-reader-cpu-floyd-steinberg-landscape.png) | [PNG](raw/scribe/20-reader-cpu-floyd-steinberg-landscape.png) |
| Quiet Fast page, portrait | [PNG](raw/pw6/14-reader-fast-portrait.png) | [PNG](raw/scribe/14-reader-fast-portrait.png) |
| Quiet Fast page, landscape | [PNG](raw/pw6/25-reader-fast-landscape.png) | [PNG](raw/scribe/25-reader-fast-landscape.png) |

All PNGs and JSON sidecars are copied byte for byte from the capture run.
No image is redrawn, thresholded, resized, composited, or reconstructed.
The bundled fictional comic is the only Reader source. Each accepted capture
waits at least 1.5 seconds and requires three identical X11 samples.

## Checks and review

The real controls select Off, Bayer8, and Floyd-Steinberg on both settings
surfaces and orientations. Durable preferences retain the selected method,
an explicit Off choice, and the last Floyd-Steinberg method across an ordinary
cold restart. Reader anchors and saved clearing intervals remain unchanged.
New methods are available independently of a Y4 or JPEG capability gate.

Normal Reader rasters use the actual physical window dimensions and retain
only native gray codes after final display sampling. This is native-code
membership, not a requirement that every comic uses all sixteen codes or a
physical panel-depth measurement. Actual presented/target algorithm versions
and disabled Y4 intent are checked separately from the visible selector.

Fast remains binary while the grayscale choice is changed and saved. The
normal choice is restored through real controls; quiet pixels match their
earlier same-method observation exactly. These desktop pixel and state checks
cannot prove DU ioctl delivery, optical appearance, ghosting, or panel latency.
The separate complete [Fast regression](fast-regression-report.json) passes
twenty captures and eight cold restarts; its four quiet page PNGs are preserved
here with their original sidecars and [run metadata](fast-regression-run.json).

The original [capture-report.json](capture-report.json) and PNG sidecars retain
their pending visual-review fields unchanged. Completed human review is stored
separately in [verification.json](verification.json). The source manifest and
[capture-run.json](capture-run.json) bind build and harness identities.
The [image identity record](visual-equivalence.json) binds earlier individually
reviewed originals to the final identical PNGs. Six final originals with a
typography or bounded scroll-position difference were inspected directly.
[manifest.json](manifest.json) lists every frozen file except its own checksum.

This is offline desktop-profile evidence. No physical device, native driver,
live account, acquired comic, or upstream transaction was used. Source-project
Aidoku/Scribe results keep their independent scope. Keep this directory
immutable after sealing.
