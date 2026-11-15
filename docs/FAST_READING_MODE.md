# Persistent fast reading

The operator requested parity with Aidoku's fast-mode behavior on 2026-11-16:
each page change uses one nonflashing DU update and retains that image after
input and idle. There is no delayed grayscale replacement or automatic
periodic clearing. Explicit manual clearing remains available. Menus and
dialogs continue to use grayscale updates.

## Settings and pixels

Reading settings provide Normal and Fast choices independently of the saved
full-clearing interval. Fast temporarily suspends that interval while retaining
its saved value. Existing preferences default to Normal. The global default and
per-comic choice are persisted in the existing account preference namespaces.
Account reading settings configure the default for comics without an existing
override. The reader settings control the open comic and preserve its current
page, fit, zoom, rotation, and source anchor.

A fast frame is generated from the source Gray8 canvas using ordered binary
dithering. It contains only black and white pixels; it is not first reduced to
the normal sixteen-tone palette. Canonical and processed Gray8 source caches
remain immutable, while prepared-frame identities include the selected mode.
Fast images stay binary until another intentional publication replaces them.

Hardware Y4 remains a saved normal-mode preference. Fast frames do not request
it. Changing a pending setting does not reclassify the old retained image:
processing and refresh routing follow the frame actually presented. Normal
mode restores its saved grayscale and dithering choices.

## Refresh ownership

The platform distinguishes ordinary scenes, content-only fast reading, and fast
reading with grayscale controls. Quiet fast pages route automatic reader and
generic UI requests to DU without advancing periodic clearing budgets. This
prevents a same-frame AUTO notification from replacing the fast request.

Overlays use their normal grayscale policy, and explicit manual clearing retains
GC16. General backend waveform precedence and retained Gray8 protection remain
in place. A bounded queue of only nonflashing DU dirty requests can coalesce
without introducing a flashing full update; mixed grayscale/manual requests
retain the stronger safety policy.

Opening or dismissing grayscale controls is outside the quiet-page guarantee.
An unconsumed grayscale control request retains its stronger waveform when it
shares a render batch with the return to binary reading. Subsequent quiet page
turns use DU again. No timer is added to restore grayscale after a turn, release,
or idle period. Lifecycle, rotation, and uncertain-delivery recovery retain
their existing display protections.

Complete canvas rewriting is separate from the chosen waveform. A transition
out of hardware Y4 still rewrites the complete mapping. DU is eligible only when
the rewritten RGB scene is verified binary and no lifecycle, rotation, sleep or
explicit grayscale clearing requirement takes precedence. Existing marker
completion barriers and uncertain-delivery handling are unchanged.

## Verification scope

Builds, application and isolated backend tests, and real desktop XTest captures
run on `test-env`. Previous UI/UX baselines remain immutable; reviewed settings
changes are recorded in a new dated supplement. A desktop profile or passing
policy test does not establish physical waveform delivery or optical results.

The [2026-11-16 verification record](FAST_READING_MODE_VERIFICATION_20261116.md)
binds the tested sources, executables, policy tests, and ARM package. The
[settings and binary-page supplement](uiux/baselines/2026-11-16-fast-mode/README.md)
preserves twenty actual XTest captures and eight cold-restart checks. No new
physical-device acceptance is claimed for this candidate.
