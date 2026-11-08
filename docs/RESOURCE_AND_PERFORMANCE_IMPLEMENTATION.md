# Resource and Performance Implementation

Status: **complete for the controlled client verification scope** for application version `0.1.6`,
recorded on 2026-11-06.

This document maps the authorized implementation to the nine rows in the earlier
[resource and performance review](RESOURCE_AND_PERFORMANCE_REVIEW.md). That
review remains the historical `0.1.5` options and observation baseline. The
implementation is frozen in the R4 candidate below. Core, desktop interaction,
controlled power signals, resource cycles, protocol/download integration, and
ARM packaging have passed their separately scoped checks. This is not a measured
battery, live-service throughput, or physical-device performance claim.

## Candidate identity and evidence status

| Item | Status |
| --- | --- |
| Application version | `0.1.6` in the current Cargo manifest |
| Frozen implementation source manifest and SHA-256 | R4, `702c78ab245e21b433d9846cbf356c412d22a043d92219bfad986553142d0f6f` |
| Tested desktop executable SHA-256 | `b0b51e4291ad2624e0a67e134b67ee7929a9163dc81bade8f3d8f13c715dcec3` |
| Verification environment | `ssh test-env` only; no local verification is authorized by this report |
| Core tests | 437 passed; zero failed or ignored, with inputs unchanged |
| Native backend component | 62 passed; zero failed or ignored. Candidate backend sources, JPEG patch, and production lock match the tested standalone inputs. |
| Desktop regression | 23 renders, 8 pointer cases, 9 reader cases, 5 guest cases, 2 startup cases, controlled launcher lifecycle, and 28 launcher fixtures passed. |
| Controlled sleep maintenance | Real SignalBridge suspend, failed anchor checkpoint, background recovery before resume, stable write count, retained reader, and cleanup passed. |
| Complete reader resource cycles | Two isolated offline cases each completed 20 close/reopen cycles with 21 actual ready generations; all 40 close observations cleared global image/model references. |
| Targeted static visual review | Nine listed software-fixture PNGs reviewed; no visible defects found. This is not review of every captured state. |
| Protocol and recovery | Five official-module cases, seven Go/JavaScript signature comparisons, two production subprocess cases, and fixture/native-signer 45-page acquisitions passed. |
| Kindle feature on the host | Three native-platform tests and the Kindle-feature binary compilation check passed without device access. |
| ARM development package | 28 entries and all nine static checks passed; ARM binary `50fce38befabc0481c9a57ffe043e536f5cb17ce800c10dd87432a00204811cb`, ZIP `b2ed7e4fce15d55e8fa1f8d4af1b9d0624d7afa14ec736c82ce70e2b20335f07` |
| Physical acceptance | No ARM execution, firmware emulator, real account, or Kindle result is claimed for this implementation. |

The earlier `0.1.5` resource samples do not verify these changes. Their 20-cycle
burst had zero `reader-ready` events and was a cancellation workload, not a
completed-reader retention test. Its RSS high water was not proof of a permanent
leak, and its single missing/cached-cover pair was not a statistical speedup or
battery result. The `0.1.4` Scribe physical results also retain their original
candidate and behavior scope. Neither result is transferred to `0.1.6`.

WeRead has confirmed native-control recovery and release of its physical-test
lease. This implementation verification has not acquired a new device lease or
operated the Kindle. Any later physical acceptance requires a fresh exclusive
handoff.

## Coverage of the nine review options

The rows describe the frozen implementation. The verification results and their
remaining limits are recorded separately below.

