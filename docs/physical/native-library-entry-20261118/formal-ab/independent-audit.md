# Independent stored-evidence audit projection

This English projection was prepared on 2026-11-18 (Asia/Shanghai) from the frozen
independent audit of `lib-ab-20261117-2e297ef4`. The original audit JSON SHA-256 is
`baba662a9e2e0caebf736e6672a94c9de94d5371b53ef7dfadbb0a2b04e17d4d`;
its original Markdown SHA-256 is
`d5de19b432430737848e0a028ed1fa0b66b771bf062d0f47761325b9530fd6b8`.
The frozen origin manifest SHA-256 is
`becc9b68204c892d11f38d62291ad88262a919b40811013207d3b72d05763f10`.
The original audit checked 689 file origins and 392 regular archive members.
This projection omits their complete listing and all raw owner/process identities.

The formal AB evidence is usable for its two predefined software endpoints.
No stored identity, clock-mirror, dropped-sample, normal-exit or restoration
anomaly was found that requires discarding the comparison. Product adoption or
acceptance, lifecycle qualification and physical-pixel qualification remain
unsupported by this AB batch. See [the exact audit projection](independent-audit.json).

The predefined schedule was two warmups followed by `ABBA ABBA ABBA`: six numbered
AB/BA pairs, three ABBA blocks, twelve primary samples and fourteen retained
launches overall. No performance outlier was removed or replaced. Four earlier
failed attempts produced zero formal samples and remain in the separate
[checkpoint](../checkpoint/README.md).

The request origin is the pinned `startup-clock` CLOCK_MONOTONIC sample before
closing its record and executing the canonical native file URI. It includes
that small record/exec boundary. The eight-second native stabilization interval
precedes and is excluded from that origin; actual stable-to-request durations
were 8307.860847 to 8585.456462 ms. All 28 private raw clock records were mode
0600 and byte/SHA-identical to their public mirrors. Original clock byte hashes,
lengths and private modes remain in the export; raw original PIDs do not.

Software ready is the unique performance `startup/ui_ready` record. First Shelf
accepted SEND is `refresh_submit.finished_monotonic_ns` on the earliest rendered,
accepted native frame with Slint render-start at or after ready and render-end
before refresh-submit start. All selected frames later record driver completion;
all have `panel_completion_observed=false`. The later frame record timestamp
and lifecycle ready elapsed time are not substituted for either endpoint.

| Exact primary statistic | First Shelf accepted SEND | Software ready |
| --- | ---: | ---: |
| Baseline marginal median | 6820.738654 ms | 6055.7048845 ms |
| Candidate marginal median | 6288.530616 ms | 5582.247731 ms |
| Difference of marginal medians, candidate minus baseline | -532.208038 ms | -473.4571535 ms |
| Marginal median percent change | -7.802791823550% | -7.818365698630% |
| Median of paired candidate-minus-baseline differences | -424.326461 ms | -399.7113845 ms |
| Candidate faster / slower / tied pairs | 5 / 1 / 0 | 4 / 2 / 0 |

Pair 01 remains included: candidate SEND is 33.072462 ms slower, and ready is
338.195000 ms slower. Pair 02 ready is 26.075923 ms slower. Differences between
marginal medians and the median of paired differences are distinct statistics.
Six pairs in one device/session do not establish a general population or causal
effect; order, cache, thermal, drift and background effects remain possible.
The older temporary-route 6.47-second result is not pooled or directly compared.

Both arms use the same production application and JPEG-worker binaries, the same
closed demo/offline fixture and common launcher instrumentation. Only the indexed
document's `--library` argument differs from no entry argument. The common
instrumentation preserves the production direct-child spawn lines; it introduces
no research supervisor in that child chain. These shared launcher, handler and
FBInk costs limit interpretation of uninstrumented absolute latency.

Every opening observes one canonical file item, title `BiliComics for Kindle`,
matching structured title, and plain shell MIME `text/x-shellscript`. All native
scanner UUID reconstructions are retained as change booleans, not raw UUIDs.
All `p_lastAccess` values increase; `p_lastOpenTime` stays unobserved/null. Document
mtime does not regress and byte length is preserved. After-opening catalog
contentSize literals equal document byte lengths, without proving the extractor
size unit or byte-content equality from size alone. Cover and thumbnail locators
remain null/unchanged, so custom-cover and thumbnail qualification remain unknown.

Baseline entry is gone at content in 7/7 cases; candidate entry remains alive at
content in 7/7. Candidate worker admission parents are their own entries; baseline
workers have reparented to init. Every generation is bound, actual worker exit
status is 0, and every recovery records normal exit, owned processes gone,
production lock absent and native control restored. These are stored normal-exit
facts, not cancellation, retirement or sleep/wake qualifications.

The sixteen current-registration projections are identical, with decoded
canonical SHA-256 `cfc0381d6262afe7aff58534a832a6c15eea3903f07f35b831c69c7102acf804`.
This actual current registration is the preservation baseline; the historical
`a594716cfe9a66e912d5883ad0ae3721f7d4f4196c13d04f7ed3158865412471` identity is
provenance only. The original native handler was hash-checked without replacement.
All four Aidoku snapshots match within their exact selected six-file scope.
The deployed shared wrapper's source-to-binary mapping was not independently
recaptured or requalified. Its registration hash alone does not prove its logic;
no whole-Aidoku-tree preservation claim is made.

All 125 recorded commands return zero without interruption/timeout. All 46
read-only SQLite attempts succeed on first attempts. Stored restoration reports
native control, lighting, catalog, registration, installation, input roster,
staging cleanup, boot identity and connection closure restored/preserved within
their recorded scope. The exact owned lease ran from
`2026-11-17T20:31:56.706115+00:00` to `2026-11-17T20:39:31.192830+00:00` (UTC),
then released after native restoration and connection closure. Its exact
owner/token check and absent lease directory are preserved as booleans;
owner values, token values and token digests are omitted.

All fourteen final PNG files have identical PNG bytes and identical complete
1860 x 2480 RGBA pixels: 4,612,800 pixels / 18,451,200 channel bytes per capture,
zero changed channels. [The single original retained PNG](logical-final.png) has
SHA-256 `0a0df97df88bbb46149381fa1c995c945d81a8b6179c28ec66094cba100d19da`.
Its explicit semantics are `Logical Slint snapshot before framebuffer compensation`.
This does not establish raw framebuffer compensation equivalence, physical panel
pixels, optical completion, input readiness or complete-cover completion. No
native Home image is exported. There are no lifecycle sleep/wake checkpoints,
reader-anchor continuity cases or cancellation/retirement qualification cases in
this AB batch.

This export only reads stored host evidence on `test-env`. It performs no new
tests, builds, runtime probes, application execution, device or lease action.
Product source, frozen tools and raw evidence are unchanged. Public source
selectors, original byte lengths/SHA-256 identities and transformations are
recorded in [manifest.json](manifest.json).
