# Native-query helper research: 2026-11-17

**The whole physical batch failed. The helper is not adopted into main.**
This record preserves an independently qualified primary subset and the reason
the additional diagnostic did not complete. Only reports and evidence are
intended for main; the runtime experiment remains on source commit `f574b269`.
No device, lease, application, or tests were accessed or run during this export.

## Six retained primary pairs

The predefined primary set contains 14 launches: two warmups and six complete
AB/BA pairs. All six pairs remain included, including slow candidate-01 and
candidate-06. Baseline is fresh source `f50ada15`, application `19708424`;
candidate is source `f574b269`, application `806a406d`, helper `ecb336e4`.
Full source fingerprints, package/executable/launcher hashes and matched release
controls are in [summary.json](summary.json).

| Primary endpoint | Baseline median | Candidate median | Candidate minus baseline mean |
| --- | ---: | ---: | ---: |
| Software ready | 5864.800538 ms | 5798.234347 ms | +181.221051 ms |
| First business SEND accepted | 6643.891885 ms | 6650.601077 ms | +103.429307 ms |

For business SEND, the candidate is faster in three pairs and slower in three;
the difference of independent medians is +6.709192 ms. The separately calculated
paired median differs from that difference and is retained alongside every
sample, range, mean and signed pair delta. These observations establish no
overall startup benefit. They are instrumented warm process launches with
profiling/demo work, not tap-to-panel latency, cold boot, or a universal result.

[primary-qualified-projection.json](primary-qualified-projection.json) is the
original independent projection. Its new [projector](tools/project_primary.py)
calls the unchanged frozen [as-built analyzer](tools/as-built-analyze_startup_ab.py)
functions; it does not rewrite the analyzer's whole-batch rules or promote the
original report's `success=false`. The selected batch had 16 cases and 15
controller-completed cases. Compatibility and diagnostics are outside primary
statistics.

## Compatibility and incomplete diagnostic

The helper-omitted compatibility case completed and restored native control.
The diagnostic's independent Rust calibration has five successful queries over
three property types, followed by finish(requests=5), clean EOF and helper exit.
The later UI window has its own process identity, readiness/accepted frames and
normal launcher status zero. This is not a five-query session inside that UI
process, nor coverage of eight ABI operations.

The separate eight-operation process was refused before spawn with
`reserved-descriptor-in-use`: PID, executable and hash are null, identity was
not observed and stdin was not released. It is not a helper timeout or an ABI
pass. The diagnostic whole case remains unqualified and the global batch remains
failed. No further descriptor or sleep testing was performed for this decision.

## Sources, controls and earlier failures

The narrowly scoped controller lock-retirement fix and its incident record are
preserved under [tools/control2/](tools/control2/). Its frozen record documents
33 newly executed controller regressions, plus unchanged prior sets of 27 ABI
probe and 65 analysis/clock tests: a latest inventory of 125, not a single rerun
of 125. The original 120-test record and rejected partial/preflight history are
not reclassified. Root's existing three [preflight records](preflight/) are
preserved byte for byte and are not rewritten by this exporter.

Saved recovery flags retain native/lighting restoration, transport closure,
same boot/input roster, temporary-stage removal and exact owned-lease release.
Installation preservation covers only nine selected regular-file identities
or recorded absences, not a whole-tree census. Native Home pixels were privately
reviewed; no native Home, native before/final content, owner/token/configuration,
raw private logs, databases or output archives are public here.

## Six synthetic application images

| Last measured pair | Early memory projection | Later memory projection | Original logical PNG |
| --- | --- | --- | --- |
| Baseline-06 | [early](images/baseline-early.png) | [settled](images/baseline-settled.png) | [logical](images/baseline-final-logical.png) |
| Candidate-06 | [early](images/candidate-early.png) | [settled](images/candidate-settled.png) | [logical](images/candidate-final-logical.png) |

The four Gray8 PNGs losslessly project every 1860 x 2480 packed visible byte
from saved memory (device stride 1872). No resizing, cropping, inversion,
rotation, masks or tone changes were applied. Original raw files remain private;
raw, pixel and PNG hashes are retained. The other two PNGs are untouched logical
Slint snapshots before framebuffer compensation.

Each memory-image sidecar retains actual request and completion markers,
offsets from readiness/business SEND, and same-application liveness before and
after capture. These are helper request/completion windows, not instantaneous
SEND images or exact read instants. Both early windows finish before the shared
first-business SEND +1500 ms gate; the later settled windows occur after that
gate. The later scheduled target and lateness remain explicit. All last-pair memory pixels
and final logical pixels match between variants, but no optical panel completion
or stability at an earlier time is established.

Memory/input samples also follow the common gate. VmHWM is the kernel high-water
value when sampled; no continuous full-lifetime peak profile is claimed. SEND
acceptance, marker/driver completion and panel observation remain separate.
No new touch, rotation, sleep/wake, model or firmware qualification is established.

[manifest.json](manifest.json) binds the public files and separately lists the
existing preserved preflight records. Original private sources are identified
by hash; the failed batch and frozen raw evidence were not modified.
