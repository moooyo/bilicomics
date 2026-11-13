# Header and reader refinement

The user approved this refinement on 2026-11-13 after reviewing Scribe and
Paperwhite proposal drawings. The initial implementation is development version
0.1.24; version 0.1.25 applies the user's fit clarification from 2026-11-14.
The supplied v2 design remains the reference for shared components and grayscale
styling; these approved interaction changes supersede its affected screens.

## Primary pages

The bottom navigation identifies the four primary pages. Their content headers
now expose actions or useful state instead of repeating those page names.

- Bookshelf filtering belongs to its content controls. Pagination is below the
  book grid, with the grid clipped above the footer. The continuation entry is
  retained on both device profiles.
- The bookstore search field spans the content width. Its entire visible width
  remains an input target, with an explicit accessibility label.
- Downloads separate active, paused, and failed task state from storage totals.
  Pause/resume and selection actions retain their original semantics. Selection
  mode keeps its task-specific heading.
- Account home begins with profile and wallet information. Application exit is
  a borderless X with a 56/48 logical-pixel hit area and a descriptive
  accessibility label. Sign-out remains a separate action. Secondary account
  pages keep their return navigation and specific page titles.

## Reading controls

Text and search fields retain rectangular borders. Two-state switch styling
uses rectangular tracks and square thumbs; this does not alter range-slider
thumbs or the device-shell illustration.

The user confirmed the Height/Width overflow rule explicitly during supervised
testing: the selected axis remains filled, and the other axis is cropped
symmetrically when necessary. Whitespace is conditional, not automatic for
every aspect-ratio mismatch. On a 1860 by 2480 viewport, the 3400 by 2360 wide
fixture is approximately 3573 by 2480 before Height cropping; Width instead
draws approximately 1860 by 1291 with centered top/bottom whitespace.

The expanded reader has a single book/chapter heading, without connection,
prefetch, or mode/fit captions. Its bottom controls are progress, named fit
choices, and four tools: chapters, reading settings, full refresh, and manual
frontlight controls. The control panel remains 208 logical pixels tall on
Scribe and is 184 pixels tall on Paperwhite.

The four named fits use the user's clarified geometry. `page` intentionally
stretches independently on the two axes; the other three preserve aspect ratio:

| Stored key | Behavior |
| --- | --- |
| `original` | Keep native pixels for a small source; otherwise uniformly reduce by `min(1, viewport_width / source_width, viewport_height / source_height)`. Center the complete image without overflow. |
| `page` | Independently resize width and height to fill the complete viewport. Keep the stored key for preference compatibility. |
| `height` | Uniformly scale to viewport height and center; horizontally overflowing content is cropped symmetrically. |
| `width` | Uniformly scale to viewport width and center; vertically overflowing content is cropped symmetrically. |

Page reading uses one centered view for each source page. Its fit does not add
hidden overflow-page steps. Existing panel maps and strip-content segmentation
remain separate reading-mode behavior; the active region receives the selected
fit. Named fits reset an accumulated zoom multiplier rather than applying a
second hidden scale. Per-comic and default fit preferences are persisted.
Switching fit retains the current source page; sleep reconciliation retains the
presented reading anchor. Oversized centered fits use bounded source sampling
rather than allocating an unbounded enlarged image.

The page-overview UI and its action entries are retired, including the former
accessibility fallback from progress to the thumbnail grid. Retired or unknown
sheet intents cannot start thumbnail jobs. The small internal overview used by
panel reading is independent of that retired feature and remains available.
Progress is a proper slider with pointer, keyboard, and accessibility actions.

## Complete chapter directory

The chapter tool opens an opaque full-window directory inside the retained
reader session. It does not open an intermediate half-height sheet or close
the live reading core. The directory supports all, unread, readable, and
downloaded filters; ascending/descending order; current-chapter location; and
pagination. It opens on the page containing the current chapter.

The portrait layouts show 14 rows on Scribe and 10 on Paperwhite 6. Capacity is
recomputed from logical window geometry for other sizes. Only the visible
chapter rows are projected, using the existing catalog permission and cached
local-inventory evidence. Source metadata alone does not establish a verified
download. Selecting the current chapter closes the directory and retains the
same frame, reader generation, and source page. Returning to reading also keeps
the original source anchor.

## Verification scope

The results below describe the initial frozen implementation. The subsequent
supervised device observation and presentation follow-up are documented in
[Reader geometry presentation](READER_GEOMETRY_PRESENTATION.md), with independent
source and run identities.

All execution and verification use `test-env`. No local build or runtime probe,
physical Kindle operation, live account, or upstream purchase is used.

The frozen source passes 728 application library tests, with three existing
ignored tests, the sleep-screen integration test, the desktop build, and the
canonical optimized ARM build with the declared Cargo release profile. The
desktop executable passes four actual XTest journeys, totaling 94 clicks:
primary pages and reader controls on each of Scribe and Paperwhite 6. These
exercise the four fits and their stored values,
all 32 fixture chapters through pagination, filters and sorting, current-page
return, dynamic download controls, and application exit through the X.

Three supplemental journeys pass in an isolated network namespace: fresh
unsigned-in accounts on both profiles, plus a Scribe height-fit sleep/wake
cycle. They verify X exit for guests, the same reader process and source
geometry after sleep, identical quiet-reading pixels, and the first post-wake
page turn. High-DPI runtime and complete process-restart behavior are not
claimed by these supplemental journeys.

Builds and UI evidence retain separate source, pipeline, executable, and harness
identities. The UI harness additionally records which screenshots require
visual review. Desktop fixture results and ARM build results do not establish
firmware-emulator or physical-device acceptance.

Evidence is retained under the ignored
`artifacts/uiux/20261113-implementation/` directory. The
[actual runtime gallery](../artifacts/uiux/20261113-implementation/actual-ui/index.html)
contains 14 unchanged screenshots from the compiled application. Early ARM
attempts exceeded private temporary-cache or shared-memory limits. The final
successful ARM build uses a task-owned disk cache and the unchanged release
profile. Every attempt retains its separate identity; failed results are never
reassigned to the final executable.
