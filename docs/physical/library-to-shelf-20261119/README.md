# Native Kindle Library click to BiliComics Shelf

**2026-11-19, first-generation Kindle Scribe, firmware 5.19.5.** From release of
the native Library card to the first complete Shelf frame, the repeat median was
**7.137 s to submit and 7.503 s to driver completion**. This measures a click on
the actual native card, not a command opening its URI. It is not a camera or
finger-to-glass measurement.

## Measured times

`lib-t-1119-a709c5c7` completed one first launch and the five planned repeats.
Each had a distinct application process. The device stayed booted and OS caches
were retained; the first launch after temporary installation is not a cold boot.
All six targeted the same offline demonstration Shelf. No case was replaced.

| Endpoint from card release | First in batch | Repeat median | Repeat min-max |
| --- | ---: | ---: | ---: |
| Rust process entry | 4.317 s | 4.367 s | 3.948-4.616 s |
| First application startup screen submitted | 4.790 s | 4.634 s | 4.212-4.889 s |
| Shelf software ready | 6.030 s | 6.676 s | 5.427-6.924 s |
| First full Shelf frame submitted | 7.226 s | 7.137 s | 6.191-7.396 s |
| Driver confirmed the same Shelf update | 7.592 s | 7.503 s | 6.557-7.762 s |

The origin above is the device's `CLOCK_MONOTONIC` immediately **before sending
button-up**. The confirmed-up (`XSync` return) medians were 7.124 s and 7.490 s
for Shelf submission and driver completion. The release brackets were 0.681 to
13.121 ms. Counting instead from just before button-down gave **7.190 s and
7.555 s**. The target checks and native-screen stabilization precede the click
and are excluded. The shared native handler can show feedback before the first
application-owned startup screen.

The native GUI was on the Library tab with the real indexed `bilicomics.sh`
entry. On-device checks confirmed the public navigation controls, the expected
card's unique full title, and its unchanged title region immediately before a
50 ms XTest button-1 click. No native screen image or unrelated title was
exported. This exercises the graphical activation path but not the digitizer.
Neither URI dispatch nor the separate clock before the click helper is the
click timestamp.

The selected SEND must be a successful full 1860 x 2480 replacement whose Slint
render began after `startup/ui_ready`, with the lifecycle and final route both
`shelf`. The driver endpoint is the end of the successful wait for that same
positive update marker. In all six cases the first Shelf RGB fingerprint also
matched the decoded final logical PNG. The last case's separate, later live
Gray8 framebuffer capture matched the prior qualified Shelf bytes. This does
not time optical settling, first accepted user interaction, live-account sync,
or cover downloads. The opt-in demo RGB fingerprint cost about 51 ms on the
first Shelf frame; timing includes this diagnostic work.

## Optimization priorities

1. **Look before Rust first.** Release to Rust took 4.367 s at the median.
   Release to the instrumented entry mark took about 0.45 s. The worker mark to
   the start of native sampling took 2.75 s, including the production launcher's
   unconditional [`sleep 2`](../../../packaging/kindle/launch.sh). The sample
   took 0.45 s, subsequent preparation to the start of display takeover 0.35 s,
   and takeover itself 0.10 s. The fixed timer accounts for about **27%** of the
   measured end-to-end time. Two seconds is the budget of this timer, **not**
   demonstrated savings. Trial a positive readiness signal for the native window
   handoff and power/USB event subscription; retain a conservative fallback.
   A/B the same card and verify startup-time events and native restoration before
   adopting any shorter wait. Do not equate observer process existence with
   readiness.
2. **Target the extra startup refresh, not the route calculation.** In repeats
   01, 03 and 04 the app yielded between Route and Views and sent a full flashing
   GC16 update with the same RGB fingerprint as the previous startup frame.
   The interval between those stages was 820-825 ms, versus less than 1 ms in
   repeats 02 and 05. The latter still waited 334-337 ms for a preceding update
   before rendering the Shelf. Rust-entry-to-Shelf-SEND medians were 2.780 s
   versus 2.221 s in these groups: **0.559 s is an observed difference**, not a
   guaranteed gain or a figure to add to the timer. Test coalescing that specific
   unchanged loading frame while preserving first feedback, animation changes,
   input and refresh barriers. A [prior hidden-route trial](../../STARTUP_SEQUENCE_PERFORMANCE_20261117.md)
   reduced frames without improving end-to-end time.
3. **Lower priority:** the first Shelf frame's Slint render took 344 ms and its
   framebuffer write 30 ms at the median. SEND to driver confirmation took
   366 ms. Omitting that wait from the report would move the endpoint, not make
   the panel faster. These medians describe separate per-run intervals and are
   not an additive speedup model.

## Scope and evidence

The canonical BiliComics install, both document spellings, selected sidecars and
catalog entries were absent at preflight. The run temporarily staged the
qualified development package (`c57d5fc3...`) with a common instrumented launcher
and the unchanged application (`04fc6f93...`), using `--library` and the isolated
`--demo --offline --screen shelf` fixture. The product sources match `main` at
`d81b529` (packaging `39fac82`, runtime `530f151`). Normal exit, the original
native tab and pointer, lighting, selected registration and original BiliComics
absences were checked. Owned connections were closed and the exact device
lease released. No permanent install or product change was made.

- [Click timings and recovery](click-timing.json): one first launch and all five
  repeats, all four input brackets, selected frame markers and source hashes.
  [Launcher intervals](click-launcher.json) and [frame comparison](click-frames.json)
  give the underlying evidence for the two optimization candidates.
- [Same-day URI control](uri-partial.json): only its first launch completed
  (6.481 s to SEND, 6.846 s to driver completion). The next case stopped before
  dispatch; independent recovery restored and released the device. It is not a
  repeat distribution. A separate click batch refused an occupied lease and
  never accessed the device. Neither attempt contributes a click sample here.
- [Earlier six URI launches](archived-timing.json) and [their launcher spans](archived-launcher.json):
  6.289 s median to Shelf SEND, 6.654 s to driver completion. These are native
  request measurements, not card clicks. Subtracting these medians from today's
  results would not measure the cost of the GUI click.

The `project_*.py` readers in this directory operate on stored evidence on
`test-env`, without a device lease. Private transport inputs, tokens, raw
archives, account data and native screenshots are not included.
