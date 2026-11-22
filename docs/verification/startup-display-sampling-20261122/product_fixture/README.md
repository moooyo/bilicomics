# Full product capture qualification on test-env

This is a functional gate for the engineering executable, not a startup timing or physical-device visit. Run only in a fresh, root-owned `0700` test-env workspace. `run.sh` accepts that workspace, the separate completed engineering build directory and a fresh `r1`, `r2` or `r3` evidence directory name. It checks the successful full x86-64 `--features kindle --bin bilicomics` build and both guard refusals, then streams its executable into a private tmpfs (no duplicate on the shared root disk). Do not run `inside.sh` on the host or use a Kindle address, lease or real framebuffer.

Prepare `inputs/` with the `.c`, `.sh` and `.py` files in this directory. Copy the archived, byte-matched production launcher as `inputs/stock-launch.sh`. Under `inputs/studies/`, preserve the dated directory layout of `host_bench.py`, `collect_display.py`, their `handoff_collect.py` loader and that loader's `shelf_operator.py` and `log_bounds.py`. Create `reports/` with mode `0700`. Compile only the two hardware-fixture tools inside the same Ubuntu userspace, in `/work/inputs`:

```sh
gcc -O2 -Wall -Wextra -Werror -fPIC -shared -o fb-read.so fb_read.c
gcc -std=c11 -D_POSIX_C_SOURCE=200809L -O2 -Wall -Wextra -Werror -o usleep usleep.c
```

Then, on test-env, outside the jail, run `bash "$workspace/inputs/run.sh" "$workspace" "$engineering_build" r1`. Loopback, a private PID namespace and private mount propagation are mandatory. The product reads the actual kernel process tree, uses the stock permission files and runs the complete command parser and capture code. Only the regular-file framebuffer's two read-only ioctls, the input inventory and the firmware getter answers are simulated. The shell's `usleep` replacement really sleeps for its requested period. No clocks or process identities are replaced.

The Scribe automatic and PW6 fixed positive cases require the expected query order, both framebuffer observations and a fresh restricted product sidecar; the collector reads, decodes and checks its counts and clock nesting, and its process identities must match the live getter ancestry. The independent fourth PID and bounding interval come from the host controller, **not** from an asserted Shelf UI or Library card. The other cases require a normal selector-off success, refusal on withdrawal during the first getter, and refusal when that getter exceeds the original budgets. No failed or unselected case may emit a sidecar, and no owned descendant may survive. No firmware, rendering, physical interaction, device performance or release result is inferred.

Publish only the aggregate `host-product-qualification.json`. The per-case directories and all run-specific identities stay private. A missing build, incomplete case or failed check does not qualify the build. Keep the aggregate before cleaning only the dedicated workspace; the jail's temporary mounts disappear with its process namespace.
