# BiliComics startup A/B protocol

This is a new BiliComics measurement. Aidoku results are reference material and
must never be entered as BiliComics observations.

## Source and build controls

- Baseline: unchanged main `06ad90b02edb28db43abeed3031932a91dc2f8df`.
- Candidate: the finalized startup optimization source snapshot.
- Both development ZIPs must bind their application, JPEG worker, launcher,
  source manifest, compiler identity and release settings. The compiler
  fingerprint must be identical. Preserve the Bili release profile: thin LTO,
  one codegen unit, stripping, identical target and Kindle features. The Aidoku
  release overrides are not this experiment's baseline.
- Use six Cargo jobs, `CARGO_INCREMENTAL=0`, and `RUST_TEST_THREADS=1` on
  `test-env`. Source preparation and all builds happen outside device ownership.
- Use one already seeded, closed, isolated demonstration data directory. It
  contains no account credentials, acquired comics or remote writes. Compute
  its manifest with sorted relative paths and exact bytes. Copy the identical
  seed separately for every case before the first warmup. Reject links and
  SQLite WAL/SHM sidecars. Do not copy device-collected exit databases back into
  the seed.

## Host-only preparation

`startup_ab.py prepare --config PRIVATE_CONFIG --batch-root NEW_ROOT` creates
a fresh aggregate identity, immutable payload, private transport configuration,
independent case nonces and preparation receipt. It does not import the lease
utility or open a device transport. `NEW_ROOT` is a new immediate child of
`/root`, separate from the software build tree. No prepared run can be rebound
to a replacement lease.

Configuration fields are illustrated in `config.example.json`. Exact paths,
hashes and build fingerprints must be supplied from this task's sealed
artifacts. The fixture fingerprint is SHA-256 of UTF-8 compact canonical JSON
`[{"path": relative, "bytes": size, "sha256": digest}, ...]` with sorted keys
and a final LF. Inputs are sorted by relative path. The default profile and
firmware are scoped to this supervised Scribe; the prepare helper adds only the
same two runtime approval records in each isolated case.

The compiled `startup-clock` helper uses a real POSIX private `/tmp` directory
for its exclusive `0600` evidence files. It does not weaken its mode checks to
accommodate VFAT. The resulting records are copied into the case output only
after recording. Executables, fixture data and output remain under a fresh
`/mnt/us/bilicomics-startup-*` stage outside indexed documents.

## Device batch

The operator reviews preparation, final tools and all host regressions first.
Only `startup_ab.py run --batch-root NEW_ROOT` acquires the shared lease, and
does so immediately before its first device command. One fresh lease covers
the continuous two-warmup plus ten-start batch. Case identities are new
children of this aggregate; they are not reused physical-harness runs.

1. Require active `powerd`, native `awesome` processes running with recorded
   PID/birth/executable identities, a running `statusbar`, and no application
   launcher lock. Capture boot identity, bounded sysfs input roster, original
   brightness/warmth and explicitly selected daily installation/data/entry
   file identities. `install_unchanged` covers these selected regular files
   and recorded absences; it is not a whole-filesystem census.
2. Upload source-bound variants and approved helpers into the unique owned
   stage, verify deployed hashes and the actual unpanned 1860 by 2480 geometry,
   and copy the fixed host seed into twelve separate case directories.
3. Use the actual `com.lab126.appmgrd` shell-integration Scriptlet route with
   an equivalent temporary `entry.sh`. The ordinary Library document entry is
   never rewritten. The instrumented entry exports the case token before any
   exec, then invokes that variant's normal production launcher entry. Both
   production validation passes and its return-before-takeover detach behavior
   remain. Only the detached target gains an independent watchdog parent,
   which directly owns the `--detached` production launcher.
4. Warm up A and B once, then run AB, BA, AB, BA, AB. Each case is offline/demo,
   starts the Shelf route, records performance/lifecycle diagnostics, captures
   the final logical snapshot and exits 4500 ms after software readiness. No
   replayed touch or device setting interaction is used. There is no reboot or
   page-cache eviction; these are warm process starts.
5. The per-case supervisor allows 45 seconds. It binds its direct launcher by
   PID, birth, command and initial environment token, and records the exact
   production lock inode, PID, nonce and generation. On timeout it may TERM
   only that same direct launcher. The production launcher handles its own
   children and normal service restoration. Unknown or active workers are
   never killed by this coordinator and prevent successful handoff. Independent
   native fallback is permitted only after complete owned-process exit and
   exact lock/native identity proof; it never broadens worker signal authority.
6. Every case must finish with normal application/launcher status zero, all
   recorded identities gone, no matching case-token process, no production
   lock, the original native WM identities running and `statusbar` restored.
   Original brightness/warmth is restored and read back before the next case.
7. Collect all results, verify the daily selected files and boot/input roster,
   restore lighting/native state, remove only the precisely owned temporary
   stage/runtime, verify every nonmultiplexed owned host transport has exited,
   and release the exact aggregate lease. Recovery uncertainty retains the
   lease and private evidence. Native cleanup and release precede analysis.

## Endpoints and evidence

`startup-clock OUTPUT -- lipc-set-prop ...` opens and checks the new evidence
file, then samples `CLOCK_MONOTONIC`, writes/closes its small marker, and execs
the native launch request without a shell. The endpoint therefore includes
marker write/close/exec overhead; it is an instrumented native request, not a
physical tap. `CLOCK_BOOTTIME` is recorded only as an additional diagnostic.

- Software ready: the unique Bili `startup/ui_ready` monotonic timestamp.
- Initial startup SEND: first rendered, accepted native frame preceding ready.
- First Shelf SEND: the first rendered, accepted frame whose Slint render begins
  at or after ready. Use `refresh_submit.finished_monotonic_ns`, not callback
  observation time or driver/panel completion. Refresh-only records do not
  satisfy the business-frame endpoint.
- Shell uptime markers are coarse, independent phase observations. Never
  subtract `/proc/uptime` directly from a monotonic application timestamp.
- Capture actual input descriptor nodes and RSS/HWM at the first business-frame
  observation. The input nodes must agree across all twelve cases. This is a
  sample after submission, not continuous peak-memory measurement.
- Preserve the entire early framebuffer read in the last pair, including
  whole-frame differences caused by asynchronous covers. Do not require all
  covers ready at the first Shelf SEND. Also capture a separate framebuffer at
  target `ui_ready + 1000 ms`, using an absolute monotonic wait and recording
  actual start/slip. A late capture is reported as late, never backdated. It is
  outside the startup endpoint. Final PNGs remain logical Slint snapshots before
  framebuffer polarity compensation and are compared as such.
- Analyze all lifecycle records and reject any case with suspend/resume or
  unknown lifecycle transitions. A final active state alone cannot exclude an
  intervening system suspend.

`analyze_startup_ab.py --batch-dir NEW_ROOT` runs only after successful native
restoration and exact lease release. It excludes both warmups, publishes each
pair and ranges as well as medians, preserves early framebuffer differences,
and keeps accepted, driver-completed and optical observations separate. No
optical completion, physical-touch acceptance, cold-boot performance, other
models, battery improvement or universal percentile is claimed.

## Host verification

Run the two Python fixture files with `python3 -I` on `test-env`. The isolation
flag avoids an unrelated `/root/inspect.py` shadowing the standard library.
Compile `startup-clock.c` both with host `cc -O2 -Wall -Wextra -Werror` for its
fixtures and `. /root/.cargo/env; zig cc -target arm-linux-musleabihf -static`
for the device. Root must freeze the final helper/tool hashes after all fixes;
earlier compiled helpers must not be used with a newer protocol.
