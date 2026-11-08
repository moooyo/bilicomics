# Wake Continuity and Settings

Status: **complete for the identified implementation and verification scopes**.
Recorded on 2026-11-07, Asia/Shanghai.

This is an implementation and evidence report for the requested retained-reader
wake correction and removal of the user-facing inversion setting. It does not
replace the supplied design. The completed
[0.1.6 resource work](RESOURCE_AND_PERFORMANCE_IMPLEMENTATION.md) retains its
original identities and acceptance scope.

## Resulting behavior

The wake change keeps the live reader while returning display and input control
through the stock sleep and unlock lifecycle. For a supported eight-bit
framebuffer, the application obtains fresh kernel framebuffer information for
the current resume request. It can select output compensation without bringing
native Home forward merely to query the window manager. Other valid formats
retain the reviewed fallback.

The application still preserves the system's native display-mode preference.
The kernel observation is read-only. Logical comic pixels, aspect ratio,
contextual reader controls, and saved source anchors remain independent of
native `Y8INV` output compensation. PIN and advertising-lockscreen information
describes the observed device environment; it does not authorize or implement
an unlock bypass.

The launcher binds its private capability and `FRAMEBUFFER` resume marker to
the current launch, application identity, and lifecycle sequence. A fresh kernel
GET must validate the retained framebuffer layout before that path is used.
The native backend schedules a complete retained scene after resume and records
the first full rendered frame when diagnostics are enabled. That record concerns
drawing and refresh submission, not physical panel-update completion.

The R5 launcher correction accepts an owner-private directory carrying SGID as
well as the same directory without it: observed directory modes `0x45c0` and
`0x41c0` both preserve permission bits `0700`. Ownership, private permissions,
context, identity, and capability checks remain required. This corrects the R4
capability rejection; it does not grant group or public access.

## Removal of the inversion preference

Version `0.1.9` removes the account setting, its model property and action, the
stored `Preferences.invert` field, and its two localization entries. The source
changes are in [the account view](../ui/account.slint),
[account actions](../src/app/account.rs),
[preferences](../src/app/mod.rs), and
[account localization](../assets/i18n/account.json).

An existing preference object containing `invert: true` remains loadable.
Deserialization ignores that retired field while retaining settings such as
`direction`, `launch`, and `cache_limit`. Saving the loaded preferences omits
the retired key. The database compatibility regression
`retired_inversion_preference_does_not_affect_loading_or_persistence` passed in
the identified 458-test `0.1.9` library run. The controlled UI matrix, three
targeted settings captures, and a fresh ARM development-package build and static
inspection have also passed within their separate scopes.

Internal native output compensation, the sequence-bound `FRAMEBUFFER` path,
and selected-button `inverted` styling remain. They do not expose an account
inversion preference or change the system theme.

## Candidate and verification stages

The [aggregate record](../artifacts/verification/wake-render-20261107/summary.json)
keeps these stages separate. A passed component or observed behavior is not
transferred to a later whole executable without its own identified evidence.

| Stage | Identified result | Boundary |
| --- | --- | --- |
| Earlier R3 | Native Home remained visible during wake. | The visual failure remains a failed historical result. |
| R4, `0.1.8` | 457 library tests, 93 backend tests, 72 launcher fixtures, nine reader cases, two startup cases, the controlled real-process lifecycle, 44 real-ELF relay cases, six physical-helper fixtures, and the Kindle-feature binary compilation check passed. | Desktop fixtures and the compile check are not physical acceptance. The compile check is not counted as tests. |
| R4 ARM | A fresh ARM build and 28-entry, nine-check development-package inspection passed. | The static checker did not execute ARM or operate a device. |
| R4 physical attempt | The capability check rejected the private SGID directory; the timing record shows the classic property-observation fallback rather than the intended fast path. | The visual wake goal remained unaccepted. This result does not establish that the new wake path works. |
| R5 launcher follow-up | Two additional launcher cases bring that suite to 74. The real-process lifecycle, six helper fixtures, rebundling, and nine static package checks passed. | R5 changes launcher handling and reuses the exact R4 application source inputs and ARM executable; its pipeline and package have distinct identities. |
| R5 supervised observation | The user reported the same comic page directly after wake, no intervening native Home, normal colors, and restored native dark Home and touch after stopping. The independent resume analysis passes all 23 compared identity/anchor fields. | The automated latest-state comparison failed after later intentional menu/page interaction. Its original failure and pending automated panel-review fields are retained. |
| Final `0.1.9` | 458 library tests, 23 default renders, eight pointer cases, nine reader cases, two startup cases, the controlled real-process lifecycle, and three targeted settings captures passed. The three settings PNGs received separate manual review. A fresh ARM package passed 28-entry, nine-check inspection. | Retained backend/launcher results are mapped components, not new executions. The final ARM executable was not run on a device. R5 is not whole-binary physical acceptance of `0.1.9`. |

