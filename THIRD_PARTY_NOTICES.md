# Third-Party Notices

This file records principal reused sources and assets. It does not select this project's license or replace original license texts.

| Material | Local location | License and provenance |
| --- | --- | --- |
| Kindle Slint backend | `vendor/slint-backend-kindle/` | Original MIT and Apache-2.0 texts, revision, and patch history retained |
| zune-jpeg strict decode adaptation | `vendor/zune-jpeg/` | Pinned crate 0.5.15; original Apache-2.0, MIT, and Zlib texts, upstream metadata, pristine changed files, and strict-mode patch retained |
| Single-channel Triangle resize | `src/reader/triangle.rs`, `vendor/image-triangle-reference-0.25.10/` | Derived from pinned image 0.25.10 under MIT OR Apache-2.0; complete licenses, exact source snapshots, crate checksum, Git revision, and numerical contract retained |
| WeRead infrastructure | Platform, cache, input, and launcher | Exact working-tree input hashes and adaptations in `docs/REFERENCE_PROVENANCE.md` |
| Bilibili protocol wrappers | `vendor/bili-protocol/` | Pinned plugin revision and adaptations in its README; no project-wide license grant inferred |
| wasm3 | `vendor/bili-protocol/wasm3/` | MIT; original LICENSE retained |
| cJSON | `vendor/bili-protocol/cjson/` | MIT; original LICENSE retained |
| Mbed TLS subset | `vendor/bili-protocol/legacy_v5/vendor/mbedtls/` | Apache-2.0 option; original LICENSE, manifests, and patches retained |
| Pinyin dictionary | `assets/pinyin/` | AOSP-derived; original Apache-2.0 attribution and license retained |
| Noto Sans and Serif CJK SC | `assets/fonts/` | SIL Open Font License 1.1; original copyright and notices retained |
| Fictional preview artwork | `assets/demo/` | Exported from the user-supplied design snapshot; see `docs/DESIGN_SOURCE.md` |
| Rust dependencies, including Slint | `Cargo.toml`, `Cargo.lock` | Individual package licenses and distribution model require review before public release |

The font bundle contains Sans Regular, Medium, and Bold, and Serif Regular and Bold. The retained original notice also mentions additional source faces that are not bundled here.

Official signing WASM is not redistributed here. Runtime acquisition checks the pinned digest and remains separate from account data. Public asset URLs are not represented as a redistribution grant.

## Aidoku native JPEG processing

The isolated JPEG process owner, conservative entropy admission and shared C
transaction in `src/native_media/`, `src/bin/kindle-jpeg-worker.rs`,
`scripts/native-codec-probe.c` and `scripts/native-codec-bench.h` are adapted
from AidokuForKindle's uncommitted 2026-11-15 media candidate under
GPL-3.0-or-later. The complete original license, unchanged selected-source
snapshots, base-commit identity, handoff-declared candidate fingerprint,
per-file SHA-256 records and BiliComics changes are retained in
`vendor/aidoku-native-media/`. This source-specific notice does not select an
overall project license. Aidoku performance and physical-device results do
not establish BiliComics acceptance; image processing and Y4 display behavior
retain independent qualification boundaries.

## Aidoku spatial grayscale dithering

The versioned Bayer8 and Q8 Floyd-Steinberg implementation in
`src/reader/spatial_dither.rs` is adapted under GPL-3.0-or-later from the
operator's AidokuForKindle candidate recorded on 2026-11-16. The original
module, complete license, input/expected PNGs, per-file hashes, and scoped
research evidence are retained unchanged in
`docs/research/aidoku-spatial-dither-20261116/`. The declared complete upstream
candidate fingerprint is
`25df24627f3db1dc3a334b1af6f30246744e3fda9a679191e15711f43c4fde97`;
its base commit alone does not reproduce those uncommitted inputs. BiliComics
adapts cancellation and final-viewport integration independently. Aidoku's
Scribe results do not establish BiliComics hardware or optical acceptance.
