# Startup-sequence item 1 software evidence: 2026-11-17

This bundle exports already completed `test-env` software checks through a fixed
allowlist. Exporting it ran no new test, build, application, firmware emulator,
device operation, or lease operation. It does not establish startup speed,
physical input readiness, panel completion, optical quality, hardware support,
or live-account acceptance. Any physical or performance batch has separate
records and conclusions.

## Source and build identity

The frozen candidate is `530f1516c332931059c52161b52d64e86488b3f1`.
Its source archive SHA-256 is
`5bf6950ebf72f38acb00cb1061ce165d54c117b934849dc4065954cda72e0576`.
[source-inputs.sha256](source-inputs.sha256) binds 544 compiled inputs;
[archive-source-inputs.sha256](archive-source-inputs.sha256) binds all 1,242
archived source files, including build scripts and packaging inputs. Their exact
identities and the selected desktop ELF, ARM ELF, JPEG worker, ZIP, and package
manifest identities are retained in [software-summary.json](software-summary.json).
Executable and archive bytes are excluded from this export.

The ARM target is `armv7-unknown-linux-musleabihf`. The official release profile
is ThinLTO (`lto = "thin"`), `codegen-units = 1`, and `strip = true`; the actual
application and worker rustc commands use `opt-level=3`, `lto=thin`,
`codegen-units=1`, and `strip=symbols`. No release override was used. The recorded
tools are Rust/Cargo 1.99.1, cargo-zigbuild 0.24.0, and Zig 0.15.2; tool executable
hashes are in [compiler-profile.json](compiler-profile.json).

[arm-compilation-proof.json](arm-compilation-proof.json) retains the real app and
backend compile announcements plus actual rustc invocations. Its complete
[verbose Cargo log](original/attempt-r3/logs/arm-build-cargo-verbose.txt) is
unchanged. The target was initially absent in checks-r2; checks-r3 continued that
owned target after the main test command. Before ARM compilation, only
`bilicomics` and `slint-backend-kindle` in this run's ARM release target were
cleaned. Dependencies were retained. All build jobs were limited to 6,
incremental compilation was disabled, Rust tests used one thread, and Cargo
ran offline. Distinct paths or mtimes alone are not used to establish a rebuild.

## Stored results

| Check | Result |
| --- | --- |
| Main library | 891 passed, 0 failed, 4 existing ignored benchmarks |
| Main binary | 18 passed, 0 failed |
| Kindle JPEG worker test binary | 0 tests; compiled by the selected lib/bin command |
| Main lib/bin total | 909 passed, 0 failed |
| Complete isolated backend | 244 passed, 0 failed, 0 ignored |
| Backend dependency alignment | 465 packages; all versions and checksums match production |
| PW5/Scribe startup normal and controlled error fixtures | 4 cases passed |
| Three package regression modules | 26 tests passed |
| Static ARM package checker | 17 checks passed; 45 exact archive entries |
| Desktop build and official ARM release build | Exit status 0 |

The 26 regression tests and 17 static checks are separate counts.
[package-check.json](package-check.json) verifies the selected app and worker
bytes in the ZIP, notices, manifest, executable modes, and static ARM ELF
properties; it did not execute ARM code. [arm-literals.json](arm-literals.json)
records zero forbidden diagnostic prefixes and the expected implementation
literals. Original warnings remain visible in the logs.

## Preserved attempts and continuation

- [checks-r1](original/checks-r1-summary.json) remains failed. The unsupported
  `cargo zigbuild --version` metadata query exited with 2 before tests or builds.
- [checks-r2](original/checks-r2-summary.json) remains failed. Its main test command
  returned 0 with 891 library and 18 main-binary tests passed, but the runner
  incorrectly rejected four existing ignored benchmarks.
- [checks-r3](original/checks-r3-summary.json) is the authoritative passed report.
  It retained that completed suite after matching the frozen commit, archive,
  compiled-input manifest, compiler identities, and original log hash. The main
  suite was not repeated; the remaining stages ran normally. The derived summary
  also compares the stored command, working directory, and key environment
  fields. Raw failed and passed reports are never rewritten.

The four ignored benchmarks were never enabled:
`app::perf05_benchmark::perf05_controlled_host`,
`reader::profile_tests::measure_reader_stage_candidates`,
`reader::profile_tests::verify_full_frame_cancellable_production_paths`, and
`reader::triangle::tests::exhaustive_rounding_core_matches_the_original_float_conversion`.

## Desktop fixture scope and raw bytes

The [startup fixture directory](fixtures/startup) contains only an enumerated set
of screenshots, capture JSON, result JSON, and process output. These are isolated
demo/Xvfb desktop captures: PW5 uses 618x824 pixels and Scribe uses 930x1240 pixels.
Those fixture geometries are not physical-device acceptance or a support claim.
The unchanged startup helper ran its two original normal cases through `runpy`;
the extension then called that same helper's `execute(..., fail=True)` once for
each profile. The original two-normal-case [results.json](fixtures/startup/results.json)
is retained unchanged. The independent four-case
[boundary-results.json](fixtures/startup/boundary-results.json) includes the two
controlled error cases. Producer, launcher, extension, exporter, and executed
helper source snapshots are in [original/helpers](original/helpers).

[export-manifest.json](export-manifest.json) maps every exported record to its
origin, byte count, SHA-256, and processing method. Raw records preserve exact
bytes; `.log` to `.txt` changes only the destination filename. The manifest's own
digest is in [export-manifest.sha256](export-manifest.sha256). No source tree or
fixture data directory was recursively exported. ELF, ZIP, tar, SQLite, account,
authentication, session, lease-token, and physical-run material are excluded.
