# V2 Implementation Review

Reference: the user-supplied Downloads design, SHA-256 `62425b5a36df75457fd1769667b5fe88ef06a9d7c6c566a9608a12549aa88414`.

Baseline: application 0.1.1. This is a review of the implementation against the supplied design, including reachable interaction states; it is not a new product specification.

## Source and comparison

All 26 templates were statically extracted without running their JavaScript on the operator's machine. The v1/v2 comparison separates actual structure/style/state changes from generated resource IDs. Reference rendering and computed-layout inspection run through `ssh test-env`.

The reference set contains 21 application default views and 36 additional states. Five additional renders measure active/error download drawers and Scribe storage directly, including the complete Paperwhite chapter-list viewport. A separate, immutable 0.1.1 executable produced 31 targeted before images. Unavailable authentication-success states were recorded as gaps rather than simulated by a real login. Separate visual-only fixtures set the real generated view models without constructing credentials; these results remain distinct from actual application-process checks.

Generated evidence lives under `artifacts/reference-v2/`, `artifacts/verification/design-v2-before-0.1.1/`, and the current v2 verification directory. Earlier design and application evidence are retained.

## Confirmed correction areas

| Area | Corrections made during the review |
| --- | --- |
| Shared controls | Replaced font chevrons with rounded SVG arrows, corrected button weight and icon sizing, and retained intrinsic bounds, vertical alignment, keyboard focus, and minimum touch areas. |
| Status and selection | Removed decorative status checkmarks and follow-plus suffixes. Checkmarks remain for actual multi-selection. Single-choice cards retain a stable outer border and use an inset selection stroke. |
| Catalog | Corrected the selection layout, column widths, margins, locate control, store-header height, and full-row search hit area. Footer content is centered inside full-height wrappers with an uninterrupted top rule. Filter counts, signed-out local-content access, and no-history cover routing were corrected. |
| Search and input | Separated committed query from underlined composition, centered composition and caret, preserved uncommitted input during editing changes, and distinguished numeric comic IDs from ordinary words. Candidate capacity/paging, return-group alignment, empty-result button bounds, and duplicated status text were corrected. |
| Download confirmation | Blocked enqueue preserves selected chapters and authentication/navigation. Only an accepted or durably deferred task clears its selection. Returning from authentication retains bounded IDs, obtains fresh account permissions, restores the readable intersection, and requires another confirmation. |
| Reader | Corrected portrait/spread headers, footer spacing, temporary-access styling, spread width threshold, independent-cover blank side, strip-end restoration, and menu-preserving start navigation. Actual pointer checks exposed an unpositioned left tap zone; its bounds are now explicit. Settings rows have stable outer bounds. Thumbnail preparation exposes loading feedback until actual rasters arrive. Permission/cache text uses confirmed state. |
| Downloads | Kept resume-all available after pausing, retained the actual progress bar while paused, distinguished failed/queued/active drawers, corrected progress-fill geometry and removal-capacity totals, and limited local reading to actual local content. Exact drawer measurements corrected an oversized header/task area that clipped the final Paperwhite chapter row. Queue priority now affects persisted acquisition ordering. |
| Account and authentication | Corrected verified-name presentation, offline connection feedback, fixed QR states, success layout, icon dimensions, onboarding controls, and storage segmentation. Publishing a new account immediately clears the previous account's visible models and permissions, including on subsequent initialization failure. |
| Purchase | Removed the obsolete success illustration, preserved locked selected-offer readability, displayed the insufficiency difference, and refreshed a complete quote rather than combining a fresh wallet with an old offer. |
| E-ink feedback | Keyboard down/up use A2 dirty-region updates without spending the list-refresh budget. Ordinary release feedback is distinct from explicit content actions. Lists use six UI updates followed by a full refresh on the seventh; the reader's separate six-page policy is preserved. |

## Preserved behavior and evidence limits

Startup remains a real first scene with a bounded activity indicator while expensive preparation runs off the UI thread. Sleep keeps the same reader process, route, position, settings, and rasters; the v2 review must not restore the former screensaver-exit behavior.

The production client continues to use confirmed source or local state. Demo illustrations, account amounts, and chapter data are isolated fixtures. Prototype timers, automatic successful payments, and unproven entitlement changes are not implementation contracts. Unknown transactions are persisted/reconciled without retransmission.

