# Perf05 reader priority and verification: 2026-11-16

This update removes full-frame pixel copying from Reader UI attachment and
gives active reading priority over optional cover work. The measurements use
BiliComics' own offline PNG path on `test-env`. They do not establish Scribe,
native SEND, panel-update, optical, or release-build performance.

## Implementation

The worker now finalizes the RGBA payload in Slint `SharedPixelBuffer` storage.
Only immutable shared handles reach the UI after the existing account,
generation, viewport, and source checks pass. UI attachment creates image
references without copying the pixel payload. The worker uses one safe
`SharedPixelBuffer::clone_from_slice` allocation/copy, without a separate
default-initialization pass. Cancellation is checked before and after this
uninterruptible copy; the existing eight-million-pixel limit bounds it to
32,000,000 RGBA bytes. The original `Vec` is released before delivery, and
cancelled payloads are discarded. Shared retained allocations are counted once.
This changes copy placement; it does not eliminate copies from the pipeline.

Reader activity advances a five-second idle deadline and cancels optional
sleep-cover preparation. Resident previews remain usable through a read-only
lookup; a miss does not start the ordinary thumbnail pipeline from Reader.
Sleep-cover and maintenance computation use nonqueued idle admission. A busy
attempt retains no compute permit or queued reservation. Cancellation retains
the work identity until the actual worker and result delivery have drained;
only then can the next normal UI tick retry reader prediction. Retry deadlines
and generation fences survive this handoff.

A cold offline resume can still use the ordinary URL-scoped disk thumbnail
when no full sleep derivative exists. It is read through the bounded sleep-cover
worker and remains a thumbnail, rather than being marked as a full-resolution
cover or restarting ordinary Reader thumbnail work. A ready high-resolution
cover remains available for the existing sleep presentation path.

CPU Off/Bayer8/Floyd-Steinberg output, binary Fast mode, manual Full, physical
render geometry, named fits, reading anchors, immutable input proofs, and JPEG
admission remain intact. Existing viewport, cache, compute, and retained-result
limits still apply. A planned-memory reservation is not an RSS ceiling.
Ordinary image-library decode/resize calls are checked at their boundaries;
cancellation cannot preempt arbitrary instructions inside those calls.

## Candidate and evidence status

The current candidate is R4 at `79264b7`, which replaces the extra
default-initialize-plus-copy path used in the earlier private experiments.
R2/R3 timing and package results are superseded and do not accept this revision.

| Item | Identity |
| --- | --- |
| Candidate label | `candidate-r4` |
| Candidate code commit | `79264b7a7b0b1dc5cdda92025e56afe3dbf064b9` |
| Candidate source inventory | 539 entries; [source-build.sha256](verification/perf05-20261116/source-build.sha256) |
| Candidate source fingerprint | `2989f346437b5b447dd03aea4ad514bbe88dbcacde2d08ae99f78e94b4fbf9ab` |
| Package source inventory | 539 entries; [source-package.sha256](verification/perf05-20261116/source-package.sha256); same fingerprint |
| Host measurement libtest ELF | `2e66edaa8cf2fd62b66c45388efe28d91f0fc5a26711d700e5e47c50f72a8c87` |
| ARM application | `a44be36d60afa8f285260859ddd5b9d9f61bb0f985debdc243775980ae2f3f36` |
| ARM JPEG worker | `e39d7a897f13ce53412f42040d124a106634322899df515b5bc2e96b781914cd` |
| Development ZIP | `6ba4f155dce800e13f4bf21f171035aa9f22bda0a7e7abd74466b84347d2a8b3` |

The byte-preserving font-notice attributes from `4f6b6da` remain in place.
The fresh R4 archive passed its own package checks; it does not inherit the
earlier notice-only repack result. Earlier failures and experiments remain
preserved privately.

## Build and regression checks

The software checks, fresh ARM build invocation, and package checks below all
use the frozen R4 source. Existing dependency caches were reused; this is not a
clean rebuild claim.

| Check | Result |
| --- | --- |
| Application library on the Linux host | 850 passed, 0 failed, 4 opt-in tests ignored |
| Kindle-feature spatial tests on the Linux host | 27 passed |
| Kindle-feature refresh routing on the Linux host | 7 passed |
| Package fixtures | 8 passed |
| ARM application and worker compilation | Passed; cross-compiled binaries were not executed |
| Development ZIP and ELF inspection | 15 static checks passed; 39 exact entries |

The [verification record](verification/perf05-20261116/verification.json) and
[package record](verification/perf05-20261116/package.json) bind the
final R4 source and artifacts. Host-targeted
Kindle-feature tests overlap application coverage and are not additional ARM,
driver, or device executions. Ignored tests are not counted as benchmark results.

## Perf05 comparison

The private input contains eight original `2480 x 3508` PNGs. Its archive
SHA-256 is
`0e11719838c8da38388be9f3f389fa59ce671625bb033b1707842bce520cf076`.
Extraction and fixture construction occur before measurement. BiliComics has
no CBZ-import UI in this experiment: the App reads isolated guest receipts
whose validation markers start at zero. No live account or upstream write is
used. Source images, comic screenshots, private databases, and application logs
remain outside the repository; only numeric/hash summaries are retained.

