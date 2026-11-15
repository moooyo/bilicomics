# JPEG acceleration UI supplement: 2026-11-15

This six-image implementation record supplements the
[complete 2026-11-14 baseline](../2026-11-14-v0.1.25/README.md), which retains
51 captures of the full application. It records the JPEG acceleration controls
and their explanation on account reading settings and portrait/landscape reader
settings. It does not replace the earlier baseline or expand design approval.

The PNGs and matching state sidecars under `raw/` are copied byte for byte from
the final R3 captures. No screenshot was redrawn, retouched, resized, composited,
or reconstructed. All displayed content belongs to the bundled fictional demo.

| Identity | Value |
| --- | --- |
| Application | BiliComics 0.1.25 |
| Source base commit | `2da87ab512fded000174b7542767082a3a94e147` |
| Source worktree | Uncommitted changes; no clean-checkout claim |
| Source fingerprint / manifest SHA-256 | `f3e47bc467922a429c291cbd62f317aa912c4e14bdc8e60b85ba4ea369e99a55` |
| Source inputs | 514 files recorded in `source-inputs.sha256` |
| Executable SHA-256 | `0656bb79039344b3b59fd4815ec3ed03125df5d0b69ba27ed99aa5d70dda4e5c` |
| Capture run | `bilicomics-native-media-20261115-01a1d11a-r3 / media-ui-final` |
| Capture host | `test-env` |
| Renderer | Slint winit software renderer, scale factor 2 |
| Capture input | Actual XTest pointer input |
| Paperwhite 6 profile | `pw6`, 1264 x 1680 portrait pixels, 3 captures |
| Scribe profile | `scribe`, 1860 x 2480 portrait pixels, 3 captures |
| Landscape | Portrait width and height are swapped |
| User date / timezone | 2026-11-15 / Asia/Shanghai |

The source commit is the base revision, not the identity of a clean source
checkout. The source manifest and its fingerprint identify the exact R3 inputs
used by the captured executable.

## Original captures

| Scene | Paperwhite 6 | Scribe |
| --- | --- | --- |
| Account reading defaults | [PNG](raw/pw6/10-account-reading.png) | [PNG](raw/scribe/10-account-reading.png) |
| Reader settings, portrait | [PNG](raw/pw6/20-reader-settings.png) | [PNG](raw/scribe/20-reader-settings.png) |
| Reader settings, landscape | [PNG](raw/pw6/27-reader-settings-landscape.png) | [PNG](raw/scribe/27-reader-settings-landscape.png) |

Each accepted capture waited at least 1.5 seconds and required three consecutive
identical X11 samples. The sidecars retain the observed UI and reader state,
native dimensions, original capture time, executable identity, source identity,
and stability measurements.

## Verification scope

These are offline desktop software simulations at device-profile resolutions.
The capture environment reports JPEG acceleration unavailable with reason
`platform`, so the controls are disabled and show the preview-environment
explanation. The images document that unavailable state; they do not demonstrate
JPEG hardware decoding or physical-device support.

The preserved report records six successful XTest capture sessions. Pressing a
disabled switch left the page body and preferences unchanged; comparisons
excluded the status clock. Reader checks retained the same source page, reading
anchor, fit, zoom, rotation, and Y4 processing intent. All native-media worker
statistics remained zero, and every owned application process exited cleanly.

Separate policy cases seeded a requested-on preference before an ordinary cold
restart. Requested intent, the reading anchor, and Y4 state survived while the
effective media policy remained off. Additional forged account and reader enable
intents were rejected by the capability gate. Those scripted policy cases are
distinct from the six XTest screenshot sessions and are excluded from visual
capture acceptance. No native driver, hardware Y4 path, firmware emulator,
physical Kindle, or live account was exercised by this baseline.

## Evidence and visual review

- [capture-report.json](capture-report.json) is the original
  `native-media-settings-result.json`, copied without changing its bytes or its
  `visual_review_status: pending` field.
- [capture-run.json](capture-run.json) preserves the original outer run metadata.
- [source-inputs.sha256](source-inputs.sha256) is the unchanged R3 gate source
  manifest.
- [verification.json](verification.json) records the final root visual review
  separately from the original capture report.
- [manifest.json](manifest.json) binds all six captures and every frozen file
  except its own checksum.

Root reviewed all six original R2 images. The final remote SHA-256 comparison
confirmed that all six R3 PNGs are byte-identical to those reviewed images. Root
also directly inspected R3 `pw6 / 20-reader-settings` and
`scribe / 27-reader-settings-landscape`, and confirmed clear controls and
explanations with no new clipping or overlap. This is a review of the captured
implementation, not a new product specification or physical-device acceptance.

E-ink refresh, ghosting, native framebuffer compensation, frontlight appearance,
touch latency, actual JPEG/DMA transactions, and supported-device qualification
remain outside this desktop capture scope. Keep this directory immutable after
sealing and use a new dated directory for subsequent reviewed captures.
