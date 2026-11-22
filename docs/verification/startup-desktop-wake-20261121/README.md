# First desktop wake after the startup screen

**Decision: keep the callback diagnostics, not the experimental scheduler.** Waiting for the first 80 ms maintenance tick explained the gap after visible loading feedback on the stock desktop. A wake following a successful full-window software submission made the complete Shelf measurably earlier in **6/6 pairs**, by **56.338 ms** at the paired median. The tested hook is private to the research build. Do not shorten the product timer, ship a renderer fork, or apply this result to Kindle.

## Stock attribution

On the ordinary offline ten-cover Shelf, **the 80 ms maintenance timer both entered the UI driver first and admitted the workers**. The production Winit software renderer did not register a Slint render notifier; there was no `first_scene_rendered` event. The explicit `driver_first_entry` and `ui_worker_gate` records identify the source directly, rather than guessing it from the elapsed time. The [stock qualification](stock-diagnostics.json) and [independent audit](stock-audit.json) are from an untimed control. A separate empty, signed-out launch also took the timer path; its screen is not the measured Shelf.

| Populated stock control (A0) | Milliseconds from application entry |
| --- | ---: |
| Window shown | 15.961 |
| Actual X11 loading-title read returned | 34.933 |
| Actual X11 initial activity-region read returned | 35.309 |
| First UI driver entry | 95.384 |
| Worker admission | 95.441 |

The later of the two positive pixel reads preceded admission by **60.132 ms**. An [earlier untimed setup control](stock-initial-setup.json) also established the timer as the source, with a 62.028 ms gap, but predated the nonempty process-environment check. These gaps are observations, not estimates of savings. Neither local software submission nor two matching crops is an optical measurement. The completed Shelf is a separate endpoint, requiring an exact match of the entire 1860 × 2480 window and an independent logical image.

## Scope of the comparison

The [study design](STUDY.md) and [frozen protocol](PROTOCOL.md) keep A0 outside the comparison. A and B use one byte-identical research executable, one early software-submission sensor and the same deferred 16 ms callback. A records the callback and remains on the normal startup path. B lets the callback enter the normal driver only if work has not started and the UI is still live, booting and unsuspended. The original 80 ms timer and the first UI yield are present in both. The callback is local to the application window; the external pixel observer never starts the workers.

