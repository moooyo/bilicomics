# Pre-device Verification

Recorded on 2026-11-04 (Asia/Shanghai).

The [machine-readable final summary](../artifacts/verification/predevice-20261104/verification-summary.json)
pins the selected archive, each evidence class, retained source identities, and
the physical stop point.

Status: the selected pre-device checks passed. Physical Kindle testing is held
for the user. Completed checks, interrupted attempts, and unexecuted checks
retain their own source and executable identities. This document is a
verification record, not a replacement product or interface specification.

## Task boundary

The user authorized remote testing and required work to stop before physical
Kindle testing. No physical device connection, installation, display takeover,
or change to a physical device's acceptance configuration is authorized in this
round. The next physical run remains pending the user's participation.

All compilation, tests, application execution, and rendering use `ssh test-env`.
Windows is used for source editing, static file inspection, transfer, and viewing
saved images. A remote outage does not permit local verification.

The existing 0.1.2 snapshot 9 package and evidence remain immutable. The new
development version is 0.1.3. It rejects a NUL in a signing query before the C
host can truncate the input and restores persistent touch coordinates when
reacquiring input after sleep. The public KPM manifest remains an unpublished
draft; updating its version does not publish or install a package.

## Corrected candidate and results

The selected [corrected 0.1.3 development archive](../artifacts/verification/predevice-20261104/corrected-candidate/bilicomics-kindle-0.1.3-dev.zip)
is 49,927,231 bytes, SHA-256
`58d94bad279ee3a4bb26abbaa102017d39753a8caa9f6391466e6ab4f39db0eb`.
Its 64,632,960-byte ARM executable has SHA-256
`6f178d7b0c4d2d4334fe7d0136b55ad65334388fed5beb271f599086bf2f3fff`.
The corrected [ARM source manifest](../artifacts/verification/predevice-20261104/corrected-candidate/evidence/arm-package/source-inputs.sha256)
has SHA-256 `a589d72383a8d9bfd01a0a2f2f27b1e9eacf2f1c096533c4129da89338c009af`.

| Check | Result | Scope |
| --- | --- | --- |
| Application library | 149 passed, no failures or ignored tests | First 0.1.3 desktop candidate; application runtime sources are unchanged by the corrected Kindle dependency. |
| Default and paused views | 23 automated captures and 23 manual visual reviews passed | Desktop demonstration data. |
| Actual desktop pointer input | Eight cases, 54 clicks passed | Catalog, selection, search, and downloads. |
| Reader and guest regression | Nine Reader and five guest cases passed | Desktop input and network-isolated synthetic local content. |
| Startup and retained Reader | Two startup cases and one real-process launcher/Reader case passed | Controlled Linux services; same PID, source position, and subsequent page turn. |
| Launcher fixtures | 28 passed | Substituted services, not native firmware acceptance. |
| Corrected native backend | 34 passed, no failures or ignored tests | Includes all seven new touch regressions and matched production dependency versions. |
| Native signing | Five checks and seven official Go/JS comparisons passed | Actual digest-pinned WASM and synthetic requests on the host. |
| Production services and restart | Two subprocess cases passed | Synthetic QR/identity, committed receipt, SIGKILL, missing-page recovery, corrupt-page repair, and unknown-intention startup state. |
| Actual non-demo offline App | Two independent processes and one real XTest turn passed | UI page 2 to page 3, then cold restart at page 3 with identical comic-body pixels. |
| Anonymous public metadata | Three requests passed with HTTP 200 and business code 0 | Sixteen styles, three orders, and eighteen public comic summaries; the signed category response was encrypted and decoded by the real adapter. |
| Corrected firmware UI | Seven cases passed | Independent Scribe 5.20.1 guest, final corrected ARM executable, native input/display, same-process sleep continuation, and system restoration. |
| ARM signing execution | Five cases and seven official reference comparisons passed | Separate immutable ARM protocol test executable; no service or account operation. |
| Development archive | All 22 allowed entries and static ARM ELF checks passed | CRC, digests, modes, fixed timestamps, and runtime-data exclusion. |
| Verifier failure cleanup | Two controlled fault cases passed | Timeout escalation and nonzero-leader cleanup left no owned process-group members. |

