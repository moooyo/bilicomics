# Failed independent qualification: r1

This fixed stored-host export concerns only `lib-qualify-20261117-b130366f`,
prepared on 2026-11-18 (Asia/Shanghai). Original source timestamps remain UTC.
All export parsing and hashing ran on `ssh test-env`; no new device, lease,
probe, build, or test action was performed to create this bundle.

The original report remains **`success=false`, `completed_cases=[]`**. The first
`baseline-qualify` command returned 1 and stopped with `identity-unproven`.
The archived observer log records that reading the entry's
`/proc/<entry-pid>/cmdline` failed with `no such file`. Its stage path and PID are
omitted. This records metadata disappearance; the exact race cause remains
unproven pending a deterministic fixture. Native pixel, entry-retirement,
cancellation, and lifetime qualification did not complete. No successful A/B
result or accepted startup timing sample is included.

The archived recovery result also retains `passed=false` and
`reason=identity-unproven`. Separate cleanup fields report worker signaling
with status 143, entry status 0, owned processes gone, native restoration,
and production-lock absence. These cleanup results do not make the planned
retirement or cancellation experiment pass.

The final report records native, installation, catalog, lighting, registration,
input-roster, temporary-stage, same-boot, transport, and release checks as true.
The six protected Aidoku file records are byte-identical across all four
preservation snapshots. The exact lease window is
`2026-11-17T21:00:50.786927+00:00` through
`2026-11-17T21:02:15.250484+00:00` (UTC); acquisition and release receipts remain
separate evidence. This export does not alter the earlier failed timing
[checkpoint](../checkpoint/README.md) or the separate clock-record
[selfcheck](../clock-selfcheck/README.md).

Four numbered JSON projections preserve literal status fields separately from
interpretation. `manifest.json` binds each fixed source and named archive member
to its byte length, SHA-256, selector, and transformation. Archive contents were
read only by the six explicit member names; no archive is exported. Raw tokens,
token hashes, owner identities, process identities, transport details, device
addresses, protected paths, and content are omitted. `SHA256SUMS` covers the
other six files.
