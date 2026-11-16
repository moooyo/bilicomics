# Startup software verification: 2026-11-17

This frozen bundle preserves existing `test-env` software checks. Its export
executes no application, test suite, build, or device operation. It does not
establish startup speed, physical input, panel completion, or live-account
acceptance. Desktop captures and any later physical results have separate scope.

## Source and compiler identity

- Baseline: `06ad90b02edb28db43abeed3031932a91dc2f8df`; 540 compiled inputs in
  [baseline-source-inputs.sha256](baseline-source-inputs.sha256).
- Candidate: `585e571b422e9586f1e53a417cbaf330a9ec473f`; 542 compiled inputs in
  [candidate-source-inputs.sha256](candidate-source-inputs.sha256).
- Both attempts bind these same source manifests. The archive and compiled-input
  digests are retained in [software-summary.json](software-summary.json).
- The common ARM release profile is `lto = "thin"`, `codegen-units = 1`,
  `strip = true`; no release override was used. The recorded target is
  `armv7-unknown-linux-musleabihf`, using `rustc 1.99.1`, `cargo 1.99.1`,
  and Zig `0.15.2`. See [compiler.txt](compiler.txt) and
  [compiler-profile.json](compiler-profile.json).
- Builds used Cargo jobs 6 and `CARGO_INCREMENTAL=0`; Rust tests used one thread.

## Retained initial failure

The first attempt failed: **866 library tests passed, eight failed, and four were
ignored**. All eight failures reported `The lifecycle directory must be owned and
private`; later steps stopped after this library failure. The first runner did
not set a private umask. The second runner explicitly set `umask 077`, after which
the unchanged candidate passed. This was an environment correction; no production
source change was made to bypass the ownership check. The initial result remains
failed in [its original summary](original/checks-r1-summary.json) and
[unaltered log](original/checks-r1-library-and-binary-tests.txt).

## Completed remote software checks

| Check | Result |
| --- | --- |
| Candidate library | 874 passed, 0 failed, 4 ignored |
| Candidate main binary | 8 passed, 0 failed |
| Kindle backend fixtures | 236 passed, 0 failed |
| Launcher lifecycle fixtures | 129 passed |
| Startup notice package regressions | 8 passed |
| Spatial-dither package regressions | 5 passed |
| Native-media package regressions | 3 passed |
| Candidate desktop build and ARM release build steps | Exit status 0 |
| Baseline static package checker | 15 checks passed; 39 archive entries |
| Candidate static package checker | 16 checks passed; 42 archive entries |

The three package regression suites total 16 tests; this differs from the static
checker counts. The baseline ARM build log records a completed release build;
its selected application, worker, and archive identities passed static checking.
The package checkers did not execute either ARM binary or verify a device.
Build warnings and the four ignored library tests remain visible in the logs.

See [the final original summary](original/checks-r2-summary.json),
[baseline-package.json](baseline-package.json), and
[candidate-package.json](candidate-package.json) for exact commands, byte counts,
and source-bound hashes. The backend fixture's production dependency alignment
and source binding are retained in [its summary](original/backend-summary.json).

## Evidence integrity

Files under `original/` retain their source bytes, including logs stored with a
`.txt` extension. [export-manifest.json](export-manifest.json) maps every exported
record to its origin, byte count, SHA-256, and any aggregation; its own digest is
in [export-manifest.sha256](export-manifest.sha256). The exporter reads only a
fixed allowlist. ELF and ZIP bytes, databases, credentials, private transport,
account content, and physical-run output are excluded.
