# Startup and Sleep Lifecycle

The user requested these changes on 2026-11-04: show the project name and startup activity before lengthy initialization, and return to the current reading session after sleep rather than the bookshelf.

## Startup

The first application scene displays BiliComics, a Chinese startup label, a real preparation phase, and four discrete black-and-white activity segments. The indicator advances at approximately 400 ms intervals. It does not report an invented percentage or animate the whole screen.

When startup opens a ready, unobscured Shelf on Kindle, its first complete frame may render into private RGB memory while the preceding panel update is still running. Mapped framebuffer writes must still wait for that update. The new Shelf update must also complete on its own accepted marker. Loading frames, Reader, sleep and wake do not take this path. See the [paired Scribe measurements](physical/shelf-render-20261121/README.md).

A 22,040-byte, independently named font subset supplies this scene before the full CJK fonts are decompressed. The Kindle backend is installed once with this font before worker threads are created. The same application window and event loop remain in use throughout startup.

Workers prepare the full fonts, credentials, SQLite state, catalog and transaction recovery, demo assets, pinyin dictionary, local availability snapshots, and initial cover pixels. Full-font registration and UI attachment then yield between stages. The startup scene is removed only after the prepared application is attached. Preparation failure stops activity and presents an exit action.

`--capture-startup <path>.png` records two actual software-rendered startup frames and metadata. The demonstration-only `--startup-delay-ms` option supplies a bounded slow-worker fixture; production startup does not introduce that delay. Scripted actions and bounded normal captures begin their clock at application readiness.

### Primary startup metric: first Shelf render

Start at release of the **ordinary Kindle Library card**, not a URI request, Rust process entry or software Ready. Stop when the **first Shelf frame in that launch finishes rendering** its normal layout, entries (or the appropriate empty state), and navigation. Cover placeholders qualify. A loading screen or a generic skeleton without those Shelf elements does not. Do not wait for cover completion or send a second tap to test input.

Identify the qualifying scene without cover pixels and report, from the same card-release origin: software render completion (`render_end`), the accepted `SEND` for that frame, and driver completion for that same positive update marker if observed. Label missing timestamps rather than substituting `ui_ready`, an unrelated receipt or another launch. These are software and driver observations, not the instant the ink first became visible. Startup feedback and cover completion are separate secondary measurements. Do not ask the user to tap, record video or run scripts. See the [Kindle verification procedure](verification/startup-interactive-kindle-20261121/README.md).