The [R4 controlled result](../artifacts/verification/wake-render-20261107/controlled-r4/summary.json)
binds 426 source inputs to manifest SHA-256
`7c67b09e0d5e638a8067c992d99b7feb697fcc37cf0acb19983afd05a7caa790`
and desktop executable SHA-256
`9e21f8c8ffd3d8cb64cdf6e96d3916605058e3276b0c66971e7f24557675ea71`.
The independently serialized backend manifest has SHA-256
`85c2dee2146c5ac1ef4fe62b61e1499bd474026a72cdf052799746366e55bf71`.
The [source mapping](../artifacts/verification/wake-render-20261107/controlled-r4/source-manifest-equivalence.json)
proves that all 426 path/digest pairs match while explicitly preserving the
different serialization hashes.

The [R4 ARM result](../artifacts/verification/wake-render-20261107/arm-r4/package/package.json)
records executable SHA-256
`d8a62076bccddf63e4736069c6f3566fc03e303bfec06bffff1e43006e72938c`
and archive SHA-256
`8f2cdbe14d2516abd1c129834914291ca1b975d25cb0d0e7c0329c7640025fbb`.
The R5 rebundle has archive SHA-256
`f2f3b5fbe4d1014d408e544316e4b2ad8ac3743df2a8316cbdd0847689dc9399`.
All 426 R5 application source path/digest pairs and their manifest SHA match R4.
The runner's source selection contains `assets`, `src`, `ui`, `vendor`, `tests`,
`Cargo.toml`, `Cargo.lock`, and `build.rs`; it excludes `packaging`.
The changed launcher is recorded by the separate pipeline selection, which
also includes `scripts` and `packaging`. R5's pipeline/launcher provenance will
not be substituted for the application manifest. The
[R5 runtime result](../artifacts/verification/wake-render-20261107/r5-sgid/runtime/summary.json)
records pipeline SHA-256
`5c67450d0f0c9567c59d71f20b84c1b3d1c5642cfe200bd39fbd7e85303b4ae0`.
The [rebundle record](../artifacts/verification/wake-render-20261107/r5-sgid/rebundle.json)
replaces only `bilicomics/launch.sh`, whose new SHA-256 is
`a2b3ae9329ca9ec8bbf72303de3cf97943907b1b12b60e9eb39325bc1bac08dc`.
It proves that the other 27 entries remain byte-identical with their modes and
timestamps preserved. The exact rebundle then passed the
[nine static checks](../artifacts/verification/wake-render-20261107/r5-sgid/package.json).

Actual ARM build diagnostics remain in
[kindle-build.log](../artifacts/verification/wake-render-20261107/arm-r4/kindle-build.log).
The empty packaging-wrapper log is a separate file and does not establish a
warning-free build. JPEG attribute, deprecated atomic API, unused-helper, and
compiler-wrapper diagnostics remain visible in the actual build log.

## Final 0.1.9 controlled and settings verification

The [final controlled result](../artifacts/verification/settings-no-invert-20261107/controlled-r6/summary.json)
binds 426 source inputs to manifest SHA-256
`2048cedc74c0370bff8825897ce4e5d28025400f46e111c79a9e14475aa4be1d`
and the new desktop executable to SHA-256
`73b832e498ff7882c3e2f6f103a174a845bcd0d68acc710835d0dc32fb4f262c`.
It passes 458 library tests, 23 default renders, eight pointer cases, nine reader
cases, two startup cases, and the controlled real-process lifecycle. Inputs and
the selected executable remained unchanged during that run. Guest, launcher,
and package phases were not selected in this controlled run.

The [component mapping](../artifacts/verification/settings-no-invert-20261107/component-provenance.json)
checks 22 unchanged backend source files and eight unchanged native source
files. Cargo configuration and the locked dependency graph match except for
the application's version, and the launcher is byte-identical to verified R5.
This retains the R4 93-test backend result and R5 74-test launcher result within
those component boundaries. It does not claim another execution of those suites,
identical whole executable bytes, or a new `0.1.9` physical-device result.

