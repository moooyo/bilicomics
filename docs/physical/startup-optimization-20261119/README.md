# Startup optimization candidate

**Status: the candidate passed independent build and software verification;
the [six-pair physical comparison](../library-pairs-20261120/README.md) is complete.** See the
[fresh-worker results](../../verification/startup-rebuild-20261120/README.md)
and [session handoff](../../HANDOFF_STARTUP_20261120.md). The previous
[native Library measurement](../library-to-shelf-20261119/README.md) is the
historical baseline, not a control observation for these changes. The measured
benefit was mixed; no permanent installation or release is claimed.

## What changed

- The [launcher](../../../packaging/kindle/launch.sh) starts its existing power
  observer before preparing the log sinks. A timer owned by that observer's
  process group runs in parallel with log setup. Both the full two-second timer
  and successful log setup are required before sampling native presentation or
  taking the display. The evidence does not justify shortening the timer or
  equating its completion with acknowledgement by the native window manager.
- The [application](../../../src/app/startup.rs) can still yield before selecting
  the initial route. When it requests the ordinary Shelf **and actually arrives
  there**, it finishes projecting the Shelf views in the same UI callback. This
  closes the Route-to-Views yield that previously allowed an unchanged loading
  screen to be submitted again. Ready and the normal full-screen update remain
  after successful view preparation. Other routes, including a login redirect
  or resumed reader, retain the previous budget boundary.

## Verification status

The new Rust cases cover route selection and the budget, populated Shelf
content, non-Shelf routes, a login redirect, suspension and projection errors.
The [shell fixtures](../../../packaging/tests/launcher_test.py) cover overlap,
the full time guard, invalid completion, early sleep/USB events, and cleanup
on cancellation or logging failure. The [watchdog tests](../../../scripts/verification/test_physical_harness.py)
also cover an owned empty timer marker left by an interrupted launcher, without
allowing unrelated or nonempty objects to be removed. The [real-reader harness](../../../scripts/verification/launcher_real_reader.py)
also requires the production delay; it must not replace it with a mock.

`test-env` became unreachable over SSH during the original preparation. The
rebuilt worker subsequently passed the ARM, desktop and fixture checks linked
above. No local test was substituted, and the earlier physical and UI
baselines have not been changed. The [predeclared six-pair plan](PLAN.md)
subsequently ran with a freshly qualified native detector, independently
rebuilt control, fixed canonical test entry and new admission. The
[closed evidence report](../library-pairs-20261120/README.md) uses the same
endpoint definition without rebinding the historical runner to different inputs.
