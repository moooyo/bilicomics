# Formal Library entry candidate: 2026-11-18

## Status

The implementation and remote software checks are complete. **The candidate
has not completed formal-entry device qualification and is not adopted on
main.** The active candidate source is
`39fac822a982868078201e605e50bdacf1dcf3ed` on `codex/library-entry-lifetime`.
Main remains at the previously qualified `f8c24f8` runtime.

All four attempted timing batches stopped before dispatching an application;
there are zero startup samples. The later tiny device selfchecks pass only
observer initialization and private clock-record creation/copy. Neither proves a startup gain, working
Library lifetime, or same-process screensaver behavior for this candidate.
Further device work was deferred when the shared Kindle was reassigned to the
higher-priority project. BiliComics has restored and released its exact leases.

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

## Bounded device observations and harness corrections

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

The failed batches are retained separately; none supplies a latency sample:

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

## Evidence still required before adoption

The real indexed path must open through native file URI on both sides, with
only the document's `--library` argument different. Predetermined warmups and
six balanced AB/BA pairs must retain every observation. An eight-second native
stabilization period precedes, and is excluded from, each request. Endpoints
are BiliComics' existing software Ready and first accepted Shelf SEND; this
build has no qualified visible-cover or software-input-ready stamp.

Separate qualification must cover native pixels, actual worker/entry lifetime,
normal exit, precise restoration and same-process reader screensaver cycles.
Physical power-button and touch observations require explicit operator evidence.
Submission is not optical panel completion, and neither a timer nor the tiny
selfcheck supplies any of these missing passes. Prior temporary Scriptlet
timings are not an equivalent absolute baseline for this formal entry.
