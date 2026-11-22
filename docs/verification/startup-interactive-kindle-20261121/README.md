# Native Library card to first rendered Shelf

**Physical result: two qualifying Scribe launches.** The card-release-to-renderer-return intervals were **6.450490–6.451615 s** and **6.726484–6.728292 s** (bounds rounded outwards). Both used the same diagnostic ARM binary, a fresh isolated offline demo Shelf, the installed lowercase Library card and automatic input on that card only. They are two baseline observations, not an A/B comparison, stock-installation timing, or optical measurements. [Machine-readable results](physical-result.json).

The [primary endpoint](../../STARTUP_AND_SLEEP.md#primary-startup-metric-first-shelf-render) is the first unobscured Shelf with its ordinary layout, navigation and book entries or a meaningful empty state. A cover may still be a placeholder. No input is sent to the Shelf; opening a menu, loading all artwork, or recording the screen is not part of this test.

Use the existing native Library card. The automated device-side observer must identify its complete public title and bracket button release with the device's monotonic clock. For the same launch, distinguish these endpoints:

| Endpoint | What it establishes |
| --- | --- |
| Shelf render finished | The software renderer returned after drawing a qualifying Shelf frame. This is the primary interval. |
| Shelf update accepted | The display driver accepted the submission of that frame. This is not the time at which drawing began or the panel finished. |
| Same update completed | The driver acknowledged that update's marker, if observed. This is not a measurement of optical settling. |

The start is bracketed, not a physical finger timestamp. Report a range when the release interval is nonzero. A splash, software Ready, an unrelated refresh, an interrupted sample, or an app landing on Reader or Welcome cannot supply the Shelf endpoint. A measurement of an isolated diagnostic build is identified as such; it is not timing evidence for the uninstrumented installation or a real account.

Run the host checks before applying for the shared device. Prepare the payload and qualify automatic recovery first. Only the exact owner may acquire the real host lease, take the measurements, restore the original Library entry and native navigation, close its connections and release the lease. Summarize the event logs after release. No operator input is required.

The [historical desktop click study](../startup-interactive-shelf-20261121/README.md), [complete-frame driver comparison](../../physical/shelf-render-20261121/README.md), and [superseded manual protocol](MANUAL-HISTORICAL.md) have different endpoints. Their results are not renamed or used as samples for this measurement.

## Scribe results

| Observed endpoint from card release (seconds, rounded outwards) | Launch 1 | Launch 2 |
| --- | ---: | ---: |
| First qualifying software render completed | 6.450490–6.451615 | 6.726484–6.728292 |
| Submission of that frame accepted | 6.486666–6.487791 | 6.758427–6.760235 |
| Driver confirmed the same marker | 6.852572–6.853697 | 7.124547–7.126355 |

The first application trace header arrived **3.684–3.685 s** and **3.768–3.770 s** after release (approximate). From that header to render completion took **2.766 s** and **2.959 s**. The header is emitted during trace configuration after argument parsing, not at process entry. The first span includes native dispatch, launcher work, process loading and early application setup; it is not time attributable to the launcher alone. From render return to SEND took 36.176 and 31.943 ms; the driver then took 365.906 and 366.120 ms. The latter is outside the primary endpoint. The next comparison should separate native dispatch, launcher and early application work before changing refresh or cover policy. No production speedup is inferred.

Both launches naturally exited. During the first return, opening the native card had changed only the document's modification time. The ordinary restore check refused it. Still under the original lease, the coordinator verified the same document bytes, inode, mode, size, link count, catalog identity and all other protected journal fields, then completed restoration. No second launch was made in that lease. The permanent fix ignores **only** the native card document's mtime in its identity comparison; it does not relax the launcher or binary checks. A fresh lease and the same ARM executable were used for the second launch. The fixed controller restored the shipping launcher and original native navigation and pointer, removed staged payloads, closed all device connections and verified its own release automatically at **12:11:52 Asia/Shanghai**. There is no instrumentation left installed.

For offline replay, `artifacts/verification/shelf-render-primary-20261121/<run>/` retains the allowlisted card origin, anonymous trace, retirement, release and intervals. These local artifacts are not in Git. For each run, `python3 -IB log_bounds.py <run-directory>/card.json <run-directory>/<run>.jsonl` recomputes the three intervals without a device connection. The exact diagnostic ARM payload is retained locally for a future paired comparison; temporary build trees and upload archives were removed from `test-env`.

## Off-device qualification

On `test-env` the reviewed sources built as a static ARMv7 Kindle release and a Linux desktop executable. The application library (904 passed, five ignored), application binary (19 passed), and Kindle backend (252 passed) passed. Two reader tests initially lacked their frozen PNG inputs in the isolated checkout; after staging those original fixtures the entire application suite passed unchanged. The final log auditor (12 cases), native return guard (six), reversible route (18), and host coordinator (20) also passed. All fixture runs were independent of the shared device lease. See the [operator instructions](OPERATOR.md) for the automated route and post-release audit.

The [emulator and surface receipt](emulator/results.json) separates ARM user-mode QEMU startup checks from actual X11 presentation at Scribe and PW6 dimensions. Synthetic non-cover title, filter and navigation regions appeared on both displayed desktop Shelves. A Scribe control using the same staged demo preferences reached Shelf without a forced route. These are not measurements of Kindle card launch or physical ink. No full firmware boot or PW6 physical run is claimed.
