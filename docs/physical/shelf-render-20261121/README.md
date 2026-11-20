# First Shelf frame: private render before the mapping barrier

**Result:** Keep the candidate. On the first-generation Scribe (5.19.5), it
reached the complete offline Shelf sooner in all six paired native Library
launches. The median paired improvement to matching driver completion was
**585 ms**. The optimization does not remove any panel wait: it uses the time
before the write barrier to render the next scene in private memory.

## Device results

`lib-p-b4a63eed` completed both warmups and all six alternating pairs. A is
`57265de`; B is the `shelf-private-rgb-v1` working-tree build on top of
`ab52019`. Times start just before releasing a 50 ms XTest click on the
positively identified native Library card. Every process used a separate copy
of the same closed synthetic seed. There was no URI dispatch, real account,
cache eviction or sample replacement.

| Endpoint from card release | A median (range), s | B median (range), s | Median paired B−A | B faster |
| --- | ---: | ---: | ---: | ---: |
| First accepted complete Shelf SEND | 6.966 (5.942–7.385) | 6.436 (5.446–6.899) | −584 ms | 6/6 |
| Driver confirmed the same marker | 7.333 (6.307–7.751) | 6.802 (5.812–7.265) | −585 ms | 6/6 |

| Pair | Order | SEND B−A, ms | Driver B−A, ms | Ready→SEND B−A, ms |
| ---: | :---: | ---: | ---: | ---: |
| 1 | AB | −397 | −397 | −395 |
| 2 | BA | −1,356 | −1,356 | −344 |
| 3 | AB | −805 | −821 | −187 |
| 4 | BA | −682 | −684 | −387 |
| 5 | AB | −270 | −271 | **+102** |
| 6 | BA | −487 | −486 | −521 |

All six measured B frames rendered the private RGB buffer before their actual
previous-marker wait; A always waited first. Median residual wait fell from
**319.551 ms to 0.025 ms**; ready-to-SEND medians were **825.135 ms and
588.555 ms** (paired median −365.366 ms). Pair 5's slower B render outweighed
the shorter wait at this stage. The following SEND-to-driver interval stayed
about 366 ms. Pre-Rust time, loading-frame counts and rendering costs also
varied. Do not attribute the entire 585 ms paired change to the barrier move,
add it to November 20's result, or interpret it as a guaranteed one-second or
finger-to-glass improvement. The two marginal driver medians differ by 531 ms;
this is not the median of the six paired differences.

All 14 launches preserved the initial loading scene and exhibited three to
five distinct activity frames. Their first complete Shelf matched the final
logical RGB pixels; the two final-pair visible Gray8 samples also matched the
qualified reference. The controller restored the original Home tab and pointer,
the native foreground/window manager, lighting and protected peer files/index
before removing its own stage and releasing the exact lease at **04:04:50
Asia/Shanghai**. It notified the waiting Aidoku session. This comparison itself
left no BiliComics installation or Library row behind.

The [paired endpoints](paired-result.json), [per-frame and order-group
breakdown](frames.json) and [decision checks and launcher spans](decision.json)
are generated from the closed records. Replaying all 165 allowlisted files on
`test-env` reproduced all three reports byte-for-byte. The raw archive is in
`artifacts/verification/shelf-render-20261121/lib-p-b4a63eed-evidence.tar.gz`;
it excludes the transport, password, private token, runnable payload, and native
Library screenshots.

## Software checks

- The [ARM build](build-receipt.json) used the shared toolchain, two Cargo jobs,
  optimization level 3, ThinLTO, one codegen unit and stripped symbols. The
  [static package comparison](package-static.json) found only the main ARM
  application different; the other 48 members, including worker and launcher,
  retained their bytes and metadata. No new package checksum was required.
- [Host checks](software.json): 898 library, 18 application-binary and 249
  backend tests passed, including six new targeted cases; four existing tests
  remained ignored. An initial two-job test compile ran out of memory; the
  one-job retry passed without a source change. PW6 and Scribe desktop startup
  feedback and Shelf captures passed; both final Shelves had zero differing
  RGB pixels from the frozen references. The QEMU check was CLI-only.
- The [same-package rehearsal](rehearsal.json) verified the fixed canonical
  entry and one-shot selection across 14 cases as well as refusal to proceed
  on changed WeRead files or index rows. These host and desktop checks are not
  part of the physical timings.

## Why this frame

An [offline replay](november20-frames.json) of the November 20 raw device
records found a repeatable cost **after** software readiness. Across the 12
measured launches, the first complete Shelf spent a median **332.306 ms**
waiting for the previous panel update (only **0.063 ms** of thread CPU), then
**345.642 ms** rendering into private RGB memory (**236.482 ms** of CPU).
Median ready-to-SEND was **759.301 ms**. Taking the shorter of the wait and
the render within each frame gives a median overlap ceiling of **329.893 ms**,
not an expected speedup. The click-to-Rust interval and a later driver wait
are separate; they are not saved by changing this transaction order.

A separate observation explains some pre-ready variation. In four launches
(`b03`, `a04`, `b05`, `a06`), the prepared-state timestamp fell 1.710–11.574 ms
**after** the third loading frame began waiting. The UI recorded that result
254.561–275.955 ms after the producer timestamp. These cases had four distinct
loading frames; their first-loading-SEND to ready median was 1,218.756 ms,
versus 792.932 ms for the other eight. This is not a reason to suppress a
changing loading frame or a causal estimate of what doing so would save.

