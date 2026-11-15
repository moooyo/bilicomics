# Physical JPEG acceleration evidence: 2026-11-16

The final exact-tuple package completed R7's 18 ordered phases and R8's ordinary
application-restart continuation with zero new critical kernel diagnostics.
R7 completed five hardware calls; R8 completed one after restoring the actual
R7 exit database. The final sections below record those bounded results.

The earlier R4/R6 sections retain the preliminary faults that led to the final
restriction. All four runs used one operator-confirmed Kindle Scribe 2022,
model code `23L`, with observed firmware `5.19.5`, and controlled offline guest
receipt fixtures. No live account or remote account writes were used.
Application actions were scripted; finger-touch operation is not claimed.

JPEG/IMGRZ call completion, reader-model publication, framebuffer writing,
accepted EPDC submission, observed driver completion, and optical panel
completion are separate evidence boundaries. No recorded batch establishes general
hardware support, optical gray-level acceptance, or a release pass.

## Runs and policy changes

| Run | Recorded policy and result | Resulting restriction |
| --- | --- | --- |
| R4: `media-20261115-r4-19712539` | Gray completed once; the following baseline Color 4:2:2 request produced new IOMMU read-fault diagnostics. Hardware was disabled and the color page was published through CPU fallback. | All color JPEG submissions were removed; a Gray-only candidate was prepared. |
| R6: `media-20261116-r6-5a0429ee` | The Gray-only candidate completed five requests, then the sixth request, Tall Gray `1280 x 2560 -> 1240 x 2480`, produced new IOMMU read-fault diagnostics. The failed result was superseded and CPU fallback published the page. | A hotfix is being prepared to restrict all three admission layers to the two exact completed tuples below. |

R4 started at `2026-11-15T16:00:04.932814+00:00`, which is November 16 in
Asia/Shanghai. Its retained run-directory date and run ID remain unchanged.
Firmware is provenance for these observations, not a new runtime qualification
rule or a claim about other models or system versions.

The post-R6 hotfix is limited to these exact source/target pairs:

| Source pixels | Target pixels | R6 completed calls |
| --- | --- | ---: |
| 1920 x 2560 | 1860 x 2480 | 4 |
| 1920 x 1280 | 1860 x 1240 | 1 |

The intended gates are the parent metadata/decode path, isolated worker, and C
entry point. These pairs do not imply a scale-ratio rule, a minimum width, or
support for neighboring dimensions. At this note's initial preparation, the
restricted replacement package was still being compiled. Those R4/R6
observations did not establish acceptance of that package. The subsequent
R7/R8 sections record the completed targeted runs with their own identities.

The root cause of either fault remains unknown. The diagnostic class does not
identify a particular engine or establish a common internal cause. Neither
faulting case was retried.

## Reading the evidence tables

`N/H` is the operation preparation profile's
`native_sources / processed_source_hits`. `A/C/F` is the cumulative native-media
`attempts / completed / software_fallbacks` at the matching published state.
Every recorded completion was also a resize. Eligibility rejections that used
ordinary CPU preparation did not increment the native failure counter.

`P` refers to one-based lines in the run's `performance.jsonl`; `L` refers to
one-based lines in `lifecycle.jsonl`. R4 ledger references are zero-based JSON
array indices in its retained `live-ledger.json`. A requested or effective
`decode_and_resize` setting alone does not establish that a specific operation
submitted hardware work.

## R4: Color 4:2:2 fault and Gray-only response

| op / version | Actual source / fit | Actual published result | N/H | A/C/F | Evidence |
| --- | --- | --- | ---: | ---: | --- |
| 2 / 1 | Gray page 1 / page, Off | CPU, published | 0/0 | 0/0/0 | P146 preparation, P153 publication; ledger 26 |
| 4 / 2 | Gray page 1 / page, On | Hardware call completed, published | 1/0 | 1/1/0 | P164 preparation, P171 publication; ledger 29 |
| 5 / 3 | Color422 page 2 / page | Not published; cancelled and superseded | 0/0 | No matching ready state | P183 preparation, P187 superseded |
| 6 / 4 | Same Color422 page 2 / page | CPU fallback, effective and presented policy Off | 0/0 | 2/1/1 | P198 preparation, P205 publication; ledger 32 |

