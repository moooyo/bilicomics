# Ordinary offline Shelf startup phase measurements

This diagnostic breaks down the startup between Rust entry and the first normal Shelf. It does not change the order of JPEG discovery, state preparation, font registration, the first bootstrap UI turn, cover preparation, attachment, or display work. It also does not repeat the [sleep-cover query A/B](../startup-cover-overlap-20261126/README.md), whose inputs were isolated demo data.

## Measurement

The primary interval starts at the bracketed `CLOCK_MONOTONIC` release of the original indexed Library card, and ends at the first qualifying full-window Shelf renderer return in the same process. An initialized empty Shelf and a populated Shelf with cover placeholders both qualify; the boot screen and a signed-out route do not. Accepted SEND and completion of its same positive update marker are independent secondary intervals. No software or driver event establishes optical visibility.

- The account is fictional and the application runs with `--offline`, never `--demo` or a forced `--screen`. Both closed schema-4 data images keep the same session, 20 fictional comics, cached Store and covers, and onboarded `launch=shelf` settings. The empty control removes only the 20 Shelf memberships and checkpoints before copying. The populated source has 20 entries across two Scribe Shelf pages. No live credentials, cache, database or collection contents are inputs.
- Physical visits are preassigned: three populated launches, then one empty control. Each starts from its own copy, and uses the actual public-card artwork to locate and verify the one Library entry; no second click is used to qualify the frame. The first valid launch is included because there is no before/after comparison. Boot, diagnostic files, run token, staging tree, process and original card release are checked independently. A failed or interrupted launch does not silently become another sample.
- Only the synthetic session directory is mounted on a 64 KiB read-only `tmpfs` to retain the application's `0700` directory and `0600` file checks on Scribe's FUSE media. The SQLite database and cover data remain on the device's usual user-storage filesystem. This is a scoped diagnostic mount, not a proposed account storage change. The original seed objects, stock launcher inode, native navigation and services are restored before exact lease release. No daily installation is deployed.

## Instrumentation

`--startup-phases` is a hidden Linux Kindle switch for `--kindle --offline --shelf-render-trace`. It cannot run alongside demo, a forced screen, general timings, the older startup handoff trace, the lifecycle trace, forced serial Shelf drawing, or captures. The recorder samples only already existing operations. It reuses the backend's native timing callback without a pixel fingerprint; it neither requests a frame nor adds a wait. No per-phase log writes occur. After a successful event-loop exit, a bounded, single-write sidecar records numeric marks and intervals bound to the same run, boot, PID and exact first Shelf renderer timestamp. The control route preserves the usual short-lived card and refresh receipts but does not enable launcher or application lifecycle stage logging during the measurements.

- JPEG capability is the wall and calling-thread CPU of `native_media::initialize`, including helper admission and a capability reply when present. Its CPU excludes the helper's CPU. It is not a JPEG decode, a pure ioctl duration, or an observed two-second timeout.
- Session load is nested inside state client construction. The metadata parent includes it, the full SQLite open, preferences, cache maintenance, catalog and account preparation, deferred intents, resume anchor, and the startup cover list. The cover gate waiter and the prepared-state producer/receiver are separate from these synchronous calls. Font preparation overlaps other work (these faces are static embedded data, not a startup decompression); UI font registration is measured separately.
- The attachment interval is partitioned **per launch** into measured attachment calls, explicit display barriers inside preceding loading-frame transactions, other time in those transactions, and unattributed gaps. Unattributed wall time is not proven I/O or a removable wait. The first Shelf's own mapping barrier is separately placed relative to the renderer; in `render_then_wait` it is strictly after the primary endpoint. CPU, wall spans and cross-thread wall overlaps are not additive. The new observer has not been measured as an A/A treatment.

## Test-env qualification

