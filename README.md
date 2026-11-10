# BiliComics Kindle

An independent Rust and Slint Bilibili Comics application for jailbroken Kindle devices, with a desktop software-rendering backend for development.

The interface follows the user-supplied Paperwhite and Scribe design: bookshelf, bookstore and Chinese search, comic details, paged comic reading, panel and strip views, landscape spreads, downloads, account settings, QR sign-in, and explicit chapter purchase flows. Interface text is Chinese; code and documentation are English.

## Development

The operator environment is Windows and PowerShell. Build, tests, and rendering run on the designated remote worker:

```powershell
.\scripts\verify-remote.ps1
```

Add `-CheckOnly` for compilation only, or `-Kindle` to also cross-compile and verify the ARM development bundle. Each run uses a new remote workspace and a new local evidence directory under `artifacts/verification/remote-runs/`; existing sources and results are never overlaid. Builds use the committed `Cargo.lock` with `--locked`. The upload excludes Git metadata, credentials, runtime data, and acquired comics.

Use `-Phases` to select the controlled regression stages and `-RemoteTargetDirectory` to reuse an explicitly selected remote Cargo cache. Cargo compilation and default-view rendering run one job at a time by default. The native-signing, controlled-service, anonymous-public-metadata, and firmware runners have separate evidence boundaries described in [pre-device verification](docs/PRE_DEVICE_VERIFICATION.md). None of these commands connects to a physical Kindle.

The client has its own SQLite database, account namespaces, credential file, image cache, preferences, and launcher. KOReader, Jobman, a custom server, and a server database are not runtime requirements.

Startup now presents the project name and a low-frequency activity indicator before full font and local-state preparation. Sleep retains the same reader process and page; native display/input are handed to the system screensaver and reclaimed after unlock. This is separate from the ordinary startup destination preference. See [startup and sleep lifecycle](docs/STARTUP_AND_SLEEP.md).

## Design preview

After building on the remote worker, use the actual Cargo target directory:

```sh
xvfb-run -a -s '-screen 0 2200x3000x24' ./target/debug/bilicomics \
  --demo --device pw5 --screen shelf --data-dir /tmp/bilicomics-preview \
  --capture /tmp/bilicomics-shelf.png
```

Profiles are `pw5`, `pw6`, `scribe`, and `scribe2025`. Routes are `welcome`, `login`, `shelf`, `store`, `search`, `detail`, `reader`, `downloads`, `purchase`, and `account`. Use `--landscape --device scribe --screen reader` for the spread preview, and `--offline` for unavailable-network behavior.

`--capture-startup <path>.png` writes two startup frames; `--lifecycle-trace <path>.jsonl` records readiness and suspend/resume state without credentials. Demonstration verification can supply `--startup-delay-ms` to model slow preparation. Scripted actions and bounded normal captures start their clock after application readiness.

`--demo` uses a separate `demo/` data directory and embedded fictional design fixtures. Its source client rejects network operations; it never uses a real session or performs an upstream purchase. Missing design pages are explicitly fixture substitutions rather than acquired comic content.

Normal execution uses the per-user `bilicomics` data directory; `--data-dir` overrides it. On Kindle, the launcher uses `/mnt/us/bilicomics/data`.

## Handoff

