# Aidoku startup-sequence item 1 reference

Date: 2026-11-17, Asia/Shanghai.

This directory records the selected source reference for the BiliComics item 1
candidate: prepare background font bytes and application metadata after an
accepted initial full SEND, while the existing checked completion barrier is
still pending. It records provenance and adaptation scope, not a final adoption
decision or a BiliComics performance result.

## Pinned source identities

Source repository: [AidokuForKindle](https://github.com/moooyo/aidokuForKindle).
The reviewed main revision is
[`0992fee1377f08c5d4b537a26985bca8d4a52898`](https://github.com/moooyo/aidokuForKindle/commit/0992fee1377f08c5d4b537a26985bca8d4a52898).
Its startup-sequence report identifies
[`fc51eac0625565052fb3fd0ef7c1f117705ecba5`](https://github.com/moooyo/aidokuForKindle/commit/fc51eac0625565052fb3fd0ef7c1f117705ecba5)
as the compiled production source; later report commits do not change these
selected inputs. Every retained file has the same Git blob at both revisions.
[UPSTREAM_FILES.json](UPSTREAM_FILES.json) records the full revisions, source
paths, Git blobs, byte lengths, and SHA-256 digests.

The upstream application declares `GPL-3.0-or-later` in its pinned `Cargo.toml`.
Its complete original `LICENSE` is retained unchanged as
[LICENSE.AIDOKU](LICENSE.AIDOKU). The backend retains its independent
`MIT OR Apache-2.0` declaration and the original
[MIT](../../../vendor/slint-backend-kindle/LICENSE-MIT) and
[Apache-2.0](../../../vendor/slint-backend-kindle/LICENSE-APACHE) notices.
These records do not relicense that dependency or select an overall BiliComics
license.

The four complete code snapshots under `upstream/0992fee/` are unchanged Git
blobs. They retain surrounding upstream behavior for context; retaining a
complete file does not adopt every feature in it.

| Original source path | Item 1 reference |
| --- | --- |
| `src/bootstrap_metrics.rs` | The active initial/full/submitted checkpoint calls `prepare_after_submission` |
| `src/bootstrap_startup.rs` | Idempotent background font/metadata preparation and the existing UI-turn attachment boundary |
| `vendor/slint-backend-kindle/src/platform.rs` | Accepted SEND checkpoint before the checked initial completion wait |
| `vendor/slint-backend-kindle/src/frame_observer.rs` | The upstream general observer mechanism, retained as adaptation context |

## BiliComics candidate scope

The backend's [patch record](../../../vendor/slint-backend-kindle/PATCHES.md)
describes the independent one-shot `std::sync::mpsc` adaptation. The notification
carries only `()` after a real accepted initial complete draw. It does not
execute the upstream arbitrary observer callback while retaining the framebuffer
borrow. The existing checked SEND completion and lifecycle/input ordering stay
after that notification. Acceptance alone is not input readiness, driver
completion, or optical panel evidence.

The application candidate admits only background font bytes and metadata early.
Font registration, Slint object construction, application attachment, and startup
cover pixel preparation retain their existing UI-turn gate. BiliComics already
had its own prepare/attach boundary, coalesced wake handling, bounded startup
feedback, and first-turn continuation. The upstream startup module itself names
that earlier BiliComics boundary; these mechanisms are not claimed as new ports.
BiliComics keeps its own fonts, application identities, account and persistence
code, lifecycle ownership, cancellation, and recovery behavior.

Items 2 through 6 from the upstream sequential report are outside this change:
post-business-frame cover admission, derivative reuse and cover publication
coalescing, combined sleep observation/native-query calibration, batched lighting
updates, and shared-dispatch work are not implemented here. In particular, the
cover release in the retained `bootstrap_metrics.rs` is not part of this item 1
adaptation. No GNU helper, release-profile change, upstream library contents,
cover artifacts, device capture, benchmark dataset, or deployment record is
imported.

## Verification and distribution scope

No tests, builds, runtime checks, or device operations were run for this
provenance update. Any later BiliComics verification must identify its own exact
source and artifact inputs. Aidoku results do not establish BiliComics speed,
device compatibility, panel completion, optical quality, or physical acceptance.
This directory contains no credentials, account data, signed image URLs, comic
images, private device identifiers, or research archive.

Development archives retain only this README, the complete original license,
and the upstream manifest in `notices/aidoku-startup-sequence/`. The selected
code snapshots remain source records, outside the runtime package. The package
checker binds this separate notice group to the pinned source identities and
retained bytes. Existing startup and native-query provenance directories remain
independent. No published release is established by these records.
