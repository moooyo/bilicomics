# Bound lock retirement observation

This revision changes only the automatic device test controller. The candidate
application, helper, worker and product launcher remain unchanged. The original
120-test tooling and the failed `105808c7` run are preserved.

The failed candidate case recorded `native_released` at device uptime 197302.35
and entered supervisor cleanup at 197302.45. Its launcher later exited zero,
native control and all recorded processes were restored, and the exact lease
was released. The stock launcher's cleanup releases display ownership, stops
logging, removes `pid`, other control leaves, `nonce` and `generation`, then
removes the directory. The old observer treated some absent/unreadable anchors
or namespace stat races as an ownership mismatch. That timing is consistent
with an ordinary unlink window.

The failed run did not record the particular failed predicate or value. Final
health cannot prove that a foreign mismatch never occurred. Its failed result
is retained and is not retrospectively accepted.

The revised observer separates three outcomes after binding:

- Coherent current identity and exact expected anchor bytes.
- Affirmative foreign inode, type, link or readable bytes: a sticky hard failure.
- Missing or unreadable namespace/anchor: indeterminate, never evidence of exit.

An indeterminate bound observation permits only a two-second terminal drain.
Ordinary sampling does not resume during that drain, even if an anchor appears
again. Acceptance requires the original recorded direct launcher to be verified
exited, the namespace to be absent under both `-e` and `-L`, and the existing
native/process recovery proofs to pass together. An active or unknown task is
not waited on as if it had exited. No signal target, escalation or recovery
authority is added. An affirmative mismatch cannot be erased by later healthy
teardown.

`lock-observation.tsv` now records fixed reason labels without nonce contents.
The three exact expected anchors are retained only in private run output.
`cmp` availability is checked read-only before deployment or application startup.
The ordinary two-second native capture deadline and primary timing endpoints
are unchanged. A future complete A/B run must use a fresh identity and this
revision consistently for both variants.

Host fixtures use real `/proc` identities and cover slow stock-like unlinking,
an active launcher with missing anchors, a foreign readable PID during the
drain, a replaced directory containing matching anchor bytes, and a hard
foreign observation followed by otherwise healthy teardown. These fixtures
are controller checks, not new Kindle measurements.
