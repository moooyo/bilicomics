# Physical device verification: 2026-11-05

The retained evidence supports the specific offline demonstration checks below on one first-generation Kindle Scribe running firmware `5.19.5`. This is a limited physical result for a frozen development candidate. Later changes, including the subsequent immersive-reader UI work, do not inherit this acceptance.

The first reader run retained an inconclusive first post-wake tap because it coincided with the configured four-minute exit window. A separate reader run subsequently established the post-wake finger interaction. The original inconclusive result remains part of this report.

## Frozen candidate

| Item | Recorded value |
| --- | --- |
| Application version | `0.1.3` |
| Application source inputs | `339` |
| Source-input manifest SHA-256 | `19e4eeadb21394d5481487e3391a2a0dc46a3dd678525e2d01c6dafc55d44bdb` |
| Pipeline-input manifest SHA-256 | `986595723bbb015c7bce1fbadfc045a40ebdd2733fd20fd5adeac5cdf39b5cf9` |
| ARM executable SHA-256 | `fa819c514400c84be113a2e6d4feeeb2931bf41014bc63a16098736e5f87fbfb` |
| Development ZIP SHA-256 | `f5ba2203c7342b00c5afd639695dab48bd5476fc849197b0c8b3d80ece4136d1` |
| Packaged launcher SHA-256 | `427540e6cd615b9bd2352b8f1b9a97d170393e33229fbef14b1abeda109863c0` |

The [candidate summary](../artifacts/verification/physical-20261105/candidate/candidate-summary.json) records a fresh ARM build and successful static package checks on `test-env`, with unchanged source inputs, pipeline inputs, and lockfile during that build. The [source comparison](../artifacts/verification/physical-20261105/candidate/source-comparison.json) binds the 339 application inputs to the frozen manifest. Those statements describe the archived candidate, rather than the current working tree after later edits.

The main ARM build retained two `cc`/`cargo-zigbuild` wrapper warnings containing `aeabi_unaligned.c` and `FileNotFound`. The successful build was not warning-free. The separate read-only framebuffer helper has its own [build record](../artifacts/verification/physical-20261105/candidate/helper-build.json), including the earlier failed helper-build attempt; it does not replace or erase the main-build warnings.

## Device and controlled scope

| Item | Observed value |
| --- | --- |
| Device | First-generation Kindle Scribe |
| Firmware | `5.19.5` |
| Selected design profile | `scribe` |
| Visible framebuffer | `1860 x 2480` pixels |
| Virtual framebuffer | `1872 x 4960` pixels |
| Stride | `1872` bytes |
| Pixel format | 8-bit grayscale |
| Panning offsets | `0, 0` |
| Reported framebuffer `rotate` | `3` |
| Reader configuration exercised | Portrait page reading, `ltr`, `fit=page`, `launch_preference=resume` |
| Data and connectivity | Isolated `--demo --offline` data |

The `rotate` value above is the driver's reported framebuffer value. The retained reader anchors separately report reader rotation `0`. The exact profile/firmware entry was selected only in each isolated run's runtime data. No repository allowlist entry or general hardware-support certification was created.

The physical host harness, application wrapper, watchdog, read-only framebuffer helper, and test-only power-event identity shim formed a controlled supervisor. The shim preserved the native power-event arguments and executed the original command. These additions do not establish that every production-launcher PID signal has been hardened with process birth and executable checks. The archived [harness preflight checks](../artifacts/verification/physical-20261105/harness-preflight-checks.json) describe the checks performed on `test-env` before these runs.

## Run results

| Run | Configuration | Supported result |
| --- | --- | --- |
| [smoke-01](../artifacts/verification/physical-20261105/smoke-01/run.json) | 30,000 ms after readiness; 90 s initialization and 180 s session limits | Shelf displayed; launcher exit `0`; the user confirmed normal display, automatic exit, native Home, and native touch. |
| [reader-01](../artifacts/verification/physical-20261105/reader-01/run.json) | 240,000 ms after readiness; 90 s initialization and 360 s session limits | Finger input advanced page 18 to 19. Suspend and resume retained page 19 and its full anchor in the same process. The first post-wake tap remained inconclusive. Launcher exit was `0`, with successful recovery. |
| [reader2-01](../artifacts/verification/physical-20261105/reader2-01/run.json) | 480,000 ms after readiness; 90 s initialization and 540 s session limits | Suspend and resume retained page 18 and its full anchor in the same process. The user's first finger tap after wake advanced to page 19 while the application remained in the reader. The coordinator then stopped the run; launcher exit `143` accompanied successful recovery. |

In `reader-01`, the native application identity remained PID `28634`, start ticks `694109659`, with the pinned ARM executable. The [page-19 checkpoint before sleep](../artifacts/verification/physical-20261105/reader-01/reader-after-finger.checkpoint.json), [suspended checkpoint](../artifacts/verification/physical-20261105/reader-01/reader-suspended.checkpoint.json), and [resumed checkpoint](../artifacts/verification/physical-20261105/reader-01/reader-resumed.checkpoint.json) retain the reader identity and anchor. The recorded pre-sleep and resumed PNG SHA-256 is `a393077b3cafe3914b7d1869c94fa3c2c4eaf9df6a3f575981fcf76264c6c9d1`; the recorded framebuffer-pixel SHA-256 is `6284b79ede16e8a65dfe357df773c0e90210234cc681f078bb505f02c8acc1b6` for both captures. Offline visual review also found the same page-19 image.

