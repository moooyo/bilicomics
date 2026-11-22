# First Shelf render-order A/B

## Question and scope

Compare the first ordinary, non-cover Shelf render after opening the existing
native Library card on a Kindle Scribe running firmware `5.19.5`.

- **A:** the same ARM application binary with `--serial-shelf-render`, which
  requires the preceding panel update to finish before the first Shelf render.
- **B:** that binary with the default first-Shelf private-render behavior.
  `render_then_wait` is the intended order. A recorded `wait_then_render`
  fallback is valid assigned-arm data, but it does not qualify as an observed
  private-render-first visit.

Both arms use the ordinary native card, the same offline `design-demo` seed
recipe and asset source, and a fresh isolated data copy per visit. Keep the
product binary and seed source identities common to the entire series. Do not
change the launcher path, Shelf destination, account state, or cover policy
between arms. Each visit gets a fresh run identity and its own real-device
lease. No visit opens a real account or touches a comic.

The primary endpoint is the device-monotonic interval from the card button
release bracket to the first software renderer return for a normal Shelf with
its navigation and entries or appropriate empty state. A cover may remain a
placeholder. Use the existing `log_bounds.audit` rules for the click origin,
run, boot, process, render, accepted `SEND`, and same-marker driver receipt.
`initial_fill` is not a measured render-order visit. A must record
`wait_then_render`; B may record `render_then_wait` or its ordinary
`wait_then_render` fallback. Any other arm/order combination is invalid.

Report the accepted `SEND` and same-marker driver completion as secondary
endpoints on their own. Software render completion, driver receipt, and
physical/optical display are distinct observations. Do not click the Shelf
again to test input, capture a screen, or claim optical visibility.
Historical interactive and cover-complete results retain their original
names, endpoints, and sample sets.

## Frozen schedule

Warm up once in order **A, B**, then run six adjacent measured pairs in this
order:

| Visit | Pair | Arm | Role |
| ---: | ---: | :---: | :--- |
| 1 | 0 | A | Warmup |
| 2 | 0 | B | Warmup |
| 3–4 | 1 | AB | Measured |
| 5–6 | 2 | BA | Measured |
| 7–8 | 3 | AB | Measured |
| 9–10 | 4 | BA | Measured |
| 11–12 | 5 | AB | Measured |
| 13–14 | 6 | BA | Measured |

`series.py` writes the exact `bili-shelf-ab-v1` `plan.json` before any visit.
It contains `schema`, the owner and host boot IDs, and the 14 ordered
`{pair, arm, run, batch}` rows. Keep that plan unchanged. A batch can qualify
only once. Warmups never enter measured statistics.

Complete and release each visit before beginning the next. If a visit is
unavailable, fails to qualify, or cannot verify release, stop that series at
that point. Do not queue for a busy device, substitute a run, reorder arms, or
fill a missing pair later. The analyzer accepts only a consecutive prefix;
later attempted visits after a missing or invalid visit make the series
invalid. A B wait-first fallback is recorded and may be followed by the next
scheduled visit because it remains the assigned default arm.

## Paired analysis

For each completed pair and endpoint, report the measured A and B intervals
and calculate:

```text
B−A = [B.lo − A.hi, B.hi − A.lo]
```

Negative values favor B. Report a direction only if the entire interval lies
on one side of zero. Calculate the median from the paired B−A intervals, not
by subtracting the marginal A and B medians. The median interval is the
outward-rounded median of the lower bounds and the outward-rounded median of
the upper bounds. Keep `SEND` and driver summaries separate from the primary
renderer-return summary.

The predeclared primary timing gate requires all six measured pairs, B faster
in at least five pairs by non-overlapping interval direction, and a median
paired renderer-return interval whose upper bound is at most `-100 ms`.
Also report whether every pair observed the intended A/B render order. A
positive mechanism-level conclusion requires all six pairs to have
`wait_then_render` in A and `render_then_wait` in B. If B falls back, retain it
in the nominal assigned-arm summary, count it separately, and do not silently
drop, reclassify, replace, or relabel it. The actual-order-qualified subset is
reported separately as descriptive data and is not substituted for the
nominal result.

An intentionally shortened series may stop after four complete measured pairs
or after all six. If a failure stops it earlier, report that incomplete
consecutive prefix as-is. The six-pair timing gate remains unmet unless all six
pairs qualify. Missing renderer endpoints do not become `SEND` or driver
substitutes. Report whichever secondary endpoints exist, with their own pair
counts.

## Independent replay

After all attempted visits are closed, replay the plan and the small anonymous
records without a device connection:

```bash
python3 -B docs/verification/startup-shelf-ab-20261121/analyze_shelf_ab.py /var/tmp/bilicomics-shelf-ab-1fxb3YEY/ab/plan.json
```

The plan must retain its original absolute batch paths. Each analyzed batch
needs only `admission.json`, `origin.json`, `retired.json`, `released.json`,
`card.json`, `intervals.json`, its run-named Shelf trace JSONL, and the
`attempted` marker. The analyzer verifies the existing release receipts,
replays `log_bounds.audit`, and checks the recorded `render_order`. It emits
timing-only JSON and no raw trace contents, run paths, application identity
secrets, hashes, screenshots, account data, or acquired content.
