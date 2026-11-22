# Fixed native-card visit series

Prepare two excluded warmups (A, B), then six fixed pairs: AB, BA, AB, BA, AB, BA. A opts into `--serial-shelf-render`; B uses the default. One freshly built static ARM application, one worker, one detector, the qualified shipping launcher/card and one generated `design-demo` Shelf seed are used throughout. The seed contains only the ordinary launch preference. The preparer repacks that same file into each visit, never an application's mutated database. No real account, production data copy, `--screen` override, in-app tap or manual input is part of the measurement. The isolated launcher is diagnostic, not the untouched production binary or an optical measurement.

## Prepare offline

Run the [ordinary controller gates](../startup-interactive-kindle-20261121/OPERATOR.md) and this suite in a private Linux workspace on `test-env`. The route fixture needs a native C compiler; the isolated Ubuntu toolchain can run it. Do not call `start` or acquire a lease for preparation. The actual host `prepare` command requires the verified main conversation identity, root-owned inputs and the existing paired detector archive plus its adjacent receipt. Use the trusted source tree (including `assets/demo`), not a directory of detached scripts. All input paths are absolute; `--output` must not exist.

```sh
study="$workspace/docs/verification/startup-shelf-ab-20261121"
python3 -IB "$study/test_series.py"
python3 -IB "$study/series.py" prepare --output "$scratch/ab" \
  --application "$candidate" --worker "$worker" --route-fs "$route_fs" \
  --shipping-launcher "$known_installed_launch_source" \
  --document-launcher "$known_installed_document_source" \
  --native-kit "$paired_archive" --rollout-root "$rollout_tools"
```

The host makes one temporary expanded kit. Only the run binding, shim, launcher and corresponding exact argv verifier vary between packed visits. The comparison arm's private `native-guard.sh` differs by the same single argv token in its NUL-delimited process comparison; recovery uses that original guard. Each visit gets an independently prepared controller batch and never shares an owner token, device stage or mutable seed with a peer. The expanded template is removed before the plan is published. Budget space for fourteen compressed batches: they can be transferred and recovered independently. Neither the Kindle nor its lease is inspected during preparation.

## Execute once

After all build, emulator, route and controller checks have passed and a device operation can start immediately, run on the **host**, not inside Ubuntu. Use a private foreground PTY with tracing off. The authorized coordinator supplies the device credential once at the no-echo prompt; the runner retains it in memory only, without asking the Kindle user to take any action. Do not put it in a command, argument, transcript or plan.

```sh
python3 -IB "$study/series.py" start --plan "$scratch/ab/plan.json"
```

The runner starts each original single-visit controller immediately after the previous one is closed. Each admission is nonblocking. Each successful visit restores the shipping launcher and original native navigation/pointer, removes owned route/detector stages, closes connections and verifies release of its exact owner/token **before** checking its observation and attempting the next visit. The detector and application are uploaded again per visit specifically to avoid leaving either resident under a different lease. No optimization can be inferred from upload, cleanup, warmup or previous historical timings. The runtime never edits source or switches a live run's arm.

The first busy lease, incomplete acquisition, recovery issue, missing primary observation or protocol failure stops the sequence; there is no waiting, retry, replacement or new arm chosen from results. Preserve the completed prefix and all missing cases. `plan.json` records `pair: 0` for warmups and pairs `1`–`6` for the six timed pairs, the prescribed arm and its private run/batch. After release the runner requires exactly one `ready` or `empty` Shelf render for this seed. `initial_fill`, a signed-out Shelf or a missing order stops the series. A must record `wait_then_render`; B may record `render_then_wait` or fall back to `wait_then_render`. That fallback is not a private-first observation. Do not treat SEND or driver completion as optical or use either to replace the primary renderer return.

If the controller returns recovery-required, continue only the last printed **original** private batch using `shelf_operator.py recover --batch ...` and its saved guard, under the exact original lease. Do not restart the series, append a case, swap a token, infer restoration from an idle snapshot, or release for another owner. Retain the plan and anonymous case projections. Once the series is closed and no recovery is outstanding, any never-attempted host batches contain only unused prepacked material; the operator can discard them after verifying their individual plan identifiers. Do not touch the live lock or any other batch for this cleanup.
