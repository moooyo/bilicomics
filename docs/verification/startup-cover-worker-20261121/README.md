# Startup cover handoff on the offline desktop Shelf

**Decision: keep A.** Moving the RGBA copy to the worker made both UI attachment batches cheaper in every pair, but did not make the complete Shelf reliably earlier. Only **one of six** pairs put B's entire first-Shelf interval before A's; four overlapped and one put B entirely later. The predeclared requirement was at least five clear wins. The two candidate Rust files have been restored; the preceding catalog optimization is unchanged. The non-default candidate is available as a patch, not a feature flag. Nothing was installed or measured on a Kindle.

## Compared builds

Both x86-64 Release builds used `9175002` plus the same [cached Shelf membership change](../../STARTUP_PRE_READY_20261121.md#one-product-candidate). A copied decoded RGBA into a Slint allocation during UI attachment. B made the same copy on the preparation worker and imported that allocation on the UI. The [candidate patch](worker-pixels.patch) applied to A reproduced both of B's changed source files byte-for-byte before measurement. No runtime switch, animation, cover order, memory budget or display barrier changed.

The private `test-env` target used `cargo build --release --locked --offline -j1 --features kindle --bin bilicomics`, with the default desktop feature enabled. The two executables were saved separately. The desktop uses its normal state worker; a native Kindle launch has different worker gating and a display driver. This study measures neither a native Library launch nor an E Ink update.

## Software and pixel checks

- B passed [903 library tests and 18 application-binary tests](suites.json) in an isolated network namespace; five library tests were ignored by default. Six targeted invocations also passed and are not added to those totals. They cover straight-alpha bytes and identical allocation across the worker/UI boundary, the exact 16 MiB retention limit, partial-batch yielding, split/serial equivalence, suspension and cancellation.
- In [four separate, non-timing Xvfb cases](visual.json), PW6 and Scribe matched their frozen initial and final logical RGB references. The activity region changed by 1,280 pixels in every case. Both normal cases showed the complete two-page Shelf with zero RGB differences at the first post-ready observation and three subsequent samples. Both controlled failures exited with status 1 and never reported ready.
- The [predeclared comparison](PLAN.md) uses natural startup without the artificial delay used for the activity check. Setup and observer failures are recorded separately from valid samples; they are not timings of either product build.

## Natural startup: where the time went

The completed trial used an offline Scribe-sized Xvfb window, two excluded warmups and six alternating AB/BA pairs. Each launch received its own copy of the same closed synthetic seed. Every one of the 14 launches reached the exact full-window Shelf, matched it a second time and matched the independent final logical image. No run was removed. The [case and pair tables](paired-timings.md) show the two UI batches separately, including their wall and thread CPU times and every transition bracket. The [original observer output](desktop-observations.json) also retains the warmups.

| Measured stage (ms) | A median | B median | Median paired B−A |
| --- | ---: | ---: | ---: |
| Worker cover preparation, wall | 4.358 | 4.696 | +0.348 |
| Worker cover preparation, CPU | 4.325 | 4.661 | +0.347 |
| Both UI cover batches, wall | 1.681 | 0.014 | −1.665 |
| Both UI cover batches, CPU | 1.664 | 0.014 | −1.648 |
| Gap between the two UI batches | 1.108 | 1.048 | −0.043 |
| Process entry to worker result | 158.935 | 155.001 | −2.511 |
| Worker result to UI receipt | 0.095 | 0.081 | −0.015 |
| UI receipt to software ready | 4.942 | 2.990 | −1.692 |
| Process entry to software ready | 164.146 | 158.361 | −4.150 |
| Process entry to first exact X11 Shelf (upper bound) | 201.264 | 200.105 | −5.395 |

The arm medians and the medians of the six within-pair changes are different statistics. Five first-match upper bounds favored B, as did their median in both order groups (AB: −7.669 ms; BA: −1.093 ms). But a first observation is not the instant the window changed. Across processes, median polling intervals were 4.08–4.14 ms; the actual last-negative-start to first-positive-return brackets were **10.251–13.430 ms** wide. Only pair 6 was unambiguously earlier; pair 4 was unambiguously later. Earlier software readiness and cheaper attachment do not meet the user-visible endpoint. The desktop's roughly 1 ms inter-batch gap also does not reproduce the historical 410–509 ms native gaps with visible loading updates.

The two largest first-match differences point in opposite directions and predate UI attachment. In pair 4, B was first observed 629.378 ms later than A; in pair 6, B was first observed 188.794 ms earlier. The [application's demo-only database seeding spans](demo-seed-spans.json) were 672.035 ms in `b04` and 246.837 ms in `a06`, versus 46.270–54.570 ms in the other ten cases. This is the **per-launch demo stage**, not the harness's prior seed preparation. The traces locate the long spans, but do not identify their underlying cause or predict a real account's behavior. Neither outlier was excluded.

The first observed feedback upper-bound medians were 31.409 ms (A) and 29.643 ms (B); the lower bound is zero when the first window sample already shows feedback. After the second confirmed Shelf observation, median `/proc` VmRSS and VmHWM read 69.26 MiB for A and 69.11 MiB for B. These are readings taken at that instant, not whole-run peak memory or evidence of lower Kindle memory use.

## Failed admission and replay

The [attempt log](setup-attempts.json) retains two non-performance failures. An initial A seed was not admitted because an Xvfb child had not yet retired; a bounded post-exit check fixed setup without changing an endpoint. The original timing run then stopped at its **first A warmup**, before any completed warmup or measured pair: the mock used depth 24, but the application window used depth 32. An [untimed control qualification](x11-layout-qualification.json) checked the RGB planes with both masks against the frozen full-window reference. This distinction is consistent with the [Xlib definition of XGetImage](https://www.x.org/releases/X11R7.6/doc/libX11/specs/libX11/libX11.html): ZPixmap depth follows the drawable. The [v2 addendum](PLAN.md#independent-format-admission-v2) admitted the verified format before starting a new trial from scratch. No cases were replaced, combined or selected across runs.

For fresh runs, use the [frozen protocol](PLAN.md#desktop-protocol-frozen-before-measurement) on `test-env`. From an A checkout, `git apply docs/verification/startup-cover-worker-20261121/worker-pixels.patch` reconstructs the experimental B files. The final working tree's two restored cover sources and the three catalog inputs were byte-compared on `test-env` to the inputs used by A.

For read-only replay on `test-env`, extract the retained `artifacts/verification/startup-cover-worker-20261121/desktop-stage-replay.tar.gz` into a fresh private directory and run `python3 -B summarize_pairs.py /path/to/extraction/trial`. This optional archive lives outside Git: it contains the observer report and nine allowlisted startup events for each launch (126 total), not images, databases or binaries. On the verification host the extracted data reproduced `paired-timings.md` byte-for-byte. The checked-in JSON and timing tables are sufficient to review the decision without the archive.
