# Perf05 completed prediction delivery: 2026-11-16

This follow-up addresses the repeat-next regression recorded in the
[Reader-priority R4 report](PERF05_READER_PRIORITY_20261116.md). R4 is the
before variant for this change. The older `baseline-r2` runs are historical
context, not the acceptance comparator for the new implementation.

**Status: fresh builds, library regressions, seven strict comparison pairs,
and the separate guest pixel comparison passed. Repeat-next now hits in all
six repeated Fast cases; this is a scoped host result, not a universal speedup.**

## Correction to the initial explanation

The R4 round-trip miss was not an unfinished prediction. In all six Fast
cover-on/off runs, the same prediction operation reported `ready=true`,
`cancelled=false`, and `speculative_result.ok=1` approximately 158-208 ms before
the repeat-next input. Its UI callback began approximately 0.68-0.84 ms after
that input. Every repeat-back checkpoint still reported the slot as
`preparing`, with zero retained bytes; no `next_frame_retained` occurred for
that prediction.

For `candidate-r4/fast-cover-on-1`, times relative to repeat-back are:

| Time, ms | Operation | Observed event |
| --- | --- | --- |
| 632.878 | Prediction 21 | Worker begins during the publication of Reader operation 20, version 7 |
| 1299.884 | Prediction 21 | Preparation succeeds; complete shared RGBA payload |
| 1300.392 | Prediction 21 | Successful result is observed by the async delivery wrapper |
| 1508.209 | Reader 22 | Repeat-next input arrives |
| 1508.376 | Reader 22 | Render request targets version 8 |
| 1508.424 | Reader 22 | Prepared-frame lookup reports `candidate=false` |
| 1508.991 | Prediction 21 | UI callback begins; no frame is retained |

Advancing the version and target before delivery invalidates the old origin
fence. The trace does not demonstrate a malformed prediction key or record a
specific rejection-reason field or queue-insertion timestamp. The initial
explanation that the worker had not finished is therefore withdrawn. The useful
work was already complete; its delivery had not been consumed before
navigation changed the origin.

## Narrow implementation

`collect_completed_reader_prediction` runs only for `turn:next`, before Reader
activity handling or any core/version advance. It first checks the existing
origin, viewport, settings/raster identity, generation, epoch, cancellation,
access, preload, and App pending-sequence conditions.

`JobMailbox::take_ready_speculative` uses `try_lock` and extracts only an
already queued event belonging to the exact latest live speculative token,
including its `Arc` identity, epoch, sequence, and key. It cannot consume a
regular, durable, or cover result. A busy lock, an unfinished worker, or an
identity mismatch immediately retains the existing miss/cancel behavior.
There is no wait for prediction completion.

The caller follows the normal tick's pending/epoch removal and view-dirty
handling, then invokes the original prediction callback. That callback still
checks the key, cancellation token, current origin, access, and retained-byte
limit. Collecting a result does not publish reading pixels. Normal navigation
must still match the target, and `reuse_next_frame` performs the existing fresh
read/hash, receipt/object, and source-proof checks before reusing its pixels.
Changed or missing sources retain their normal fallback.

The change moves an existing result between existing owners. It adds no
compute slot, result permit, retained-frame cache, pixel allocation, or memory
budget. Delivery capacity remains with the returned callback until apply/drop.
It does not add backward prediction or broaden Page/FitPage eligibility.

## Frozen identities and completed validation

| Item | Identity or status |
| --- | --- |
| Before code | R4 `79264b7a7b0b1dc5cdda92025e56afe3dbf064b9` |
| Before source fingerprint | `2989f346437b5b447dd03aea4ad514bbe88dbcacde2d08ae99f78e94b4fbf9ab` |
| New code | `747aecc65d150f6588beec7dbf0fa4fd8be967e0` |
| New source inventory | [539 inputs](verification/prefetch-delivery-20261116/source-inputs.sha256) |
| New source fingerprint | `40d126600b44ca4169db51db21e3779a21ae7ef26de20318f83b384e79b3a3c4` |
| Fresh run | `bilicomics-prefetch-delivery-20261116-01a1d11a-r1` |
| Application library on Linux host | 857 passed, 0 failed, 4 opt-in tests ignored |
| ARM compilation and package checks | Fresh build invocation; 15 static checks passed, 39 archive entries |
| Measurement libtest | `e6303d280cc8d1706524e5b63b3edb1f7d3c559e2c121a9dc58cb7a270264bf5` |
| Desktop executable | `24bcc939ea56ec83b25d2ad4c21641a0ab73fdd2ec0a0edf9271b97408bf41db` |
| ARM application | `bd4d51dcf60ebb32854cb00532e913f08ab5ca35f1dfa1cb60f6d87f4f6ffff6` |
| ARM JPEG worker | `e39d7a897f13ce53412f42040d124a106634322899df515b5bc2e96b781914cd` |
| Development ZIP | `d19f7889ef3b9d1ac6fba26c1b18b25dc25b0d628be0350185d77abe013cec63` |
| Common Python harness | `16005b0d10282aa6d810d3ae1fd24071358b03b44cf56ccff99a70e302a261c5` |
| Common App-library runner | `a3638da08406b1e738fb825810aea42292383ea8c73a56c71f562ca6a6eeeede` |

