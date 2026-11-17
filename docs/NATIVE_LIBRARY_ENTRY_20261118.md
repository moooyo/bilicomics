# Formal Library entry candidate: 2026-11-18

## Status

The implementation, remote software checks, first formal-entry timing comparison
and separate framebuffer/entry-retirement qualification are complete.
The Library entry change was merged to `main` by operator request on 2026-11-18.
**Manual reader sleep/wake and UI-exit qualification remain pending.**
The merged implementation is `39fac822a982868078201e605e50bdacf1dcf3ed`;
`e8d06e3` records the automatic qualification on the merged branch. The merge
does not deploy the package or establish manual device acceptance.

The first four attempted timing batches stopped before dispatching an
application. After the harness corrections and two bounded initialization
selfchecks, `lib-ab-20261117-2e297ef4` completed all fourteen launches.
The six primary pairs show a first Shelf SEND median of 6820.739 to 6288.531 ms,
with five pairs faster. This timing evidence does not establish optical panel
completion or same-process screensaver behavior. Every executed device batch
has been restored and its exact owned lease released.

A later independent qualification batch stopped at its first baseline identity
observation and completed no qualification cases. Its failed status and full
recovery are retained separately; the successful timing comparison is unchanged.
After a reproduced observer-only correction, a fresh four-case qualification
batch completed with identical visible framebuffer bytes and successful entry
retirement/cancellation recovery. It does not replace the required reader cycle.

## Implementation

The new single package document, `documents/bilicomics.sh`, retains the old
source launcher's `BiliComics for Kindle` title declaration and executes:

```sh
exec sh /mnt/us/bilicomics/launch.sh --library
```

There is no new `DontUseFBInk` or Icon header. The existing shared handler's
descriptor, splash and pipeline behavior is therefore retained. No shared
handler, MIME association, scanner implementation, registry entry, native
daemon or interposition module is changed.

The waiting entry starts the original `setsid sh launch.sh --detached` worker
and waits for that direct child's actual exit status. It does not acquire the
launcher lock, display, input or sleep protocol. Cancellation binds the child
to PID, process birth, live state and PPID; it waits for the existing
generation/nonce publication after complete cleanup traps are installed,
requests TERM once, and continues waiting for the real status. A blocked
preflight file read can defer cancellation; there is no unprotected KILL
fallback. Retiring only the waiting entry leaves native recovery with the
independent worker.

Ordinary no-argument and `--detached` admission are unchanged. This project has
no production KUAL integration or KPM installer; future direct hooks must keep
detached admission. The application, JPEG worker, Rust sources, rendering,
grayscale choices, input barriers, preferences and caches are unchanged.

Source provenance, original licenses, selected immutable upstream blobs and
the independent BiliComics ownership adaptation are recorded in
[the reference directory](research/aidoku-native-library-entry-20261117/README.md).
Aidoku results and test counts are not BiliComics acceptance.

## Software qualification

All checks ran on `ssh test-env`; no Windows-local test or build ran. No Rust
rebuild or ARM execution was needed. The 544 complete binary-input path/hash
pairs exactly match the previously qualified `530f151` build. Its native
manifest remains `8537123d6b1dc0c75a048b09d1204664988a6c1668ea35d721bfbac641ff49c6`.
The same map has a different `run_suite` serialization order; both identities
and the first verifier's pre-test ordering failure remain explicit.

| Check | Result |
| --- | --- |
| Launcher lifecycle suite | 146 passed, including 17 new cases |
| Library document/package regressions | 25 passed |
| Existing startup/spatial/media package regressions | 18 / 5 / 3 passed |
| Production shell syntax | Both passed |
| New development ZIP static checker | 20 checks passed; 49 entries |

There are 197 distinct test cases. The new 17 also ran in a preliminary subset,
so the stored logs contain 214 executions. They are not 214 unique tests.
The new cases cover real worker status, early/repeated cancellation,
readiness, PID-birth rejection, independent recovery after waiting-entry KILL,
same-process fixture sleep/wake, duplicate admission and old-entry compatibility.

The rebundler accepts only the exact old 45-entry or new 49-entry schemas. It
adds one document and three provenance notices, updates the launcher and two
packaging descriptions, and preserves the other 42 old entries byte for byte,
including their original modes and timestamps. It pins the old ZIP, both
executables and the complete binary-source manifest. The new checker binds
the exact document and canonical launcher bytes to the selected source.

| Selected artifact | SHA-256 |
| --- | --- |
| Application ELF, reused | `04fc6f93b4327911ca1a3bb5200175c9f2e13fb21ac5e56280012ff44b6e2e59` |
| JPEG worker ELF, reused | `12c8988c6a93cade9923d54bef784b774e13cb5e3b042f2e72ea5ecb6b897beb` |
| New ZIP | `c57d5fc3674f1a0de448b85beb2552ee0959b83756c3e8f62afb559bf62a4882` |
| Production launcher | `584ad6065565bf5529fd4ef17393a454a0db811824c685e104a360c82080dea4` |
| Production document | `7ac07e2a4e2b790642ff88787deaf05988e1bb8786d418b45bfd092d64d023f4` |

