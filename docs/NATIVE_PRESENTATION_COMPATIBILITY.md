# Native presentation compatibility

## Problem and resulting behavior

Version 0.1.7 preserves the system's native display-mode preference while
presenting logical application pixels with their original grayscale meaning.
The earlier framebuffer path wrote pixels unchanged; it did not compensate
when the native window manager reported `Y8INV`.

The launcher observes `com.lab126.winmgr epdcMode` before startup takeover and
each confirmed user resume, while the native window manager is running. It
accepts only `Y8` or `Y8INV`. Observation has a bounded private process group,
bounded output, and verified cleanup. It does not call a system-mode setter or
write a persistent theme configuration.

The application reads a private, context-bound startup record before acquiring
display or input. A resume record additionally matches the current lifecycle
sequence and is available only within its corresponding resume callback.
Unknown, stale, or unreadable records fail through the existing restoration
path. Files are read through a pinned private directory with no-follow and
identity checks.

The native backend compensates at the framebuffer write boundary. Initial
clears and full or dirty pixel writes use the same setting; 32-bit alpha and
row padding are unchanged. A setting change requires a complete scene rewrite
and full refresh. Resume configures compensation before input is reacquired.
The reader's logical pixels, image fit, contextual controls, and saved anchor
do not change because of this system-mode compensation.

## Ordered power events and bounded ownership

Native events pass through the application's hidden `--relay-power-events`
entry point before SignalBridge, data-directory, platform, UI, or runtime
initialization. It uses fixed 256-byte record buffers and nonblocking polling.
Each complete output frame is one atomic short write.

Native input and supervisor-ready control enter the same relay. A ready notice
is emitted only after readable native input, buffered records, and any pending
native output have been drained to a complete-record boundary. The first
pending or incomplete control starts a two-second deadline that includes output
backpressure; a newer sequence cannot extend it. Unknown, incomplete, oversized,
or malformed streams fail closed. Separate open-file descriptions prevent the
relay's nonblocking flags from changing the master's descriptors.

The native source, relay, and wrappers belong to one verified private process
group. Its anchor remains alive until cleanup. PID birth, parent, process group,
session, and launch context are checked before group termination. The physical
supervisor additionally retains independent process proofs for a master crash
and refuses to signal an identity it cannot prove.

## Evidence classes

Slint captures contain logical pixels before native output compensation.
Physical capture keeps raw visible framebuffer bytes and the original memory
PNG, with a separate EPDC-mode projection only when an explicit same-process
mode is available. The latest missing or unknown mode does not fall back to an
older trace or a preflight guess. A projection is not a panel photograph and
does not establish that the panel completed an update.

The [verification task record](../artifacts/verification/native-presentation-20261106/task-state.json)
tracks the frozen snapshots and pending stages. The first snapshot passed 445
library tests, 74 backend tests, and 42 host-tool tests. Its launcher run passed
37 of 38 cases; the full-FIFO failure remains a failed historical result. The
follow-up implementation fixes both record interleaving and ready-notice
ordering. Its [frozen controlled result](../artifacts/verification/native-presentation-20261106/r2-controlled/summary.json)
passes 450 library tests, 46 launcher cases, nine reader cases, two startup cases,
the real-process lifecycle case, 44 production-relay subprocess cases, and four
physical-helper fixtures. The Kindle binary test target compiles and starts but
executes zero tests; it is not counted as three native tests. The retained 74-test
backend execution maps to 19 identical backend files and identical Cargo inputs,
as recorded in the separate component mapping.

The candidate has 423 source inputs with manifest SHA-256
`2cf959672ea259dbd006d953d258cb95360f1c04e43edde47cdb6aec1e22ab27`.
Its sealed desktop SHA-256 is
`732453b9ed09e4cfe4815fd3a603764ac93aeff1aedc15fdff96ca561c6bb01e`.
The [ARM static checks](../artifacts/verification/native-presentation-20261106/arm-r2/package/package.json)
pass nine checks for 28 entries. ARM SHA-256 is
`9e93972007e46a3aeb7d23d82e6325b4e97ef6e1949848908a89e62eb4fbc16d`;
the [development archive](../artifacts/verification/native-presentation-20261106/bilicomics-kindle-0.1.7-dev.zip)
SHA-256 is `e8846d73e454d642a28e2a8c91ba77192543a59553db71f62c652fd7a741ad8d`.
The actual build diagnostics are retained separately from the empty packaging
wrapper log. Safe-export manifests identify portable subsets and any redacted
file identities; original report hashes are not substituted for exported hashes.

A fresh physical lease was acquired for one Scribe2022 on firmware 5.19.5.
Independent native preflight reported `Y8INV`, with 1860 by 2480 visible pixels,
eight bits per pixel, visual 1, grayscale 2, and no panning. The user observed
startup feedback and a normal bookshelf, then confirmed restored native dark
Home and touch after the supervised stop. The complete reader image had normal
colors and no persistent badges. Physical sleep/wake retained PID 19205, its
birth identity, and all 23 compared reader identity/anchor fields on page 18.
The first post-wake finger tap advanced to page 19 in that same process. The user
also observed contextual controls without image rescaling. Their final capture
was not acquired because the 600-second supervisor deadline had ended; this is
an operator observation, not an automated menu-image assertion.

Native baseline and launcher-lock checks passed, the user confirmed native dark
Home and touch, and a final read-only preflight still reported `Y8INV`. All
guarded device commands ended, and the exact shared lease was released at
2026-11-06T15:04:52Z. The public acquisition/release receipts retain only its
token digest. The first 30-second smoke was not visually observed; the requested
90-second repeat supplied the explicit startup/bookshelf observation.

The user observed native Home briefly before the retained comic returned in
this historical candidate, despite functional continuation. That failed visual
result is retained. The subsequent 0.1.8 kernel-observation path and inherited
SGID correction received a separate successful user observation of direct
same-page wake without native Home. The final 0.1.9 settings removal has its own
controlled and ARM evidence. See [Wake continuity and settings](WAKE_CONTINUITY_AND_SETTINGS.md)
for the later candidate identities, source mappings, and physical scope; those
results do not relabel this earlier run.

Earlier 0.1.4 physical acceptance and the completed
[0.1.6 resource work](RESOURCE_AND_PERFORMANCE_IMPLEMENTATION.md) retain their
original candidate identities and scopes.

The new read-only observation used the local WeRead tree as contextual source
material; its limits are recorded in [reference provenance](REFERENCE_PROVENANCE.md).
