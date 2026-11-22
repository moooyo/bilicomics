# Three native startup display-capture observations

**Question:** What does the unchanged, approved Scribe display capture spend time doing inside its capture helper on ordinary startup? This is a three-visit, single-build engineering baseline. There is no candidate arm, duration target, warmup exclusion, retry or claim of savings. The experiment is not a mechanism to enable a product change. Do not combine the [historical 350–440 ms go-to-observed-ready intervals](DEVICE_REPLAY.md) (median 410 ms, 10 ms `/proc/uptime` resolution) or any host-fixture measurements with the new device clocks.

## Admission and safety

- Prepare exactly ordinals 1, 2 and 3, using the same separately qualified engineering ELF, worker, approved source-matched stock launcher/document, known offline seed, diagnostic flags and fresh run-specific stage for each. Before access, verify the prepared archives against the original inputs by bytes, exercise both the normal and rejected paths of the staged exact selector/cleanup, and test the pinned guard/argv, positive fixture, failure paths, and Scribe/PW6 QEMU admission on `test-env`. A Linux fixture is not a Scribe and a user-mode ARM refusal does not measure framebuffer or LIPC performance.
- Freeze the original installed launcher and Library document from the retained stock archive. Only the staged **startup call** to `capture_native_epdc_mode` receives `BILICOMICS_DISPLAY_COST_RUN=<this run>`. The actual helper argv remains `--capture-native-display --kindle --device scribe`; the original `ready:...` stdout parser is unchanged. The main UI retains `--kindle --device scribe --data-dir <stage>/data --demo --offline --shelf-render-trace <stage>/data/<run>.jsonl --exit-after-ms 2500`. Do not instrument wake, pass a handoff or lifecycle/performance trace, add shell clock helpers, or enable a production fast path. The staged selector and guard must verify these choices byte-for-byte before launch.
- Keep the original capture deadline, independent two-second launcher watchdog, permission/identity rechecks, command result checking, bounded pipe and parser, kill/reap behavior, cold/active gate and final native restoration. Do not turn a refusal, timeout, changed orientation/power/ownership, partial record or secondary measurement failure into a success. Do not force a new sample, direct-invoke the helper over SSH, change the shipping binary or alter the installed Library entry. Retain the exact original launcher for verified restoration.
- Trigger the existing native Library card once and let the diagnostic offline application reach the first populated `ready` Shelf, then exit naturally. This baseline retains the original software renderer endpoint (ordinary layout, entries and navigation; placeholder covers allowed). Accepted SEND and same-marker driver completion are optional and separate; no in-app tap, camera, optical or clickability inference. The Shelf is not a screenshot or a timing proxy for the display helper.
- Collect just the approved, run-specific `<run>.display-cost.json` after independent application retirement, not from the running app or its generic logs. A device-side allowlist matches the complete bounded writer format before emitting anything: extra fields or malformed records yield no stdout. The short-lived capture helper and the later UI process are **different processes**. The private helper record is self-reported by the qualified code after a successful, authorized startup capture. The staged route, fresh exclusive leaf, launcher admission, device boot, run and timing window bind it to the later UI; the UI PID is independently witnessed by the original guard. Do not claim independent observation of the departed helper PID. Never publish the boot/run IDs, raw framebuffer or property values, process identifiers, lock records, session/nonce, arguments, paths or raw logs.
- Hold a fresh exclusive lease only for each continuous device visit, not for preparation, analysis, waiting or code changes. A collision before admission means no device contact or reservation: report it and stop. Restore stock launcher and native Home/Library/pointer, remove only the allowlisted staged objects, close connections, and invoke exact owner/token release before analyzing any durations. Stop on failure; an unknown dispatch or return cannot be overridden by an idle device. Only necessary restoration may continue under the original lease. Zero, one or two qualified observations remain incomplete, not an invitation to achieve 3/3 by retrying.

## Interpretation and replay