The successful desktop executable remains
`48a4de7b9201dc4cdeb43f90297c32d7cfac20f5f8b96bd47b820f53311b7370`,
with its original source manifest
`899e85f81f219a371ca34778994ae4d0f1a8b734599223299a52df33a2733083`.
The corrected candidate changes only the optional Kindle dependency's
`src/touch.rs` and `PATCHES.md` among the 333 recorded build inputs. The desktop
feature does not link that dependency. Its earlier evidence is retained under
its original identity, as are the unchanged protocol and application-service
checks. [Scope evidence](../artifacts/verification/predevice-20261104/corrected-candidate/evidence/candidate-scope.json)
records that separation.

The standalone backend test lockfile is generated only inside an isolated copy;
all 465 resolved package versions and checksums were verified against the
application's production lockfile. Production sources and lockfiles were not
changed by this test setup. Suite and supplementary manifests use different
line ordering; their parsed path/digest maps match. The corrected backend test
and corrected ARM build likewise have equal input maps.

The independent ARM protocol executable is
`a09d6d766a49c1fd950f603c7ae5e300dfc61ecfff76f949767e48ca023d644d`,
built from the earlier isolated protocol snapshot. Its whole-source identity is
not reassigned to the final application. All 168 recorded protocol/domain and
vendored-protocol component inputs match the final application; the firmware
run compares all seven complete synthetic request inputs before comparing
their signature digests with the official Go/JS results.

## Verification tracks

| Track | Intended evidence | Important boundary |
| --- | --- | --- |
| Remote regression | Library tests, default views, pointer input, Reader and guest cases, startup, retained Reader, launcher, and archive checks | Automated state and pixel checks do not replace visual review or physical panel acceptance. |
| Native signing | The digest-pinned official WASM runs through the production native callback and is compared with an independent official Go/JS host | Synthetic signing inputs do not establish service acceptance or account authentication. |
| Controlled production services | QR candidate persistence, image acquisition, receipt commit, forced process termination, missing-page recovery, corrupt-page repair, and local Reader rendering | The transport and signer are fixtures. This is not a real account or complete application authentication click flow. |
| Anonymous public metadata | At most three allowlisted HTTPS requests read labels, initialize an anonymous device context, and read one signed category page | No account session, comic page acquisition, history, favorite, purchase, or recharge operation is allowed. |
| Independent firmware guest | The ARM application uses the guest framebuffer, input driver, launcher, and power-daemon interfaces | The exact emulator revision, modified guest, and firmware must be recorded separately from a physical Kindle. |

Generated evidence belongs under
`artifacts/verification/predevice-20261104/`. Each runner writes into a new
directory and records its input manifest and executable digest. Earlier failed
or interrupted attempts must not be overwritten by later successful ones.

The remote regression entry point is `scripts/verify-remote.ps1`; individual
controlled runners live in `scripts/verification/`. Public network tests remain
explicitly opt-in. The default library suite must not silently contact services.

## Firmware isolation

The BiliComics guest uses a new workspace, a separate copy of the immutable
prepared Scribe base, and its own writable overlay, loopback ports, sockets, and
logs. The running WeRead guest, its writable disk, data, and ports are not test
inputs. Base digests and modes, and the original WeRead QEMU process identity,
are recorded before and after setup.

The candidate guest is first-generation Scribe firmware 5.20.1. The physical
device identified in the WeRead session is first-generation Scribe firmware
5.19.5. Results for one environment do not accept the other.

The initial BiliComics QEMU launch used the emulator's 2048 MiB TCG cache default.
A lower 256 MiB host cache is prepared for subsequent isolated runs. Any changed
host parameter or guest compatibility adjustment requires a separate recorded
environment delta; it is not a product installation prerequisite.

## Interrupted remote attempts

The host kernel recorded two global OOM events at 13:39:08 and 13:42:58 on
2026-11-04, each terminating a Rust compiler with approximately 3.5 GiB anonymous
RSS. The corresponding integration builds ended with `SIGKILL`, and remote SSH
and some desktop/guest cases timed out during this interval. These are failed or
interrupted attempts, not successful checks. Timing overlap alone does not
assign every test failure to OOM.

After confirming its command line, the root agent stopped only the newly created
BiliComics QEMU process. The existing WeRead instance was not stopped. Subsequent
builds use one Cargo job, default rendering uses one process, and the BiliComics
guest stays stopped until compilation has completed. Raw host and failed-run
evidence is preserved under the current ignored verification directory.

The user subsequently rebooted `test-env`. The first post-reboot SSH sample at
13:58:47 reported boot ID `abfcc985-e193-4902-a00d-b818846764e4`, 10,949 MiB
available memory, and no running firmware guest. This is an external environment
reset, not a BiliComics application recovery result. The application and
firmware runs after that reset use new output directories.