[The software bundle](verification/native-library-entry-20261117/README.md)
retains actual commands, warnings, complete logs, failed verifier attempt,
package provenance, entry audit and a source-mapped export manifest. The ZIP
retains `device_verified=false`.

## First completed formal-entry timing comparison

Batch `lib-ab-20261117-2e297ef4` retains two warmups followed by six alternating
AB/BA pairs (three ABBA blocks). Both variants open the actual indexed
`file:///mnt/us/documents/bilicomics.sh` through the native application manager.
The same product ELF, JPEG worker, common instrumented launcher, warm offline
fixture and device boot are used. Only the document's final `--library`
argument differs. Common research instrumentation and its source hashes are
separate from the production package; no parent is inserted between the
waiting entry and its worker.

| Endpoint from the native request | Detached control median, ms | Library-wait median, ms | Candidate faster |
| --- | ---: | ---: | ---: |
| Software Ready | 6055.705 | 5582.248 | 4/6 |
| First Shelf accepted SEND | 6820.739 | 6288.531 | 5/6 |

The difference of SEND medians is 532.208 ms (about 7.8%); the median paired
reduction is 424.326 ms. Candidate pair 01 was 33.072 ms slower and remains in
the distribution. No run or slow sample was replaced or removed.

Each request follows eight seconds of native stabilization, excluded from its
measured interval; observed brackets are 8307.861–8585.456 ms. All 28 private
clock records have mode `0600` and match the archived bytes/hashes. The source
clock and its pre-exec sampling origin remain unchanged. The accepted business
frame must render after the real Ready timestamp. Neither endpoint is a
physical tap, optical completion, complete-cover or input-ready measurement.

All fourteen final logical Slint captures have identical full RGBA pixels at
1860x2480. These exit-time snapshots are not live framebuffer qualification.
Separate capture and lifecycle batches are still required.

This batch preserves the **new** Aidoku-only Scriptlet registration captured
in the current inventory, rather than the old registration used by earlier
harness attempts. Its selected read-only projection is
`cfc0381d6262afe7aff58534a832a6c15eea3903f07f35b831c69c7102acf804`.
The wrapper, Aidoku document, launcher, application/helper and cover fixed-file
snapshots remain unchanged before/after. No registry database or Aidoku file
is restored from an old snapshot. The current wrapper's passthrough policy is
an installed integration condition, not an independently rebuilt binary proof.

Native state, lighting, BiliComics' original absent paths/catalog rows,
registration, boot and input metadata were restored/unchanged; all connections
closed. The exact lease interval was `2026-11-17T20:31:56.706115+00:00` through
`2026-11-17T20:39:31.192830+00:00` (about 7 min 34 sec), followed immediately by
the handoff notification. The preceding temporary-script absolute timings
must not be treated as this formal entry's baseline.

[The formal AB export](physical/native-library-entry-20261118/formal-ab/README.md)
retains all fourteen case projections, raw integer endpoint clocks, exact
statistics, independent audit, safe restoration/provenance projections and one
original logical PNG. The complete private originals remain source-hash bound.

## Independent qualification attempt

The subsequent `lib-qualify-20261117-b130366f` batch failed on its first
`baseline-qualify` observation with `identity-unproven`. It completed no pixel,
entry-retirement or cancellation qualification case. Its observer log records
the entry's `cmdline` disappearing during an identity read. The entry's actual
admission status was 0; recovery bound the detached worker and its generation,
requested TERM, observed worker status 143, and restored native control.

Original installation/document/catalog absence, lighting, current registration,
selected Aidoku files, boot and input metadata were preserved or restored.
Connections closed and the exact lease acquired at
`2026-11-17T21:00:50.786927+00:00` was released at
`2026-11-17T21:02:15.250484+00:00`. The failed result remains independent of the
successful AB. The observer correction was withheld until a host-only fixture
reproduced this failure; the reproduction and fresh batch are described below.
No product source or selected package change followed from this failure.

[The failed qualification export](physical/native-library-entry-20261118/qualification-r1/README.md)
keeps the original failed observer status, separate cleanup results and exact
release projections with source hashes. Cleanup success is not a qualification
pass.

## Completed automatic framebuffer and entry qualification

