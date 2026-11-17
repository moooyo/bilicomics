# Read-only startup build-cache audit

Date: 2026-11-17, Asia/Shanghai.

Scope: read existing source archives, build logs, Cargo metadata, file metadata,
and sealed executable/ZIP bytes on `test-env`. No build, test, application
execution, cache cleanup, device connection, or lease acquisition was performed.
This note is an ignored investigation artifact, not a change to old evidence.

## Finding

No evidence of Aidoku's specific stale diagnostic-backend contamination was found
in BiliComics' published startup baseline or candidate. The original source-bound
and physical records should remain unchanged. This audit is stronger than a
manifest-only check, but is not a claim that a strict clean rebuild has occurred.

Different absolute source directories did **not** isolate the backend Cargo
artifact: both variants used the same `slint-backend-kindle-706b69faa5e671b5`
fingerprint directory. The baseline and candidate application fingerprints both
record dependency ID `14858941681709501900` and dependency fingerprint
`18108384928962262943`. The backend depfile uses relative `vendor/` source paths.
Do not use the distinct checkout paths as an isolation argument.

## Actual rebuild evidence

Remote root: `/root/bilicomics-startup-20261117-01a1d11a`.
All times in this table are UTC+08:00 on 2026-11-17.

| Observation | Value |
| --- | --- |
| Workspace directory birth | 00:27:29.192514948 |
| ARM target directory birth | 00:28:08.658854141 |
| Backend fingerprint directory birth | 00:28:08.698852458 |
| Baseline source archive entry mtime | 00:12:46 |
| Baseline backend fingerprint file birth | 00:29:04.084522910 |
| Baseline ARM build completed | 00:32:33.071743 |
| Candidate source archive entry mtime | 00:35:33 |
| Candidate backend invoked.timestamp mtime | 00:46:04.477736 |
| Candidate backend fingerprint completion | 00:46:08.029588 |
| Candidate backend rlib birth | 00:46:08.017587919 |
| Candidate ARM build completed | 00:49:28.041214 |

The sealed logs explicitly compile the selected backend paths:

- `docs/verification/startup-20261117/original/baseline-arm-build.txt:362`:
  `Compiling slint-backend-kindle v0.7.1 (.../baseline/app/vendor/slint-backend-kindle)`.
- `docs/verification/startup-20261117/original/candidate-arm-build.txt:26`:
  `Compiling slint-backend-kindle v0.7.1 (.../candidate/app/vendor/slint-backend-kindle)`.

The remote original log hashes still equal the exported records:

| Log | SHA-256 |
| --- | --- |
| Baseline | `6dc846716463b4d931db71e5c99c75baeef022a2970c22033a4507cdb0d0f7aa` |
| Candidate | `e6730d3873e038f6adfcf11ac41438c46775909c8e1886a35ea224e259b7fa2b` |

The baseline archive has Git PAX comment
`06ad90b02edb28db43abeed3031932a91dc2f8df`; the candidate archive has
`585e571b422e9586f1e53a417cbaf330a9ec473f`. Every archived backend file matches
its extracted source bytes (35 baseline files, 36 candidate files). The candidate
source timestamp is later than the completed baseline build, so this is not
Aidoku's older-preserved-source-mtime sequence.

All baseline backend file ctimes are the extraction time, 00:27:40.112055; all
candidate backend file ctimes are 00:35:35.628083. Neither backend directory
contains symlinks. These observations support the retained source-to-build
sequence without assuming that archive contents alone establish linked contents.

## Sealed executable checks

The ZIP app entry matches its corresponding sealed stage executable byte for
byte. Actual full SHA-256 values still match the original records:

| Variant | App SHA-256 | ZIP SHA-256 |
| --- | --- | --- |
| Baseline | `ef3f7c352ff39e08ca14a5f040ada158b3c4d895650c451e2bfb99fb9f46261b` | `80daa02cb838e8d70a1444f4b563df56d5bca7e6d28f6ffd08cc6c799d95f913` |
| Candidate | `f5f958b219b8faef1776ba14f50b9d483f628f792ad43503fa46cf0d7879714f` | `6669b93445c2bc6cfd729d014ea2c3d156f40f307cfb233ae5ca09e7c39fc985` |

| Literal | Baseline count | Candidate count |
| --- | --- | --- |
| `aidoku: startup-probe` | 0 | 0 |
| `aidoku: startup-probe-barrier` | 0 | 0 |
| `render_us=` | 0 | 0 |
| `settle_us=` | 0 | 0 |
| `submit_us=` | 0 | 0 |
| `wait_us=` | 0 | 0 |
| `SLINT_KINDLE_INPUT_DIAGNOSTICS` | 1 | 1 |
| `SLINT_KINDLE_INPUT_DIAGNOSTIC seq=` | 1 | 1 |
| `input discovery metadata is unavailable or inconsistent` | 0 | 1 |
| `/sys/class/input` | 0 | 1 |
| `device/capabilities/abs` | 0 | 1 |
| `device/name` | 0 | 1 |
| `backend_install_begin` | 0 | 1 |
| `bootstrap_font_registered` | 0 | 1 |

The first two prefixes identify the retained Aidoku diagnostic source at
`/root/aidoku-native-render-profile-20261117/source/vendor/slint-backend-kindle/src/platform.rs:1100`
and `:1187`, as read independently by the diagnostic-source subtask. They do not
occur in either selected BiliComics source revision. The four candidate-only
input-discovery literals are in the new backend source, demonstrating that the
candidate executable contains that backend implementation. The last two markers
are candidate main-package markers.

The input diagnostic literals are legitimate opt-in production code in both
selected revisions, requiring `SLINT_KINDLE_INPUT_DIAGNOSTICS=1`, with a maximum
of 16 samples. Existing optional performance markers, including
`native_initial_setup`, `rgb_fingerprint`, `fnv1a64-rgb8-row-major`,
`trace_started`, and `trace_stopped`, are also legitimate. The baseline and
candidate blobs for backend `performance.rs`, `src/platform/native.rs`, and
`src/performance.rs` are unchanged. Presence of these markers is not contamination.

## Recommendation

The available evidence does not require invalidating or rerunning the old physical
batch. Before the next authoritative helper comparison, use a fresh, separate
`CARGO_TARGET_DIR` for each variant, or explicitly clean `bilicomics` and
`slint-backend-kindle` in this task's own target before each ARM build. Never clean
another task's cache. Preserve build logs and check both forbidden diagnostic
prefixes and expected candidate backend literals in the packaged app.

If exact reproducible identity of the old published package is required, perform
an additional strict rebuild in the original selected source directory, with the
same toolchain and flags, after cleaning those two packages in this task's target.
Compare the result with the frozen digest before drawing further conclusions.
A differing hash would require investigation; by itself it would not prove
diagnostic contamination. No such rebuild was performed during this read-only
audit.
