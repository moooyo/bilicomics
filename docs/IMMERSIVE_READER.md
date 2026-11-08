# Immersive Reader Update

Updated on 2026-11-05 for development version 0.1.4.

The user's subsequent full-page reading request supersedes the older design's
standing reader margins and badges. The established monochrome components,
navigation conventions, source modes, and account boundaries remain in use.

## Reading surface and contextual controls

Normal whole-page reading uses the entire window as its raster and image area.
`FitMode::Page` preserves the complete source and its aspect ratio; letterboxing
caused by a different image ratio is image layout rather than reserved UI space.
An explicitly selected width fit, panel region, or strip viewport retains its
corresponding reading behavior.

Quiet reading has no standing chapter footer, page-number band, panel border or
locator, strip overlap rule, or spread-leaf annotation. A cover's unused spread
leaf is plain paper. Center tapping reveals the header and bottom progress/tool
controls as solid, legible overlays. Progress, source mode, chapter/title/cache
information, time, connectivity, and battery are available there. Layout and
cover options are grouped in reading settings, and page overview remains an
explicitly opened sheet.

Opening and closing the menu leaves the reading raster, viewport and source
anchor unchanged. Backdrop tapping dismisses controls without turning the page;
navigation, beginning, seeking and source-page selection return to quiet reading.
Actionable sheets stay open while the user is choosing. E Ink interactions use
immediate state changes rather than additional menu animation or timed dismissal.
Loading, error recovery, first-use guidance and chapter-end actions retain their
own contextual presentation.

The UX review used the skill's static quick-reference entries for content
priority, progressive disclosure, layout stability, touch spacing, and native
control labels. Slint has no stack-specific entry in that skill. The existing
Kindle tokens and components take precedence over generic phone/web examples.

## Rendering and compatibility

The controller converts the complete logical window to output pixels. Eligible
landscape spreads divide that width into two full-height leaves. Content layout
uses the core's authoritative spread state, keeping cover placement consistent
when a desktop preview window changes size. A single-page layout no longer
implicitly overrides the user's selected fit mode.

Panel frames retain the renderer's complete canvas instead of trimming an
assumed centered letterbox. This preserves short, top-aligned width-fit panels.
The legacy footer preference remains readable and serializable, but cannot
restore quiet-state chrome or trigger an otherwise unchanged raster/layout.
Source-relative anchors, strip overlap, panel order, cover behavior and pixel
budgets remain separate from the presentation of controls.

Thumbnail and chapter rows retain a stable Slint model throughout a reading
session. Refreshes update only changed rows, with additions and removals limited
to the changed tail. Image readiness and unrelated clock updates therefore keep
the existing row and its pointer press alive instead of resetting a repeater.

## Verification identity

Verification runs on `ssh test-env`; no local application build, test, or runtime
probe is authorized. The focused harness uses generated source images in a real
offline guest cache, real `ReaderCore` rendering, and actual XTest pointer events.
Whole-window RGB comparisons test the image content and natural letterbox.
Menu checks compare the unoccluded artwork, source anchor, raster dimensions and
render version. Persistent legacy `footer=true` data is deliberately exercised.
Optional input cases press an actual loading thumbnail and hold a press across
the real thumbnail worker's completion. They use generated higher-resolution
source pages to create work, without a production delay or an injected action.

The final R5 application manifest contains 340 inputs and has SHA-256
`4dcf953fa4786f842a8f58faad42e01fb57ed8a3bc023252265b1a71e73c3d91`.
Its sealed desktop executable has SHA-256
`2d607224cec16a346bab9c359d67b0ab8e96624c85661feb5b57fadd4b80c578`.
All checks below ran remotely against that identity:

| Check | Result | Evidence |
| --- | --- | --- |
| Locked offline check, library tests, desktop build | Passed; 231 tests, zero failed or ignored | [Core and reader suite](../artifacts/verification/immersive-reader-20261105/r5/evidence/20261104T185853Z-3dfbd43f/summary.json) |
| Existing reader controls and boundaries | Nine cases passed | Same suite |
| Offline guest guidance, loading and recovery | Five cases passed | Same suite |
| Focused portrait and landscape overview selection | Two cases passed | [Focused run](../artifacts/verification/immersive-reader-20261105/r5/immersive-focused/immersive-reader-result.json) |
| Full-window page, panel, strip, spread and loading input | All 16 cases passed | [Immersive run](../artifacts/verification/immersive-reader-20261105/r5/immersive-full/immersive-reader-result.json) |
| Startup feedback and retained live Reader | Two startup cases and the real-process lifecycle fixture passed | [Lifecycle regression](../artifacts/verification/immersive-reader-20261105/r5/lifecycle-regression/summary.json) |