The [sanitized summary](verification/prefetch-delivery-20261116/summary.json)
has SHA-256
`cb1694d7fc63685280bce6b6d179f9c96906a1fa47f454627432d1f3663f67a5`.
It binds the actual core counts, artifact identities, source manifest, original
comparisons, and exporter source. The
[package record](verification/prefetch-delivery-20261116/package.json) checks
the ARM images statically; they were not executed. This is a fresh invocation
using reusable build caches, not a claimed clean rebuild. No separate
Kindle-feature host-test group is claimed by this follow-up.
The public package record removes seven absolute build paths and retains the
raw-report hash in `export_provenance`; the summary's package receipt hash
continues to identify the original unmodified record.

## Measured comparison

Repeat hit counts below require `prepared_frame_hit` on the exact navigation
operation. Latencies are request-to-model wall times in milliseconds, not paint
or display-submission times. Both Fast variants report median [minimum-maximum]
across three independent processes per cover setting.

| Metric | Before: R4 | After: this change |
| --- | --- | --- |
| Repeat-next latency, cover on | 638.836 [637.045-647.945] | 29.660 [29.230-31.361] |
| Repeat-next latency, cover off | 637.962 [637.455-641.550] | 29.688 [28.001-30.021] |
| Forced retained-frame next, cover on / off | 38.164 [37.669-42.944] / 29.029 [28.848-31.937] | 28.992 [28.819-34.041] / 30.625 [27.262-34.314] |

Repeat-next changes from 0/3 to 3/3 hits in each cover configuration.
Repeat-back remains 0/3. Natural-next 2, 3, and 4 and the forced retained-frame
control each remain 3/3 in both variants. UI pixel-copy observations remain
`0` before and after; this fix restores delivery, rather than removing another
pixel copy. The cover-off forced-hit median is slightly higher, so these
results do not show every path becoming faster.

The new delivery marker is linked to `prepared_frame_hit` on the same Reader
operation in all six Fast runs: eleven linked hits, of which five are
repeat-next turns. Each linked operation completes `receipt_lookup`,
`source_read`, `source_hash`, and `receipt_recheck`, with one unchanged source,
zero rendered frames, and zero resized pixels. All eleven reach model
publication. The remaining repeat-next turn uses the ordinary already-Ready
result and is not counted as an early-handoff observation. Normal has no such marker
and is only a regression control. The new marker proves an event was queued
when input collected it, not its earlier insertion timestamp.

The seven groups are Fast/cover-on x 3, Fast/cover-off x 3, and
Normal/Bayer8/cover-on x 1, using the same fixed input and harness as R4. The
completed pixel comparison matches all 84 Reader checkpoints and seven sleep
overlays, with 84 private Reader anchor comparisons. Exact Reader cancellation
operations start, retire without publication, and reach observed worker ends
in both variants. Existing quiet-frame, idle, and controlled sleep/resume
contracts pass. Only five distinct displayed source pages receive this raster
comparison; it is not an eight-page visible-raster oracle.

The same-thread rapid group publishes two frames per new run, versus one in
R4. Its first-intent-to-final-model median grows from 1111.210 to
1546.955 ms with cover on and 1113.003 to 1539.472 ms with it off. This is a
measured endpoint regression with more intermediate publications. Input cadence
also depends on drawing, so neither that endpoint nor surviving frame timings
represent four independently paced click latencies.

The separate [guest XTest record](verification/prefetch-delivery-20261116/guest.json),
SHA-256 `02d46e3e1f0e37126a70e570f07c3f903e14ba8c164ab4e2053480cd59fee50e`,
uses one actual desktop process per variant and matches ten Reader RGB frames
and private anchors exactly. It exercises real window motion/button input,
but is not a repeated performance sample and does not test cancellation or the
Kindle-only sleep-cover route.

These are controlled App-library measurements on `test-env`, with scripted
App intents and actual software draws. Source identity reads occur before the
timed sequence, so this is not a cold-disk measurement. This follow-up adds no
ARM ABI runtime, physical-device, native SEND, panel-waveform, optical, or release-profile
performance claim. Private images, databases, and raw account material remain
outside the repository; only approved numeric/hash evidence may be retained.
