# Physical Library-parent A/B

This is a launcher experiment, not a second render-order experiment. The same retained 74,392,336-byte instrumented ARM executable, worker, detector, route helper, synthetic database and `--demo --offline` arguments are used throughout. The ordinary native Library card activates the isolated application. The stage for A runs the shipping launcher's repeated input checks; B skips them only in the waiting `--library` parent. The independent worker still checks the inputs. Neither arm passes `--serial-shelf-render`. The retained ELF predates the render-order field; its absence is not disqualifying and supplies no evidence about the actual draw order.

## What binds the arm

- The control launcher and document must match the same-named members of the retained installation ZIP, byte for byte. B must exactly equal the reviewed two-location edit of those verified control bytes. The shorter-lived descriptor prototype and any unrelated changes are rejected.
- The existing private `shelf_operator.py` prepackages 14 single-use visits: excluded A/B warmups, then six measured pairs in order AB, BA, AB, BA, AB, BA. The arm receipt binds the assignment, run, application size and default render policy. A receipt, a filename or equal file sizes alone do not establish the treatment.
- Before acquisition, `check` and `start` independently reconstruct the expected arm and compare every uncompressed route member, the probe pack and the identical process guard to their inputs. During each leased activation the original switch checks the real production launcher/document and protected installation as before; the switch's exact-byte verifier additionally checks the **entire** assigned diagnostic launcher before parking the original. It never accepts the candidate as the stock comparison source.
- The run-bound shim, source input gates, native app/PID proof, original guard, stock restoration and exact owner/token release use the existing [unattended controller](../startup-interactive-kindle-20261121/OPERATOR.md). The same `CLOCK_MONOTONIC` card release and first normal, non-cover-gated Shelf render are required. SEND and same-marker completion are optional, separate endpoints, not optical evidence. The controller checks the original anonymous projection again after release, never comparing times between arms while choosing whether to continue.

## Off-device preparation

The physical series is closed. For an offline reproduction on `test-env`, the checked-in `packaging/kindle/launch.sh` is A, not B. The `materialize` command reconstructs B only at a new explicitly named path and compares the exact edit to [the independent frozen diff](worker-only.patch). It does not contact the device or use a lease. The [control's lifecycle fixture](../../../packaging/tests/launcher_test.py), the [candidate fixture](test_candidate_launcher.py) and the route-verification tests can run in the private Ubuntu workspace. The native series retains the stronger requirement that the actual stock input match the launcher member of the retained installation archive.

```sh
w=/var/tmp/bilicomics-startup-next-oGmoCt3G
v=docs/verification/startup-library-admission-20261121
stock=/var/tmp/bilicomics-shelf-render-20261121.tIiP8C4F/candidate/app/packaging/kindle
paired=/var/tmp/bilicomics-click-20261120.ZP9wXtaX/detector
rollout=/var/tmp/bilicomics-rollout-20261121.Z5EH6RUU
mkdir -m 700 "$w/research-candidate"
python3 -IB "$w/$v/native_series.py" materialize --stock "$stock/launch.sh" --output "$w/research-candidate/launch.sh"
/opt/kindle-toolchain/ubuntu-run "$w" unshare --net python3 -IB "/work/$v/test_candidate_launcher.py" -v
/opt/kindle-toolchain/ubuntu-run "$w" unshare --net python3 -IB "/work/$v/test_native_series.py" -v
python3 -IB "$w/$v/native_series.py" prepare --output "$w/native" \
  --application "$w/arm/bilicomics" --worker "$w/arm/kindle-jpeg-worker" --route-fs "$w/arm/route-fs" \
  --shipping-launcher "$stock/launch.sh" --candidate-launcher "$w/research-candidate/launch.sh" \
  --document-launcher "$stock/document-launcher.sh" --native-kit "$paired/paired-kit.tar.gz" --rollout-root "$rollout"
python3 -IB "$w/$v/native_series.py" check --plan "$w/native/plan.json"
```

`--output` must be a new private directory. Retain the exact input files, including the binary, through the before-start archive checks. These commands do not acquire a Kindle lease. Keep the original owner, host boot and plan; do not set an unrelated conversation's identity. Do not allocate device time until all inputs, preflights and recovery are ready.

## Start, handoff and return

Run `python3 -IB "$w/$v/native_series.py" start --plan "$w/native/plan.json"` in the main operator's private foreground PTY. The single no-echo credential never enters arguments or a file. Every visit uses a fresh, nonblocking, exact lease; a busy or unsuccessful visit stops the series, without retry or replacement. A completed visit naturally exits, restores the installed launcher and native controls, removes its own stages, closes its connections and releases its exact lease before validation of the next visit.

For a requested safe handoff, the main operator may create the host-only `$w/native/hold-between-visits` marker before the current visit finishes. It takes effect **only between visits, after verified release**, not during device operation. Wait for the runner's `Off-lease handoff before visit ...` message before telling a peer to apply. Removing that exact marker after the peer reports its return lets the next visit attempt its normal fresh lease without reservation. There is no polling of shared lease state or stopping a process while it owns the device. Do not repeat `start`. On recovery-required use only `shelf_operator.py recover --batch <original-last-batch>` with its original owner and exact lease; do not start another visit.

Once the controller has exited and any necessary recovery has confirmed release, `python3 -IB "$w/$v/native_series.py" discard --plan "$w/native/plan.json"` removes only whole, never-attempted, correctly bound host batches whose roster contains nothing unexpected. Any attempted batch stays intact. This operation neither connects to the device nor reads or modifies its lease. Retain the plan and the anonymous closure/timing evidence for independent analysis; do not retain or publish credentials, generic application logs or screen contents.