The 16 immersive cases comprise Paperwhite 5 and Scribe in portrait and
landscape across three source modes, both Scribe spread directions, and two
loading-input sequences. The held-press case observed five real thumbnails
becoming ready while XTest still held the original current-page target, then
released successfully. No input coordinate or image/anchor assertion was relaxed
to obtain the passing result. The lifecycle fixture uses controlled Linux
services and a real application process, not native Kindle sleep.

Selected final screenshots received a separate [visual review](../artifacts/verification/immersive-reader-20261105/visual-review.json).
Raw automated reports retain their original visual-review fields. The main
fictional comic fixture also provides a [quiet reading preview](../artifacts/verification/immersive-reader-20261105/r5/evidence/20261104T185853Z-3dfbd43f/reader/scribe-portrait-beginning-closes-menu/beginning-with-menu-open.png)
and the same page with [contextual controls](../artifacts/verification/immersive-reader-20261105/r5/evidence/20261104T185853Z-3dfbd43f/reader/scribe-portrait-beginning-closes-menu/menu-before-beginning.png).

The fresh [0.1.4 ARM development archive](../artifacts/verification/immersive-reader-20261105/arm-r5/bilicomics-kindle-0.1.4-dev.zip)
is 49,958,399 bytes, SHA-256
`cf6c04bee9fcf9c60acb5cf76b6a7cdc10ab9bb5c57f8597558fc3113e773d94`.
Its 64,661,416-byte ARM executable has SHA-256
`684ba4221830a24199d4c9f475383c8e16bc25c062e5542d17d6f1b36f08d671`.
The [candidate record](../artifacts/verification/immersive-reader-20261105/arm-r5/candidate-summary.json)
binds both to the same R5 source. All 22 allowed archive entries and static ARM
EABI5 hard-float checks passed, including absence of `INTERP` and `NEEDED`.
Two cc/cargo-zigbuild wrapper warnings remain in the original build log; this
is not a warning-free ARM build. That static build stage did not run the ARM
executable. The subsequent supervised physical result is recorded separately
below, and the release acceptance allowlist remains empty.

Original failed compile, focused input, and empty-image equality regression runs
are retained rather than overwritten. Slint 1.17.1 compares two default empty
images as unequal; thumbnail comparison now treats only two empty `0x0` images
as equivalent after checking all row metadata. Loaded images retain identity
comparison, including a regression for same-size replacement pixel buffers.

## Subsequent supervised physical check

After a fresh exclusive handoff from WeRead, the same 0.1.4 ARM candidate received
[scoped physical verification](PHYSICAL_IMMERSIVE_VERIFICATION.md) on one
first-generation Scribe running 5.19.5. The operator confirmed startup feedback,
complete artwork without standing badges, contextual information and controls,
physical power-button sleep/wake retaining the comic, and the first subsequent
finger page turn from page 18 to 19. The live process birth/executable identity,
all reader identity and source-anchor fields, full 1860-by-2480 geometry and
before/after framebuffer pixels support the retained-page result. Opening the
menu kept the raster and render version unchanged; unoccluded artwork pixels
also matched.

The final sleep run read persisted suspend/resume events after the operator
woke and unlocked the device. It sent no Kindle commands during the confirmed
screensaver hold. An earlier live-suspended polling checkpoint was not obtained
and remains recorded with its successful supervisor recovery; its cause is not
assigned to the application or SSH. An earlier reader run ended after the
operator's explicitly confirmed navigation and intentional exit.

Native Home and touch were restored, all 32 tracked process identities were
absent, coordinator SSH connections were closed, and the owner/token-checked
lease was released at 23:24:42 on 2026-11-05 in Asia/Shanghai. WeRead explicitly
acknowledged the return. These checks used portrait, left-to-right, whole-page
fit, the resume startup preference, and isolated offline demonstration data.
Real accounts, downloads, transactions, long sleep, other configurations and
other devices retain separate acceptance. The [earlier 0.1.3 physical report](PHYSICAL_VERIFICATION.md)
keeps its original source and executable identity.

The subsequent [0.1.5 completeness change](DOWNLOAD_COMPLETENESS.md) shares a
bounded strict decoding policy between acquisition and reading. Its source and
candidate identity differ from the 0.1.4 physical run above; that physical result
is retained with its original scope.