| Document | Contents |
| --- | --- |
| [Implementation status](docs/IMPLEMENTATION_STATUS.md) | Implemented behavior, verification evidence, and acceptance boundaries |
| [Design source](docs/DESIGN_SOURCE.md) | Downloaded design provenance and reference renders |
| [V2 implementation review](docs/DESIGN_REVIEW_V2.md) | Confirmed visual and interaction corrections against the latest Downloads reference |
| [Pre-device verification](docs/PRE_DEVICE_VERIFICATION.md) | Remote regression, native signing, controlled recovery, firmware integration, and the physical handoff gate |
| [Physical verification](docs/PHYSICAL_VERIFICATION.md) | Scoped Scribe 5.19.5 offline checks, retained reading, physical input, recovery, and exclusive handoff |
| [Immersive reader](docs/IMMERSIVE_READER.md) | Full-window image rendering, contextual progress controls, and source-specific verification |
| [Immersive reader physical verification](docs/PHYSICAL_IMMERSIVE_VERIFICATION.md) | Scoped 0.1.4 Scribe checks, physical sleep/wake, contextual controls, recovery, and device return |
| [Technical handoff](docs/HANDOFF.md) | Request context, source provenance, reusable infrastructure, migration risks, and pending inputs |
| [Protocol source notes](docs/API_REFERENCE.md) | Source-derived Bilibili interfaces, authentication, page acquisition, native dependencies, and evidence limits |
| [Cookie management review](docs/COOKIE_MANAGEMENT_REVIEW.md) | Request scope, durable session updates, concurrent response handling, and verification limits |
| [Download performance](docs/DOWNLOAD_PERFORMANCE.md) | Batched image grants, bounded scheduling, cache recovery, and controlled timing evidence |
| [Download completeness](docs/DOWNLOAD_COMPLETENESS.md) | Full selected-index acquisition, decoded image proofs, safe recovery, permission refresh, and verified scope |
| [Resource and performance review](docs/RESOURCE_AND_PERFORMANCE_REVIEW.md) | Cancellation lifetimes, idle cover work, reader/cache retention, optimization priorities, and bounded remote observations |
| [Resource and performance implementation](docs/RESOURCE_AND_PERFORMANCE_IMPLEMENTATION.md) | Version 0.1.6 implementation mapping, resource budgets, durability boundaries, controlled verification results, and acceptance limits |
| [Startup and reader performance](docs/PERFORMANCE_IMPLEMENTATION_20261109.md) | Version 0.1.19 startup scheduling, pixel-equivalent reader resizing, page retention, durable progress, resource bounds, and scoped software/physical results |
| [Startup and page-turn cost breakdown](docs/PERFORMANCE_COST_REVIEW_20261110.md) | R45 timing boundaries, root-cause and feasibility review, remaining reader/composition work, and a scoped traversal experiment |
| [Performance diagnostics](docs/PERFORMANCE_DIAGNOSTICS.md) | Opt-in 0.1.20 stage tracing, operation correlation, clock boundaries and verification expectations |
| [Measured ARM resize cause](docs/PERFORMANCE_ROOT_CAUSE_20261110.md) | R48 CPU attribution, R49 component comparisons, bounded 0.1.21 implementation, and R51 App acceptance |
| [KOReader technique adoption](docs/KOREADER_OPTIMIZATION_AUDIT_20261110.md) | Implemented, partial and unimplemented cache, grayscale, streaming and refresh mechanisms with upstream references |
| [Streaming resize and prepared next page](docs/PERFORMANCE_STREAMING_PREFETCH_20261110.md) | Version 0.1.22 bounded row storage, verified next-page hits, source checks, resource lifetimes, and selected software/Scribe observations |
| [Native presentation compatibility](docs/NATIVE_PRESENTATION_COMPATIBILITY.md) | Version 0.1.7 read-only display-mode observation, pixel compensation, ordered power events, and scoped verification |
| [Wake continuity and settings](docs/WAKE_CONTINUITY_AND_SETTINGS.md) | Verified no-Home wake on the scoped Scribe, the SGID compatibility correction, and removal of the user inversion setting in 0.1.9 |
| [Wake orientation](docs/WAKE_ORIENTATION.md) | Version 0.1.15 independent holding/scanout mapping, R26/R27/R29 evidence, scoped Scribe wake/controls/native return, and completed device handoff |
| [Live portrait orientation](docs/LIVE_ORIENTATION.md) | Version 0.1.16 active Up/Down rotation, accepted event sensitivity, R31/R32 evidence, scoped R33 checks, and final device return |
| [Return to Kindle navigation](docs/EXIT_NAVIGATION.md) | Header-level native-return affordance and verified local/global Account navigation in 0.1.10 |
| [Protocol implementation](docs/PROTOCOL_IMPLEMENTATION.md) | Rust and native integration and offline verification |
| [Reuse provenance](docs/REFERENCE_PROVENANCE.md) | Reviewed WeRead source hashes and adaptations |
| [Kindle packaging](packaging/kindle/README.md) | Launcher and exact model/firmware acceptance |
| [Third-party notices](THIRD_PARTY_NOTICES.md) | Dependency and asset attribution |

