# Download Completeness Audit

Updated on 2026-11-06 (Asia/Shanghai). Candidate application version: `0.1.5`.

The final R2 candidate passes the controlled client-completeness checks below.
This document distinguishes those results from service behavior that has not
been independently verified. Original failed runs and the corrected loading
fixture retain separate records.

## Three different scopes

| Scope | What the application can establish | What it does not establish |
| --- | --- | --- |
| Comic and chapter selection | The selected chapter identities that become individual retained tasks. Authentication return preserves the selection and rechecks it against fresh permissions. | That every chapter in the comic was selected, or that the returned catalog is an independently verified complete official catalog. |
| One chapter's image index | Every ordered page in the bound `ImageIndex` has its matching receipt and checked local file before the chapter is completed. | That a structurally valid index returned by the service includes every official page rather than an authorized subset or preview. |
| One encoded image file | The supported static image's container and declared pixels pass bounded complete decoding, and its digest identifies the exact checked bytes. | That those bytes depict the expected upstream page, or that local decoding proves the service delivered all intended content. |

The chapter selector currently displays seven or nine entries per local UI page,
depending on the layout. Its readable-page selection operation affects only that
visible chapter page. It is not a whole-comic selection operation. A comic-level
offline badge or downloaded filter can mean that some retained chapters are
available; it must not be interpreted as a full-book completion certificate.
Download percentages describe their current chapter tasks and indexes.

## Index traversal and the twenty-page batch

The production image adapter iterates every entry in the returned `images`
array, preserving its order. It does not take only the first twenty entries.
The scheduler walks all missing positions through `missing.chunks(...)` and
continues after each batch. Twenty is the existing reviewed bound on a single
`ImageToken` request, not the chapter length or a whole-book limit. The current
index safety bound rejects unsupported oversized indexes instead of silently
cutting them to a prefix.

An invalid index position, duplicate page identity, token count mismatch, or
missing final indexed page cannot satisfy the normal completed-index checks. Token
results still depend on the response's ordered mapping. Geometry checks alone
cannot detect two different same-sized pages being exchanged, and repeated
image bytes can be legitimate. A digest calculated from received bytes is a
durability and identity check, not an independent upstream expected-content hash.

The fixed protocol reference is `moooyo/bilicomics.koplugin` revision
`a05ffb987c2089ca389cb3462aafd3ee1c0fa36c`. Its index and catalog adapters also
iterate the returned arrays without a documented continuation request. That
source observation does not prove that the current service always returns a
complete catalog or full chapter. No new pagination parameters are inferred.

Relevant implementation: [source images](../src/source/images.rs),
[download scheduler](../src/downloads.rs), and
[fixed protocol notes](API_REFERENCE.md).

## Implemented corrections

These source changes are covered by the final controlled R2 checks below.
Their verification scope remains separate from real-service content acceptance.

| Previously identified failure | Candidate correction |
| --- | --- |
| A running retained prefix could swallow a later whole-chapter request. | Request coverage is checked instead of treating retained ownership as sufficient coverage. An explicit fresh whole-chapter request starts the appropriate new generation. |
| Receipt count could publish `Completed` before the final file check. | Page commits update receipts and progress; whole-chapter completion is a separate confirmation after the bound index's files and validation proofs are checked. |
| A valid image header and self-consistent digest could hide an invalid compressed body. | Acquisition, cache validation, and reading use the shared complete-decode validator rather than accepting dimensions alone. |
| Old container-only receipts could permanently bind an initially invalid image digest. | Legacy receipts remain unproven until actual decoding supplies an opaque proof. Exact invalid-encoding evidence permits the narrowly defined legacy repair below. |
| A descriptor could shrink or change while retaining the same revision. | Storage rejects changes to an existing revision's ordered descriptor. A checked source refresh binds a distinct revision through a durable generation transition. |
| Downloads accepted up to 64 million pixels while the reader accepted only 32 million. | The shared source-pixel ceiling remains 32 million; the reader's limit is not increased. |
| Non-object catalog rows could be silently discarded. | Malformed episode rows and invalid or duplicate identities are rejected instead of becoming a shorter successful detail response. Returned-array counts are not promoted into an independently verified service total. |
| Authentication return silently retained at most 256 selected chapters. | The in-memory return intent preserves selected identities; restoration still excludes identities not readable in the newly checked detail. |
| Queued or explicitly repeated downloads could reuse stale permissions and descriptors. | The production download adapter refreshes account-bound chapter access before requesting an index. Explicit fresh whole-chapter acquisition cannot finish from an unchecked old cache. |

