# Overlap the launcher’s sleep-cover probe with the existing settle

## Question

The stock launcher waits for the full power-observer settle, captures live
power, EPDC mode and portrait metadata, publishes the owned startup records,
and then queries the loaded `blanket` modules. That last read only determines
whether the launcher may request a custom sleep cover. It is not a condition
for display takeover or for rendering Shelf.

The candidate issues the same read after logging and observer ownership are
established, while the two-second observer timer is already running. The
supervisor still owns and reaps a single bounded native reader at a time;
there is no extra query, worker, or wait. Unknown, malformed, advertisement
and failed queries still select the native screensaver. The live display
queries, their repeated checks, the portrait record, relay-ordered takeover,
and framebuffer and input barriers are unchanged. This is a startup scheduling
candidate, not a new sleep-cover capability or a deployment.

### Prior evidence used to select the change

These are different studies, not measurements of the current pair or values
to add together:

| Observation on Scribe 5.19.5 | Previous measurement |
| --- | ---: |
| Ordinary card release to instrumented launcher shim | 437–438 ms |
| Shim to application invocation, including the protected observer settle | 3,220 ms |
| Invocation to Rust `main`, and `main` to trace header | 19 ms and 17 ms |
| App trace header to first complete Shelf render | 2,658 ms |
| Standalone native display helper, including five firmware getter calls | 301 ms wall, of which 214 ms in `poll` |
| Launcher cover query, grant to observed result and subsequent reap | 85 ms and 10 ms |

The first four lines are from the [stock A/A](../startup-clock-aa-20261122/README.md),
the fifth from the [display-helper study](../startup-display-sampling-20261122/README.md),
and the sixth from the [launcher replay](../startup-decisions-20261121/launcher-replay.md).
The power settle is not available time to cut. The November design handoff no
longer displays the transient `attach` text, so the older label experiment is
not a valid treatment on this revision. Native JPEG qualification and cover
preparation are separate possible dependencies on the app side; neither was
changed by this comparison.

## Fixed physical plan

- Use the existing native Library card on the same first-generation Scribe
  (5.19.5); never call the app or the launcher directly for the timed run.
- Both arms use the **same 74,940,168-byte** ARM release from `d293782`, the
  same JPEG worker, an isolated copy of the same offline demo preference and
  assets, and the same instrumentation. The only A/B factor is the position
  of the sleep-cover query. The shipping card, binaries, data, and launcher
  are not updated by this study. A bounded, reversible route temporarily
  selects the isolated launcher and restores the original inode afterward.
- Warm up once per arm (A, B), then run six adjacent pairs in order **AB, BA,
  AB, BA, AB, BA**. Interruptions between complete pairs for device handoffs do
  not reuse any earlier lease, case, or sample. Do not retry a card release,
  replace a failed visit, or skip a slot within a series. A series that fails
  admission before any card release contributes no timed cases.
- A starts the cover query after portrait publication; B must finish its
  successful query before the observer-settle receipt and before display
  sampling. Launcher subspans use the supervisor’s 10 ms `/proc/uptime`
  trace and are not subtracted from the monotonic render clock. App/helper
  phase marks and the card bracket use device `CLOCK_MONOTONIC`.
- The primary interval is from ordinary card release to completion of the
  first unobscured, populated Shelf draw. Loading does not qualify; covers
  are not a condition. Accepted SEND and completion for that same positive
  marker are independent secondary intervals. There is no optical, live-account,
  PW6 hardware, page-turn, or touch-response measurement.
