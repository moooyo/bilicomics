# November 25 UI handoff walkthrough

## Reference and verification scope

This walkthrough implements the user-supplied November handoff described in
[Design Source](../../../DESIGN_SOURCE.md). The reference has 215 state images
across Paperwhite 6 and Scribe. The screenshots are evidence of the working-tree
implementation based on `32380fc`, not of a published release or a clean checkout.
The implementation preserves the earlier frozen baselines unchanged. The
[review supplement](../2026-11-25-design-handoff-review/README.md) records later
corrections; the captures and sidecars below still describe the original run.

| Profile | Logical viewport | Captured pixels |
| --- | --- | --- |
| Paperwhite 6 | 632 × 840 | 1264 × 1680 |
| Kindle Scribe | 930 × 1240 | 1860 × 2480 |

Three capture scopes are kept separate:

- **Application intents:** the actual Linux Rust/Slint application runs under a
  private Xvfb display on `test-env`. The runner drives the production
  controllers with the bundled demo and uses private data for every scene.
- **Pointer input:** the same application is driven through XTest, with no
  directed actions. Logical hit points are checked against the observed viewport.
- **Presentation fixtures:** Slint's same software renderer receives synthetic
  backend observations for states such as QR confirmation, a pending purchase,
  recharge records, the full sleep overlay, and JPEG capability. This checks the
  layouts, not that a service, codec, or panel reported those observations.

No live credentials, account data, real transactions, native panel photographs,
framebuffer refresh measurements, or device driver behavior are inferred from
these images. Runtime and compilation work stayed on `test-env`.

## Results and original captures

[Scene index](scene-index.json) and [verification summary](verification.json):
111 Paperwhite 6 and 104 Scribe captures,
all at the same dimensions as their paired reference. The index identifies
151 application-replay images, 10 pointer images and 54 presentation-fixture
images. The earlier 2026-11-14 baseline is unchanged. A representative page from
each family:

| Surface | Paperwhite 6 | Scribe |
| --- | --- | --- |
| Shelf | [PNG](raw/pw6/shelf.png) | [PNG](raw/scribe/shelf.png) |
| Store | [PNG](raw/pw6/store.png) | [PNG](raw/scribe/store.png) |
| Search results | [PNG](raw/pw6/search-results.png) | [PNG](raw/scribe/search-results.png) |
| Details | [PNG](raw/pw6/detail.png) | [PNG](raw/scribe/detail.png) |
| Reader | [Settings](raw/pw6/reader-layout.png) | [Settings](raw/scribe/reader-settings.png) |
| Purchase | [PNG](raw/pw6/purchase.png) | [PNG](raw/scribe/purchase.png) |
| Downloads | [PNG](raw/pw6/downloads.png) | [PNG](raw/scribe/downloads.png) |
| Account | [PNG](raw/pw6/account.png) | [PNG](raw/scribe/account.png) |
| Onboarding | [PNG](raw/pw6/welcome.png) | [PNG](raw/scribe/welcome.png) |
| Startup / sleep | [Startup](raw/pw6/startup.png) / [Sleep](raw/pw6/sleep.png) | [Startup](raw/scribe/startup.png) / [Sleep](raw/scribe/sleep.png) |

The two-profile XTest run checks eight flows and saves 24 observations. It
exercises returning from the directory on page 18, opening reading and frontlight
settings, blocking a paused setting, applying an account picker option, closing
popovers, opening a download sheet, selecting a book, switching a purchase from
one to five episodes, and searching from history. Replay separately verifies
Search return, purchase-to-directory return, and cancellation with completed
chapters retained. The two stale network fixture selections were replayed with
an actually uncached comic; all four corrected network checks passed.

The library suite passed **901** tests, with **7** opt-in tests ignored. The
production-flow (2), download batching (1), and sleep UI (1) integration tests
passed. The five native signing integration tests remained ignored because they
require a separately pinned signing asset. The exceptional presentation test
passed on both profiles. The successful library rerun used a private umask,
serial execution and the checked-in dither fixtures; the first run omitted
those requirements and was not accepted.

The recorded Xvfb candidate was `/work/target/debug/bilicomics` in run
`bilicomics-ui-handoff-20261125.Yy6SLU`, with 257,084,248 executable bytes.
The sidecars preserve the capture scope, route and profile metadata. No image
was normalized, retouched or rescaled for this baseline.

## Review decisions

- Place the Paperwhite Shelf resume banner above the inline pager; hide the
  banner on non-All filters per the written handoff. Scribe keeps the wide card.
  The number and order of books, last-sync times and network status come from the
  fixtures or controllers rather than being fixed for the comparison.
- Reuse one Chapter Book layout for Details and Directory. Directory filters,
  scrolling and returning preserve the live reader position. Successful payment
  still requires a verified observation, and a paid chapter does not unlock in
  the demo controller.
- Preserve the rendered reader canvas and rendering preferences independently of
  the new menu, frontlight and settings overlays. Hardware-dependent choices
  show an unavailable state when the current backend does not expose them.
- Keep downloads grouped by book. The list and sheet do not expose a cropped
  underlying book row; recorded retained and automatic-cache usage use the same
  storage data. Canceling queued work does not erase a completed chapter.
- Do not claim that a software update was checked when no update source exists.
  Do not add a system-restore toggle: native restoration is unconditional.
- Screens read different fixture values from the prototypes where appropriate.
  Typography, antialiasing, source comics/covers, timestamps and QR payloads are
  not an assertion of literal pixel equality. Real errors and account-specific
  constraints may present a different message or disable a control.

## Reproduction

Use a fresh isolated `test-env` workspace with Rust, Xvfb, libXtst, and Pillow.
Build `bilicomics` with `cargo build --locked --offline --bin bilicomics -j1`.
From the workspace source, use the included runners with fresh output paths:

- [State replay](../../../../scripts/verification/handoff_scenes.py) accepts
  `--binary`, `--base`, `--output`, and an optional `--only` list.
- [Pointer walkthrough](../../../../scripts/verification/handoff_pointer.py)
  accepts `--binary` and `--output`; start it in a private Xvfb screen at least
  1860 × 2480. It exercises navigation, setting changes, and modal dismissal.
- [Presentation fixtures](../../../../tests/handoff_visual_states.rs) run with
  `BILICOMICS_HANDOFF_FIXTURES` set to a new directory and
  `cargo test --locked --offline -j1 --test handoff_visual_states -- --ignored`.
- [Comparison and export](../../../../scripts/verification/handoff_review.py)
  takes the PNG-only reference directory and the raw capture directories in
  preference order. `--baseline` copies untouched originals and their sidecars.
  The smaller side-by-side review sheets are derivatives, not the originals.
