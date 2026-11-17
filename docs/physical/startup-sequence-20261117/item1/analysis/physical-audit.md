# Startup-sequence physical A/B evidence audit

## Decision and scope

No blocking deviation from the frozen revision 1 A/B qualification contract was
found in the stored evidence for `startup-sequence-20261117-f028b769`.
All 14 predefined launches are retained and qualified. The results support this
single-boot, warm-process, isolated demo/offline comparison of serial preparation
against the default startup sequence using the same application ELF and ZIP.

This audit read stored files on `test-env` only. It independently recomputed
identities, endpoint arithmetic, sample membership and pixel equality without
executing the analyzer, tests, builds, application, device transport or lease
utility. It did not query current device or lease state, read private transport
inputs, or modify raw evidence, frozen tools or runtime files. A separate
read-only reviewer checked the recovery and exact-release evidence chain.
This document is the only audit output added to the measurement directory.

The audited batch root is:

```text
/root/bilicomics-physical-startup-sequence-20261117-f028b769
```

Important evidence identities are:

| Stored artifact | SHA-256 |
| --- | --- |
| `report.json` | `683dbd5cbca7cc682846238f59d3e5a2830196d05cf2434949239916487e5256` |
| `analysis.json` | `fa74ed9b18b469e3ee5558cde6f40df599365c7a698727a81e5643c703d21261` |
| `inputs/payload.tar.gz` | `4af70d7506c72dab91df7a80fdfe0bd8ace2431c15dd9afbfe1722967c10ce97` |
| `outputs.tar.gz` | `9995d8ceebdc21ae418b23b3c25045dae41140b9f02549b97589a78182ec6040` |
| `evidence/lease-released.json` | `903e50c3ac7e3c0e0d5108ae0bbc578a86b5d444a20a22e85a73d2edfa5b638f` |

The analysis names the exact report digest above. All 184 analysis input-file
digests were independently matched to the extracted files and their collected
copies in `outputs.tar.gz`.

## Frozen comparison identity

Both variant records share all eight required identities, including the complete
nonempty build configuration. The actual selected binaries, ZIP entries and
payload copies match their declared hashes.

| Shared identity | Value |
| --- | --- |
| Source commit | `530f1516c332931059c52161b52d64e86488b3f1` |
| Source-input fingerprint | `8537123d6b1dc0c75a048b09d1204664988a6c1668ea35d721bfbac641ff49c6` |
| Application ELF | `04fc6f93b4327911ca1a3bb5200175c9f2e13fb21ac5e56280012ff44b6e2e59` |
| JPEG worker | `12c8988c6a93cade9923d54bef784b774e13cb5e3b042f2e72ea5ecb6b897beb` |
| Original launcher | `212527233f8bba1709ff0734ff217ccce67fc843cef06c617b9ededaeff0aef8` |
| Development ZIP | `1fe8ac38613d21904d187b41e973ff80302b0c78fe69b320e3d7143a31fd53ee` |
| Compiler/profile fingerprint | `8dae7219954e57236434f834b51c6dd74256ad7e14e5b68f88cf91864f31c955` |

The source archive, compiler profile, ARM compilation proof and software-summary
files match the hashes embedded in `build_configuration`. The 544 selected
source-input hashes match the stored source tree and are an exact subset of the
archive manifest. All 1,242 regular archive entries match that manifest, with an
exact file set. The stored software summary declares the source commit above.
These are provenance checks of the stored build, not a new build or a claim that
all dependency artifacts were rebuilt during this audit.

The instrumented launcher hashes are:

- Baseline: `59b8371d8124cae903f21b9be78ff31e614520cf08a9b83f6a6a6b061e0d1493`.
- Candidate: `7a5e7cd0ebe1ab26d5581e91d96dadacea68c92305ab5d9068b7e0bff7df61f3`.

Removing exactly one ` --serial-startup-preparation` occurrence from the stored
baseline launcher produces the candidate launcher byte for byte. The frozen
arguments are baseline `--demo --offline --serial-startup-preparation` and
candidate `--demo --offline`; route, tracing, capture and bounded-exit arguments
are common. Every case's deployed identity matches the shared ELF and its
variant's instrumented launcher.

The actual batch controller, recovery fragment, public guard and lease-utility
copies match `frozen-tools-r1.json`. The analyzer source also matches its frozen
hash. The payload supervisor matches
`04984a2c4301989dc068ce11f02b01b57470d0dd0ccd3d4c602ae62b0de0e845`.
The ZIP and payload contain no native-query helper; the payload has only the
baseline and candidate application variants, with no ABI, compatibility or
diagnostic launch extras. Ordinary trace-start/trace-stop diagnostic envelopes
are retained.

The canonical manifest retains the closed 37-file anonymous fixture fingerprint
`a930027e7e297c89b61c963d2dd17d30c605bca3d4260e1805b7aea2257f3f79`.
Every manifested fixture payload file matches. The only two additional fixture
payload files are the expected `device-profile` and `device-allowlist`.
The ARM request clock and read-only framebuffer helper match their frozen pins.

## Complete sample schedule and endpoints

The declared schedule, completed-case list, analysis rows and actual request
order agree exactly:

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
are excluded from the six primary samples per variant. No case is discarded,
replaced or promoted after observation. All requests occur after the preceding
application trace finishes; the smallest such gap is `6270.288616 ms`.

Each complete trace contains one process identity, one real `ui_ready`, one
process-entry record and a clean, untruncated terminal record. Every case has an
accepted, forced-full initial frame before readiness. Each selected business
frame is rendered, has a returned accepted SEND, and has a Slint render that
starts at or after readiness and finishes before submission. The 14 final
sidecars identify the same isolated demo/offline Shelf route and a ready,
nonsuspended session in `Y8`; their additional startup telemetry fields do not
change this qualification.

