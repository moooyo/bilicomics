# Which part of display sampling took time on Scribe?

**Most of the measured interval lay between the launcher granting permission to the guarded display helper and the launcher observing its success receipt: 410 ms at the median.** The six archived traces do not say how much of that interval was `lipc-get-prop`, process startup, guard checks, framebuffer reads or waiting to notice completion.

These are the six measured B launches of the [closed November 21 native Library study](../../physical/shelf-render-20261121/README.md#device-results), not a new device run. Each `launcher-stages.tsv` contained exactly 14 parent-worker marks and matched its own `worker.pid`. The [collector](../../physical/startup-optimization-20261119/resume/rebuilt/device-pair.sh) exported eight allowed event names, with `go`/`ready`/`reaped` each occurring for three sequenced transactions. No child fork, `exec`, file open/read, query start/return, or polling iteration has a timestamp here. All differences below use one case's `/proc/uptime` marks, at **10 ms resolution**. No absolute times or process identifiers are exported.

| Case | Sample begin → go | Go → ready | Ready → reaped | Reaped → sample ready | Sample total | Outside go → ready |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| b01 | 40 | 410 | 10 | 10 | 470 | 60 |
| b02 | 40 | 380 | 10 | 10 | 440 | 60 |
| b03 | 30 | 410 | 30 | 20 | 490 | 80 |
| b04 | 30 | 440 | 10 | 10 | 490 | 50 |
| b05 | 40 | 350 | 10 | 10 | 410 | 60 |
| b06 | 50 | 420 | 10 | 10 | 490 | 70 |
| **Median (ms)** | **40** | **410** | **10** | **10** | **480** | **60** |

The total and the residual are calculated within each case before taking medians. The median row is not additive. The protected observer settle completes **before** sample begin; none of the four columns measures or removes it.

## What the boundaries include

The shared launcher in this comparison is the one at `57265de`. In that revision of `packaging/kindle/launch.sh`:

- **Sample begin → go** (lines 522–593, 720–734, 1560–1564): the outer call begins; the worker clears prior result files, starts the `setsid` wrapper, establishes its PID birth, ancestry, group and session, publishes permission, then records `native_read_go`. This is not a standalone fork measurement. The child could already be scheduled after permission publication and before the parent's trace append.
- **Go → ready** (lines 540–600): the wrapper notices permission, runs the command with bounded stdout collection, publishes status; the parent polls and validates the completion record, then records `native_read_ready`. The successful typed display record has **not** yet been parsed by the outer caller. The [Rust capture implementation](../../../src/platform/native_capture.rs) performs protected context validation, multiple power/mode/holding queries as applicable and two metadata observations. Those operations have no individual clocks in these six receipts. The source's polling periods are not measured per-case costs.
- **Ready → reaped** (lines 489–520, 599–609): the parent exits the polling loop, stops the owned process group, reaps the wrapper, cleans the coordination files, then records successful reaping. It is not a timestamp for the getter's own exit.
- **Reaped → sample ready** (lines 617–620, 735–755, 1560–1564): the helper wrapper returns; the launcher reads the bounded output with `native_read_hex`, validates the record and requires a known display state. The next outer mark precedes publishing that state. The 10–20 ms belongs to this whole interval, not to parsing alone.

The [full numerical replay](device-replay.json) also splits three formerly grouped gaps: sample ready → portrait permission **40 ms** (30–50), portrait reaped → cover-query permission **45 ms** (40–50), and cover-query reaped → takeover begin **40 ms** (30–60). Those intervals straddle consecutive operations: publication and next guarded setup; portrait result validation and next guarded setup; cover parsing, startup notification and supervisor dispatch. They are not measurements of individual getters. The earlier [three-read and takeover report](../startup-decisions-20261121/launcher-replay.md#measurements) remains unchanged.

To replay on `test-env`, run `python3 -B replay_device.py /path/to/closed/lib-p-b4a63eed` from this directory. Only the twelve named TSV/worker identity inputs are opened. Report host-fixture or inner-query measurements separately; do not subtract unrelated monotonic stamps from this launcher clock, extrapolate removable milliseconds, or weaken sampling, cleanup or takeover guards based on this table.
