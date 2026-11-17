# Native-query startup prototype: not adopted

Date: 2026-11-17, Asia/Shanghai.

The native-query prototype is **not adopted**. Its local sampling stage became
shorter, but six matched warm-start pairs did not show a consistent improvement
from launch request to the first business frame. Prototype transport, helper
build, and package-checker changes stay off `main`; the runtime prototype is
retained at `f574b2692e537f14b54f99f8f0f41b6e4c0885d6` on
`codex/startup-native-query`
(implementation `74c259338f1cfc234f92a160ccf582dbe292c8d1`). Main receives the
research and evidence; runtime remains the `f50ada1` baseline's 542 compiled
inputs, fingerprint
`33bda43e34f74c7e73058880b4a1ae7a8d4b00d3ec0ae9a1ee22a074b59beb28`.

## Physical result and retained failure

Batch `native-query-20261117-97f1de5a` completed all fourteen predefined primary
starts: two warmups, then six alternating AB/BA pairs. The **whole batch remains
failed**. Its separate eight-opcode diagnostic refused reserved descriptor 7
before starting the C helper: PID, executable and hash are null, and no stdin was
sent. This is not returned bad ABI data or a completed eight-operation ABI test.

A separate strict primary projection reused the frozen source, clock,
lifecycle, pixel, capture-window, and release checks. It qualifies only the
predefined primary dataset, without changing raw success fields or promoting
the entire batch to passed. Compatibility and diagnostic extras and both
warmups are excluded from the following statistics. All six pairs, including
slower pairs 1 and 6, are retained. Values are milliseconds, rounded for display.

| Pair | Baseline ready | Prototype ready | Baseline SEND | Prototype SEND |
| --- | ---: | ---: | ---: | ---: |
| 1 | 5796.202 | 6834.653 | 6581.684 | 7366.525 |
| 2 | 5832.594 | 5634.589 | 6628.367 | 6408.192 |
| 3 | 5935.839 | 5686.852 | 6742.579 | 6450.548 |
| 4 | 5887.711 | 5851.657 | 6654.538 | 6666.807 |
| 5 | 5871.855 | 5744.811 | 6647.538 | 6634.395 |
| 6 | 5857.746 | 6516.710 | 6640.246 | 6989.060 |
| Median | 5864.801 | 5798.234 | 6643.892 | 6650.601 |
| Mean | 5863.658 | 6044.879 | 6649.159 | 6752.588 |

Readiness was faster in 4/6 pairs, but its mean worsened by 181.221 ms. SEND
was faster in 3/6 pairs; its median worsened by 6.709 ms and mean by 103.429 ms.
These are instrumented native-request-to-software-ready and first-business-SEND
acceptance endpoints, not optical panel completion or all-cover readiness.

Stored launcher TSVs show a narrower sampling envelope:
`startup_sample_begin` to ready fell from median 655 ms (580–750) to 385 ms
(350–400), shorter in all six pairs. The uptime clock has 10 ms resolution and
this interval includes surrounding work; it is not pure LIPC time. Its 270 ms
median reduction did not become an equivalent whole-startup benefit.

An excluded diagnostic Rust session completed five queries in 109.619 ms from
`session_ready` to `session_finished`, excluding handshake and the complete
capture. This independent success does not repair the refused eight-op probe.
The helper-missing compatibility case used application
`806a406dba49b4cb671f3670a1c0106b2dffae6b592409f0f47aa589409b1de6`
in a separate directory, omitted the helper, exited zero, and matched final
logical pixels. It is not a primary timing sample.

Early/settled framebuffer memory, final logical images, actual capture times,
and liveness evidence remain in the [physical research bundle](physical/native-query-20261117/README.md).
Recorded native restoration, selected-file preservation, lighting restoration,
process retirement, transport closure, stage removal, and exact lease release
passed. Native control has been returned. This establishes no sleep/resume,
complete eight-opcode ABI, or optical qualification. No further power-key or
file-descriptor experiment is planned for this unadopted prototype.

The strict projection SHA-256 is
`1475464028f82974e5f58ed0175ff66730bdd162c1c58e55cc9b1b3acc4bb577`;
its `project_primary.py` SHA-256 is
`e991f01792fa9b771d12a64c18e450c1435c7aca7ca24d8f22a374e451fbc01e`.

## Prototype and software evidence

The [source provenance](research/aidoku-native-query-20261117/README.md) retains
Aidoku's exact reference and license. The prototype preserves automatic
power/mode/holding/holding/power and fixed power/mode/power capture orders,
framebuffer/sensor fences, a two-second parent deadline, raw UTF-8 validation,
poisoned connected failures without CLI splicing, pre-query fallback, and
asynchronous owned-child cleanup. Its separate eight-operation read-only helper
has a five-second backstop and parent-death handling. Aidoku's first-view status
batch was not imported; BiliComics battery/light reads already run in background.

The [software bundle](verification/native-query-20261117/README.md) retains
904 library passes, four ignored tests, 11 main passes, 14 C/mock passes, and
44 new plus 16 existing package regressions. Runtime tests executed on
`74c2593`; its 545-input manifest matches `f574b26` byte for byte. Separate fresh
targets rebuilt the authoritative binaries. Static checkers passed all 16
baseline checks (42 entries) and 19 prototype checks (46 entries). Software R2
denotes the checker revision, not controller revision 2.

The first R1 package remains rejected for unreviewed `libpthread.so.0`.
The [ABI audit](research/aidoku-native-query-20261117/LINK_ABI_AUDIT.md) explains
why Zig's already-enabled `--as-needed` retained ordinary `read`, `write`,
`__errno_location`, and `sigaction` imports through pthread at `GLIBC_2.4`.
The experimental checker pins exactly libc/libdl/libpthread and those imports;
App/JPEG remain static musl. This audited exception is not adopted into main.

Earlier No-route `fe7fc669`, unacquired `f50724b2`, and lock-observer-failed
`105808c7` receipts remain separate. Controller 2 subsequently ran 33 control
tests (28 original plus five new); the other 92 prior host tests were not
rerun. There are 125 defined fixtures, not 125 newly executed passes.

The retained cache audits found no evidence of the specific stale diagnostic
backend contamination. Demonstrated workspace-path differences do not prove
whole-binary equivalence. [Earlier published startup results](STARTUP_PERFORMANCE_20261117.md)
remain unchanged and bound to their own executables; neither they nor Aidoku's
percentages are benefits established by this prototype.
