# Linux attribution of the stock display read

**The shell's observation and reaping time is not the Rust helper's execution time.** On `test-env`, the uninstrumented stock transaction took **68.289 ms** at the median on the simulated automatic Scribe path (66.935–89.238 ms) and **68.235 ms** on the fixed PW6 path (67.199–71.379 ms). Neither is a Kindle timing or an optimization result. Both A/A labels used identical executables; only the geometry and capability fixture differed between profiles. No account, device, real framebuffer, native LIPC daemon or physical lease was involved.

The [per-visit measurements](host-measurements.json) retain the six AB/BA pairs per geometry, two excluded warmups each, and separate diagnostic runs. Median within-pair B minus A was −0.317 ms on Scribe (−1.779 to +21.491 ms) and −0.313 ms on PW6 (−1.994 to +3.373 ms). Empty request/response medians were 0.049 and 0.065 ms; no overhead was subtracted.

## Inside the shell transaction

The stock `native_read_command` created a new private process group, published a real read permission, observed the bounded result, then retired the group. Rust performed its genuine private-directory and `/proc` checks on the live process tree. The fixture substituted only its outer program/CLI, the getter path, the hardware observations and the underlying property provider. Each getter was a separate process, not a Rust callback. Both sides used Linux monotonic time, but shell marks are the parent's receipt of a short pipe notification. A receiver may run late. The reported intervals are observations, not exact fork, exec or exit instants.

| Median of three separate diagnostic visits, ms | Scribe (automatic) | PW6 (fixed) |
| --- | ---: | ---: |
| Sample begin → permission (`go`) | 24.031 | 23.183 |
| `go` → parent observed valid `ready` | 22.088 | 22.223 |
| `ready` → native group reaped | 21.825 | 22.405 |
| Reaped → parsed display result | 1.271 | 1.394 |
| `go` → Rust helper entered measurement | 4.584 | 3.670 |
| Inside the Rust capture helper | 7.097 | 4.986 |
| Rust capture returned → shell observed `ready` | 11.547 | 14.595 |
| Initial permit acquisition | 0.186 | 0.174 |
| Subsequent full authorization rechecks combined | 3.401 | 2.552 |
| Mock getter bodies combined | 0.062 | 0.036 |

The last six rows are **not additional phases to add** to the shell rows. The go-to-entry interval includes gate observation, process creation, loading and scheduling; the return-to-ready interval includes writing the diagnostic summary, helper exit, the wrapper's collector/status files and the supervisor's polling. The stock 10/20 ms sleeps are retained. The driver metadata and synthetic getter values are not device work. All three Scribe diagnostics had one acquisition, **31 rechecks, 288 `Writer::process` calls, five getters** and two metadata reads. The three PW6 diagnostics had one acquisition, **23 rechecks, 216 `Writer::process` calls, three getters** and two metadata reads. Periodic rechecks or a different process tree can change those counts. An immutable parser cost estimate, if useful, must not substitute for the full authorization cost.

## Commands and process creation

A further three diagnostics per profile timed the real `Commands::run` and its nested `Command::spawn` call. The following are sums within each launch, then medians across launches. The remainder was calculated **per launch**, not by subtracting column medians.

| Per-visit metric (ms) | Scribe: five getters | PW6: three getters |
| --- | ---: | ---: |
| Inclusive command runner | 4.680 | 2.078 |
| Time in `Command::spawn` | 0.681 | 0.318 |
| Runner-invoked permission checks | 1.114 (10 checks) | 0.469 (6 checks) |
| Remainder after runner checks and spawn | 2.790 | 1.295 |

One Scribe visit spent **25.364 ms** in the runner, of which **23.724 ms** remained after spawn and checks. It is retained. This remainder mixes concurrent getter execution, output/exit polling and scheduling. The getter can execute before `spawn` returns; do not subtract the mock getter body a second time. These runs were separately instrumented, not spliced into the uninstrumented A/A visits. In particular, none of the host values identifies the cost of one LIPC call on the Kindle. The [six historical Scribe samples](DEVICE_REPLAY.md) only bracketed the display read between the launcher's `go` and observed `ready` (median **410 ms**, 10 ms resolution).

## Guard controls and reproduction

All seven controls passed on both profiles: good response, inactive at entry, inactive on the final power read, malformed mode, nonzero getter exit, deadline and active revocation. The blocking getter recorded entry before the timed fault. Timeout returned in **2.009/2.006 s**; unlinking this fixture's read permission during the live getter returned in **175.8/173.0 ms**. The launcher reaped its group and the host verified no fixture descendants remained. These controls do not establish all possible revoke races. No live authorization check, deadline or display sample was removed.

Two preliminary fixture errors are excluded from all published numbers. An early Linux master inherited an unusable process-group context; the test now starts it in its own session. A first polling shim recognized `/path/usleep`, but not `usleep` as supplied by the invoking shell; it returned instead of sleeping. The final run separately asserted real 10 and 20 ms sleeps and that the blocked-getter deadline and revocation had the expected separation before timing any pairs. All reported visits use this corrected fixture. No incomplete run was used to pad a pair.

Use the [frozen protocol](HOST_PLAN.md) with the 80,185-byte stock launcher and the matching `src/platform/{mod.rs,native_capture.rs}`, `src/native_presentation.rs` and `src/native_presentation/portrait_writer.rs`. Those four files were checked byte-for-byte against the retained November 21 stock source. Resolve `anyhow` and `libc` to the audited versions `1.0.104` and `0.2.191` from the source lock. Run in an isolated Ubuntu workspace with an empty result path and only loopback networking:

```sh
python3 -IB host_bench.py --source /work/source --stock /work/study/stock.sh --output /work/evidence/host-qualified.json
python3 -IB host_bench.py --source /work/source --stock /work/study/stock.sh --command-spans --output /work/evidence/command-spans.json
```

The harness uses temporary Release builds of two small fixture programs in `/tmp` and removes them on exit. The fixture is not a production executable or an authorization mechanism for a physical device.
