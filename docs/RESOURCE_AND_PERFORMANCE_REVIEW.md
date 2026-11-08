# Resource and Performance Review

Reviewed on 2026-11-06 against application version `0.1.5`. This is a review of
options, not an implementation change. No application source, dependency,
resource budget, or data-retention policy was changed during this review.

The main findings are cancellation-related work overlap, session-long metadata
retention, avoidable idle work, and bounded image retention. Static inspection
did not establish an unconditional per-operation FD, FFI-output, or private-key
leak. That observation does not prove that every resource path is leak-free.

## Recommended order

| Priority | Finding and trigger | Proposed change | Required protection |
| --- | --- | --- | --- |
| P1 | Cancelling a download can release its outer slot while source-level decrypt, sign, or complete-image decode continues in an already-started `spawn_blocking` closure. Rapid cancel/retry or account replacement can overlap old and new work. | Put a shared compute/memory lease inside the actual blocking closure; make queued obsolete work cancellable before large allocation. | Avoid nested-permit deadlocks. Preserve prompt network cancellation, task generations, account restrictions, and complete-image validation. |
| P1 | An offline or empty-URL missing cover can repeatedly schedule a local file miss, deliver an event, dirty the UI, rebuild models, and schedule the same miss again. | Add a distinct negative cache for unavailable local covers, invalidated on URL/account/connectivity changes or successful publication. | A temporary offline miss must recover later; do not make it a permanent failed-cover flag. |
| P2 | Explicitly closing a reader drops `ReadingState`, while the global `ReaderModel` still holds the last page, second page, overview, thumbnails and chapter model. | Clear those UI references on explicit reader close. | Keep menu dismissal and physical sleep separate: neither may discard the live reading frame. |
| P2 | Panel/strip steps reread and decode the source; Panel prepares its overview with another complete source render. | Reuse one verified gray source within a render job for the main frame and overview, then evaluate a byte-bounded cross-call cache. | Retain exact encoded identity, corruption handling, source anchors and layout invalidation. Do not remove integrity checks for speed. |
| P2 | Completed request/control entries, retired reader IDs, and completed application pending keys remain for the manager/account lifetime. | Prune safe completed request metadata and acknowledge terminal events; define a generation/lease mechanism before compacting retirement history. | Missing controls currently allow events through the filter. Blindly deleting controls or LRU-evicting retirement markers can admit stale work or revive closed owners. |
| P2 | Download and App result channels, plus the native closure queue, have no explicit capacity. Suspended or slow consumers can retain stale results and large pixel buffers. | Coalesce progress and obsolete render/cover results; bound noncritical result work and release stale data before enqueue. Check native quit and enqueue under the same queue lock, clearing pending outside the lock. | Financial outcomes must be durably preserved. Do not abort or drop a submitted purchase/recharge result merely because its UI became stale. Avoid blocking sends while holding publication locks. |
| P2 | Prepared cover PNG files are outside the temporary-page eviction budget and have no corresponding automatic eviction path. Old URL variants and crash temporaries can accumulate. | Wire cover storage to an encoded-file quota/LRU and explicit cleanup, with ownership-safe temporary recovery. | Preserve retained comic pages and live reader pins. Keep signed URL semantics; do not strip query strings to invent cache identity. |
| P3 | Every dirty update refreshes all six view models. Page commits also reload and aggregate the chapter receipt list; suspended ticks attempt local flush/checkpoint every second. | Introduce per-view dirty state, reduce repeated receipt aggregation/connection setup, and retry idle persistence only while work or errors remain, with backoff. | Keep final whole-index completion validation, durable writes, wake acknowledgement deadlines, and unknown-transaction recovery. |
| P3 | LIPC timeout handling has early-error child-reaping gaps and stdout reads outside the nominal deadline. Fixed signing assets are reread/rehashed and WASM runtimes rebuilt per call. | Use an owned child guard with a whole-operation deadline/output bound; separately measure immutable signing-asset/module reuse. | Preserve process identity, cleanup on every exit, cryptographic isolation, and sensitive-buffer erasure. |

## Source evidence

Download slots and work lifetimes are in [downloads.rs](../src/downloads.rs),
particularly `acquire_page`, its cancellation `select`, the later blocking
verification/commit, `Drop`, and `drain_events`. The source-level blocking
operations are in [images.rs](../src/source/images.rs) and
[client.rs](../src/source/client.rs). Moving the slot into the manager's later
commit worker is already correct; it does not cover the earlier source closure
after its awaiting future is dropped. A started Tokio blocking closure is not
stopped by dropping its handle. This is potential overlap/backlog, not evidence
that its buffers survive after the closure finishes.

