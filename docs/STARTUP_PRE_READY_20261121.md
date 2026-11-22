# Before Shelf readiness: result delivery and cached catalog reads

**Decision:** retain the working-tree catalog change on top of `9175002` as a populated-cache optimization. Remote Release tests and desktop pixel checks passed. There is a small empty-Shelf cost, and no new Kindle launch timing. The installed Scribe application and its [six-pair result](physical/shelf-render-20261121/README.md) remain unchanged.

The [subsequent cover-worker trial](verification/startup-cover-worker-20261121/README.md) is closed. **Do not move the startup RGBA copy to the worker:** both UI batches became cheaper, but only one of six desktop pairs demonstrated an unambiguously earlier complete Shelf. The product uses the measured control.

The separate [ordinary-cache direct PNG trial](verification/startup-direct-png-20261121/README.md) also kept the control: it eliminated the copy for all ten visible covers but produced **0/6** clearly earlier complete Shelves. This second trial did not reuse the previous demo's timing samples.

The [desktop wake trial](verification/startup-desktop-wake-20261121/README.md) identified the next delay: loading pixels were already visible while stock waited for the 80 ms maintenance tick to start the workers. A guarded wake after a successful software submission made the complete desktop Shelf clearly earlier in **6/6** pairs (median paired improvement **56.338 ms**). Keep the new diagnostic markers, not the private renderer patch. This is not a Kindle result.

## What the stored cases can tell us

The [November 20](physical/library-pairs-20261120/README.md) and [November 21](physical/shelf-render-20261121/README.md) studies timed native Library card releases on one Scribe 5.19.5 with isolated offline data. The numbers below are **within-batch medians of the six candidate launches**; the two warmups are excluded. The candidate changed between batches. Do not subtract these rows to estimate an optimization or add the segment medians. SEND is driver acceptance, not optical completion.

| Batch | Card to complete Shelf SEND / same-marker completion (s) | First loading SEND to ready (ms) | Loading SEND to worker observation (ms) | Worker observation to UI record (ms) | UI record to ready (ms) |
| --- | ---: | ---: | ---: | ---: | ---: |
| November 20, `57265de` | 5.935 / 6.301 | 818.194 | 487.504 | 14.693 | 317.866 |
| November 21, `shelf-private-rgb-v1` | 6.436 / 6.802 | 1,398.287 | 749.438 | 68.586 | 477.977 |

The original [frame](physical/shelf-render-20261121/frames.json) and [card](physical/shelf-render-20261121/paired-result.json) projections did not retain the absolute click origin. The replay matched each original `CLOCK_MONOTONIC` button-release timestamp to its own process. This identifies both sides of the handoff without treating the unexplained remainder as worker time. The UI record includes dispatch and recording; it is not pure scheduler time.

On November 21, `b03` took **906.490 + 13.825 + 510.063 = 1,430.378 ms** from first loading SEND to ready. The slowest case, `b04`, took **1,013.428 + 285.749 + 486.492 = 1,785.669 ms**. Prioritizing delivery cannot explain the long wait in `b03`. Conversely, four November 20 results arrived 1.710–11.574 ms **after** the third loading frame began waiting; the UI recorded them another 254.561–275.955 ms later. Draining the queue again just **before** the wait would have missed those arrivals.

**What happened after delivery?** All six November 21 candidate launches attached covers in two UI chunks. Measured from the first chunk's end record to the second chunk's begin record, the gaps were 0.293 and 0.309 ms for `b01` and `b02`, but **427.053, 409.603, 414.301 and 508.739 ms** for `b03` through `b06`. Each of the latter gaps contained one accepted, nonflashing 880 × 96 update. The recorded logical RGB differed from the preceding accepted frame; these were not duplicate loading images. On November 20, the same four late-delivery cases had gaps of 415.895–421.674 ms with a changed frame; the other eight measured cases had gaps below 0.33 ms. The gap includes useful feedback and display work. It is not time proven recoverable by deleting a frame or changing the chunk budget.