The fresh production path obtains `ComicDetail` for the expected comic and
episode, then checks the current account and entitlement before `GetImageIndex`.
Missing, locked, unavailable, expired, or unknown access stops that acquisition
before the index request. This describes the production manager's
`image_index_for` path, not a new guarantee attached to every lower-level client
method.

Stable descriptors and existing pinned revisions remain distinct from mutable
acquisition grants. Refreshing a download does not silently replace a live
reader's pinned content with a different revision. Retained objects from older
revisions are not mixed into a newly bound revision to manufacture completion.

## Complete decoding and resource policy

The shared [image integrity module](../src/image_integrity.rs) enforces these
upper bounds:

| Bound | Value |
| --- | --- |
| Encoded image bytes | 64 MiB |
| Source pixels | 32,000,000 |
| Decode allocation budget | 128 MiB |
| Successful in-memory validation metadata | At most 128 entries |

These are acceptance and decode-buffer budgets, not a measured total resident
memory or Kindle hardware guarantee. Encoded input, streaming scratch, metadata,
codec workspace, concurrent tasks, and rendering state also consume memory.
Individual protocol branches and conservative format-specific preflights may
apply tighter limits. Being below the upper bounds does not guarantee that an
encoding is supported.

Every validation call hashes its current bytes again and reapplies the caller's
pixel limit, including a stricter cover limit. Only successful results enter the
bounded memo. Persisted validation versions allow already proven unchanged
objects to avoid repeating full decoding on every lookup; ordinary file size,
digest, identity, and geometry checks remain necessary. Failure results are not
memoized as successes.

| Format | Current acceptance boundary |
| --- | --- |
| PNG | Static images, including supported interlacing. Chunk CRCs and the terminal container are checked; concatenated IDAT data must reach zlib `StreamEnd`, pass Adler checking, consume the complete compressed input, and produce the exact expected filtered row bytes before pixel decoding succeeds. APNG is unsupported. |
| WebP | A static picture with complete RIFF and chunk boundaries, followed by full picture decoding. Animated payloads are unsupported. This is not a claim that every unused compressed padding bit is canonical. |
| GIF | One static image, complete blocks and trailer, frame bounds, declared pixels, an LZW end code, and EOF. Animation is unsupported. The underlying library may skip remaining compressed sub-block bytes after an end code; this is not a canonical-code-stream proof. |
| JPEG | Supported eight-bit, single full-component sequential scans and supported progressive images through the pinned strict decoder. Sequential component-separated or multiple-SOS coding, non-1x1 single-component sampling, twelve-bit precision, dynamic-height DNL, and other unsupported coding modes do not receive a success proof. |

The JPEG wrapper validates container boundaries and supported capabilities
before invoking the strict decoder. Within its supported sequential path, SOS
parameters must be `Ss=0`, `Se=63`, `Ah=0`, `Al=0`. The vendored patch distinguishes
real entropy bits from lookup padding, retaining ordinary baseline and
progressive decoding without allowing synthetic padding to supply missing
coefficients. The source pin, original licenses, pristine modified files, and
exact patch are retained in [the vendor record](../vendor/zune-jpeg/UPSTREAM.md)
and [patch notes](../vendor/zune-jpeg/PATCHES.md).

`Unsupported` and `ResourceLimit` are distinct from `Corrupt`. They do not
produce invalid-encoding evidence and cannot authorize the legacy-history
revocation described below. An unrecognized decoder error remains conservative;
the decoder's two known malformed-Huffman error forms have explicit corruption
classification in R2, verified by the retained assertions and matching evidence
checks in the passing remote library suite.

## Legacy cache and immutable history

