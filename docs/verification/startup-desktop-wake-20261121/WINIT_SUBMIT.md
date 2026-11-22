# Research-only Winit software submit observer

The [patch](winit-submit.patch) applies to the crate root of **`i-slint-backend-winit` 1.17.1** from the application's locked Cargo registry. Use the same private patched crate and enable `slint/unstable-winit-030` for **both** A-observe and B-wake. The stock A0, production manifests, vendor tree, shared registry, Kindle backend and deployed binaries are not inputs to modify. Preserve the upstream crate's manifest, notices and `LICENSES/` when copying it.

## Reproduce on the worker

Inside the isolated Linux verification workspace, set `STUDY` to this study's source directory and `PRIVATE_VENDOR` to a directory in that workspace. The following commands require the pinned toolchain's `CARGO_HOME` and a fresh destination:

```sh
set -eu
umask 077
crate=i-slint-backend-winit-1.17.1
registry_crate="$CARGO_HOME/registry/src/index.crates.io-1949cf8c6b5b557f/$crate"
mkdir -p "$PRIVATE_VENDOR"
test ! -e "$PRIVATE_VENDOR/$crate"
cp -a "$registry_crate" "$PRIVATE_VENDOR/$crate"
cd "$PRIVATE_VENDOR/$crate"
patch --batch --forward --dry-run -p1 --fuzz=0 < "$STUDY/winit-submit.patch"
patch --batch --forward -p1 --fuzz=0 < "$STUDY/winit-submit.patch"
rustc --edition 2024 --test renderer/sw_submit.rs -o "$PRIVATE_VENDOR/submit-guards"
"$PRIVATE_VENDOR/submit-guards"
```

Point both research builds' private Cargo `[patch.crates-io]` entry for `i-slint-backend-winit` at this copy. The standalone command runs the two unit tests of the exact state and geometry module invoked by `renderer/sw.rs`. It tests rejected and accepted results, origin, extent, per-renderer isolation, duplicate and late registration, and one-shot delivery. It is not a full framework test, an injected X11 surface failure, or a substitute for application and pixel qualification.

## Receipt semantics

`slint::winit_030::WinitWindowAccessor::on_first_full_software_presented` accepts `impl FnOnce(u32, u32) + 'static` and returns whether this window's software renderer accepted the observer. Register before its first qualifying submission; an unsupported backend, duplicate registration or already-submitted full buffer returns `false`. Registration does not need a native window yet and never requests a redraw. One window cannot satisfy another window's observer.

`render_and_present` returns a damage record only after `softbuffer::Buffer::present_with_damage` has returned `Ok`. By then the target buffer is consumed; returning from the helper also releases the mutable surface borrow before delivery. Only a **nonempty damage bounding rectangle starting at `(0, 0)` and equal to the full physical drawable size** qualifies. A skipped draw, absent surface, zero size, empty or partial damage, or any error does not consume the observer. The callback receives the two physical dimensions, not pixels or a graphics API handle. The observer is already closed and unborrowed when the callback runs; the wake arm may enqueue a later event-loop turn, never run the startup driver synchronously inside rendering.

This establishes a successful software buffer submission, not which scene it contains, the first externally visible frame, an X server acknowledgement, compositor completion, or panel timing. Apply the study's independent [window pixel and worker admission checks](PROTOCOL.md) to those separate questions.
