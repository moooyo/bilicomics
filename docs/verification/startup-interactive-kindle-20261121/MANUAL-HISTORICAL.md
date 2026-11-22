# Installed Kindle: one interactive Shelf check

**Superseded procedure; no physical interactivity sample was taken.** The current [render-only study](README.md) uses neither an operator nor a camera. The following notes describe an earlier proposal only.

**Prepared, not measured.** The target is the [interactive Shelf](../../STARTUP_AND_SLEEP.md#primary-startup-metric-interactive-shelf) reached from the existing `BiliComics for Kindle` Library card, not cover completion. Device event logs are the preferred timing method; a camera is optional. The [desktop measurements](../startup-interactive-shelf-20261121/README.md) and any timing-tool fixtures are not device samples.

## What the logs can establish

The [ordinary launcher](../../../packaging/kindle/launch.sh) keeps `launcher.log` and `application.log`. Detailed [performance tracing](../../PERFORMANCE_DIAGNOSTICS.md) is opt-in. The [ordinary-install smoke test](../../physical/shelf-render-20261121/README.md#normal-entry) did not enable it. Even in a diagnostic run, `ui_ready` precedes the refresh request. Neither it nor a driver-completed frame alone proves a Shelf control responded. The installed logs do not establish this end-to-end time retrospectively.

A log-based sample needs the following events from **one launch on one device monotonic clock**. These are requirements for a diagnostic run, not events already collected:

1. Observe release on the identified native Library card outside the application. Label a device-generated test click as such; neither shell nor process entry is the card release.
2. Identify the first unobscured Shelf frame with non-cover content and a visible filter. Associate it with completion of that frame's display update, not merely a request to draw. Record card release to this frame as the presentation interval.
3. After that frame, observe a fresh normal touch on the visible filter, its closed-to-open action and the corresponding menu frame. A callback invoked by a script or an unrelated refresh does not qualify. Use only anonymous event labels, frame identities and times, not book titles or touch coordinates.

Report the successful tap as validation of the earlier presentation and the confirmed menu time as a conservative interactivity upper bound. Keep operator reaction separate. Missing events permit only the measured segments: for example, an absent card release means process-to-frame is not card-to-frame. Driver completion is a device event, not an optical measurement.

## Manual fallback without changing the installation

The operator can verify the same interaction without any recording. Use a phone camera only if an independent finger-to-screen time is wanted; capture the finger and screen together in one uninterrupted shot. No Kindle SSH password, account login, content download or package change is required for this fallback. Do not send private book titles or a full library video: the operator may review it privately and report only frame numbers or timestamps. Eyewitness evidence alone confirms functionality, not latency.

The host coordinator must acquire a **fresh** exact-owner lease immediately before device use ([host procedure](LEASE.md)); permission to test or an earlier free-lock observation is not a reservation. The operator waits for the coordinator's explicit start signal before touching the Kindle. If another project owns the lease, do not click or replace the lock. After admission, confirm the normal native UI and note the original navigation state. Do not change settings to force an expected route.

## One continuous observation

| Mark | What to identify in the recording |
| --- | --- |
| Card release | The finger lifts from the correct application card in the native Library. This is the optical launch origin, not a Rust or shell timestamp. |
| Shelf shown | The app's unobscured Shelf text, row or appropriate empty-state message and its filter control are visible. Artwork may be missing. |
| New filter press | Only after seeing that Shelf, the operator presses its **visible** filter control with a fresh touch. Do not tap remembered coordinates during startup. |
| Filter release | The finger lifts after the single press. Do not tap again or choose a filter option. |
| Menu shown | Content unique to the previously closed filter menu is visibly present, e.g. its All/current row. A highlight or a panel flash alone is not a response. |

Classify a populated, empty or signed-out Shelf separately. If the normal entry resumes Reader or arrives at Welcome, or the relevant control cannot be identified, record the actual route as not eligible. Do not log in, change the destination, or silently retry to obtain a sample. If a valid tap has no observed response, record that failure; do not report a latency for it.

Close the menu by tapping outside, exit normally, restore the original native view, and confirm native touch works. Tell the coordinator that restoration is complete **before** reviewing the video. The coordinator closes any owned connections and immediately releases the exact owned lease; never wait for the operator to transcribe timings while holding it. If restoration fails, report it and finish the necessary recovery rather than declaring success.

## Reporting

Review adjacent frames around each transition. Use the actual constant frame rate, or actual presentation timestamps for variable-frame-rate video. Record the Shelf presentation interval, the operator's reaction interval and the response to the fresh tap separately. The latest observed menu appearance relative to earliest possible card release gives a **conservative upper bound** on when the Shelf was interactive. Do not subtract reaction time to manufacture an exact app-ready time. One physical sample is not a device median. A purely verbal report may establish that the normal entry and control worked, but chat timestamps are never a stopwatch.

After returning the device, the operator can provide just the actual recording rate and the five frame numbers above. For variable-rate footage use the before/after frame presentation times for each mark instead of the nominal camera setting. The [annotation tool](frame_bounds.py) can process those numbers on `test-env`; it does not read the footage. For a known constant rate, a single frame number asserts that the preceding frame was checked and still showed the old state. If menu appearance and finger lift cannot be ordered at the camera's resolution, keep the card-to-confirmed-menu bound where supported but leave the release-to-menu duration unmeasured. Use `--source kindle-operator --report-only observed` for an eyewitness-only successful interaction; its times are deliberately null.
