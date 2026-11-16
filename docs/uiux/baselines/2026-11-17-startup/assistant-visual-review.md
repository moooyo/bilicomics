# Assistant visual inspection: 2026-11-17

The primary Codex assistant inspected the six original candidate PNGs below
after the frozen desktop export. This is assistant visual review, not human
acceptance or a physical-device observation. The original export manifest and
its capture-time review status remain unchanged.

| Profile and scene | Original PNG | SHA-256 from the remote export |
| --- | --- | --- |
| PW6 startup | [PNG](raw/pw6/candidate/normal/startup-0.png) | `a7b9184a5fe4c369929bf0e2f0e3d918ba43179486c33a5b868b586cbdab0264` |
| PW6 stable shelf | [PNG](raw/pw6/candidate/normal/final-x11.png) | `cd41f55862c02675cec6816f6c5a0af4ad697d486ef501165e1f879fdc0e1020` |
| PW6 startup failure | [PNG](raw/pw6/candidate/failure/final.png) | `003291f392fd4597eacd12316938956e3a867878e76b0c758e324e770475c48a` |
| Scribe startup | [PNG](raw/scribe/candidate/normal/startup-0.png) | `3b5e007eac0e4a5a30b77920cb3f8497fbfe61e55650345a5929cf3c101b4d1d` |
| Scribe stable shelf | [PNG](raw/scribe/candidate/normal/final-x11.png) | `d05e029ed941bae55951c1d28b597a9cf6b9239bf76c10a7195919b9923e6dc9` |
| Scribe startup failure | [PNG](raw/scribe/candidate/failure/final.png) | `c17b3072ab8a6e9601576edad8c2c95f2b669bf73c77bb86f2236a8a47e6ac23` |

The visible Chinese startup and error messages are complete and legible, with
no missing-glyph boxes or clipped lines. Startup keeps the bounded four-cell
activity indicator. Both stable shelves retain readable labels, complete
controls, and the expected fictional cover content or offline placeholders.
The error pages retain their complete explanation and rectangular exit button,
and the loading indicator is absent. No visual blocker was found in these six
frames.

The independent automated record establishes ten exact RGB comparisons and
the actual indicator pixel change. This inspection does not extend those
assertions to uninspected frames, optical appearance, touch behavior, or panel
refresh completion. Human acceptance is not claimed.
