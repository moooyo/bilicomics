# Prepared state to UI receive

**Physical result: none of three launches reached the predeclared 250 ms threshold.** On the Scribe (5.19.5), the prepared-state timestamp to the UI's first successful receive was **0.082, 105.155 and 33.879 ms**. All three reached the complete populated Shelf and exited normally. This does not exclude an occasional longer delay; it does not justify changing wake delivery, skipping a loading frame or weakening a display barrier.

| Visit | Prepared → first UI receive | Card release → first complete software render | Card release → accepted SEND | Card release → same-marker driver completion |
| --- | ---: | ---: | ---: | ---: |
| 1 | 0.082 ms | 6.339–6.342 s | 6.380–6.383 s | 6.743–6.745 s |
| 2 | 105.155 ms | 6.372–6.374 s | 6.406–6.408 s | 6.770–6.773 s |
| 3 | 33.879 ms | 6.264–6.267 s | 6.305–6.307 s | 6.670–6.672 s |

The intervals round the card-release brackets outwards. Each UI receive occurred in `runtime_wakeup`; this identifies the callback that consumed the result, **not what caused it to wake**. The [exact durations](physical-result.json) come from three new single-arm visits, not the [identical-launcher A/A](../startup-clock-aa-20261122/README.md). The two studies used different instrumentation and cannot be subtracted to estimate a speedup. Each visit restored the original launcher and native library, closed its connection and released its own lease. The final release receipt was recorded at **05:43:47 Asia/Shanghai**. The daily installation is unchanged.

## Why measure receipt?

The November 20 replay found four cases in which the producer observation was
1.710–11.574 ms after a loading-frame wait began and the UI performance record
followed 254.561–275.955 ms later. The historical end is a recording timestamp,
not the exact receive boundary. The fixed-ELF A/A cannot add that boundary to
archived records. See [the replay](../../STARTUP_PRE_READY_20261121.md#what-the-stored-cases-can-tell-us).

For a separately qualified native diagnostic build, use a fresh opaque
run name of the form `bili-shelf-<32 lowercase hex>` and a private staging
directory. Keep the ordinary native Library route and isolated offline Shelf
fixture. Add these options only to the authorized diagnostic app invocation:

```sh
--shelf-render-trace "$PRIVATE_DIR/$RUN.jsonl" \
--prepared-handoff-trace "$PRIVATE_DIR/$RUN.prepared-handoff.json"
```

The first successful UI-thread receive of `StartupEvent::State` observes two
timestamps on `CLOCK_MONOTONIC`. `prepared_monotonic_ns` is the timestamp already
carried by the prepared result immediately before `send`; it is not the exact
end of a worker substage or proof the channel already held the result.
`received_monotonic_ns` is read in that `try_recv` success arm, before retaining
the state or writing startup logs. The difference includes publication, dispatch,
and any scheduling or UI work before receive. `receive_source` distinguishes
`runtime_wakeup` from `maintenance_timer`; a runtime wake is shared and does
not identify its publisher or prove that a dedicated wake was lost.

Only the existing first qualifying whole-window Shelf render can qualify the
sample, with render time at or after receive. A missing clock, missing frame,
non-Shelf destination or unsuccessful exit produces no sidecar. On successful
exit the program creates a separate private JSON file containing one record:
`schema` (`kindle-prepared-handoff-v1`), `run_id`, `boot_id`, `process_id`,
`clock` (`CLOCK_MONOTONIC`), the two timestamps and `receive_source`. It does
not change the Shelf JSONL. Validate the run, boot, PID and render time against
the audited Shelf stream before subtracting timestamps. Neither a render nor
this handoff is an optical, input or startup improvement measurement.

On `test-env`, the source-only tests run in an isolated Linux workspace without
Cargo or an ARM build:

```sh
rustc --edition=2024 --test /work/prepared_handoff.rs -o /work/prepared-handoff-tests
/work/prepared-handoff-tests
```

**Host integration verified, November 22.** The real Linux application test
binary was built on `test-env` with the Kindle backend, without the desktop
feature, on `x86_64-unknown-linux-gnu` with Rust 1.99.1:

```sh
cargo test --locked --offline --no-default-features --features kindle --bin bilicomics -j1 prepared_handoff
```

All five filtered-in tests passed: four exercise capture, eligibility and
private single-use sidecar output; the fifth parses the real application CLI
and rejects conflicting instrumentation. Twenty other binary tests were not
selected. The [host build receipt](host-cargo-result.json) records the command,
names and resource bounds. The private, nonincremental build ran in a network
namespace using the shared build lock; its temporary output was removed. This
is not an ARM build or a Kindle run. The separate physical study above established
the same-launch sidecar on natural exit; the fixed-ELF A/A cannot supply that evidence.

**[ARM qualification, November 22](arm-qualification.json).** The same source produced one 74,406,144-byte
static ARMv7 hard-float application in a single-job, network-isolated release
build (428 seconds); the JPEG worker did not change. Under QEMU, both Scribe
and PW6 recognized the two diagnostic options and rejected the missing native
launcher snapshot. Neither rendered a Shelf or wrote a handoff sidecar. The
real CLI rejected a missing Shelf trace and a conflicting lifecycle trace.
Five route/guard tests and nine collector tests also passed on `test-env`.
The [three-visit protocol](PLAN.md) is separate from the fixed-ELF A/A:
neither emulator refusal nor a synthetic collector test measures UI delivery.