- The corrected ARM release and worker were built offline with one Cargo job. The deployable application is 74,978,696 bytes; its private copy matched the build output byte for byte. The worker matched the previous ARM worker. The initial Rust compile rejected a backend handle used after its move into the UI driver; the diagnostic now retains its own callback handle.
- The complete Kindle-feature Rust suite passed in a private Linux namespace with the original fixture inputs, `umask 077`, and one test thread: **918 library tests passed, 7 ignored; 26 binary tests passed**. An earlier parallel invocation in an incomplete build mirror failed on shared fixtures, modes, and missing reference images; the corrected run did not change those tests or any runtime guard. All **155** launcher tests passed. The controller passed **14** off-device study fixtures before the final physical series and **15** on replay, including bad-clock/marker refusal, both draw orders, interrupted mount acknowledgement, real private mount permissions, stock launcher restoration, and one confirmed native tab change without retrying its input.
- The [software qualification receipt](software-qualification.json) covers the fixture, desktop, ARM user-mode guard and the first two unsuccessful firmware attempts. Separate Linux desktop software runs used the accepted November 25 build, whose layout, localization and Shelf code match `d293782` byte for byte. The initialized full Shelf rendered 18 visible cards at 1860×2480; the initialized empty Shelf rendered its title, message, two actions and navigation. Both defaulted to Shelf with `--offline` and without `--demo` or `--screen`. XTest exercised Store, Shelf, and Account and exited through the Account control with status 0. This verifies the fixture and UI, not native frame timing. ARM user-mode QEMU also verified that both native profiles reject a start without a launcher-owned display snapshot; it produced no qualifying Shelf or timing sidecar.

- Separate, fresh ARM runs in private Scribe 5.20.1 firmware guests [passed the `ready` and `empty` Shelves](firmware-qualification.json): qualified pixels, Store/return/Account touch navigation, natural exit, and same-boot native restoration. The independent replayer matched 46/44 application marks and spans and five native observations per run to the first Shelf, accepted SEND, and completion of its same marker. The populated draw was `render_then_wait`; the empty draw used `wait_then_render`. These runs used the firmware shell, not a physical Library-card release. The populated guest exercised the missing-helper JPEG fallback; the empty guest included the copied worker but is not a hardware JPEG measurement.
- Three other firmware attempts stopped in the existing native display sample before starting the main app. The first quiet populated attempt reached Shelf and changed tabs but failed a stale Account pixel coordinate and did not retire naturally. A superseded empty guest was stopped during boot. None is a complete timing observation. The final harness does not poll over guest SSH during native handover, gives the guest time to settle after transfers, and uses the current layout. No production timeout or native guard was loosened.

## Physical admission

An initial physical preflight was refused after a confirmed native tab change briefly failed the idle guard. The original tab and pointer were restored, the stages were removed, and the exact lease was released. The card was never clicked and the stock launcher was not moved, so there is no timing from that preflight. The controller now makes a bounded read-only retry of this exact guard result after a positively acknowledged tab change. It does not retry the tab button, relax the guard, or touch the timed card. A new complete plan was prepared with fresh run identities; the three unopened older payloads were removed.

## Physical results

All four measured launches used the **ordinary native Library card** on a first-generation Kindle Scribe with firmware 5.19.5. Each completed naturally, restored the stock launcher and native navigation, unmounted its private session, removed its stage and detector, and returned its own exact lease. The last release was independently confirmed at 16:54:34 Asia/Shanghai on November 26. The preceding pre-card refusal is excluded. See the [per-run numeric projection](physical-result.json). These are different fixture inputs, not an optimization A/B. The intervals reflect uncertainty in the observed XTest button release, not sample variation or optical timing.

| Visit | Shelf | Card to renderer return (s) | Card to accepted SEND (s) | Card to same-marker driver completion (s) |
| --- | --- | ---: | ---: | ---: |
| 1 | Populated | 5.084999–5.086513 | 5.151753–5.153267 | 5.516220–5.517734 |
| 2 | Populated | 5.365481–5.367087 | 5.406219–5.407825 | 5.772025–5.773630 |
| 3 | Populated | 5.255551–5.256683 | 5.340135–5.341268 | 5.703829–5.704961 |
| Populated median | 3 runs | **5.255551–5.256683** | **5.340135–5.341268** | **5.703829–5.704961** |
| 4 | Initialized empty | 4.819834–4.820889 | 4.891169–4.892225 | 5.257136–5.258191 |

### Before renderer return

Selected timings below are in milliseconds. Values in the first two numeric columns are the three populated visits' medians, with observed wall ranges. The empty control is one visit. A dash means no same-thread CPU span was measured. Nested and concurrent rows must not be added.