A receipt migrated from an older schema does not become trusted merely because
its old digest, dimensions, and file length agree. A successful current decode
produces an opaque, versioned proof bound to that exact digest, length, and
geometry. Storage checks the current receipt again before promoting it, so a
concurrent retain or object replacement is not overwritten by an old snapshot.

Automatic removal of an untrusted legacy content identity is deliberately
narrow. The original bytes must still exist, their digest and length must match
the legacy receipt, and actual validation must produce matching invalid-encoding
evidence. The receipt and historical identity must still refer to that same
unproven object when storage performs the atomic revocation. A trusted current
decode proof cannot be downgraded or revoked through this repair path.

If the original bad bytes have already disappeared, or the current disk bytes
no longer match the historical digest, there is no evidence that the historical
identity was itself invalid. The expected identity remains. A routine retry does
not automatically forget it to accept different content. Explicit removal of
that retained revision, or a separately checked new source revision, is the path
for intentionally changing content identity.

The frozen scheduler change treats an existing receipt or pending journal whose
bytes are unsupported or exceed the current resource policy as validation
unavailable. It preserves the encoded object and its receipt or journal
reference and history, without minting a new proof, returning a readable path,
or publishing `PageReady` or `Completed`. A matching non-refresh task that was
queued, running, or completed becomes failed; paused and cancelled tasks remain
incomplete. A stale inspection does not downgrade a newer object or receipt.

Recovery retains an unavailable pending journal even on a later pass where the
failed task is no longer eligible to commit it. The existing receipt and journal
references protect the original object from the existing garbage collector;
historical identity alone is not turned into a general file-retention reference.
An unsupported newly acquired network packet does not gain a new persistent
cache entry. Ordinary missing-file, digest-mismatch, and proven-corruption
recovery rules remain distinct.

This preservation change and its three synthetic regressions are a post-R1 source
delta included in the passing R2 library suite. Catalog preparation also checks
that an unavailable chapter leaves its original data intact while other valid
chapters remain available.

Relevant implementation: [receipt proofs and migration](../src/store.rs),
[local file inspection and recovery](../src/downloads.rs), and
[reader decoding](../src/reader.rs).

## Verification record and limits

| Evidence | Recorded status |
| --- | --- |
| R1 locked offline core check | Passed. |
| R1 application library suite | 298 passed, 2 failed, 0 ignored. |
| R1 failed cases | Two JPEG corruption-kind assertions: missing progressive DC entropy and a missing sequential block hidden behind alignment padding. |
| R1 runtime phases | Not executed after the core dependency gate. |
| R2 locked offline check, library suite and desktop build | Passed; 304 tests passed, zero failed or ignored. |
| Long-index, tail and migration regressions | Included in the 304 tests: independent 21/41/57-page manifests, withheld or failed tails, running subset expansion, fresh 21-to-57 revision refresh, proof migration, and unavailable-original preservation. |
| Selection and permission regressions | Included: 600 readable chapters across UI pages and authentication return, malformed directory rejection, current permission checks before index acquisition, and temporary-expiry handling. |
| Production source/cache subprocess and startup-intent integration | Two cases passed; committed data reused after SIGKILL, corrupt data repaired, offline raster retained, and synthetic unknown transaction intentions not replayed. |
| Production batch adapter | Four cold runs, two per source contract, each verified all 45 image payloads, decoded fixture pixels and durable receipts; batches were 20/20/5. |
| Reader/startup/lifecycle regression | Nine reader cases, two startup cases and the controlled real-process retained-reader fixture passed. |
| Corrected guest fixture | Five cases passed with zero loading observability gaps using the same sealed desktop executable. |
| ARM development package | Fresh 0.1.5 build and 28-entry static archive/ELF checks passed; ARM was not executed. |

The [original R1 library log](../artifacts/verification/download-completeness-20261106/r1/evidence/20261105T170403Z-2e38c76e/logs/cargo-lib-tests.log)
is preserved unchanged. Both failed cases had rejected their malformed input
before failing the error-kind assertion. R1 does not contain the raw decoder
diagnostics needed to describe those classifications as a passing result.
R2 adds precise diagnostics and a narrow correction for the pinned decoder's
known Huffman failures. Both original corruption assertions and evidence checks
pass in R2; they were not weakened.