Waveform request mappings, source fixtures, desktop pixels, simulated operating-system services, ARM compilation, and actual physical Kindle behavior are separate evidence. No new hardware capability or device/firmware combination is accepted by matching a design image.

## Verification results

All builds, tests, runtime checks, and rendering ran through `ssh test-env`. The operator's Windows machine was used for editing, static extraction, transfer, and viewing saved images. The final development candidate is application **0.1.2**, snapshot 9.

The [verification summary](../artifacts/verification/design-v2-0.1.2/verification-summary.json) records the exact inputs and evidence. The final [source-input manifest](../artifacts/verification/design-v2-0.1.2/source-inputs-round9.sha256) has SHA-256 `0aaead665f56074d7c6709f428b0b4c68d38d49634efa685d8624d70322db0b3`; the desktop executable has SHA-256 `dc13dc374525fe98e3bb43d2deed9526f560f0839c47b22f37701cfd471048c9`.

| Check | Result | Recorded identity |
| --- | --- | --- |
| Application library tests and desktop build | 148 passed, none failed or ignored; desktop build completed without warnings | Snapshot 9 |
| Default and paused application views | 23 captures passed, including all 21 default design views and both paused-download views; changed regions received visual review | Snapshot 9 |
| Actual pointer interaction | 8 cases passed, 54 real clicks, 70 images; paused progress stays at its original 40% | Snapshot 9 |
| Authentication and Guide view fixtures | Six authentication states and two Guide projections; no live authentication | Snapshot 9, view-model fixtures only |
| Complete earlier v2 matrix | 44 application captures plus 11 targeted corrected-state captures passed and received owner visual review | Snapshot 8, retained unchanged |
| Reader interaction | Original 9 cases passed, including both cover directions, the landscape width threshold, menu-preserving beginning, and strip viewport restoration | Snapshot 8 |
| Isolated guest reader | 5 cases passed, including actual Guide dismissal/persistence and loading/error boundaries | Snapshot 8 |
| Startup and retained-reader protection | 3 checks passed; animated startup frames and the same PID/reader page across sleep, followed by a real page turn | Snapshot 8 |
| ARM development package | Release build and all 22 allowed archive entries, digests, CRC, modes, timestamps, and runtime-account-data exclusion passed | Snapshot 9 |

Snapshot 9 changes only three Slint layouts and the paused-download progress-bar projection relative to snapshot 8. The [scope record](../artifacts/verification/design-v2-0.1.2/source9-change-and-scope.json) identifies those four files and excludes reports from binary inputs. Earlier reader, guest, and lifecycle results retain their original executable identity; they are not relabeled as snapshot 9. Their implementation was unaffected by these final presentation changes.

The final XTest harness sends and synchronizes pointer motion, allows an 80 ms event-loop interval, then sends the original 80 ms press/release. Batched synthetic motion/click delivery was timing-sensitive in the earlier headless run. Original failures and separate diagnostics are retained; no production fix is attributed to that input-delivery investigation. Coordinates, expected destinations, timeouts, and acquisition-order assertions remain intact. The paused-bar assertion was corrected to require the design's retained bar and unchanged progress.

Authentication-success fixtures are explicitly `view_model_fixture_only=true` and `auth_verified=false`. They use generated UI models without constructing an application session or source client. They establish presentation only. No real Bilibili login, purchase, coupon consumption, recharge, or physical Kindle acceptance is claimed.

The [0.1.2 ARM development ZIP](../artifacts/verification/design-v2-0.1.2/bilicomics-kindle-0.1.2-dev.zip) is 49,923,219 bytes, SHA-256 `02a85557728b3bb612929afa6e96c89a9d0cd5434ca98bd0e6514d54bb057038`. Its static ARM EABI5 hard-float executable is 64,628,976 bytes, SHA-256 `76a8c9969120903db6b6df532eb8cf469932a6f169d0593134e5d0ef0ead7842`, without an interpreter or required shared libraries. Exact checks are in the [package report](../artifacts/verification/design-v2-0.1.2/package-verification.json). The device/firmware acceptance allowlist remains empty.