The color fixture was baseline 8-bit JPEG with sampling
`2x1 / 1x1 / 1x1`. The recorded request was the complete
`1920 x 2560 -> 1860 x 2480` source, without a sampled crop. The published CPU
replacement matched page 2 and a `1860 x 2480` reader frame.

For op5, `native_decode_resize` records one attempt, zero completions, and one
partial span. Op6 decoded through software and has no native stage. R4 has no
processed-cache hit. Its phase plan's expected color completion and subsequent
Color444, fallback, fit, or cache cases are not actual completed evidence.
In particular, R4 op6 is a CPU fallback; R6 op6 below is a different operation in
a different run.

## R6: Gray calls, cache, fits, and CPU paths

| op / version | Actual source / fit | Actual published result | N/H | A/C/F | P / L |
| --- | --- | --- | ---: | ---: | --- |
| 2 / 1 | Gray page 1 / page, Off | CPU | 0/0 | 0/0/0 | 146 / 27 |
| 4 / 2 | Gray page 1 / page, On | Hardware call completed | 1/0 | 1/1/0 | 164 / 30 |
| 5 / 3 | Gray page 1 / page, Off | Canonical source-cache reuse | 0/0 | 1/1/0 | 182 / 33 |
| 6 / 4 | Gray page 1 / page, cacheOn | Processed source-cache hit | 0/1 | 1/1/0 | 200 / 36 |
| 7 / 5 | Gray page 1 / original | Hardware call completed | 1/0 | 2/2/0 | 219 / 39 |
| 8 / 6 | Gray page 1 / height | Hardware call completed | 1/0 | 3/3/0 | 238 / 42 |
| 9 / 7 | Gray page 1 / width | Hardware call completed | 1/0 | 4/4/0 | 257 / 45 |
| 10 / 8 | Color422 page 2 / width | CPU; no additional hardware attempt | 0/0 | 4/4/0 | 277 / 48 |
| 11 / 9 | Color444 page 3 / width | CPU; no additional hardware attempt | 0/0 | 4/4/0 | 297 / 51 |
| 12 / 10 | Color420 page 4 / width | CPU; no additional hardware attempt | 0/0 | 4/4/0 | 317 / 54 |
| 13 / 11 | Progressive page 5 / width | CPU; no additional hardware attempt | 0/0 | 4/4/0 | 337 / 57 |
| 14 / 12 | PNG page 6 / width | CPU; no additional hardware attempt | 0/0 | 4/4/0 | 356 / 60 |
| 15 / 13 | Wide Gray page 7 / width | Hardware call completed | 1/0 | 5/5/0 | 375 / 63 |
| 16 / 14 | Wide Gray page 7 / height | CPU centered crop | 0/0 | 5/5/0 | 395 / 66 |
| 17 / 15 | Tall Gray page 8 / height | Not published; cancelled and superseded | 0/0 | No matching ready state | 414 / none |
| 18 / 16 | Same Tall Gray page 8 / height | CPU after failure latch, effective policy Off | 0/0 | 6/5/1 | 428 / 69 |

The Off -> On -> Off -> cacheOn sequence retained page 1, source anchor
`[0,0]`, fit `page`, unit zoom, zero source rotation, and the same reader
viewport. Op5 has `reused_sources=1`, which is ordinary original-size source
reuse. Op6 is the actual processed hit: its lookup completed once,
`processed_source_hits=1`, `decoded_sources=reused_sources=0`, and there is no
`native_decode_resize` stage. P192 also reports
`prepared_frame_lookup.candidate=false`; this was not a prepared whole-frame
hit.

The page/original/height/width Gray calls all used
`1920 x 2560 -> 1860 x 2480`. That source has the viewport's aspect ratio, so
the four fits happened to produce the same geometry. This does not accept all
four fits at arbitrary aspect ratios or independently test stretched page-fit
geometry on a mismatched source.

