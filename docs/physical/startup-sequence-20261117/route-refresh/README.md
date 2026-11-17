# Route-refresh physical startup experiment, 2026-11-17

**Decision: NOT ADOPTED.** The route-only candidate removes redundant accepted
pre-Ready GC16 complete replacements, but does not produce a stable overall
startup gain. This independent experiment remains archived; its source is not
the retained runtime. No future coalescing or backend-loop design was implemented
or measured by this batch.

Both variants use source `f6208d6f7dd3d8e64a09ef0ceb2ceb6b0c45d0f5`, ELF
`3962bc49bb8cd33d21651845fe5de229e1b8458e5d0ba9026485c8bef7224525` and ZIP
`ff41fdf74505020f50ab9928b1a7168ed773b82713b3330ce6b71a0b915b194c`.
Both retain default overlapped preparation. Baseline uses
`--demo --offline --legacy-startup-route-refresh`; candidate uses
`--demo --offline`. There is no serial-preparation variant in this experiment.
[summary.json](summary.json) retains exact source-input fingerprint, compiler,
worker, launcher, package, build and fixture identities. This is separate from
item1; the batches are not pooled or compared as paired treatment effects.

## Complete measured result

All 14 launches remain: two predefined warmups and six balanced AB/BA pairs.
Only those warmups are omitted from primary distributions. Every slow sample
and all original performance/lifecycle windows remain in [cases/](cases/).

| Primary endpoint, ms | Baseline median | Candidate median | Faster pairs |
| --- | ---: | ---: | ---: |
| First Shelf SEND accepted | 6509.286116 | 6521.791231 | 3/6 |
| Software Ready | 5712.525615 | 5884.013962 | 3/6 |

Both marginal medians are slower for the candidate. The medians of paired B-C
differences are +39.451 ms for SEND and +26.821 ms for Ready; those are different
statistics and do not establish a stable gain. Full per-pair results are in the
summary, [frozen analyzer](analysis/frozen-analyzer.json) and
[critical-path report](analysis/critical-path.md).

The request helper samples device `CLOCK_MONOTONIC`, writes and closes the
request record, then calls `execvp()` for
`lipc-set-prop com.lab126.appmgrd start` with the native Scriptlet URI. Endpoints
remain the `startup/ui_ready` envelope stamp and first accepted Shelf SEND whose
Slint render begins after Ready. The interval includes request-record writing,
LIPC/appmgr dispatch and launcher work before application entry. This is a
warm-start, single-boot experiment with fresh application processes, retained
OS/filesystem caches and profiling. It does not measure physical-tap latency,
cold boot, complete covers, physical input readiness or optical panel completion.

## Why fewer complete submissions did not give an overall gain

No candidate launch, including warmup, has an accepted non-initial pre-Ready
forced-full GC16 flashing replacement. Baseline warmup, 02 and 06 each have one.
The initial GC16, sequence 2 full-screen-region DU and small activity AUTO
updates remain. A full-screen region is not the same as a complete replacement.

Candidate 02/03/06 still contain an unchanged-scene render attempt with zero
SEND ioctls. They wait for the preceding marker and compute RGB fingerprints;
they do not submit another partial or full update. Route-to-Views work therefore
remains about 400-411 ms in those samples, versus about 822-823 ms for the two
primary baseline replacements. Candidate 03 crosses this boundary where its
baseline does not, regressing Ready by 371.433 ms and SEND by 59.787 ms. The
earlier state-to-attachment handoff and cover-attachment presentation boundaries
also remain. These observed paths explain why saved submissions are insufficient
for adoption; no unrecorded scheduling/cache/power cause is asserted.

The complete [timeline](analysis/timeline.csv), [frame spans](analysis/frame-spans.csv)
and [independent physical audit](analysis/physical-audit.md) preserve actual
attempts, stages and bounds. Overlapping spans must not be added into elapsed
startup time. Bounded dispatch/coalescing at an existing barrier is only an
unimplemented design hypothesis, with no promised benefit.

