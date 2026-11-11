# Reference implementation provenance

Recorded: 2026-11-03 (Asia/Shanghai).

## Comic-cover sleep reference reviewed on 2026-11-12

The operator requested the corresponding optimization from the WeRead
sleep-cover chat. The same local reference checkout is now clean at revision
`6a1d070f3b4730146eafeea849fe8b67fc5cf14d`. The following inputs informed the
bounded awake cover preparation, aspect-preserving overlay, explicit renderer
completion gate, independent panel-update markers and retained native display
ownership. Their reviewed bytes are identified below.

| Reference input | SHA-256 |
| --- | --- |
| `src/app/sleep_screen.rs` | `25aa0e49f9038b1e7e659e1979524204298c29009925a7803e66fe6bf5232a66` |
| `src/app/managed_sleep.rs` | `14eeb244a813cb59e7136fdf7a840fae41fa8eb50920cd45c72b0c26db26e1f4` |
| `ui/sleep-screen.slint` | `0badece6fbf819e838acdbfb99e4ad9a38d5da049a10d67b7424ff7fc33c1c74` |
| `packaging/kindle/launch.sh` | `713167110d4839f14505cd03c6b30753476772c729fe92890a86fbd0085fe6be` |
| `vendor/slint-backend-kindle/src/sleep_presentation.rs` | `f152a12e9c525c7a09d96ae5f47a717a821ca456e58cc3895cd48d95c873f75a` |
| `vendor/slint-backend-kindle/src/framebuffer/mod.rs` | `d6182d2272acfcc06621063e109d98af99c7a41d45c68a46b1ef2eda4512321d` |

BiliComics keeps its own lifecycle-request protocol, account epochs, comic
source adapter, managed cover cache, read-state checkpoints, native-presentation
records and display lifecycle handle. WeRead business logic, account/session
data, books, logs and research archives were not imported. Its accepted physical
demonstration and software test results are reference-project evidence; they do
not verify BiliComics. The backend's original licenses and pinned upstream
revision remain unchanged. See [comic-cover sleep](SLEEP_COVER.md) for this
project's implementation and verification boundary.

## Imported local working tree

The technical reference is the user-supplied local WeRead project at `C:/Users/moooyo/Documents/Codex/2026-11-02/d/outputs/weread`.

Its inspected Git HEAD is `71b8456f39a51af636764ffd45b1c2c4c0efb8e8`, but that commit does not contain the complete inspected implementation. At inspection, `src/`, `ui/`, `assets/`, `vendor/`, `packaging/`, `scripts/`, `Cargo.toml`, `Cargo.lock`, and several implementation documents were untracked; existing architecture and repository documents were modified. The source of this import is therefore the local working tree observed on the recorded date, not an assertion that the commit reproduces it. The original source-file SHA-256 values below identify the actual inputs.

Only explicitly selected source modules, launcher/configuration examples, dictionary resources, and third-party notices were imported. Reference runtime directories, account files, content, logs, screenshots, archives, build output, and Git metadata were not copied. The original reference directory was not modified.

## Imported units and local changes

| Destination | Input and changes |
| --- | --- |
| `src/platform/` | Imported the reference platform boundary, geometry profiles, refresh scheduler, and native adapter. Renamed application runtime/thread identifiers to `BILICOMICS`/`bilicomics`. Removed WeRead page margins and cover/list/catalog page counts. Preserved geometry parsing and platform APIs. |
| `src/platform/native.rs` | Removed the reference-specific experimental framebuffer override. Native operation requires `BILICOMICS_APPROVED_PROFILE` and `BILICOMICS_APPROVED_FIRMWARE`, exact reference framebuffer geometry, and zero panning offsets. |
| `src/input.rs` | Imported the standard-library offline pinyin engine. Its only resource dependency is `assets/pinyin/dictionary.tsv`. No WeRead network, UI, or domain type is required. |
| `src/cache.rs` | Imported the bounded, single-process file cache and its independent SQLite index. Renamed the test temporary-directory prefix. Added a narrow Linux FAT/exFAT permission fallback, adapted from the reference `src/session.rs`. Filesystems other than FAT/exFAT and errors other than `EPERM`/`EOPNOTSUPP` still fail. Added a pure fixture for that classification boundary. |
| `assets/pinyin/` | Copied the dictionary, generator, and complete AOSP notice without modifying their bytes. Updated the readme to distinguish reference-project checks from BiliComics evidence. |
| `vendor/slint-backend-kindle/` | Copied the reviewed vendored library, manifest, original licenses, README, upstream identity, and full patch history without modifying their bytes. |
| `packaging/kindle/` | Adapted the launcher, independent paths/environment variables, package identity, and explanatory notes. Removed the experimental display configuration channel. Kept the example allowlist empty. No release package was built. |