| Review row | Current implementation candidate | Protection and remaining boundary |
| --- | --- | --- |
| 1. Cancellation and source blocking budgets | [Source work admission](../src/source/work.rs) shares two CPU slots and a 384 MiB planned-memory reservation pool. Source signing, response decoding, image key/decrypt/decode, and App image preparation use this path. The lease belongs to the actual blocking closure; cancelling its waiter does not release a running closure's reservation. Queued work checks cancellation before entering its callback. | Started noninterruptible operations still have to return. This is admission accounting, not an RSS ceiling or a promise to stop native code immediately. Source/download cancellation passed the controlled regression cases. |
| 2. Missing-cover rescheduling | [Cover state](../src/app/covers.rs) tracks account, full-URL hash, generation, pending work, and distinct negative reasons. Offline local misses are suppressed until a bounded recheck or a relevant context change. Empty URLs and missing demo mappings wait for identity/context changes rather than repeatedly probing disk. Transient download/storage failures back off. | Account, URL, connectivity, or successful publication can release negative state. Admission pressure is a deferred retry, not a permanent failed-cover flag. Recovery tests and the bounded local-cover idle pair passed. |
| 3. Explicit reader close | [Reader close](../src/app/reading.rs) saves the position, retires reader work, drops `ReadingState`, and clears global `ReaderModel` page, second page, overview, thumbnails, and chapter references. | Menu dismissal and physical suspension retain the live reader. The new close diagnostic records actual UI image sizes and row counts; its existence alone is not a passing retention test. |
| 4. Repeated reader/overview decoding | [Reader rendering](../src/reader.rs) and [frame preparation](../src/app/reading.rs) share a validated unrotated gray source between the main image and panel overview. A [32 MiB, 128-entry gray-source cache](../src/reader/resources.rs) permits bounded reuse across calls. Thumbnail updates compare actual image identity, with a special case only for empty Slint images. | Reuse remains tied to current encoded bytes and a sealed validation proof, including digest, length, geometry, and validation version. Corruption handling, layout changes, anchors, and existing layout limits remain required. Oversized valid sources can bypass retention. Pixel equality, corruption, and cache-eviction regression cases passed. |
| 5. Completed metadata and reader retirement | [Download ownership](../src/downloads.rs) replaces a permanent retired-ID set with a retired high-water mark plus explicit active leases. Completed controls/requests and unprotected terminal prefetch metadata can be reclaimed. [App jobs](../src/app/jobs.rs) retire accepted or discarded latest-key entries. | A newly registered owner must exceed the retirement high water; a lower owner already active retains its lease. Retirement invalidates leases and removes associated partial intents/pins. GC does not delete comic receipts, identity history, source indexes, or commit journals. Active tables still scale with actual active owners/tasks; no constant-size global task claim is made. |
| 6. App, download, and native result queues | [App mailboxes](../src/app/jobs.rs) bound ordinary keys, durable financial admission, and delivery leases. [Download events](../src/downloads.rs) coalesce bounded transient progress and retain terminal recovery through schema 4 task markers and a [SQLite outbox](../src/store.rs). The [native queue](../vendor/slint-backend-kindle/src/wakeup.rs) rejects overflow and serializes quit with enqueue. | Financial observations are preserved independently of stale UI delivery. Terminal replay is paginated; outbox storage is not a global fixed-row or byte quota. Native closure counts are not memory-byte bounds. Queue saturation, account churn, persistence failure, and quit races passed their scoped fixtures. |
| 7. Cover resident and disk storage | [Resident cover caching](../src/app/covers.rs) accounts for 64 MiB of map-owned RGBA pixels and limits cover observation metadata to 512 entries. [Disk maintenance](../src/app/covers/disk.rs) targets 128 MiB and 1,024 managed PNGs, updates access times in background batches, and removes only recognized new-format temporaries whose owner is proven dead. | Cache filenames hash the full URL, including its query. Regular-file, ownership, link/reparse, directory, and file-identity checks protect replacement and removal. Alive/unknown owners and unrecognized legacy temporary names are preserved. In-flight protected files can defer eviction; these are maintenance targets, not instantaneous total-disk or process-RSS bounds. |
| 8. Hidden views, aggregation, and idle checkpoints | [Per-view dirty state](../src/app/views.rs) refreshes the visible top-level view and forces refresh on entry. [Catalog refresh](../src/app/catalog.rs) also gates shelf/store/search/detail internally and separates search input from results. [Receipt progress](../src/store.rs) uses SQL counts/sums rather than materializing every receipt in Rust. [Suspended persistence](../src/app/lifecycle.rs) retries actionable failures with backoff. | The SQL indexed aggregate still scans the descriptor array; it is not constant-time progress accounting, and worker connection setup has not universally disappeared. Final whole-index file validation remains a completion requirement. Successful idle checkpoints stop repeating; new observed receipts can request prompt local persistence. Controlled UI and sleep regressions passed within the scopes below. |
| 9. Native process deadlines and signing assets | [Native command ownership](../vendor/slint-backend-kindle/src/command.rs) uses one shared child limit and one reaper, nonblocking bounded stdout, a common operation deadline, and owned child/process-group cleanup. [Signing assets](../src/source/assets.rs) reuse a digest-checked immutable byte snapshot; the [FFI caller](../src/source/crypto.rs) holds it across each invocation. | The C host still creates, parses/loads, and destroys fresh mutable runtime state for every call. No shared mutable WASM runtime, Go globals, account state, or private key is introduced. Controlled native cleanup and crypto-equivalence checks passed; no battery or cryptographic-throughput improvement is inferred. |

