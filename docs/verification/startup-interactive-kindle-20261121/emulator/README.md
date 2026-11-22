# Offline Scribe and Paperwhite 6 compatibility checks

These are pre-device checks of a **newly built** candidate, not native Library timing samples. They do not connect to a Kindle, obtain the shared device lease, use a real account, capture video or inject a tap. Do not convert a host process launch into a native card-release origin.

The [verification receipt](results.json) records five checks: the two ARM startup guards, both desktop profile surfaces, and an independent default-Shelf launch and data inventory. All three live desktop windows matched their non-cover references and exited normally. The revised switch's read-only cleanup verifier accepted the actual default-route tree, including all 36 bundled fixture leaves and no unknown additions.

The [historical firmware preparer](../../../../scripts/verification/firmware_setup.py) requires its reviewed system emulator and immutable first-generation Scribe 5.20.1 base. No usable matching system emulator/base or Paperwhite 6 firmware profile was found in the rebuilt worker's designated toolchain and standard image locations. Do not treat historical guest results as current. The available Ubuntu runtime instead provides ARM user-mode QEMU and Xvfb; these answer different questions:

- **ARM user mode:** run the newly built Kindle ELF under `qemu-arm` for each product profile, checking the actual version, process-entry lifecycle record and controlled rejection of a native launch without an owned native launcher snapshot. The isolated runtime has neither a Kindle framebuffer nor native input. An expected refusal is a successful *guard* check, never a successful native UI launch or a Shelf render.
- **Desktop surfaces:** run the newly built Linux desktop ELF on each profile's private 24-bit Xvfb, with the stock software renderer, isolated demonstration data, `--offline`, `--screen shelf` and 2x scaling. This is intentionally a test of Shelf rendering at both geometries, not an ordinary account launch. Read actual pixels from a viewable application window after checking its PID and physical dimensions. Require three independently frozen, non-cover regions (the resume title, filter control, and active Shelf navigation label/icon) to match the [reference](../../../uiux/baselines/2026-11-17-startup/README.md); all must differ from the older loading reference. Also require an ordinary successful exit and a `ready` lifecycle record on Shelf from the same process. No cover, whole-window equality, or user action is a prerequisite.
- **Automatic route and cleanup:** use a third independent Scribe Xvfb launch with `--demo --offline` and **no** screen override or input. It starts with the [route preparer's](../route/prepare_route.py) actual SQLite seed (`design-demo`, default Shelf destination). It must satisfy the same live pixel checks. After exit, inventory only its synthetic `data/` and run the [shipping switch's](../route/switch.sh) literal `clean_tree` verifier against that private tree and its generated asset roster. This calls the read-only verification branch, not the switch's main program, any device path, or its pruning operations. The desktop application does not reproduce device launcher logs or font staging.

## Reproduce

Stage `profiles.py`, `prepare_route.py`, `switch.sh` and the existing [X11 sampler](../../startup-cover-worker-20261121/desktop_pairs.py) in a private `emulator-surface/` subdirectory of the approved build workspace. The current build source and its bundled demo files are at `/work/app`. The sampler is imported only for `XGetImage`, not its historical schedules. From `docs/uiux/baselines/2026-11-17-startup/raw/{scribe,pw6}/candidate/normal/`, copy `final-x11.png` and `startup-0.png` as `<profile>-shelf.png` and `<profile>-loading.png`, respectively. Only the references are reused. Supply the newly completed ARM and desktop candidate paths from the current build owner; no previous executable is an input.

Run inside the existing `/opt/kindle-toolchain/ubuntu-run` workspace with a separate `unshare --net` namespace containing only loopback. Do not take its build lock for these runtime checks. From `/work/emulator-surface`:

```bash
set -eu
umask 077
python3 -B profiles.py arm scribe "$ARM_CANDIDATE" 1
python3 -B profiles.py arm pw6 "$ARM_CANDIDATE" 1
xvfb-run -a -s '-screen 0 1860x2480x24 -nolisten tcp' python3 -B profiles.py route scribe "$DESKTOP_CANDIDATE" 1
xvfb-run -a -s '-screen 0 1860x2480x24 -nolisten tcp' python3 -B profiles.py desktop scribe "$DESKTOP_CANDIDATE" 1
xvfb-run -a -s '-screen 0 1264x1680x24 -nolisten tcp' python3 -B profiles.py desktop pw6 "$DESKTOP_CANDIDATE" 1
```

Each case creates a new private directory and never overwrites an earlier failed attempt. Publish only bounded counters and outcomes, not application logs, raw frames, state databases or account data. Keep driver, firmware and physical timing fields unavailable unless independently measured in those environments.
