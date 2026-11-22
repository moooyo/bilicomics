# Handoff: first Shelf render and installed Kindle entry

For the consolidated **keep / do not ship / next engineering work** decisions, see the [startup decision register](verification/startup-decisions-20261121/README.md). Its [six-case launcher replay](verification/startup-decisions-20261121/launcher-replay.md) completes the previously proposed offline breakdown without a new device run.

**Primary metric correction (November 21):** use [ordinary native card release to the first Shelf render](STARTUP_AND_SLEEP.md#primary-startup-metric-first-shelf-render). Require the normal layout, entries or appropriate empty state, and navigation in the same launch; neither covers nor a second tap are required. Separate render completion, accepted SEND and any same-marker driver completion. The timings below retain their historical **cover-complete frame** definition; do not relabel them.

**New physical baseline:** the [two automatically activated Scribe diagnostic launches](verification/startup-interactive-kindle-20261121/README.md#scribe-results) completed the first qualifying software draw in approximately **6.451 s** and **6.727 s**. They used the same binary and isolated demo Shelf; this is not a speedup comparison. The normal installation was restored after each launch. The second release was verified at 12:11:52 Asia/Shanghai.

**Subsequent first-render A/B is closed:** the [same-binary device comparison](verification/startup-shelf-ab-20261121/README.md) confirmed both actual render orders but not a material gain: 4/6 pairs faster, paired median improvement 14.8–17.3 ms (threshold: 5/6 and at least 100 ms). Both warmups and all 12 measured launches returned native control and released their own leases. No new package was installed.

**Library preflight, November 21:** [worker-only Library admission](verification/startup-library-admission-20261121/README.md) saved about 3 ms in the isolated `test-env` stage. Both Scribe warmups and the first timed control completed, then native navigation failed before the timed candidate's card click. This series ended with zero complete pairs; four owned leases were returned and the ten remaining visits were not attempted.

**Library preflight, November 22: do not deploy for speed.** The [independent six-pair Scribe comparison](verification/startup-library-admission-20261122/README.md) is complete. From ordinary card release to the first complete Shelf software render, the candidate was faster in 3/6 pairs and its paired median was **64.139–66.997 ms later**. The first launch in every pair was faster regardless of arm; this is not proof that the candidate itself causes a slowdown. All 14 visits restored native control and released their leases. The ordinary installation is unchanged.

**Card-to-header timing is now split.** The [independent identical-launcher A/A](verification/startup-clock-aa-20261122/README.md) closed all ten Scribe visits with a normal Shelf and verified return. Excluding two warmups, the eight measured launches had a median of **6.324215–6.325388 s** from card release to renderer return, not a speedup. The second launch was faster in 2/4 pairs, so do not apply a first-launch correction to the earlier A/B. The broad launcher span was 3.220 s from the post-guard shim mark to the pre-invocation mark, including the required two-second observer settle. The [separate prepared-state handoff study](verification/startup-prepared-handoff-20261122/README.md) targets UI receipt, which this A/A did not observe.

**Prepared-state receipt, November 22: keep delivery and refresh behavior.** A [separate three-visit run](verification/startup-prepared-handoff-20261122/README.md) recorded the pre-send to actual UI receive gap as 0.082, 105.155 and 33.879 ms. None reached 250 ms. The older four delayed *log records* are not receiving timestamps. All three diagnostic apps exited normally and the installed version was restored after every visit.

The [historical desktop interactive measurement](verification/startup-interactive-shelf-20261121/README.md) verified 6/6 real input responses, with a median upper bound of 180.376 ms from host launch. A separate missing-cover control remained interactive. No native launch was made for this measurement; the shared maintenance closure was subsequently reported as cleared for ordinary lease requests, not as evidence of a Kindle timing or a reservation.

**Private-render decision: keep the installed application and its first-Shelf render ordering.** Source commit
`b267e3d29c25` records the six Rust files measured as `shelf-private-rgb-v1`
on top of `ab52019`. The [build receipt](physical/shelf-render-20261121/build-receipt.json)
identifies a working-tree build: the commit was made after testing. The
comparison used `57265de` as its control, not the older November 19 application.

## What changed and what passed

Only the first ready, unobscured Shelf may render into private RGB memory
while the previous panel update finishes. Writes to the mapped framebuffer
still wait for that update. The new Shelf SEND must complete on its own
marker. Loading feedback, Reader, sleep/wake, and unsafe or ineligible frames
retain the original ordering.

- On one first-generation Scribe (5.19.5), two warmups and six alternating
  pairs used a genuine native Library card and isolated offline demo data.
  From card release to driver-confirmed complete Shelf, the arm medians were
  **7.333 s → 6.802 s**; all six pairs improved, with a **585 ms median paired
  improvement**. All 14 launches preserved loading feedback and matched final
  Shelf pixels. This is neither optical timing nor a live-account measurement.
  [Paired results and limits](physical/shelf-render-20261121/README.md).
- On `test-env`, 898 library, 18 application-binary and 249 backend tests
  passed (1,165 total, four existing tests ignored). PW6 and Scribe desktop
  Shelf captures matched the references. Only the main application changed
  among the 49 package members. [Software receipt](physical/shelf-render-20261121/software.json).

## Installed state to preserve

The [separate ordinary-entry smoke test](physical/shelf-render-20261121/rollout-result.json)
verified the production arguments, lifecycle-ready record, framebuffer and
input descriptors on the same Scribe. It ended with a managed exit. It did
not exercise login or purchases. The installation was retained; BiliComics
restored native control and released its own lease at **04:47:08 Asia/Shanghai**.

There is **one** actual `/mnt/us/documents/bilicomics.sh` and one matching
catalog row. Preserve `/mnt/us/bilicomics/launch.sh`, both binaries under
`/mnt/us/bilicomics/app/`, data and the `scribe 5.19.5` runtime allowance under
`/mnt/us/bilicomics/data/`. The same-volume uppercase lookup can return a
different inode without a second directory entry. Do not remove either spelling,
create a second entry, or rerun an installer that requires an empty device.
The public baseline is `test-env:/var/tmp/kindle-build-coordination/bilicomics-production-20261121.txt`.

## Where to resume

1. **The baseline launches, render-order and Library-parent comparisons, identical-launcher A/A, and three-visit prepared-state study are closed.** Do not infer current device availability from any historical handoff. A future physical batch needs a fresh lease immediately before actual device use.
2. No rebuild or installation is required to continue analysis. The
   [per-frame breakdown](physical/shelf-render-20261121/frames.json) and
   [decision checks](physical/shelf-render-20261121/decision.json) are in Git.
   For raw replay on `test-env`, follow the [replay instructions](physical/shelf-render-20261121/README.md#replay).
   The allowlisted raw archive and development ZIP are retained outside Git
   under `artifacts/verification/shelf-render-20261121/`; a fresh clone will
   not contain them. Do not take a device lease for this work.
3. The [three display-capture visits](verification/startup-display-sampling-20261122/README.md)
   also closed. The median helper wall time was 300.502 ms, including
   213.896 ms in `poll` and 8.388 ms in process creation. The single-arm
   median from card release to Shelf render was 6.430–6.432 s, not a gain.
   In the [completed three-visit poll split](verification/startup-display-poll-20261122/README.md),
   81.5–90.1% of poll time preceded observed EOF; just 20.195–42.504 ms
   followed it. Card release to first Shelf render was 6.401–6.403 s at the
   median, also not a speed comparison. Stop the exit-detection optimization;
   next compare metadata-first Shelf rendering with deferred cover attachment
   off device. Do not subtract this helper interval from an older launcher's
   410 ms observation, omit live permission checks, or deploy the Linux parser prototype.
   Preserve the activity indicator and the full observer settle. For new builds, reuse the shared `test-env` toolchain,
   `/opt/kindle-toolchain/build.lock`, and a private target. One Cargo job
   was sufficient for the test build after two jobs exhausted memory.

The [pre-ready investigation and cached Shelf comparison](STARTUP_PRE_READY_20261121.md) retains the catalog optimization for populated caches. Two different experiments rejected changing startup cover copying: [moving the copy to the worker](verification/startup-cover-worker-20261121/README.md) gave one clearly earlier complete desktop Shelf in six pairs; [direct decoding on an ordinary offline Shelf](verification/startup-direct-png-20261121/README.md) gave none. The latter saved only 0.529 ms of combined cover CPU at the paired median. Keep the current cover code, batches and activity frame. The installed baseline was not changed.

The [subsequent desktop wake trial](verification/startup-desktop-wake-20261121/README.md) confirmed that the maintenance timer, not a render notification, first admitted workers on the ordinary stock desktop. A research-only wake after successful software submission advanced the first complete Shelf in **6/6 pairs** (56.338 ms median paired improvement) while retaining visible feedback. Keep only the opt-in source diagnostics in product code: the private dependency patch is not for deployment. No Kindle was used or updated. For the next device-relevant investigation, break down `state-cache-maintenance` on the ordinary offline path before changing its behavior; do not port the desktop timer change to native startup.

The [cache-maintenance follow-up](verification/startup-cache-maintenance-20261121/README.md) closed that question for the ordinary twenty-comic fixture. The original seed rewrote one semantically unchanged recency row on each independent launch, whereas product-written text changed no rows. The **same** desktop executable took 7.481 vs 0.490 ms in maintenance; the difference was concentrated in commit (six pairs, no actual evictions). Keep the transaction, protection, durability and retry behavior. Use product-written inputs for steady-state studies and preserve the original seed for first-rewrite comparisons. This is diagnostic evidence, not a performance change deployed to Kindle. A device-relevant next step is offline replay of the archived native launcher intervals, not a timer or cache-policy change.
