# UI/UX baselines

The current frozen implementation baseline is
[2026-11-14 / v0.1.25](baselines/2026-11-14-v0.1.25/README.md).
It contains 51 untouched application screenshots for Paperwhite 6 and Scribe,
page indexes, overview sheets, four-fit comparisons, and an exported viewer.

[Open the standalone viewer](baselines/2026-11-14-v0.1.25/viewer.html) or use the
[device comparison](baselines/2026-11-14-v0.1.25/previews/device-comparison.png).
The viewer is a self-contained HTML file and can be opened from a checkout
without a server or a Codex conversation. Its 13 selected states use reduced
previews; the complete native-resolution images are under each baseline's
`raw/` directory.

These snapshots record the implemented UI. The
[downloaded v2 design](../DESIGN_SOURCE.md) and subsequently approved
[header and reader changes](../UI_REFINEMENT_20261113.md) remain the design
requirements. Preserving a screenshot does not approve every current behavior
or establish physical-device compatibility.

## Use in subsequent iterations

1. Inspect the current baseline before changing a page. Use the stable scene ID
   and device profile in review notes, such as `scribe / 19-chapter-directory`.
2. Build and capture changed code on `test-env` with fresh isolated demo data.
   Match the source page and state, not only the route name.
3. Compare the native PNGs. Overview sheets and viewer previews are reduced
   derivatives and are unsuitable for exact pixel comparisons.
4. Preserve the existing baseline. Store a new reviewed snapshot under a new
   date/version directory and update this index. Keep the executable identity,
   source revision, capture scope, image hashes, and state metadata together.

## Capture the same scenes

The reusable runner is
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
