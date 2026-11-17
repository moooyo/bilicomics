# Route-refresh software evidence: 2026-11-17

This independent nested bundle exports only stored `checks-r4` software evidence.
The parent item 1 files and their fixed export manifest remain unchanged.
Exporting this bundle ran no new test, build, application, firmware emulator,
device operation, or lease operation. These records establish no physical,
optical, startup-speed, live-account, or adoption conclusion. Physical and
performance batches retain their own identities and analysis.

## Frozen source and actual builds

The route-refresh candidate is `f6208d6f7dd3d8e64a09ef0ceb2ceb6b0c45d0f5`.
Its source archive SHA-256 is
`146a8f85798b2fce58345307c872c0cb5ce6977ee430dac7e8a255270da6bfb2`.
[source-inputs.sha256](source-inputs.sha256) binds 545 compiled inputs;
[archive-source-inputs.sha256](archive-source-inputs.sha256) binds 1,243 archived
source files. Only this source revision is represented by the new runtime source
snapshots in `original/frozen-source/`.

[The authoritative raw report](original/checks-r4-summary.json) and
[software-summary.json](software-summary.json) retain exact source, compiler,
desktop ELF, ARM ELF, worker, ZIP, and sidecar identities. Binary and archive bytes
are excluded. The selected ARM app SHA-256 is
`3962bc49bb8cd33d21651845fe5de229e1b8458e5d0ba9026485c8bef7224525`.

The ARM target is `armv7-unknown-linux-musleabihf`. The official release profile
is `lto = "thin"`, `codegen-units = 1`, `strip = true`; both actual binary rustc
invocations use `opt-level=3`, `lto=thin`, `codegen-units=1`, and `strip=symbols`.
There was no release override. Rust/Cargo 1.99.1, cargo-zigbuild 0.24.0, Zig
0.15.2, and executable hashes are recorded in
[compiler-profile.json](compiler-profile.json). The complete
[ARM verbose log](original/logs/arm-build-cargo-verbose.txt) and
[compile proof](arm-compilation-proof.json) retain actual application/backend
announcements and rustc invocations.

The target already existed in r4 and was not copied. Its initial absence belongs
to the parent's [checks-r2 record](../original/attempt-r2/initial-target-state.json).
Before the new ARM build, only this target's `bilicomics` and
`slint-backend-kindle` ARM release artifacts were cleaned; dependencies were kept.
Cargo used 6 jobs, no incremental compilation, one Rust test thread, and offline
dependencies. Source paths and mtimes alone are not rebuild evidence.

## New checks and inherited results

| Record | Result and execution scope |
| --- | --- |
| New main library | 897 passed, 0 failed, 4 existing ignored benchmarks |
| New main binary | 19 passed, 0 failed |
| New lib/bin total | 916 passed; all 7 new route-refresh regressions executed successfully |
| New demo/Xvfb startup fixtures | 4 cases passed |
| New desktop and official ARM builds | Exit status 0 |
| New ZIP static checker | 17 checks passed; 45 exact archive entries |
| Inherited backend fixtures | 244 passed; not rerun; 465 dependency versions/checksums aligned |
| Inherited package regressions | 26 passed across 3 modules; not rerun |

The four ignored benchmarks were not enabled. Their exact names and all seven
new regression results are in the raw and derived reports. The eligible JPEG
worker test binary compiled with the main command and reported zero tests.
The new [package checker](package-check.json) independently checked the new ZIP,
selected app/worker identities, notices, manifest, modes, and static ARM ELF
properties; it did not execute ARM code. Its 17 checks differ from the 26
inherited package regression tests. [arm-literals.json](arm-literals.json)
records zero forbidden diagnostic prefixes and the expected implementation
literals, including the isolated `legacy-startup-route-refresh` control.

The inherited results remain bound to item 1 source
`530f1516c332931059c52161b52d64e86488b3f1` and its old 544-input manifest.
See the parent's [backend report](../original/backend/summary.json),
[backend raw logs](../original/backend/logs/), and
[package regression log](../original/attempt-r3/logs/package-regressions.txt).
The new [scope bindings](inherited-evidence-bindings.json) prove equal relative
file sets, sizes, and hashes for 86 backend inputs and a safe superset of 37
package/producer inputs. `build-kindle.sh` is a producer contract in that scope;
the 26 unit tests do not execute it. Original commands, working directories,
logs, and source identities retain their old origins. No old full application
manifest was rebound to the new source.

[scope-audit-derived.json](scope-audit-derived.json) is explicitly a derived
static comparison, not an original runtime receipt. It compares 246 existing
scoped source files without differences. Archive-source differences are only
four changed product files and the added `startup_refresh_tests.rs`; no removed
file or inherited-scope difference was found.

## Desktop fixture and export boundaries

The [startup fixture directory](fixtures/startup) contains an enumerated set of
capture JSON, result JSON, 12 PNGs, and process output. These isolated demo/Xvfb
captures use PW5 618x824 and Scribe 930x1240 pixels. They are not physical-device
acceptance or a hardware-support claim. `runpy` executes the unchanged helper's
two original normal cases, then the extension calls the same
`execute(..., fail=True)` once for each profile. The original two-case
[results.json](fixtures/startup/results.json) remains byte-exact and separate
from the four-case [boundary-results.json](fixtures/startup/boundary-results.json).

[export-manifest.json](export-manifest.json) maps every exported record to its
origin, size, SHA-256, and processing method; its own digest is in
[export-manifest.sha256](export-manifest.sha256). Raw logs, reports, source
snapshots, and PNGs retain their original bytes; `.log` to `.txt` changes only
the filename. Inherited log copies explicitly identify their r3 execution
origins. Only the fixed allowlist is copied. No fixture data directory, ELF,
ZIP, tar, SQLite, account, authentication, session, lease token, or physical-run
material is exported.