The [targeted settings capture result](../artifacts/verification/settings-no-invert-20261107/settings-r6/no-invert-settings-results.json)
contains three successful captures with zero capture failures: Paperwhite
portrait, Scribe portrait, and Scribe landscape. Automatic checks cover the
fixture route, profile, demo state, and completed startup. The separate
[manual review](../artifacts/verification/settings-no-invert-20261107/visual-review.json)
inspected those three logical software PNGs and found the inversion item absent,
readable remaining labels, no internal blank placeholder, and no overlapping
controls. It is not approval of every default view, touch behavior,
accessibility, native output compensation, or the physical panel. The original
automation's visual-pending fields remain intact.

The [final ARM build result](../artifacts/verification/settings-no-invert-20261107/arm-r6/summary.json)
uses that same `0.1.9` source and pipeline identity with inputs unchanged.
Executable SHA-256 is
`16cbf6dcb7b2080142372303eedc672531f1b59eadf91074fd634d595a2e2534`
for 66,051,736 bytes. The
[development archive](../artifacts/verification/settings-no-invert-20261107/bilicomics-kindle-0.1.9-dev.zip)
SHA-256 is
`a81144aaba2ee5430697f59a09989cf3ffa6c173bd0f3e69840d366e6408e854`
for 50,551,411 bytes. All 28 entries and all nine
[static checks](../artifacts/verification/settings-no-invert-20261107/arm-r6/package.json)
passed, including selected binary identity, empty device-acceptance allowlist,
notice provenance, and static ARM ELF properties. The executable was not run
during this final verification.

The actual [final build log](../artifacts/verification/settings-no-invert-20261107/arm-r6/kindle-build.log)
records `Finished release` in 3 minutes 41 seconds. It retains the JPEG attribute
warning, deprecated atomic API warning, two compiler-wrapper `FileNotFound`
diagnostics, and unused `publish_cover` warning. Its 2,044 bytes are distinct
from the empty packaging-wrapper log. No warning-free build is claimed.

## R5 resume checkpoint and cleanup

The physical reader evidence run is `sgid-reader-e2731c4d`. Its shared lease is
separately identified as `sgid-wake-20261107-e2731c4d` in the lease receipts.

The independent
[resume analysis](../artifacts/verification/wake-render-20261107/physical-r5/resume-anchor-analysis.json)
passes all 23 compared reader identity/anchor fields between the before-sleep
snapshot and the matching resume trace record. It retains PID `8315`, birth start ticks `711609694`,
reader generation 2, and page index 17, the eighteenth page. This is a comparison
of that resume checkpoint, not every Reader/UI field or the later latest state.

Later `menu: true`, `menu: false`, and page 19 states were explicitly confirmed
by the user as deliberate finger interactions. The latest-state comparison
therefore failed and caused a safe stop. That original failure remains intact.
It is not renamed as a passing latest-state checkpoint, even if an independent
comparison of the earlier resume checkpoint passes. The
[original run](../artifacts/verification/wake-render-20261107/physical-r5/run.json)
retains `last_failure`, `phase: stopped`, `physical_panel_review: pending`, and
`physical_panel_observed: false`. The no-Home and normal-color observations are
separately attributed to the user, including in the independent analysis. There
is no post-wake panel photograph; the exported framebuffer/projection images
are from before sleep.

Recorded uptime observations are screensaver exit `7116182.35`, window-manager
pause `7116182.55`, and first full rendered-frame diagnostic `7116183.24`.
They describe request and rendering order. They are not a panel photograph,
a zero-latency measurement, or proof of the physical update's completion time.
The [launcher timing](../artifacts/verification/wake-render-20261107/physical-r5/launcher-timing.tsv)
and [lifecycle trace](../artifacts/verification/wake-render-20261107/physical-r5/lifecycle.jsonl)
show `native_capability_framebuffer`, the framebuffer wake barrier, native
window-manager pause, a resume request, `output_mode_ready`, and the first full
rendered-frame diagnostic. The sequence-bound `FRAMEBUFFER` request and fresh
kernel GET are the path required by the identified implementation and protocol;
their selection is inferred from that recorded event chain and the frozen
source. The export does not directly record the wire payload or ioctl calls.

