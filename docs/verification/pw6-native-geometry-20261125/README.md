# Paperwhite 6 native framebuffer geometry

Paperwhite 6 is the 2024 Kindle Paperwhite (12th generation, Sangria).
The desktop preview continues to use the [design handoff](../../DESIGN_SOURCE.md)
canvas. Native initialization instead checks the visible framebuffer and requires
zero display offsets. The existing model/firmware allowlist is unchanged.

| Boundary | Logical pixels | Display or capture pixels |
| --- | --- | --- |
| Desktop design at 2× | 632 × 840 | 1264 × 1680 |
| Native Paperwhite 6 at 2× | 636 × 848 | 1272 × 1696 |

The stock [Amazon 5.20.1 firmware](https://s3.amazonaws.com/firmwaredownloads/update_kindle_all_new_paperwhite_12th_5.20.1.bin)
was examined through the prepared firmware emulator's unmodified boot FIT on
`test-env`. The Sangria FIT configuration selects `fdt-mt8113-sangria`; the
Scribe/Barolo tree in the same image was not used to identify this display.
In the Sangria tree, `/hwtcon_v2@14000000/epd` has `panel_width = 1272`,
`panel_height = 1696`, `display_width = 1272`, and `display_height = 1696`.
The touch node `/i2c@1001e000/focaltech@38` has
`focaltech,display-coords = <0 0 1272 1696>`. The old application's
[emulator report](../../uiux/baselines/2026-11-25-design-handoff-review/firmware-preview/pw6/display-helper.txt)
had already observed that native visible size, but rejected it because its
expected size came from the design canvas.

The [device profiles](../../../src/platform/profile.rs) now expose separate
values, and the [native framebuffer gate](../../../src/platform/native.rs) uses
only the native expectation. No extra rows are treated as padding, and the
application does not accept either resolution interchangeably. Earlier design
and firmware captures remain unchanged.

## Validation

On `test-env`, 43 Rust platform tests, the full-window reader viewport test,
and 17 framebuffer-harness tests passed. They cover the native expectation,
the separate desktop design size, rejection of the old design-sized
framebuffer, and continued rejection of panning. Fresh
[desktop Shelf and Reader captures](desktop.json) were pixel-identical to the
preceding candidate on both Paperwhite 6 and Scribe, at their respective 2×
design sizes.

The [Paperwhite 6 firmware run](emulator/result.json) built and executed the
ARM application on an isolated 5.20.1 guest. The
[native display helper](emulator/display-helper.txt) and the
[application](emulator/application.txt) both accepted the observed 1272 × 1696
framebuffer with offsets `(0, 0)` at 2×. The first display submission then
returned `EINTR` (`Interrupted system call`, errno 4). The existing delivery
guard stopped further writes; the application exited before reaching Shelf.
The guest was stopped. This verifies the geometry fix, not native UI startup
or physical-device support. No retry or delivery policy was changed.

To reproduce, cross-build `bilicomics` on `test-env` for
`armv7-unknown-linux-musleabihf` with `--no-default-features --features kindle`.
Use the [firmware walkthrough instructions](../../uiux/baselines/2026-11-25-design-handoff-review/README.md#reproduce-the-firmware-walk)
with the new executable, its matching launcher and `--profiles pw6`. The
walkthrough runs against a disposable firmware overlay in a loopback-only
namespace. It does not install to a physical Kindle or measure panel refresh.
