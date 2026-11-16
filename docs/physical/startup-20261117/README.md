# Physical startup evidence: 2026-11-17

This is a whitelist export of an already completed Scribe startup experiment.
No device, lease, application, or tests were accessed or run during export.
Only the bundled offline synthetic demo appears in the six public application
images. Native Home/library pixels, native before/final captures, private logs,
ownership files and values, output archives, credentials, and databases remain
private.

The software source tags are baseline `06ad90b` and candidate `585e571`.
[summary.json](summary.json) retains their full source, inventory, package,
launcher, executable, worker, compiler, fixture, and tool identities. The
37-file fixture fingerprint is `a930027e7e297c89b61c963d2dd17d30c605bca3d4260e1805b7aea2257f3f79`. Both builds used the
same release profile and compiler: thin LTO, one codegen unit, six Cargo jobs,
incremental compilation disabled, and the same ARM target/features. The frozen
host tooling records 49 successful fixture tests separately from device results.

## Timing scope

One warmup per variant is excluded. All five subsequent pairs are retained in
AB/BA/AB/BA/AB order, including the slower candidate-04 launch. These are warm
process launches in one boot with profiling and demo RGB fingerprints enabled,
without cache eviction. The clock begins immediately before the instrumented
appmgr/Scriptlet request. It includes marker and exec overhead and is not a
physical tap measurement.

| Endpoint median | Baseline | Candidate |
| --- | ---: | ---: |
| Software ready | 9.089 s | 5.986 s |
| First business SEND accepted | 9.407 s | 6.850 s |
| Actual per-case ready to SEND interval | 291.541 ms | 772.638 ms |

The last row increased. It is the median of separately calculated intervals,
not a difference between the two independent endpoint medians. candidate-04
remains at 6.747 s ready and 7.203 s business SEND. Every value, range, pair,
and signed paired delta is retained in the JSON. There is no universal speedup,
percentile, cold-boot, or profiling-disabled claim.

[stage-projection.json](stage-projection.json) is the original independently
audited anonymous stage projection, including the longer business render spans,
repeated frames, diagnostic CPU work, and unresolved gaps. Spans can overlap and
must not be summed into end-to-end latency. The extra pre-ready repeated frame
is present in all five baseline samples and one candidate sample; it is not a
candidate-only mechanism. No dictionary cause is established. The detailed
launcher phase file was not collected, so only the request-to-Rust coarse
boundary and source-defined waits are available.

## Recovery and memory

All 12 cases retain same-process final/lifecycle/performance identity checks,
matching selected input nodes, no recorded sleep/power transitions, launcher
status zero, native service restoration, owned-process exit, and absent
production locks. No emergency recovery command was recorded. This does not
claim that every possible input-discovery fallback was absent.

The controller restored lighting, kept the same boot/input roster, removed its
temporary stage, closed its owned transports, and released only the exact owned
lease. The public release projection retains the explicit owner/token-check
and directory-removal flags without their private identity values. Installation
preservation covers exactly four selected regular-file identities/absences;
it is not a whole-tree census. Native Home framebuffer pixels were privately
reviewed by the primary agent. **Fresh user confirmation of native touch is
pending at export**; a later confirmation must be attached separately.

VmRSS and VmHWM are one sample near first business content. VmHWM is the kernel
high-water value at that reading, not continuously monitored full-run peak
memory. There is no new touch, rotation, sleep/wake, model, or firmware acceptance.

## Six application images

| Software | Early memory projection | Later memory projection | Original logical snapshot |
| --- | --- | --- | --- |
| Baseline-05 | [early](images/baseline-early.png) | [settled](images/baseline-settled.png) | [logical](images/baseline-final-logical.png) |
| Candidate-05 | [early](images/candidate-early.png) | [settled](images/candidate-settled.png) | [logical](images/candidate-final-logical.png) |

The four Gray8 PNGs are lossless projections of all 1860 x 2480 packed visible
memory bytes. The device row stride was 1872; the saved reader payload excludes
padding. No crop, resize, rotation, inversion, masking, or tone correction was
applied. Each sidecar retains the original raw/header/pixel/PNG identities; raw
files stay private. The logical PNGs are original bytes from Slint snapshots
before framebuffer compensation.

The later observation targeted ready + 1,000 ms, but its actual request was
late by 2,782.799 ms for baseline-05 and 2,892.385 ms for candidate-05. These
images therefore do not establish settlement at exactly one second. The
requests occurred near ready + 3.783 s and + 3.892 s, respectively, and their
timestamps are independent of primary SEND endpoints. All early/later visible
memory pixels match in this last pair, as do the final logical pixels. This is
memory evidence, not an optical camera capture or panel-completion proof.

SEND acceptance, driver completion, and panel observation remain separate
fields. The first business updates have accepted/completed matching markers
and recorded driver completion; panel completion was not observed.

[manifest.json](manifest.json) binds the exported bytes. Original private report
and analysis hashes are retained without copying those unfiltered files. Only
the final BiliComics protocol/analysis sources are retained under `tools/`;
reference Aidoku scripts, private configuration, and raw test logs are excluded.
