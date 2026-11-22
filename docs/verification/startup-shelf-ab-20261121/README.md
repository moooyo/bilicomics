# Does rendering before the panel wait speed up the first Shelf?

**The switch works, but this test did not establish a material startup improvement.** On a first-generation Scribe (5.19.5), rendering into private memory first was faster in **4 of 6** pairs. The median paired improvement to the first Shelf render was **14.8–17.3 ms** on a roughly seven-second startup. The [predeclared gate](PLAN.md) required at least five wins and a median improvement of at least 100 ms. Neither condition passed. Keep the existing installation and framebuffer safety barrier; do not claim a half-second gain on this endpoint or make a new rollout on this evidence.

## What was measured

All 14 launches used the same instrumented ARMv7 release executable (74,394,400 bytes), the existing lowercase Library card, and fresh copies of the same offline demo Shelf. There was no forced screen override. The clock starts at the bracketed device-side XTest button release and stops when the Slint renderer returns for the first normal Shelf frame, including navigation and entries; cover placeholders qualify. There was no Shelf input, recording, live account, or optical measurement.

A used `--serial-shelf-render`; B used the default. No other application argument differed except the run-specific private paths. The actual first draw, not the requested option, recorded `wait_then_render` in all six timed A launches and `render_then_wait` in all six B launches. Both forms retained the write barrier and the new update's marker. All frames qualified as `ready` and all optional same-marker completions were observed.

The two warmups (A, B) were excluded. All six timed pairs ran as planned, with no replacements, on one device boot with distinct application processes. Between pairs 3 and 4 another project used and returned the device under its own leases; no BiliComics pair straddled that interruption. Host and device caches were not reset. Each BiliComics visit naturally exited, restored the installed entry and native navigation, removed its temporary payloads, closed connections and released its own exact lease. The shipping application was not replaced.

## Paired results

Times are **B minus A**, in milliseconds; negative favors the default. Each interval brackets uncertainty in button release, not statistical confidence. Bounds below are rounded outwards; the [complete machine-readable analysis](physical-result.json) includes the original per-arm intervals in nanoseconds.

| Pair | Order | Card to Shelf render | Card to accepted SEND | Card to driver completion |
| ---: | :---: | ---: | ---: | ---: |
| 1 | AB | −102.193 to −99.849 | −102.492 to −100.148 | −102.401 to −100.058 |
| 2 | BA | −6.531 to −3.697 | +8.390 to +11.223 | +5.612 to +8.445 |
| 3 | AB | −82.869 to −80.546 | −40.193 to −37.870 | −40.400 to −38.077 |
| 4 | BA | +5.061 to +8.240 | +30.655 to +33.834 | +27.807 to +30.986 |
| 5 | AB | +285.319 to +288.298 | +306.894 to +309.873 | +306.698 to +309.677 |
| 6 | BA | −28.044 to −25.830 | −57.619 to −55.405 | −47.879 to −45.665 |
| **Median paired delta** | | **−17.287 to −14.764** | **−15.901 to −13.323** | **−17.394 to −14.816** |
| **B faster** | | **4/6** | **3/6** | **3/6** |

The typical absolute software-render time was about 6.983 s in A and 6.938 s in B (arm medians). Subtracting these two numbers does not give the median paired difference. Two of three pairs favored B in each order; the AB median improvement was about 82 ms, the BA median about 5 ms. Three pairs per order are not enough to establish an order effect. The [historical 585 ms result](../../physical/shelf-render-20261121/README.md#device-results) used different builds and a driver-confirmed, cover-complete Shelf; it is not the result of this experiment.

## Where to look next

It took about **3.69 s in A and 3.67 s in B** (arm medians) just to reach the first application trace header. That includes native dispatch, launcher work, process loading and early app setup; the header is not process entry. The paired median favored B by 16.3–19.1 ms before that header, but the header-to-render span was 11.7 ms longer in B at its paired median. Segment medians are not additive. In pair 5 the latter span alone was about 273 ms longer in B; this does not identify a cause. Next, measure native dispatch, safe launcher handoff and early app initialization separately, on the same clock, before changing them. Do not remove panel waits, the loading feedback, or cover placeholders to improve this figure.

Before device admission, `test-env` passed 924 application/library tests (five ignored), 252 Kindle-backend tests and 74 route, controller, audit and analysis tests. The full application suite was run single-threaded with a private umask after parallel lock contention and a nonprivate fixture directory affected earlier attempts. The [emulator receipt](emulator/results.json) covers four ARM user-mode guard checks and two real desktop window checks; these are neither full-firmware timing nor a physical Paperwhite 6 run. The [operator notes](RUNNER.md) describe isolated staging and restoration. Only the anonymous records required for replay were archived in `artifacts/verification/shelf-render-ab-20261121/closed-receipts.tar.gz` outside Git; no device credential, account data or application payload is included.