Wide Gray width fit completed `1920 x 1280 -> 1860 x 1240`, centered at canvas
offset `[0,620]`. Its subsequent height fit used CPU centered cropping: the
published source anchor was `[0.25,0]`, focus width `0.5`, and the corresponding
source window was `[480,0,960,1280]`. This did not submit another hardware
request.

Tall Gray height fit requested `1280 x 2560 -> 1240 x 2480`. Op17 records one
native attempt, zero completions, one partial native span, and
`outcome=cancelled`; P418 marks it superseded. Op18 then publishes the same page
and fit through CPU processing. L69 records availability `failed`, with
`attempts=6`, `completed=resized=5`, and `software_fallbacks=1`.

Tall-width cropping, small-image upscaling/native-size behavior, and identity
dimensions existed in the fixture plan but had no executed operation or ready
state before the stop. Zoom, source rotation, panel/strip modes, and those
unreached cases are not physical passes in this run. All observed states kept
`hardware_y4_requested=false` and `hardware_dither=false`; actual hardware Y4
execution was not tested.

## Publication, backend refresh, and panel boundaries

For the first Gray On operation, P171 publishes op4, generation 3, version 2.
P172 is the corresponding backend Frame 13 with `reader_operation_id=4` and
accepted update marker 13. It records a complete viewport refresh region,
`refresh_ioctl_returned=true`, and `refresh_accepted=true`. The same frame has
`driver_completion_observed=false` and `panel_completion_observed=false`.
Reader-ready is therefore not a physical-panel completion event.

The named-fit replacements retained the approved complete-frame path:

| R6 operation | Backend frame | Accepted marker | Complete-frame replacement | Driver completion observed | Panel completion observed |
| --- | ---: | ---: | --- | --- | --- |
| op7 / original | 19 | 16 | true | true | false |
| op8 / height | 21 | 18 | true | true | false |
| op9 / width | 23 | 20 | true | true | false |
| op16 / Wide Gray height crop | 42 | 33 | true | true | false |

Later empty or loading frames can still carry the previous reader operation ID.
They must not be substituted for the first nonempty frame following a matching
model publication. A completed driver marker or driver wait remains distinct
from an optical panel observation.

As one trace example, op4's native call span was 260.858 ms. Render request to
model publication was 426.563 ms; render request to refresh-submit return was
618.772 ms. These are individual software/driver boundaries, not measured
page-turn or optical-panel latency. Aggregate preparation spans must not be
added to their child stages.

## Selected R6 grayscale capture

The retained [original PNG](physical/native-media-20261116/r6-gray-hardware.png)
and [sanitized capture manifest](physical/native-media-20261116/manifest.json)
bind the successful first Gray request to the recorded fixture process,
generation 3/version 2 publication, op4/Frame13 backend submission, and visible
framebuffer-memory context. The
[capture README](physical/native-media-20261116/README.md) records its scope.

The current EPDC mode is `Y8`, sourced from the application's current
reader-ready record at L30, with no preflight-mode fallback. The capture has
`1860 x 2480` visible pixels, 8-bit grayscale, stride 1872, raw rotation 3, and
zero offsets. The source receipt still records the original `1920 x 2560`
dimensions, 230,973 encoded bytes, and validation version 1. Its source digest
is `8b28a2b01477186adbff47d1ee66f1090e8553fd13a7fc63127cd971c1585af7`.

There is no synthetic nonce oracle and no recorded RGB fingerprint. The PNG is
not a panel photograph, an independent nonce-based pixel proof, an optical
gray-level measurement, or a finger-touch test. The original memory PNG and
derived EPDC projection have the same SHA-256 because verified `Y8` required no
additional inversion. The capture predates the later R6 fault and does not make
the entire run a pass.

## Fault stop and restoration

