# Bundled interface fonts

The five canonical gzip files remain the source assets. The build expands each
face into `OUT_DIR/fonts` with the existing 64 MiB uncompressed-size limit and
gzip checksum validation. Generated payloads are rewritten only when their bytes
change. No font subsetting, outline transformation, family rename, or weight
change is part of this startup optimization.

| Source | Requested family | Weight |
| --- | --- | --- |
| `NotoSansSC-Regular.otf.gz` | `Noto Sans CJK SC` | 400 |
| `NotoSansSC-Medium.otf.gz` | `Noto Sans CJK SC` | 500 |
| `NotoSansSC-Bold.otf.gz` | `Noto Sans CJK SC` | 700 |
| `NotoSerifSC-Regular.otf.gz` | `Noto Serif CJK SC` | 400 |
| `NotoSerifSC-Bold.otf.gz` | `Noto Serif CJK SC` | 700 |

`src/fonts.rs` embeds the generated OpenType payloads in the application and
shares immutable static slices with Fontique. The small reference-counted handles
do not copy the font payloads. Runtime font parsing and raster caches still have
their own costs. A larger ELF and installation archive may affect cold-start
mapping and storage costs; warm-start measurements alone cannot establish that
tradeoff.

The existing 22,040-byte `StartupSansSC.otf` bootstrap face also shares its static
bytes. Its source, retained names, exact hash, character coverage, and generation
process remain in `startup-manifest.json` and `prepare_startup_font.py`. The
complete application families are selected only after every full face has been
registered. Per-face ordering, cancellation, and delivery errors retain their
existing behavior.

This implementation adapts the build-time expansion and static ownership pattern
from AidokuForKindle commit
[`e141d4479ae6440e2ba3f7b93d8f0e616458055f`](https://github.com/moooyo/aidokuForKindle/commit/e141d4479ae6440e2ba3f7b93d8f0e616458055f).
BiliComics retains its existing five-face set and public font-loading API. The
font tests compare every generated payload byte with its canonical gzip source,
verify static pointer sharing, and cover interrupted or rejected delivery.

Original font licenses and copyright notices remain in `OFL.txt`,
`NotoSansCJK-LICENSE.txt`, `NotoSerifCJK-LICENSE.txt`, and `NOTICES.txt`.
