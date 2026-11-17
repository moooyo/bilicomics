# Native-query software and build evidence: 2026-11-17

This bundle publishes stored `test-env` checks and build evidence for the fresh
native-query comparison. The exporter runs no tests, builds, application, or
device operation. Physical compatibility, optical results, and performance
acceptance remain pending outside this software bundle.

## Selected sources and artifacts

The authoritative runtime pair is **baseline `f50ada1`** and **candidate-r2
`f574b26`**, using separate targets that were absent before each build. Full
revisions, archives, source fingerprints, executable hashes, and selected package
identities are in [software-summary.json](software-summary.json) and the
[original authority record](original/software-identity-r2-summary.json).

| Source | Compiled inputs | Role |
| --- | ---: | --- |
| `f50ada15dd934c986e8305a1f9dd8e46f51b2529` | 542 | Fresh control |
| `74c259338f1cfc234f92a160ccf582dbe292c8d1` | 545 | Rust and C-mock test source; initial package rejected |
| `f574b2692e537f14b54f99f8f0f41b6e4c0885d6` | 545 | Authoritative rebuilt candidate and final package checker |

The two candidate runtime manifests are byte-identical. The Rust suite ran on
`74c2593`; its runtime inputs match `f574b26` exactly in the retained manifests.
The helper also retains the exact same SHA-256 through the ABI review.
Applications built in different roots retain their distinct executable identities.

The common ARM profile is ThinLTO, one codegen unit, and symbol stripping, with
`--no-default-features --features kindle` for packaging. Builds used Rust/Cargo
1.99.1, Zig 0.15.2, Cargo jobs 6, and `CARGO_INCREMENTAL=0`. Rust tests used one
thread. [compiler-profile.json](compiler-profile.json), source manifests, initial
target-absence assertions, and full verbose ARM logs are retained. The summary
locates both application and backend compilation announcements and actual rustc
invocations in each selected fresh build log.

## Completed software checks

| Check | Result |
| --- | --- |
| Rust library, default desktop plus Kindle feature superset | 904 passed; 0 failed; 4 ignored |
| Rust main binary | 11 passed; 0 failed |
| JPEG worker Rust target | 0 tests; no failure |
| C helper protocol mocks | 14 passed |
| Native-query package regressions | 44 passed |
| Existing startup / spatial-dither / native-media package regressions | 8 / 5 / 3 passed |
| Fresh baseline static package checker | 16 checks passed; 42 entries |
| Authoritative candidate-r2 static package checker | 19 checks passed; 46 entries |

The four package regression suites total 60 tests. They are distinct from the
static checker counts. The C tests use controlled host mocks, and the static
checkers did not execute ARM code or verify a device. Original logs preserve
warnings and ignored tests without changing their status.

## Retained package rejection and ABI review

The [initial candidate package record](packages/candidate-r1-rejected.json)
remains **failed**: its direct `libpthread.so.0` requirement had not been reviewed.
The C helper and runtime inputs did not change to make this rejection disappear.
After the symbol audit, candidate-r2 was rebuilt in its own fresh target and
checked under the narrower reviewed contract.

The selected helper requires exactly `libc.so.6`, `libdl.so.2`, and
`libpthread.so.0`. Four ordinary C imports (`__errno_location`, `read`,
`sigaction`, `write`) resolve to the pthread namespace at `GLIBC_2.4`; it imports
no pthread API. The compiler already uses `--as-needed`, and the isolated
[comparison](abi/as-needed-comparison.json) shows that adding it does not change
the helper. This is an actual symbol dependency, not an unused library entry.
The final checker binds the selected helper bytes, exact dependencies, four
reviewed symbols, loader, and hardening conditions. Other application and JPEG
worker binaries remain subject to the static executable rules. See the
[link ABI audit](../../research/aidoku-native-query-20261117/LINK_ABI_AUDIT.md).
These static conditions do not establish firmware support.

## Cache audit and older results

The two notes in `cache-audit/` are copies of read-only audit narratives.
No separate audit JSON receipt was recorded. They found no evidence of
Aidoku's specific diagnostic-backend contamination in the older
published BiliComics packages. Different absolute source paths did not isolate
the backend Cargo identity. Equal compiled-input manifests across build roots
also do not establish equal final binaries: generated files and executable
strings contain build paths, while whole-program path-normalized equivalence
was not proved. The old results retain their original executable identities and
scope; the fresh control is a new, separately identified package. The root task
performed the new fresh builds before or outside the authors' read-only audits.

## Publication integrity

[export-manifest.json](export-manifest.json) maps every exported record to its
origin, byte count, SHA-256, and any aggregation. Its own digest is in
[export-manifest.sha256](export-manifest.sha256). Original logs stored as `.txt`
retain their bytes. The fixed allowlist excludes ELF and ZIP bytes, databases,
credentials, private transport, account content, and physical-run output.
