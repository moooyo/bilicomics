# Download Batching and Verification

Updated on 2026-11-05 (Asia/Shanghai).

This record retains the batching implementation and controlled measurements
from its original source snapshot. The subsequent [0.1.5 completeness review](DOWNLOAD_COMPLETENESS.md)
adds fresh permission metadata requests and complete decoded-image validation.
The request totals and timing medians below are historical and are not
reassigned to that newer implementation.

## Implemented acquisition path

The production download adapter now prepares up to 20 missing source pages with
one `ImageToken` request. The scheduler still downloads and commits each image
individually, using the existing user-selected concurrency of one through four
(default two). Each prepared batch owns its P-256 key and original page mapping.
Signing URLs and keys remain transient and are never stored as download state.

For a complete uncached 45-page chapter, the expected path changes from one index,
45 token requests, and 45 image requests to one index, three token requests
(20/20/5 pages), and 45 image requests. The batch size is the reviewed client's
bound; this is not a new claim about an official service limit or archive API.

Preparation and page acquisition share one capacity budget. Within the same
retained priority, ready pages are admitted before another chapter's preparation;
the existing reader fairness and explicit download priority rules remain active.
Page tickets are registered before the preparation permit is released, so a
queued chapter cannot fill the handoff gap with more metadata work. This bounds
index/token preparation instead of admitting every queued chapter at once.

The scheduler rechecks verified receipts before each batch. Selected cached pages
are reused, corrupt pages are invalidated, and a reopened incomplete task prepares
only its missing pages. Each completed page still receives its individual durable
file and SQLite receipt before progress is published. Integrity checks, `fsync`,
task generations, account separation, and source revision checks are retained.

Image-header validation and hashing run in the existing blocking commit worker,
before the publication lock. This does not claim that every pixel is decoded.
That worker owns its admission permit until it exits, even if its async caller is
cancelled. Existing lower-level crypto helpers may still finish uncancellable
blocking work after cancellation; the admission result is not a universal bound
on every underlying CPU worker or worst-case resident memory.

## Grant lifetime and cancellation

A local monotonic 60-second freshness budget prevents indefinite reuse after a
long queue wait. This budget is not the server's declared token lifetime. Refresh
is coalesced within the prepared object and excludes pages already acquired.
The first age refresh is allowed; later refreshes require additional completed
pages, and the total is bounded by the original batch's page count. A failed
refresh is not repeatedly submitted, and HTTP 403 does not trigger a blind retry.

In-flight images hold their own cloned grant and key. Refreshing another page
cannot replace that key. The original account epoch is checked before and after
refresh, after network acquisition, and after image inspection. A logout, explicit
session replacement, or account switch cannot revive an old grant through refresh.
Pause and generation changes discard the acquisition plan; a later resume obtains
fresh grants from the retained stable descriptors and verified receipts.

State-change and failure updates use the existing publication lock so an older
task snapshot cannot overwrite the progress of a page committed concurrently.

## Verification scope

Verification runs only on `ssh test-env`, with a single build job and controlled
synthetic accounts and images. The 45-page benchmark compares a compatibility
source using the former per-page contract with the production batch adapter under
the same current manager. It does not compare two historical application binaries.
Balanced execution order, request counts, durable receipts, hashes, decoded fixture
pixels, cold timing, cache reopen, and partial recovery are recorded separately.

The optional native mode executes the digest-pinned official signing WASM. The
transport remains a controlled fixture inside an isolated Linux network namespace;
network delays are injected. Neither fixture timing nor native signing timing
establishes live Bilibili acceptance, real network throughput, physical Kindle
speed, battery use, or a hardware memory bound. No real account, acquired comic,
purchase, firmware run, or physical Kindle connection is used in this review.

## Recorded results

All checks below passed on `test-env` from the frozen application snapshot:

| Check | Result |
| --- | --- |
| Locked offline compilation and desktop build | Passed without warnings |
| Application library suite | 222 passed, 0 failed, 0 ignored |
| Production-adapter batch regressions | 17 cases, included in the library suite |
| Scheduler batch regressions | 8 cases, included in the library suite |
| Download interruption, repair, offline reading, and startup-intent integration | 2 passed |
| Fixture-signing batch benchmark | Passed, four cold runs per mode in balanced order |
| Official-WASM-signing batch benchmark | Passed, four cold runs per mode in balanced order |

Both benchmark modes produced exactly the same 45 image payloads, decoded fixture
pixels, and durable receipt content. Cold acquisition used 45 token requests in
the single-page source and three requests of 20/20/5 pages in the batch source.
CDN image requests remained 45. Total requests fell from 91 to 49; the approximately
93% reduction applies to token requests specifically. Key generation fell from
45 to three operations, and index-plus-token signing calls fell from 46 to four.

| Controlled condition | Single-page median | Batch median | Elapsed reduction |
| --- | --- | --- | --- |
| Injected signer, token delay 20 ms, CDN delay 5 ms | 1,043.006 ms | 736.844 ms | 29.35% |
| Official signing WASM, token delay 0 ms, CDN delay 5 ms | 1,268.405 ms | 742.208 ms | 41.48% |

The official signing module was provisioned separately and checked against
SHA-256 `39bc0676953752c461197df592e1f5894f1a7492a29400c946e560fc109a8e2e`.
The timing interval includes production acquisition, controlled response delays,
image inspection, and durable page writes; fixture generation and final readback
are excluded. Complete cache reopens made zero requests. A four-page selection
followed by reopening fetched exactly the remaining 41 pages, and the separate
SIGKILL fixture reused the committed first page, repaired only the corrupt page,
and preserved the offline raster and anchor.

Evidence is under `artifacts/verification/download-batching-20261105/`: the `r1`
core summary, `batch-fixture`, `batch-native`, `production-flow`, and
`batch-identity.json`. A remote comparison confirmed equality of all 339 core
source path/hash pairs between core and service-flow runs. Both 367-file benchmark
pipeline manifests are equal and include those same core inputs. The core manifest
SHA-256 is `19e4eeadb21394d5481487e3391a2a0dc46a3dd678525e2d01c6dafc55d44bdb`;
the sealed desktop executable is
`99121bb7fa8073baa572544c09f5589ea7c1f5f69477c34583f49eaa97956f45`.
No verification process remained in the isolated workspace after completion.

The original benchmark runner's top-level `network_latency` label says
`injected_token_response_delay_only`. That label omitted the fixed 5 ms CDN delay;
the nested timing fields and tables above specify both delays correctly. Raw
results retain the executed runner's identity. A metadata-only wording correction
in the current runner makes both injected delays explicit; application inputs and
measurement logic are unchanged. The accompanying `measurement-erratum.json`
records this clarification without changing the historical reports.

Earlier cookie, firmware, and ARM package evidence retain their original source
identities. At the close of this performance review, no replacement Kindle
package had been built. A subsequent fresh 0.1.3 candidate and supervised Scribe
5.19.5 offline run are now recorded in [physical verification](PHYSICAL_VERIFICATION.md).
That UI/input/sleep result does not measure live download speed, and the later
immersive-reader work retains its own source and verification scope.
