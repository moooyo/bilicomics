# Physical immersive-reader verification: 2026-11-05

The retained evidence supports the listed immersive-reader checks on one first-generation Kindle Scribe running firmware `5.19.5`, using the frozen `0.1.4` R5 candidate. The controlled short sleep/wake sequence, first finger input after wake, and menu overlay checks passed. The shared device was restored and returned at Beijing time `2026-11-05T23:24:42.461761+08:00`.

An earlier live suspended-state checkpoint was not obtained. That attempt remains unaccepted for its live check, with its recovery and unknown sampling chronology preserved. The later successful historical-before/after method does not rewrite it as a pass. This is limited candidate-specific acceptance, rather than a general hardware-support certification.

## Candidate boundary

| Item | Recorded value |
| --- | --- |
| Application version and round | `0.1.4`, `R5` |
| Application source inputs | `340` |
| Source-input manifest SHA-256 | `4dcf953fa4786f842a8f58faad42e01fb57ed8a3bc023252265b1a71e73c3d91` |
| Pipeline-input manifest SHA-256 | `025cbf090c8bcc102b2f970c6fb02db180516e5cb9c4541954d9f629c574421b` |
| ARM executable SHA-256 | `684ba4221830a24199d4c9f475383c8e16bc25c062e5542d17d6f1b36f08d671` |
| Development ZIP SHA-256 | `cf6c04bee9fcf9c60acb5cf76b6a7cdc10ab9bb5c57f8597558fc3113e773d94` |

The [R5 candidate summary](../artifacts/verification/immersive-reader-20261105/arm-r5/candidate-summary.json) records the fresh ARM build and successful static package checks, with the original R5 inputs and lockfile unchanged during the build. The main ARM build retained two `cc`/`cargo-zigbuild` wrapper warnings containing `aeabi_unaligned.c` and `FileNotFound`; it was not warning-free. Upstream desktop checks remain a separate evidence class.

The [earlier 0.1.3 physical report](PHYSICAL_VERIFICATION.md) remains unchanged. Its results do not certify this R5 candidate. This report does not transfer acceptance to later source changes or other builds.

## Device and scope

| Item | Observed value |
| --- | --- |
| Device | First-generation Kindle Scribe |
| Firmware and profile | `5.19.5`, `scribe` |
| Visible framebuffer | `1860 x 2480` pixels |
| Virtual framebuffer | `1872 x 4960` pixels |
| Stride | `1872` bytes |
| Pixel format | 8-bit grayscale |
| Panning offsets | `0, 0` |
| Reported framebuffer `rotate` | `3` |
| Reader configuration exercised | Portrait page reading, `ltr`, `fit=page`, `launch_preference=resume` |
| Data and connectivity | Isolated native `--demo --offline` arguments verified in the checkpoints |

The exact profile/firmware entry was selected only for each supervised isolated run. The profile is a geometry reference; no repository support entry was created. The host harness, watchdog, read-only framebuffer helper, application wrapper, and test-only power-event identity shim formed a controlled supervisor. Their use does not certify every production-launcher PID signal as hardened.

## Run outcomes

| Run | Outcome |
| --- | --- |
| [smoke-01](../artifacts/verification/physical-immersive-20261105/final-safe-export/smoke-01/run.json) | A 30,000 ms ready-clock smoke completed with launcher exit `0` and successful recovery. The operator confirmed the project name, startup feedback and activity indicator, healthy shelf display, automatic return to native Home, and functioning native touch. |
| [reader-01](../artifacts/verification/physical-immersive-20261105/final-safe-export/reader-01/run.json) | Stable full-viewport quiet captures and menu/route traces were retained. The operator confirmed intentional browsing and exit, with native Home and touch normal afterward. Launcher exit was `0`. A menu-open capture attempted after this run ended remains unavailable. |
| [reader2-01](../artifacts/verification/physical-immersive-20261105/final-safe-export/reader2-01/run.json) | The before-sleep view was captured, but the live suspended checkpoint was not obtained. The harness failure-recovery path requested stop; `operator-stop`/`143` and successful recovery were recorded. This live checkpoint remains unaccepted. |
| [sleep-01](../artifacts/verification/physical-immersive-20261105/final-safe-export/sleep-01/run.json) | A controlled short physical power-button sequence retained the same process, 23 identity/anchor fields, geometry, render version, and pixels. The first finger tap after wake advanced page 18 to 19 while remaining in the reader. A subsequent menu overlay retained the source anchor and image geometry. The coordinator then stopped the completed run; `operator-stop`/`143`, recovery, and final native Home/touch confirmation were recorded. |

### Quiet full-viewport rendering and intentional browsing