## Budget accounting and queue semantics

The current constants are scheduling and retention policies. Independent limits
must not be added together and presented as a measured maximum RSS.

| Resource | Current policy observed in source |
| --- | --- |
| Shared source/App compute admission | 2 CPU slots; 384 MiB of planned in-flight reservations |
| Heavy reader, signing, complete decoding, and selected recovery work | 192 MiB reservation per admitted closure |
| Cover preparation | 80 MiB reservation |
| Small metadata/key work | 4 MiB reservation |
| Source image decrypt and asset checking/publication | Currently 64 MiB reservations; these are not all charged as heavy work |
| Gray-source retention | 32 MiB of cache-owned pixel-buffer capacity; at most 128 entries |
| Resident covers | 64 MiB of cache-owned RGBA pixel bytes; 512-entry cover bookkeeping policy |
| Managed cover disk files | Maintenance target: 128 MiB and 1,024 PNG files |
| App job admission | 128 ordinary latest keys; 16 durable in-flight operations |
| App delivery | 8 leases across regular execution and unconsumed UI results. Protected financial execution uses its separate 16-operation admission and persists before waiting for UI capacity. |
| Download transient progress | At most 256 entries, coalesced by task/generation and progress kind |
| Download terminal retry memory | At most 64 retry entries; durable task markers permit later recovery after transient eviction |
| Download reconciliation/replay | 64-task reconciliation pages; at most 128 active-reader tasks scanned per replay pass |
| Native event closures | At most 256 pending plus one detached execution batch of at most 256: at most 512 closures across those two containers |
| Native subprocess ownership | 8 shared child permits; permits remain held through delayed reaping |
| LIPC command policy | 2-second operation budget and 4,096-byte stdout bound |
| Orientation command policy | The same runner, with a 2-second budget and 64-byte stdout bound |
| Suspended failure retry | Starts at 1 second, doubles, and caps at 30 seconds; only pending failed writes or newly observed receipts are actionable |

Network response bodies and already captured inputs can exist outside a compute
reservation. UI models, allocator arenas, caches, and callers holding evicted
`Arc` values also have separate lifetimes. Gray-source and cover budgets count
what their own cache maps retain, not every remaining reference in the process.
An evicted gray source can remain live in an admitted render job. Likewise,
skipping a hidden Catalog model rebuild does not itself clear that model's old
image references.

The cover negative-state policy currently uses a 60-second offline local recheck,
30-second transient retry increasing to at most five minutes, and a short
deferred-admission retry. Empty-URL/demo-mapping misses do not have a periodic
deadline. Maintenance uses bounded candidate/removal batches and can schedule a
subsequent batch when protected objects or remaining excess prevent completion.

The schema 4 outbox keeps the current terminal event for each account/task and
supports acknowledged, ordered replay. Its persistent row count can scale with
unacknowledged tasks. The 64-entry retry buffer and pagination sizes are not a
claim that the SQLite outbox is globally limited to 64 rows. Ordinary progress
loss is repaired from current indexed state; it is not used as evidence that a
chapter finished. Reader leases and event generations remain part of admission
and stale-result filtering rather than optional cache hints.

The LIPC deadline is an operation budget, not an absolute wall-clock guarantee
for an operating-system spawn that cannot be interrupted. Deferred reaping may
continue after the command returns while retaining its shared child permit.
Cleanup is scoped to the owned child/private process group rather than broadly
terminating unrelated processes.

