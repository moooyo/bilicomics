# Rebuilt-worker native Library comparison

**Result:** On one first-generation Scribe (firmware 5.19.5), the rebuilt
candidate was faster in four of six pairs. The median time from releasing
the native Library card to driver confirmation was **6.355 s for A and
6.301 s for B**. The median within-pair change was **−171 ms**; individual
pairs ranged from **−667 ms to +511 ms**. This does not establish a reliable
one-second gain. No permanent install or performance promotion was made.

## Measured results

`lib-p-adc0890e` completed two warmups and six balanced pairs, all as new
processes, with no case replacement or cache eviction. A is the independent
`d81b529` control and B is the `57265de` candidate. The origin is the
device's monotonic clock immediately before sending the button-up event on
the unique full title in the selected native Library. The driver endpoint
is the completion of the **same positive marker** as the full Shelf SEND.
It is not optical settling or physical touchscreen latency.

| Endpoint from card release | A median (min–max), s | B median (min–max), s | Median paired B−A | B faster |
| --- | ---: | ---: | ---: | ---: |
| First accepted full Shelf SEND | 5.989 (5.808–6.590) | 5.935 (5.800–6.378) | −171.417 ms | 4/6 |
| Matching driver completion | 6.355 (6.175–6.955) | 6.301 (6.166–6.745) | −171.307 ms | 4/6 |

| Pair | Order | SEND B−A, ms | Driver B−A, ms |
| ---: | :---: | ---: | ---: |
| 1 | AB | −49.331 | −49.431 |
| 2 | BA | −293.504 | −293.183 |
| 3 | AB | +362.550 | +361.742 |
| 4 | BA | −668.076 | −666.779 |
| 5 | AB | +510.397 | +511.463 |
| 6 | BA | −532.980 | −532.908 |

The difference between the two marginal driver medians is about **−54 ms**,
not the median of the six paired differences. B was faster in all three BA
pairs but only one of the three AB pairs. This is a reason for caution, not
proof that launch order caused the difference. Warmups (excluded) took
6.458/6.824 s to SEND/completion for A and 6.021/6.388 s for B. Do not
subtract these results from the 7.503 s historical median.

All 14 launches preserved the initial loading scene and showed three or four
distinct loading-frame fingerprints before readiness. Each first complete
Shelf frame matched its process's final logical PNG; all 14 final RGB images
were identical. In the last pair, both live packed 1860×2480 Gray8 samples
also matched one another and the earlier qualified framebuffer. Neither arm
submitted a duplicate full GC16 frame in the Route-to-Views interval in the
six measured pairs. The trial therefore cannot isolate savings from removing
that duplicate or from the launcher overlap.

The [per-case and paired results](paired-result.json) include both endpoints,
loading frames, click brackets and driver markers. The
[restored native navigation](restored-navigation.json) matches the original
Home tab and pointer. The launcher lock was absent after every case, the
temporary entry/install and staging were removed, the two selected catalog
paths returned to zero rows, protected state matched and SSH closed before
the exact lease was released at **17:19:22 Asia/Shanghai**.

The allowlisted raw records (151 files, including all 14 offline demo captures,
not the user's native Library images) were exported and replayed on `test-env`.
Replaying the exported copy reproduced the published `paired-result.json`.
The archive is retained locally at
`artifacts/verification/library-pairs-20261120/lib-p-adc0890e-evidence.tar.gz`;
it contains no password, SSH configuration, private lease token or runnable
device payload.

## Method

This study compares the independently rebuilt control and candidate, not the
candidate against the November 19 distribution. Both use the same formal
Library document, offline demo Shelf, native card recognizer and timing
definition. See the [predeclared plan](../startup-optimization-20261119/PLAN.md)
and [software qualification](../../verification/startup-rebuild-20261120/README.md).

Before any timed launch, the actual ARM recognizer and input helper passed
[five synthetic Xvfb tests](target-fixtures.txt). A separate
[navigation-only Scribe trial](navigation.json) located both public tabs and
the selected Home underline without clicking. A [same-package rehearsal](same-package-rehearsal.json)
selected all 14 planned arms and refused to remove a replaced document in
an isolated host filesystem. None of these observations is a startup timing.

## Early attempts

- `lib-p-9e21c8c6` refused an occupied lease before any device contact. No
  samples or installation belong to this admission.
- `lib-p-b61b9d40` reached physical staging, then refused the selector's
  initial ownership check. No formal document was created, no application
  was started and no click was timed. The original Home tab and pointer,
  canonical absence and staging removal were checked. SSH closed and the
  exact lease was released at 16:47:50 Asia/Shanghai.
- A separate [filesystem-only probe](filesystem.json), `lib-p-276307dc`,
  found that `umask 077; mkdir` inherits the set-group-ID bit (`02700`) in
  the private parent. Explicit `chmod 700` gives `0700`. Owner markers,
  canonical paths and byte comparisons passed. The kernel's no-replace
  directory-rename probe on the media filesystem **failed**. No document,
  application or UI was involved; staging was removed and the lease was
  released at 16:52:24. The next protocol avoids renaming the canonical
  directory altogether; it does not weaken the no-overwrite check.

The [fixed-entry rehearsal](fixed-entry-rehearsal.json) verified that the
canonical entry's inode and bytes stayed unchanged across all 14 selections.
It dispatched the correct `--library` arm once per armed case, refused an
unarmed or repeated dispatch, preserved a foreign installation and refused to
remove a foreign replacement document. The shared document remains the
unmodified package member. Both arms include the same extra shell hop before
their normal production launcher, so their absolute times are instrumented
times rather than retail-launch measurements.
