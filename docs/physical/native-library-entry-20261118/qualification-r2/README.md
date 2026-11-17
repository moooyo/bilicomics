# Stored automatic qualification, R7

Prepared on 2026-11-18 (Asia/Shanghai) from `lib-qualify-20261117-7ddd041f`.
All four planned cases completed and the complete frozen qualification analyzer
returned zero. The independent stored-evidence audit supports the recorded
visible-framebuffer, entry-retirement and entry-cancellation qualifications.
It found no new blocking identity, action, frame or restoration anomaly.
This bundle does not establish optical completion, manual reader sleep/wake,
input readiness or final product acceptance. Startup AB timings are not included.

| Case | Stored action | Actual entry status | Actual worker status | Bounded observation |
| --- | --- | ---: | ---: | --- |
| baseline-qualify | No injected entry signal | 0 | 0 | Entry gone at content; worker continues |
| candidate-qualify | No injected entry signal | 0 | 0 | Entry alive at content |
| candidate-retire | KILL to the bound entry | Unobserved / null | 0 | Worker survives entry retirement |
| candidate-cancel | TERM to the bound entry | 143 | 143 | Planned cancellation and cleanup |

The original `recovery.reason=normal-exit` values are preserved. That label does
not turn the signal scenarios into four natural zero-status or normal UI exits.
The killed entry's status is not inferred as 137. Entry-action worker identity
is bound to worker admission, separately from the Rust application identity.
The saved application process group/session match its worker, and the live
frozen observer checked ancestry through the production runner. Complete
intermediate runner `/proc` birth chains were not archived, so every ancestor
cannot be independently replayed offline; no direct application-parent claim
is made.

The two `*-qualify` captures contain identical **packed visible Gray8** payloads:
1860 x 2480, 4,612,800 bytes, zero changed bytes and maximum absolute difference
0. Both original raw files are 4,613,205 bytes with SHA-256
`299a4d97e6217aa25ea40ae96c84553ec3cc79c24e840418808495254a591104`.
Each includes a 405-byte metadata line with newline; visible payload SHA-256 is
`12ff9def5d77c72f473a4703635a192e9c79efaa71420605a5145bbc436dacc3`.
[framebuffer-comparison.json](framebuffer-comparison.json) preserves both original
metadata objects and their byte/hash identities, including `line_length=1872`
and the unchanged `rotate=3` enum. Row padding, offscreen virtual buffers and
the entire `smem_len` allocation were not the compared extent. These are stored
read-only framebuffer-memory captures of the Shelf fixture, not optical panel
images or logical Slint snapshots. Raw and PNG pixels are not published.

Twelve private clock originals (eight startup and four capture records) have
mode `0600`, UID 0 and byte/SHA-identical output mirrors. Public projections
retain only their integer monotonic/boottime fields and original byte identities,
without PIDs. Every recorded native-stable dwell is at least eight seconds.
All after-opening contentSize literals match document byte lengths. The
candidate-qualify before-opening catalog literal **85** versus document **95**
is retained, rather than normalized; these literals do not prove the extractor's
size unit or document-content equality from equal sizes.

R7 is the observer-only metadata retirement-race patchset, source-freeze SHA-256
`84c4c164500f485b3f7ac841397aa24da7d46bdb566774ca8b1e9f2f7bafec26`.
The original controller and product remain unchanged. The package source remains
`39fac822a982868078201e605e50bdacf1dcf3ed`, with reused runtime source
`530f1516c332931059c52161b52d64e86488b3f1`, selected ZIP SHA beginning `c57d5fc3`
and unchanged application/worker identities. [provenance.json](provenance.json)
separates the **stored 31 observer host fixtures** from the **stored 11 new
qualification binding checks**; neither was rerun for this export. The actual
admission records contain 55 command events with observed maximum 7814 bytes;
preparation limits are separate policy values. The previous
[failed qualification](../qualification-r1/README.md) remains unchanged, and
this fresh successful run does not establish its exact failure cause.

The actual current registration is preserved at canonical SHA-256
`cfc0381d6262afe7aff58534a832a6c15eea3903f07f35b831c69c7102acf804`.
Aidoku's four snapshots match within the selected six-file scope. Deployed
shared-wrapper source-to-binary mapping was not independently recaptured.
[restoration.json](restoration.json) retains the successful restoration and
exact owned-release projections. The recorded lease ran from
`2026-11-17T21:25:40.633000+00:00` to `2026-11-17T21:28:43.952785+00:00` (UTC),
then closed after native restoration and connection closure.

[summary.json](summary.json), four `cases/` projections and
[analyzer-result.json](analyzer-result.json) preserve the bounded results.
[independent-audit.json](independent-audit.json) projects the frozen independent
audit, whose original JSON SHA-256 is `cbb9cfdcd952f23f4fb0e17a6c0410920f6d673113a54bc828851d054bdd80f5`
and original Markdown SHA-256 is `aa03237354911917359602b83cf88f9ad1ce0838a0a22cfcda7ca1e95d6e780a`.
Its source origin manifest SHA-256 is `7094b4832cd006339d7b0b33335a9e17b44a2e588ef19250b330e05f245cb7de`.
[manifest.json](manifest.json) maps every exported payload to original byte
lengths/SHA-256 identities, fixed selectors and transformations. `SHA256SUMS`
covers every other public file including that manifest.

Owner, nonce and token values/digests, original process/native UUID identities,
addresses, account data, private transport content, native Home, raw archives,
ELF/ZIP contents and complete research tools/logs are omitted. All parsing,
hashing and comparison ran only on stored host evidence through `ssh test-env`;
no new tests, builds, application, device or lease actions were performed by this
export. Existing evidence and other exports remain unchanged. The manual reader
lifecycle remains outside this batch; see [the full status](../../../NATIVE_LIBRARY_ENTRY_20261118.md).
