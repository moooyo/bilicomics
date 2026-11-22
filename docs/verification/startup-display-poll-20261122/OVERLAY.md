# Engineering overlay

Generate the four-file Rust overlay from the stock source into a new directory
outside that source. The generator reads the preceding study's
`stage_overlay.py` and `capture_cost.rs` from the adjacent directory; it never
edits them or layers on top of an already modified product source. It verifies
the expected source anchors before writing.

```sh
python3 -B "$STUDY/stage_overlay.py" "$BASE" "$NEW_OVERLAY"
```

Only the same authorized startup capture can use
`BILICOMICS_DISPLAY_POLL_RUN=<run>` and create `data/<run>.display-poll.json`,
with schema `kindle-startup-display-poll-v1`. The preceding selector and receipt
are not used. `command_poll_stdout_open` and `command_poll_after_eof` partition
`command_poll` by the actual descriptor passed into the call. The observer
uses the parent's one clock pair and adds the resulting interval to just one
child. Counts and nanoseconds add exactly; there is no additional syscall,
retry, status observation or change to the timeout. The second bucket does not
identify post-exit detection time. The original command residual subtracts the
parent only, never both the parent and its two children.

The overlay fixture compares the old and split recorders with the original
shell read guard and real Linux process authorization. Its controlled getters
exercise an open pipe, a delayed exit after EOF, nonzero exit, timeout and
permit withdrawal; it also checks inactive power and the other study's selector.
Both successes use the actual shell record filter and Python decoder. The
temporary `/mnt` is private to the toolchain namespace, with no hardware access.

```sh
/opt/kindle-toolchain/ubuntu-run "$PRIVATE_ROOT" unshare --net -- \
    python3 -B /work/base/docs/verification/startup-display-poll-20261122/test_overlay_fixture.py /work/base /work
```

On `test-env`, both synthetic successes of the split recorder passed the exact
partition and demonstrated the expected dominant bucket. Old and split builds
agreed on the six guarded outcomes and did not accept each other's selectors.
The small Rust harness passed 29 recorder, transaction and permission tests,
including exact parent–child accumulation and `errno` preservation. These
checks are separate from full-build, ARM and physical evidence.
