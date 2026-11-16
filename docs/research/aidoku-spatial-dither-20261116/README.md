# Aidoku software dithering implementation handoff

Date: 2026-11-16.

The user requested the completed Aidoku dithering capability and matching
settings in BiliComics. Aidoku has implemented the following mutually exclusive
choices and deployed them to the physical Scribe. This directory supplies
reference code, lossless fixtures, provenance and scoped verification evidence
for the BiliComics implementation.

| Choice | Algorithm | Product behavior |
| --- | --- | --- |
| Off | Nearest native level | No added spatial pattern; preserve the user's disabled choice across restart |
| 8x8 Bayer | Ordered 64-position lookup, 16 KiB | Default software option; fast and periodic texture |
| High-precision Floyd-Steinberg | Serpentine Q8 error diffusion, remainder-conserving neighbor weights | Optional finer spatial error distribution at higher CPU cost |

All three output the same native codes `0,17,...,255`. The two dithers represent
additional regional means through neighboring-pixel distribution. They do not
change the panel's physical per-pixel gray depth. Both qualified prototypes
produced 256 strictly increasing means in the aligned 256-patch Scribe test.

## Implementation requirements

1. Add the same three settings and persist the selection. CPU dithering does
   not require a hardware-capability gate. Target PW5, PW6 and Scribe.
2. Apply the selected method after final physical-size resizing, cropping and
   contrast operations. Preserve the current image color/alpha conversion.
3. Start with [the reference module](spatial_dither.rs). Adapt its
   `super::check_decode_cancelled` hook to BiliComics cancellation. Retain bounded
   checks within rows, the Bayer `(x,y)` phase and Q8 remainder conservation.
4. Include the algorithm/version in processed, resident, retained and prefetched
   image identities. Aidoku uses `off`, `bayer8_v1` and
   `floyd_steinberg_q8_v1`; original decoded sources can remain shared.
5. Cancel obsolete rendering on selection changes and reject late results.
   Test next-page prefetch, previous-page retention and cold restart.
6. Do not layer Y4 or a second software dither over these output pixels.
   Aidoku migrates older enabled software/Y4 preferences to Bayer8 and keeps
   older disabled preferences off. Adapt migration to BiliComics storage without
   unconditionally reenabling dithering at startup.
7. Preserve the existing persistent binary Fast mode. Save this grayscale choice
   for normal reading; keep manual-only clearing and do not reintroduce delayed
   grayscale settlement or periodic full clearing in Fast.
8. Keep JPEG hardware acceleration independent. Existing decoder admission and
   DMA/worker lifecycle protections are not changed by this feature.
9. If displaying a test strip, generate it through the selected production
   algorithm at physical resolution. Do not use already dithered fixtures as
   normal input or interpret a histogram as an optical panel measurement.

## Fixtures and results

- [Original Gray8 input](input-gray8.png), 1860 x 2480.
- [Expected Off output](expected-off.png).
- [Expected Bayer8 output](expected-bayer8.png).
- [Expected Floyd-Steinberg Q8 output](expected-floyd-steinberg-q8.png).
- [Native application verification](native-results.json).
- [Production/prototype equality](algorithm-equivalence.json).
- [Scribe CPU benchmark samples](scribe-cpu-timings.json).
- [Source snapshot](source-manifest.json) and [copied file identities](UPSTREAM_FILES.json).
- [Upstream license](LICENSE.AIDOKU).

The production Reader snapshots matched all three expected outputs exactly,
including a cold-start Floyd-Steinberg run. Y4 was available on that Scribe but
was neither requested nor effective. All four native settings-strip comparisons
also had zero differing pixels. The final batch passed 11 native runs and kept
seven consecutive Fast page updates on DU without an additional idle update.

The earlier physical CPU benchmark medians were 55.84 ms for Bayer8 and
194.01 ms for Floyd-Steinberg Q8. These include resident PNG decoding, identity
fitting, allocation and dithering; they exclude file I/O and screen refresh.
The separately named host timing file is not a Scribe performance result.
Optical appearance still depends on the panel, waveform, lighting and artwork.

The complete integration contract and frozen UI evidence are in the sibling
AidokuForKindle checkout under `docs/SOFTWARE_DITHER_SETTINGS_20261116.md` and
`docs/review/software-dither-settings-20261116/`. The qualified source snapshot
is `25df24627f3db1dc3a334b1af6f30246744e3fda9a679191e15711f43c4fde97`;
its base commit is `a53b9716d64fe5521fac38431b24732c75cb8ff7` plus recorded
uncommitted implementation changes. The copied module and fixtures are frozen
reference inputs, not a claim that BiliComics integration is already complete.

Run BiliComics builds, tests and UI checks on `ssh test-env`, with Cargo jobs
limited to six. Physical Paperwhite behavior and BiliComics integration need
their own validation; Aidoku's Scribe evidence must retain its original scope.
