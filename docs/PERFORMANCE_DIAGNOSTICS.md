# Performance Diagnostics

Version `0.1.20` adds optional observation of the existing startup, reader and
native presentation paths. It does not change font admission, task priorities,
pixel algorithms, refresh policy or the launcher's two-second deadlines.
Verification of this candidate is recorded separately from the accepted
`0.1.19` physical result.

## Activation and ownership

Pass `--performance-trace /absolute/path/to/new.jsonl` to an isolated application
run. The parent directory must already exist. Diagnostics are disabled by
default; the application does not create a writer thread or keep a window alive
for them. A disabled path does not serialize records or sample stage clocks.

The output is a newly created private regular file. Existing output is never
overwritten. If configuration fails, the application reports the error kind and
continues with diagnostics disabled. A path identical to the lifecycle output
also disables the new diagnostic to avoid mixing formats. After configuration,
an I/O failure or size limit stops diagnostic output without failing the business
operation. The limit is 8 MiB including the terminal record, with a 64 KiB maximum
complete JSON line. Verification must inspect completeness; an absent or stopped
trace is not a successful measurement.

The physical verification harness supports `start --performance-trace`. It only
adds the option to its isolated offline demonstration UI process; native capture,
portrait publication and power relay helpers keep their existing command lines.
It collects `performance.jsonl` separately from the historical lifecycle trace.
A physical run still requires a fresh exclusive handoff, lease and native return.

Records contain static categories, opaque numeric identities and quantitative
timing/geometry/resource fields. Do not add account names, comic or episode IDs,
paths, URLs, credentials, raw errors, pixels or input coordinates to traces.
Content hashes are forbidden in real-account traces. Isolated synthetic
`--demo` runs may record prepared RGBA and native RGB fingerprints to distinguish discrete geometry replacement
from repeated UI feedback. Fingerprinting is off by default in the backend,
requires enabled diagnostics and explicit demo activation, and is reset when
the callback is replaced or cleared. It never adds raw image bytes to a trace.
Real-account traces remain private; committed verification must use fictional
fixtures and an explicitly filtered export.

## Clocks and format

Each JSONL envelope includes:

- `schema_version`, `process_id`, `operation_id`, `kind` and `stage`.
- `monotonic_ns`, `thread_cpu_ns` and `thread_id`, or null when unavailable.
- `trace_elapsed_us`, a fallback elapsed clock from diagnostic configuration.
- `data`, containing only that stage's quantitative fields and fixed categories.

The Linux monotonic clock can correlate UI, blocking-worker and native-backend
events in one process. `trace_elapsed_us` starts at configuration, not the main
entry; do not treat it as the historical App `elapsed_ms`. Preserved startup
observations can carry their original captured timestamp in `data.observed`.
Do not derive elapsed CPU from two different thread IDs. A stage span's `cpu_us`
is null when a reading is unavailable, belongs to another thread or goes back in
time. It is not replaced with zero.

Serialization precedes the short append lock, so records from different threads
need not appear in timestamp order in the file. Analyze operation identities and
captured monotonic times, not line order alone.

Diagnostics are synchronous and have observer cost. Inner samples finish before
their corresponding record is serialized. Outer `worker_end` and `ui_apply_end`
spans include nested diagnostic serialization/writes performed inside the work
or callback. They are inclusive wall/CPU observations, not pure business CPU;
subtracting leaf totals from them does not isolate a clean residual stage.
Compare trace-on/off runs of the same binary and preserve that limitation.

The file begins with `diagnostic/trace_started` and normally ends with
`diagnostic/trace_stopped`, reason `finished`, with `truncated=false`. A size,
serialization or I/O stop invalidates assumptions about missing events. A write
failure can also make a terminal record impossible; the process-local diagnostic
status distinguishes this case. Diagnostic sampling does not request a refresh
or add a completion wait. The native presentation policy can require checked
EPDC marker completion before reusing its mapped pixels and after complete
replacement. Its trace reports `driver_completion_observed` separately from
physical panel observation; the driver receipt never proves optical behavior.

## Reader operation path

A same-chapter navigation starts with `reader/navigation_request` before the
action handler changes the core. Only the allowlisted action category is logged,
not its value. Initial rendering starts with `reader/render_request` and records
the numeric epoch, reader generation/version and source kind (`demo` or `receipt`).
Queued intents retain their operation identity when replayed. A coalesced request
can be superseded, retired or cancelled without ever publishing an image.

The successful path is:

`render_request -> job_queued -> delivery_wait_begin -> delivery_acquired ->
compute_wait_begin -> compute_admitted -> worker_begin -> reader_preparation ->
worker_end -> mailbox_publish_begin -> ui_apply_begin -> ui_image_import ->
ui_apply_end -> reader_model_published`

