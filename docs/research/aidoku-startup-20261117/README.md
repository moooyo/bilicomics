# Aidoku startup implementation provenance

Date: 2026-11-17, Asia/Shanghai.

The user requested that BiliComics adopt the applicable startup improvements
completed in AidokuForKindle. This directory retains selected original source
files and their license for review. It contains no account data, device logs,
comic images, performance captures, or complete research archive.

## Pinned sources

Source repository: [AidokuForKindle](https://github.com/moooyo/aidokuForKindle).
The source package declares `GPL-3.0-or-later`; its complete original GPLv3
text is retained as [LICENSE.AIDOKU](LICENSE.AIDOKU). The upstream backend's
existing MIT and Apache-2.0 notices remain in `vendor/slint-backend-kindle/`.
These source-specific records do not select an overall BiliComics license.

| Commit | Retained implementation | BiliComics adaptation |
| --- | --- | --- |
| [`6660e32a5035a61705b942635fc41c7a16f749f7`](https://github.com/moooyo/aidokuForKindle/commit/6660e32a5035a61705b942635fc41c7a16f749f7) | `packaging/kindle/launch.sh`, `src/app/startup.rs` | Reduce bounded observer/native-reader polling intervals while preserving their total budgets; coalesce lightweight attachment stages between time-budget checks |
| [`e141d4479ae6440e2ba3f7b93d8f0e616458055f`](https://github.com/moooyo/aidokuForKindle/commit/e141d4479ae6440e2ba3f7b93d8f0e616458055f) | `build.rs`, `src/bootstrap_fonts.rs`, `src/bootstrap_startup.rs` | Expand BiliComics' own canonical font bundle at build time, share static font bytes, and retain activity feedback during actual preparation waits |
| [`4ce1fd156a7a4a75ca2fda5f4324991d6b02ce92`](https://github.com/moooyo/aidokuForKindle/commit/4ce1fd156a7a4a75ca2fda5f4324991d6b02ce92) | Backend `input_discovery.rs`, `lib.rs`, `portrait_pose.rs`, `touch.rs` | Add bounded, verified sysfs hints and transfer the validated touch descriptor to its owner |

The nine selected code files are unmodified Git-blob snapshots under
`upstream/<commit-prefix>/`. [UPSTREAM_FILES.json](UPSTREAM_FILES.json) binds
each local path to its full commit, original path, byte length, and SHA-256.
The retained license is taken from the final pinned commit. The font payloads
remain BiliComics' five existing canonical gzip faces and startup font; Aidoku
fonts, application identities, artwork, and library contents are not imported.

BiliComics already combined its initial clear with the first rendered startup
scene and already used a shell builtin for its principal process-identity
reads. Those existing mechanisms are not claimed as new ports. The adaptation
keeps BiliComics' launcher identities, lifecycle and recovery barriers, fresh
portrait checks, input grabs and synchronization, and legacy discovery fallback.
No input device number is hardcoded and no pose is cached across calls.

## Verification and distribution scope

Aidoku's reports and physical-device timings retain their original scope.
They do not establish BiliComics performance, device compatibility, panel
completion, or optical quality. BiliComics requires its own source-bound remote
verification and matched-profile physical comparison. Builds, tests, and
validation run through `ssh test-env`; physical operations additionally require
the shared device lease and complete native-control restoration.

Development archives retain only this README, the complete original license,
and the upstream manifest in `notices/aidoku-startup/`. The selected code
snapshots remain in the source repository and are not runtime package assets.
The package checker verifies the exact notice group, pinned source identities,
and retained source bytes. No published release is established by these records.
