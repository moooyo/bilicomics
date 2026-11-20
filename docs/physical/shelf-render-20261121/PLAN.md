# First Shelf render and the preceding panel update

## Question

In the [closed November 20 comparison](../library-pairs-20261120/README.md),
all 12 measured first Shelf frames waited roughly 329–342 ms for the preceding
update before rendering, then spent roughly 284–455 ms rendering into a private
RGB buffer. These are intervals in the same frame, not differences between
unrelated launch medians. Does letting that **private render** run while the
panel works shorten a real Library-card launch? It is not safe to overlap
writes to the mapped framebuffer or submit the next marker speculatively.

The earlier four launches with four loading frames also took longer between
the first loading SEND and software readiness. That correlation is a separate
question. Do not remove feedback, delay a worker, change the route budget, or
shorten the two-second power-observer settle in this comparison.

## Single change

- **A:** the independently qualified `57265de` application and launcher used
  as B on November 20. The older `d81b529` application is not this control.
- **B:** a new application built from the same production tree plus the
  first-Shelf private-render experiment. Both arms use the same production
  launcher, Library document, JPEG worker, package resources, and closed,
  offline demo seed. Record the actual working-tree delta; do not label it as
  a committed revision or as the already qualified control.
- For B, permit only the ordinary initial Shelf's complete replacement to
  render early. All framebuffer fills and writes still require successful
  completion of the previous update. Submit once, wait for the new update's
  own positive marker, and fail closed on uncertain delivery. Loading frames,
  Reader, overlays, orientation/dithering transitions, suspend and resume do
  not opt into an early render. There is no new background UI thread.

## Gates and interpretation

1. On `test-env`, verify the ordering and independent fallback paths, including
   a previous wait failure leaving mapped pixels untouched and no new SEND.
   Build the actual ARM release configuration; compare package members against
   the qualified control. Rehearse the fixed-entry selector, peer protection,
   and result reader off device before acquiring a lease.
2. Use one fresh, owned host lease for genuine, positively identified native
   Library card clicks on the shared Scribe. Warm up once per arm, then run six
   adjacent, alternating AB/BA pairs. Keep the two warmups separate and retain
   every attempt; no cold-cache claim, URI fallback, or sample replacement.
   Preserve the live WeRead/Aidoku state, restore native control and navigation,
   close SSH, and release the exact lease **before** analyzing outcomes.
3. Use device monotonic time immediately before button-up to the first accepted
   complete Shelf SEND whose render starts after software readiness, and to
   successful driver completion of **that same** marker. This is not optical
   settling. For each measured pair report both B-minus-A endpoints, time
   before Rust, ready-to-SEND, render and residual previous-marker wait. The
   historical trace fingerprint is demo-only instrumentation in both arms.
4. Require the initial feedback and changing activity, identical final logical
   Shelf pixels, and matching visible Gray8 samples for the last pair. A must
   demonstrate the old transaction order and B the new order when eligible.
   A shorter reported wait on its own is not a user-visible improvement.
5. To recommend enabling the candidate, require both endpoints faster in at
   least five of six pairs, a median paired driver improvement of at least
   100 ms, and a shorter median ready-to-SEND within **both** order groups,
   with all behavior gates passing. Report contrary data and withhold promotion
   if the criteria are not met. An unavailable or sleeping device is not a
   timing sample. Do not generalize a single offline fixture to live accounts
   or other hardware.