**What occupied the worker?** `state-catalog` in the old candidate measured just 7.586 ms and 11.162 ms wall time at the median on November 20 and 21 respectively. That span includes the entire catalog preparation, not just Shelf membership. Cover preparation took 109.906 and 96.378 ms. The **demo-only** fixture load took 93.834 and 218.632 ms; fixture seeding took 219.925 and 251.123 ms. A database optimization for a populated account needs a separate synthetic account comparison; the demo costs do not predict production gains.

### Reproduce the replay

On `test-env`, the checked-in reports suffice for the handoff durations, but not for the two intervals that need the absolute click origin. Without it, the analyzer reports those fields as `null`:

```sh
cd docs/physical/shelf-render-20261121
python3 -B test_analyze_pre_ready.py
python3 -B analyze_pre_ready.py
```

For the full replay, set `BATCH` to the unpacked allowlisted batch directory and `PAIRED` to its matching `paired-result.json` from one of the source reports linked above. Use a fresh private output path, and repeat for the other batch. No Kindle connection or device lease is needed.

```sh
python3 -B test_project_frames.py "$BATCH" "$PAIRED"
python3 -B project_frames.py --include-release-clock "$BATCH" "$PAIRED" > "$RESULTS/anchored.json"
python3 -B analyze_pre_ready.py --batch "$RESULTS/anchored.json" "$PAIRED"
python3 -B project_cover_gaps.py "$BATCH" "$PAIRED"
```

Both default frame projections reproduced the frozen JSON byte-for-byte; the enriched versions only added each case's verified release origin. The five analyzer tests and 18 projection tests per batch passed on `test-env`. The [cover-gap reader](physical/shelf-render-20261121/project_cover_gaps.py) also replayed both original archives, retaining the per-case gaps, accepted-frame comparisons, and worker spans. None of these are new device runs.

## One product candidate

Catalog preparation previously loaded and decorated every cached comic, then loaded and decorated Shelf members again just to collect their IDs. Decoration reads the account's anchor and, when available, the corresponding image index to derive progress. The candidate reads a Shelf membership flag alongside each comic using the existing account/scope/comic key. It collects the **payload** ID for members and computes each comic's progress only once in these two reads. The ordinary comic-list query still has no membership join; ordered History and Store queries are unchanged. An uninitialized empty Shelf still falls back to favorites, whereas an explicitly initialized empty Shelf stays empty.

This changes worker work, not the scheduling of UI notifications. There is no evidence that catalog reads caused the delays in `b03` or `b04`. A populated account may benefit; when there are few or no Shelf members, the join may cost more than the work it replaces. The demo can override membership from its fixture, so a demo-only launch is not a sufficient test of the database hypothesis.

## Worker comparison

Two invocations of the same optimized x86-64 test executable compared the two-call strategy (A) with the combined read (B). Each invocation created its own synthetic account with 256 cached comics and 64 anchors pointing to single-page indices. It warmed both paths on the same connection at each density, alternated six AB/BA pairs, and checked complete results outside the timed interval. An empty Shelf here has 256 cached comics but **zero members**. No real account or Kindle was accessed.

| In Shelf | Run 1: median paired B−A (range), µs | B faster | Run 2: median paired B−A (range), µs | B faster |
| ---: | ---: | ---: | ---: | ---: |
| 0 | +35.5 (−177 to +92) | 2/6 | +15 (−42 to +41) | 2/6 |
| 8 | −72 (−329 to +96) | 5/6 | −22.5 (−43 to +7) | 5/6 |
| 256 | −1,108.5 (−1,150 to −1,032) | 6/6 | −697 (−776 to −653) | 6/6 |

