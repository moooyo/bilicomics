# Route-refresh physical A/B evidence audit

## Decision

The stored observation chain satisfies the frozen revision 2 measurement
contract. It does **not** demonstrate a stable performance improvement from the
route-only optimization. The candidate medians are slower for both primary
endpoints, and candidate is faster in only three of six pairs for each endpoint.
This is a valid experiment with a negative performance result, not a basis for
adopting the route-only change.

The analyzer's `passed` status qualifies the evidence, pixel comparisons and
restoration chain. It does not declare the candidate a performance winner.
The retained paired summaries also have mixed directions; they must not be used
selectively to claim a benefit.

The audit concerns only batch `startup-route-20261117-196c8a73` at:

```text
/root/bilicomics-physical-startup-route-20261117-196c8a73
```

It read stored files on `test-env` and independently recomputed identities,
sample membership, endpoint arithmetic, capture windows and full visible-pixel
comparisons. It did not execute tests, builds, the analyzer, product/runtime,
device transport or lease utilities; query current device/lease state; read
private authentication inputs; or modify raw evidence or frozen tools. A
separate read-only reviewer audited recovery and exact release. This document is
the only audit file added to measurement revision 2.

## Evidence identity

| Stored artifact | SHA-256 |
| --- | --- |
| `report.json` | `dec81ee7cd2d7e9032735bbd07dfe9c900cc783754542c2602a54e615e933091` |
| `analysis.json` | `de6cd8fa9b7d2f4faa6e5ac876543cd8464f480abf917f6f8714a35cc86f9751` |
| `inputs/payload.tar.gz` | `aa84612539525c23a1ace41b069c4aead03d223b3a8d70f6cae1cd33354f1e5a` |
| `outputs.tar.gz` | `d3a46de96b5d4881f09007eb366aa8242c56b092dc795933607f5bec03ec068f` |
| `evidence/lease-released.json` | `372375d976bfeef21f768ef2ee9d005f52487e648cb498b9752551da837da693` |

The analysis names the exact report digest above. All 184 analysis input-file
digests match both the extracted files and their collected copies in
`outputs.tar.gz`.

## Same-ELF single-variable contract

Both variant records share the complete source, application, worker, original
launcher, ZIP, compiler and build identities. Actual selected files, ZIP
contents, payload copies and per-case deployed identities agree.

| Shared identity | Value |
| --- | --- |
| Source commit | `f6208d6f7dd3d8e64a09ef0ceb2ceb6b0c45d0f5` |
| Source-input fingerprint | `1ef8122dcafbbc0554af12c135a1a592c68bb5b4722f3f6b2109dceacae94e03` |
| Application ARM ELF | `3962bc49bb8cd33d21651845fe5de229e1b8458e5d0ba9026485c8bef7224525` |
| JPEG worker | `12c8988c6a93cade9923d54bef784b774e13cb5e3b042f2e72ea5ecb6b897beb` |
| Original launcher | `212527233f8bba1709ff0734ff217ccce67fc843cef06c617b9ededaeff0aef8` |
| Development ZIP | `ff41fdf74505020f50ab9928b1a7168ed773b82713b3330ce6b71a0b915b194c` |
| Compiler/profile fingerprint | `cb86aeb94e3cd05a435ecb31f97e0f928d4a5eff6afb98afcc89064814c8ce91` |

The source archive, compiler profile, ARM compilation proof and software-summary
files match the hashes embedded in `build_configuration`. All 545 selected
source-input hashes match the stored source tree and form an exact subset of the
archive manifest. All 1,243 regular archive files match that manifest with an
exact file set. The stored software summary declares the source commit above.
This audits the stored build provenance; it is not a new build or a claim that
all dependency artifacts were rebuilt during the audit.

The declared arguments are exactly:

- Baseline: `--demo --offline --legacy-startup-route-refresh`.
- Candidate: `--demo --offline`.

The baseline launcher has hash
`5bf1b12714efef2bebe2860b069e0b2f5c97d033f4068bfb9d2e06ad871e00d5`;
candidate has hash
`7a5e7cd0ebe1ab26d5581e91d96dadacea68c92305ab5d9068b7e0bff7df61f3`.
The complete legacy parameter name occurs once in baseline and is absent from
candidate. Removing exactly that injected space-plus-parameter sequence from
baseline produces candidate byte for byte. The complete serial preparation
parameter name is absent from both. Thus both arms retain item 1's default
overlap, and only the legacy route-refresh argument differs.

