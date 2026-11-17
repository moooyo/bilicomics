# Aidoku retained Library-entry lifetime reference

Date: 2026-11-17, Asia/Shanghai.

This directory records the selected source reference for the BiliComics
Library-document entry candidate. The reference keeps the native document
handler active until an independently supervised launcher worker exits after
native-state recovery. These records establish source provenance and adaptation
scope; they do not establish final BiliComics adoption or acceptance.

## Pinned source identities

Source repository: [AidokuForKindle](https://github.com/moooyo/aidokuForKindle).
The reviewed main revision is
[`06b21491f7bf8977f1f44eb95e484ff9a14d6b8a`](https://github.com/moooyo/aidokuForKindle/commit/06b21491f7bf8977f1f44eb95e484ff9a14d6b8a).
The retained implementation was introduced in
[`bed9167565106bb0e6382e7890b1a1170cc143e6`](https://github.com/moooyo/aidokuForKindle/commit/bed9167565106bb0e6382e7890b1a1170cc143e6).
All five retained inputs, including the license, have the same Git blob at
both revisions. [UPSTREAM_FILES.json](UPSTREAM_FILES.json) records the complete
revisions, original paths, Git blob identities, byte lengths, and SHA-256
digests. The snapshots come from the retained implementation commit, rather
than an uncommitted working tree or an earlier prototype.

The upstream application's pinned `Cargo.toml` declares `GPL-3.0-or-later`.
Its complete original `LICENSE` is retained unchanged as
[LICENSE.AIDOKU](LICENSE.AIDOKU). The backend keeps its independent
`MIT OR Apache-2.0` declaration and original
[MIT](../../../vendor/slint-backend-kindle/LICENSE-MIT) and
[Apache-2.0](../../../vendor/slint-backend-kindle/LICENSE-APACHE) notices.
This record does not relicense that backend or select an overall BiliComics
license.

The four complete code snapshots under `upstream/bed9167/` preserve surrounding
source context. Retaining a complete file does not adopt every feature or test
within it.

| Original source path | Retained reference |
| --- | --- |
| `packaging/kindle/launch.sh` | `--library` waits for the independent worker, preserves its exit status, and requests worker cleanup on catchable cancellation |
| `packaging/kindle/document-launcher.sh` | ZIP Library document selects `launch.sh --library` |
| `packaging/kpm/document-launcher.sh` | KPM Library document selects the same canonical launcher and `--library` lifetime |
| `packaging/tests/launcher_test.py` | Complete fixture context for exit-status propagation, same-process sleep/wake, and independent recovery after retirement of the small waiting entry |

The upstream `packaging/tests/kpm_hooks_test.py` diff changes only the document
entry's expected target and arguments. That test file is not needed to preserve
the new lifecycle-test context and is not copied. The upstream report remains
available at its
[pinned source location](https://github.com/moooyo/aidokuForKindle/blob/06b21491f7bf8977f1f44eb95e484ff9a14d6b8a/docs/review/native-library-entry-20261117/README.md).
Its measurements, test counts, and physical-device observations describe Aidoku,
not BiliComics results.

## BiliComics adaptation scope

The candidate applies the retained Library-document lifetime convention to
BiliComics' existing launcher. Its waiting entry leaves native display, input,
sleep/wake, and recovery ownership with the independent worker. Ordinary direct
launch admission retains its detached behavior. This bundle creates no KUAL
integration or KPM installer; future direct menu/package hooks must preserve
that detached behavior. The adaptation
does not import Aidoku's document identity, cover assets, native registration,
scanner machinery, or recent-access evidence. BiliComics native indexing and
recent-access behavior require separate qualification.

This is an adaptation rather than a verbatim copy of the upstream trap and
wait block. In that block Aidoku uses its existing parent-PID ownership check
before signaling the worker. BiliComics additionally binds direct-child
signaling to PID, process start time, and PPID. Readiness, pending cancellation,
and the wait loop are integrated with BiliComics' existing worker-generation
protocol. Those are independent BiliComics changes, not safeguards attributed
to the upstream waiting entry.

The application ELF, native helper, Rust application code, account and storage
logic, rendering, caches, and backend are outside this source adaptation. The
provenance update does not replace existing backend or third-party notices.

No native entry daemon, application-manager bridge, shared executable wrapper,
alternate handler, interposition library, or replacement registration is
imported. The two rejected approaches and the `prototype/` tree in the upstream
review are excluded. Their code, measurements, and device records are not
retained here.

## Verification and distribution scope

No tests, builds, runtime checks, SSH operations, or device operations were run
for this provenance update. Static source inspection and byte extraction only
establish the recorded input identities. Any later BiliComics verification must
bind its own exact source and artifact inputs. Aidoku results do not establish
BiliComics startup speed, device compatibility, panel completion, optical
quality, or physical acceptance.

Development archives retain only this README, the complete original license,
and the upstream manifest in `notices/aidoku-native-library-entry/`. The selected
code snapshots remain source records outside the runtime package. The separate
notice group records this candidate's provenance without replacing earlier
startup, startup-sequence, native-query, media, or dithering records.

This directory contains no credentials, account contents, session files, signed
image URLs, acquired comics, private device identifiers, or research archive.
It does not declare an overall project license or a published release.
