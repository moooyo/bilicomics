# Preliminary R6 framebuffer evidence

[r6-gray-hardware.png](r6-gray-hardware.png) is the original R6
`gray-hardware.png`, copied byte for byte from the retained `test-env` evidence.
It was not rendered again, resized, retouched, composited, or reconstructed.
The image contains a controlled offline grayscale receipt fixture, not account
content or an acquired comic.

This directory is a selected, sanitized supplement to the
[preliminary physical report](../../KINDLE_MEDIA_ACCELERATION_PHYSICAL_20261116.md).
The [manifest](manifest.json) records the exact PNG, application, worker,
package, fixture, and source-manifest identities, together with the selected
current-process, reader-publication, backend, EPDC, and framebuffer bindings.
Original sidecars, logs, databases, lease records, tokens, nonces, and private
configuration are not copied into the repository.

The source is run `media-20261116-r6-5a0429ee` on the tested Scribe 2022
`23L`, with observed firmware `5.19.5`. The captured state is Gray page 1,
operation 4, reader generation 3, render version 2, and backend Frame 13.
JPEG/IMGRZ completed once for `1920 x 2560 -> 1860 x 2480` before this capture.
The current application trace reports EPDC `Y8`; no preflight-mode fallback is
used. The visible framebuffer is `1860 x 2480`, 8-bit grayscale, with stride
1872 bytes and raw rotation 3.

This is framebuffer-memory evidence. The companion EPDC-mode projection had
the same PNG hash because `Y8` required no additional inversion. It is not a
panel photograph, an optical gray-level measurement, or a finger-touch test.
There is no synthetic nonce oracle and no recorded RGB fingerprint. The
binding establishes the recorded fixture process, current model publication,
backend submission, and framebuffer-memory context; it does not provide an
independent nonce-based pixel oracle or prove panel completion.

Frame 13 records an accepted refresh ioctl, while its driver-completion and
panel-completion observation flags are false. Hardware Y4 was not requested or
applied. The later R6 Tall Gray transaction faulted; preserving this earlier
successful capture does not make the complete run a pass or accept the later
restricted replacement package.

The PNG is 50,902 bytes. Its SHA-256 is
`5a76710d128c5d861979f6d132695195744eff19d8de2181370eb78ee6d44109`.
This manifest describes the selected capture, not a final frozen inventory for
future R7/R8 evidence.

## Final exact-tuple package: R7 and R8

The R6 text above is retained as the historical preliminary capture description.
The collection now also contains the completed R7/R8 batches and a selected
file inventory in [manifest.json](manifest.json).

| Run / checkpoint | Original capture | Selected evidence |
| --- | --- | --- |
| R7 / first Gray hardware frame | [PNG](r7-gray-hardware.png) | [R7 JSON](r7-evidence.json) |
| R7 / final identity page, width, On | [PNG](r7-final.png) | [R7 JSON](r7-evidence.json) |
| R8 / Gray hardware frame after cold restart | [PNG](r8-gray-hardware.png) | [R8 JSON](r8-evidence.json) |
| R8 / final same Gray page, width, Off | [PNG](r8-final.png) | [R8 JSON](r8-evidence.json) |

All four additional PNGs are unchanged copies of their recorded host artifacts.
R7 completed all 18 ordered phases with five completed hardware calls and one
processed-cache hit. R8 started from the exact 155,648-byte R7 exit database,
restored page 10 / width / On with zero process-local hardware attempts, then
completed one Gray call and switched Off without another hardware attempt.
Both final batches recorded zero new critical kernel lines, the same boot,
native recovery, drained workers, and an exact owned lease release.

Both runs used application SHA-256
`e56a0ac6e22e65d40ddebb3825555394b64123825d740e543589ba9ebc47c14c`,
worker `f135945d85928464850c4036b2875ca4cb7031da315f4993e7e906ae93f07127`,
and source manifest
`17629c73eafb64cff585bea4046e9cdd8092eacda91c13921d71a60cc552792c`.
Each selected JSON contains its own fixture identity, original receipt fields,
actual operation profiles, capture hashes, and current model/backend bindings.
The R8 JSON separately records the byte comparison and the persisted preference
and reading-anchor changes; the database itself is not included.

These are completed controlled physical batches for the exact recorded package
and cases. All captures remain framebuffer-memory evidence: no nonce oracle,
independent pixel oracle, optical panel completion, optical gray-level
measurement, finger-touch acceptance, or hardware Y4 execution is claimed.
The identical first-Gray PNG hashes do not replace the separate per-run process
and publication bindings.

[The collection manifest](manifest.json) records the later owned-stage
cleanup result. Eight isolated test stages were removed, original installations
were unchanged, native services and lighting were retained, and the fresh
cleanup lease was released. No secret token, raw sidecar, database, or private
log was copied into this collection.