`mailbox_publish_begin` is deliberately captured before publication, not labeled
as a completed publish. A fast UI consumer can run immediately after actual
publication. `compute_admitted` records the execution memory reservation; it is
not a measured RSS cap.

`reader_preparation.data` contains an outcome, a preparation span, the existing
reader profile and `stage_scope`. The profile preserves historical `*_us` totals
and adds a `stages` map. Entries include wall/CPU totals, attempts, completed and
partial counts. Missing keys mean the stage was not attempted. The fixed source,
receipt, integrity, decode, gray, layout, resize and canvas leaf names are
documented by `ReaderPreparationProfile::STAGE_SCOPE`; aggregate parents must not
be added to their own children. Errors and cancellation can leave partial stages.

`ui_image_import` measures the existing RGBA copy into Slint and records the
copied byte count. The marker is bound to the operation only after its image is
published to `ReaderModel`. Closing or failing the reader clears native association.
Sleep cancels undisplayed-target identities along with their navigation, retains
the current frame's identity, and allows it to be rebound for a retained redraw.
Initial loading without a presented frame follows the existing continuation rule.

Cross-chapter navigation opens another reader generation and operation. The
current diagnostic does not transmit a navigation parent ID across that handoff;
do not interpret an unmatched old navigation as a failed same-chapter turn or
use it to report complete cross-chapter latency.

## Startup and local continuation

Startup operation 0 records the first driver entry, gate changes, worker queuing
and execution, font registration/finish, per-face preparation, App preparation
substeps, attachment stages and UI readiness. Face records contain an index and
byte counts rather than filenames. Original readiness observations are preserved.
The diagnostic does not change the first-scene-before-worker rule.

`startup-manager` and `download-recovery` operations use numeric parent links to
show manager construction, installation, recovery admission/work and restoration.
Strict offline recovery uses `pause_all`, not an online resume. These events expose
the real local-resolver and reader-lease dependency that the demo path lacks.
They do not move restoration or drain normal jobs early during startup.

Use a fictional non-demo account fixture with real local receipts, a saved
nonzero anchor and `launch=resume` to verify this path. No auth session is needed.
The existing missing/corrupt and bounded SQLite pause fixtures remain separate
negative cases; a new trace must not relax their original fault windows.

## Native presentation

`startup/native_initial_setup` records initial I/O, framebuffer and RGB-buffer
preparation, window setup and observer work. `native/frame` is emitted only for
actual drawing or refresh activity, not idle poll iterations. It contains a
render sequence and the associated reader operation, or null when no reader
image is associated. Multiple redraws can legitimately share one reader operation.

The frame sample separates Slint render, grayscale conversion/framebuffer writes,
refresh submission and existing lifecycle completion work. The callback runs
after the framebuffer and lifecycle locks are released. Submission reports retain
the attempted ioctl count, accepted state and numeric errno without changing
ABI fallback, waveform, marker or wait behavior.

An accepted ioctl means the kernel accepted the update request. It does not mean
that the panel finished changing pixels. `panel_completion_observed` remains
false. Desktop parity cases do not invent native EPDC events.

## Verification expectations

Run all checks on `ssh test-env`, using isolated workspaces and bounded build
concurrency. Compare the same executable with diagnostics disabled and enabled:
full reader pixels, source/anchor/settings, receipt validation, retained sleep,
resource ownership and natural shutdown must remain correct. A pre-existing
diagnostic output must be preserved while the application continues normally.
Readiness and observation timestamps are not moved to make a run appear faster.

After software and ARM gates pass, a fresh physical handoff can supply ARM stage
attribution. Use the resulting distribution to choose a measured bottleneck;
neither warm x86 resize proportions nor a single software interval supplies a
complete touch-to-visible or normal-launch-to-visible measurement.

## Prepared-next-page observations in 0.1.22

The optional `reader-prepare` operation records immediate admission or a busy
skip, actual worker execution, preparation, result delivery, and retained byte
capacity. A `next_frame_retained` record is a completed cache entry, not a
displayed image or a saved reading position. Normal foreground reader operations
record candidate lookup separately from `prepared_frame_hit`, changed source,
or missing source. A hit is reported only after a fresh source read/hash and
the current receipt/object recheck. Its profile has no resize or canvas work,
while the subsequent UI import still copies the retained RGBA image.

Resource snapshots expose the single slot's state, actual retained bytes, and
32 MiB budget. Cancellation or a retired ticket does not prove that the blocking
worker has exited; its execution-end record and resource ownership must be
considered separately. An admission skip is an ordinary miss, not a failed
foreground turn. See the [streaming and prediction record](PERFORMANCE_STREAMING_PREFETCH_20261110.md)
for the candidate scope and independent verification categories.
