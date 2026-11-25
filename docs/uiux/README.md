# UI/UX baselines

The [2026-11-25 design handoff walkthrough](baselines/2026-11-25-design-handoff/README.md)
records all 215 redesigned Paperwhite 6 and Scribe states. Its
[review supplement](baselines/2026-11-25-design-handoff-review/README.md) records
subsequent corrections and separates application walkthroughs, presentation
fixtures, and firmware-emulator results. The original captures remain unchanged.
The [Paperwhite 6 native geometry record](../verification/pw6-native-geometry-20261125/README.md)
explains why its native framebuffer is larger than the design captures.
The [Scribe physical smoke](../physical/handoff-scribe-20261125/README.md)
records a separate offline run of the redesigned UI, including sleep continuity.

The [2026-11-17 startup comparison](baselines/2026-11-17-startup/README.md)
preserves startup, loading, error, and unchanged final shelf pixels before and
after the startup optimization. Its automated desktop checks passed; human
visual review is pending. This supplement retains the reviewed baselines below
and does not establish physical-device support or startup performance.
An additional [assistant visual inspection](baselines/2026-11-17-startup/assistant-visual-review.md)
records six original candidate frames without changing the frozen export.

The reviewed [2026-11-16 software grayscale settings update](baselines/2026-11-16-spatial-dither/README.md)
records the Off/Bayer8/Floyd-Steinberg behavior before the November handoff
changed the layout. It preserves twenty-four original settings captures plus
four quiet Fast pages at physical raster resolution, with exact source and
executable identities and independent automated and human review records.

Use the reviewed [2026-11-16 persistent fast reading update](baselines/2026-11-16-fast-mode/README.md)
as the earlier Normal/Fast implementation record. It preserves twenty native-resolution
Paperwhite 6 and Scribe portrait/landscape captures, exact source/binary
identities, preference recovery, and disabled periodic-clearing checks. Its
images remain immutable; the software settings update records current controls
and actual physical-resolution Fast pages.

Use the reviewed [2026-11-15 JPEG settings update](baselines/2026-11-15-jpeg-acceleration/README.md)
as the earlier six-image JPEG-control record. Its captures remain immutable;
the newer fast-mode supplement records the subsequently added settings.
Other scenes retain the complete baseline below.

The complete frozen implementation baseline is
[2026-11-14 / v0.1.25](baselines/2026-11-14-v0.1.25/README.md).
It contains 51 untouched application screenshots for Paperwhite 6 and Scribe,
page indexes, overview sheets, four-fit comparisons, and an exported viewer.

[Open the standalone viewer](baselines/2026-11-14-v0.1.25/viewer.html) or use the
[device comparison](baselines/2026-11-14-v0.1.25/previews/device-comparison.png).
The viewer is a self-contained HTML file and can be opened from a checkout
without a server or a Codex conversation. Its 13 selected states use reduced
previews; the complete native-resolution images are under each baseline's
`raw/` directory.

These snapshots record the implemented UI at their respective dates. Use the
[current handoff](../DESIGN_SOURCE.md) for presentation and interaction changes;
use earlier baselines and [the prior refinement](../UI_REFINEMENT_20261113.md)
for historical comparison. Preserving a screenshot does not approve every
current behavior or establish physical-device compatibility.

## Use in subsequent iterations

1. Inspect the current baseline before changing a page. Use the stable scene ID
   and device profile in review notes, such as `scribe / 19-chapter-directory`.
2. Build and capture changed code on `test-env` with fresh isolated demo data.
   Match the source page and state, not only the route name.
3. Compare the native PNGs. Overview sheets and viewer previews are reduced
   derivatives and are unsuitable for exact pixel comparisons.
4. Preserve the existing baseline. Store a new reviewed snapshot under a new
   date/version directory and update this index. Keep the executable identity,
   source revision, capture scope, and state metadata together.

## Reproduce the current walkthrough

Use the replay, pointer and fixture runners listed in the
[November handoff walkthrough](baselines/2026-11-25-design-handoff/README.md).
The older pixel-coordinate runners describe their frozen layouts rather than
current controls; the handoff pointer runner covers the current reading settings.

## Reproduce the historical November 14 baseline

The historical runner is
[`capture_uiux_baseline.py`](../../scripts/verification/capture_uiux_baseline.py).
It uses the existing XTest pointer adapter and profile geometry, runs the real
Rust/Slint executable at scale factor 2, and requires a new output directory.
Each capture waits at least 1.5 seconds and then requires three identical X11
frames. Directory return also checks the retained reading anchor. Session
reports keep visual review pending until the images have been inspected.

After preparing and building a fresh remote source snapshot, replace these
example paths and the commit with that build's recorded identities:

```powershell
$remoteSource = '/root/<fresh-run>/app'
$remoteBinary = '/root/<fresh-run>/target/debug/bilicomics'
$remoteOutput = '/root/<fresh-run>/uiux-captures'
$sourceCommit = '<40-character-captured-source-commit>'
$binarySha = (ssh test-env "sha256sum '$remoteBinary'").Split(' ')[0]
ssh test-env "xvfb-run -a -s '-screen 0 2800x2800x24 -nolisten tcp' python3 '$remoteSource/scripts/verification/capture_uiux_baseline.py' --binary '$remoteBinary' --expected-sha256 '$binarySha' --source-commit '$sourceCommit' --output '$remoteOutput'"
```

Defaults capture both `pw6` and `scribe` across `navigation`, `reading`, and
`extras`. Use `--profiles scribe` or `--stages reading` for a focused capture.
The runner refuses a mismatched executable or an existing output directory.
It uses only bundled demonstration data and does not connect to a Kindle or
operate a live account. Screen coordinates describe this baseline; update them
deliberately when the UI changes and inspect the resulting images.

## Check a frozen baseline

The artifact-only integrity check verifies file hashes, the complete file
inventory, PNG dimensions, capture identities, and matching state sidecars.
It does not build or start the application. Run it on the remote snapshot:

```powershell
ssh test-env "python3 '$remoteSource/scripts/verification/verify_uiux_baseline.py' --baseline '$remoteSource/docs/uiux/baselines/2026-11-14-v0.1.25'"
```

Local tests, runtime probes, and rendering still require explicit authorization.
Frozen files bypass Git newline conversion so their recorded bytes survive
Windows and Linux checkouts.
