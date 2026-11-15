# JPEG acceleration verification: 2026-11-15

The off-by-default BiliComics JPEG decode/downscale integration passed the
remote checks below. These are build, fixture, simulated-driver and desktop
results on `test-env`. No physical Kindle, live account, actual JDEC transaction
or BiliComics hardware performance measurement was used.

See the [implementation contract](KINDLE_MEDIA_ACCELERATION.md) and the
[separately attributed Aidoku evidence](KINDLE_MEDIA_ACCELERATION_AIDOKU_20261115.md).

## Candidate identity

The candidate is based on `2da87ab512fded000174b7542767082a3a94e147`
with uncommitted changes. The base commit alone does not reproduce it.
The build manifest covers 514 inputs, including the two C inputs under
`scripts/`. Source files remained unchanged throughout the final build/checks.

| Artifact | SHA-256 |
| --- | --- |
| Build source manifest | `f3e47bc467922a429c291cbd62f317aa912c4e14bdc8e60b85ba4ea369e99a55` |
| Desktop executable used for final Reader/UI checks | `0656bb79039344b3b59fd4815ec3ed03125df5d0b69ba27ed99aa5d70dda4e5c` |
| ARM application | `9b76cda1e3240fd9213f86107f946cbd4bf6e78ccddad60def0bc50a706d75ab` |
| ARM JPEG worker | `855c04a48a1a9cf84664cecac164c81c9ee61efb66bf775df8d9e6a6a80ba4d7` |
| Development ZIP | `879069dacaf230fab6607f9b1dbe6a4dfca23a08b978b7ff82eba8a02345b2c3` |

The development bundle and build evidence are retained under
[`artifacts/verification/native-media-20261115-r3/`](../artifacts/verification/native-media-20261115-r3/).
The bundle is a development candidate, not an accepted device installation or
published release. The runtime device allowlist remains empty.

## Executed checks

| Check | Result and scope |
| --- | --- |
| Default-feature library tests | 769 passed, three existing ignored |
| Kindle-feature library tests | 784 passed, three existing ignored; host tests, not ARM execution |
| Main binary unit tests | Eight passed |
| Closed production-flow subprocess tests | Two passed, with synthetic images and transport |
| C transaction contracts | Five methods, 52 simulated scenarios; no real device operations |
| Invalid-package contracts | Three passed: absent helper, non-executable helper, and replaced helper with a recomputed manifest |
| Reader pointer matrix | Nine cases passed on Paperwhite 5 and Scribe desktop profiles |
| Media settings UI | Six screenshots and six policy cases passed on Paperwhite 6 and Scribe profiles |
| ARM application and worker | Built with locked, offline dependencies and bounded Cargo concurrency |
| Development archive | 36 entries and 14 static checks passed, including both ELF identities, linkage, modes and notices |

Library fixtures cover strict JPEG admission, original-dimension sealed
proofs, receipt rechecks, the four fit definitions, cache isolation, policy
changes, prepared-frame invalidation, reading anchors, Y4, exact output length
and cancellation. Admission rejects trailing data, invalid alignment padding,
unsupported markers, excessive segments and inconsistent Adobe/XMP or color
metadata without CPU IDCT.

Worker fixtures cover malformed IPC, abnormal exit, a surviving independent
worker after the caller deadline, draining and cross-process lock ownership.
The C mocks check OUTPUT-first ordering, source-change negotiation, full-pitch
NV16 planes, zero tails, payloads, errors, retries and cleanup memory retention.

The six UI captures use actual XTest input, scale factor two and stable X11
frames. Disabled clicks preserve body pixels, preferences, anchors and Y4.
Two cold restarts preserve requested-on preferences while effective policy
remains off; four separately labelled scripted requests cannot enable
unsupported hardware. Native attempts, completions, resizes and fallbacks all
remain zero. The [reviewed six-image baseline](uiux/baselines/2026-11-15-jpeg-acceleration/README.md)
preserves the PNGs, observed state, source identity and original report; the
previous 51-image baseline is unchanged.

## Reproducibility details

Native-feature tests use the verification runner's private `0077` umask.
An initial supplemental command omitted that setup and eight existing
lifecycle fixtures correctly rejected their directory permissions. The full
784-test set subsequently passed with the standard environment and unchanged
production code.

The Reader harness now targets the beginning control in the toolbar's first
row and waits for menu readiness. Its former coordinates selected the newer
fit row. The final nine-case run preserves the original page-one/menu-closed
assertions and additionally requires fit to stay unchanged. Failed records
remain separate from the final passing result.

| Verification input | SHA-256 |
| --- | --- |
| Corrected Reader harness | `38595bc8ece7b8164407c64c5e2b7e9da61155b08c26d34077ebdba889e505f0` |
| Final UI harness | `47a5c1bc78497fb23c225a77530ec9a4ee841c2a28b36135a9c94fb909ce2664` |
| Final UI report | `0de9006ea93f38f7d99c2e3fa467534753c793b52ff7204d24cd0b62295fda30` |

Physical rendering quality, actual decoder/driver cleanup on BiliComics,
hardware speedups and end-to-end page-turn timing require a separate supervised
physical batch. Aidoku measurements remain evidence for its own candidate.