The protocol notes follow [`bilicomics.koplugin` at `a05ffb987c2089ca389cb3462aafd3ee1c0fa36c`](https://github.com/moooyo/bilicomics.koplugin/tree/a05ffb987c2089ca389cb3462aafd3ee1c0fa36c). The WeRead reference is a local working tree, whose provenance and limitations are recorded in the handoff.

Device profiles are design geometries. Desktop rendering and ARM compilation do not establish physical-device compatibility. Supervised checks retain their exact model, firmware, source, and behavior scope; the launcher acceptance allowlist remains empty. No project license or published release has been selected; current artifacts are development candidates.

The earlier corrected 0.1.3 development candidate passes 149 application library tests, 34 native-backend tests, the controlled desktop regression, actual signing WASM checks, download interruption/recovery, an anonymous public category request, and the scoped Scribe firmware-emulator cases. Its exact evidence and the original physical gate are recorded in [pre-device verification](docs/PRE_DEVICE_VERIFICATION.md). Development packages and historical results are linked from [implementation status](docs/IMPLEMENTATION_STATUS.md).

The subsequent [cookie management review](docs/COOKIE_MANAGEMENT_REVIEW.md) fixes
scope, expiration, durable updates, concurrent responses, and account binding.
Its changed source passes 197 library tests and two controlled subprocess
integration tests on `test-env`. The historical Kindle package above was not
rebuilt with these changes; its firmware evidence retains its original identity.

The later [download performance update](docs/DOWNLOAD_PERFORMANCE.md) prepares
missing images in batches of up to 20, preserves per-page durable progress, and
shares bounded capacity between preparation and image work. Its remote snapshot
passes 222 library tests, two recovery integration tests, and controlled fixture
and official-WASM timing comparisons. These measurements retain their synthetic
transport scope; no new Kindle package or physical-device result is claimed.

A later fresh 0.1.3 ARM candidate received the user's supervised physical checks
on one first-generation Scribe running 5.19.5. Offline startup/exit, finger page
turning, same-process sleep continuation, and post-wake touch were checked, with
native control restored and the exclusive device returned to WeRead. The
[physical report](docs/PHYSICAL_VERIFICATION.md) retains the exact candidate and
the initial timed-exit interference. That result predates the 0.1.4 immersive
reader work and does not certify the newer UI or real-account downloads.

The [0.1.6 resource update](docs/RESOURCE_AND_PERFORMANCE_IMPLEMENTATION.md)
implements all nine reviewed projects, including actual blocking-work admission,
recoverable cover caching, verified gray-source reuse, reader image release,
durable bounded event delivery, and background-only sleep maintenance. Its frozen
419-input candidate passes 437 library tests, 62 matching native-backend tests,
the desktop matrices, real-signal checkpoint recovery, 40 complete reader
close/reopen cycles, protocol comparisons, and controlled 45-page acquisitions.
The 28-entry ARM development package passes nine static checks. No new physical
Kindle, live-account download, network-speed, or battery result is claimed.

The [0.1.5 download completeness review](docs/DOWNLOAD_COMPLETENESS.md) adds full
bounded image validation, durable decoded-image proofs, final whole-index
completion checks, safe legacy-cache repair, current permission/index refresh,
and complete chapter-selection restoration. The frozen candidate passes 304
library tests, controlled 45-page production-adapter acquisition, subprocess
recovery, and reader/startup regression. Its 28-entry ARM development package
passes static checks; no 0.1.5 physical-device or real-account result is claimed.
Download scope remains the selected readable chapters and the indexes returned
by the service; a comic-level offline indicator does not establish a full book.

The [0.1.4 immersive reader update](docs/IMMERSIVE_READER.md) uses the full reading
window and shows progress and tools on center tap. Its final source passes 231
library tests, 16 immersive pointer cases, the existing reader/guest checks, and
controlled startup/lifecycle regression. The fresh ARM development package also
received [supervised physical checks](docs/PHYSICAL_IMMERSIVE_VERIFICATION.md) on
one first-generation Scribe running 5.19.5: startup feedback, full-window artwork,
contextual controls, retained reading through physical sleep/wake, and the first
post-wake finger page turn. Native control and touch were restored, and the
exclusive device was released and explicitly returned to WeRead. These checks
used isolated offline demonstration data.

The 0.1.2 update retains the accepted v2 design review and its original 148-test evidence identity.

Earlier evidence remains available: 0.1.1 has 129 application tests, 27 native-backend tests, 28 launcher fixtures, and real-process startup/retained-reader checks. The 0.1.0 visual baseline retains its 22 captures, 16 pointer checks, and 14 state checks.
