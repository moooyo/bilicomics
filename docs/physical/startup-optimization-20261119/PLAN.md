# Native Library startup optimization comparison

This plan was set before the new candidate was timed. The comparison is against
the previously packaged implementation, not against the medians of an earlier
session. [The earlier study](../library-to-shelf-20261119/README.md) defines the
native click and Shelf frame endpoints.

## Changes under test

- Start the existing power observer sooner and overlap the full two-second
  settle with bounded production log setup. Both must finish before sampling
  native presentation or taking display/input ownership. Do not treat a live
  child, a private FIFO, or an active-app string as proof that the window
  manager or power event source has finished handoff.
- For startup directly into the ordinary Shelf, avoid an event-loop yield
  between changing the route and projecting that route's initial views.
  Preserve the first application-owned loading frame, startup activity,
  error and suspend behavior, and the final full-frame request. Do not apply a
  no-refresh policy to ordinary navigation, a resumed reader, or overlays.

## Acceptance

1. Exercise both changes and their failure paths on `test-env`. Compare the
   same isolated fixture and settings. The candidate must use the changed
   production sources; pin executable, launcher, package and fixture identities.
2. On the shared Scribe, use a fresh lease immediately before the batch, a
   temporary indexed canonical Library entry, and the positively identified
   native card on each launch. No cold boot or cache eviction. Alternate the
   order within neighboring pairs, with one warmup per arm and six planned
   pairs. Retain all cases and failed attempts separately; do not replace slow
   samples or silently change the route to a URI launch.
3. Use device monotonic time immediately before release of the native XTest
   click. Report first accepted full Shelf SEND and checked driver completion
   for that same marker. Report medians for both arms, per-pair differences
   (candidate minus baseline), count of faster pairs and observed ranges. The
   click bracket, first loading screen and `ui_ready` are secondary endpoints.
4. Require the correct final Shelf, clean exit, normal native control,
   restoration of the original tab/installation, and exact lease release.
   Check for a duplicate startup-frame submission and compare final fixture
   imagery. A local stage getting faster is not sufficient to claim faster
   startup. A full-screen submission or driver completion is not an optical
   measurement or a physical power-button/USB test.

Prefer the change only if the total latency and the paired results support it
without losing behavior. Treat the previously suggested one-second reduction
as a target, not a condition for reporting an honest result. This comparison
measures the combined change, not independent causal speedups for each patch.