The [private Winit patch](WINIT_SUBMIT.md) consumes a one-shot only after a nonempty full-drawable `present_with_damage` has succeeded and the surface borrow is released. The [research application patch](app-submit-probe.md) schedules the guarded turn. The [locked Winit software backend](https://docs.rs/crate/i-slint-backend-winit/1.17.1/source/renderer/sw.rs) exposes no equivalent public per-window signal. `RedrawRequested` precedes drawing, and Slint's [`AfterRendering`](https://docs.rs/slint/1.17.1/slint/enum.RenderingState.html) precedes presentation even on a renderer that supports it. Neither is a substitute. The [common opt-in diagnostics](../../PERFORMANCE_DIAGNOSTICS.md) work without either research patch.

## Paired results

On `test-env`, each launch used a fresh Xvfb and a separate copy of the same offline, synthetic, **non-demo** profile. The [stock reference](shelf-qualification.json) showed all ten distinct page-one covers; the signed-out control with the same cache and resident bytes showed none. The [two research configurations](wake-qualification.json) also matched the reference before timing. The formal sequence kept both warmups out of the medians and retained all six alternating AB/BA pairs, without replacement. All 14 windows matched the complete Shelf twice and their independent final logical image; all had ten resident covers (5,324,800 bytes) in two attachment batches.

| Time from process entry (ms) | A: observe, median | B: wake, median | Median paired B−A |
| --- | ---: | ---: | ---: |
| First exact loading-title observation (upper bound) | 29.239 | 30.125 | +0.731 |
| First UI driver entry | 92.467 | 36.210 | −56.555 |
| Actual worker admission (`workers_started` producer stamp) | 92.510 | 36.225 | −56.578 |
| Software `ui_ready` | 111.353 | 52.724 | −59.835 |
| First exact complete Shelf observation (upper bound) | **132.184** | **75.981** | **−56.338** |

Using each process's own entry as zero, the timing gate required B's first full-window match to **finish before A's last nonmatching read even started** in at least five pairs. All six passed despite observation brackets 7.614–11.551 ms wide. The smallest separation between B's upper and A's lower bound was 42.905 ms. The median change in first-match upper bounds was −54.576 ms in AB order and −60.104 ms in BA order. Each difference is calculated within a pair; subtracting the arm medians does not reproduce the paired statistic. See the [complete brackets](shelf-pairs.md) and [endpoint summary](shelf-summary.json).

The [callback audit](wake-audit.json) identified the same source for first driver entry and actual worker admission in every launch: `maintenance_timer` in A, `desktop_startup_fallback` in B. The median observed submit-to-turn interval was 16.208 ms in A and 16.094 ms in B (warmups excluded). These intervals are neither the time the loading screen was visible nor the improvement. In **14/14** starts, both the title and initial activity reads from the application's own X11 window returned *before* workers started. In B the later read still preceded admission by 5.269–6.833 ms. The benefit was less waiting **after feedback**, not earlier feedback or a faster cover decoder.

## Regressions and replay

- The [Release build](build.json) and [test suites](suites.json) ran in isolation on `test-env`: stock application 18/18; research library 901 passed, five ignored by default; research application 19/19. The three targeted application passes are included in those 19. The [exact renderer module](fallback-qualification.md) passed two additional tests for unsuccessful, partial and duplicate submissions and per-window ownership.
- In a separate [real suspended desktop process](suspend-turn-receipt.json), the full submit succeeded but the deferred turn observed suspension and did not start work. After resume the existing runtime wake admitted each worker exactly once; the process reached Shelf and exited normally. This was not a timed pair or a device sleep test.
- All [four PW6/Scribe desktop visual cases](visual.json) matched the frozen initial and final logical images. During deliberately slow startup each window showed **0 → 1 → 2 → 3 → 0** on the activity bar. Both normal Shelves matched on four post-ready X11 samples; both controlled failures showed the error view, returned status 1 and never published ready.
- An [untimed `check-a`](setup-attempts.json) and a [visual preflight](visual.json) stopped on an empty immediate `/proc/PID/environ` read; neither produced a usable sample. Qualification was repeated with a nonempty live environment check on the same process, executable identity and matching application events. No warmup or measured pair was retried.
- The [per-case observations](desktop-observations.json) and [replay receipt](replay.json) are in this directory. The optional ignored `artifacts/verification/startup-desktop-wake-20261121/desktop-wake-replay.tar.gz` retains 57 allowlisted files for the 14 launches (17,028 bytes), not images, application databases, session material or executables. The [replay procedure](PROTOCOL.md#restricted-replay) compared the original, reduced and independently unpacked records. A final `test-env` replay from the returned archive reproduced the callback audit, the endpoint summary and the exact pair table. The delivered manifests, main program and retained catalog/cover inputs were also compared byte-for-byte to the stock build inputs.

To inspect the restricted archive again, unpack it into an empty private directory on `test-env`. From the repository root, set `TRIAL` to the unpacked `trial` directory and run:

```sh
python3 -B docs/verification/startup-desktop-wake-20261121/audit_wake_sources.py "$TRIAL"
python3 -B docs/verification/startup-cover-worker-20261121/summarize_pairs.py "$TRIAL"
python3 -B docs/verification/startup-direct-png-20261121/stage_costs.py "$TRIAL" --route shelf --ready-covers 10
```

## Next decision

Revisit desktop scheduling only when the application can subscribe to a supported, per-window successful submission. Retain the original UI yield, fallback and visible-feedback tests for that comparison. For Kindle, the native path already admits font/metadata work after the first accepted full SEND: in a [separate November 17 device batch](../../physical/startup-sequence-20261117/item1/analysis/critical-path.md), workers began 1.090–2.879 ms after that SEND in the six measured candidate launches. This study used no Kindle, device lease or real account, and did not change the installed application.

The next cross-platform target is the ordinary state worker's `state-cache-maintenance`, **not** another cover-copy or timer change. In the [previous ordinary control](../startup-direct-png-20261121/phase-profile.md) that span took 7.512 ms at the median. First separate `Store::try_prune_catalog_cache` into lock acquisition, recency/protected-set reads, candidate selection and deletion, remaining-set reconciliation and recency save, and commit. Record durations and existing affected-row counts without new queries or identifiers; do not interpret a zero-row statement as a disk write. Keep the nonblocking lock attempt, rollback, protection of active/durable items and deferred retry unchanged until there is evidence for a specific change.

Follow-up: the [ordinary cache-maintenance study](../startup-cache-maintenance-20261121/README.md) distinguishes the original fixture's first rewrite from reopening the same library with product-written recency. Those samples are independent of this desktop wake comparison.
