# Ordinary Library card to first Shelf render: Scribe, November 22

**Decision: keep the installed launcher and the same control in packaging.** Checking the configuration once instead of twice saved about 3 ms in the launcher stage on Linux, but did not make the first Shelf render consistently earlier on the Scribe. The candidate was faster in **3 of 6 pairs**; its paired median was **64.139–66.997 ms later**. It is retained as a [research-only diff](../startup-library-admission-20261121/worker-only.patch), not a startup optimization to deploy.

## What was measured

On one first-generation Scribe (5.19.5), **A** ran the original preflight in both the ordinary Library parent and its detached worker. **B** removed only the parent's check; the worker still validated the current configuration before taking the display lock. Both used the same 74,392,336-byte diagnostic application, default renderer, isolated offline Shelf and ordinary lowercase native Library card. This tested a launcher change, not a different app binary or render policy.

The primary duration started at release of that card and ended when the software renderer returned from drawing the first complete normal Shelf: layout, entries or appropriate empty state, and navigation. Cover placeholders qualify. The release and app events used the same device monotonic clock. No follow-up tap, cover-complete observation or recording was needed. Neither renderer return, accepted SEND nor driver completion establishes optical visibility.

## Paired result

The [frozen plan](PLAN.md) excluded two warmups (A, B), then measured six pairs in order **AB, BA, AB, BA, AB, BA**. All six qualified. No visit from the [incomplete November 21 series](../startup-library-admission-20261121/README.md) was reused. The threshold was at least five clear B wins and a paired median improvement of at least 100 ms; neither condition passed.

| Endpoint after card release | Pairs with B faster | Median paired B minus A |
| --- | ---: | ---: |
| First complete Shelf software render (primary) | 3/6 | +64.139 to +66.997 ms |
| Same frame's accepted SEND | 3/6 | +62.359 to +64.766 ms |
| Driver completion for that SEND's marker | 3/6 | +63.508 to +65.748 ms |

Positive means B took longer. Each interval reflects uncertainty in the instant of card release, **not a statistical confidence interval**. The medians come from the six within-pair differences. The separate arm medians were about **6.354 s (A)** and **6.438 s (B)**; subtracting those is not the paired result. The [endpoint data](physical-result.json) include every visit, pair and threshold check.

**The first launch in every measured pair was faster.** B lost all three AB pairs (median B minus A: +154.030 to +156.277 ms) and won all three BA pairs (−33.745 to −31.014 ms). That order pattern does not identify its cause, or show that removing the check intrinsically costs 65 ms. In the [stage breakdown](segments.json), card release to the first app trace header was 4.8–7.2 ms later at the paired median, and trace header to render was 48.9 ms later. The header is not process entry, and these stage medians do not add to the primary median.

## Verification and next step

- The [two Linux comparisons](../startup-library-admission-20261121/HOST_RESULTS.md) saved 3.186 and 2.838 ms in preflight. All 158 launcher lifecycle tests passed. Before this series, `test-env` also passed the operator (25), timing-bound (15), series (8) and analysis (20) tests. The [ARM checks](arm-qualification.json) exercised the expected missing-launcher refusal for Scribe and PW6 under QEMU; they are not physical PW6 measurements.
- The host recorder now distinguishes a navigation failure from an uncertain input dispatch without retrying a card tap. It writes the optional diagnostic only after exact lease release. This changed failure reporting, not either measured treatment.
- All 14 launches used distinct processes on one device boot. Every launch produced a ready Shelf and a matching driver receipt, exited normally, restored the original launcher and native control, closed its connections and released its own lease. The final release was at **04:09:14 Asia/Shanghai**. The daily installation is unchanged; temporary deployment payloads were removed and the allowlisted receipts retained for replay.

Close this candidate's performance study. For a larger target, first separate native dispatch, launcher handoff and early app initialization on the device clock: card release to the first diagnostic header alone took about **3.7 s** in both arms. That span is not all launcher work and is not a savings estimate.
