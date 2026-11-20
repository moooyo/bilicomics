# Handoff: first Shelf render and installed Kindle entry

**Decision: keep the optimization and the installed application.** Source commit
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

1. **Leave the Kindle alone pending Aidoku's recovery closeout.** After its
   05:06 release of an earlier batch, Aidoku reported that a different batch
   stopped before the first click and its SSH endpoint became unreachable.
   At this handoff it was recovering under its original owner/token. This is a
   peer report, not a BiliComics probe. Do not test the connection, take over,
   or clear `test-env:/tmp/codex-kindle-physical-test.lock`. A future physical
   batch needs a fresh lease immediately before actual device use.
2. No rebuild or installation is required to continue analysis. The
   [per-frame breakdown](physical/shelf-render-20261121/frames.json) and
   [decision checks](physical/shelf-render-20261121/decision.json) are in Git.
   For raw replay on `test-env`, follow the [replay instructions](physical/shelf-render-20261121/README.md#replay).
   The allowlisted raw archive and development ZIP are retained outside Git
   under `artifacts/verification/shelf-render-20261121/`; a fresh clone will
   not contain them. Do not take a device lease for this work.
3. If startup work continues, investigate the interval **before** software
   readiness: the prepared-state result sometimes reached the UI only after
   a loading-frame wait. That is an observation to investigate, not proof
   that removing a frame helps. Preserve the activity indicator and the full
   observer settle. For new builds, reuse the shared `test-env` toolchain,
   `/opt/kindle-toolchain/build.lock`, and a private target. One Cargo job
   was sufficient for the test build after two jobs exhausted memory.
