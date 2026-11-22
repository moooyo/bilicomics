# Direct PNG decode on a populated ordinary offline Shelf

**Decision: keep A.** Direct decoding did eliminate the final decoded-RGBA-to-Slint copy for all ten visible covers. It reduced the combined CPU used by cover preparation and UI attachment by a median **0.529 ms per pair**, but the complete desktop Shelf was unambiguously earlier in **0/6** pairs. It was unambiguously later in two; the other four overlapped. Neither order group favored B. Keep the existing decoder and UI attachment in the product; retain the [experimental patch](direct-pixels.patch), not a runtime switch. The earlier, separate [populated-cache SQL optimization](../../STARTUP_PRE_READY_20261121.md#worker-comparison) is unchanged.

## What was measured

The [study design](STUDY.md) and [protocol](PROTOCOL.md) preceded timing. Two x86-64 Release binaries on `test-env` shared the build settings and inputs except for the three [listed candidate files](build.json). Applying the patch to A reproduced those three B inputs byte-for-byte. After the decision, the three restored cover inputs and the three retained catalog inputs were byte-compared on `test-env` to measured A. The change used the [locked image PNG decoder](https://docs.rs/crate/image/0.25.10/source/src/codecs/png.rs) to write into a unique [Slint RGBA pixel buffer](https://docs.rs/crate/i-slint-core/1.17.1/source/graphics/image.rs). Decoder workspace, destination initialization, full-window composition and X11 transfer still exist. Demo and unsupported inputs stayed on A's path.

This was a network-isolated, **non-demo** ordinary offline Shelf with synthetic data, not a live-account or physical-device test. The [fixture](FIXTURE.md) contained 20 distinct, partially transparent 320 × 416 PNGs re-encoded with the managed writer's encoder and settings. The first page had ten covers (5,324,800 decoded bytes) attached in two batches of eight and two. The [positive control](qualification.json) showed the expected ten visual codes in the actual Scribe-sized X11 window and independent logical image. A signed-out control with exactly the same catalog and covers had the same resident count but showed **zero** codes and the login prompt. Window and process identity, not just queue state or allocated bytes, mattered.

## Results

One warmup per arm was excluded. All twelve measured launches completed in the prescribed alternating order without failures, replacements or additional samples. Every launch matched the entire frozen A window twice and its final logical reference. Each first complete observation is an upper bound; the interval starts at the last known mismatch. Actual interval widths were **8.502–12.280 ms**, not the 4 ms polling target. The image included the unmasked clock: an [application-only fixed wall clock](wall-qualification.json) made the pixels comparable without changing monotonic or thread CPU timing.

| Metric (ms) | A median | B median | Median paired B−A |
| --- | ---: | ---: | ---: |
| Worker cover preparation, wall | 3.808 | 4.030 | +0.393 |
| Worker cover preparation, CPU | 3.778 | 3.994 | +0.397 |
| Both UI batches, wall | 0.983 | 0.018 | −0.964 |
| Both UI batches, CPU | 0.975 | 0.018 | −0.956 |
| Worker plus both UI batches, CPU | 4.759 | 4.012 | −0.529 |
| Process entry to worker result | 106.554 | 106.699 | +1.121 |
| Process entry to software ready | 110.744 | 109.411 | −0.168 |
| Process entry to first exact complete Shelf (upper bound) | 132.559 | 136.710 | **+3.496** |

Compute each paired change before taking its median; subtracting the two arm medians answers a different question. UI attachment CPU fell in all six pairs and combined cover CPU fell in five. The full-Shelf **upper-bound** paired medians were **+11.329 ms (AB)** and **+0.740 ms (BA)**. A positive change means B was observed later. The predeclared requirement was five clearly earlier intervals out of six and an earlier median first-match endpoint in both orders. It failed both conditions. This rejects the change as a startup optimization; it does not establish a universal slowdown or a Kindle result. See the [uncertainty for every pair](paired-timings.md), [endpoint summary](endpoint-summary.json) and [thread costs](stage-costs.json).

## Next measurement

The [phase profile](phase-profile.md) places the first observed loading feedback **62.615–66.170 ms before** the desktop state worker was queued in every measured launch. Once queued, that worker actually started in a median 0.21–0.24 ms. Identify whether the first UI turn was woken by rendering or by the 80 ms maintenance timer before attempting any scheduling change. This is not a forecast of savings: the Kindle already has separate first-SEND admission, and the loading feedback and initial yield must remain. For a common state-worker investigation, break down `state-cache-maintenance` (A median 7.512 ms here) before changing any cache policy.

## Verification and replay

- B passed [904 library and 18 application-binary tests](suites.json); six library tests were ignored by default. The nine targeted checks, including straight-alpha bytes, cross-thread allocation identity, fallbacks and budget boundaries, are already in the full counts. One separately invoked fixture test measured no time: it selected the same ten inputs, proved **10 direct, 0 fallback**, and matched all 5,324,800 RGBA bytes to the old decoder. No per-cover instrumentation was added to the timed runs.
- All [four PW6/Scribe demo visual regressions](visual.json) passed against their frozen normal/error references. They retained changing activity (1,280 changed pixels each); failures did not publish ready. They check unchanged demo behavior, **not** direct decode. The ordinary positive/negative controls and actual seed check provide that separate evidence.
- The [14 process observations](desktop-observations.json) and tables are available without private inputs. The optional ignored `artifacts/verification/startup-direct-png-20261121/desktop-stage-replay.tar.gz` contains only the allowlisted timings and checks. Replaying all 57 projected files reproduced both the pair tables and the CPU summary from the full traces; see the [receipt](replay.json) and [replay commands](PROTOCOL.md#reproducible-restricted-output). No session, database, image, account content or executable is archived in it.
- No Kindle lease, deployment, panel observation, SEND timestamp or same-marker driver-completion timing was part of this test. Do not select this patch for installation on the strength of the CPU result.