Reader close/render/thumbnail ownership is in
[reading.rs](../src/app/reading.rs). Per-reader work has local cancellation and
one-worker/again guards, but no global cross-reader compute limit. Reader layout
metadata is bounded to eight remembered pages and 512 regions per page;
explicit panel metadata is also bounded. The image-integrity memo holds at most
128 successful metadata proofs and retains no decoded bitmap. The single-call
`decode_image` path already reuses its validation decode; the remaining repeated
decodes occur at the reader/overview level, not inside that helper.

The cover miss loop is visible in [mod.rs](../src/app/mod.rs): `cover_image`
schedules `cover-local`, the callback removes `cover_pending`, and `request_cover`
returns for offline/empty URLs without negative state. `tick` still marks the
valid event dirty; `refresh_views` refreshes all models, and the next model
construction schedules the missing cover again. `prepare_covers` also ignores
`Ok(None)`. A cached cover terminates the loop through `cover_images`.

`pending` is not removed after an accepted JobEvent finishes; distinct cover/job
keys can remain until invalidation or account clearing. Same-key replacement is
not itself unbounded growth. `cover_images` has a 64 MiB pixel-byte budget;
metadata and references held by current UI models are outside that accounting.

Prepared covers are written under `data/covers` in
[mod.rs](../src/app/mod.rs). The bounded [FileCache](../src/cache.rs) implementation
is not wired to this App cover path. `evict_account_cache` and `unique_cache_bytes`
in [account.rs](../src/app/account.rs) manage automatic comic-page receipts, not
these cover files. Covers contribute to total directory storage but not the
temporary-page quota or its cache-byte figure. Cover metadata maps and failure
sets likewise have no independent entry cap.

Per-page receipt aggregation is in [store.rs](../src/store.rs); it reloads all
matching receipts to derive counts/bytes after each page. This is a candidate
for reducing repeated work on long chapters, rather than a correctness reason
to remove the final file validation. The periodic suspended flush is in
[lifecycle.rs](../src/app/lifecycle.rs). Its battery impact was not measured.

Native queue, child-process, and FFI ownership were inspected in
[wakeup.rs](../vendor/slint-backend-kindle/src/wakeup.rs),
[platform.rs](../vendor/slint-backend-kindle/src/platform.rs),
[native.rs](../src/platform/native.rs), and
[crypto.rs](../src/source/crypto.rs). FFI output guards and C cleanup cover normal
and error returns, and the last sensitive-key owner erases its buffer. Native
framebuffer mappings are reused and touch files use explicit lifecycle/RAII
cleanup. Shared HTTP pool sockets and one-time font registration are intentional
retention; they cannot be labelled leaks solely because they stay resident.

## Bounded remote observations

All probes ran on `ssh test-env` using the existing sealed desktop executable
SHA-256 `5a8d3bd15e07163186d5c413068eed5afb981e3c79a4504ba453a148dee84fc4`.
No Cargo, real account, remote service, or Kindle was used. Each App ran offline
inside a network namespace, with an absolute process limit and a 768 MiB App RSS
safety ceiling. All three observed application identities exited and were absent
at cleanup.

A scripted burst submitted 20 open/close cycles and 60 intents. The trace shows
20 reader and 21 shelf UI states, but zero `reader-ready` events: this is a rapid
cancellation workload, not a fully rendered reading-session benchmark. FD count
stayed 31 and peak threads were 11. Median warm RSS was 192,892 KiB (about
188.4 MiB), rising to 245,960 KiB (about 240.2 MiB) during activity and remaining
near that level during the bounded idle tail. Allocator arenas, UI references,
and caches can retain high-water memory; this single trace does not identify a
permanent leak or attribute that difference to a particular object.

A separate pair used normal guest/offline reader data and changed only whether
the empty-URL cover's local cache file was present. Both reached a real ready
reader. Over approximately 7.27 seconds after startup settled, the missing-cover
process used 0.28 CPU seconds versus 0.09 CPU seconds with a cached cover. Both
held 32 FDs; median threads were 12 versus 11. The cached-cover process had higher
RSS because it also retained the decoded cover. This one pair is consistent with
the statically established idle miss loop, but it is not a statistical speedup,
battery, or hardware-memory measurement.

The [safe evidence export](../artifacts/verification/resource-review-20261106/safe-export/evidence-manifest.json)
indexes 16 payload files plus the manifest. Raw samples, actions, process/birth
identities and the cover-pair probe source are retained. The initial burst probe
was supplied through remote Python stdin; its actions and observations are
retained, without claiming a captured standalone sampler source for that run.
Runtime databases, sessions and final window captures were excluded from export.

The most useful first implementation batch is the source compute lease, missing
cover negative state, and explicit reader UI-resource release. Source reuse,
queue coalescing, metadata pruning, cover disk eviction and lower-frequency
maintenance can follow with targeted comparisons. None was implemented by this
review, and no existing completeness, durability or sleep guarantee was weakened.
