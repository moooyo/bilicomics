# Overlap cached-cover preparation with the initial loading update

The [ordinary offline phase study](../startup-ordinary-phases-20261126/README.md#before-renderer-return) found that JPEG capability and ordinary metadata already overlapped the first loading frame's driver wait. The old cover gate could not use the rest of that wait because it opened on the first bootstrap UI turn. This comparison tested earlier admission of the same bounded cover work, without changing the cover count, postponing startup feedback, or removing a display barrier.

## Frozen treatment and qualification

- **A:** the cover gate opens on the first eligible bootstrap UI turn. Font and metadata workers still start early on a validated initial SEND when available.
- **B:** after that same accepted initial full-window SEND, the existing worker notice may open the cover gate. Metadata must still finish first. The original UI turn is the fallback. The first UI yield still precedes state consumption and attachment, irrespective of how soon the worker finishes.
- The backend continues to require a real initial render, a full accepted SEND with a positive current marker, and active ownership without pending sleep. The framebuffer write, completion, input and sleep barriers are unchanged. Earlier cover work is pure cached file read/decode with the same count and memory limits. No network request, account restoration, side effect or UI work is admitted by that notice. Close/quit/error still retire the one-shot notification and discard a canceled preparation.
- Both arms use the **same ARM binary**, retained launcher and populated ordinary offline fixture (20 fictional books with saved `launch=shelf` and initialized current-schema state). No demo, forced route, manual tap, capture of native collection contents or second touch after the card release is a measurement input. Only B passes `--early-startup-covers`, accepted only together with the existing isolated `--startup-phases` diagnostic. An independently verified accepted-loading stamp and the winning gate source distinguish a real early admission from fallback. The loading stamp is reused from the existing submit span; there is no new sampling or file write in the driver's critical section.
- The source, package, and controller must pass on `test-env` before a physical lease: Linux tests for duplicate SEND/fallback races, no early UI attachment, cancellation and closed-gate behavior; current ARM execution of A and B on private firmware, including the actual Shelf, navigation, natural exit and same-run phase/marker replays; byte comparisons of all planned archives and exact guards.

## Test-env qualification

The same 74,983,432-byte ARMv7 ELF and byte-identical shipping JPEG worker served both arms. With privately checkpointed populated fixtures, both [Scribe 5.20.1 firmware guest launches](firmware-qualification.json) showed the ordinary Shelf, visited Store and Account through native touch, returned to Shelf, exited normally, and restored their own native state. Each complete first-frame record matched its accepted SEND and the same marker's driver completion. The control reported `ui`, the candidate `send`; neither guest had positive cached-cover overlap with its first driver wait. This qualifies the mechanism and navigation, not a physical timing benefit.

The Linux application suite passed 918 library tests (seven ignored) and 28 binary tests, the Kindle backend passed 253 tests, and the launcher passed 155 tests. The 16 phase/mount/restore fixtures, 14 A/B controller fixtures, and five unchanged historical route replay fixtures passed independently. All 14 assembled physical archives and run-specific guards were checked against one set of exact inputs before the first physical visit, using byte comparisons rather than package hashes.

## Frozen schedule and decision

The study used one A/B warmup followed by six adjacent pairs in order **AB, BA, AB, BA, AB, BA**. Each visit used its own stage and exact owner/run/token lease; native recovery and release preceded analysis or the next visit. The protocol prohibited retrying a clicked visit, replacing an unavailable endpoint, or skipping a case, and allowed pauses only at a closed pair for explicit coordination. The hardware result applies to first-generation Scribe, firmware 5.19.5; PW6 software coverage is not PW6 hardware acceptance.

- Primary: card-release bracket to the software renderer return of the first ordinary, unobscured populated Shelf (cover placeholders allowed). Report its accepted SEND and same-positive-marker driver completion independently. No optical claim is made.
- Take the outward median of the **six paired** B−A brackets, not the difference of the arms' medians. To promote, require at least five clear primary wins and a paired median primary upper bound of **−100 ms or better**. All six measured pairs must independently show UI gate admission in A and initial SEND gate admission in B to support the mechanism. The controller also verifies the assigned gate source in both warmups. A B fallback remains assigned to B; do not discard it or reclassify it. Actual overlap between cover decoding and the measured loading-frame completion wait is attributed by intersecting the two same-boot spans, not inferred from different runs.
- Keep the ordinary path unchanged if the acceptance gate fails. A favorable software result, a faster loading-page draw or an earlier accepted SEND alone cannot qualify the optimization. Validate the promoted no-diagnostic official development package separately before installation; neither experiment arm is a daily installation.

## Completed six-pair campaign

The [sanitized result](completed-result.json) is a fresh, complete run on one first-generation Scribe (5.19.5), not a continuation or a pooling of earlier samples. The detached host terminal kept the controller running past the 30-minute read-only observer timeout. All 14 visits returned native control, removed their temporary device payloads and private mounts, and released their exact leases. Two warmups were excluded. The closed report replayed byte for byte.

| Endpoint from ordinary Library card release | Paired median B−A, outward (ms) | Clear B wins |
| --- | ---: | ---: |
| First normal Shelf renderer return | −131.655 to −128.908 | 5/6 |
| Accepted SEND for that Shelf | −150.111 to −147.543 | 5/6 |
| Driver completion of the same marker | −150.752 to −148.185 | 5/6 |

Every A visit admitted from `ui` and every B visit from `send`, including warmups. In measured B visits, cover decoding overlapped the first loading frame's checked completion wait by 170.313–248.225 ms (0 ms in A). This is an observed intersection, not an estimate of removable time. B was slower in the first pair by 167.272–169.959 ms at the primary endpoint. The series met the predeclared gate, but does not show a win on every launch, optical visibility, a live-account result, or PW6 hardware performance.

The approved production change removes the experiment-only switch and admits cached covers after the validated initial SEND without requiring diagnostic timing. The original UI turn still opens the gate if it wins, and the UI still yields before consuming prepared state. To reproduce the original A/B or replay its evidence, use the [frozen controller](ab.py) with the checkpointed diagnostic executable at `3fef9d6`, **not** a production build of the promoted default. This comparison did not install or retime the final development package.

## Earlier partial campaigns

The previous intentional pause ended after the first measured pair, at −272.161 to −268.748 ms to renderer return. A separate attempt lost its SSH-owned terminal at the observer deadline, closing 11 visits (including two warmups and only four complete pairs). That attempt returned all 11 exact leases and retired its three unopened payloads. None of these visits contribute to the completed campaign.

## Interrupted campaign and restart

The [first campaign](interrupted-result.json) qualified both warmups. Visit 3 then retired the app normally and restored the original launcher, but the native screensaver activated during navigation restoration and the Kindle became unreachable over SSH. After the operator woke the device, the original transaction restored navigation, removed the private mount and stage, and returned its original exact lease at 18:00:29 Asia/Shanghai on November 26. The interrupted visit is not a timing observation; its preceding warmups are not reused. The unopened payloads from that campaign were discarded.

The restart uses new run, archive, and lease identities throughout. The test controller refreshes the native inactivity timer using the application's existing `touchScreenSaverTimeout` command at bounded pre-card boundaries, after proving native ownership on the same boot. Both arms use the same commands. There is no background process, persistent timeout setting, or added power operation during a measurement or its retirement. Refusal stops the visit before card dispatch. Three additional Linux fixtures exercise these boundaries and reject lost native ownership, screen saver state, and an already requested click. The Scribe firmware guest accepted the exact transient property call when active, but its `activeApp` getter did not identify KPPMainApp; the combined physical admission therefore correctly refused that guest. The selected ARM app had already passed the separate full GUI check in that firmware. The same active-native timer call was previously used on the physical Scribe by the Aidoku startup operator; the real BiliComics controller does not relax its native checks or depend on a guest exception.

The [operator](ab.py) uses the existing first-card press bracket, private synthetic-session mount, exact stock-route proof, and restoration controller. No real session or device artifacts belong in this study directory.
