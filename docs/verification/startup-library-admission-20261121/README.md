# Run ordinary Library admission once, in the worker

**Decision: keep the shipping Library parent and worker preflights.** Removing the parent's check was an isolated research candidate, not a demonstrated Kindle Shelf speedup. The [frozen candidate diff](worker-only.patch) and [six candidate-only regressions](test_candidate_launcher.py) remain reproducible without changing the packaging source or the installed launcher.

**November 22 follow-up:** a [new, independent Scribe series](../startup-library-admission-20261122/README.md) completed all six measured pairs. B was faster in three, but its paired median card release to first Shelf render was 64.139–66.997 ms later. The rollout threshold was not met; keep the installed launcher. The incomplete November 21 series below still has zero complete pairs and was not pooled with the new one.

The [previous six Scribe pairs](../startup-shelf-ab-20261121/README.md#where-to-look-next) put roughly 3.7 seconds between release of the installed Library card and the first app trace. This includes more than launch validation. The ordinary `--library` parent used to check the same application, device profile, firmware allowance, framebuffer access and output directories that its independently supervised `--detached` worker checked again. The parent synchronously waits for that worker's exit status. B removes only the parent's copy. The worker reads the current values before it can take the display lock; it does not trust an inherited approval. Background admission, cancellation, error reporting, orientation reads, cover handling and framebuffer barriers retain their behavior. The change does not introduce a model-specific optimization switch.

## Evidence

| Environment and endpoint | What happened |
| --- | --- |
| `test-env`, real shell Library entry to worker preflight complete | In the [two predeclared blocks](HOST_RESULTS.md), B won **6/6 in each**, with median paired reductions of **3.186 ms** and **2.838 ms**. Separate call counters showed the two complete preflights became one; refused configurations still failed before the lock. The full launcher lifecycle suite passed **158/158** on the final candidate. |
| `test-env`, real desktop app via the fixture Library entry to first matching Shelf pixels | Both arms passed the Scribe and PW6 window checks. In [six Scribe-size pairs](emulator/README.md), B was clearly earlier in two, later in two, and the intervals overlapped in two. The median paired B−A interval was **−11.201 to +3.256 ms**. A host shell dispatch is not a Kindle card release, and X11 does not provide an e-ink SEND. |
| Physical Scribe (first generation, 5.19.5), normal card release to first normal Shelf renderer return | **Zero complete timed pairs.** Both excluded warmups and the first measured A completed. Visit 4 (pair 1, B) stopped in native navigation with SSH status `255`, **before card identification, launcher activation or click**. There is no B endpoint for that pair and no paired median. The [audited result](physical-result.json) retains all missing endpoints separately. |

Both physical arms used the same retained 74,392,336-byte diagnostic ARM application, worker, isolated demo seed and default renderer. A used the original launcher; B used the exact audited worker-only change. Neither arm required all covers or a second click. The available measurements are shown below to distinguish a missing observation from slow startup. Values are seconds from the same device's native card release, rounded outwards.

| Visit | Role | Shelf rendered | Same-frame SEND accepted | Same-marker driver completion |
| --- | --- | ---: | ---: | ---: |
| 1, A | Excluded warmup | 6.27316–6.27439 | 6.30697–6.30820 | 6.67142–6.67266 |
| 2, B | Excluded warmup | 6.29324–6.29450 | 6.32717–6.32844 | 6.68978–6.69105 |
| 3, A | Pair 1, control only | 6.25777–6.25917 | 6.29006–6.29146 | 6.65525–6.65665 |
| 4, B | Pair 1, no card launch | — | — | — |

All four attempted visits restored native navigation, closed their connections and released the exact owned lease. The first three applications exited naturally. Visit 4 never switched the stock launcher; its staged files were removed. The transport receipt does not distinguish a network interruption, an authentication issue or another SSH error. It is not evidence of an app refusal. The ten unused visits were discarded without acquisition. No missing sample was replaced, analyzed as zero, or filled from the warmups. Minimal replay records are retained privately in `artifacts/verification/startup-library-admission-20261121/closed-prefix-receipts.tar.gz`; the [plan](PLAN.md) and [operator notes](NATIVE.md) document the scope.

## Next decision as of November 21

Keep the candidate for code review, not as a release with a claimed performance gain. If an end-to-end decision about this change is needed, first label the navigation observer's failing substep in the host diagnostic using fixed, nonsecret categories. Do not log raw SSH output, loosen native guards or retry an uncertain input. Then use a **fresh full series** with new identities on a stable test path; do not resume this prefix. For a larger performance target, separately instrument native dispatch and launcher handoff on the device clock. Neither the other Kindle projects' orientation timings nor their JPEG probe savings are BiliComics measurements.