## Comparison

The [predeclared plan](PLAN.md) defines the application pair, one-shot Shelf
eligibility, unchanged mapped-write and checked-completion barriers, both card
endpoints, and the decision rule. The November 20 control here is the **newer**
application tested on that date. Comparing to the older historical median
instead would mix the render change with a different launcher and application.

## Normal entry

**Installed and smoke-tested on the same Scribe.** The
[ordinary-entry receipt](rollout-result.json) records one actual click on
the indexed `BiliComics for Kindle` card in `bili-prod-f3690b72`. It ran the
measured candidate and byte-identical shipping `launch.sh`, without demo,
offline, trace, exit-timer or wrapper arguments. The ordinary application's
exact argv, executable identity and ancestry, generation-bound lifecycle-ready
record, and live framebuffer and evdev descriptors were verified. This is not
a login, a visible-content assertion, a retail-launch latency measurement, or
an assertion of no network requests. There was no login or purchase action.

The first observer rejected the production lock because `/tmp` is a link to
`/var/tmp` on this device. No second click or lease was used: the operator
revalidated the still-running original process. The narrowly scoped check
requires the root-owned, non-symlink lock leaf at its exact resolved path,
matching device/inode and private mode (`0700` or inherited `02700`), as well
as the original process/nonce/argv checks. After a **managed** exit the
original Home tab, pointer, native foreground, lighting and peer state were
restored, temporary staging removed, normal installation independently read
back, all connections closed, and the original lease released at **04:47:08
Asia/Shanghai**. The stricter permanent controller change was exercised with
both direct and aliased `/tmp` in the host fixture; the same leased recovery
was the device observation. Aidoku was notified immediately after release.

The retained installation has one actual document `/mnt/us/documents/bilicomics.sh`
and one correctly titled catalog row. Its launcher is `/mnt/us/bilicomics/launch.sh`;
application and worker are under `/mnt/us/bilicomics/app/`, data and the exact
`scribe 5.19.5` runtime allowance under `/mnt/us/bilicomics/data/`, and package
notices inside the BiliComics root. No demo seed or diagnostic helper was left
installed, no shared handler or peer application was changed, and the repository
example allowlist was not opened up. The current public protection record is
`test-env:/var/tmp/kindle-build-coordination/bilicomics-production-20261121.txt`.

### Earlier deployment attempts

The paired run was a temporary offline comparison. In a separate deployment
attempt (`bili-prod-ac86ba02`), the installer refused to proceed **before any
card click**. The media volume enumerated only `bilicomics.sh`, but lookup by
`BiliComics.sh` also succeeded. The two spellings returned different inode
numbers on the same device, each with link count one. The index contained only
the exact lowercase row. The installer did not treat the second lookup as
permission to remove a second file. Recovery rechecked the package bytes,
the actual directory spelling, both lookups, the creation journal, and peer
state; it removed only journal-owned objects. Both document lookups then
returned absent, the row count returned to zero, and original navigation was
restored. The exact lease was released at **04:26:52 Asia/Shanghai**. No
application launch, performance sample, or retained installation came from
this attempt.

A second admission (`bili-prod-14fd0768`) also stopped before the click. In
its independent, run-owned same-volume test directory, the lowercase and
alternate lookups observed two successive values written only through the
lowercase spelling. The uppercase `stat` remained positive immediately after
unlink even though the empty probe directory could be removed. An immediate
negative lookup was therefore not a usable test of directory contents. Again
the operator recovered **only** the journal-owned installation, confirmed both
lookups absent, zero rows, protected state and native navigation, and released
the same lease at **04:36:34 Asia/Shanghai**. Neither failed admission is
included in timing or normal-entry success counts.

The corrected check runs its same-volume experiment before creating any
formal files. It verifies two writes, a single real directory entry, exclusive
removal of the owned sample and its empty directory, and bounded time for both
lookups to disappear. File bytes, on-device volume/boot, actual directory
spelling, single-link types, and the sole selected catalog row are still
checked independently on the production document. A later prepared admission
(`bili-prod-90a80124`) refused an occupied lease and never contacted the device.

## Replay

On `test-env`, run `python3 -B project_frames.py BATCH PAIRED_RESULT_JSON`
against a closed batch and its qualified native-card report. The timing reader
links the preceding and current positive SEND markers, the first complete
Shelf and the loading frames to the same process, and checks that no real
framebuffer write or submission preceded the required barrier. Both transaction
orders are supported without reordering timestamps or changing the endpoint.
`python3 -B test_project_frames.py BATCH PAIRED_RESULT_JSON` exercises this
reader using the closed input plus deliberate invalid cases.

New cases also retain a narrow startup-launcher projection, not the full
device log. `python3 -B project_launcher_stages.py BATCH/CASE` uses only the
worker's centisecond `/proc/uptime` records. Its intervals begin at bounded
log readiness and end at display takeover; never subtract them directly from
the click or Rust `CLOCK_MONOTONIC` timestamps. The November 20 archive does
not include this file and was not retrofitted with synthetic observations.
