# Formal native Library AB stored evidence

Prepared on 2026-11-18 (Asia/Shanghai) from the successful stored run
`lib-ab-20261117-2e297ef4`. Original run/lease timestamps remain UTC. This bundle
contains usable formal AB timing evidence; it does not adopt the candidate or
complete product, lifecycle or physical-pixel acceptance.

The predefined schedule completed **14 retained launches**: two warmups and six
alternating AB/BA pairs (`ABBA ABBA ABBA`, three blocks). All twelve primary
samples remain. There were no performance outlier removals or replacements.

| Software endpoint | Baseline median | Candidate median | Reduction of marginal medians | Faster pairs |
| --- | ---: | ---: | ---: | ---: |
| First Shelf accepted SEND | 6820.738654 ms | 6288.530616 ms | 532.208038 ms (7.8027918%) | 5/6 |
| Software `ui_ready` | 6055.7048845 ms | 5582.247731 ms | 473.4571535 ms (7.8183657%) | 4/6 |

The paired median candidate-minus-baseline differences are -424.326461 ms for
SEND and -399.7113845 ms for ready. These are different statistics from the
differences between marginal medians above. Pair 01's 33.072462 ms slower SEND
remains included. One device/session and six pairs do not establish a population
or causal effect. The older temporary-route 6.47-second result is not pooled or
directly comparable; the separate [failed-attempt checkpoint](../checkpoint/README.md)
and [clock-only selfcheck](../clock-selfcheck/README.md) remain unchanged.

Every launch opened the real indexed lower-case canonical file URI
`file:///mnt/us/documents/bilicomics.sh`, following an eight-second native-stable
dwell. The request origin is sampled before the pinned request-record close and
native URI exec; that small boundary is included, while the preceding stable
dwell is excluded. Software ready uses the unique performance `startup/ui_ready`
record. SEND ends at the first qualifying Shelf frame's
`data.stages.refresh_submit.finished_monotonic_ns`, not its later frame record,
driver completion or optical panel completion. There is no input-ready or
complete-cover endpoint in this build.

Both arms use the same production application/worker, demo/offline fixture and
common launcher instrumentation. Only `--library` versus no entry argument differs
in the document. Production direct-child spawning is preserved. Packaging source
is `39fac822a982868078201e605e50bdacf1dcf3ed`; the unchanged reused runtime source is
`530f1516c332931059c52161b52d64e86488b3f1`. The selected ZIP SHA-256 begins
`c57d5fc3`; full package, binary, fixture, compiler, frozen-tool, overlay and actual
host-admission identities are in [provenance.json](provenance.json). No ARM/Rust
rebuild was performed for this entry package. Shared launcher, handler and FBInk
costs remain part of the measured path.

The actual current registration (`cfc0381d...`) is preserved, rather than restored
to the historical `a594716c...` identity. The original native handler is unchanged;
Aidoku's four snapshots match within the selected six-file scope. The shared
wrapper's deployed source-to-binary mapping was not independently requalified.
Native scanner row UUIDs are allowed to reconstruct. All observed recent-progress
updates remain recorded; null cover/thumbnail locators leave their qualification
unknown. [restoration.json](restoration.json) preserves the successful normal-exit
and exact-release projections. The original lease closed at
`2026-11-17T20:39:31.192830+00:00` after native restoration and connection closure.

All fourteen final PNGs have identical complete 1860 x 2480 RGBA pixels. Only
[one original PNG](logical-final.png) is retained here. Its source explicitly says
`Logical Slint snapshot before framebuffer compensation`: this is logical Slint
capture equality, not actual raw-framebuffer, physical-pixel, optical, cover or
lifecycle acceptance. This AB batch contains no sleep/wake, cancellation,
retirement or reader-anchor continuity qualification cases.

[summary.json](summary.json) and [analysis.json](analysis.json) expose the summary
and a fixed-field projection of the frozen canonical analyzer result.
[independent-audit.md](independent-audit.md) and
[independent-audit.json](independent-audit.json) preserve the independent audit's
findings, exact Decimal statistics and source SHA chain. Fourteen `cases/` JSON
files retain every native performance frame, the unique ready record and all
lifecycle record positions using explicit field allowlists, plus clock, recovery,
final-state, metadata and admission-relationship projections. Private original
clock modes, byte lengths and SHA identities are retained; their PIDs are omitted.

To recompute the two endpoints and exact statistics from these public projections,
run `python3 tools/recompute_timing.py` on a permitted host. This bounded JSON
reader follows the frozen endpoint rules; it has no device, application, network
or lease operations and does not replace the full private analyzer. Export and
review computation for this archive ran on `test-env`; no local verification was
performed.

[manifest.json](manifest.json) binds each selected original's byte length and
SHA-256 to explicit source selectors and transformations, and hashes every public
payload. `SHA256SUMS` covers every other file including that manifest. Private
owner/token values or digests, original native UUIDs, process identities, transport
and address data, account content, native Home, raw archives, acquired comics,
ELF and ZIP contents are omitted. No source, frozen tool or raw evidence was
changed; this export performs no new tests, builds, runtime probes, application,
device or lease work. See [the full status](../../../NATIVE_LIBRARY_ENTRY_20261118.md)
for the remaining qualification scope.