In `reader-01`, PID `23058`, start ticks `699776600`, and the pinned R5 executable remained unchanged across [quiet-base](../artifacts/verification/physical-immersive-20261105/final-safe-export/reader-01/quiet-base.checkpoint.json) and [quiet-stable](../artifacts/verification/physical-immersive-20261105/final-safe-export/reader-01/quiet-stable.checkpoint.json). Each records frame and viewport dimensions `1860 x 2480`, complete normalized source focus `[0, 0, 1, 1]`, page `18`, `menu=false`, and an empty sheet. The legacy saved footer preference remains `true`; no footer overlay appears in the reviewed quiet images.

The captures share recorded framebuffer-pixel SHA-256 `3e6e476f4ea28b20ada8c9a5552ab41d8fe8a46807f20206fef554c6ba55cd74` and PNG SHA-256 `09829367906806d0356aeabeeca07b321829b1ed0de72645822cc85f918536f7`. Offline visual review found the complete comic frame without persistent application chrome. The [quiet checks](../artifacts/verification/physical-immersive-20261105/final-safe-export/reader-01/quiet-checks.json) retain the whole-viewport and complete-source-focus results. These captures are framebuffer-memory evidence. Separate operator observations in `reader2-01` and `sleep-01` corroborate the physical view; no capture is a panel photograph.

The `reader-01` lifecycle trace shows the menu opening, closing, and opening again, followed by routes `detail`, `shelf`, and `account`. Its final run record includes the operator's explicit confirmation that the browsing and application exit were intentional. No crash, timer-triggered exit, or unintended navigation is inferred.

The original `last_failure.operation=checkpoint`, with message `The run has ended; collect recovery evidence.`, remains in that record. The coordinator identified it as a menu-open checkpoint/capture attempt after the run ended. The successful later menu capture belongs to `sleep-01`; it does not turn the earlier failed attempt into a captured menu. Incidental suspend/resume events in `reader-01` are not counted as controlled physical power-button acceptance.

### Unaccepted live suspended-state attempt

In `reader2-01`, PID `27903`, start ticks `701927671`, had a valid before-sleep checkpoint. The retained failure is `A native lifecycle transition has not suspended the application.` The harness then followed its automatic stop/recovery path, recorded as `operator-stop`, launcher status `143`, and recovery checks passed. The operator later confirmed native Home and touch.

The retained trace contains historical suspend/resume records, but no precise timestamps establish the sampling chronology of the failed live check. The report does not attribute this miss to a timer, crash, or SSH behavior. It also does not claim that a live suspended checkpoint succeeded.

### Controlled sleep/wake through historical before/after evidence

In `sleep-01`, the [before-sleep checkpoint](../artifacts/verification/physical-immersive-20261105/final-safe-export/sleep-01/before-sleep.checkpoint.json) establishes PID `20013`, start ticks `701992733`, and the pinned R5 executable. The operator explicitly pressed the physical power button, observed and held the native screensaver, then woke and unlocked with the physical power button. The coordinator recorded sending no Kindle commands during that confirmed hold.

After wake, the [retained checkpoint](../artifacts/verification/physical-immersive-20261105/final-safe-export/sleep-01/wake-retained.checkpoint.json) and [sleep-retention checks](../artifacts/verification/physical-immersive-20261105/final-safe-export/sleep-01/sleep-retention-checks.json) bound the same PID-filtered trace from count `4` to count `6`. The only two records strictly after the baseline and through the wake checkpoint are [suspend](../artifacts/verification/physical-immersive-20261105/final-safe-export/sleep-01/historical-suspend.json) followed by [resume](../artifacts/verification/physical-immersive-20261105/final-safe-export/sleep-01/historical-resume.json). Both record page `18` and a saved checkpoint; their suspended flags are respectively `true` and `false`.

All 23 named reader identity/anchor fields were retained, including the comic, episode, revision, reader generation, route, launch preference, settings, page ID/index, crop, source position, and zoom. Frame and viewport stayed `1860 x 2480`, normalized focus stayed `[0, 0, 1, 1]`, and `render_version` stayed `1`. Before-sleep and wake-retained framebuffer pixels share SHA-256 `3e6e476f4ea28b20ada8c9a5552ab41d8fe8a46807f20206fef554c6ba55cd74`; the PNG hashes also match. Visual review found the same quiet comic image, and the operator explicitly confirmed the same physical image without returning to the shelf.

This method deliberately uses the bounded historical trace after wake, rather than live device sampling during the screensaver hold. It establishes this short operator-assisted native screensaver/power-button sequence. It does not measure sleep duration, deep system suspend, long-term retention, or power consumption.

### First finger input and menu overlay after wake

The operator explicitly confirmed that the first reader-body finger tap after wake, on the right middle, advanced to the next comic image and remained in the reader. The [wake-first-finger checkpoint](../artifacts/verification/physical-immersive-20261105/final-safe-export/sleep-01/wake-first-finger.checkpoint.json) retains PID `20013` and its birth/executable identity, changes page index `17` to `18` and page ID to the page-19 ID, and records `render_version=2`, `menu=false`, and route `reader`.