## Admission, local state, and shutdown

Capacity checks precede taking the download manager out of the App and precede
financial submission journal/UI transitions. A capacity rejection therefore
does not silently consume manager ownership or assert that an operation was
submitted. A submitted purchase/recharge observation is handled before its
optional UI result is published. Failed persistence remains retryable local
work; stale UI or account navigation is not a reason to submit the transaction
again.

Network approval now has its own verified account epoch. It does not depend on
a completed `network-check` key remaining in the job registry; invalidation and
account replacement clear that proof. Reader wallet labels use the App's actual
locally held wallet state rather than relying on a hidden `AccountModel` having
been refreshed. The Catalog local-inventory result separately invalidates the
Reader cache label without constructing hidden Catalog models.

The [main loop](../src/main.rs) adds a `reader-closed` diagnostic containing live
UI image dimensions and thumbnail/chapter row counts, as well as gray-cache
accounting. This permits later tests to distinguish controller teardown from
real global-model resource release. A valid retention comparison must first
observe a rendered `reader-ready` state for each tested session.

Native suspension parks the backend without running ordinary Slint timers. The
[maintenance driver](../src/app/lifecycle_maintenance.rs) therefore uses a single
Tokio task with pending-only deadlines and receipt notifications. Its scoped UI
control callback captures RAM state and installs results; SQLite work runs on a
background worker. Resume and quit revoke older scopes, and the checkpoint
transaction checks revocation before writing and before commit. Download pause
rows are scanned in pages of 64, with RAM resume tokens staged before the CAS.
No input/display lease, network request, wake lock, or RTC wake is introduced.
Kernel-level suspension can still stop all user-space work.

Shutdown saves local state and flushes already observed financial receipts,
pauses downloads, checkpoints metadata, and closes new job admission. The main
loop uses a bounded runtime shutdown and then makes a final observed-receipt
flush. It does **not** wait for every network operation or infer success for a
response that never arrived. Pre-submission/unknown journals and explicit
reconciliation remain the recovery mechanism for uncertain financial outcomes.

Immutable signing reuse is limited to provider-private, digest-pinned asset
bytes, currently a single cached snapshot with a 16 MiB asset-size limit.
Observed asset identity changes or invalid/missing assets invalidate that
entry. The [native host](../vendor/bili-protocol/host/biliwasm.c) still constructs
fresh Host/Job/environment/runtime state and clears per-call state. The candidate
does not cache a compiled mutable WASM module or runtime.

## Verification

The [candidate record](../artifacts/verification/resource-optimizations-20261106/candidate.json)
binds the sealed executable to the frozen source manifest. The
[core result](../artifacts/verification/resource-optimizations-20261106/r4-core/evidence/20261106T071719Z-934bc577/summary.json),
[native component result](../artifacts/verification/resource-optimizations-20261106/native-r1c/summary.json),
[safe desktop result](../artifacts/verification/resource-optimizations-20261106/runtime-r4-safe/summary.json),
and [controlled sleep result](../artifacts/verification/resource-optimizations-20261106/sleep-r4/results.json)
retain their own scope and identities. The desktop safe export excludes fixture
databases and materialized runtime data; its new manifest identifies the subset.
The [targeted visual review](../artifacts/verification/resource-optimizations-20261106/visual-review.json)
records the nine images actually inspected without changing the original
automation's visual-pending fields.

The [resource observations](../artifacts/verification/resource-optimizations-20261106/resources-r4/results.json)
used the same complete fictional page during an exclusively arranged sampling window; WeRead explicitly paused its Cargo work. In both cases the
cache retained one 4,012,000-byte gray source within its 32 MiB budget. After the
initial RSS increases, ready-reader RSS plateaued at about
226.3 MiB without a cover and 228.6 MiB with a cached cover. Initial stable FD
counts were 31 and final stable counts were 32; all-phase peaks were 33/34.
Peak threads remained 11. These bounded samples do not establish that every
resource path is leak-free or attribute the retained RSS to a particular owner.