All batch host tools match frozen revision 2. Controller and analyzer hashes are
respectively
`03a470c1ce59086561cf0d6e192f731a97172efa1a734274aab6a1e908155d31`
and `210d5c80e038aff134db1832f3d108018932d9975e56fc3f39912d9f7ccede7f`.
The payload supervisor matches
`3fd01a0825c96774d0831fcb51c281c155eca2a6be88b2b8ee3e49c68959242c`.
Recovery, guard, lease and request-clock sources retain their frozen identities.
The ARM request clock and read-only framebuffer helper payloads match their pins.
There is no native-query helper, ABI probe or compatibility/diagnostic launch
extra in the ZIP or payload.

The closed anonymous fixture still has 37 manifested files, fingerprint
`a930027e7e297c89b61c963d2dd17d30c605bca3d4260e1805b7aea2257f3f79`.
Every manifested payload file matches; the only additional fixture payload files
are the expected `device-profile` and `device-allowlist`.

## Complete samples and independent endpoint calculation

Declared cases, completed cases, analysis rows and actual request order agree
exactly:

```text
baseline-warmup, candidate-warmup,
baseline-01, candidate-01,
candidate-02, baseline-02,
baseline-03, candidate-03,
candidate-04, baseline-04,
baseline-05, candidate-05,
candidate-06, baseline-06
```

Both warmups remain in the evidence and qualification checks. Only the warmups
are excluded from the six primary samples per variant. No observation is
dropped, replaced, reordered or promoted. The 14 application PIDs are distinct.
Every request follows the previous application's clean terminal trace; the
smallest gap is `6285.980385 ms`.

Each trace has one application identity, one process-entry record, one real
`ui_ready` and a complete, untruncated terminal record. Every case has an
accepted forced-full initial frame before readiness. Each selected business
frame is rendered and accepted, with a returned SEND and a Slint render starting
at or after readiness and finishing before submission. Final sidecars qualify
the same ready, nonsuspended demo/offline Shelf route and `Y8` mode; lifecycle
records contain no intervening suspend/resume transition.

Independent subtraction from the request `CLOCK_MONOTONIC` origin reproduces
all 14 readiness and first qualified Shelf SEND endpoints. Readiness uses the
actual `ui_ready` envelope timestamp; SEND uses
`refresh_submit.finished_monotonic_ns`, not trace-write time. Shell uptime,
bounded exit timers, driver completion and optical panel completion do not
replace these endpoints.

| Primary endpoint | Baseline median, ms | Candidate median, ms | Baseline minus candidate median, ms | Candidate faster pairs |
| --- | ---: | ---: | ---: | ---: |
| Software readiness | 5712.525615 | 5884.013962 | -171.488347 | 3 / 6 |
| First Shelf business SEND acceptance | 6509.286116 | 6521.791231 | -12.505115 | 3 / 6 |

The descriptive reductions from medians are `-3.002%` for readiness and
`-0.192%` for SEND. The median paired reductions are nevertheless positive:
`26.821077 ms` for readiness and `39.450538 ms` for SEND. The full paired
differences explain the mixed summaries; positive means candidate is faster:

| Pair | Order | Ready baseline minus candidate, ms | SEND baseline minus candidate, ms |
| --- | --- | ---: | ---: |
| 1 | AB | -85.681692 | -79.150615 |
| 2 | BA | 145.772924 | 164.229155 |
| 3 | AB | -371.432770 | -59.786539 |
| 4 | BA | -142.490001 | -178.566462 |
| 5 | AB | 139.323846 | 138.687615 |
| 6 | BA | 425.064308 | 435.689462 |

These are six warm-process pairs in one boot, without cache eviction. Neither
the positive paired medians nor selected pairs support a stable-improvement
claim. Conversely, this sample does not establish a universal regression across
other devices or conditions. It supplies no measured performance justification
for adopting this route-only follow-up.

## Pixel and resource observation boundaries

All 14 final logical PNGs decode to identical `1860 x 2480` RGB pixels, SHA-256
`cd103a87490f36b9a5298b142e349ef63c4be974dbf54e06988df20cf429330f`.
They are logical Slint snapshots at application exit before framebuffer
compensation, not optical observations.

