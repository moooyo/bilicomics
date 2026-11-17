# Stored failed-attempt checkpoint

Prepared on 2026-11-18 (Asia/Shanghai) from fixed stored host files. Original
source timestamps remain UTC and run identities retain their original date.
This export performs no new device action. Candidate `39fac822` is not adopted
on main; the qualified main runtime remains `f8c24f8`.

All four timing batches stopped before URI dispatch or application start:
**zero startup samples**. The first failed during SQLite BUSY (5); the second
confused plain catalog MIME with the appreg `MT:` namespace. The third rejected
the observer's initial runtime context. The fourth passed that initialization,
then rejected an empty clock record with mode `0777` instead of required `0600`
(rc74). The stored C file is a safety-rule reference, not a new ARM build.

| Batch suffix | Original failed report: success / lease released | Separate later recovery: success / lease released |
| --- | --- | --- |
| `59ba43e0` | false / true | Automatic restoration recorded in the failed report |
| `a2c375d4` | false / true | Automatic restoration recorded in the failed report |
| `7a5ffc65` | false / false | true / true |
| `0c9ca0a3` | false / false | true / true |

The original failed reports remain unchanged. Their incomplete automatic
restoration fields are preserved beside independent recovery receipts. The
last exact release was `2026-11-17T18:58:46.056169+00:00` (UTC). The device was
then reassigned to a higher-priority project; BiliComics has no active lease.

The two tiny selfchecks concern observer initialization only: `/tmp` resolved
to `/var/tmp`; using the canonical runtime passed those gates. Both report no
URI and no application. None of these observations qualifies performance,
Library lifetime, reader cycles, input readiness, covers, or optical completion.

`summary.json` and eight numbered projections contain the bounded evidence.
`manifest.json` binds each original source's byte length and SHA-256 to explicit
selectors and transformations. UUIDs, owner/token values, process identities,
private transport details, command bodies, archives, and acquired content are
omitted. `SHA256SUMS` covers the other eleven files and is the bundle identity
input. See [the full status](../../../NATIVE_LIBRARY_ENTRY_20261118.md) for the
remaining qualification scope.