The exact missing-`cmdline` race was reproduced on `test-env`: the initial
live-state and readable-file checks succeed, then the original entry retires
before the shell opens its command line. The unchanged R6 observer returns
unobservable (2). The new R7 observer rechecks the same PID/birth after metadata
failure and returns retired (1) only when that original process is definitely
gone or replaced. A still-live mismatch or unreadable identity remains 2;
metadata failure never proves a live owned entry. All candidate-content,
worker, parent, generation, context and signaling gates remain unchanged.
The original 18 observer and 13 targeted race fixtures passed, including this
exact readable-to-open interleaving. The selected product package is unchanged.

R7 is a research observer patchset, not an application version. Its source
freeze is `84c4c164500f485b3f7ac841397aa24da7d46bdb566774ca8b1e9f2f7bafec26`;
the common controller retains its original bytes and tool revision 6. The fresh
batch `lib-qualify-20261117-7ddd041f` follows the failed batch without rewriting
it. Eleven new host binding/recovery/construction checks and independent review
passed before device access. Native identity admission is Q<=128 and each
complete command remains below 8192 bytes, including its marker and header.

| Actual case | Observed result |
| --- | --- |
| Detached baseline | Live content with retired admission entry; normal worker/entry statuses 0. |
| Waiting Library entry | Entry alive at content; normal worker/entry statuses 0. |
| Entry-only SIGKILL | Bound waiting entry retires; independent worker survives, exits 0 and restores native control. No entry exit status is invented. |
| Entry-only TERM | Bound entry requests cleanup; real worker and entry statuses are both 143, with native recovery complete. |

The application retains its production runner parent. The pinned observer
checks live ancestry through that runner; intermediate runner birth records
are not fully archived, so a complete offline replay of that ancestry is not
claimed. The worker's direct relationship to the waiting entry is recorded.

Both qualified captures contain the same 405-byte geometry header and exactly
4,612,800 identical visible Gray8 bytes at 1860x2480; there are zero differing
pixel bytes. The entire raw captures have SHA-256
`299a4d97e6217aa25ea40ae96c84553ec3cc79c24e840418808495254a591104`.
The captures occur while the application is still alive. This establishes
actual visible framebuffer-memory equality, independently of the earlier
logical Slint PNG comparison; it does not establish optical panel completion.
All twelve private clocks have mode 0600 and match their archive mirrors.
Qualification timings remain outside the six-pair primary AB statistics.

All four cases completed and recovered. Current registration and the selected
Aidoku files were unchanged, original Bili absences/catalog were restored,
native control and lighting were restored, boot/input metadata matched, and
owned connections closed. The exact lease interval was
`2026-11-17T21:25:40.633000+00:00` through
`2026-11-17T21:28:43.952785+00:00` (about 3 min 3 sec), followed by the handoff
notification. No reader sleep/wake or human UI-exit observation occurred in
this automatic batch; its two ordinary application exits use the stated timer.

The unchanged complete qualification analyzer passed once, and the independent
stored audit found no blocking action, identity, capture or restoration anomaly.
It also retains the candidate's pre-opening catalog size of 85 versus the
95-byte document; the post-opening value matches the document byte count.
The literal mismatch is not normalized away or used to discard the case, and
the installed extractor's size unit remains independently unqualified. The
primary AB is unchanged.

[The automatic qualification export](physical/native-library-entry-20261118/qualification-r2/README.md)
preserves the four case results, framebuffer comparison, clock and restoration
records, and source hashes for the frozen analyzer and independent audit.

## Earlier bounded observations and harness corrections

The initial read-only inventory found both known document spellings and the
canonical `/mnt/us/bilicomics` install absent; the two document paths had zero
catalog rows. This is a bounded canonical-layout observation, not a whole-device
census. There is no existing BiliComics icon/cover identity to claim preserved.
Qualification temporarily creates one actual indexed entry and must restore
those absences, without permanently installing the application.

The original default shell MIME association and shared executable remain:

- handler: `tech.hackerdude.shell_integration.launcher`;
- command: `/var/local/kmc/bin/sh_integration_launcher`;
- executable SHA-256:
  `2b3db59ed7856b3f5026e3837f7ef07fc9bb6bc1c042274b48b3cd3fc2e04abc`.

The earlier failed batches are retained separately; none supplies a latency sample:

[The bounded checkpoint export](physical/native-library-entry-20261118/checkpoint/README.md)
keeps their failed statuses, independent recovery receipts and initialization
selfchecks as separate fixed-field projections, with original source hashes.

1. `lib-ab-20261117-59ba43e0` stopped during initial catalog polling. SQLite
   returned BUSY (5) while the normal scanner wrote the index. The harness's
   `set -e` exited before its intended polling loop. A narrow, audited rc5/6
   retry was added within a shared host-monotonic deadline; other errors and
   late successes remain fatal.
