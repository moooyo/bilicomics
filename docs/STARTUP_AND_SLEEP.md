# Startup and Sleep Lifecycle

The user requested these changes on 2026-11-04: show the project name and startup activity before lengthy initialization, and return to the current reading session after sleep rather than the bookshelf.

## Startup

The first application scene displays BiliComics, a Chinese startup label, a real preparation phase, and four discrete black-and-white activity segments. The indicator advances at approximately 400 ms intervals. It does not report an invented percentage or animate the whole screen.

A 22,040-byte, independently named font subset supplies this scene before the full CJK fonts are decompressed. The Kindle backend is installed once with this font before worker threads are created. The same application window and event loop remain in use throughout startup.

Workers prepare the full fonts, credentials, SQLite state, catalog and transaction recovery, demo assets, pinyin dictionary, local availability snapshots, and initial cover pixels. Full-font registration and UI attachment then yield between stages. The startup scene is removed only after the prepared application is attached. Preparation failure stops activity and presents an exit action.

`--capture-startup <path>.png` records two actual software-rendered startup frames and metadata. The demonstration-only `--startup-delay-ms` option supplies a bounded slow-worker fixture; production startup does not introduce that delay. Scripted actions and bounded normal captures begin their clock at application readiness.

## Sleep and wake

Sleep preserves the same application process, `ReadingState`, reader core, settings, page images, route, and source position. It does not close the reader, initialize a new application, or apply the ordinary `launch=shelf` preference.

The launcher observes the native power daemon. On entry to the screensaver, it requests a suspend transition from the existing application. The main thread inhibits ordinary intents and download transport, then the native backend cancels pointer state, releases the exclusive input device, gates timers and drawing, clears stale refresh requests, and waits for existing panel work. Reading/preferences and local receipts are checkpointed. Only a matching acknowledgement lets the launcher release display services that it owns.

An acknowledgement binds the application generation, random launch token, request sequence, and action. The signal bridge uses a nonblocking self-pipe; its reader posts control closures to Slint's existing event-loop proxy. Resume therefore remains deliverable while ordinary native timers are paused. No signal handler mutates UI state, locks a Rust mutex, or performs storage work.

Wake requires the user's screensaver-exit flow and an active/unlock confirmation. A periodic CPU/RTC wake is insufficient. The backend reacquires fresh input and rewrites the complete retained scene before submitting a GC16 full update and admitting normal input/timers. The reader resumes from RAM; a failed storage checkpoint does not replace it with an older saved anchor.

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
