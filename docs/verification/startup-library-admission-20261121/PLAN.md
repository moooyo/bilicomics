# Ordinary Library preflight A/B

## Question and arms

Compare the existing ordinary Kindle Library launch with one shell-only
admission change:

- **A:** the Library parent validates the configured profile, firmware
  allowlist, framebuffer access and log/font directories; its detached worker
  repeats the same checks before taking the display lock.
- **B:** the Library parent starts its supervised worker without repeating
  those checks. The worker performs the same validations before taking the
  display lock.

The application, Shelf destination, retained ordinary Library entry, saved
offline demo state and all Rust code remain unchanged. The exact launcher arm
is fixed in each run's `launcher-arm.json`; both arms bind the retained
74,392,336-byte application and the default Shelf render policy. This is not a
test of skipping device admission, changing the display lock, or changing the
Shelf renderer's order.

## Off-device gates

The pre-lock Linux fixture in [HOST_PROTOCOL.md](HOST_PROTOCOL.md) measures
only shell launch to a synthetic worker receipt. Its two independent blocks
must each retain all six adjacent pairs, have B strictly faster in at least
five pairs, and have a median paired B−A of at most −1 ms. The refused-profile,
firmware, allowlist and missing-executable controls must keep the original
failure behavior and must not reach the application or display lock.

The final descriptor-corrected launcher is 80,710 bytes; its two host blocks
passed 6/6, with paired median B−A values of −3.186 ms and −2.838 ms. Only
the new [final host result](HOST_RESULTS.md#final-candidate) qualifies this
revision. The earlier 80,642-byte pilot is explicitly excluded. These values
measure pre-lock host admission only.

Run the already-frozen [desktop/X11 protocol](emulator/PROTOCOL.md) as written:
four functional qualifications (Scribe A/B and PW6 B/A) followed by its one
predeclared six-pair Scribe comparison. Preserve that comparison as descriptive
host-surrogate data only. It has no additional pair-win or millisecond
performance gate and cannot establish Kindle timing or qualify the physical
primary endpoint. Keep its process identity, normal Shelf route, and
worker/entry retirement and restoration checks intact; do not turn it into a
second series or derive a new go/no-go threshold from its output.

**Correction retained:** an earlier draft of this plan incorrectly required
two independent X11 series and imposed a five-of-six/1 ms performance gate.
That requirement was not in the frozen protocol and is withdrawn. No X11
result was read to establish or justify that threshold. The only performance
admission gate for considering the physical experiment is the two-block
pre-lock launcher mechanism benchmark above, plus the existing launcher
regressions and functional run-readiness checks.

Passing those checks permits considering a physical measurement; it does not
automatically authorize, start, or reserve a device slot. No automatic
deployment follows from any result.

## Physical series

The physical schedule is frozen before any visit: two excluded warmups A, B,
then six measured pairs ordered **AB, BA, AB, BA, AB, BA**. The unchanged
`bili-library-admission-ab-v1` plan has 14 `{pair, arm, run, batch}` rows.
`native_series.py` prepares all launcher and fixture inputs and binds each
batch to its exact arm before `start` is considered. Run identity and the
original batch path are immutable; do not rebuild, replace an arm, append
visits, or combine a failed series with a later one.

Acquire a fresh shared exclusive Kindle lease only when all inputs are ready,
the device is immediately available, and a visit can begin. Each visit uses
its own batch lease. Restore native control and close connections before
releasing the exact owner/token lease. If the device is busy, stop instead of
waiting or reserving a turn. If any visit needs code changes, rebuilding,
investigation, or other off-device work, restore and release immediately;
prepare again and use a new series if another physical attempt is authorized.
Do not ask the user to click, record, or run a script.

The primary endpoint is the existing same-device `CLOCK_MONOTONIC` interval
from ordinary Library-card release to the first complete normal Shelf
renderer return, with navigation and entries or the appropriate empty state.
The existing `log_bounds.audit` binds the click bracket, run, device boot,
application PID, render, accepted SEND and same-marker driver completion.
Shelf `render_order` is optional in the retained trace and is not an
admission criterion. Report accepted SEND and driver completion separately;
neither substitutes for renderer return. Do not infer optical visibility or
collect screenshots or follow-up input. Historical clickability and
cover-complete samples keep their existing names and endpoints.

Stop after an unavailable or unqualified visit and retain that prefix. An
attempted visit without exact verified release is a hard stop, not a missing
sample. The analyzer rejects attempted visits after the first missing or
invalid case, changed device boot, overlapping visit chronology, mismatched
owner/run/process identity, unexpected arm marker, partial trace, or an
unrestored device. If a released batch records failure in native navigation
and has no activation, card-click, retirement or trace evidence, report a
pre-click navigation failure with no launch and no elapsed time; do not invent
a Shelf interval. If any evidence suggests activation or a click, do not infer
that the application was never launched.

## Decision threshold

The physical A/B is worth retaining only if all six measured pairs have a
qualified primary renderer endpoint, B is faster in at least five pairs by
non-overlapping release-bracket intervals, and the upper bound of the median
paired B−A renderer-return interval is at most **−100 ms**. Compute the median
from paired B−A intervals, not arm medians. Keep SEND and driver completion
summaries independent. A missing secondary endpoint cannot invalidate a
qualified renderer result or become its substitute.

The physical and host thresholds answer different questions: the off-device
−1 ms gate is a mechanism signal; it is not evidence of a 100 ms Kindle
benefit. The frozen X11 series is descriptive and is not a gate. A failed
host gate means keep the launcher unchanged.

## Commands

Run the synthetic analyzer tests and any offline fixtures only in the
designated `test-env` workspace, never on a Kindle:

```sh
cd /var/tmp/bilicomics-startup-next-oGmoCt3G/docs/verification/startup-library-admission-20261121
python3 -B -m unittest -v test_analyze.py
```

After a fully closed physical series, replay only its small run-bound receipts
and Shelf traces:

```sh
python3 -IB analyze.py /var/tmp/your-series/plan.json
```

The JSON report omits owner, boot, PID, path and raw trace data. Do not archive
credentials, private tokens, account content, acquired comics, or device
images.