The [timeout assessment](../artifacts/verification/physical-20261105/reader-01/timeout-assessment.json) preserves the boundary of that first attempt. Exit `0` and the configured deadline are consistent with a normal timed exit, and the retained evidence contains no crash diagnostic. The retained run record does not precisely identify which exit trigger fired or the exact ordering of the tap and exit. It therefore does not accept that first post-wake tap as a successful interaction check.

In `reader2-01`, PID `31373`, start ticks `694165690`, and the pinned executable remained unchanged across [before sleep](../artifacts/verification/physical-20261105/reader2-01/retry-before-sleep.checkpoint.json), [suspend](../artifacts/verification/physical-20261105/reader2-01/retry-suspended.checkpoint.json), [resume](../artifacts/verification/physical-20261105/reader2-01/retry-resumed.checkpoint.json), and [post-wake finger input](../artifacts/verification/physical-20261105/reader2-01/retry-first-finger-after-wake.checkpoint.json). The first three checkpoints retain the full page-18 reader state: comic, episode, revision, reader generation, page ID/index, crop, source position, zoom, rotation, route, and saved settings. The finger checkpoint records page index `18` / displayed page `19`, after page index `17` / displayed page `18`, with the route still `reader`.

The user explicitly confirmed the physical power-button sleep/wake sequence, the retained page after wake, and the subsequent finger transition while the reader stayed open. The framebuffer images corroborate page 18 followed by page 19. They are framebuffer-memory captures, not photographs or an independent measurement of panel update behavior; the physical observations are recorded separately in `reader2-01/run.json`.

The reader runs exercised the saved `resume` launch preference. They do not physically establish sleep continuation under the ordinary-startup `shelf` preference variant. The smoke run's explicit initial shelf route does not fill that gap.

## Recovery and return of the shared device

All three run records report successful watchdog recovery, matching statusbar and awesome baselines, and an absent production-launcher lock. In the second reader run, `operator-stop` and launcher status `143` record the coordinator-requested termination. The user subsequently confirmed the native Kindle library and normal touch operation.

The [final restoration snapshot](../artifacts/verification/physical-20261105/final-device-restoration.json) and [native state text](../artifacts/verification/physical-20261105/final-device-restoration.txt) record zero owned device processes, an absent launcher lock, `com.lab126.KPPMainApp` in the foreground, running statusbar/powerd/lab126_gui services, and awesome PID `3495` in state `S`. The JSON snapshot's `exclusive_lease_still_held=true` describes the restoration step before release; it has no embedded timestamp and is not the later lease-return record.

The [lease-return record](../artifacts/verification/physical-20261105/lease-return.json) records release at `2026-11-04T17:37:16.539341+00:00`, equivalent to Beijing time `2026-11-05T01:37:16.539341+08:00`. It binds owner thread `01a1a209-30fb-7ef0-b27c-58d1b4b4d494`, lease run `20261105-1e4a0f27`, and lease-token SHA-256 `235b32be34e44beb1256e47e68872ff2c6fd4b03dbe585f52fe8a8b89fc5b841`. No raw lease token is reproduced here.

The return record confirms release, an absent lease directory after the owner/token-checked release, zero remaining owned device processes, an absent launcher lock, restored native statusbar/awesome, and closed coordinator-owned device SSH connections. The coordinator reported closing both of its device SSH connections and explicitly returning the device to WeRead thread `01a19d47-c440-7e92-a7d0-f8bb928f4dc3`; the peer's acceptance was confirmed to the report author by the coordinator. The JSON record identifies the recipient but does not itself contain the peer acknowledgement transcript.

The coordinator reported that the isolated test directories remained only as audit artifacts, with no running application or Kindle library entry. A new explicit exclusive handoff is required before any further device operation.

## Retained issues and limits

The second reader run retains a post-stop `last_failure`: a `confirm` annotation was invoked with `--panel` but without a framebuffer checkpoint. The later correct Home/touch confirmation succeeded. This annotation-parameter error remains in the original run record and does not change the device interaction or recovery evidence.

This report does not establish real-account login, service access, network downloads, purchases or other transactions, USB behavior, power consumption, long-duration sleep, performance, other reader configurations, other devices, or other firmware versions. The ready shelf capture and normal-display observation do not separately certify startup-animation behavior or startup performance. Fixture and firmware-emulator results remain separate evidence classes and are not promoted to physical results here.

The report was assembled through offline review of retained host files and existing images. No local verification program, new test, physical CLI call, SSH helper, or Kindle operation was executed while preparing it. The machine-readable [summary](../artifacts/verification/physical-20261105/summary.json) preserves the same candidate boundary and run-specific outcomes. Subsequent UI changes require their own verification and physical acceptance.