## Native continuation finding in the first candidate

The first 0.1.3 ARM candidate retained the same process, Reader generation,
chapter, source revision, page 19, and `launch=shelf` across a real guest
screensaver/wake cycle. However, the next tap at the same physical position
`(1580, 1240)` moved backward to page 18. A second run reproduced this with two
stable comic-body captures before and after wake, excluding a loading frame as
the explanation. These original results remain failed continuation evidence.

The input backend reopened evdev with zeroed coordinates, although Protocol B
reports state changes and can omit unchanged axes. The correction restores
per-slot coordinates without restoring an active gesture, preserves coordinate
changes during input draining, and distinguishes actual slot/tracking
capabilities. A raw guest probe confirmed that the second same-point tap omitted
both position axes while `EVIOCGMTSLOTS` still retained `1580,1240`; global
`BTN_TOUCH` preceded the slot tracking event. The corrected firmware run retained
the same PID and page 19 through sleep/wake, then advanced to page 20 on the
original first tap. Stable comic-body hashes were identical before and after
wake. Page 20 deliberately reuses the known page-18 fixture artwork; it is not
acquired upstream content. The initial archive remains a failed historical
candidate.

Two recoverable `cargo-zigbuild`/`cc` compiler-wrapper identification warnings
remain in both ARM build logs. Read-only inspection strongly attributes them to
`--version` probing and ARM shim injection. Actual argv was not captured during
that probe, so the exact cause is not promoted to a complete runtime diagnosis.
The shim exists, compilation completed, and ELF and guest execution passed;
these builds are not described as warning-free.

## Remaining acceptance boundaries

- No real account authentication, account content acquisition, purchase, coupon
  consumption, recharge, or credential renewal was performed. Anonymous public
  category success does not accept those operations or future service behavior.
- The production asset manager's lazy download and atomic-publication path was
  not exercised by these signing checks. Their public asset was provisioned and
  digest-checked explicitly.
- Initial input snapshots are not atomic across a kernel frame hidden before
  `SYN_REPORT`. Observed queue races are covered by bounded retries; full
  multi-pointer acquisition semantics remain unaccepted. The final firmware
  cases use supervised single-contact sequences.
- No physical model/firmware acceptance, panel timing, ghosting, battery,
  physical Wi-Fi, pen, or USB behavior is established. Repository acceptance
  allowlists remain empty. No public release or project license was selected.

## Required continuation assertions

BiliComics retains the application process across the screensaver lifecycle.
The firmware case must compare the PID, `/proc` start time, executable identity,
comic, episode, index revision, page or strip anchor, and Reader settings before
sleep and after a confirmed wake. It must then turn a page through native input
and observe the expected next source page. A startup preference of `shelf` must
not replace the retained Reader route.

Application snapshots and QMP framebuffer captures are separate artifacts.
Screensaver events and a software-rendered image do not establish physical
suspend power, panel completion, ghosting, battery use, or unlock timing.

## Physical handoff gate

The user authorized the subsequent supervised run on 2026-11-05. Its exact
0.1.3 candidate, limited Scribe 5.19.5 results, timed-exit interference, recovery,
and explicit exclusive return to WeRead are recorded in
[PHYSICAL_VERIFICATION.md](PHYSICAL_VERIFICATION.md). This earlier gate and its
emulator results remain historical. Further device access requires a fresh
exclusive handoff; the later immersive-reader source does not inherit the older
candidate's physical acceptance.

After the remote and firmware checks have been assessed, stop and wait for the
user. The prepared first physical run should:

1. Use an isolated offline demonstration directory and an exact model/firmware
   selection. Preserve existing application data and independently verify the
   deployed executable and launcher digests.
2. Establish that other applications have released display and touch ownership.
   Keep a recovery connection and a launcher-owned external watchdog available.
3. Begin with a 30-second display run and a separate initialization timeout.
   Confirm the startup feedback and restoration of native Home and touch.
4. Extend to supervised reading, page turns, strip navigation, settings, and
   explicit exit only after the first run restores native control correctly.
5. Sleep from a noninitial reading position, complete native wake/unlock, and
   confirm same-process continuation followed by a real page turn.
6. Record user-observed panel behavior separately from logs, framebuffer pixels,
   database anchors, and resource samples.

Real-account login, acquired content, transaction acceptance, USB transitions,
long-run power measurements, other devices, and release distribution retain
their own explicitly scoped runs. No compatibility allowlist entry is granted
by desktop or firmware-emulator results.
