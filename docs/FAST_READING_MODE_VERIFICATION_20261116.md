# Fast reading verification: 2026-11-16

The persistent [fast reading implementation](FAST_READING_MODE.md) was built
and checked on `test-env`. This record covers offline application fixtures,
host-executed backend policy tests, and an ARM development package. It does not
claim physical Kindle waveform delivery or optical acceptance.

## Build identity

| Item | Identity |
| --- | --- |
| Base commit | `2da87ab512fded000174b7542767082a3a94e147` |
| Worktree | Uncommitted changes; no clean-checkout claim |
| Frozen workspace | `/root/bilicomics-fast-reading-20261116-01a1d11a-r5` |
| Compiled-input manifest | 518 files |
| Compiled-input fingerprint | `f1d99a8addddf145c0ab46683619156d04a6beae54380f329e18a484532fc7ee` |
| Desktop executable | `497dc507f433019828874f5789496eacce7ed4c0d7c9e8846add56d3fe4bc067` |
| ARM application | `eb6977cabf79ccbb947979a9c184f069d9beaa3e4ed3f50522085ae1e091b774` |
| ARM JPEG worker | `f135945d85928464850c4036b2875ca4cb7031da315f4993e7e906ae93f07127` |
| Development ZIP | `7f84ebff2a11bef0a9105521d59496396b7e132be3a7130821d43d30586c1c44` |

All digest values are SHA-256. The source manifest includes the two C codec
build inputs. The JPEG worker remains byte-identical to the earlier qualified
worker; the application and bundle have new identities. Earlier physical JPEG
results retain their original application identity and do not certify this
fast-mode candidate.

## Executed checks

| Gate | Result | Preserved evidence |
| --- | --- | --- |
| Locked, offline application check | Passed | [Core/package report](verification/fast-reading-20261116/core-package.json) |
| Default-feature application library | 798 passed, 0 failed, 3 existing opt-in tests ignored | [Core/package report](verification/fast-reading-20261116/core-package.json) |
| Desktop executable build | Passed and sealed by executable digest | [Core/package report](verification/fast-reading-20261116/core-package.json) |
| Existing Reader runtime matrix | 9 automated cases passed | [Runtime report](verification/fast-reading-20261116/reader-runtime.json) |
| Isolated native backend | 227 passed, 0 failed, 0 ignored | [Backend report](verification/fast-reading-20261116/backend.json) |
| Kindle-feature refresh scene mapping on Linux host | 7 passed, 0 failed; other tests filtered | [Original test log](verification/fast-reading-20261116/kindle-refresh-scenes.txt) |
| ARM application and worker cross-compilation | Passed | [Core/package report](verification/fast-reading-20261116/core-package.json) |
| Development archive and ARM ELF checks | 14 passed; 36 exact archive entries | [Package report](verification/fast-reading-20261116/package.json) |

Compilation used two Cargo jobs, serial library tests, a private target cache,
and a private temporary directory. The isolated backend verifier copied the
crate instead of mutating the production snapshot. All 465 resolved backend
dependency versions and checksums match the production lockfile; its source
inputs remained unchanged. The three ignored application tests were not
reported as executed benchmarks.

## Actual desktop presentation

The accepted `fast-ui-r2` run uses the sealed R5 desktop executable and passes
all eight main sessions, twenty baseline captures, and eight ordinary cold
restarts across Paperwhite 6 and Scribe portrait/landscape profiles. Every
preference change uses actual XTest controls. Fast disables periodic clearing
while retaining every-six and dithering choices; the source anchor remains
unchanged. Restoring Normal reproduces the original quiet reader PNG exactly.

The final UI harness has SHA-256
`133cc74a0bf0a15816901d47ba57ec6786bbc6dab22996e86c729c51f0049d37`.
It is staged at `tools/verification-ui-r2` outside the frozen compiled-source
snapshot. The compiled-input manifest excludes Python test scripts; the
capture report records the executed harness and dependency digests separately.
No compiled input changed after the R5 build.

Reader readiness is a model publication event, so the harness also waits for
the actual X11 paint: at least 1.5 seconds and three identical eligible frames
within a bounded timeout. Fast frames must contain exactly black and white;
restored Normal must match its original pixel digest. Neither condition is
weakened to accommodate startup loading frames. Bounded idle samples retain
the same pixels, source anchor, reader generation, and render version.

The [dated UI supplement](uiux/baselines/2026-11-16-fast-mode/README.md) preserves
the original PNGs, state sidecars, complete capture report, source manifest,
and separate human review. Account scroll checks use actually presented,
stable Reading frames and keep the header and bottom navigation unchanged.
They also prove that lower clearing/JPEG controls remain reachable.

All twenty final PNGs are byte-identical to the individually reviewed original
captures. The sealed baseline contains 46 files; its manifest SHA-256 is
`ca7960480cdf90a0f2b0d875213727d019ea4e6a12e2abc620e85dd538477364`.
The original capture report keeps its pending visual-review field unchanged;
the completed human review and exact image identity pairs are separate records.
The existing nine-case Reader runtime matrix also passes with the same sealed
binary, covering ordinary portrait controls, strip completion, landscape
cover/direction/single-page states, and chapter-end controls. Its additional
screenshots retain their own unclaimed visual-acceptance scope.

## Policy coverage

Reader regressions exercise direct Gray8-to-binary dithering, all Bayer phases,
both canvas paths, immutable source caches and proofs, cancellation, geometry,
rotation, zoom, source anchors, and final-frame preparation identity. Application
tests cover preference persistence, per-comic defaults, retained Gray8 during a
pending mode change, delayed Y4 changes, grayscale controls and feedback,
expired sources, toast visibility, saved periodic intervals, idle behavior,
and explicit manual clearing.

Platform tests retain normal refresh budgets while bypassing automatic
clearing during quiet Fast content. Backend tests preserve nonflashing DU
under a pure dirty-request flood, retain stronger mixed grayscale/manual
requests, and reject the DU full-rewrite exception for gray or colored pixels,
incomplete buffers, Y4 intent, lifecycle recovery, sleep, and rotation.
Complete mapping replacement and update-completion barriers remain intact.

These are request-policy and host-memory checks. A recording sink acknowledges
queueing, and fake framebuffer receipts acknowledge their simulated driver;
neither is evidence of an actual panel update. The ARM executables were checked
statically and were not executed on a Kindle in this run. No physical lease,
live account, real comic acquisition, or upstream write was used.
