# Ordinary Library preflight A/B: 2026-11-22

## Status and scope

This is a new, independent BiliComics series. The product source is unchanged:
do not modify or rebuild the application or launcher treatment for this study.
The assignment remains the final descriptor-corrected launcher experiment
frozen in [the 2026-11-21 physical protocol](../startup-library-admission-20261121/NATIVE.md).

- **A:** the ordinary `--library` parent validates the configured profile,
  firmware allowlist, framebuffer access and log/font directories; its
  detached worker repeats those checks before taking the display lock.
- **B:** the ordinary `--library` parent delegates those checks to the
  supervised worker. The worker performs the same validations before taking
  the display lock.

Use the final **80,710-byte B launcher** and the same retained
**74,392,336-byte application** in every visit. Keep the two arm assignments,
default render policy, ordinary lowercase Library card, isolated offline
Shelf seed, route and launch arguments unchanged. The final descriptor-corrected
two-block host mechanism result is documented in
[HOST_RESULTS.md](../startup-library-admission-20261121/HOST_RESULTS.md#final-candidate);
its pre-lock host timing is not a Kindle result.

This plan does not authorize or request a device lease. Prepare the complete
source, fixture, application, recovery and analysis package off-device first.
Acquire a fresh exclusive lease only when a visit can begin immediately, restore
native control and close connections before releasing the exact owner/token
lease, and stop without waiting or reserving if the device is unavailable. Any
blocker requiring code changes, rebuilding or investigation means release now
and re-prepare off-device before a newly authorized attempt. There is no
automatic deployment.

## Frozen schedule

Create a new `bili-library-admission-ab-v1` plan and new run/batch identities
for this date. Keep the original paths immutable. Run two excluded warmups
(A, B), then exactly six measured pairs in this order:

**AB, BA, AB, BA, AB, BA**

The 14 rows are the only visits in this series. Do not copy, reuse or pool the
November 21 warmups, its one incomplete measured pair, any yesterday result,
or samples from another run. If a visit is unavailable, fails, is unqualified,
or cannot verify release, stop at that prefix. Never replace, reorder, append
or silently fill a missing visit. An attempted visit without a verified exact
release is a hard stop, not an ordinary missing sample.

Each `launcher-arm.json` must bind the predeclared arm, run, preflight policy,
application byte size and default render policy. Preserve the final 80,710-byte
B source; reject the earlier 80,642-byte pilot. The host comparison remains
the only predeclared performance admission gate: both final launcher
mechanism blocks passed at least five of six pairs and a paired median
B−A benefit of at least 1 ms. The already-frozen desktop/X11 protocol retains
its four functional qualifications and one descriptive six-pair series; do
not add another X11 run or use it as an additional performance gate.

## Failure evidence

Keep the existing host failure field unchanged: `failure.json.phase` remains
`native-navigation` for a failure in that operation. The new recorder's fixed
probe substate is a separate, optional `probe-failure.json` receipt; it is
diagnostic evidence only, never proof of a successful probe, click or launch.
Its exact six fields are `schema`, `phase`, `action`, `step`, `tab_request`
and `card_request`. The schema is `bili-native-probe-failure-v1`; `phase` is
one of `native-navigation`, `identify-card` or `card-release`; `action` is one
of `read-navigation`, `request-target-tab`, `identify-card` or `open-card`;
`step` is one of `native-guard`, `operation-request`, `response-validation`
or `receipt-accepted`; and each request state is one of `not-requested`,
`dispatch-possible` or `receipt-confirmed`. Do not infer or invent a substate
from the generic `failure.json.reason`.

The auditor must report `tab_request` and `card_request` independently. An
intent marker alone is not a request receipt. `dispatch-possible` is
ambiguous: it must never be reported as either "not sent" or "confirmed sent".
`receipt-confirmed` records an input receipt, not a successful application
launch or a rendered Shelf. Only a fixed receipt whose `card_request` is
`not-requested`, paired with no card receipt, app retirement, card origin,
Shelf trace or timing projection, can establish that no card input was
requested and the candidate application was not launched. A `tab_request` of
`dispatch-possible` must still be reported as ambiguous native tab input; do
not claim that no native input at all was sent.

If the new receipt is absent, partial, malformed or unknown, keep the legacy
evidence readable and classify the case conservatively as unqualified; never
retrofit the new diagnosis onto the November 21 batch.

## Primary measurement

Use the existing same-device `CLOCK_MONOTONIC` metric: ordinary native
Library-card release to the first complete normal Shelf software renderer
return, including navigation and entries or the appropriate empty state.
The existing `log_bounds.audit` binds the release bracket, run, device boot,
application PID, render, accepted SEND and same-marker driver completion.
The retained application may omit `render_order`; its absence is not a reason
to reject a visit or infer render ordering.

Report the renderer return as primary. Accepted SEND and same-marker driver
completion remain separate, optional secondary endpoints and never substitute
for the primary interval. These are software submission and driver observations,
not optical visibility. Do not ask the user to click, record or run a script.
Do not capture screen images, test follow-up clickability, or relabel historical
clickability/cover-complete measurements.

## Decision threshold

The physical result qualifies only with all six measured primary pairs
complete, B strictly faster in at least five pairs by non-overlapping
release-bracket intervals, and the upper bound of the median paired B−A
renderer-return interval at or below **−100 ms**. Compute the median from the
six paired B−A intervals, never by subtracting marginal arm medians. Keep
accepted SEND and driver-completion summaries independent. A missing secondary
endpoint cannot invalidate a qualified renderer result or replace it.

If fewer than six pairs qualify, the six-pair threshold is unmet. Keep all
observed pairs and the stopped prefix; do not combine this date's incomplete
or unfavorable result with an earlier series. The host preflight gate and
desktop surrogate are mechanism/function evidence only; neither establishes
the 100 ms Kindle result.

## Analysis and retention

Prepare and run the auditor's synthetic tests in the new isolated
`test-env` workspace. Replay only this date's private plan and its allowlisted,
released evidence. Keep raw batches and the previous date's records unchanged.
For this series, require the new fixed failure sidecar explicitly; keep the
legacy-compatible default for the November 21 plan:

```sh
cd /var/tmp/bilicomics-library-20261122-s6FIBuJ1/docs/verification/startup-library-admission-20261121
python3 -B -m unittest -v test_analyze.py
python3 -IB analyze.py --require-probe-failure /var/tmp/bilicomics-library-20261122-s6FIBuJ1/native/plan.json
```

The strict option prevents a missing new sidecar from being interpreted using
the previous date's pre-click heuristic. A fixed sidecar only clarifies its
recorded request state; it does not change primary eligibility or create a
timing endpoint.

The date-specific report may contain run status, paired intervals and
anonymized endpoint summaries only; omit owner IDs, paths, PIDs, private tokens,
generic logs, account data, acquired comics and device images.
