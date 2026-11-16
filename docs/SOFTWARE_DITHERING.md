# Versioned software grayscale dithering

BiliComics provides mutually exclusive Off, 8x8 Bayer, and high-precision
Floyd-Steinberg choices in default reading settings and per-comic reader
settings. They are CPU capabilities on every supported UI profile, independent
of Y4 or JPEG hardware availability. New installations use Bayer8.

## Processing and settings

Off rounds to the nearest native code. Bayer8 uses a two-dimensional ordered
pattern and a 16 KiB lookup table. Floyd-Steinberg uses signed Q8 serpentine
error diffusion and conserves the final neighbor remainder. All three produce
only `0, 17, 34, ..., 255`; additional regional means come from pixel
distribution. They do not increase independently addressable panel gray depth.

The selected algorithm runs once on the final physical-size Gray8 viewport,
after existing source conversion, resizing, rotation, cropping, and placement.
Only then is the result expanded to opaque RGBA. Window physical dimensions
and actual scale factor determine the raster; final CPU-tone images use nearest
sampling in Slint to preserve their native codes and spatial pattern.

The stored `dither` boolean retains an explicit disabled choice. The last
`dither_method` is `bayer8` or `floyd_steinberg`, including while Off is selected.
Loading an older enabled `software` or `hardware` method migrates it to Bayer8;
an older disabled choice remains Off. Unknown stored methods normalize to
Bayer8. Startup does not force the boolean on. Default changes retain existing
per-comic overrides.

These product choices do not request Y4 and are never layered with another
spatial dither. `ReaderSettings.spatial_dither = None` preserves an explicit
low-level Gray8/Y4 research path; product preference construction always uses
the typed CPU path. Historical Y4 evidence keeps its original source scope.

## Cache and publication identity

Final processing identities include `off`, `bayer8_v1`, or
`floyd_steinberg_q8_v1`, together with the reader raster version. They bind
`FrameInput`, prepared next pages, retained displayed cores, and thumbnail
plans. A method change cancels obsolete work and replaces thumbnail proof
aliases. Source identity alone cannot reuse pixels processed by another method.

Canonical decoded Gray8 and native resized Gray8 caches remain shared because
they precede final dithering. Their integrity and processed-source versions are
unchanged. JPEG decoder admission, DMA ownership, deadlines, and cleanup are
independent of this feature.

An active render records its physical viewport. Import checks that viewport
against the current window before accepting the result; resize or scale changes
retire old work and prepare a new complete frame. Pending settings preserve the
old displayed pixels and their processing intent until a matching replacement
is ready. Cancellation is checked during canvas creation, source copying,
dithering, and RGBA expansion; partial frames never escape.

## Persistent Fast mode

Fast continues to produce binary pixels directly from Gray8 using its existing
binary Bayer pattern. It bypasses the selected sixteen-tone algorithm. The
grayscale choice remains saved for Normal, and changing it does not restore
delayed grayscale settlement or periodic full clearing. Explicit manual
clearing and grayscale menus retain their existing policy.

## Provenance and verification scope

The [frozen Aidoku handoff](research/aidoku-spatial-dither-20261116/README.md)
retains the original module, complete GPL license, lossless Gray8 input and
three expected outputs, source identities, and reference-project evidence.
The adapted production module retains its GPL provenance. The source project
was an uncommitted candidate; its recorded base commit alone does not reproduce
the supplied implementation.

BiliComics tests compare its algorithms and complete Reader output with those
frozen expected pixels, and exercise migration, restart, cancellation,
asynchronous publication, cache separation, physical viewport changes, and
Fast-mode continuity on `test-env`. Source-project Scribe results and CPU times
are references; they do not establish BiliComics optical or physical-device
acceptance. This integration does not add a product test strip or use a
pre-dithered fixture as an ordinary comic source.

The [BiliComics verification record](SOFTWARE_DITHERING_VERIFICATION_20261116.md)
binds its own tests, executables, development ZIP and UI captures. The
[reviewed settings supplement](uiux/baselines/2026-11-16-spatial-dither/README.md)
keeps the original X11 PNGs and separate human-review evidence.
