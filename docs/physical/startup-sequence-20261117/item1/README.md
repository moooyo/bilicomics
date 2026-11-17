# Item 1 physical startup evidence, 2026-11-17

This fixed-allowlist export records an ended Scribe demo/offline A/B batch.
Export and its independent review read stored files on `test-env` only; no test,
build, application, device transport, input, lease operation or new capture ran.
The measured batch passed its frozen contract; it is **not final adoption** of
the startup candidate. The separate route-refresh followup has another source
identity and is outside this result.

Both variants use source `530f1516c332931059c52161b52d64e86488b3f1`, ELF
`04fc6f93b4327911ca1a3bb5200175c9f2e13fb21ac5e56280012ff44b6e2e59`, and ZIP
`1fe8ac38613d21904d187b41e973ff80302b0c78fe69b320e3d7143a31fd53ee`.
Only `--serial-startup-preparation` differs: baseline includes it; candidate
uses the default preparation sequence. [summary.json](summary.json) retains
the exact source fingerprint, compiler, package, worker, launcher, build and
fixture identities. This compares two paths in one ELF.

All 14 launches remain: two predefined warmups, then six balanced AB/BA pairs.
Only those warmups are omitted from primary distributions. No slow sample is
discarded, including candidate-03 Ready and candidate-06 Ready/SEND regressions.

| Primary endpoint, ms | Baseline median | Candidate median | Faster pairs |
| --- | ---: | ---: | ---: |
| First Shelf SEND accepted | 6696.776654 | 6465.361424 | 5/6 |
| Software Ready | 5905.555154 | 5691.152808 | 4/6 |

The clock is device `CLOCK_MONOTONIC`, from the request helper's timestamp
sample before it writes the request record and execs
`lipc-set-prop com.lab126.appmgrd start` for the native Scriptlet dispatch, to
`startup/ui_ready` or the first accepted rendered Shelf SEND whose Slint render
starts after Ready. The interval includes request-record writing, LIPC/appmgr
dispatch and launcher work before application entry. This is a warm-start,
single-boot experiment with fresh
application processes, retained OS/filesystem caches and profiling. It does not
measure physical-tap latency, complete covers, optical panel completion, cold
boot, or a statistical population guarantee.

[Frozen analyzer output](analysis/frozen-analyzer.json), complete original
allowlisted [case files](cases/), [critical-path analysis](analysis/critical-path.md),
[timeline](analysis/timeline.csv), [frame spans](analysis/frame-spans.csv) and an
[independent physical audit](analysis/physical-audit.md) retain the full windows.
Candidate font-byte work overlaps the original first-frame completion wait;
metadata overlaps only partly. Cover pixels start after the original prepare
gate and checked completion. Candidate-03/06 each present an additional complete
frame between Route and Views before Ready. The record locates that cost but
does not identify the scheduling or instrumentation cause of crossing the
stage budget. The followup must be assessed independently.

Every case records normal recovery. The controller records restored native
control, selected installation identities, lighting, input roster and same
boot, removed its stage, closed owned transports and released the exact owned
lease at `2026-11-17T10:18:37.889218+00:00`. These are completed-event receipts,
not a claim about the current lease. Selected-file preservation is not a
whole-tree census. Native restoration is not new physical-touch acceptance.

The resource gate targets first business SEND +1500 ms. Actual procfs reads
follow that gate and any framebuffer capture. VmHWM is the kernel high-water
value when sampled, not a continuous startup-only or whole-lifetime peak.
Historical host-only fixture checks in the tooling manifest are separate from
these physical measurements and were not rerun during export.

## Six application images

| Case | Early memory | Scheduled memory | Original logical PNG |
| --- | --- | --- | --- |
| baseline-06 | [early](images/baseline-06-early-memory.png) | [scheduled](images/baseline-06-scheduled-memory.png) | [logical](images/baseline-06-final-logical.png) |
| candidate-06 | [early](images/candidate-06-early-memory.png) | [scheduled](images/candidate-06-scheduled-memory.png) | [logical](images/candidate-06-final-logical.png) |

The four Gray8 PNGs losslessly encode every one of the 4,612,800 visible memory
bytes at 1860 x 2480. Device stride is 1872; packed captures exclude padding.
No pixels are masked, cropped, resized, rotated, inverted or tone-corrected.
Sidecars bind the original raw/header/pixel identities, complete geometry,
compensation scope, capture bracket and PNG identity. Original raw framebuffer
files remain private. All four visible-pixel payloads match, and each complete
baseline/candidate comparison has zero differences. This covers **two cases
and four raw captures**, not 14 raw framebuffer captures.

| Case | Early request after SEND, ms | Scheduled request after SEND, ms | Scheduled helper bracket, ms |
| --- | ---: | ---: | ---: |
| baseline-06 | 560.504769 | 1500.400000 | 682.882923 |
| candidate-06 | 436.286462 | 1501.645385 | 709.222923 |

The early requests are delayed post-SEND observations, not the exact first-frame
instant. Scheduled pixels are observed within their recorded software bracket,
not instantaneously at +1500 ms; resource reads occur afterward. These are memory
projections, not camera photographs or optical panel acceptance. The two logical
PNGs retain original bytes before framebuffer compensation. All 14 original
logical snapshots have equal full RGB pixels in the frozen analysis.

## Separate incomplete assisted attempt

[The bounded assisted record](assisted-attempt/summary.json) uses the same ELF
with the isolated demo/offline reader through the production launcher. It is
separate from the Scriptlet performance entry. A before-sleep checkpoint records
reader page 18, but no actual operator key/touch feedback arrived within the
bounded attempt. Only `before-sleep` exists; there is no resumed or after-input
checkpoint or actual confirmation. The six-minute exit upper bound ended with
watchdog reason `launcher-exit`, status zero; stop, native recovery, transport
closure and exact lease release are recorded.

Its status is **incomplete / no_operator_observation**. Sleep/wake, reading
continuation and physical touch remain unverified. A timer or safe automatic
exit cannot supply those passes. Only fixed field projections and raw source
identities are exported; `run.json` is not copied in full.

## Export boundaries

This is export revision 2. Only this README wording and the export manifest
changed; all measurement data, image bytes, analysis and archived source payloads
retain revision 1 bytes. The manifest records the previous export identity,
the exact documentation edits and their stored-source confirmation. The
archived `tools/export_item1.py` remains the unchanged revision 1 exporter.

`cases/` contains complete original bytes only for individually privacy-reviewed
performance/lifecycle/shell, memory, request/gate, recovery and demo snapshot
sidecar files. `analysis/` contains explicitly derived frozen analysis. Files
named `*.projection.json` are derived fixed-field projections and retain the
original input byte count and SHA-256; they must never be treated as raw originals.

The [export manifest](export-manifest.json) maps every payload to exact source
paths, byte counts, SHA-256 identities and transformations. Its own digest is in
[export-manifest.sha256](export-manifest.sha256). Public measurement source is
preserved under `tools/`; it is archival source, not permission to run it. The
tooling manifest identifies its original full freeze; only the declared sources
are included here, and its historical test logs are not exported. The
analysis manifest's filenames are relative to its original frozen analysis
directory; the outer export manifest maps their renamed export destinations.

No credentials, session files, signed URLs, private transport/ownership values,
native Home/library images, real account content, databases, ELF/ZIP payloads,
raw framebuffer files, full output archives or research archives are exported.
Fixture results, this offline-demo physical batch, the incomplete assisted
attempt, live-account behavior and optical/hardware acceptance remain separate.
