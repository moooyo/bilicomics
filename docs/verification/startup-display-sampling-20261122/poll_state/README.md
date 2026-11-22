# What did the getter's `poll` wait for?

**The probe distinguished two controlled Linux cases. It cannot divide the earlier Scribe measurements.** When the synthetic getter kept stdout open for 120 ms before writing, most `poll` wall time was on a real pipe descriptor. When it wrote the same record, closed stdout, and stayed alive for 120 ms, most was in polls with `fd == -1`.

| Getter case (one split run each) | Polls before observed EOF: calls / ms | Polls after observed EOF: calls / ms | Result, both variants |
| --- | ---: | ---: | --- |
| Hold open, then write and exit | 14 / 121.221 | 1 / 10.072 | Parsed `Y8` |
| Write, close, hold, then exit | 1 / 1.039 | 12 / 120.973 | Parsed `Y8` |
| Write, close, hold, then exit 7 | 1 / 0.880 | 12 / 120.923 | Rejected nonzero exit |
| Write, close, then outlast the deadline | 1 / 0.341 | 199 / 1998.052 | Rejected at the two-second deadline |
| Write, close, withdraw the fixture permission, stay alive | 1 / 0.361 | 10 / 101.007 | Rejected before the deadline |

The buckets classify the descriptor **on entry to each existing poll call**, not whether bytes became available during the call. The `-1` case occurs after a zero-byte read when a successful child exit is not yet confirmed. The ignored descriptor cannot signal child exit; the existing timeout can still elapse. A real descriptor does not prove the getter was computing throughout the call. Both buckets include scheduling time. Neither is a measurement of removable latency.

## Scope and controls

[run.py](run.py) takes the actual `Commands::run`, `OwnedChild`, deadline/context checks, record parser, limits, and poll intervals from the retained stock `src/platform/native_capture.rs`. The control compiles those sections unchanged. The diagnostic copy wraps only the existing `libc::poll`: same descriptor, timeout, events, return value and error handling. The observer reads a monotonic clock, preserves `errno` on error, accumulates two in-memory count/time pairs, and emits the totals only after the run. No sampling loop, wake-up or command retry was added. This is a single getter fixture, not the full display transaction.

The [fixture](fixture.rs) spawns an actual child. Both success cases returned the same `Y8\n` bytes and passed the stock record parser. In both variants, the nonzero child failed even after delivering that record; timeouts returned at 2.001/2.000 s (stock/split), and withdrawal at 102.371/101.655 ms. Withdrawal removes a synthetic permission file checked by the original callback schedule, not a real `NativeReadPermit`. All ten getter processes were verified reaped. [Raw results](results.json) retain per-run wall time, callback counts and both incremental poll totals. The two builds are a behavioral control, not a speed comparison.

The [three previously archived physical captures](../physical-result.json) recorded **only the unsplit `command_poll` bucket** (median 213.896 ms within a 300.502 ms median helper). They contain no descriptor-state evidence. Do not apportion that time using this host fixture or infer a startup saving. No Kindle, physical lease, framebuffer, actual LIPC service, or shared build reservation was used here.

## Reproduce on `test-env`

Stage these two fixture sources in a fresh private work root under `fixture/`, the matching stock `Cargo.lock` and `src/platform/native_capture.rs` under `source/`, and create an empty `evidence/` directory. Run once with the toolchain's isolated Ubuntu runner; the workspace is mapped to `/work`. The script requires Linux, loopback only, and no framebuffer or input device. It builds just the fixture twice, using the locked `libc` offline with one Cargo job, in a temporary directory removed on exit. There is no full application build.

```sh
/opt/kindle-toolchain/ubuntu-run "$study" unshare --net -- \
    python3 -IB /work/fixture/run.py /work/source /work/evidence/poll-state.json
```
