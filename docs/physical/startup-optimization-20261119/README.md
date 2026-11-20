# Startup optimization candidate

**Status: candidate source is in Git; build, regression, and device qualification are pending.** Start with the
[2026-11-20 session handoff](../../HANDOFF_STARTUP_20261120.md). The previous
[native Library measurement](../library-to-shelf-20261119/README.md) is the
baseline, not a result for these changes. No candidate speedup, install or
release is claimed here.

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

`test-env` became unreachable over SSH during preparation. The candidate tests,
ARM build, and fresh Kindle A/B have **not** been verified. No local test was
substituted, no device lease was acquired for this candidate, and the earlier
physical and UI baselines have not been changed. Resume with remote regressions
and package identity checks, then follow the [predeclared six-pair plan](PLAN.md).
Only a completed and restored batch is eligible for the [paired report](project_pairs.py).