| Recorded result | R4 | R6 |
| --- | --- | --- |
| Final batch status | `driver_fault` | `driver_fault` |
| New critical kernel diagnostic lines | 10 | 10 |
| Same boot preserved | true | true |
| Native recovery recorded | true | true |
| Final lease release recorded | true | true |
| Lighting preserved | true | true |

Each stop was followed by recorded native-baseline restoration and process
cleanup. The final external release records checked the owning identity and
token and recorded native restoration and closed connections before release.
The nested stop operation did not itself release the externally managed lease;
the later final release record is authoritative.

Programmatic restoration is not a finger-touch claim. The retained records keep
`physical_home_and_touch_confirmation` pending operator observation,
`physical_panel_review=pending`, and `hardware_support_accepted=false`.

## Recorded identities and retained scope

R4 identities are retained here without copying its raw sidecars or account-shaped
fixture database:

| Artifact | R4 SHA-256 |
| --- | --- |
| Application | `51defabe274fdf5976b3f3a0573c0df9a70d08bd9452410c28bc1671d96075e4` |
| Worker | `5154b973c9ca78c3af25adc1a979dfac590f8acc95fc2732e476dc96f696a44a` |
| Package | `bd9e347463993c4d17057ec932b6534e0ab8394eaec46a8fcac1fd76873a6495` |
| Fixture manifest | `419493db7dbe1103ea769411294261dd91dfd0422e41e2bf32e4e11737b2c29d` |
| Source manifest | `288cdfb7a2ca53b2e9a4a3c92289399fac7e5ed94e87cc0a510519bde8d7611b` |

The selected R6 manifest records application
`4be5abe096a3b8aa1e68a13bb2626c8a37821780cd8d6e1562f2d2e84868b65f`,
worker `290e51e6417f98d65ff2ead8e9cc4d990efdda328213e5674fca701dcfb61453`,
and source manifest
`0815e502d1adf4e13026bd313dee274863cb14e5a80a40ca6b681a15e32d6a92`,
together with its exact package, fixture, and PNG identities. These are R6
identities, not identities of the post-fault replacement build.

The full logs, framebuffer dump, original sidecars, and recorded databases stay
outside the repository. This document and selected sanitized capture record were
prepared by reading already collected host-side evidence. No new device access,
tests, local validation, or physical acceptance was performed to author them.
The following R7/R8 results have separate recorded identities and evidence scope.

## Final package and R7 completion

R7 (`media-20261116-r7-04df24e0`) and R8
(`media-20261116-r8-5619c7e9`) used the same restricted package:

| Artifact | SHA-256 |
| --- | --- |
| Application | `e56a0ac6e22e65d40ddebb3825555394b64123825d740e543589ba9ebc47c14c` |
| Worker | `f135945d85928464850c4036b2875ca4cb7031da315f4993e7e906ae93f07127` |
| Package | `84b27b9cb960ab1d28ad560f6a5415463e8bfef487be0fff42f3093f34b43d5e` |
| Source manifest | `17629c73eafb64cff585bea4046e9cdd8092eacda91c13921d71a60cc552792c` |

R7's fixture manifest is
`ff860e5511934e04a0983de3c72268154d4a46dbb500afd93b4d84daa4aba6b6`.
Every row below has an actual model publication and matching reader-ready
state. The [selected R7 JSON](physical/native-media-20261116/r7-evidence.json)
retains the profiles, exact line references, original receipt fields, and
backend observations without the private raw logs.