The baseline is the earlier software-dithering R4 candidate with the common
test-only runner and a cover observer, identified by source fingerprint
`4f1761aadc105a79b5ab1ba769f84ac2b340418fc13162e5a613d0c60295b7ab`.
It is not a clean checkout of `217c4c0d11808d1412ca16a81f58c2b57e6c82d2`.
Before and after use the same runner, input seed, geometry, settings, and
unoptimized dev-test profile. A controlled App-library fixture changes only
platform metadata to admit the sleep-cover path while retaining Desktop
DeviceControl and a `1860 x 2480`, scale-2 software window. These are scripted
App intents. Independent R4 guest XTest evidence is reported separately below.

| Group | Independent processes per variant | Purpose |
| --- | --- | --- |
| Fast, saved Bayer8, Page fit, preload 1, cover on | 3 | Reader import, natural prediction, burst, cancellation, idle cover |
| Same Fast configuration, cover off | 3 | Reader comparison with the cover preference as the single control |
| Normal/Bayer8, Page fit, preload 1, cover on | 1 | Pixel and lifecycle regression; no repeated timing claim |

First reading is followed by three natural forward reads, each after 1500 ms
of host idle after its completed checkpoint. They do not wait for prediction.
A separate return-to-start group deliberately waits for a retained prediction
and requires `prepared_frame_hit` on that exact navigation operation. Repeated
reads, four rapid intents, cancellation after worker start, reopening, idle,
and controlled sleep/resume follow. Actual paints and quiet scene/overlay
flags must pass the existing strict binary or native-code pixel conditions.

## Measured R4 results

All seven pairs pass the strict configuration, pixel, anchor, cancellation,
and drain contracts: 91 paired pixel checkpoints, comprising 84 Reader frames
and seven sleep overlays. This accepts the measured contracts, not an overall
performance improvement. The retained
[Perf05 summary](verification/perf05-20261116/perf05-summary.json) has SHA-256
`a032bf3257a1d240053fe42ddab44f221465536cbda882f77a3ea33436aba832`.
Each Fast timing below is the median of three independent processes, with the
observed minimum and maximum in brackets. Normal/on is a single pixel and
lifecycle regression pair, not a repeated timing result.

The UI payload-copy count changes from `18,451,200` bytes to `0` in every
observed import, including Normal. On the deliberately retained-frame hit:

| Cover | Baseline UI import, microseconds | R4 UI import, microseconds |
| --- | --- | --- |
| On | 31,758 [30,568–37,289] | 20 [8–45] |
| Off | 31,135 [30,008–46,049] | 20 [19–97] |

Request-to-model publication wall times are milliseconds; they exclude the
later paint/hash checkpoint wait and do not measure display submission:

| Phase | Baseline, cover on | R4, cover on | Baseline, cover off | R4, cover off |
| --- | --- | --- | --- | --- |
| First Reader render | 1596.184 [1172.883–1814.395] | 1345.243 [1323.497–1360.913] | 1526.476 [1476.352–1781.502] | 1340.947 [1339.437–1355.855] |
| Natural next 2 | 942.850 [936.533–1187.554] | 30.800 [29.925–31.727] | 1022.895 [946.437–1494.835] | 29.020 [28.720–31.145] |
| Natural next 3 | 60.948 [60.676–62.151] | 30.493 [28.680–36.934] | 71.371 [61.768–102.071] | 29.889 [29.817–30.295] |
| Natural next 4 | 62.277 [60.401–70.080] | 35.360 [30.641–43.188] | 62.519 [61.326–97.148] | 31.305 [30.019–36.487] |
| Return to start | 836.190 [823.818–890.599] | 793.597 [791.750–797.112] | 863.665 [840.375–1273.649] | 795.250 [792.092–800.783] |
| Forced retained-frame hit | 65.687 [58.895–67.740] | 38.164 [37.669–42.944] | 68.354 [64.263–76.423] | 29.029 [28.848–31.937] |
| Repeat back | 671.270 [668.453–739.051] | 637.162 [633.646–639.669] | 740.793 [669.543–1033.074] | 633.702 [632.869–635.042] |
| Repeat next | 60.457 [59.035–63.594] | 638.836 [637.045–647.945] | 64.773 [57.531–79.776] | 637.962 [637.455–641.550] |
| Reopen after cancel | 680.772 [677.948–775.242] | 638.671 [636.244–647.990] | 782.776 [714.993–806.769] | 640.234 [639.282–650.085] |

Prediction improves for the first natural forward read, but not uniformly.
Counts below are actual `prepared_frame_hit` operations per navigation request,
aggregated over the three processes; retained or cancelled predictions do not
count as hits.

