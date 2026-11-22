# Three single-arm prepared-State observations

**Question:** On any of three predeclared ordinary native Library-card launches of the diagnostic offline Shelf, is the time from the producer's `State` timestamp to the first UI receive **at least 250,000,000 ns**? Count equality. This protocol is not a performance comparison or a change to the production application. It is independent of the earlier A/A positional study.

- Predeclare visits **1, 2, 3** in order. Use the same separately qualified diagnostic ELF, stock-equivalent launcher, public card, fresh demo seed, and arguments checked before admission. Each visit has its own run, natural-retirement witness, device boot, and exact returned lease. No warmup exclusions, replacements, extra trials or early stop based on latency or receive source. Stop on access, safety, recovery, or evidence failure. Never reserve a device for analysis.
- Preserve the primary context: bracket the one normal card release, then take the first qualifying whole-window `ready` Shelf software renderer return with layout, entries and navigation. Covers may be placeholders. A signed-out/empty destination, splash, initial fill or driver write alone is not a ready sample. Accepted SEND and completion of that same positive marker are independent optional endpoints; neither establishes clickability or optical settling.
- Use only the approved, separately bound Shelf trace plus the [exit-time Rust sidecar](README.md). Its producer clock was taken immediately before publishing the prepared result; the receiver clock is taken in the first successful `StartupEvent::State` receive arm. `runtime_wakeup` and `maintenance_timer` identify the **receiving UI dispatch**, not which task woke it or why. Both are legitimate; do not preselect one. Do not add lifecycle/performance logging, synthetic interactions, repeated reads while the app is running, or substitutes for either timestamp.
- After independent retirement the collector fetches only `<run>.prepared-handoff.json` from the private staged data directory (one root-owned, non-symlink, single-link regular file, maximum 1 KiB). It accepts only the documented schema, run, boot, PID, clock, source and two integer timestamps. No subtraction, browsing, general application-log copying or statistics on lease. Restore original native state, remove the stage and close the exact lease even if evidence is unavailable.
- Offline, verify the original click, full Shelf trace, independent application PID and exact release. Require `trace_started <= prepared <= received <= first ready renderer`. Report `received − prepared` and the source for **every observed slot**; report `null` for any unobserved endpoint. Show the card-to-render bracket and optional same-frame SEND/driver brackets alongside each. If one valid observation reaches the threshold, the answer is “observed,” with its denominator. If all three are valid and below, answer only “not observed in these three.” Otherwise answer “inconclusive.” Don’t erase a qualified renderer interval just because its separate handoff sidecar was missing.

The [historical four cases](../../physical/shelf-render-20261121/README.md) had **254.561–275.955 ms** between a producer's timestamp and a **later UI performance record**, not this exact first receive. Report that separately. A threshold hit measures late delivery/observation, not a lost wake, avoidable work, a saving of 250 ms, or a product change. Do not pool these launches with the historical data or rank sources from three visits.

## Verification and replay

For private batch replay, supply `BILICOMICS_STUDY_OWNER` from the original operator approval, independently of the plan; an active `CODEX_THREAD_ID` is not needed offline.

Run on `test-env` in an isolated workspace. `python3 -IB test_handoff_collect.py -v` uses synthetic evidence and a private fake stage; it does not run the product, access a Kindle or use a lease. The Rust/ARM/emulator, exact route/argv guard and native restoration qualifications are separate gates. A synthetic test or producing this plan is not a report of executing any of the three physical visits.

After the original runner has exited and any necessary recovery has finished, replay **before discarding unattempted prepared directories**:

```sh
python3 -IB handoff_collect.py --plan "$private_plan"
```

The output lists all three slots, labels unattempted and incomplete visits, and omits absolute clocks, PID, boot/run IDs, local paths and credentials. Unknown lease disposition, mismatched identities and gaps in the prescribed sequence fail closed, not as zero or “below threshold.” Retain the anonymous result and only the small private evidence needed to verify it.