## Recovery, resource and pixel scope

All 14 recoveries record normal exit and restored native control. Selected
installation identities, input roster and boot match; lighting was restored,
the temporary stage removed and owned transports closed. Exact owned lease
release is recorded at `2026-11-17T11:03:28.437714+00:00`. These are completed-event
receipts, not current lease claims. Selected-file preservation is not a whole-tree
census, and native restoration does not establish physical touch acceptance.

The resource gate targets first business SEND +1500 ms. Actual procfs reads
follow the gate and any scheduled framebuffer capture. Sampled VmRSS medians
are 55964 versus 55914 KiB, with mixed paired directions; this 50 KiB difference
does not establish a memory gain. VmHWM is the kernel high-water field when
sampled, not a continuous startup-only or whole-lifetime peak measurement.

| Case | Early memory | Scheduled memory | Original logical PNG |
| --- | --- | --- | --- |
| baseline-06 | [early](images/baseline-06-early-memory.png) | [scheduled](images/baseline-06-scheduled-memory.png) | [logical](images/baseline-06-final-logical.png) |
| candidate-06 | [early](images/candidate-06-early-memory.png) | [scheduled](images/candidate-06-scheduled-memory.png) | [logical](images/candidate-06-final-logical.png) |

The four Gray8 PNGs losslessly encode all 4,612,800 visible memory bytes at
1860 x 2480. Device stride is 1872; packed raw captures exclude padding. No
mask, crop, resize, rotation, inversion or tone correction is applied. Sidecars
bind original raw/header/pixel identities, complete geometry, compensation
scope, actual capture bracket and PNG identity. Raw framebuffer files remain
private. Both whole-visible-pixel pair comparisons have zero differences; all
four payload hashes also match. This covers two cases and four raw captures.
All 14 final logical Slint snapshots have equal full RGB pixels before
framebuffer compensation; the two exported logical PNGs retain original bytes.

| Case | Early request after SEND, ms | Scheduled request after SEND, ms | Scheduled bracket, ms |
| --- | ---: | ---: | ---: |
| baseline-06 | 469.139538 | 1501.679154 | 664.976846 |
| candidate-06 | 503.963846 | 1501.608461 | 660.513077 |

Early requests are delayed post-SEND observations, not the exact first-frame
instant. Scheduled pixels are observed within their recorded software bracket,
not instantaneously at +1500 ms. Sixth-pair procfs reads follow capture
completion at approximately +2167/+2162 ms. Memory pixels are not camera
photographs or optical panel completion. There is no new touch, sleep/wake,
rotation, live-account or hardware qualification, and no assisted attempt is
included in this independent route-refresh bundle.

## Export boundaries

Export and its independent review use stored files on `test-env` only; no test,
build, application, emulator, runtime probe, input, device transport or lease
operation runs. Original individually privacy-reviewed case files keep complete
bytes. Frozen analysis is explicitly derived; `*.projection.json` files are
fixed-field projections and retain raw source byte counts and SHA-256 identities.
They must not be treated as raw originals.

The [export manifest](export-manifest.json) maps every payload to exact source
paths, bytes, SHA-256 and transformations; its digest is in
[export-manifest.sha256](export-manifest.sha256). Public measurement revision 2
source is archived under `tools/`. Its historical tool-only host fixtures and
pre-review/final-source distinctions remain exactly in the frozen manifest,
separate from physical results; they are not rerun here. Only declared source
files are included from that larger tooling freeze. The analysis manifest uses
its original filenames; the outer manifest maps renamed export destinations.

No token or private ownership values, credentials, sessions, signed URLs,
private transport bindings, native Home/library images, real account content,
database payloads, ELF/ZIP binaries, raw framebuffer files, full output archives
or research archives are exported. Fixture results, this physical offline-demo
experiment and optical/hardware acceptance remain separate evidence classes.
