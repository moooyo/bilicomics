# November 25 design handoff: review corrections

This supplement follows the [original 215-state capture](../2026-11-25-design-handoff/README.md).
The original captures are unchanged. The review used 24 side-by-side sheets and
inspected the originals where a thumbnail could conceal a difference. The
references are the Paperwhite 6 and Scribe portrait states in the supplied
[November handoff](../../../DESIGN_SOURCE.md), not a new product specification.

## Corrections

- Search and Details share the decision to restart a completed comic. The search
  result opens Details across the full row except where its explicit reading
  button takes precedence.
- Locate in Details and the Reader directory changes only the page within the
  selected sort and filter. It is hidden if the filter excludes the current
  chapter. Batch selection has its own touch area above the chapter controls;
  page counts reflect the available height in that mode.
- The Reader toolbar uses sliders for settings and a circular refresh arrow.
  The demo's sleep page uses the existing unlettered cover artwork and draws
  the title once; production cover preparation and wake ownership are unchanged.
- Confirmed sign-in stays on the success card until the next action. Login
  returns to its source, while onboarding advances to reading preferences.
  The fixture injects a confirmed presentation state, not a real account.
- The Scribe second-page check adds enough fictional favorites in a fresh demo
  tree to make the grid span two pages. The default demo, like the supplied
  Scribe reference image labeled `shelf-p2`, fits on a single grid page.

## Evidence and limits

[Verification](verification.json) separates desktop application replay, real
pointer input, synthetic presentation fixtures, and the ARM firmware emulator.
The desktop captures are at 1264 × 1680 and 1860 × 2480, both at 2× scale.
The corrected build produced [215 dimension-matched pairs](scene-index.json),
passed all [175 controller replays](replay-report.json) and ten pointer flows,
and passed 901 library tests (seven ignored) and five selected integration tests.
Four additional landscape replays opened Reader settings and the chapter
directory at the swapped native design dimensions:
[Paperwhite 6 settings](landscape/pw6/settings/window.png),
[Paperwhite 6 directory](landscape/pw6/directory/window.png),
[Scribe settings](landscape/scribe/settings/window.png), and
[Scribe directory](landscape/scribe/directory/window.png).
Login and payment fixtures remain separate from actual input and upstream services.

The [firmware runner](../../../../scripts/verification/handoff_firmware.py)
uses disposable overlays, a loopback-only network namespace, and the real ARM
application and launcher. Its guest installs under an executable private
`/var/local` directory without remounting the stock no-exec userstore. It does
not simulate a release of the ordinary Kindle Library card or measure optical
refresh. The Scribe VM uses a synthetic model identity. The earlier ARM run
preserves its [Shelf](firmware-preview/scribe/shelf.png),
[Directory](firmware-preview/scribe/directory.png), and
[Reader settings](firmware-preview/scribe/reader-settings.png) frames. The
Paperwhite 6 VM's 1272 × 1696 framebuffer was rejected by the existing
1264 × 1680 admission check, as recorded in the
[display helper log](firmware-preview/pw6/display-helper.txt). The guard
and shipping allowlist were not changed.

The corrected ARM build passed the [Scribe navigation run](firmware-final/navigation/result.json):
[Shelf](firmware-final/navigation/shelf.png), Reader, the
[full-screen directory](firmware-final/navigation/directory.png), return to page
18, [Reader settings](firmware-final/navigation/reader-settings.png), Account,
and a normal exit that restored native control on the same guest boot.

A separate [sleep attempt](firmware-final/sleep-attempt/result.json) did not
observe a firmware suspend event after a powerd request; it does not qualify
suspend, wake, or custom-cover delivery on this emulator. The sleep presentation
fixture, application sleep UI test, and three launcher lifecycle fixtures passed
independently.

The supplemental PNGs are original application captures, not side-by-side
thumbnails. Login, purchase and recharge success do not establish that any
live service performed those operations. No physical Kindle was accessed.

| Corrected surface | Paperwhite 6 | Scribe |
| --- | --- | --- |
| Search results | [PNG](raw/pw6/search-results.png) | [PNG](raw/scribe/search-results.png) |
| Details: descending and located | [PNG](raw/pw6/detail-locate-desc.png) | [PNG](raw/scribe/detail-locate-desc.png) |
| Details: chapter selection | [PNG](raw/pw6/detail-sel-all.png) | [PNG](raw/scribe/detail-sel-all.png) |
| Directory: unread filter | [PNG](raw/pw6/directory-unread.png) | [PNG](raw/scribe/directory-unread.png) |
| Reader toolbar | [PNG](raw/pw6/reader-menu.png) | [PNG](raw/scribe/reader-menu.png) |
| Login success fixture | [PNG](raw/pw6/login-success.png) | [PNG](raw/scribe/login-success.png) |
| Sleep cover fixture | [PNG](raw/pw6/sleep.png) | [PNG](raw/scribe/sleep.png) |
| Book grid, page two | [PNG](raw/pw6/shelf-p2.png) | [PNG](raw/scribe/shelf-p2.png) |

### Reproduce the firmware walk

On `test-env`, use an isolated source workspace and one Cargo job. Build
`bilicomics` with the `kindle` feature for `armv7-unknown-linux-musleabihf`.
Supply the reviewed QEMU installation and its read-only 5.20.1 base image root,
the new ARM executable, and the launcher from the same source snapshot. Choose
a short output path because QMP uses a Unix-domain socket:

```sh
unshare --net sh -c 'ip link set lo up; exec python3 -B "$@"' sh \
  "$source/scripts/verification/handoff_firmware.py" \
  --emulator "$emulator" --binary "$candidate" \
  --launcher "$source/packaging/kindle/launch.sh" \
  --output "$fresh_output" --profiles scribe
```

The runner stops its own guest on exit. The host network, the prepared device
images, and any shared physical Kindle lease are not modified.
