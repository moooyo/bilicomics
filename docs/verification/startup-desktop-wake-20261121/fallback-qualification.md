# Rejected submissions and late callbacks

These are correctness checks for the private Linux/Winit study, not speed samples. A routine successful X11 launch cannot demonstrate how the renderer reacts to a failed submission or whether a suspended callback abstains.

## Renderer: exact research module

On `test-env`, the standalone input was compared byte for byte with `renderer/sw_submit.rs` from the exact patched backend source used for the research build. The shared Ubuntu toolchain ran `rustc --edition 2024 --test sw_submit.rs -o /tmp/sw-submit-tests` with Rust 1.99.1 and executed the two tests serially. Both passed. An initial attempt on the bare host lacked a C linker and ran no tests; the retry used the existing Ubuntu toolchain, not a second source version. Both temporary outputs were removed.

| Actual module case | What passed |
| --- | --- |
| `only_a_successful_nonempty_full_submit_consumes_the_observer` | An error is returned unchanged. No submission, zero dimensions, empty damage, short/oversized damage and offset damage do not consume the armed observer. A subsequent complete successful submission invokes it once with the drawable dimensions; duplicate registration, reentrant registration and a second successful submission never invoke it again. |
| `another_renderer_cannot_notify_this_window_or_register_after_its_own_submit` | Each renderer owns a separate one-shot. A completed first submission prevents later registration on that renderer without consuming another renderer's callback. |

Code inspection connects these tests to the Winit call site: no drawable or no damage returns `None`; `Some(SubmittedDamage)` is constructed only after `present_with_damage` returns successfully. The externally callable closure runs after `render_and_present` returns and its surface borrow ends. The tests inject return values into the actual receipt module, not a fault into X11, the compositor or the display driver. A real rendering error may still propagate and terminate the application; retaining the observer is not a guarantee of application recovery from the error.

## Application: timer expiry and worker admission

The release binary test suite for the exact research application passed **19/19** on `test-env`. `trace_tests::desktop_post_submit_is_wake_only_and_fails_closed_for_each_guard` exercises the very predicate used immediately before the optional call. An eligible wake is allowed; observe mode never drives. Starting from the eligible state, setting each condition alone to its invalid value rejects the wake: workers already started, weak window missing, bootstrap complete, suspended, quitting, cancelled, driver unavailable, or non-UI thread. These are predicate tests, not eight induced X11 races.

The same suite passed the existing worker gate race and first-UI-turn yield tests, plus disconnected signal, cancellation, retirement and failed metadata cases. The guard, admission race and original yield were each run separately as well; those three focused passes are already included in the 19. At the call site the live weak handles, bootstrap property, shutdown/suspend/cancellation state, owner thread and nonblocking driver borrow are read when the timer expires. The normal driver is called only if the resulting guard allows it, without a retry queue. There was no live forced quit, driver borrow conflict, non-UI timer or destroyed-window callback in this study.

## Real suspended wake turn

One non-timed run of the research release in `wake` mode exercised a suspended 16 ms callback on `test-env`. The [observer](suspend_turn.py) used a fresh PID, mount and loopback-only network namespace, Xvfb, and an empty ordinary offline guest profile. Only the research child, checked via `/proc/PID/exe` and a pinned pidfd, received signals. The observer waited for the actual `desktop_software_full_submit` before sending `SIGUSR1`; it sent `SIGUSR2` only after observing the pending post-submit callback. It did not alter a callback, timer, renderer, or frozen A/B sample. There was one attempt, no replacement, no forced termination, and no Kindle access.

| Event | Time after software submit |
| --- | ---: |
| The application acknowledged suspend while still booting | 3.624 ms |
| The post-submit callback observed `suspended=true` and `will_drive=false` | 15.956 ms |
| The application acknowledged resume while still booting | 16.546 ms |
| Workers admitted once, through `runtime_wakeup` | 16.577 ms |
| Software `ui_ready` | 30.974 ms |

At the callback, the window and driver were available, the UI was booting on its owner thread, workers had not started, and neither cancellation nor quitting was active. Thus suspension, not a second guard or observe mode, rejected the wake. Each worker was queued exactly once after resume, the same PID reached Shelf and exited normally (status 0). The [scoped receipt](suspend-turn-receipt.json) preserves the monotonic event ordering. The isolated Xvfb, copied binary, empty-profile database and temporary files were removed. This guest Shelf is **not** a substitute for the ordinary, populated visual reference or a device sleep/wake test. Intervals describe only this controlled attempt, not typical startup latency.

In the separate ordinary populated desktop comparison, all seven natural `wake` cases, including the excluded warmup, allowed the callback and crossed the `desktop_startup_fallback` gate. None supplied an additional real late-worker or suspended rejection. The injected-signal case is not one of those fourteen launches.

## An unavailable observer

The [stock executable's populated Shelf qualification](stock-diagnostics.json) observed its actual loading title and initial activity before worker admission. The public rendering notifier was unavailable, neither research event occurred, and the original `maintenance_timer` both entered the driver and admitted the workers. This is a real stock fallback, not an induced failed registration in research mode. The [research application patch](app-submit-probe.patch) treats rejected registration as an optional diagnostic and proceeds to the unchanged event loop and timer. No live duplicate/late registration or destruction of an X11 window during the pending timer was forced.
