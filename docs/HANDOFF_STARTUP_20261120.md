# Startup optimization handoff: 2026-11-20

**Next action: verify the candidate on `test-env`. Do not deploy it yet.**
The operator requested a source merge to `main` so another session can continue.
That merge is not a release or a performance result. The last session lost SSH
access to `test-env` before a candidate build or regression run. No candidate
ELF, ZIP, real-device samples or speedup were established; no Kindle lease was
acquired for the candidate. Recheck host availability rather than assuming it
is still down.

## What to compare

| Item | Identity and meaning |
| --- | --- |
| A: measured product | Git baseline `d81b529fd8f84790d91dfe3a8396853c296a8280`; qualified development ZIP SHA-256 `c57d5fc3674f1a0de448b85beb2552ee0959b83756c3e8f62afb559bf62a4882`. Use its verified package, application, worker and launcher, not a reconstruction from an arbitrary `main`. |
| Previous physical evidence | `55189dd9bb48226cb9039a138ab686f7c329b290` records the [card-click study](physical/library-to-shelf-20261119/README.md). From release of an XTest click on the native Library card to the full Shelf frame: **7.137 s** median to accepted SEND and **7.503 s** to the matching driver completion (range **6.557-7.762 s**). One first launch was recorded separately from five repeats, with new processes, retained caches and an offline demo on one Scribe. Not optical settling or live-account timing. |
| B: unverified candidate | `57265de60553ea97b01894592427d60c9008f850` contains exactly six implementation and test files. The launcher overlaps log setup with the **full two seconds** of observer settling. Ordinary Shelf navigation and projection finish in one UI callback only if the resulting route is still Shelf. The commit includes the corresponding recovery and regression changes. [Change details](physical/startup-optimization-20261119/README.md). |

The plan is to measure the **combined** changes. Do not subtract a new run from
7.503 s, add the two suspected savings, or present the one-second target as an
achieved result.

## Resume in order

1. **Check the checkout and host, without a lease.** Read `AGENTS.md`, fetch
   `origin`, and confirm B is an ancestor of `origin/main`. Preserve unrelated
   edits. Check `ssh test-env`, free disk/memory and the retained build inputs.
   A fresh clone contains the [build sources](physical/startup-optimization-20261119/resume/build/README.md)
   and [paired-runner sources](physical/startup-optimization-20261119/resume/pair/README.md),
   not the private transport, old build cache, clocks or detector artifacts.
   Identify the retained click workspace by its `click-r2/admission.json`
   (SHA-256 `ff79272b72dabd0fc46ee3a525bed11c15686ddaf9c005de0464dfd1076fb6de`).
   Do not use this chat's ignored staging tree as the authority for a new run.
   If a pinned input is missing, recover or independently qualify it before use.
2. **Build and verify B on `test-env` only.** Export the six candidate files
   from the exact code commit above and archive A separately, following the build
   instructions. Work in a fresh private workspace, with two Cargo jobs and a
   read-only historical cache. Check the source fingerprint, actual ARM build,
   package contents, Rust tests, launcher and watchdog regressions, startup
   frames, and real-reader fixtures. Require successful `build-receipt.json`,
   `package-static.json` **and** `software.json` for the same candidate. A ZIP or
   `candidate.json` by itself is not a regression pass. If SSH or a check fails,
   stop here; do not fall back locally or acquire the device.
3. **Prepare the comparison, still without the device.** Use the new session's
   actual thread ID, a fresh host work directory and the authorized target with
   its pinned transport. Review the unexecuted paired-runner templates. Run the
   selector's host tests, build its ARM helper and use a dry-only same-package
   fixture first. Then prepare and check a *new* real admission with separately
   pinned A and B identities. Never reuse an earlier owner, token, admission or
   evidence directory. Keep historical tool and package hashes unchanged.
4. **Run the [predeclared comparison](physical/startup-optimization-20261119/PLAN.md).**
   Acquire `test-env:/tmp/codex-kindle-physical-test.lock` only immediately
   before actual device access. Coordinate with the current owner if occupied;
   after a host restart a missing `/tmp` lock alone does not establish that an
   interrupted device session was restored. The batch opens the genuine native
   card on both arms: one warmup each, then six alternating AB/BA pairs. Keep all
   launches and failures. Restore the original native tab, control and installation
   state, close the connections and release the exact owned lease before analysis.
5. **Publish the outcome, not just the fastest launch.** Run the
   [paired analyzer](physical/startup-optimization-20261119/project_pairs.py)
   on the closed evidence on `test-env`. Report both endpoints, the signed
   per-pair B-minus-A differences, ranges, and how often B was faster. Verify
   the complete Shelf and the preserved startup feedback. If the total latency
   does not improve or behavior regresses, record that and withhold promotion.