| Phase | op / version | Actual page / fit / selection | Published preparation | N/H | A/C/F | P / L |
| ---: | --- | --- | --- | ---: | ---: | --- |
| 0 | 2 / 1 | Gray 1 / page / Off | CPU | 0/0 | 0/0/0 | 146 / 27 |
| 1 | 4 / 2 | Gray 1 / page / On | Hardware completed | 1/0 | 1/1/0 | 164 / 30 |
| 2 | 5 / 3 | Gray 1 / page / Off | Canonical source reuse | 0/0 | 1/1/0 | 182 / 33 |
| 3 | 6 / 4 | Gray 1 / page / On | Processed source hit | 0/1 | 1/1/0 | 200 / 36 |
| 4 | 7 / 5 | Gray 1 / original / On | Hardware completed | 1/0 | 2/2/0 | 219 / 39 |
| 5 | 8 / 6 | Gray 1 / height / On | Hardware completed | 1/0 | 3/3/0 | 238 / 42 |
| 6 | 9 / 7 | Gray 1 / width / On | Hardware completed | 1/0 | 4/4/0 | 257 / 45 |
| 7 | 10 / 8 | Color422 2 / width / On | CPU | 0/0 | 4/4/0 | 277 / 48 |
| 8 | 11 / 9 | Color444 3 / width / On | CPU | 0/0 | 4/4/0 | 297 / 51 |
| 9 | 12 / 10 | Color420 4 / width / On | CPU | 0/0 | 4/4/0 | 317 / 54 |
| 10 | 13 / 11 | Progressive 5 / width / On | CPU | 0/0 | 4/4/0 | 337 / 57 |
| 11 | 14 / 12 | PNG 6 / width / On | CPU | 0/0 | 4/4/0 | 356 / 60 |
| 12 | 15 / 13 | Wide Gray 7 / width / On | Hardware completed | 1/0 | 5/5/0 | 375 / 63 |
| 13 | 16 / 14 | Wide Gray 7 / height / On | CPU centered crop | 0/0 | 5/5/0 | 395 / 66 |
| 14 | 17 / 15 | Tall Gray 8 / height / On | CPU; tuple excluded | 0/0 | 5/5/0 | 414 / 69 |
| 15 | 18 / 16 | Tall Gray 8 / width / On | CPU centered crop | 0/0 | 5/5/0 | 433 / 72 |
| 16 | 19 / 17 | Small Gray 9 / width / On | CPU upscale | 0/0 | 5/5/0 | 452 / 75 |
| 17 | 20 / 18 | Identity Gray 10 / width / On | CPU; unchanged size | 0/0 | 5/5/0 | 471 / 78 |

The four full Gray calls used `1920 x 2560 -> 1860 x 2480`; Wide Gray used
`1920 x 1280 -> 1860 x 1240`. No other row added a hardware attempt. R7 op6 is
the real processed hit: `processed_source_hits=1`, no native stage, and no CPU
decode or canonical-source reuse. Op5 has `reused_sources=1` instead. The
Off/On switches retained page 1, source anchor `[0,0]`, unit zoom, zero source
rotation, and the software Y4/dither state.

The Wide Gray height crop retained anchor `[0.25,0]`. Tall Gray width retained
the centered vertical origin `source_y=1/6`. The Tall Gray height combination
that faulted under R6 never reached hardware in R7. Small Gray upscaling and
identity dimensions also produced ready CPU frames. These are software
eligibility paths, not hardware failures, so `software_fallbacks` stayed zero.
As in R6, the four first-page fits share one aspect ratio; arbitrary-ratio
page stretching and unexecuted zoom/rotation/panel/strip cases are not added to
the physical acceptance scope.

## R8: actual persisted state and cold restart

R8's fixture manifest is
`bfb32b74ec96304220f71f7d64dac03c8c423413b41348a94de21a7f27a255a2`.
Its starting database was compared byte for byte with the R7 application-exit
database: both are 155,648 bytes with SHA-256
`42e23cc9c39e74239aab496a56453f1f71f9a873c52dc28fa348b6cd6d855a36`.
The recorded deployed database hash matches. The fixture records
`application_state_written_by_generator=false`; this continuation was not a
fresh preference/anchor seed.

| op / version | Actual published state | N/H | A/C/F | Preparation / publication / ready |
| --- | --- | ---: | ---: | --- |
| 2 / 1 | Cold restore: identity page 10, width, On; CPU | 0/0 | 0/0/0 | P146 / P153 / L27 |
| 4 / 2 | Gray page 1, width, On; hardware completed | 1/0 | 1/1/0 | P165 / P172 / L30 |
| 5 / 3 | Same Gray page and anchor, width, Off; CPU | 0/0 | 1/1/0 | P183 / P191 / L33 |

