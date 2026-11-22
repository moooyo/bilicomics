# Interactive Shelf startup: desktop measurement

**Result: all six measured desktop launches responded to one fresh click.** From host launch to an externally confirmed filter-menu response, the median upper bound on interactive readiness was **180.376 ms** (164.409–187.333 ms). The non-cover Shelf controls and first title had already been observed by **132.761 ms** at the median. One warmup was excluded; there were no failures, replacements or extra timed launches. These are first baseline measurements of one build, not a speed improvement or a Kindle time.

## What counts

This study used the [historical interactive-Shelf endpoint](../../STARTUP_AND_SLEEP.md#primary-startup-metric-interactive-shelf), which needed *both* presentation and working input. The [current Kindle primary metric](../../STARTUP_AND_SLEEP.md#primary-startup-metric-first-shelf-render) ends at the first qualifying Shelf render and requires no input probe. Here the observer recognizes the populated Shelf from its toolbar and first book's title, sends a fresh primary-button click to the visible filter in the same window and process, and verifies the menu's contents. It does not wait for cover artwork or rely on software Ready. The end-of-run semantic check corroborates the response; it does not time it.

The [protocol](PROTOCOL.md) separates three timestamps: first observed non-cover Shelf, first actual click, and first confirmed menu response. The last gives a **conservative upper bound** on the first time a user could interact; the sampling interval, pointer checks, deliberate tap hold and rendering the small response all consume time. It is not the exact instant the app enabled input. The desktop origin is the host launching the process, not a user's click on a native Kindle Library card. Numbers from the [November 21 driver study](../../physical/shelf-render-20261121/README.md#device-results) have a different origin, platform and endpoint; do not subtract them from this baseline.

## Measurements

The host launched the [stock Release executable](build.json) on a fresh Scribe-sized Xvfb for each case. Each new copy of the [offline synthetic profile](seed.json) opened its saved Shelf by default, without a route override. The pointer button was not pressed until the non-cover Shelf regions on the owned window **and** the visible desktop root matched. The menu was still closed at that point. All seven processes including the warmup accepted the normal XTest gesture, showed the expected menu row in the same window and on the root, and exited with the menu open in the independently recorded final application state. Neither full performance tracing nor UI tracing was enabled.

| Measured interval | Median (ms) | Min–max (ms) |
| --- | ---: | ---: |
| Host launch to confirmed non-cover Shelf pixels | 132.761 | 128.902–145.804 |
| Shelf pixels confirmed to first press request (observer checks) | 0.605 | 0.575–0.988 |
| Press synchronization returned to release requested (deliberate hold) | 16.183 | 16.173–16.228 |
| Release synchronization returned to confirmed menu pixels | 27.497 | 14.500–34.259 |
| **Host launch to confirmed menu response (interactive upper bound)** | **180.376** | **164.409–187.333** |

These are medians of intervals calculated separately for each launch; do not add them. The 80 ms stability recheck and end-of-run capture took place *after* the reported response. The six [individual observations](results.json) retain the click and sampling brackets. To reproduce the [summary](summary.json) without the application or private data, run `python3 -B audit_interaction.py results.json` from this directory on `test-env`. The [independent audit](audit.json) also checked the original per-launch lifecycle and final-state records.

## Why covers do not gate the test

In [separate functional controls](visual-qualification.json), the same twenty book entries remained on the Shelf after removing the synthetic cached artwork. The ten visible slots displayed labeled placeholders, and **zero** cover images were resident. A fresh click opened the same filter menu; clicking outside restored the Shelf. With cached artwork, the same control also opened and closed. Hovering without a click did not open the menu during the observation window; a signed-out view did not qualify as the populated Shelf. These four controls are not timing samples. The six measured launches used the ordinary populated cache and the observer never used its cover count to time or trigger the tap.

## Limits

This is an isolated, ordinary offline desktop launch with synthetic local books and Scribe geometry, not a live-account or physical-panel test. Missing cover files are a useful functional control: a book title and a working Shelf control must still qualify without their artwork. That does not simulate slow decoding or progressive network delivery. Current startup prepares available initial cover pixels on the worker and consumes its prepared batch before finishing attachment; this single-build baseline cannot quantify the benefit of deferring that work.

**Native-card result: not measured.** Maintenance initially prevented ordinary testing. During this desktop batch Aidoku reported that its administrative cleanup was complete and ordinary lease requests had reopened (10:50 Asia/Shanghai); maintenance is not the remaining blocker. An old card-to-complete-frame measurement and an input check from another run cannot establish this endpoint. The qualified desktop XTest path is not a tap on the Kindle application's native input path. No device connection, lease or native launch was requested for this study.

For new native measurements, see the [first Shelf render procedure](../startup-interactive-kindle-20261121/README.md). It starts at the ordinary Library card release and times rendering, not the menu response used here. It does not require a second tap, a recording or a user-run script.