For `baseline-06` and `candidate-06`, both early `content-frame.raw` and scheduled
`settled-frame.raw` have equal geometry and all `4,612,800` visible 8-bit pixels
were compared directly. Each comparison has zero changed pixels, with no masks,
excluded regions or derived projection used to establish equality. All four raw
visible-pixel payloads have SHA-256
`12ff9def5d77c72f473a4703635a192e9c79efaa71420605a5145bbc436dacc3`.
Framebuffer memory equality does not prove optical panel completion.

| Case and capture | Request after first SEND, ms | Complete after first SEND, ms | Bracket, ms |
| --- | ---: | ---: | ---: |
| baseline-06 early | 469.139538 | 1127.581692 | 658.442154 |
| candidate-06 early | 503.963846 | 1162.232769 | 658.268923 |
| baseline-06 scheduled | 1501.679154 | 2166.656000 | 664.976846 |
| candidate-06 scheduled | 1501.608461 | 2162.121538 | 660.513077 |

The same UI PID and positive birth identity are proven live around both capture
windows. Each early window ends before the scheduled window starts, and all
windows end before the application's terminal trace. The early observation is
not an exact capture of the first accepted frame. Scheduled pixels span a
software request-to-completion bracket, not an instantaneous sample at exactly
`+1500 ms`.

All 14 resource gates target first business SEND plus exactly `1500 ms`.
Recorded gate-request lateness ranges from `0.167847` to `3.252077 ms`.
Procfs sampling follows the gate and any scheduled capture, so the sixth pair's
resource reads follow capture completion at approximately `+2167` and `+2162 ms`.
RSS, sampled VmHWM and input descriptors are not exact-SEND measurements, uniform
exact-time samples or whole-lifetime peak measurements. Both primary endpoints
precede these observations. All cases retain the same selected input nodes,
`/dev/input/event2` and `/dev/input/event4`.

## Recovery and fresh exact release

All 14 recoveries record normal exit, launcher status zero, absence of owned
processes and production lock, and restored native control. There is no fallback
attempt, affirmative foreign-lock failure or proof-failure record. Each
case-command stdout is byte-identical to its collected recovery JSON. The
production traces retain native-release markers, consistent native identities
and statusbar-running observations that link consecutive launches.

Six cases enter the preserved terminal drain: `candidate-warmup`, `baseline-01`,
`candidate-01`, `candidate-02`, `candidate-03` and `baseline-06`. They record
verified direct-child exit and absent namespace within approximately `20-40 ms`
at coarse uptime resolution, below the two-second bound. Lock disappearance is
not substituted for true process exit. Recovery methods and terminal-drain logic
match the inherited safety source.

All 46 saved commands return zero without timeout/interruption; their stderr
streams are empty. Selected installation/data identities, boot ID and input
roster are byte-identical before and after the batch. The original lighting
values `12` and `8` pass the frozen per-case and final restore/readback checks.
These facts qualify the selected-file and native process/service scopes, not a
whole-tree census or identical native optical image.

Transport closure records no active owned transports and zero unreadable host
processes. Acquisition and release receipts match owner, run and token digest;
both this run identity and token digest differ from item 1, so that lease binding
was not reused. Release records owner/token checks, native restoration, closed
connections and absent lease directory after release at:

```text
2026-11-17T11:03:28.437714+00:00
```

The report finishes immediately afterward at
`2026-11-17T11:03:28.437825+00:00`, with all required success/restoration flags
true. This establishes the stored release event; the audit does not assert
current device or lease state.

## Separation from item 1 and other acceptance

Item 1 used adopted source `530f1516c332931059c52161b52d64e86488b3f1` and a
different application ELF. Its limited single-batch `3.456%` difference of SEND
medians remains documented independently in revision 1. This route-only result
does not replace that record, pool samples with it, or turn cross-batch timing
differences into a controlled comparison. Older native-query results are also
separate.

The route-only optimization is not supported for adoption by this performance
experiment. Keeping that negative result separate is compatible with retaining
the independently measured item 1 source. This audit performs no branch, merge,
push or release action.

Neither first business SEND nor a later matching capture proves that all covers
became visible at the endpoint. SEND acceptance is not driver completion or
optical panel completion. No touch, rotation, sleep/wake, model, cold-boot or live
account acceptance follows from this experiment or from automatic exit and
native restoration in a separate assisted run.