| Stage | Populated wall median (range) | Populated thread CPU | Empty wall / thread CPU |
| --- | ---: | ---: | ---: |
| Rust entry to first Shelf renderer return | 1,577.0 (1,461.9–1,629.4) | — | 1,212.8 / — |
| First loading frame: wait for its own completed marker | 364.7 (361.4–384.8) | 0.11 | 365.3 / 0.05 |
| Native JPEG capability (all four `available`) | 52.1 (18.9–88.1) | 1.4 | 70.9 / 1.1 |
| Ordinary metadata, inclusive | 80.4 (56.1–81.4) | 31.2 | 116.6 / 31.6 |
| Session load (inside metadata) | 7.8 (4.8–9.5) | 1.9 | 5.2 / 2.0 |
| SQLite open (inside metadata) | 27.1 (20.5–34.2) | 9.0 | 35.4 / 9.5 |
| Cover worker: gate wait | 226.8 (193.7–285.7) | — | 159.1 / — |
| Cover worker: cached read/decode and preparation | 304.2 (274.9–310.5) | 115.4 | 0.02 / 0.01 |
| Prepared-state producer to first UI receive | 0.11 (0.10–0.26) | — | 120.3 / — |
| Attached application to UI Ready | 56.6 (56.4–67.6) | — | 29.9 / — |
| UI attachment operations within that interval | 52.4 (49.3–59.9) | 28.1 | 16.2 / 4.1 |
| Ready to actual Shelf draw | 2.0 (0.5–21.6) | — | 167.1 / — |
| Shelf software draw | 511.0 (501.3–536.4) | 139.7 | 266.6 / 58.0 |

- Card release to Rust entry took 3,623–3,738 ms in the populated visits (median bracket 3,678.525–3,679.657 ms); that includes the protected observer settle and existing launcher handover, not just overhead that can be removed.
- The first loading frame had a real accepted marker and a matching completion in all four runs. The JPEG capability check and **all** measured metadata overlapped its display completion wait in every run. The time spent waiting for the cover gate overlapped that same display wait by **193.1–285.5 ms** on populated launches. Those are intersected timestamps, not additional durations. Removing just the JPEG exchange or accelerating SQLite would not directly remove that time from the observed populated path.
- The attach interval contained **no earlier display-barrier time** in these four runs. The populated attachment calls took 49–60 ms, with 4–8 ms in gaps; intervening frame work did not block them. The 120.3 ms produced-to-received gap was observed only in the empty control and essentially did not overlap a measured display wait. Its cause is not established.
- Every populated Shelf used `render_then_wait`; its own mapping wait was about 0.015 ms and was **after** renderer return. The empty Shelf used `wait_then_render`, with a **166.986 ms** pre-draw wait on the previous loading update. Both are different from driver completion of the Shelf itself, observed about 364–366 ms after SEND. The renderer's wall-minus-thread-CPU remainder is not evidence of additional driver wait or storage I/O.
- The largest measured application stages on the populated first-frame path were the software draw and then startup cover preparation. This diagnostic did not change scheduling or establish how much time could be saved. The [later six-pair experiment](../startup-cover-admission-20261126/README.md#completed-six-pair-campaign) tested admitting the same capped cover work after the first validated loading SEND, retaining the original UI-turn fallback and display barriers.

## Reproduction and safeguards

Use [the bounded projector](project_phases.py) and [the Linux fixture suite](test_phases.py) off-device. The [native controller](native_visit.py) reuses the existing exact-stock, original-inode restoration, process guard, pinned SSH, public-card witness and lease routines. Its [private mount primitive](private_session_mount.sh) is copied verbatim from the November 22 ordinary-Shelf private-session study. It verifies the original tree and the owned mount identity, and refuses foreign, replaced or still-live mounts. The ARM app and JPEG worker, clock/device helpers, fresh synthetic seeds, runtime roster and exact route archives are prepared and byte-compared on `test-env` before a lease. Neither raw traces nor synthetic profiles are written to the repository. The task-owned build cache, staged data archives, copied executables, guest overlays, frame captures and synthetic seed copies were removed after verification; the content-free projection and private clock/closure receipts were retained. Do not retry a released case under a different lease or infer an interval from an unavailable endpoint.