| Phase | Cover on: baseline to R4 | Cover off: baseline to R4 |
| --- | --- | --- |
| Natural next 2 | 0/3 to 3/3 | 0/3 to 3/3 |
| Natural next 3 and next 4, each | 3/3 to 3/3 | 3/3 to 3/3 |
| Forced retained-frame next | 3/3 to 3/3 | 3/3 to 3/3 |
| Repeat back | 0/3 to 0/3 | 0/3 to 0/3 |
| Repeat next | 3/3 to 0/3 | 3/3 to 0/3 |
| Four-intent rapid group | 2/12 to 0/12 | 1/12 to 0/12 |

Repeat-next is a measured regression in this sequence: it loses reuse and
returns to about 0.64 seconds in both cover configurations. Worker preparation
also is not uniformly cheaper. With cover on, first-render preparation rises
from 821.030 [800.158–1250.843] to 1037.961 [1007.031–1057.813] ms, and the
forced-hit preparation median rises from 24.469 to 32.596 ms. The latter still
has a shorter request-to-model time because UI import no longer copies pixels.

The rapid group delivers four Reader requests in every process. Its endpoints
are measured from actual scripted intent timestamps, not inferred from surviving
frames:

| Cover | Baseline first intent to final model, ms | R4 first intent to final model, ms | Baseline last intent to final model, ms | R4 last intent to final model, ms |
| --- | --- | --- | --- | --- |
| On | 1643.366 [1151.248–1703.357] | 1111.210 [1103.455–1114.178] | 1096.270 [1016.233–1158.655] | 978.016 [969.009–981.793] |
| Off | 1369.354 [1184.522–1725.055] | 1113.003 [1097.260–1118.795] | 1180.397 [1053.008–1237.188] | 983.675 [961.106–985.814] |

Baseline publishes `[2,2,1]` frames with cover on and `[2,1,1]` with it off;
R4 publishes `[1,1,1]` in both cells. Thus a shorter final endpoint accompanies
more coalescing, not four individually faster displayed turns. Actual input
intervals also differ: baseline includes 455.793–460.519 ms first gaps in some
on runs, while R4 on gaps are 42.175–46.249 ms. The nominal 40 ms loop is not
an externally paced four-click arrival test.

Each cancellation target starts a real Reader worker, is retired without being
published, and reaches an observed worker end. Cover-on cancellation-to-worker-end
time is slower: 15.360 [13.606–15.600] to 21.356 [17.591–22.005] ms. Cover-off
improves from 32.358 [25.850–54.323] to 17.238 [16.983–21.766] ms. These are
host cooperative-cancellation observations, not immediate-preemption guarantees.

With cover on, raw complete worker pairs give a cover/Reader wall-envelope
intersection of 1465.654 [1439.944–1983.719] ms before and 0 [0–0] ms in R4.
Each baseline run has three regular cover pairs; each R4 run has two speculative
pairs. The corresponding summed UI callback envelopes are 127.573
[126.687–160.764] and 0.410 [0.373–0.505] ms, with the same complete callback
pair counts. They include trace overhead and callback state work, not just an
image copy, and are not CPU-overlap measurements. R4 cover-off has no observed
cover worker/callback pairs, so those timings remain `null`, not zero.

All enabled-cover pairs retain an actual `1448 x 2048` cover after idle, and
the retained Reader frame/anchor survives reopen, idle, and controlled sleep/
resume. Cover availability after idle alone does not establish when admission
occurred. Only the five displayed source pages receive this raster comparison.

The independent [guest pixel comparison](verification/perf05-20261116/guest-pixel-summary.json)
passed one actual XTest process per variant. All ten quiet window captures
matched exactly at `1860 x 2480`, scale 2, and source-anchor equality was checked
privately. The final desktop executable is
`d350070ebc223eaf5c1ece38e7a6ed9b9f725080d2e70804315f0728c2a7993f`.
This separate single-run group establishes desktop input and pixel continuity,
not a repeated timing result. Foreground cancellation and the Kindle-only
sleep-cover route remain covered by the controlled-library groups.

## Interpretation limits

- Use independent-process medians and observed ranges. Three runs do not
  support a reliable p95, and frames within one process are not independent
  cold-start samples. Input hashing reads files before timing; the OS page cache
  is not controlled.
- The 1500 ms value is checkpoint-to-idle scheduling, not a fixed interval
  between external inputs. The rapid intents share the owner thread with real
  drawing. Superseded frames and publication counts remain visible; an average
  over published survivors is not the latency of all four requests.
- Worker preparation, nested profiles, UI import, and software drawing have
  distinct boundaries. Do not add overlapping spans. A zero UI copy count does
  not mean zero decode work or zero total allocations.
- Cover overlap is an intersection of worker wall-time envelopes, not proof
  of simultaneous CPU execution. A UI callback total is not an isolated image
  copy. A cover available after idle does not alone prove idle-only admission.
- Thirteen pixel checkpoints mean twelve Reader frames plus one sleep
  overlay. They cover five distinct displayed source pages, not an eight-page
  visible-raster oracle. Private anchor equality is verified without exporting
  reading metadata.
- These host results add no ARM ABI runtime, physical-device, firmware-emulator,
  native SEND, reflectance, or optical grayscale qualification. Existing immutable
  [software-dithering captures](SOFTWARE_DITHERING_VERIFICATION_20261116.md)
  retain their original identities and scope.