For each slot show the primary card-release-to-ready-Shelf renderer interval, the two optional display milestones, and the separate counts, elapsed times, and declared residuals for its display capture. Use device `CLOCK_MONOTONIC`. Costs inside a parent interval are *parts of that interval*: do not add a child or guard to its enclosing call, add group medians to make a timeline, or call unexplained wall time CPU, firmware or removable delay. The native launcher before/after the helper (permission signaling and completion polling, pipe drain, reaping, parsing and scheduling) is not directly measured by this receipt. Probe overhead and per-visit scheduling remain in the sample. A separate, balanced, independently approved A/B would be needed to test whether any actual change shortens card-to-Shelf time without weakening safeguards.

- The automatic Scribe path retains **all five property queries** (power, mode, holding, holding, power) and **two hardware metadata observations**. The fixed path naturally omits both holding queries; its three-count result is equally eligible, including its ready Shelf primary. Report each visit and denominators for `automatic-five` and `fixed-three`. Report three-visit bucket medians only when **all three** completed the same branch; for mixed or incomplete visits report each observation, never a pooled phase median. Do not force orientation, make up a fourth visit, or trade either repeated safety observation away. `process_stat_reads` counts entries to the existing process-identity reader; it has **no measured duration** and is not an exact count of kernel syscalls.
- `helper_elapsed_ns` is the difference between the native helper's start and finish anchors. Its disjoint top-level buckets are launch approval, permit acquisition, initial recheck, query pre/post checkpoints, metadata pre/post checkpoints, metadata calls 1 and 2, and `commands_run`. The helper residual is its elapsed time minus those top-level totals. The setup before its start and sidecar write after its finish are outside it. The interval must lie after the confirmed card release and before the UI's Shelf trace header.
- `commands_run` contains inner pre/post/periodic permission checks plus `command_spawn`, `command_read`, `command_try_wait` and `command_poll`. The command residual subtracts only these nested buckets from `commands_run`. Polling uses the existing **10 ms maximum poll slice** and 100 ms periodic context-check schedule, not a fixed amount spent per poll. Spawn, I/O, waits and scheduler delays are not pure LIPC service time. The native watchdog remains authoritative; do not infer success or failure from a post-hoc two-second cutoff or compare this aggregate with optical panel response.
- `metadata_1 + metadata_2` includes the six original internal metadata checkpoints. Report their combined residual after subtracting `metadata_checkpoint` **once**, not once from each call; the remaining time is not a direct measurement of individual drivers. Any median is calculated from the three per-visit quantities, not by subtracting or adding independently calculated medians.

Replay on `test-env` only after the original runner has exited and any necessary recovery is closed, **before** discarding unattempted prepared batches:

For the private plan command, supply `BILICOMICS_STUDY_OWNER` from the original operator approval, not from the plan. No live `CODEX_THREAD_ID` is needed for this offline check.

```sh
python3 -IB test_collect_display.py -v
python3 -IB collect_display.py --plan "$private_plan"
```

The public result includes each ordinal, even unattempted or incomplete. Missing helper evidence is not zero; do not erase an independently qualified primary Shelf interval merely because its helper breakdown is absent. Unconfirmed release, mismatched provenance, malformed or impossible clocks, or a visit after a stopped slot are not eligible. No physical observation or product speedup follows from writing this plan or passing its synthetic tests.

To retain an archive, save only the collector's public JSON projection and pass it
through the independent arithmetic/schema replayer. Keep both files outside the
repository and the raw closed batches private; never archive the raw display
sidecar, private plan, or closure receipts.

```sh
python3 -IB collect_display.py --plan "$private_plan" > "$projection"
python3 -IB archive_display_projection.py "$projection" > "$sanitized_archive"
```

The archive replayer accepts no run, boot, process, path, or clock-anchor fields.
It recomputes per-visit residuals and three-visit medians using integer
arithmetic. It does not independently establish release, provenance, or
physical-device validity; those remain the collector's responsibility.
