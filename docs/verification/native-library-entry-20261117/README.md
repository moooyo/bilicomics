# Native Library entry software evidence: 2026-11-17

This fixed-allowlist bundle preserves completed `test-env` Linux fixture and
static-package results. Exporting it ran no new tests, builds, application,
runtime probes, remote commands, device operations, or lease operations. It does
not establish native Library behavior, physical acceptance, hardware support,
startup speed, or reading performance. Physical and performance evidence remain
separate.

The entry-script and package candidate is
`39fac822a982868078201e605e50bdacf1dcf3ed`. Its archive SHA-256 is
`5c513d92e48cbcdcbf408dbc02a80a6d9678455e4e06c95a62dd63523736b4b5`.
The stored PAX commit matches; all 1,782 extracted files matched the archive.
[software-summary.json](software-summary.json) links the raw source identities.
The canonical manifest binds 544 binary inputs and is byte-identical to the old
build manifest, SHA-256
`8537123d6b1dc0c75a048b09d1204664988a6c1668ea35d721bfbac641ff49c6`.
The `run_suite` serialization has SHA-256
`5d7d60c1164ceffe2b6a7339990cbb49b408d134aa0c8afc62f1367fdfec461b`;
its path/digest mapping is identical. The retained
[ordering diagnosis](original/manifest-order-diagnosis-r1.json) records no added,
missing, or changed inputs.

The application and JPEG worker were reused from the passed R3 build of
`530f1516c332931059c52161b52d64e86488b3f1`; they were not compiled from the new
entry-script commit. Their SHA-256 values are
`04fc6f93b4327911ca1a3bb5200175c9f2e13fb21ac5e56280012ff44b6e2e59` and
`12c8988c6a93cade9923d54bef784b774e13cb5e3b042f2e72ea5ecb6b897beb`.
The historical compiler-profile digest is
`8dae7219954e57236434f834b51c6dd74256ad7e14e5b68f88cf91864f31c955`.
No Rust compilation or ARM execution occurred in this software run. Historical
build-summary test counts are not included in the following totals.

| Stored check | Result |
| --- | --- |
| Full launcher lifecycle suite | 146 passed, including 17 new entry cases |
| Earlier targeted execution of those same new cases | 17 passed; repeated coverage |
| Library package regressions | 25 passed |
| Existing startup / spatial-dither / native-media package regressions | 18 / 5 / 3 passed |
| Unique cases | 197: 146 lifecycle + 51 package |
| Test executions | 214: 197 unique + 17 targeted repeats |
| Production shell syntax | 2 passed |
| Strict static package checker | 20 checks passed; 49 exact entries |

The refreshed ZIP digest is
`c57d5fc3674f1a0de448b85beb2552ee0959b83756c3e8f62afb559bf62a4882`.
It adds the document entry and three notices, updates README, third-party
notices, and the canonical launcher, and retains the other 42 old entries byte
for byte. Every old entry retains its mode and timestamp. The seven selected
packaging inputs, all 49 entry identities, and ELF/worker/ZIP identities are in
the retained provenance, metadata audit, checker, and sidecar records. The
launcher and document source digests are respectively
`584ad6065565bf5529fd4ef17393a454a0db811824c685e104a360c82080dea4` and
`7ac07e2a4e2b790642ff88787deaf05988e1bb8786d418b45bfd092d64d023f4`.

[Attempt R1](original/attempt-r1/summary.json) remains failed, with its original
error and zero executed tests. Its runner compared differing manifest orders.
[Attempt R2](original/attempt-r2/summary.json) is the passed software report; it
used a fresh runner and output directory. Both attempts' source, archive, and
runner integrity checks remained true. All 13 R2 command logs are retained,
including the two empty successful shell-syntax logs.

[export-manifest.json](export-manifest.json) maps every payload file to its raw
or derived status, origin, bytes, SHA-256, and transformation. Raw records and
runner snapshots preserve exact bytes; `.log` to `.txt` changes only filenames.
The manifest excludes itself and its digest from its payload list, and its own
SHA-256 is in [export-manifest.sha256](export-manifest.sha256). No directory was
recursively exported. ELF, ZIP, tar, SQLite, account/authentication/session data,
token values, private device transport, and physical-run material are excluded.