- The existing [timing gate](../startup-shelf-ab-20261121/PLAN.md#paired-analysis)
  calls for all six pairs, at least five clear primary wins, and an upper
  bound of at most −100 ms for the median paired B−A. Report the independently
  witnessed phase ordering and absolute and paired timings even if the gate
  is not met. Never substitute a later frame or another arm for a missing
  endpoint.

## Software qualification

- A one-job offline Linux ARM release built on `test-env`. The fresh app
  and worker matched the selected release bytes with `cmp` (no code or package
  digest). The clean retry removed macOS transfer-generated AppleDouble
  entries that had been mistaken for localization inputs.
- All **154** launcher fixtures passed on `test-env`. The changed ordering
  fixture holds the settle timer while the cover read retires and shows that
  startup display sampling and takeover have not started; it then still
  requires the full two seconds. A separate timeout fixture reaches startup
  and uses stock sleep. Existing suspend, cover, USB, and recovery cases
  remain in the suite. On the first suite run, the log-quota fixture wrote
  its artificial burst after the newly earlier reader emitted shell cleanup
  output. The fixture was corrected to fill the freshly opened log before
  any native read; log quotas and production handling did not change.
- The Scribe 5.20.1 firmware guest separately used the candidate launcher:
  the cover reader succeeded and retired before native display sampling, the
  actual Shelf rendered, Reader and directory navigation worked, and a normal
  exit restored the guest on the same boot. This test did not simulate sleep
  or time the ordinary card. The Shelf capture had normal entries and navigation
  with transient guest boot text over its hero region; it is not an optical or
  pixel-equivalence result.
- The temporary route, native controller, Shelf trace audit, and host clock
  fixtures passed separately. The route and clock-mark helpers were built for
  ARM, and both prepacked warmup launchers and their shell guards passed shell
  syntax checks and the exact archive member checks. Those are off-device only.

## Physical status

The initial attempt stopped at the installed-route snapshot guard **before
clicking the card or replacing the launcher**. Both temporary staging trees
were removed, native control was confirmed, and the original exact lease was
released. An SSH outage of `test-env` followed. No render, SEND or completion
observation can be recovered from this attempt; no original files were changed.
That incomplete attempt is separate from the six completed pairs below.
The [frozen measured candidate](candidate.patch) and [comparison runner](compare.py)
are retained so the paired result does not depend on later product changes.

## Updated stock admission

After `test-env` access returned, two separately leased, read-only visits
identified the mismatch. The current clean installation has the 158-byte
icon-bearing document and a native `bilicomics.sh.sdr` metadata directory.
There is only one lowercase executable card and one exact index row. The
[anonymous inventory](stock-audit.json) also confirms the known launcher,
application and worker identities, profile and firmware, with native control
and lease release verified. The [probe](stock_probe.py) did not open the app
or read metadata contents. The route now uses the exact icon document and
recognizes only its one canonical metadata directory without modifying it.
[Linux fixtures](test_compare.py) verify that preservation and continued
refusal of duplicate cards, wrong bytes and untrusted metadata paths.

The new route passed stock inspection, but its first warmup stopped before
activation: the historical full-title OCR method could not uniquely locate
the card. The visit restored native navigation and released its lease; it
contributes no timing. The [artwork detector](icon_match.c) is generated from
the project's public cover, does not scan book text, and retains the original
active-tab and device-monotonic XTest release checks. Its builder takes
inspiration from WeRead `d4e2a80`'s public-tile recorder and reuses this
repository's original X11 input boundaries. No other application's artwork,
user library screenshot, private reference or saved input point is used.
On `test-env`, the rebuilt ARM recognizer and input helper passed all three
[QEMU/Xvfb fixtures](test_icon.py): the unique scaled cover yields the same
50 ms release receipt; duplicates, partial artwork, an empty view and
Home-only artwork refuse input. Of the 69 diagnostic kit members, only the
two executables differ from the historical kit, verified byte for byte.
The [separate native locator visit](icon_probe.py) found exactly one match on
each of two successive Library reads ([anonymous receipt](icon-qualification.json)).
It neither activated a temporary route
nor opened the card. The original tab and pointer were restored, the detector
stage was removed, and the exact device lease was released; this is not a
warmup or a timing sample.

## Six measured pairs

On one first-generation Scribe (5.19.5), the two warmups and all six measured
pairs qualified. Each used the ordinary indexed native Library card, isolated
offline demo data, a separate app process, and the same installed-revision ARM
binary in both arms. Every run naturally retired, restored the original
launcher, tab and pointer, removed its temporary payloads and private token,
and released its own exact lease. The final release receipt was recorded at
15:25:58 Asia/Shanghai on November 26. The 31-minute gap after pair 1 was an
*off-lease* handoff between pairs, not part of any launch. No installation was
permanently changed. The [complete public projection](physical-result.json)
includes both excluded warmups and every measured endpoint.

The first-Shelf gate passed: six clear wins and a **237.703–240.456 ms** paired
median reduction, about **3.8%** of the stock median. Intervals bracket the
observed XTest button release, not statistical confidence or optical visibility.
The paired reduction is calculated within visits before taking the median;
it is not the difference between the two marginal medians.

| Endpoint from card release | A median | B median | B earlier | Paired improvement |
| --- | ---: | ---: | ---: | ---: |
| First normal Shelf software render | 6.316 s | 6.077 s | 6/6 | 237.703–240.456 ms |
| Accepted SEND for that Shelf | 6.370 s | 6.118 s | 6/6 | 249.914–252.668 ms |
| Driver completion on the same marker | 6.735 s | 6.483 s | 6/6 | 249.228–251.981 ms |

| Pair | Order | A first render | B first render | Paired first-render improvement |
| ---: | :---: | ---: | ---: | ---: |
| 1 | AB | 6.430 s | 6.274 s | 155.430–158.119 ms |
| 2 | BA | 6.106 s | 5.845 s | 259.695–262.075 ms |
| 3 | AB | 6.390 s | 6.141 s | 247.670–250.247 ms |
| 4 | BA | 5.991 s | 5.832 s | 157.786–160.658 ms |
| 5 | AB | 6.443 s | 6.150 s | 291.410–294.560 ms |
| 6 | BA | 6.243 s | 6.014 s | 227.736–230.665 ms |

The arm columns round the center of the original release bracket; exact
intervals and per-pair SEND and completion are in the projection. No sample
requires the covers, a second click, or a claim that ink was optically stable.

### Where the time went

| Stage | A median | B median |
| --- | ---: | ---: |
| Card release → launcher shim | 432 ms | 418 ms |
| Launcher shim → app invocation | 3,290 ms | 3,166 ms |
| Observer admitted → full settle receipt, inside launcher | 2,085 ms | 2,080 ms |
| Blanket query, including validation and reader reap | 150 ms | 150 ms |
| Settle receipt → native takeover begins | 835 ms | 690 ms |
| App invocation → Rust `main` | 18 ms | 17 ms |
| Rust `main` → window shown; shown → worker admission | 146 / 231 ms | 74 / 123 ms |
| State worker start → metadata ready; metadata → prepared state | 1,216 / 160 ms | 1,281 / 158 ms |
| Prepared state → attached; attached → first Shelf render | 1 / 738 ms | 1 / 749 ms |
| Render → accepted SEND; SEND → same-marker completion | 47 / 366 ms | 41 / 366 ms |

The blanket query itself did not become faster. In all six measured B visits
it had completed before the two-second gate, at a median **295 ms after
observer admission**. The protected wait, live display capture, portrait
publication and ownership barriers remained. At the within-pair median,
settle-to-takeover shortened **155 ms**, and shim-to-app invocation shortened
**133 ms**. All 14 observations retained the same positive sleep-cover
eligibility; this is a capability check, not a physical sleep/wake test.
The app-side and native dispatch intervals also varied, even though neither
implementation changed. Do not assign the whole 239 ms to the blanket
command, add these medians, or extrapolate to a signed-in library or PW6.
The [application-side replay](PRE_SHELF_ANALYSIS.md) separates the remaining
pre-Shelf spans and identifies the next measurement gaps without a new launch.

### Product disposition

Keep the overlap in the working tree; no device installation is authorized by
this comparison. The [frozen measured treatment](candidate.patch) is kept for
replay. The retained launcher additionally verifies that an exceptional
unreaped optional reader actually retires before the required capture can
reuse its records. It makes no new helper or wait on successful reaping, which
was observed in every physical launch. If a reader still owns the slot after
the bounded stop retry, startup refuses before touching display ownership.
This defensive failure branch is qualified off-device, not a new A/B arm.

The retained launcher passed all **155** lifecycle fixtures on `test-env`,
including the simulated unreaped helper, timeout fallback, advertisement and
native takeover checks. The first targeted invocation had a misspelled test
selector; after correction all six targeted tests and the complete suite passed.
The [separate final 5.20.1 Scribe firmware guest smoke](software-firmware.json)
also passed Reader and directory navigation, normal exit and native restoration.
Its first file labeled Shelf captured the loading scene, and later Reader
captures showed guest console text. These captures do not qualify a Shelf
presentation benchmark; the physical marker-bound series supplies that result.
