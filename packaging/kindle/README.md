# Kindle development packaging

This directory contains a source-level launcher and provisional KPM manifest for BiliComics. No BiliComics device/firmware combination has been accepted. The committed allowlist contains comments only, and the launcher refuses to take over the display until an operator records an exact accepted profile/firmware pair in the runtime data directory.

## Independent installation layout

A development bundle should stage this layout beneath the Kindle USB storage root:

```text
documents/bilicomics.sh
bilicomics/app/bilicomics
bilicomics/app/config/device-profile.example
bilicomics/app/config/device-allowlist.example
bilicomics/app/manifest.draft.json
bilicomics/app/licenses/backend/
bilicomics/app/licenses/pinyin/
bilicomics/data/
```

Stage `launch.sh` as `documents/bilicomics.sh`. The application executable must be the Linux ARM Kindle build, rather than a Windows or desktop executable. The reference build target is `armv7-unknown-linux-musleabihf`; target buildability and device acceptance must be established separately for this application.

The executable, bundled resources, and notices belong under `app/`. Persistent metadata, cache, account material, logs, and device configuration belong under `/mnt/us/bilicomics/data/`. Updating the application directory must preserve the data directory. Do not include real account material, downloaded comics, operator logs, or acceptance configuration in development archives.

Keep the original backend licenses, upstream revision, and patch history in the staged notices. Preserve the AOSP PinyinIME notice with the dictionary. Font and other resource notices must also accompany any resources actually bundled by the application.

`BILICOMICS_HOME` may override `/mnt/us/bilicomics` with an absolute path. The launcher invokes:

```sh
bilicomics --kindle --device PROFILE --data-dir DATA_DIRECTORY
```

## Model and firmware gate

Copy `device-profile.example` into the runtime data directory as `device-profile` only for an explicitly selected acceptance run. Supported geometry identifiers in the source are `pw5`, `pw6`, `scribe`, and `scribe2025`; identifiers select reference geometries rather than certify hardware compatibility.

The runtime `device-allowlist` contains exact two-field entries, `profile firmware`, added only after the relevant supervised acceptance decision. The repository example intentionally includes no entry. The launcher reads the actual firmware from `/etc/prettyversion.txt` and parses both configuration files as data, never as shell commands.

Only after an exact match does the launcher export `BILICOMICS_APPROVED_PROFILE` and `BILICOMICS_APPROVED_FIRMWARE`. Native initialization requires those values and rejects a framebuffer whose visible resolution or panning offsets differ from the selected reference geometry. No experimental display geometry override from the reference project is enabled in BiliComics.

## Display ownership, sleep, and exit

The launcher uses its own `/tmp/bilicomics-launcher.lock`. It leaves `powerd` and `lab126_gui` running, stops a running `statusbar` only when service controls are available, and pauses only `awesome` processes that it finds running. Its exit and signal traps resume exactly those changes. Application exit status is preserved.

Before startup takeover and a legacy resume without current framebuffer capability, it reads `com.lab126.winmgr epdcMode` while the native window manager still owns the display. The transaction has one bounded private process group and output collector. Only `Y8` and `Y8INV` are accepted. Startup and sequence-bound resume snapshots live in the private launcher directory; the application verifies their context before mapping the display or reclaiming input. An unreadable or unknown required mode ends the supervised launch through its restoration path. A current framebuffer-capable application instead uses fresh, layout-checked kernel metadata on explicit screen-saver dismissal, preserving the immediate window-manager pause without new wake-time LIPC queries.

The same pre-takeover startup transaction establishes portrait orientation. Its internal `--probe-native-portrait-framebuffer --kindle --device PROFILE` application branch reads approved framebuffer metadata and the input-device inventory without initializing Slint, workers, signal delivery, touch, or pixel writes. `AUTO:rotation` requires two matching `com.lab126.winmgr accelerometer` directions and matching metadata samples. The confirmed `scribe` profile normalizes native `U`/`L` to `U` and `D`/`R` to `D`; other profiles accept only `U` or `D`. `FIXED:rotation` is accepted only after successful inventory reads establish no accelerometer; it performs no accelerometer query. The entire transaction remains within the existing two-second private read-group deadline.

The private `native-portrait-orientation` record is exactly `generation:token:U|D|FIXED:rotation` followed by one LF. Rotation is a canonical kernel code from 0 through 3, never a global mapping to a particular device's Up or Down. Before first native I/O the application verifies that fresh metadata still matches this same-launch baseline. On each resume, `resume-native-portrait-orientation` is exactly `generation:token:lifecycle-sequence:CALIBRATED` followed by one LF. That record grants only the current resume callback permission to observe fresh GETVAR metadata; it does not contain or certify a current direction. The application checks the existing private-directory, launcher/application, token, sequence, and output-capability contract before and after the observation. Only a zero or half-turn relative change is admitted for automatic portrait orientation; landscape, unknown rotation, fixed-direction changes, and retained-layout changes fail before input acquisition.

These two sidecars are published by the internal `--publish-native-portrait-orientation VALUE --kindle --device PROFILE` branch inside the launcher's bounded native-read group. It verifies the live permitted reader, helper ancestry, process births, launcher token/generation, and preceding suspend acknowledgement where required. It creates only a fixed pending leaf using descriptor-relative `openat` with exclusive/no-follow flags and commits an absent target using `renameat2(RENAME_NOREPLACE)`. A pre-existing pending file, FIFO, link, target directory, changed ownership, or unsupported exclusive-rename operation fails without following an external target. Exclusive-writer failures retain pending evidence for the owning launcher's recovery after its bounded reader has stopped; the helper performs no named pending deletion that could race a replacement. Neither the writer nor the metadata branch creates an application identity or starts the UI. The Linux desktop fixture can exercise this filesystem/protocol writer with isolated approval records; it establishes no native display or touch result.

