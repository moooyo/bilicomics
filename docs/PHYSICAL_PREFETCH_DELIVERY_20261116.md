# Physical Reader and prefetch delivery: 2026-11-16

Run `prefetch-20261116-r1-443abc8c` exercised manual finger operation with
isolated offline guest data and the socket guard. Its twelve synthetic
`1700 x 2360` PNG pages are not the eight original Perf05 images. No live
account was used. The user confirmed three rounds each of Normal and Fast
reading; the trace includes additional turns and ends on actual page 1.
The previously identified Scribe 2022 reported firmware `5.19.5`, a
`1860 x 2480` visible 8-bit framebuffer, stride `1872`, and raw rotation `3`.

## Frozen candidate

| Item | SHA-256 or identity |
| --- | --- |
| Code | `747aecc65d150f6588beec7dbf0fa4fd8be967e0` |
| Source fingerprint | `40d126600b44ca4169db51db21e3779a21ae7ef26de20318f83b384e79b3a3c4` |
| ARM application | `bd4d51dcf60ebb32854cb00532e913f08ab5ca35f1dfa1cb60f6d87f4f6ffff6` |
| ARM JPEG worker | `e39d7a897f13ce53412f42040d124a106634322899df515b5bc2e96b781914cd` |
| Development ZIP | `d19f7889ef3b9d1ac6fba26c1b18b25dc25b0d628be0350185d77abe013cec63` |
| Numeric summary | [summary.json](physical/prefetch-delivery-20261116/summary.json); `2c13938613ac8797c329428ab1578fcec1a3699112ecb0e3100fcbd5439fb5b5` |

## Observed reading

All 16 actual turn operations published their target models. Hits below require
`prepared_frame_hit` on the same navigation operation.

| Scene | Forward turns / hits | Back turns / hits |
| --- | --- | --- |
| Normal, dithering Off | 5 / 3 | 4 / 0 |
| Fast | 3 / 1 | 4 / 0 |

Normal forward request-to-model times were 27.366-473.376 ms; back times were
482.624-491.643 ms. Fast hit operation 34 took 26.541 ms. Fast forward misses
38 and 42 took 407.784 and 437.041 ms; their predictions were not ready before
input and were subsequently cancelled. Fast back times were 406.867-463.908 ms.
These intervals are not finger-input latency or panel-update timing.

The early-delivery marker occurred zero times. This batch therefore does not
directly demonstrate execution of the
[completed-result collection branch](PERF05_PREFETCH_DELIVERY_20261116.md).
There is no physical before comparison and no supported physical speedup or
29.7 ms claim.

Seven nonempty Fast content frames used DU/nonflash; nine Normal content frames
used AUTO/nonflash. SEND acceptance was true with null errno. Startup separately
used GC16/full. Driver completion for the turn frames was not observed. All 18 UI imports reported
`copied_bytes=0`; JPEG attempts and calls remained zero.

Three accepted captures bind the marker, model, stride, and current EPDC mode:
initial page 1/version 1/operation 2, Normal-final page 2/version 10/operation 27,
and Fast-final page 1/version 18/operation 44. Their reviewed framebuffer-derived
projections accompany the numeric summary; they are not optical panel images.
The [initial](physical/prefetch-delivery-20261116/initial.projected-panel.png),
[Normal](physical/prefetch-delivery-20261116/normal-rounds.projected-panel.png),
and [Fast](physical/prefetch-delivery-20261116/fast-rounds.projected-panel.png)
originals are retained without resizing. The accepted Fast projection contains
only luminance codes `0` and `255`.

## Incomplete idle check and recovery

The final Fast-idle checkpoint overlapped the configured 570000 ms App exit;
the independent session bound was 600 s. Native proof failed with `env=44` /
`stat=41`; no idle checkpoint or capture was accepted. The batch status remains
**failed**, with finalization **released**.

No later native frame or GC16 record appears during the 139.537 s trace interval
after the last Fast native frame. This does not establish a passed before/after
idle pixel-retention check. Manual Full was not tested.

The watchdog recorded `launcher-exit`, launcher status 0, and complete recovery.
All 27 recorded PID/birth identities were gone. Native/statusbar/awesome,
launcher-lock, and owned-process recovery checks passed. Host closure reported
no active owned transports and zero unreadable processes. Exact lease release
checked owner, run, and token. The user explicitly confirmed normal native Home
and touch afterward.

The 714-row performance trace remains private. This report does not establish
optical quality, driver completion, or new device qualification. Raw logs,
tokens, and private transport paths are excluded from the retained summary.