A centered tap then revealed the top status and bottom progress/tools. The operator confirmed the menu was normal and the image did not rescale. The [menu-open checkpoint](../artifacts/verification/physical-immersive-20261105/final-safe-export/sleep-01/menu-open.checkpoint.json), [actual menu capture](../artifacts/verification/physical-immersive-20261105/final-safe-export/sleep-01/menu-open.png), and [menu-overlay checks](../artifacts/verification/physical-immersive-20261105/final-safe-export/sleep-01/menu-overlay-checks.json) retain the source anchor, all geometry, and `render_version=2`. Unoccluded image pixels in the recorded comparison region `[0, 200, 1860, 2060]` are identical to the quiet page-19 image. The menu is an overlay; the comparison does not claim that pixels covered by its controls are unchanged.

## Final restoration and shared-device return

The coordinator explicitly stopped the completed `sleep-01` run. Its `operator-stop`/`143` result, successful recovery checks, and final operator confirmation of native Home and functioning native touch are retained.

The [final restoration record](../artifacts/verification/physical-immersive-20261105/final-safe-export/final-restoration.json) reports checking 32 recorded PID/birth/executable identities, with zero remaining owned identities and an absent BiliComics launcher lock. The [native state text](../artifacts/verification/physical-immersive-20261105/final-safe-export/final-native-restored.txt) records `com.lab126.KPPMainApp` in the foreground, running statusbar/powerd/lab126_gui services, and awesome in state `S`. Both coordinator mux sockets were absent and its device SSH connections were closed. The restoration record's lease-held flag refers to the step before release.

The [acquisition record](../artifacts/verification/physical-immersive-20261105/final-safe-export/lease-acquired.json) records an atomic acquisition at UTC `2026-11-05T08:53:01.664233+00:00`, Beijing time `16:53:01.664233+08:00`, after prior ownership was removed. The [return record](../artifacts/verification/physical-immersive-20261105/final-safe-export/lease-return.json) records release at UTC `2026-11-05T15:24:42.461761+00:00`, Beijing time `2026-11-05T23:24:42.461761+08:00`.

Both records bind owner thread `01a1a209-30fb-7ef0-b27c-58d1b4b4d494`, run `20261105-immersive-6e3027af`, and public lease-token SHA-256 `07dd7c42118d0bd0fb4af68ee979c0ae06e64a6e92530d5351783200b7a9be4c`. Release checked owner and token, removed only this owner's JSON, then removed the directory; the directory was absent immediately afterward. The old controller's acquisition path was disabled. No raw token or credential is reproduced here.

The [direct peer acknowledgement](../artifacts/verification/physical-immersive-20261105/peer-return-acknowledgement.json) records an incoming Codex app coordination message from WeRead thread `01a19d47-c440-7e92-a7d0-f8bb928f4dc3` after release. The peer explicitly acknowledged the 0.1.4 test return and the native Home/touch confirmation, stated it would acquire a fresh exclusive lease, and requested no further BiliComics device operations. This is confirmed receipt of the return coordination; it does not independently observe the peer's later installation or tests. The lease-return JSON identifies the recipient, while the separate acknowledgement record supplies the receipt evidence. Further BiliComics device work requires a fresh explicit exclusive handoff.

## Evidence provenance and limits

The [final safe export manifest](../artifacts/verification/physical-immersive-20261105/final-safe-export/evidence-manifest.json) indexes 76 payload files covering the completed runs, operator confirmations, restoration, and returned lease. Including the manifest itself, the export contains 77 files. Private tokens, credentials, host keys, private control files, and native Home images were excluded. The initial safe export is preserved as an earlier snapshot; final conclusions use the newer local final export, including the operator-confirmed intentional exit in `reader-01`.

A separate [framebuffer visual-review record](../artifacts/verification/physical-immersive-20261105/framebuffer-visual-review.json) lists four existing PNGs inspected with `view_image`, including the post-wake quiet reader and menu-open captures. It reports no visual defect in those images, with legible complete overlays and retained artwork geometry. Its scope is framebuffer memory; operator panel and physical-input observations remain separate.

Acceptance is restricted to this R5 candidate, this single Scribe/firmware pair, offline demo data, portrait `ltr` page reading with `fit=page` and `launch_preference=resume`, and the short controlled sequence described above. The ordinary-startup `shelf` preference variant, other reader configurations, real accounts, live services, network downloads, purchases or other transactions, USB behavior, performance, deep/long-duration sleep, power consumption, other devices, and other firmware versions are not certified. Startup feedback and activity-indicator acceptance is an explicit operator observation, without a timing or refresh-rate measurement.

The earlier draft used existing public files on the `test-env` host. Final closeout used only the local final safe export and existing PNGs. No local test or validation suite, new device test, device SSH/helper/physical-CLI call, or lease operation was performed by the report author for this closeout. Only this report and its [machine-readable summary](../artifacts/verification/physical-immersive-20261105/summary.json) were updated; original evidence and the old 0.1.3 report were preserved.