Portrait direction stages a renderer offset independently of application rotation and native output polarity. The next active frame applies the same combined rotation to pixels, window geometry, and refresh regions, then completes the existing full redraw and old-input drain before processing touch. Native touch already follows the framework's portrait direction, so input conversion still reverses only application rotation and divides by the window scale once.

Keep the executable and launcher from the same development bundle. The older 0.1.11/0.1.12 EPDC and output-capability formats remain readable, but contain no protected portrait calibration. A new launcher paired with an older executable fails its unsupported metadata helper before display takeover. A new executable paired with an older launcher rejects the missing portrait contract before its first framebuffer clear or touch acquisition, and returns through launcher cleanup. No environment offset or cached startup direction substitutes for a current resume observation.

The application compensates `Y8INV` at its framebuffer output boundary, including the initial clear and subsequent full or partial pixel writes. Logical image pixels keep their original meaning. This is separate from any reader preference: the launcher and application do not set or persist the system display mode. Slint captures contain logical pixels before this compensation; a framebuffer-memory image and an EPDC-mode projection have distinct evidence meanings.

Screen-saver entry keeps the same application process and reader state in memory. The launcher atomically records a suspend request and signals the actual application PID only after its lifecycle signal bridge is ready. It releases its display ownership only after the application acknowledges that rendering is paused and the native touch grab has been released. A checkpoint error is reported separately: the in-memory session can continue, but the launcher does not claim that its reading position was durably saved.

The optional application-owned cover follows the WeRead sleep-cover implementation inspected on 2026-11-12. While the native framework is running, a bounded `com.lab126.blanket load` read must establish a nonempty configuration without `ad_screensaver`. Every custom transition also requires that this launcher still owns the display, its captured window-manager identities remain stopped, the status bar is quiesced, and `/var/local/system/userpasswdenabled` is neither present nor a link. Unknown capability, advertisement configuration, or native password protection retains the stock path. No setting, service policy, advertisement, or password is changed by this feature.

For an eligible suspend, `sleep-cover-request` contains exactly the current `generation:token:sequence:suspend` record followed by one LF. Only after checked panel presentation, native input/display suspension, and a successful checkpoint may the application publish the matching `sleep-cover-ready`, then its ordinary `:ok` lifecycle acknowledgement. The launcher retains ownership only when both cover records, the current acknowledgement, the stopped native identities, and the same application's protected framebuffer capability agree. It leaves the already presented cover on the panel without restarting the status bar, continuing the window manager, or calling `xrefresh`. A disabled preference, missing or malformed cover confirmation, checkpoint failure, or unsupported framebuffer capability uses the normal native handoff instead. The eight-second lifecycle deadline still bounds preparation.

An application-owned cover does not require the native `exitingScreenSaver` event to wake. The current `outOfScreenSaver` intent and two bounded `powerd` reads confirming `active` permit a relay-ordered wake barrier. The application then observes fresh kernel output polarity and portrait calibration while native services remain stopped, hides the temporary cover, and restores the retained page. RTC-only wakes never query for or acquire display ownership. A queued new sleep or USB event invalidates the earlier wake barrier. A native lock or changed capability discovered during custom sleep restores the ordinary native handoff before any wake retry.

Stock resume requires `outOfScreenSaver` followed by `exitingScreenSaver`, or an explicit `powerd` state of `active` after that user wake intent. `wakeupFromSuspend` alone can be an RTC or background wake and never takes display or input ownership. A native password/unlock screen retains control until dismissal. Repeated events do not start a second application. A new sleep cancels earlier wake intent. USB takeover ends the application and the supervisor, including a pending wake.

The private lock directory contains the lifecycle exchange. The application receives `BILICOMICS_LIFECYCLE_DIR`, `BILICOMICS_LAUNCH_GENERATION`, and `BILICOMICS_LAUNCH_TOKEN`. It atomically publishes `lifecycle-ready` as `generation:token` after installing its signal bridge. The launcher atomically publishes `lifecycle-request` as `generation:token:sequence:suspend` or `generation:token:sequence:resume`, then sends `USR1` or `USR2`. The application publishes the identical fields in `lifecycle-ack`, optionally followed by `:ok` or `:checkpoint-error`. Every field must match the pending request; a stale acknowledgement is ignored. The eight-second deadline includes waiting for the early startup signal bridge. A missing acknowledgement ends the launch and releases ownership after bounded application shutdown.

The event FIFO remains responsive while acknowledgements and native unlock checks are pending. Application exit is handled in every lifecycle phase and retains its status. A stale launcher lock requires operator investigation; the script does not remove another process's lock or infer which service state should be restored. Cleanup cannot survive every kernel failure, power loss, or uncatchable termination.

## Manifest and evidence

`manifest.draft.json` uses a provisional independent package identity and the reference KPM manifest shape. Its `kindlehf` platform label is packaging metadata, not a device compatibility result. This directory does not create, publish, or verify a release package, update channel, or application license.

The launcher was adapted from the local WeRead implementation working tree inspected on 2026-11-03 and its lifecycle supervisor inspected on 2026-11-04. BiliComics uses an acknowledgement exchange to retain the same application process rather than restarting it after sleep. Reference-project test and emulator reports do not establish BiliComics behavior. No local verification was run for this import. Shell checks and isolated lifecycle fixtures run through the designated remote environment; they do not establish firmware-emulator or physical-device behavior. ARM cross-compilation and physical acceptance remain separate checks and must identify the exact model and firmware.

See [reference provenance](../../docs/REFERENCE_PROVENANCE.md) for source fingerprints, retained third-party material, and application changes.
