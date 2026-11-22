# No consistent end-to-end Shelf improvement in the host launcher surrogate

The final candidate did not consistently display the Shelf earlier. We ran the [predeclared schedule](PROTOCOL.md) on `test-env` after the launcher regression suite and mechanism benchmark had completed. All four excluded qualification runs reached the actual desktop Shelf through the fixture Library parent and separate worker on Scribe (1860×2480) and Paperwhite 6 (1264×1680), with normal exit and simulated service restoration. The timed series ran from 10:06:52 to 10:07:17 UTC on 2026-11-21.

| Scribe measure | Result |
| --- | --- |
| Arm A, median host dispatch to first matching Shelf pixels | 840.014–846.815 ms |
| Arm B, median host dispatch to first matching Shelf pixels | 850.845–858.973 ms |
| Within-pair B minus A, median across six pairs | **−11.201 to +3.256 ms** |
| Median in the three AB pairs | −13.514 to +1.605 ms |
| Median in the three BA pairs | +14.646 to +30.871 ms |
| Strictly earlier / strictly later / intervals overlap | **2 / 2 / 2** |
| Secondary: within-pair B minus A at Rust process entry | −5.055 to −3.766 ms |

All twelve timed runs visibly matched the independent resume-title, filter and active Shelf-navigation references, with no input or cover requirement. A brief improvement at process entry does not establish a gain at the Shelf. The per-arm medians and the median of paired differences summarize different data and should not be subtracted in place of pairing. The 5 ms poll interval, synchronous X11 capture and host scheduling affect the brackets. Every case is retained in the [per-pair receipt](results.json).

**Environment boundaries:** the real shell sources are 80,185 bytes (A) and 80,710 bytes (B). A shared debug desktop ELF (251,368,888 bytes) ran the naturally selected offline demo Shelf after the launcher worker invoked the app. The Linux fixture supplied the compatibility environment, native helpers and mocked services; the main app adapter selects winit, not the Kindle framebuffer. Independently, QEMU ARM 8.2.2 executed the unchanged 74,392,336-byte `bilicomics 0.1.25` for `scribe` and `pw6`. Each produced `process_enter` and rejected native startup without an owned launcher snapshot (exit 1; no presentation). ARM user mode did not measure either shell's end-to-end performance. No case used a native Library card, Kindle driver, electronic-paper panel or live account.

## Reproduce

Place `launcher_surface.py` and `summarize.py` in `/work/launcher-surface`, create `results/`, and stage [`profiles.py`](../../startup-interactive-kindle-20261121/emulator/profiles.py), [`desktop_pairs.py`](../../startup-cover-worker-20261121/desktop_pairs.py) and their independent per-profile `*-shelf.png` / `*-loading.png` references in `oracle/`. Use the frozen `/work/control/launch.sh` (A), the [materialized research](../NATIVE.md#off-device-preparation) `/work/research-candidate/launch.sh` (B) and `/work/target/debug/bilicomics`, with the `/work/packaging/tests/launcher_test.py` framework. The default packaging launcher is A. Run in the private Ubuntu workspace with an outer loopback-only network namespace; no build lock is required. Each invocation starts a new Xvfb and the Python fixture retires its own temporary app/data directory.

```bash
set -eu
umask 077
cd /work/launcher-surface
for number in 1 2 3 4; do
    if [ "$number" -le 2 ]; then size=1860x2480; else size=1264x1680; fi
    xvfb-run -a -s "-screen 0 ${size}x24 -nolisten tcp" python3 -B launcher_surface.py qualification "$number" /work/target/debug/bilicomics r1
done
for number in $(seq 1 12); do
    xvfb-run -a -s '-screen 0 1860x2480x24 -nolisten tcp' python3 -B launcher_surface.py formal "$number" /work/target/debug/bilicomics r1
done
python3 -B summarize.py results/r1 results/pairs.json
```
