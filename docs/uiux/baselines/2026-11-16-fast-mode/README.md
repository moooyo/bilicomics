# Persistent fast reading UI supplement: 2026-11-16

This twenty-image implementation record supplements the immutable
[complete baseline](../2026-11-14-v0.1.25/README.md) and
[JPEG settings supplement](../2026-11-15-jpeg-acceleration/README.md).
It records Normal/Fast controls and quiet binary pages. It does not replace
the approved design or establish physical-device acceptance.

| Identity | Value |
| --- | --- |
| Application | BiliComics 0.1.25 |
| Base commit | `2da87ab512fded000174b7542767082a3a94e147` |
| Source worktree | Uncommitted changes; no clean-checkout claim |
| Source fingerprint / manifest SHA-256 | `f1d99a8addddf145c0ab46683619156d04a6beae54380f329e18a484532fc7ee` |
| Source input files | 518 |
| Desktop executable SHA-256 | `497dc507f433019828874f5789496eacce7ed4c0d7c9e8846add56d3fe4bc067` |
| Capture run | `bilicomics-fast-reading-20261116-01a1d11a-r5 / fast-ui-r2` |
| Capture harness SHA-256 | `133cc74a0bf0a15816901d47ba57ec6786bbc6dab22996e86c729c51f0049d37` |
| Capture host and renderer | test-env, Slint winit software renderer, scale factor 2 |
| Input | Actual XTest pointer and wheel events |
| Paperwhite 6 profile | 1264 x 1680 portrait pixels; 10 captures |
| Scribe profile | 1860 x 2480 portrait pixels; 10 captures |
| Landscape | Portrait width and height are swapped |
| User date and timezone | 2026-11-16 / Asia/Shanghai |

## Original captures

| Scene | Paperwhite 6 | Scribe |
| --- | --- | --- |
| Account Normal, portrait | [PNG](raw/pw6/10-account-reading-normal-portrait.png) | [PNG](raw/scribe/10-account-reading-normal-portrait.png) |
| Account Fast, portrait | [PNG](raw/pw6/10-account-reading-fast-portrait.png) | [PNG](raw/scribe/10-account-reading-fast-portrait.png) |
| Account Normal, landscape | [PNG](raw/pw6/10-account-reading-normal-landscape.png) | [PNG](raw/scribe/10-account-reading-normal-landscape.png) |
| Account Fast, landscape | [PNG](raw/pw6/10-account-reading-fast-landscape.png) | [PNG](raw/scribe/10-account-reading-fast-landscape.png) |
| Reader Normal, portrait | [PNG](raw/pw6/20-reader-settings-normal-portrait.png) | [PNG](raw/scribe/20-reader-settings-normal-portrait.png) |
| Reader Fast, portrait | [PNG](raw/pw6/20-reader-settings-fast-portrait.png) | [PNG](raw/scribe/20-reader-settings-fast-portrait.png) |
| Quiet Fast page, portrait | [PNG](raw/pw6/14-reader-fast-portrait.png) | [PNG](raw/scribe/14-reader-fast-portrait.png) |
| Reader Normal, landscape | [PNG](raw/pw6/27-reader-settings-normal-landscape.png) | [PNG](raw/scribe/27-reader-settings-normal-landscape.png) |
| Reader Fast, landscape | [PNG](raw/pw6/27-reader-settings-fast-landscape.png) | [PNG](raw/scribe/27-reader-settings-fast-landscape.png) |
| Quiet Fast page, landscape | [PNG](raw/pw6/25-reader-fast-landscape.png) | [PNG](raw/scribe/25-reader-fast-landscape.png) |

The PNGs and their sidecars are copied byte for byte from the capture run.
No image is redrawn, thresholded, resized, composited, or reconstructed.
All content belongs to the bundled fictional demonstration. Each baseline
capture waits at least 1.5 seconds and requires three identical X11 samples.

## Verification and review

The real application controls select both modes, retain the saved every-six
and dithering preferences, disable periodic clearing in Fast, and preserve the
source anchor. Quiet Fast captures contain only black and white pixels and
remain unchanged over bounded idle samples. Eight ordinary cold restarts
retain Fast; actual controls restore Normal and the original saved choices.
Restored Normal reader pixels match their original quiet capture exactly.

Account wheel checks use stable, actually presented Reading frames and keep
the header and bottom navigation unchanged. Periodic-clearing and JPEG
controls remain reachable within the bounded reading area in both orientations.
The refresh selector and its explanation use sequential layout without overlap.

Human review of all twenty original captures is recorded separately in
[verification.json](verification.json). Every final PNG is byte-identical to
its reviewed fast-ui-r1 image; the identity pairs are retained there.
The original
[capture-report.json](capture-report.json) and PNG sidecars retain their
`visual_review_status: pending` fields unchanged. The source manifest and
[capture-run.json](capture-run.json) bind the exact build and runner identities.
[manifest.json](manifest.json) records every frozen file except its own checksum.

These are offline desktop profile captures. Their binary pixels, stable
screenshots, and request-policy unit tests do not establish native ioctl
delivery, actual DU/GC16 updates, panel ghosting, latency, or physical support.
No live account, firmware emulator, native JPEG driver, or physical Kindle was
used. Keep this directory immutable after sealing.
