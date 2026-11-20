# Startup candidate on the rebuilt verification worker

The operator confirmed on 2026-11-20 that `test-env` was completely rebuilt.
SSH works. The old Cargo target, selected baseline ZIP/ELFs, and
`click-r2/admission.json` were not found on the mounted worker storage.
This is a new software run, not a continuation of an interrupted physical run.

## Inputs and scope

- Baseline source: `d81b529fd8f84790d91dfe3a8396853c296a8280`.
- Candidate: the same baseline archive with exactly the six files from
  `57265de60553ea97b01894592427d60c9008f850` listed in the
  [build handoff](../../physical/startup-optimization-20261119/resume/build/README.md).
  Comparing the two exported trees on the worker found exactly those six
  differences. In particular, the later top-level README is not a build input
  change in this comparison.
- All execution is on `test-env`. The three Kindle projects share Rust/Cargo
  1.99.1, the ARMv7 musl target, Zig 0.15.2, cargo-zigbuild 0.24.0 and the
  Cargo registry through `/opt/kindle-toolchain/env.sh`. Use the shared
  `/opt/kindle-toolchain/build.lock` for heavy compilation, at most two Cargo
  jobs here, and a private project target and evidence directory. Do not
  provision another toolchain or alter another project's target.

## Candidate results

| Check | Result | Scope |
| --- | --- | --- |
| Launcher and watchdog shell syntax | Both passed | Linux host; no Kindle services. |
| Launcher regression suite | 152 passed | Includes the six settle/logging/cancellation cases, also run separately. The six are not additional tests. |
| Watchdog and package regression suites | 6 watchdog and 51 package tests passed | Synthetic ownership, cleanup and package inputs; not a device recovery. |
| Production ARM build | Passed from an empty private target | The actual Cargo log shows `rustc` compiling the app library, app binary, Kindle backend and JPEG worker. Both binaries used `opt-level=3`, thin LTO, one codegen unit and stripped symbols. |
| Static development-package inspection | All 20 checks passed; 49 entries | The repository checker inspected the new ZIP, both ARM ELFs and their notices. It did not execute ARM code. |
| Desktop Rust lib/bin | 882 library and 18 binary tests passed; 4 existing library benchmarks ignored | Includes all six new Shelf route/budget tests. No filters, no ignored tests enabled. |
| Startup feedback | 4/4 passed | PW5 and Scribe Xvfb profiles, normal and controlled failure; the first real frame preceded worker admission, and the next activity frame changed pixels. |
| Real Reader lifecycle | Passed | The production launcher retained its full settle delay. The same Rust process and reading anchor survived the simulated sleep/wake and accepted the subsequent page turn. Native services were fixtures. |
| Final Shelf pixels | Zero differing RGB pixels on either profile | Independent `--demo --offline --screen shelf` captures in a network namespace matched the [frozen desktop reference](../../uiux/baselines/2026-11-17-startup/README.md) at PW6 1264 x 1680 and Scribe 1860 x 2480. Neither image comparison measures e-ink timing. |

The original [candidate build receipt](candidate-build.json),
[static package report](candidate-package.json) and [software result](software.json)
are copied from the worker. They are fresh-build receipts, not an admission
accepted by the historical physical runner. The development ZIP, receipts
and scoped fixture evidence were also copied to the ignored local artifact
directory `artifacts/verification/startup-rebuild-20261120/candidate/`.

Two first attempts exposed worker setup differences and were kept separately.
Running the Rust tests with the shell's default `umask 022` failed eight
lifecycle-directory privacy assertions. The same test executables passed with
the historical runner's `umask 077`; application permission checks were not
relaxed. Separately, the first Xvfb attempt exited before the first frame:
winit could not load `libXcursor.so.1`. The retry used the distribution's
`libxcursor1` and `libxfixes3` in the project workspace, not a second toolchain
or a system-wide library replacement. The shared Ubuntu image has since added
these libraries; the private copies and the completed desktop target were removed.

## Independent control

The exported baseline source passed its own launcher suite (146 tests) and
watchdog/package regressions (55 tests). These are separate executions, not
candidate results inherited from an earlier commit. The control reused only
the project's release dependency cache; the app, backend and worker were
recompiled from control sources. Its [ARM build](baseline-build.json) and
[20 static package checks](baseline-package.json) passed. The baseline
development ZIP was kept under `artifacts/verification/startup-rebuild-20261120/baseline/`.

The [byte comparison](control-comparison.json) found the same 49 entry names
in the two new packages: only `bilicomics/app/bilicomics` and
`bilicomics/launch.sh` differ. The other 47 payloads, including the JPEG worker
and the native Library document, are identical. The new control's worker
matches the historical worker; its app and ZIP do **not** match the historical
artifacts. Neither arm was installed. Both private Cargo targets were removed
after preserving the packages, reports and compiler logs; the shared tools
and registry were left intact.

## Device boundary

No build or host fixture can replace the missing historical package, card
detector, click adapters or pinned private transport. Restore those inputs or
qualify a new control package and click setup under new identities, starting
with a host-only dry run. Do not substitute a URI launch, guess a device
address, or copy old admission or lease records. The
[7.503 s historical median](../../physical/library-to-shelf-20261119/README.md)
is not a control observation for a new paired run.

The operator subsequently confirmed the root SSH endpoint. Aidoku recovered
the existing trusted device key offline and exposed a strict, pinned alias
on the `test-env` host. No first-use key was accepted. An independent
[read-only inventory](device-inventory.json) at 16:11 Asia/Shanghai found the
canonical installation and both document spellings absent, zero matching
catalog rows, and normal native foreground and window-manager state before
and after. No application was started, and its exact lease was released.

The [selector tests](selector-fixtures.txt) passed 16 host cases, including
a same-payload 14-selection rehearsal. A separate read-only ARM navigation
probe on the Scribe uniquely identified both public tabs and the selected
Home underline (89% versus 0% for Library). No button was sent, no application
or Library entry was installed, the helper's private staging was removed, and
the lease was released at 16:22:46. These are not timing samples. See the
[independent research runner](../../physical/startup-optimization-20261119/resume/rebuilt/README.md).

The subsequent [closed six-pair Scribe comparison](../../physical/library-pairs-20261120/README.md)
completed under a different lease. Its times, install/restore scope and
image checks are device results, not part of this software qualification.

Coordination notes belong in the host's `/var/tmp/kindle-build-coordination/`.
Do not acquire the host's `/tmp/codex-kindle-physical-test.lock` for planning
or inside the isolated Ubuntu userspace. A past absence is not evidence that
the installation or lease is still absent at the start of a new batch.