Independent subtraction of the stored request `CLOCK_MONOTONIC` timestamp from
the raw `ui_ready` envelope and first qualified
`refresh_submit.finished_monotonic_ns` reproduces every case's reported endpoint.
Shell uptime, trace-write time, software exit timers, driver completion and panel
completion are not substituted for those endpoints.

| Primary endpoint | Baseline median, ms | Candidate median, ms | Difference of medians, ms | Candidate faster pairs |
| --- | ---: | ---: | ---: | ---: |
| Software readiness | 5905.555154 | 5691.152808 | 214.402346 | 4 / 6 |
| First Shelf business SEND acceptance | 6696.776654 | 6465.361424 | 231.415230 | 5 / 6 |

The corresponding descriptive reductions are `3.631%` and `3.456%`.
The retained slower candidate observations prevent a uniform-improvement claim.
Six pairs in one boot do not establish statistical significance, cold-boot
behavior, cache-evicted behavior or a result for other fixtures or devices.
This audit does not repeat the separate startup critical-path analysis.

## Pixel and resource observation boundaries

All 14 logical final PNGs decode to identical `1860 x 2480` RGB pixels, SHA-256
`cd103a87490f36b9a5298b142e349ef63c4be974dbf54e06988df20cf429330f`.
They are Slint logical snapshots at application exit before framebuffer
compensation, not optical panel photographs.

For `baseline-06` and `candidate-06`, both `content-frame.raw` and
`settled-frame.raw` have equal headers and all `4,612,800` visible 8-bit pixels
were compared directly. Each comparison has zero changed pixels, with no masks,
excluded regions or derived projection used to establish equality. All four raw
visible-pixel payloads have SHA-256
`12ff9def5d77c72f473a4703635a192e9c79efaa71420605a5145bbc436dacc3`.
Raw framebuffer equality does not prove optical panel completion.

The stored clock brackets retain the observation delay and helper duration:

| Case and capture | Request after first SEND, ms | Complete after first SEND, ms | Bracket, ms |
| --- | ---: | ---: | ---: |
| baseline-06 early | 560.504769 | 1210.230308 | 649.725539 |
| candidate-06 early | 436.286462 | 1103.622077 | 667.335615 |
| baseline-06 scheduled | 1500.400000 | 2183.282923 | 682.882923 |
| candidate-06 scheduled | 1501.645385 | 2210.868308 | 709.222923 |

The same UI PID and positive birth identity are proven live around both capture
windows. Each early window finishes before its scheduled window begins, and all
windows finish before that application's terminal trace record. The early
capture is therefore an early post-SEND observation, not an exact capture of the
first accepted business frame. The scheduled capture starts near the requested
target but spans a software bracket; its pixels are not an instantaneous
measurement at exactly `+1500 ms`.

All 14 resource gates target the first business SEND plus exactly `1500 ms`.
Their recorded request lateness ranges from `0.335461` to `1.700846 ms`.
Resource sampling occurs after the gate and after any scheduled framebuffer
capture. Consequently the sixth pair's resource reads follow the capture
completion at approximately `+2183` and `+2211 ms`, respectively. RSS, sampled
VmHWM and input descriptors must not be described as exact-SEND samples,
uniform exact-time samples or whole-lifetime peak measurements. Both primary
endpoints precede these captures and samples. All cases retain the same selected
input nodes, `/dev/input/event2` and `/dev/input/event4`.

## Recovery and exact release

All 14 saved recoveries report normal exit, launcher status zero, native
restoration, absence of owned processes and absence of the production lock.
There is no fallback attempt or affirmative foreign-lock failure. Production
traces retain native-release records and consistent native process identities.
Each case-command stdout is byte-identical to its collected recovery JSON.
All 46 recorded commands finish with exit status zero, without timeout or
interruption; their saved stderr streams are empty.

`candidate-warmup`, `candidate-06` and `baseline-06` actually enter the preserved
revision 2 terminal drain. Their lock observations record verified direct-child
exit and absent namespace within approximately `30 ms`, with both
`lock_tail_pending` and `lock_tail_drained` set. The other 11 cases do not require
that drain. A vanished lock anchor is not used as a substitute for process exit.

The selected daily installation/data identity list, boot ID and input roster
have byte-identical before/after records. This is the selected-file preservation
scope, not a whole-tree census. The original lighting values are `12` and `8`;
the frozen per-case and final restore/readback checks complete successfully.
Native process/service checks support restored native control; they do not prove
that the before/after native framebuffer or optical image is identical.

The transport-closure receipt records no active owned transports, zero unreadable
host processes and `connections_closed=true`. Acquire and release receipts have
the same owner, run identity and token digest. Release records exact owner/token
checks, restored native control and closed connections, with the lease directory
absent after release. The recorded release time is:

```text
2026-11-17T10:18:37.889218+00:00
```

The batch report finishes immediately afterward at
`2026-11-17T10:18:37.889312+00:00` and has all seven required restoration/success
flags true. This establishes the stored release event; the audit does not assert
the current lease state.

## Claims excluded from this qualification

The first business SEND does not establish that every background cover has
completed or become visible. SEND acceptance is not driver completion or optical
panel completion. No touch, rotation, sleep/wake, model qualification, live
account or cold-start acceptance is added by this batch.

The separately identified assisted run ending in `a1067e98` is outside this A/B
evidence set. Automatic bounded exit and restored native control cannot replace
missing supervised key/touch/sleep feedback or establish those checks as passed.
Older native-query physical measurements are separate experiments and are not
pooled with, substituted for, or used to qualify this startup-sequence result.
