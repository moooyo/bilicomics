# UI/UX baseline: 2026-11-14 / v0.1.25

This is the original implementation snapshot requested for subsequent UI/UX
iteration. The 51 PNGs under `raw/` retain the exact captured bytes. The
matching JSON sidecars retain the observed application state and capture
identity. All displayed accounts, balances, covers, comics, purchases, and QR
codes belong to the bundled fictional demonstration fixtures.

| Identity | Value |
| --- | --- |
| Application | BiliComics 0.1.25 |
| Captured source | `91d57c7ba9c294da3c45c779ad39347bb00c1e5e` (clean checkout) |
| Executable SHA-256 | `9fc0e395f879f2758eda41f924e5aec6888c796aa15c500f7da68833c50738f5` |
| Capture run | `kindle-uiux-20261114-cf02f69e` |
| Capture host | `test-env` |
| Renderer | Slint winit software renderer, scale factor 2 |
| Input | Actual XTest pointer input; no injected named UI actions |
| Paperwhite 6 | `pw6`, 1264 x 1680, 25 captures |
| Scribe | `scribe` (2022 / 2024 profile), 1860 x 2480, 26 captures |
| Landscape | Width and height are swapped |
| User date / timezone | 2026-11-14 / Asia/Shanghai |

The Scribe-only system-settings tab accounts for the capture-count difference.
These are desktop software simulations at device-profile resolutions, not
physical-device photographs or firmware-emulator acceptance. E-ink ghosting,
native framebuffer compensation, frontlight appearance, touch latency, and
panel refresh behavior are outside this baseline's scope.

## Browse

- [Standalone interactive viewer](viewer.html): 13 key states, device comparison,
  and individual-device views. All preview images and runtime resources are
  embedded. Open the file directly from the checkout.
- [Original viewer fragment](viewer.fragment.html): the unchanged editable
  source of the in-conversation preview. Chinese labels are also retained in
  [labels.zh-CN.json](labels.zh-CN.json).
- [Device comparison](previews/device-comparison.png).
- [Paperwhite 6 overview](previews/pw6-overview.png) and
  [Scribe overview](previews/scribe-overview.png).
- [Paperwhite 6 four-fit comparison](previews/pw6-reading-fits.png) and
  [Scribe four-fit comparison](previews/scribe-reading-fits.png).

The overview sheets and viewer use proportional reductions. Use the native
PNGs below for close inspection and future pixel comparisons. Labels are
outside the screenshots; no application pixels were retouched or reconstructed.

## Complete scene index

| ID | State | Paperwhite 6 | Scribe |
| --- | --- | --- | --- |
| 01 | Bookshelf | [PNG](raw/pw6/01-shelf.png) | [PNG](raw/scribe/01-shelf.png) |
| 02 | Bookshelf filter | [PNG](raw/pw6/02-shelf-filter.png) | [PNG](raw/scribe/02-shelf-filter.png) |
| 03 | Bookstore | [PNG](raw/pw6/03-store.png) | [PNG](raw/scribe/03-store.png) |
| 04 | Chinese search keyboard | [PNG](raw/pw6/04-search-keyboard.png) | [PNG](raw/scribe/04-search-keyboard.png) |
| 05 | Search results | [PNG](raw/pw6/05-search-results.png) | [PNG](raw/scribe/05-search-results.png) |
| 06 | Comic details | [PNG](raw/pw6/06-detail.png) | [PNG](raw/scribe/06-detail.png) |
| 07 | Downloads | [PNG](raw/pw6/07-downloads.png) | [PNG](raw/scribe/07-downloads.png) |
| 08 | Download selection | [PNG](raw/pw6/08-download-selection.png) | [PNG](raw/scribe/08-download-selection.png) |
| 09 | Account home | [PNG](raw/pw6/09-account.png) | [PNG](raw/scribe/09-account.png) |
| 10 | Default reading settings | [PNG](raw/pw6/10-account-reading.png) | [PNG](raw/scribe/10-account-reading.png) |
| 11 | Storage settings | [PNG](raw/pw6/11-account-storage.png) | [PNG](raw/scribe/11-account-storage.png) |
| 12 | Download and network settings | [PNG](raw/pw6/12-account-download.png) | [PNG](raw/scribe/12-account-download.png) |
| 13 | System-settings tab | Not exposed on this profile | [PNG](raw/scribe/13-account-system.png) |
| 14 | Reader: page fit | [PNG](raw/pw6/14-reader-page.png) | [PNG](raw/scribe/14-reader-page.png) |
| 15 | Reader controls | [PNG](raw/pw6/15-reader-controls.png) | [PNG](raw/scribe/15-reader-controls.png) |
| 16 | Reader: original fit | [PNG](raw/pw6/16-reader-original.png) | [PNG](raw/scribe/16-reader-original.png) |
| 17 | Reader: height fit | [PNG](raw/pw6/17-reader-height.png) | [PNG](raw/scribe/17-reader-height.png) |
| 18 | Reader: width fit | [PNG](raw/pw6/18-reader-width.png) | [PNG](raw/scribe/18-reader-width.png) |
| 19 | Full-screen chapter directory | [PNG](raw/pw6/19-chapter-directory.png) | [PNG](raw/scribe/19-chapter-directory.png) |
| 20 | Per-comic reading settings | [PNG](raw/pw6/20-reader-settings.png) | [PNG](raw/scribe/20-reader-settings.png) |
| 21 | Frontlight and warmth | [PNG](raw/pw6/21-reader-light.png) | [PNG](raw/scribe/21-reader-light.png) |
| 22 | Onboarding | [PNG](raw/pw6/22-welcome.png) | [PNG](raw/scribe/22-welcome.png) |
| 23 | QR login | [PNG](raw/pw6/23-login.png) | [PNG](raw/scribe/23-login.png) |
| 24 | Purchase confirmation fixture | [PNG](raw/pw6/24-purchase.png) | [PNG](raw/scribe/24-purchase.png) |
| 25 | Landscape reader | [PNG](raw/pw6/25-reader-landscape.png) | [PNG](raw/scribe/25-reader-landscape.png) |
| 26 | Offline bookstore | [PNG](raw/pw6/26-store-offline.png) | [PNG](raw/scribe/26-store-offline.png) |

## Evidence and reuse

[manifest.json](manifest.json) records the source revision, executable identity,
native image hashes, capture times, stability observations, and frozen file
inventory. [verification.json](verification.json) summarizes the original
controlled capture and preview checks without storing runtime databases,
session directories, build caches, or private host configuration.

The accepted capture take waited at least 1.5 seconds and required three
consecutive identical X11 frames. Initial fixed-delay captures were excluded
after visual inspection found delayed Scribe painting. Both directory-return
checks preserved comic `28601`, episode `2860112`, and page index 17 (page 18 of
45). The four fits were selected through the real reading controls. `original`
and `height` happen to match for this tall fixture; that is an observed result,
not a replacement definition of either fit.

The exported viewer was checked on `test-env` for all 13 choices, both device
focus modes, a narrow viewport, dark appearance, and restored selection.

Use the [baseline workflow](../../README.md) and its reusable remote capture and
integrity commands for subsequent changes. Preserve this directory as the
dated implementation record; the original v2 design and later user approvals
remain the design requirements.
