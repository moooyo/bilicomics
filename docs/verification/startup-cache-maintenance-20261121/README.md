# What startup cache maintenance actually does

**Decision: keep production cleanup and durability unchanged; correct the steady-state benchmark input.** Most of the original fixture's roughly 7.5 ms was spent committing a one-row rewrite. The same program reopened the same twenty-comic library with product-written recency in a median **0.490 ms of maintenance**, rather than **7.481 ms**. All six pairs showed this distinction. It is not a shipped startup speedup: the existing product already writes the compact representation.

## Compared inputs

The [previous ordinary desktop trace](../startup-direct-png-20261121/phase-profile.md) measured the whole `state-cache-maintenance` call: 7.512 ms at the control median. The question here is whether that cost recurs when the product reopens an unchanged library. This is a comparison of **two inputs to one executable**, not two cleanup implementations. The [study](STUDY.md) and [fixture and sampling protocol](PROFILE.md) distinguish those claims up front.

F is the original synthetic Shelf fixture. It already has a recently used catalog record covering one scope, twenty comics and no details. R starts from an independent copy of the same original; only that record's text is replaced with what a normal product launch actually persisted. The preparation does not copy other side effects of the launch. The runner compares the parsed lists in order and checks every other business-table row in both prepared copies. No real sessions, comic data, credentials or acquired content are needed for replay of the timing report.

In [qualification](qualification.json), the original was 313 bytes and the product text was 289 bytes. Removing only JSON separator spacing from the former reproduced the latter exactly. All other rows across eleven application tables matched. Launches with tracing enabled and disabled wrote the same product text and reached the ten-cover Shelf. Both positive inputs found the existing key and twenty durable comics; all four deletes affected zero rows. The recency UPSERT affected **one row on F, zero on R**. A separate missing-key launch rebuilt and inserted the record. A genuine eviction removed the expendable membership and comic without rewriting R. A signed-out control with the same ten resident covers displayed no marker codes.

## Measured result

Two warmups were excluded. All twelve measured desktop launches completed in the fixed alternating FR/RF order, with ten visible marker codes in the final logical Shelf and no replacements. All fourteen attempts committed and restored the ordinary busy timeout. The [complete table](stages.md) contains elapsed and worker CPU times for every pair and all twelve stages. The following values are elapsed **milliseconds**:

| Work | Original text F, median | Product text R, median | Median paired R−F |
| --- | ---: | ---: | ---: |
| Whole maintenance call | 7.481 | 0.4895 | −6.784 |
| Transaction commit | 6.9775 | 0.0055 | −6.9705 |
| Conditional recency UPSERT itself | 0.028 | 0.0265 | −0.0025 |
| Read protected comic IDs | 0.2515 | 0.245 | +0.0095 |

R used less elapsed maintenance time in **6/6** pairs (5.591–7.423 ms less); the paired median difference was −6.741 ms in FR order and −7.189 ms in RF order. Time the entire commit, not just the statement that made the change: the UPSERT took tens of microseconds on **both** inputs. After subtracting each launch's own commit time, the paired median difference in the remaining maintenance was **+0.019 ms**. The longest R case took 1.143 ms, including only 0.005 ms to commit. This explains the recurring cost in fresh *copies of the fixture*; the old runs did not have these substage clocks. It does not show that user startup is 6.784 ms faster or that large libraries have the same residual cost.

## Reading the trace

The [optional startup diagnostic](../../PERFORMANCE_DIAGNOSTICS.md#startup-and-local-continuation) times the existing SQL and Rust work on the state worker. The parent span includes setup, set/JSON preparation between stages, restoration of the connection's ordinary busy timeout and instrumentation overhead. The extra aggregate is serialized and written **after** the parent end event, not while holding the SQLite transaction. Normal launches without tracing do not allocate a trace or sample these clocks. Later retry attempts are not included.

`durable` counts distinct IDs returned by the existing protection query, not rows deleted or identifiers logged. `reconcile` includes three existing-set queries and the in-memory list filtering. `recency_upsert` includes the usual product serialization and conditional SQL. A successful zero-row call is still measured work. `commit` times the transaction API call, not the number of I/O operations or an isolated `fsync`. Elapsed and worker CPU are reported separately. Neither a sum of marginal medians nor a subtraction from the historical 7.512 ms is a measured saving.

## Verification and next action

- The [Release suites](software.json) on `test-env` passed: 902 library tests (five existing ignores), 18 application tests. The ten passing targeted catalog tests are already included in the library count. They cover first normalization, stable no-op, missing or malformed recency, actual deletions, protected/cross-account data, immediate lock contention, timeout restoration and rollback after partial deletions. The normal UI-retry and tracing-disabled paths keep the pre-existing transaction, busy handling and durability policy.
- The [independent audit](audit.json) matched all original performance, lifecycle, per-case and final records to the [restricted projection](projection.json). Replaying the projection reproduced the exact pair table; negative controls for a missing case, exchanged update counts and a falsely uncommitted case were rejected. From this directory on `test-env`, run `python3 -B summarize_cache_profile.py projection.json` to reproduce the table without a session, database, executable or images.
- Use R for a steady-state ordinary startup fixture; keep F explicitly labeled when studying the first rewrite. Do not silently change historical seeds, compare new and old samples as a product A/B, defer cleanup, or weaken SQLite `synchronous=FULL`. The largest observed steady-state substage here was the protection query (0.245 ms), not a new bottleneck demonstrated on other accounts. If cache scale becomes the question, vary catalog and episode counts on product-written input before changing that query.
- The [subsequent offline launcher replay](../startup-decisions-20261121/launcher-replay.md) separated the three supervised reads on their own clock. The display capture was the longest; its internal costs remain unmeasured in that batch. This cache round did not access a Kindle, take a lease, time the display, or change the installed package.