This was an ordinary application process restart on the same recorded device
boot, not a Kindle reboot. Process-local hardware statistics started at zero.
The restored identity image required no resize and submitted no hardware work.
The Gray call completed once, and switching Off decoded through software
without another attempt. R8 has no processed-cache hit.

The actual starting database had `jpeg_acceleration=true` and `fit=width` in
both the global and per-comic preferences, and the persisted anchor pointed to
page index 9. The exit database had both switches false, retained `fit=width`,
and persisted Gray page index 0. Unit zoom, zero rotation, page mode, source
anchor `[0,0]`, and the software Y4/dither settings remained unchanged for the
same-page switch. The changed exit database hash is
`f29fe2b330edbbfc3396f2011bb00a5d518fe84f362f443dedaac964ac6fe66e`;
only the starting database is claimed byte-identical to R7's exit.

All ten complete receipt rows, their payload text, and all ten identity-history
rows were equal across R7 exit -> R8 start -> R8 exit. Validation versions
remain 1, with original hashes, encoded lengths, dimensions, and verification
timestamps intact. Gray page 1 still has `1920 x 2560`, 230,973 bytes, and
`verified_at=1794762057`; its receipt did not become the `1860 x 2480` hardware
output. The [selected R8 JSON](physical/native-media-20261116/r8-evidence.json)
records these comparisons and selected persisted fields; no database was copied
into the repository.

## Final captures, recovery, and remaining limits

| Capture | Current publication / backend | Recorded state |
| --- | --- | --- |
| [R7 Gray](physical/native-media-20261116/r7-gray-hardware.png) | op4 / version2 / Frame13 | Gray page 1, page fit, On; one completed call |
| [R7 final](physical/native-media-20261116/r7-final.png) | op20 / version18 / Frame50 | Identity page 10, width, On; five completed calls |
| [R8 Gray](physical/native-media-20261116/r8-gray-hardware.png) | op4 / version2 / Frame13 | Gray page 1, width, On; one completed call |
| [R8 final](physical/native-media-20261116/r8-final.png) | op5 / version3 / Frame16 | Same Gray page, width, Off; still one completed call |

All four original PNGs are copied without pixel changes. Their selected
manifests bind the exact application/worker/source/fixture identities, process
birth, current generation/version, model publication, backend frame, original
receipt, and framebuffer geometry. EPDC `Y8` is from each current application
trace with no preflight fallback. The memory PNG and Y8 projection are identical
for each checkpoint.

These captures record accepted refresh ioctls; their driver-completion,
panel-completion, and physical-panel confirmation flags are false. There is no
nonce oracle or independent RGB pixel fingerprint. Hardware Y4 was not requested
or applied. No optical gray-level measurement, optical page-turn latency,
finger-touch acceptance, or general device support is established.

| Recorded final outcome | R7 | R8 |
| --- | --- | --- |
| Batch status | `completed` | `completed` |
| New critical kernel diagnostic lines | 0 | 0 |
| Same boot / native recovery | true / true | true / true |
| Worker drained / application exited | true / true | true / true |
| Exact owner/token release and connections closed | true | true |
| Lighting preserved | true | true |

The raw records retain physical-panel review and manual native-home/touch
observation as pending. The completed final-package batches do not erase the
R4/R6 faults or establish their unknown root cause.

After both batches, the separately recorded cleanup run
`media-clean-20261116-39a8fa8c` removed all eight owned isolated device stages.
Root reported checking each stage's run-token, canonical path, and absence of
live owned PIDs before deletion. The saved result records eight removals,
original installations unchanged, statusbar running, awesome sleeping,
brightness 12, warmth 8, and release of its fresh cleanup lease. The
[sanitized collection manifest](physical/native-media-20261116/manifest.json)
preserves these outcomes without a token, private configuration, or full log.
This documentation task only read already collected host evidence and copied
selected artifacts; it performed no device operation or new test.
