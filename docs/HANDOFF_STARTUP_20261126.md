# Kindle startup handoff — 2026-11-26

**Six-pair comparison completed; production default approved.** The [closed physical result](verification/startup-cover-admission-20261126/README.md#completed-six-pair-campaign) is from a fresh run, not the paused checkpoint. On first-generation Scribe 5.19.5, the candidate won 5/6 pairs at renderer return and met the −100 ms upper-bound gate (paired median B−A: −131.655 to −128.908 ms). Accepted SEND and same-marker driver completion are reported separately. All 14 visits restored native control and released their exact leases. The controller survived the expiration of its read-only monitor, and there is no pending experiment lease or visit.

The shipping path admits cached cover decoding from the validated initial loading SEND without `--early-startup-covers` or timing diagnostics. First-turn fallback, completed metadata, UI yield, bounded cover work and display/input/power guards remain required. Do not resume the old plans or substitute the promoted binary into the historical controller. The checkpoint below records the earlier work, not an active task or the installed daily version.

## Historical checkpoint: what was saved

- The [sleep-cover launcher change](verification/startup-cover-overlap-20261126/README.md#six-measured-pairs) overlaps the optional native capability read with the existing full two-second observer settle. It is separate from cached-comic cover decoding. Its own earlier six-pair study measured a 237.703–240.456 ms paired median reduction to first Shelf render (six wins), with isolated demo data. The retained launcher also includes the fail-closed read-reaping guard, verified in software; do not treat that exception path as another physical A/B.
- The [ordinary offline phase study](verification/startup-ordinary-phases-20261126/README.md#physical-results) measured three populated Shelves and one initialized empty Shelf. Populated median: 5.256 s to renderer return, 5.341 s to accepted SEND and 5.704 s to same-marker driver completion. JPEG discovery and SQLite work already overlapped the first loading frame's checked completion wait. This motivated the [early cover admission study](verification/startup-cover-admission-20261126/README.md).
- In the frozen A/B, both arms used the same 74,983,432-byte ARMv7 executable, launcher, JPEG worker and ordinary populated offline seed. **Only B** supplied `--early-startup-covers`, which required the hidden `--startup-phases` diagnostic. A and that executable's no-option launch admitted cached-cover work at the original first UI turn. B admitted it after the first validated full-window loading SEND, subject to completed metadata and the same work bounds. The UI turn remained the fallback; no driver, framebuffer, input or sleep barrier was removed.
- The two arms passed isolated Scribe 5.20.1 firmware navigation and exit checks, including gate-source and same-marker replays. The relevant frozen build passed the Linux Rust app suite (918 library tests passed, seven ignored; 28 binary tests passed), Kindle backend (253), launcher (155), ordinary phase/mount/restore fixtures (16), and restarted A/B controller fixtures (17). All 14 physical archives were byte-compared off-device. These results do not qualify PW6 hardware or establish a physical speedup.

## Historical one-pair prefix

On the first-generation Scribe (5.19.5), the first measured AB pair, after two excluded warmups, recorded the following from the real native Library card release:

- Renderer return: A **5.067263–5.068734 s**, B **4.796573–4.798515 s**. Paired B−A: **−272.161 to −268.748 ms**.
- Accepted Shelf SEND: B−A **−263.343 to −259.931 ms**. Same-positive-marker driver completion: B−A **−265.401 to −261.988 ms**. Neither is optical timing.
- Gate source was `ui` in A and `send` in B. Cover decoding overlapped the first loading frame's checked completion wait by **263.129 ms** in B and **0 ms** in A. This is a per-run intersection, not an estimate of removable time.

This was **one** measured pair, not part of the complete six-pair campaign. A distinct earlier campaign stopped at visit 3 when native screensaver and SSH loss interrupted restoration. The original lease was safely recovered, its mount and test payloads were removed, and its unopened archives were discarded. The [interrupted evidence](verification/startup-cover-admission-20261126/interrupted-result.json) remains separate. The restart used the same transient native inactivity reset in both arms before the timed card press; it did not change persistent power settings.

## Decision and package deployment

The complete fresh campaign is the only evidence used for the promotion decision. Earlier partial series are historical. The `0.1.25` package trial from `2f3a50a` contained only the launcher change and was not installed. Do not use it as the final development package or retime either diagnostic arm as if it were the final default.

The promoted source passed 918 Rust library tests (seven ignored), 29 binary tests, 253 backend tests, 17 frozen-controller tests, and five update/rollback fixtures on `test-env`. The unchanged launcher suite had intermittent signal or process-exit fixture failures; the original `main` launcher and fixtures reproduced the SIGINT timing failure, and the three affected fixtures passed in a focused rerun. This is not a clean full launcher-suite result.

The [official byte-verified development package](../packaging/kindle/README.md#independent-installation-layout) checked all 50 ZIP members. On isolated Scribe 5.20.1 firmware, its packaged launcher, application and worker matched the selected bytes. Populated and initialized-empty synthetic Shelves opened with the ordinary application arguments, navigated through Store, Shelf and Account, and exited normally with same-boot native restoration. This did not measure a native Library card release, a live account, or optical display.

A live update is separate from those checks. Preserve the existing daily data, device profile/allowlist, native card and sidecar, and licenses. Prepare exact predecessor bytes and rollback before taking a new deployment lease. Recheck current ownership immediately before device access and return native control and the exact lease after cleanup.
