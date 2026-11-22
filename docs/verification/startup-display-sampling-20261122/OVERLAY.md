# Engineering source overlay

This diagnostic changes only a fresh private compilation snapshot. Apply
`stage_overlay.py` to the stock source and a nonexistent output directory
outside that source. The generator checks exact source anchors, including their
expected occurrence counts. It writes four source files, under their normal
relative paths; it never edits the input, Cargo features, or the launcher.
Compare and overlay those files by bytes, not by source hash.

```sh
python3 -B "$STUDY/stage_overlay.py" "$BASE" "$NEW_OVERLAY"
```

The staged launcher binds `BILICOMICS_DISPLAY_COST_RUN` only around its normal
startup `capture_native_epdc_mode` call. Rust also requires the stock permit's
startup scope, a canonical `bili-shelf-<32 lowercase hex>` run, and the already
validated helper executable at `/mnt/us/<run>/app/bilicomics` matching the
launcher's staged home. A valid resume permit cannot qualify, even with the
variable present. Identity is copied from the authenticated permit snapshot;
there are no repeated or new permit or process reads. Only the existing
`Writer::process` entrances are counted.

The start anchor is in the helper after the original deadline starts, just
before validating launch approval. The finish anchor is after successful
sampling and before preparing the receipt. Clock reads do not redefine native
deadlines and preserve `errno` across the observed `poll` call. Aggregates
remain in memory until the successful startup result. Only then does the
helper read the current boot ID and attempt one exclusive, no-follow file
creation in the owned staged `data` directory. Logging errors do not alter the
original stdout, return status or fallback. A partial or missing file supplies
no diagnostic result; there is no mid-transaction file output.

The [physical plan](PHYSICAL_PLAN.md#interpretation-and-replay) defines the
inclusive buckets and their residuals. In particular, `command_poll` is wall
time, including the original finite poll slices and any scheduling time. It can
also include a poll with no open stdout descriptor while a child is still
pending; it does not measure the LIPC service's CPU. `process_stat_reads` is a
count of calls, not a syscall count or an independently measured duration.

On `test-env`, the small source harness uses the real overlay, stock guarded
shell transaction, existing `NativeReadPermit` and real `/proc`, with synthetic
hardware responses. Use a private toolchain and network namespace; the harness
mounts a temporary `/mnt` only inside that namespace. The second parameter is its
private writable working directory.

```sh
/opt/kindle-toolchain/ubuntu-run "$PRIVATE_ROOT" unshare --net -- \
    python3 -B /work/base/docs/verification/startup-display-sampling-20261122/test_overlay_fixture.py /work/base /work
```

The host harness verified deterministic generation and refusal to overwrite the
base or overlay. Automatic and fixed synthetic reads yielded their unchanged
stdout and a Rust record accepted by the real shell allowlist and Python
decoder. Disabled, valid resume, inactive and rejected reads yielded no
sidecar. Native transaction, permit and recorder unit tests also passed in a
small host harness. These checks are not the full application build, ARM user
mode admission or physical timing, which require separate custody and results.