For the full Shelf, the arm medians were 2.447 → 1.346 ms and 1.519 → 0.836 ms: about 45% less **catalog read time**, not 45% faster startup. Absolute timings shifted between invocations on the shared worker. The [36 individual pairs](verification/catalog-shelf-20261121/paired-times.csv) include both orders and all regressions. The small empty-Shelf cost does not justify a separate implementation here. Neither this fixture nor the historical offline Shelf is a real-account benchmark.

## Software verification

The host key was accepted by the existing strict SSH configuration. The candidate built on `test-env` using the shared Ubuntu toolchain, Rust 1.99.1, the release profile, desktop and Kindle features, a private target, the shared compilation lock and one Cargo job. Tests used `umask 077` and an isolated network namespace. The full library passed **901 tests** (five ignored by default), and the main application passed **18**. The three targeted cases are also included in the library count; the timing fixture was separately invoked twice.

All four desktop startup cases passed on PW6 (1264 × 1680) and Scribe (1860 × 2480). In normal and controlled-failure runs, the first and final logical images matched their frozen references exactly; 1,280 RGB pixels changed in the activity region in each case. Both normal Shelves had two pages and matched the reference in the first observed post-ready X11 frame and three later samples. Both failures exited with status 1, without publishing ready. These are desktop observations, not optical panel measurements. No ARM package was installed and the physical device was not used.

To reproduce the focused runs from the project root on `test-env`, use the same release profile and private build environment:

```sh
cargo test --release --locked --offline -j1 --features kindle --lib combined_shelf_read
cargo test --release --locked --offline -j1 --features kindle --lib prepared_shelf_falls_back
cargo test --release --locked --offline -j1 --features kindle --lib compare_warm_catalog_shelf_reads -- --ignored --nocapture
```

## Cover handoff and direct decode decisions

In the checked Scribe demo grid, ten different ordinary covers and a separate resume cover account for 5,740,800 RGBA bytes. The 4 MiB limit is checked before each copy, so the first nine occupy 4,320,000 bytes and the remaining two 1,420,800 bytes. There is no duplicate resume cover to eliminate in this fixture.

The [one-factor desktop comparison](verification/startup-cover-worker-20261121/README.md#natural-startup-where-the-time-went) preserved those limits, the two batches and the activity indicator. Median paired UI attachment CPU fell 1.648 ms, while worker cover CPU rose 0.347 ms. The median first-observation upper bound favored B by 5.395 ms within pairs, but four brackets overlapped and only one clearly favored B. Keep the existing UI copy and yield policy. The two large deviations occurred in demo-only seeding, not during cover attachment; do not tune ordinary-account startup from these outliers.

The [direct PNG comparison](verification/startup-direct-png-20261121/README.md#results) then tested **removing** the copy on the ordinary managed cache. It used the writer's encoding settings and a synthetic offline identity; the signed-out control had the same ten resident covers but displayed none. B decoded all ten directly with identical straight RGBA and saved a median 0.529 ms of worker-plus-UI CPU per pair. Its first exact whole-Shelf observation was instead 3.496 ms later at the paired median; two intervals were entirely later and four overlapped. Retain the original path and stop iterating on cover-copy placement as a startup priority. Do not change the animation or attach schedule based on desktop measurements. A Kindle performance claim would still require an authorized native-card comparison to the same marker's driver completion.

The [ordinary desktop timeline](verification/startup-direct-png-20261121/phase-profile.md) located a 62.615–66.170 ms gap from visible feedback to state-worker queueing. The [subsequent source audit and paired trial](verification/startup-desktop-wake-20261121/README.md#paired-results) distinguished the maintenance tick from a render wake and confirmed the gain under a real software post-submit hook. The locked backend has no supported equivalent; keep the current product fallback and the native first-SEND gate.

The [cache-maintenance investigation](verification/startup-cache-maintenance-20261121/README.md) traced the ordinary transaction without changing cleanup policy. Each old fixture copy contained an existing recency record in a text format the product normalized on its first maintenance pass. For representative steady-state comparisons use product-written text; retain the exact historical input as a first-rewrite control. Neither case measures a physical Kindle or real account.