Requested idle windows were eight seconds, with about 7.8-7.9 seconds sampled.
Missing/cached-cover CPU observations were approximately 1.77/1.54 percent of
one core initially and 1.65/1.67 percent after cycling. The pair controls local
file presence, not network cover publication. It is not a statistically
established speedup or a battery result, and it is not directly comparable to
the historical zero-ready cancellation burst. Both App/Xvfb pairs exited and
their recorded process identities were absent at cleanup.

The implemented regression coverage includes:

- Started and queued blocking cancellation, retained reservation lifetime,
  saturation, account replacement, and nested-admission deadlock avoidance.
- Missing-cover suppression and recovery after URL/account/connectivity changes,
  transient failure, capacity pressure, quota eviction, and safe temporary cleanup.
- Fully ready reader open/close cycles with actual UI-reference diagnostics,
  exact-pixel/layout/anchor equivalence, thumbnail identity changes, bounded cache
  eviction, and corruption/resource-limit behavior.
- App key churn and admission rejection, stale delivery, financial persistence
  failure, bounded download progress overflow, durable terminal replay after
  restart, and high-water reader ownership without reviving closed owners.
- Visible-route navigation, search keyboard/candidates/results, offline startup,
  detail selection, suspended retry backoff, native quit/enqueue races, child
  cleanup/deadlines, and signing/response equivalence with isolated runtimes.
- Existing completeness, recovery, purchase-uncertainty, startup, and sleep
  regressions within their separately identified fixture/component scopes.

Any performance or retention comparison must use the same frozen candidate and
controlled workload, report whether readers actually became ready, retain raw
observations and cleanup identities, and distinguish allocator high water from
unbounded growth. No battery saving, live-service throughput, real-account
download, or physical-device result is supplied by these controlled checks.

## Protocol, acquisition, package, and provenance acceptance

The [additional checks](../artifacts/verification/resource-optimizations-20261106/additional-r4-safe/summary.json)
passed the Kindle-feature host compilation and three native-platform tests,
five official-module protocol cases with seven matching official Go/JavaScript
signatures, and two controlled production subprocess cases. Both fixture and
native signers completed the production adapter's 45-page acquisition in legacy
and batched modes, four cold repetitions per mode, with exact bytes, pixels and
receipts. Complete cache reopens made zero requests. The payloads were synthetic
PNG images; this does not verify a real encrypted account comic or a full book
against the official client. Timing retains its injected-transport scope.

The [manifest mapping](../artifacts/verification/resource-optimizations-20261106/manifest-equivalence.json)
checks all 419 candidate path/digest pairs. Additional/protocol/production
manifests contain the same 419 pairs in another order; the batch manifests add
34 pipeline files and three derived Python bytecode files. Their different
whole-manifest SHA values are preserved rather than treated as identical hashes.
Every required candidate file matches, with no missing or changed path.

The [ARM development package](../artifacts/verification/resource-optimizations-20261106/arm-r4/bilicomics-kindle-0.1.6-dev.zip)
contains 28 allowed entries and passes all nine
[static checks](../artifacts/verification/resource-optimizations-20261106/arm-r4/package.json):
file CRC/digests/modes/timestamps, empty acceptance allowlist, notice provenance,
selected binary identity, ARM EABI/hard-float layout, and no dynamic interpreter
or required shared libraries. It was not executed. The actual build log retains
the JPEG attribute, deprecated atomic API, unused cover helper, and compiler
wrapper/linker diagnostics; Cargo and the static checker completed successfully.

Early R1/R2/R3 failures remain separate: small covers were enlarged unnecessarily,
one budget test assumed a partial semaphore allocation was still available,
scope allocation needed a proper error conversion, and a notification burst
needed coalescing before UI capture. The old generic missing-module error-code
expectation was tightened to explicit missing/integrity errors with unsent
assertions. None of those earlier failed candidates is relabeled as passing.

The [aggregate result](../artifacts/verification/resource-optimizations-20261106/summary.json)
collects the final source/binary/package identities and scoped counts. All Cargo
and packaging work has finished; the compile window was explicitly returned to
WeRead. This work acquired no new physical-device lease and performed no Kindle
operation.