2. `lib-ab-20261117-a2c375d4` successfully obtained one correctly titled row,
   but the harness confused appreg's `MT:text/x-shellscript` contentId namespace
   with `Entries.p_mimeType`, which is `text/x-shellscript`. The actual row
   became a regression fixture. Its size was 85 bytes, consistent with the
   baseline document; no size mismatch caused this failure.
3. `lib-ab-20261117-7a5ffc65` stopped before URI dispatch because the observer
   refused its initial context. No observer PID record, request, production
   admission or launcher lock was created. Its automatic recovery retained
   the lease because a post-dispatch recovery receipt could not exist. A
   separate recovery object proved the no-start state, reused frozen
   restoration/closure, and released the original owner/run/token. The failed
   report was not rewritten as successful.
4. `lib-ab-20261117-0c9ca0a3` passed observer initialization, then the ARM clock
   helper rejected its first, still-empty record before dispatch. Saved live
   metadata shows UID 0, one link, size zero and mode `0777`; the helper
   correctly requires mode `0600`. The observer was cancelled without an app
   admission. A separate recovery retained every ownership/no-dispatch check,
   restored original absence and native state, closed transports, and released
   the original exact lease. Its failed timing report remains unchanged.

The separate tiny selfcheck established the exact third cause: this Kindle's
`/tmp` ancestor resolves to `/var/tmp`, violating the harness's exact-realpath
condition. The follow-up selfcheck used the observed canonical `/var/tmp`
runtime directly and passed all original initialization gates. It created no
formal document and ran no scanner, app, framebuffer, input or lighting action.
It cleaned its directories, verified native state, closed transports and
released its fresh lease. The production `/tmp/bilicomics-launcher.lock`
protocol is unchanged. No general symlink exception is introduced.

The fourth attempt led to another harness correction: clock evidence is created
in the canonical private runtime filesystem, then copied byte for byte with
hashes to the archive tree. The clock's file-safety checks and pre-exec sampling
origin are retained. The corrected research tools passed 24 targeted host
fixtures. A subsequent [small clock-record selfcheck](physical/native-library-entry-20261118/clock-selfcheck/README.md)
passed on the observed Kindle in about 4.6 seconds: both records had UID 0,
one link, 74 bytes and mode `0600`; private and archive bytes/hashes matched.
It ran only the unchanged clock with `/bin/true`, cleaned its own directories,
preserved native state, closed transports and released the fresh exact lease
at `2026-11-17T19:33:10.423858+00:00`. It dispatched no URI and performed no
application, scanner, formal-document, observer, input, display or lighting
operation. This is not application or timing-comparison qualification.

The fourth recovery's original combined proof exceeded 59 KB and its SSH exec
connection was reset, while short read-only commands worked. Splitting the
same checks into sixteen commands of less than 8 KB completed restoration.
No proof was omitted and no unknown process was signalled. The temporary
network/wake request during recovery became unnecessary; it is not evidence
that the device slept or that a user power cycle occurred.

The native extractor may recreate UUIDs and sidecar locators during a genuine
file opening. Those changes are recorded rather than rejected or used to drop
samples. Only the single canonical item, title/MIME and original integration
are stable acceptance requirements; unresolved cover bytes remain unknown.
The pinned [shared launcher](https://github.com/KindleModding/sh_integration/blob/adb7ef5c570a25c76fd64e94837ae0f590bd3b9c/launcher/main.c)
requests normal timestamp/scan updates, and the
[extractor](https://github.com/KindleModding/sh_integration/blob/adb7ef5c570a25c76fd64e94837ae0f590bd3b9c/extractor/main.c)
reindexes on `SCANNER_UPDATE`. This candidate does not change that policy.

## Pending manual device qualification

The completed timing comparison opens the real indexed path on both sides,
keeps only the document's `--library` argument different, and retains the
predetermined warmups and all six balanced AB/BA pairs. Its excluded native
stabilization period and two software/SEND endpoints do not supply a qualified
visible-cover or software-input-ready stamp.

Separate automatic qualification now covers visible framebuffer memory, actual
worker/entry lifetime, ordinary timer exit, cancellation and precise restoration.
Same-process reader screensaver cycles and human UI exits remain required.
Physical power-button and touch observations require explicit operator evidence.
Submission is not optical panel completion, and neither a timer nor the tiny
selfcheck supplies any of these missing passes. Prior temporary Scriptlet
timings are not an equivalent absolute baseline for this formal entry.

Two fresh independent single-case reader batches have been host-prepared and
reviewed but remain unexecuted, awaiting current operator availability. Each
needs its own device window and fresh lease. Their eventual stored audit must
require a strict before/suspend/resume/after time bracket, the same PID/birth
and actual reader anchor, matching generation across all three checkpoints,
and equal after-wake/idle visible framebuffer bytes, plus real operator power
and UI-exit observations. The original two-case analyzer does not accept these
single-case schedules; no such analyzer or lifecycle pass is claimed.
