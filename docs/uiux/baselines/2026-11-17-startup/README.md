# Startup comparison: 2026-11-17

This supplement preserves the real desktop startup, loading, failure, and final
shelf pixels before and after the startup optimization. Automated checks passed;
**human visual review is pending**. Earlier reviewed UI baselines remain
unchanged. These captures do not establish physical-device support or startup
performance.

## Selected software

| Variant | Source commit | Source inventory SHA-256 | Desktop executable SHA-256 |
| --- | --- | --- | --- |
| Baseline | `06ad90b02edb28db43abeed3031932a91dc2f8df` | `e9eafe504fcd206aa46dcb257f60329d6d49be146d69915aedd8b031b36833a4` | `4285b97673f646b259962e4c01b04d130f01eb536f3924d5f90fd81634e910bf` |
| Candidate | `585e571b422e9586f1e53a417cbaf330a9ec473f` | `33bda43e34f74c7e73058880b4a1ae7a8d4b00d3ec0ae9a1ee22a074b59beb28` | `f9a6225638a24b1cb5f6031af6022905dc7b1325a5b37af616a261b234b9dd6c` |

The original [input selection](evidence/source/selected-inputs.json) retains the
source archive hashes. The 540 baseline and 542 candidate production inputs were
checked against their selected inventories before and after capture. Source
archives and executables are identified by hash rather than copied here.

The frozen runner is [desktop_verify-r2.py](harness/desktop_verify-r2.py), SHA-256
`172f6c80ed9e7e2df1a78da663e90471913a503fa612a2a9b098c1e5146e50c8`.
Its unchanged X11 capture dependency is also retained in [harness/](harness/).
The original [successful result](evidence/desktop-r2-results.json), SHA-256
`d326d46304a29d985f5794858ae6f82ebe72781ef234774229d9a8e47bbc05d0`,
binds every case to its executable, source inventory, source archive, and runner.

## Capture scope

All eight cases ran on `test-env`, each with a fresh Xvfb display, private data,
and a separate network namespace. The actual Rust/Slint program used `--demo
--offline --screen shelf`, a scale factor of two, and a controlled 1,800 ms state
worker delay. Only the bundled demonstration was opened. No authenticated
account, Kindle, or injected input was used.

| Profile | Logical viewport | Original PNG size | Cases |
| --- | --- | --- | --- |
| Paperwhite 6 | 632 x 840 | 1264 x 1680 | Baseline/candidate, normal/failure |
| Scribe | 930 x 1240 | 1860 x 2480 | Baseline/candidate, normal/failure |

The normal cases exited successfully after reaching the shelf. Each retained at
least 1.5 seconds of settling and three identical actual X11 samples. Controlled
failure cases supplied a regular file as the data-directory input; they exited
with status 1, never emitted readiness, and retained the real error view with
the activity indicator hidden. All eight private process groups closed with no
remaining live members.

The first startup snapshot preceded worker admission in every case. The next
snapshot overlapped preparation and changed the indicator's actual pixels.
Bootstrap character mapping and visible outlines cover all 52 non-whitespace
characters required by the startup localization. This does not replace visual
checks for legibility, missing characters, or clipping.

Ten comparisons were exactly equal in every opaque RGB pixel: the initial normal
and failure startup scenes, final shelf snapshots, stable actual X11 shelves,
and retained error views, for both profiles. The controlled delay and capture
work make these runs unsuitable as startup-performance measurements. No native
SEND, driver completion, waveform, or physical panel was observed.

## Preserved files

There are 28 main PNGs and 12 settling PNGs. Every PNG is an untouched byte copy
of its remote original. [export-manifest.json](export-manifest.json) maps each
exported file to its origin, byte count, and SHA-256.

- `raw/<profile>/<variant>/<scenario>/` contains `startup-0.png`,
  `startup-1.png`, and `final.png`, together with their original JSON sidecars.
- The four normal cases also contain `final-x11.png` and `settle/000.png`
  through `settle/002.png`. Their original metadata is in the corresponding
  case result; `final-x11.json` is explicitly marked as an export projection.
- `evidence/cases/<original-case-id>/` preserves the original result, outer
  process identity, and cleanup receipts. Remote paths and sample-relative
  filenames remain unchanged in those original records.
- `evidence/source/` preserves the selected source inventories and input
  selection. [capture-index.json](capture-index.json) locates the exported
  images and their metadata.

Application data directories, generated demo assets, SQLite databases, private
case configuration files, ownership nonces, and binary packages are excluded.

## Earlier runner failure

The first desktop runner batch failed a host `/proc` ownership assertion while
collecting case results. Five cases produced application evidence, but the
batch did not complete and is not an accepted comparison. Its original failure
receipt and log are retained under [evidence/desktop-r1/](evidence/desktop-r1/).
The corresponding runner source is preserved in [harness/](harness/).

The second runner added bounded retries for process visibility and exit races,
while retaining process birth, private group, case marker, and pidfd checks.
It also records cleanup failures and gathers all case outcomes. No production
source, executable, or prior batch record changed. The repeated batch used new
case identities and a fresh output directory.

## Visual review

Status: **pending**. Review the original candidate startup, stable shelf, and
error PNGs for both profiles. Confirm complete Chinese text, comfortable spacing,
unclipped controls, complete covers, and unchanged final shelf content. Do not
use reduced previews for the pixel comparison or treat this supplement as
physical-device acceptance.