`src/session.rs` is included in the fingerprint table solely because its filesystem permission helper informed the cache adaptation; the WeRead session module and its credential model were not imported by this infrastructure task.

The imported reference geometry and refresh policies are implementation starting points. They do not establish BiliComics UI design acceptance, device compatibility, panel quality, or a supported firmware matrix. UI design sources and application behavior are maintained separately by this project.

## Third-party origins retained

### Kindle backend

- Reviewed upstream: [KindleTweaks/slint-kindle-backend-patch](https://github.com/KindleTweaks/slint-kindle-backend-patch/tree/03aa48d3531e9eae7b360bdc24e90a72e9f307a4).
- Upstream revision: `03aa48d3531e9eae7b360bdc24e90a72e9f307a4`.
- Vendored package: `slint-backend-kindle` 0.7.1.
- Original licenses: `vendor/slint-backend-kindle/LICENSE-MIT` and `LICENSE-APACHE`.
- Identity and inherited patches: `UPSTREAM_REVISION` and `PATCHES.md` in that directory.

The inherited patch history covers queued post-render refresh, bounded wakeup delivery, application-local clipboard slots, corrected native touch scale conversion, and font dependency compatibility. Its historical test and emulator reports belong to the reference project. Those reports are preserved as provenance and are not BiliComics verification results.

The library requires Rust 1.92 and exactly Slint 1.17.1. Its direct font dependencies were already aligned with Slint's fontique 0.10 and fontdb 0.23 by the reference compatibility patch, with fontconfig loaded optionally rather than required at link time. Preserve these relationships when integrating or updating dependencies.

### Offline pinyin dictionary

- Project: [AOSP PinyinIME](https://android.googlesource.com/platform/packages/inputmethods/PinyinIME/).
- Revision: `49aebad1c1cfbbcaa9288ffed5161e79e57c3679`.
- Source: `jni/data/rawdict_utf16_65105_freq.txt`.
- License: Apache License, Version 2.0; the original NOTICE and license text are retained in `assets/pinyin/LICENSE-AOSP.txt`.
- Source SHA-256: `408700f28a56091fa07f3b849a0f134fbfc71e6b2ae9b3f52973a5b076f599ff`.
- Derived dictionary SHA-256: `a32775ccf82e47d04feb3f01e5174ec6fcf12a15064da14cb5cd840a1c1eb422`.

The transformation and format are described in the retained dictionary readme. The dictionary is input-method data rather than downloaded comic content. Third-party licenses do not select a license for the BiliComics application.

## Integration dependencies and startup

| Module | Required dependencies |
| --- | --- |
| `platform` | `slint`, `thiserror`, `libc`; Linux `kindle` builds additionally use the optional local `slint-backend-kindle`. |
| `input` | Rust standard library and the bundled dictionary. |
| `cache` | `anyhow`, `rusqlite`, `sha2`, `image`; Linux permission classification additionally uses `libc`. |
| Inherited fixtures | `tempfile` where required by existing platform/cache fixtures. |

The platform expects application features named `desktop` and `kindle`. Keep the native backend behind `cfg(all(feature = "kindle", target_os = "linux"))`.

Install the Kindle backend before starting Tokio or any other application workers, because installation configures process environment. Keep the returned backend alive through the event loop, install its refresh handle as a `KindleRefreshSink`, create the Slint window, and call `platform::configure_window()` before running it. Register the actual bundled font instead of relying on Kindle system font discovery.

UI refresh rectangles use logical coordinates; the native sink applies the two-to-one reference scale once. Producers queue refresh requests, and the backend applies them after drawing. Queue acceptance is not a hardware completion receipt. Image decode/network work should run off the UI thread, while Slint object mutation and the metadata connection remain on their owning application thread.

The cache assumes a single application process owns its directory. File keys are stable resource identities rather than signed URLs, and payload names use digests. Account credentials belong outside the cache index. Metadata/domain models, BiliComics protocol work, controllers, UI, and application entry points are integrated independently rather than importing WeRead business modules.

## Verification boundary

This import performed source reading, controlled source copying/editing, and source fingerprint recording. No local tests, build verification, smoke tests, application execution, hardware access, or runtime probes were run. Existing and newly added fixtures have not been executed as part of this infrastructure import. Verification must use `ssh test-env` unless the user explicitly authorizes local verification for the current task.

The committed launcher allowlist contains no accepted pair. No physical BiliComics Kindle run, BiliComics firmware-emulator run, ARM build, live account, or release package is established by these files. Reference-project results remain separate evidence.

## Original input fingerprints

These hashes describe the source files before the BiliComics adaptations above. Hashing read-only source inputs records provenance; it does not verify application behavior. Paths are relative to the local reference root.

| Original reference file | SHA-256 |
| --- | --- |
| src/platform/mod.rs | dd2eabde5183845b931afa8e9fff1643597f5b42c5bc1b67fd2a3e777d65138a |
| src/platform/native.rs | 2518c16ed98befa5ef6af86c220db0526aa0722a02b1d59acb50eed9b0102b92 |
| src/platform/profile.rs | f8a1c39c263557fca11bc482ec8c5bd42cc6453773a54a192b40717b28bc5547 |
| src/platform/refresh.rs | 6f522637f891391e30b61f023e2a91d3a5b9160eedf98e63f41a6bda7f4d9809 |
| src/input.rs | 0ac397cea699c0eee182f5ee9bbbbd0511ac746d0be8f95c74ee3c537ad20929 |
| src/cache.rs | 5717e54d34ea823049c7ef8b8438c9ecfeae7ca8ee54c31997843cdcbc9ac2a9 |
| src/session.rs | 0f570b6cc5318cfdae6642344bf1f9d267f53a583928ef8c5d9932fc63d3c93d |
| assets/pinyin/dictionary.tsv | a32775ccf82e47d04feb3f01e5174ec6fcf12a15064da14cb5cd840a1c1eb422 |
| assets/pinyin/generate_dictionary.py | 19b0cc0cd9070251aa16e7d5037de5b9f68ff0bbca504b7e0f48e3fec2a9c447 |
| assets/pinyin/LICENSE-AOSP.txt | b5830d96fb5a7e7e7ebcc295f352846b4b998e78fdc8f9aa68e134d2e4b39986 |
| assets/pinyin/README.md | f5fe4df2086cc682db80c2288117d6ff793937280f086a08906667a5378ae172 |
| vendor/slint-backend-kindle/Cargo.toml | a891b67d242fd43ebf1efe601504f9007890e6c6ce6d31c83799db96b8534838 |
| vendor/slint-backend-kindle/LICENSE-APACHE | cfc7749b96f63bd31c3c42b5c471bf756814053e847c10f3eb003417bc523d30 |
| vendor/slint-backend-kindle/LICENSE-MIT | ba7f9ac20add41f136054ce7e9310bafd5132b5a9a6d911cc1115c64585e0c26 |
| vendor/slint-backend-kindle/PATCHES.md | 9c372ee0766ce1ef1ddb0c0e27acf6a18bfd3372e9356b5884b68ccd27a0ad0d |
| vendor/slint-backend-kindle/README.md | 6998f58b671322f10a780686771088516f707554eefd08ce26f77d6646010e3b |
| vendor/slint-backend-kindle/UPSTREAM_REVISION | d45d10be890488152e931b37114c2df655e4c747c4eeace75b4312cd250c270c |
| vendor/slint-backend-kindle/src/clipboard.rs | bbe884363ed6e9f827965a5e6051b110ca1dca1f9f6c96cb3820504b6c06139d |
| vendor/slint-backend-kindle/src/lib.rs | db3cb5146c47e0a8a385c43619247d057380e6eb654e57c0709904249d14b668 |
| vendor/slint-backend-kindle/src/platform.rs | 33cd41cb1b367d72be86b8b6df2de237717ab54e493450543dbb8eb1af998f28 |
| vendor/slint-backend-kindle/src/power.rs | a5781e75fc2baed7d39ffd54bf13662775f92c007264ccda572ac617c6faf105 |
| vendor/slint-backend-kindle/src/refresh.rs | 20cbb6e8afed9cd0b54527aa000bf43c24949ce6db6abbc0460f6e4c0f1f0133 |
| vendor/slint-backend-kindle/src/touch.rs | 2fa9913422a76bc6e9775388736042e41abc581eaab593f0b62ce69ab4120081 |
| vendor/slint-backend-kindle/src/wakeup.rs | d2d919196de3919df8f5ebb428d64d9a468a06d5bf29297210cf5251ca8cbaf1 |
| vendor/slint-backend-kindle/src/framebuffer/ffi.rs | 59a0d64b89ecd2b1846a5bb5e6e74a9470a0b38eb8df839f3933fccc2f40621e |
| vendor/slint-backend-kindle/src/framebuffer/mod.rs | a80c371e11e7a9f6d13f89e8307642dbcb67daa0ad4a96f4431ff6eaa8fe92a7 |
| packaging/kindle/device-allowlist.example | e6c2da5683bffeaf6215b1f1864712f8af75b93b2a7d89681712a457efc2aaa8 |
| packaging/kindle/device-profile.example | 7868cc52e664bc320467fd5255663faa7a6f3e5a781be2998e3c2a6e0fd60d7c |
| packaging/kindle/launch.sh | 17531d039a2eeae148aae8c3a04a81a304d78202e7e56eb17d6ac31bd983550b |
| packaging/kindle/manifest.draft.json | de2ee0972ffb99464a2e0f42601d03bed50a3eed10f7d4e26139a8a6a2b4ba3d |
| packaging/kindle/README.md | 51b2b79348ed426921c7338941fa6e0606f79ee74730814ef3638bb1bc8ce937 |

## Launcher lifecycle adaptation on 2026-11-04

The same local WeRead working tree supplied the FIFO supervisor, child-runner shutdown handling, display ownership baseline, and native unlock probe patterns for `packaging/kindle/launch.sh`. Its checkpoint-and-restart behavior was replaced with a BiliComics-specific `USR1`/`USR2` request/acknowledgement exchange that keeps one application PID and its reader state in memory. The new exchange has an early startup readiness marker, a random launch token, strictly matched sequence/action fields, a bounded asynchronous acknowledgement deadline, and a separate checkpoint-error outcome. It does not import WeRead account, session, or reading models.

`packaging/tests/launcher_test.py` adapts the reference fixture's temporary firmware/service environment, process-group leak check, and event injection patterns. Its application fixture is new: it retains a nondefault comic, episode, revision, and page in one process and distinguishes sleep from ordinary exit. Its evidence is isolated Linux supervision evidence, not native input/display, firmware-emulator, real-account, or physical-device evidence. No local tests were run. The reference working tree was not modified.

| Reference input observed on 2026-11-04 | SHA-256 |
| --- | --- |
| `packaging/kindle/launch.sh` | `d40f1a4c2afd690b3bd7d64e9ca5d4dc2282e1053da8c4331755d342acea3a4a` |
| `packaging/tests/launcher_test.py` | `cd1762aac6c8192a60e99c7fd473ce3799a3f68b8efa0f8bd2ad86e6e61fd19d` |

## Read-only native presentation reference on 2026-11-06

The same local WeRead working tree's `src/platform/native_theme.rs` and
`packaging/kindle/launch.sh` were inspected for the `com.lab126.winmgr epdcMode`
getter, native ownership timing, and `Y8`/`Y8INV` interpretation. Their initial
observed bytes were not saved or fingerprinted. A later working-tree hash must
not be presented as the identity of those observations.

BiliComics implements its own bounded read-only launcher observation, private
startup/resume records, sequence-bound consumer, and framebuffer output
compensation. The reference's system-mode setter and persistence synchronization
were not imported. Its results are contextual evidence from another application,
not verification of BiliComics. The fixed-buffer power-event relay and its
control barrier are BiliComics additions to the existing supervisor.

## Stock screensaver event semantics observed on 2026-11-06

The public KOReader primary source
[`frontend/device/kindle/device.lua`](https://github.com/koreader/koreader/blob/master/frontend/device/kindle/device.lua#L837)
was read to distinguish a stock screensaver-close request from completed
dismissal. The observed content SHA-256 was
`b38eeb22c755a4b0538f24740f9866ed0e33269841a326b74a12a80cfb35fea0`.
Its comment at the observed line 837 distinguishes `OutOfSS` from `ExitingSS`.
The matching observed `powerd.lua` content SHA-256 was
`e4f233415386f9d5c48ad5d8222d8c4c703090908f08d61fc999978a2649282f`.
The GitHub commit lookup was unavailable, so these are observed content hashes,
not claimed pinned commit IDs. Research snapshots remain outside the repository.

Only event semantics informed the launcher review. KOReader screensaver,
framework-control, rendering, and delayed-refresh implementations were not
imported. No source observation establishes PIN, advertising-lockscreen, or
firmware compatibility for BiliComics.

## Kernel grayscale observation reviewed on 2026-11-07

The public FBInk primary header
[`eink/mxcfb-kindle.h`](https://github.com/NiLuJe/FBInk/blob/master/eink/mxcfb-kindle.h#L125)
defines the standard eight-bit grayscale values as 1 for normal and 2 for
inverted. The observed header SHA-256 was
`6e657f7c201146326f8b9bcae8a9c8308549baee73b1aefe7d6674fd341afbdd`.
The observed [`fbink.c`](https://github.com/NiLuJe/FBInk/blob/master/fbink.c#L5953)
uses the same eight-bit framebuffer field to report inverted grayscale; its
content SHA-256 was
`8e498e07d78a9f744a9c429457ab46b2709a5685410e33d645527d9b6ddcf962`.
These are observed content hashes, not claimed Git commit IDs.

BiliComics implements an independent read-only GETVAR/GETFIX observation. No
FBInk rendering, state-setting, or other implementation was imported. Other
valid pixel formats keep the reviewed native-property fallback. The kernel
format observation does not set a system theme or imply hardware acceptance.
