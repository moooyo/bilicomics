# Reader geometry presentation

Named reader fits are discrete choices. The application must retain the
presented image while it prepares the complete target raster, then publish the
target once. It must not interpolate sizes or fade between comic images.

Version 0.1.25 follows the user's corrected fit definitions from 2026-11-14:
Original contains without enlargement, Page stretches independently on both
axes, and Height/Width uniformly fit their named axis with centered placement.
See [the fit contract](UI_REFINEMENT_20261113.md#reading-controls) for the exact
geometry. Earlier physical fit observations use the superseded geometry.

## Application boundary

- Selecting the current fit at a unit zoom multiplier does not cancel matching
  work, increment the reader version, or prepare another frame. Selecting the
  same fit at another multiplier still resets that multiplier to one.
- Superseded preparation results never replace the retained image. Source-page
  and crop anchors continue to use the presented core until replacement.
- A ready frame whose fit or zoom differs from the presented core requests
  `ReaderGeometry`. This always produces a complete GC16 refresh, independently
  of the periodic page-turn policy, and restarts its partial-refresh budget.
- Ordinary page turns retain their existing refresh policy.

## Native presentation boundary

The software renderer prepares pixels outside the mapped framebuffer. A single
mapped framebuffer must not be rewritten while an accepted EPDC update can
still read it. Complete replacement requests also require a complete redraw,
one accepted full-frame GC16 submission, and a checked driver completion.

Driver marker completion establishes a kernel delivery boundary. It does not
establish optical panel behavior, absence of ghosting, or physical touch
acceptance. Those require an observer at the device.

On a strictly identified, reviewed monochrome Scribe, the read-only native
identity selects the MTK update ABI before the first submission. This immutable
selection is independent of the current image's dithering preference and of a
later Y4 capability rejection. Unknown devices do not inherit an ABI from an
application design profile. An interrupted or otherwise uncertain submission
still blocks further mapped writes and submissions.

## Diagnostic evidence

Opt-in tracing associates the requested fit, target and presented geometry,
source-page and crop indices, render version, the allowlisted refresh ABI,
EPDC marker identities, and
completion waits. Isolated synthetic `--demo` runs additionally record prepared
RGBA SHA-256 and native RGB fingerprints; these are neither computed nor
recorded for real-account traces. Fingerprints also require enabled tracing,
and the backend disables fingerprinting whenever its callback is replaced or
cleared. No account or comic identifiers, source URLs, or raw image bytes are
added to these records.

## Physical observation and scope

The first supervised Scribe batch on 2026-11-13 verified the final geometry of
the original-pixel fit. The user also reported that a fit change appeared to
pass through several intermediate sizes. Its collected software trace shows
one prepared raster and publication per request, but lacks per-native-frame
content fingerprints and driver-completion observations. The visual transition
remains unresolved by that evidence.

The presentation changes close a synchronization gap identified during that
investigation. They are not, by themselves, proof of the reported root cause.
Remote regression results and a fresh supervised device batch must be recorded
separately from the earlier implementation and physical evidence.

The R2 native startup attempt on 2026-11-14 exited before reader readiness:
the first update attempt returned `EINTR` and the conservative delivery guard
stopped the process. Its recorded native identity already established MTK
eligibility, but ABI preselection had depended on the image requesting Y4.
That compatibility path requires the identity-based preselection above. R2
does not establish fit or transition acceptance. Native control and the exact
owned lease were restored and released after the failed attempt.

R3 selected MTK before the initial update and recorded accepted/completed
markers on the scoped physical Scribe. A stable-direction run captured the
first two source pages. A later wide-source run captured its initial complete
page; the user reported that all four previous presets changed directly to
their target sizes. Its next capture raced a new reader request, so the test
wrapper rejected that candidate and restored/released the device. That test
shutdown is separate from application failure. The bounded capture retry
changes preserve stale-frame rejection while allowing a valid same-page view
change to settle within the original deadline.

The user's subsequent geometry clarification invalidates those earlier preset
definitions as acceptance for version 0.1.25. Its new mathematical, application
pixel, desktop UI, and physical evidence must retain separate identities.

Version 0.1.25's corrected geometry subsequently passed native framebuffer
and independent source-pixel checks on Scribe 2022, firmware 5.19.5, upright
1860 by 2480. The source was the isolated 3400 by 2360 wide fixture. Original
and Width drew the complete source at 1860 by 1291 with centered vertical
whitespace. Page stretched the full source to 1860 by 2480. Height selected
the centered horizontal source window, approximately x=815 through 2585,
and uniformly filled the viewport height.

Original/Page and Height/Width were captured in two independent owned runs;
the former had controls visible and the latter had controls hidden. Their
run identities remain separate. The user confirmed Width's centered top and
bottom whitespace and a single Height-to-Width size change with no intermediate
sizes. Every completed batch restored native control and released its exact
owner/token lease. These results do not establish Paperwhite physical support,
optical panel photography, or physical small-source acceptance.
