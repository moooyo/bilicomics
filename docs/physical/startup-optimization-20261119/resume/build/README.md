# Rebuild and qualify the ARM candidate

This is **host-only handoff source, not a successful build or test result**. Use only the designated Linux `test-env` worker. The scripts do not contact a Kindle, acquire a device lease, deploy, publish, or modify the original Cargo target.

**Freeze the inputs.** From a clean checkout containing both commits, create these two Git archives for transfer. PowerShell example, using a fresh temporary output directory:

```powershell
$stage = Join-Path ([System.IO.Path]::GetTempPath()) ([Guid]::NewGuid().ToString())
New-Item -ItemType Directory $stage | Out-Null
$base = 'd81b529fd8f84790d91dfe3a8396853c296a8280'
$new = '57265de60553ea97b01894592427d60c9008f850'
git archive --format=tar.gz --output="$stage/baseline.tar.gz" --prefix=app/ $base .cargo Cargo.toml Cargo.lock build.rs README.md THIRD_PARTY_NOTICES.md assets src ui vendor tests scripts packaging docs/research
git archive --format=tar.gz --output="$stage/candidate-files.tar.gz" $new packaging/kindle/launch.sh packaging/tests/launcher_test.py scripts/verification/launcher_real_reader.py scripts/verification/physical/physical-watchdog.sh scripts/verification/test_physical_harness.py src/app/startup.rs
```

**Prepare the worker.** Transfer the two archives and the three files `prepare.sh`, `box.sh`, `build.py` into a fresh mode-0700 directory outside any checkout. That deployment directory is the work root. Do not copy or hard-link mutable historical targets. On `test-env` set `BILI_BASE_BUILD_ROOT` to the canonical root of the retained startup-sequence build (`target/`, `candidate-r1/app/`, `checks-r3/` and `startup-boundaries-r1.py`) and `BILI_BASE_ZIP` to the absolute path of the qualified 49-entry ZIP, next to its selected `bilicomics` and `kindle-jpeg-worker` ELFs. The exported settings must be available to each command below. The recorded compiler proof authenticates the historical build location; a guess or substitute path is not a cache match.

In that private directory, run `bash prepare.sh`. It verifies both Git commit headers, the baseline native-input manifest, compiler and package pins, the exact six candidate member paths and their individual SHA-256 values. Directory and tar compression metadata need not match a previous archive. A failed gate is not permission to relax the hashes: retain the failed workspace and fix the input in a new one.

**Run the two software gates**, sequentially on `test-env`, with no device lease:

```sh
unshare --mount --propagation private bash ./box.sh arm
unshare --mount --propagation private bash ./box.sh verify
```

The first gate uses a private read-only lower mount and run-owned COW upper at the original compiler path, limits Cargo to two jobs, force-cleans the selected app/backend in the overlay, and verifies the ARM package. Do not run `cargo clean` directly on the historical target. The second runs the Rust, shell launcher, watchdog, real-reader lifecycle, startup UI and package regressions. Use a healthy worker with sufficient disk/memory; the free-space guards and command deadlines are intentional. The results are `combined/build-receipt.json`, `combined/package-static.json` and `combined/software.json`; `combined/candidate.json` is issued only after both gates succeed. Keep private machine paths and detailed logs on the worker. These software results do not authorize deployment or establish a performance improvement.
