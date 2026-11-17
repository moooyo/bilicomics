# Fresh-target baseline identity investigation

Date: 2026-11-17, Asia/Shanghai.

Scope: read-only metadata, source-file hashing, generated-file comparison, ELF
inspection, and byte/string inspection on `test-env`. This investigation did not
build, clean any cache, execute an app, connect to the Kindle, or acquire a lease.
The root task had already built the fresh baseline before this audit started.
Existing published records and binaries were not modified.

## Finding

The fresh baseline is a valid separately identified control for the upcoming
helper comparison. Its different app hash is not evidence of diagnostic-backend
contamination. Both selected source manifests are byte-identical, while actual
generated source and packaged ELF bytes contain build-directory-dependent paths.
Those paths are a demonstrated reason byte identity can change after moving the
build root. This audit does not prove that every final ELF difference is caused
only by paths, and does not claim whole-program binary equivalence.

| Identity | Published startup candidate | Fresh main baseline |
| --- | --- | --- |
| Source revision | `585e571b422e9586f1e53a417cbaf330a9ec473f` | `f50ada15dd934c986e8305a1f9dd8e46f51b2529` |
| Source root | `/root/bilicomics-startup-20261117-01a1d11a/candidate/app` | `/root/bilicomics-native-query-20261117-01a1d11a-r1/baseline/app` |
| Compiled input files | 542 | 542 |
| Compiled input manifest SHA-256 | `33bda43e34f74c7e73058880b4a1ae7a8d4b00d3ec0ae9a1ee22a074b59beb28` | Same, with identical manifest bytes |
| App bytes | 74,269,368 | 74,269,368 |
| App SHA-256 | `f5f958b219b8faef1776ba14f50b9d483f628f792ad43503fa46cf0d7879714f` | `1970842488d4b7f389d3e3faa493759a0294b2109ead91a8e4e76aec25dfbb5d` |
| Worker SHA-256 | `12c8988c6a93cade9923d54bef784b774e13cb5e3b042f2e72ea5ecb6b897beb` | Same |
| ZIP SHA-256 | `6669b93445c2bc6cfd729d014ea2c3d156f40f307cfb233ae5ca09e7c39fc985` | `2eb7b6f2565bd1b6c93190ae51ebeb9750fffd9150654e92d28f64f090cf3dc4` |

The input manifest was reconstructed by read-only hashing using the published
manifest's exact file scope and ordering. The tracked Git change between the two
revisions contains only documentation and the corresponding `.gitattributes`
rules; no production input or build-script changes were found. The fresh source
archive has the expected `f50ada15dd934c986e8305a1f9dd8e46f51b2529` Git PAX comment.
Its 51,481,882-byte ZIP contains the stage app byte for byte.

## Concrete path-dependent differences

The generated files under the respective
`armv7-unknown-linux-musleabihf/release/build/bilicomics-e21812236f91c1bb/out/`
directories were compared by the diagnostic-source subtask:

| Generated file | Old bytes | Fresh bytes | After replacing each workspace root with the same token |
| --- | --- | --- | --- |
| `app.rs` | 13,915,129 | 13,915,395 | Byte-identical; 38 root occurrences each |
| `demo_assets.rs` | 4,975 | 5,227 | Byte-identical; 36 root occurrences each |

Normalization replaces each complete workspace root with `<WORKSPACE>`.
Normalized SHA-256 values were
`3bc2eff0dab641627c6895bd106e8999e8ef1f63271771a07f373c49db0abee0`
for `app.rs`, and
`19fb7e2f411de9a287e11a46934547c5ae908ce9be11ff5d46752ca8a16a4304`
for `demo_assets.rs`. The raw byte-size changes exactly match the number of root
occurrences times the seven-byte root-length difference. Generated
`strings.slint` and `translations.json` hashes are unchanged.

Each actual packaged ELF contains its own build root three times, in strings for
wasm3 `m3_env.c`, wasm3 `m3_compile.c`, and generated `out/app.rs`. The fresh
offsets are 1,504,424, 1,582,726, and 1,519,211, respectively. Neither ELF contains
the other build root. `build.rs` canonicalizes demo asset paths, and the native
build uses `CARGO_MANIFEST_DIR` to construct absolute C source paths. There is no
configured Rust/C path remapping. Symbol stripping does not remove runtime
source-location or C `__FILE__` strings.

## Diagnostics, ABI, and compiler checks

Both old and fresh apps have zero occurrences of `aidoku: startup-probe`,
`aidoku: startup-probe-barrier`, `render_us=`, `settle_us=`, `submit_us=`, and
`wait_us=`. Each contains one occurrence of every expected input-discovery
signature: `/sys/class/input`, `device/capabilities/abs`, `device/name`, and
`input discovery metadata is unavailable or inconsistent`. Legitimate opt-in
`SLINT_KINDLE_INPUT_DIAGNOSTICS` remains present.

Both apps are ELF32 little-endian ARM EXEC, EABI5 hard-float, with entry point
`0x312ab80`. Neither has a dynamic section or an INTERP program header. ELF
section offsets/sizes, ARM attributes, exception tables, and compiler `.comment`
bytes are unchanged. `.rodata`, `.text`, and several pointer/data sections differ;
this is more than an in-place replacement of three string prefixes, so this
audit does not assert instruction equivalence or fully attribute linked layout.

The compiler versions are Rust/Cargo 1.99.1 and Zig 0.15.2. The fresh verbose log
records target `armv7-unknown-linux-musleabihf`, opt-level 3, ThinLTO for final
binaries, CGU1, `strip=symbols`, and the same Zig linker wrapper family. The old
and fresh backend fingerprint JSON files are identical. Main-package fingerprint
metadata has the same compiler, configuration, target, profile, and flags; the
changed dependency entry is the build-script fingerprint, followed by its main
library fingerprint in the binary record. The selected production settings are
consistent with the prior recorded release profile.

## Limits and next step

Do not label the fresh app as byte-identical to the published package. Do not
label the published package as contaminated solely because its hash differs.
Use the fresh baseline's own identity for the new helper A/B, build the helper
candidate in its own fresh target, retain both verbose logs, and compare the
new batch only. The older physical results remain records of their actual frozen
executables.
