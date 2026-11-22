# Where startup time goes on the Scribe: identical-launcher A/A

**The first launch was not consistently faster when the software was identical.** Of four complete pairs, the second launch was later twice and earlier twice. Across the eight measured launches, the median from release of the ordinary native Library card to return of the first complete Shelf software render was **6.324–6.325 s**. This is a diagnostic baseline, not a new speedup, a regression, or a reason to change the installed launcher.

## Same-clock breakdown

Both slots ran the original stock preflight, the same instrumented launcher and the same 74,392,336-byte ARM app on one first-generation Scribe (5.19.5). Each started from a fresh copy of the same offline Shelf. All ten launches, including the two excluded warmups, produced a populated `ready` Shelf, an accepted SEND, and driver completion for that marker. The primary endpoint requires the layout, entries and navigation, but not all covers, another tap or optical panel settling.

| Interval on the device monotonic clock | Median of eight measured visits |
| --- | ---: |
| Native card release → first measured point in the BiliComics launcher shim | 437.3–438.4 ms |
| Shim mark → immediately before invoking the application | **3,220.4 ms** |
| Pre-invocation mark → start of Rust `main` | 19.0 ms |
| Rust `main` → first diagnostic trace header | 16.7 ms |
| Trace header → first complete Shelf renderer return | **2,657.5 ms** |
| Renderer return → accepted SEND | 40.2 ms |
| Accepted SEND → completion for the same marker | 365.7 ms |

The shim mark follows its route checks, so the first interval includes native dispatch **and** some project code. The 3.220 s includes launcher safeguards and the required power-observer settling time; it is not a savings estimate. These are separate per-stage medians, not an additive timeline. Inside the last software stage, the worker produced the prepared state a median 1.662 s after the header; another 1.027 s elapsed from production to render. The fixed ARM did not record the UI receive time, so **1.027 s is not queue delay**. The [separate handoff study](../startup-prepared-handoff-20261122/README.md) measures receipt directly without changing this experiment.

## Did launch order matter?

| Measured pair | Second minus first, card release → Shelf render |
| --- | ---: |
| 1 | +32.3 to +35.2 ms |
| 2 | +51.9 to +54.1 ms |
| 3 | −130.0 to −127.8 ms |
| 4 | −117.8 to −115.4 ms |
| **Paired median** | **−42.734 to −40.089 ms** |

Positive means the second launch took longer. Bounds reflect the observed card-release interval, not statistical confidence. Within-pair release-to-release cadence had a median of **175.5 s**; across pairs it was **886.6 s**. Those gaps include running the app, returning native control, turnover and any intervening use; they are not pure idle time. Do not subtract this series from the [earlier preflight A/B](../startup-library-admission-20261122/README.md), assign a correction for visit position, or infer a cache effect.

## Evidence and disposition

The [frozen plan](PLAN.md), [per-visit durations and paired arithmetic](aa-result.json), [real window checks on both desktop sizes](host-profile-qualification.json), and [ARM emulator guard checks](arm-qualification.json) are separate evidence. The emulator checks did not render a Kindle Shelf. All ten device visits ran in distinct processes on one boot, exited normally, restored the installed launcher and native control, and released their own leases. Timing used logs, not user input or video. The private replay archive contains only the allowlisted records; no production installation changed.