R2's original five-case guest run passed four cases but failed the loading
fixture before readiness. That fixture held a SQLite writer lock before the
application could migrate schema 2 to 3 and upgrade legacy image proofs. The
corrected external harness first lets the real application migrate, validate and
exit, then resets only the synthetic interrupted task before taking the bounded
writer lock. It does not hand-write a strict proof or force a view-model field.
The same sealed executable subsequently passed all five cases, including real
loading capture, restored page pixels, unchanged proofs, and the expected
paused task generation. The original guest failure remains in its raw suite.

The initial subprocess runner invocation also lacked the remote Cargo environment
in its shell. Its setup failure is retained; the invocation with
`/root/.cargo/env` passed. It is not described as an application failure.

## Final candidate identity and evidence

The canonical manifest contains 390 application inputs, SHA-256
`a77924eddc4ac28f016217fa7efbef7793015f53a39345fcc0503f8fc10a5af5`.
The sealed desktop executable has SHA-256
`5a8d3bd15e07163186d5c413068eed5afb981e3c79a4504ba453a148dee84fc4`.
An explicit path/hash comparison confirms all 390 inputs are identical between
the core and subprocess-service runs; their manifest text ordering differs.

Evidence under `artifacts/verification/download-completeness-20261106/` includes
the immutable `r1` and `r2/evidence` suites, `r2/production-flow-env-corrected`,
`r2/batch-fixture`, and the separately bound `r2/guest-corrected` harness result.
The corrected fixture is a pipeline-only change after the original R2 snapshot;
its old/new script digests, original application manifest and sealed executable
are recorded in `harness-delta.json`. Raw automated visual-review fields are
preserved; [selected PNG review](../artifacts/verification/download-completeness-20261106/visual-review.json)
is recorded separately.

The [0.1.5 development archive](../artifacts/verification/download-completeness-20261106/arm-r2/bilicomics-kindle-0.1.5-dev.zip)
is 50,074,789 bytes, SHA-256
`fc54db6fa9efb2e3d8eed9d0afa7b9b4f563d36268dbe355caa6dcc372a246d4`.
Its 64,807,160-byte ARM executable has SHA-256
`4e84e90b8efa6b32c6936aed124dbc886ebfeb04f3b07074bbfeaeb8cc6e194c`.
The [build and static verification record](../artifacts/verification/download-completeness-20261106/arm-r2/candidate-summary.json)
binds it to the same frozen source. The archive retains all six added JPEG
license/provenance/patch files. The build log retains one upstream `macro_use`
attribute diagnostic and two compiler-wrapper diagnostics; two Cargo summary
lines are not additional source diagnostics. This is not a warning-free build.

The earlier 222-test batching evidence and the `0.1.4` physical-device evidence
retain their original source and executable identities. They are not reassigned
to this candidate. The fresh download path now adds a `ComicDetail` request, so
the earlier request totals, signature counts, and timing medians are not
measurements of this version. No new throughput or physical-device performance
result is claimed by this document.

Current controlled cold requests include one `ComicDetail` and one index
request in both modes: the single-page contract makes 45 token and 45 CDN
requests (92 total), while the production batch adapter makes three token and
45 CDN requests (50 total). Complete ordinary cache reopens make no requests;
an explicit fresh whole-chapter acquisition deliberately rechecks permissions
and the index. The current fixture run is used for completeness and request
accounting only. Its process-wide successful-decode memo is not independently
reset between cold disk-cache rounds, so the recorded timings are not accepted
as a new performance comparison.

This audit uses source inspection and controlled synthetic fixtures. It has not
used a real account, made a purchase, acquired real comic content, or connected
to a Kindle. The service has no independently verified full-versus-preview
indicator or authoritative page total in the reviewed application contract. A
fresh successful `ComicDetail` and a structurally valid shorter index therefore
cannot independently prove that every official chapter page was returned.

Real-content scope acceptance still requires an authorized comparison with the
official client under the same account and chapter permissions, checking the
catalog, index range, and actual first, middle, and final pages. Such a comparison
would be separate evidence; local full decoding and synthetic long-index tests
cannot replace it.