The [first render-only Scribe measurements](verification/startup-interactive-kindle-20261121/README.md#scribe-results) are approximately **6.451 s** and **6.727 s** from the automatically released native Library card to the software renderer's return. The same instrumented executable supplied both observations; these are not an optimization comparison.

The [six-pair same-binary A/B](verification/startup-shelf-ab-20261121/README.md) isolated the private-render ordering on that metric: the default improved 4/6 pairs by a paired median of **14.8–17.3 ms**, below the predeclared five-win and 100 ms thresholds. The intended order really ran in every pair. Keep the installed behavior, but do not claim a material first-Shelf-render gain or use the historical complete-frame result in its place.

The Library-parent candidate (B) removes a duplicate preflight; its worker still validates the current configuration before acquiring the display lock. Two Linux comparisons saved about 3 ms in that stage. The [November 21 Scribe series](verification/startup-library-admission-20261121/README.md) yielded no complete timed pairs. The [independent November 22 run](verification/startup-library-admission-20261122/README.md) completed all six: B reached the first Shelf render sooner in 3/6, with a paired median B−A of **+64.139 to +66.997 ms** (B slower). It did not meet the five-win and ≥100 ms improvement gate. The first launch in every pair was faster regardless of arm, so the observed difference does not establish an intrinsic regression. Do not roll out this change for speed; the installed launcher is unchanged.

The [independent Scribe A/A](verification/startup-clock-aa-20261122/README.md) used the same launcher and application in both slots. All ten visits reached the normal Shelf and returned native control. Excluding two warmups, the eight launches had a median card-release-to-render time of **6.324215–6.325388 s**. The second launch was faster in 2/4 pairs (median second minus first: −42.734 to −40.089 ms). This is a baseline, not a speedup or a fixed advantage for the first launch.

On the same device clock, median card release to the first BiliComics shim mark **after its route checks** was 437.3–438.4 ms; shim to application invocation was 3.220 s; just before invocation to Rust `main`, 19.0 ms; `main` to the trace header, 16.7 ms; and header to renderer return, 2.657 s. Do not add these medians. The launcher span includes the protected two-second power-observer settle and is not a savings estimate. The A/A did not time the UI's receipt of prepared state.

The separate prepared-state study completed three ordinary Scribe visits. The worker's pre-send timestamp to the UI's first successful receive measured **0.082, 105.155 and 33.879 ms**: none reached the predeclared 250 ms. All three first Shelves rendered; no speedup comparison was made. Keep startup feedback and display waits unchanged. See the [per-visit evidence](verification/startup-prepared-handoff-20261122/README.md).

In [three later Scribe visits](verification/startup-display-sampling-20261122/README.md), card release to the first complete Shelf render was **6.430–6.432 s** at the median. The native display helper took **300.502 ms**, including **213.896 ms** waiting in `poll`. All visits ran the same diagnostic binary; this is not an improvement or an optical measurement. Keep its live checks and stability samples. The [historical 410 ms](verification/startup-display-sampling-20261122/DEVICE_REPLAY.md) was grant to parent-observed success in a *different* launch series, inside a 480 ms median transaction. Subtracting the two would not measure the launcher's overhead.

A [new three-visit capture](verification/startup-display-poll-20261122/README.md) classified each existing `poll` call. **81.5–90.1%** of poll time was before the helper had observed EOF on the output pipe; after EOF it spent only **20.195–42.504 ms**, including any remaining child execution. Keep the original poll interval and successful-exit check. The median card-release-to-Shelf render was **6.401–6.403 s** on this single diagnostic arm. It did not change or measure the effect of progressive covers.

<a id="primary-startup-metric-interactive-shelf"></a>

### Historical interactive and cover-complete measurements

The earlier [desktop interactive-Shelf study](verification/startup-interactive-shelf-20261121/README.md) required a fresh click on the presented Shelf and a confirmed menu response in the same process: six successes, **180.376 ms median upper bound** from desktop launch. A separate zero-cover control also responded. The [6.802 s Scribe median](physical/shelf-render-20261121/README.md#device-results) instead measured native card release to driver confirmation of a complete offline Shelf; the [56.338 ms desktop improvement](verification/startup-desktop-wake-20261121/README.md#paired-results) also used a complete-frame endpoint. None is a measurement of the new primary render endpoint. The startup destination and Reader sleep/resume behavior are unchanged.

## Sleep and wake

Sleep preserves the same application process, `ReadingState`, reader core, settings, page images, route, and source position. It does not close the reader, initialize a new application, or apply the ordinary `launch=shelf` preference.

The launcher observes the native power daemon. On entry to the screensaver, it requests a suspend transition from the existing application. The main thread inhibits ordinary intents and download transport. When a current custom-cover request and preference permit it, the application shows its already prepared overlay and returns to the event loop until a checked full update completes. Only then does the backend cancel pointer state, release the exclusive input device, gate timers and drawing, clear stale refresh requests, and wait for existing panel work. Reading/preferences and local receipts are checkpointed. A matching successful cover and checkpoint acknowledgement lets the launcher retain its stopped native display services; other outcomes use the system screensaver. [Comic-cover sleep](SLEEP_COVER.md) records this additional presentation boundary.

An acknowledgement binds the application generation, random launch token, request sequence, and action. The signal bridge uses a nonblocking self-pipe; its reader posts control closures to Slint's existing event-loop proxy. Resume therefore remains deliverable while ordinary native timers are paused. No signal handler mutates UI state, locks a Rust mutex, or performs storage work.

Wake requires the user's screensaver-exit flow and an active/unlock confirmation. A retained custom cover uses the current user-wake event and confirmed active power state, without querying the stopped native window manager. A periodic CPU/RTC wake is insufficient. The overlay is hidden before the backend reacquires fresh input and rewrites the complete retained scene before submitting a GC16 full update and admitting normal input/timers. The reader resumes from RAM; a failed storage checkpoint does not replace it with an older saved anchor.

Only download work that this sleep transition paused is eligible to resume, with account, epoch, task-generation, and reader-owner checks. Previously paused, failed, cancelled, or unrelated work is not restarted. Payment/recharge recovery only persists or reconciles existing results; waking never retransmits an uncertain purchase.

USB takeover and an explicit application exit remain separate termination paths. The lifecycle protocol does not disable native passwords, bypass unlock, or introduce a separate RTC/sysfs suspend scheduler.

## Evidence boundary

The user later authorized a supervised physical Scribe 5.19.5 run of the frozen
0.1.3 candidate. [Physical verification](PHYSICAL_VERIFICATION.md) records the
same PID and full source anchor through real power-button sleep/wake, followed
by a real finger page turn and restored native control. It preserves the first
run's timed-exit interference and the successful longer retry separately. Those
results predate the 0.1.4 immersive-reader changes and retain their original
executable and source identities.

Remote software checks and simulated power-daemon events are recorded under `artifacts/verification/` and the task's isolated `test-env` directory. Launcher fixtures, real Rust-process signal tests, pure backend control tests, software rendering, ARM compilation, and physical Kindle behavior are distinct forms of evidence. A display-control acknowledgement is not proof of physical panel completion or device compatibility.

The launcher retains its exact model/firmware allowlist. No physical device or new compatibility combination is accepted by these source changes alone.

The subsequent [0.1.3 pre-device verification](PRE_DEVICE_VERIFICATION.md) executes the complete native path in an independent Scribe 5.20.1 guest. It exposed a fresh evdev lease losing unchanged position axes after wake. The corrected backend restores per-slot coordinates, retains discarded coordinate deltas without replaying wake contacts, and distinguishes global BTN events from Protocol-B tracking ownership. The final guest retains the same PID and page 19 through sleep/wake, then reaches page 20 on the original first tap. Its seven native UI cases and the corrected backend's 34 software tests passed. These results remain distinct from the pending physical Scribe 5.19.5 run and do not accept atomic multi-pointer acquisition, real panel timing, or battery behavior.

The 0.1.1 results are recorded in [the runtime summary](../artifacts/verification/startup-sleep-0.1.1/runtime-verification-summary.json) and [ARM/archive report](../artifacts/verification/startup-sleep-0.1.1/startup-sleep-0.1.1-arm-package-verification.json). The final remote runs passed 129 application tests, 27 backend tests, and 28 launcher fixtures. Three actual startup cases verified PW5/Scribe activity frames and a controlled failure page; separate real-process cases verified early power signals, retained reading with `launch=shelf`, ineffective suspended input, and checkpoint failure recovery without changing unknown payment intentions.
