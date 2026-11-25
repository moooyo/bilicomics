# November 25 handoff: Scribe physical smoke

The updated UI passed one isolated offline smoke run on a physical
first-generation Kindle Scribe at firmware 5.19.5. This result is separate from
the [desktop and firmware-emulator review](../../uiux/baselines/2026-11-25-design-handoff-review/README.md).
The same ARM executable passed the Scribe 5.20.1 firmware-emulator navigation
preflight on `test-env` before device access. The source record is the working
snapshot based on `32380fc`, including the Paperwhite 6 geometry correction.

## Observations

- The normal demo [Shelf](shelf.png), [full-screen directory](directory.png),
  [Reader settings](reader-settings.png), and [Account](account.png) rendered in
  the device framebuffer at 1860 × 2480. Returning from the directory restored
  the existing page 18, with identical framebuffer pixels.
- Native `powerd` requests advanced the device through suspend and wake. The
  same application process retained page 19 and its source anchor;
  [before sleep](reader-before-sleep.png) and [after wake](reader-after-wake.png)
  have identical framebuffer pixels. A subsequent application turn reached
  [page 20](reader-after-turn.png) and changed the rendered image.
- The generation-bound launcher log confirms that the custom cover was
  retained and that its wake barrier was accepted. The live collector did not
  capture the cover frame: its immediate check ran before the launcher had
  finished processing the suspend acknowledgement. The original
  [runner result](result.json) leaves that observation false;
  [post-run verification](verification.json) records the launcher evidence
  independently. It is not proof of optical cover appearance or long idle hold.
- The launcher exited normally. Native foreground and services were restored,
  the installed application and Library paths were unchanged, and the isolated
  stage was removed. The [lease release](release.json) followed verified
  restoration and closure of the owned SSH connections. Peer notification was
  sent after release.

The images are read-only native framebuffer captures projected according to the
observed `Y8` output mode, not camera photographs. Scripted application actions
exercised navigation and page turning; they do not prove physical touchscreen
hit testing. The run used a temporary launcher entry and did not open the
ordinary native Library card or measure time from card release. It did not use
a real account, send transactions, install the candidate permanently, or test
a physical Paperwhite 6.