The user confirmed restored native dark Home and touch. The separate
[post-stop read-only native probe](../artifacts/verification/wake-render-20261107/physical-r5/postflight.json)
reports `Y8INV` under run `sgid-postflight-e2731c4d`, bound to the same R5 lease.
Its original `preflight` schema is retained; it is an isolated native probe
after stopping, not a new application run or a value inferred from the earlier
reader trace. The
[release receipt](../artifacts/verification/wake-render-20261107/physical-r5/lease-released.json)
records owner/token checks, native restoration, closed connections, and removal
of the exact owned lease directory at `2026-11-06T18:03:16.026917+00:00`.
The [acquisition receipt](../artifacts/verification/wake-render-20261107/physical-r5/lease-acquired.json)
identifies the same run and token digest. The raw watchdog log retains its
`/proc/8055/environ` disappearance diagnostic; successful restoration checks are
not a claim that the log contains no diagnostic. A subsequent device run
requires a new exclusive handoff.

The [R4 physical run](../artifacts/verification/wake-render-20261107/physical-r4/run.json)
and its [classic-path timing](../artifacts/verification/wake-render-20261107/physical-r4/launcher-timing.tsv)
remain separate. SGID rejection was diagnosed from the private-directory
inspection and isolated follow-up; the timing record itself demonstrates the
fallback route. Its restoration and lease release do not turn the visual goal
or the unexercised fast path into a pass.

## Evidence identity and source provenance

The controlled safe export contains 143 payload files plus its manifest. The
ARM safe export contains 13 payload files plus its manifest. Source-mapping
metadata was added separately. Original reports retain raw remote paths and
raw file hashes; those are not substituted for the identity of an exported
redacted file.

The R5 controlled export contains 28 payload files plus its manifest. The R4 and
R5 physical safe manifests cover 22 and 18 payload files respectively. Each pair
of lease receipts is additional metadata; the R5 resume analysis and postflight
record are also additional files. Those added records are not claimed to be
covered by the original physical safe-export manifests.

The final controlled and settings safe exports contain 385 and 22 payload files
respectively, each plus its manifest. The final controlled lifecycle texts have
raw SHA-256
`cdf8251e61d1fa21e9a331f4860ba9cecdbeb82f4a8dbb8dc2777af1934aacfc`
and exported SHA-256
`f9001a8eb6b5bdd527f59bbfb8501ced5efeb9172c7f142f8f55b1fd2766548f`.
The new source identities do not erase the earlier R4 raw/export identities.
The final ARM safe export contains nine payload files plus its manifest; the
development archive and sidecar are separately retained artifacts.

In particular, the two controlled lifecycle text files have raw SHA-256
`2805b0b8e1d6c4d0a5361c3016d5c5eb025f597613c940833dba2d0312f7cbe2`
and exported SHA-256
`c76e388d026f1d48bb582d15fdf88785e80678de617150892773b3528b70aaf8`.
The [safe-export manifest](../artifacts/verification/wake-render-20261107/controlled-r4/safe-export.json)
records that transformation without modifying the original result.

The read-only WeRead launcher reference is the pinned file
`test-env:/root/weread-emulator/stage/native-theme/launch-799d5bba5eb9.sh`, SHA-256
`799d5bba5eb9e7c2b534437a1112e6810e5c85d73582742d770c96c6f72af234`.
Its observed `take_display` and `resume_managed_application` paths informed
architecture and private-directory permission-mask review. The associated
WeRead ELF SHA-256 is
`2fd3d6ab2d648aec2671d0d2305a5e5ebe30fa6fd5e51f11705324d561317011`;
its peer evidence path is
`test-env:/root/weread-emulator/artifacts/native-theme/physical-2fd3.json`.
The launcher digest is a file identity, not a Git revision, and that application's
physical result is not BiliComics acceptance.

Managed full-frame-before-WakeReady ordering was also reviewed in the current
local WeRead working tree's `src/app/managed_sleep.rs` and native backend
`platform.rs`. Their initial observed bytes were not fingerprinted. Those
observations do not establish the source identity of the pinned ELF above.
BiliComics implements its own state, retained reader, output handling, and
launcher; the system-mode setter was not copied. The existing
[reference provenance](REFERENCE_PROVENANCE.md) retains earlier observations
and their limits. An old working-tree HEAD must not stand in for this later
reference.

All BiliComics Cargo and verification processes ended before the final host
window was explicitly returned to WeRead. No Kindle connection, held device
lease, or further device test remains pending. The completed R5 lease release
retains its own receipt and time; it does not authorize a later device session.

Build, test, and runtime verification ran on `test-env`. Final evidence review
introduced no additional builds, tests, runtime probes, SSH operations, or hash
computations. Fixture results, user observations,
raw framebuffer memory, software captures, rendering-request diagnostics, and
physical panel evidence remain distinct. No real-account, live-service,
hardware support, battery, permanent leak-absence, or complete `0.1.9` physical
acceptance claim is made.
