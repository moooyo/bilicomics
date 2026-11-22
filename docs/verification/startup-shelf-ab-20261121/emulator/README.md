# Off-device admission for the Shelf render-order comparison

Both physical arms use **one** ARM executable. A additionally receives `--serial-shelf-render`; B omits it. Both receive `--kindle --shelf-render-trace <private-path>`. Only an actual native `shelf_rendered.data.render_order` record establishes the order that ran. A successful argument parse, a desktop frame or a missing widget is not an observation of that order.

The [verification receipt](results.json) records four accepted ARM flag/profile combinations, four additional invalid combinations rejected before a trace was created, and two shared-path desktop profiles with real, non-cover Shelf pixel matches. All six primary cases used their first attempt. No native render order or speed difference was observed off-device.

`admission.py` reuses the [previously qualified pixel observer](../../startup-interactive-kindle-20261121/emulator/profiles.py) and [X11 sampler](../../startup-cover-worker-20261121/desktop_pairs.py), not any previous application binary or measurements. The original [route seed recipe](../../startup-interactive-kindle-20261121/route/prepare_route.py) and the present build's `assets/demo` supply a single stored `design-demo` preference selecting Shelf and the bundled fixture roster. All cases get new private data directories. There is no `--screen` override, account traffic, injected gesture, or wait for complete cover pixels.

- **ARM user mode:** on each of `scribe` and `pw6`, run both A and B on the *same* completed ARM ELF under `qemu-arm`. Require the actual process-entry lifecycle record, a newly opened, same-process, cleanly closed Shelf diagnostic, and a controlled refusal for the absent native launcher snapshot. Exercise A with each required dependency missing and check that argument parsing rejects it before opening the trace. There is no framebuffer or evdev in this isolated Ubuntu namespace. Neither a native draw nor either physical render order is expected here. No full-system emulator or suitable Kindle firmware images were identified in the rebuilt toolchain and designated local image roots.
- **Ordinary desktop presentation:** the native render-order option does not apply to winit. Run the new host executable **once per geometry**, not once per artificial desktop arm. For Scribe (1860×2480) and Paperwhite 6 (1264×1680), establish that the normal saved offline demo preference opens Shelf. Before exit use `XGetImage` on the PID-verified, viewable window to match the resume title, filter control, and active Shelf navigation against the preserved [RGB references](../../../uiux/baselines/2026-11-17-startup/README.md). Independently rule out the loading reference. Require the same PID in the Shelf lifecycle record and a clean exit. These checks are off-device functional evidence, not native Library latency or electronic-paper evidence.

## Run

Stage `admission.py` in a fresh private `/work/emulator-surface/` with an empty `results/`. Under `oracle/` put `profiles.py`, `desktop_pairs.py`, the original route `prepare_route.py` and `switch.sh`, and the two frozen reference PNGs per model (`<profile>-shelf.png` from `candidate/normal/final-x11.png`, `<profile>-loading.png` from the corresponding `startup-0.png`). The current build's source and demo assets are rooted at `/work`. The route cleanup verifier is imported for synthetic data admission only; its top-level device commands never run.

Wait for the build owner to supply both executable identities. Run in the existing Ubuntu toolchain under a separate loopback-only `unshare --net` namespace. Runtime calls do not require the shared build lock. A new numbered attempt may be used for a documented harness or process failure, not as a replacement timing sample.

```bash
set -eu
umask 077
cd /work/emulator-surface
for profile in scribe pw6; do
    for arm in a b; do
        python3 -B admission.py arm "$profile" "$arm" "$ARM_EXECUTABLE"
    done
done
xvfb-run -a -s '-screen 0 1860x2480x24 -nolisten tcp' python3 -B admission.py desktop scribe shared "$DESKTOP_EXECUTABLE"
xvfb-run -a -s '-screen 0 1264x1680x24 -nolisten tcp' python3 -B admission.py desktop pw6 shared "$DESKTOP_EXECUTABLE"
```
