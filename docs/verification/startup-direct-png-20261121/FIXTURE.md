# Closed offline cover fixture

This input exercises the ordinary application catalog and managed cover cache without an account login, external requests or purchased content. No account data, captured comics or existing SQLite rows are inputs. It is not a real-account or physical-device performance result.

## Prepare on the verification worker

Use a private working directory, private home, network-isolated environment and the control application. The directories and output file in this example must not already exist. The exit of the control and Xvfb must be observed before invoking the generator.

```sh
umask 077
SLINT_BACKEND=winit-software WINIT_UNIX_BACKEND=x11 SLINT_SCALE_FACTOR=2 \
  xvfb-run -a -s '-screen 0 1860x2480x24 -nolisten tcp' \
  ./control --offline --screen shelf --device scribe --data-dir "${PWD}/empty-control" --exit-after-ms 800
python3 -B make_fixture.py "${PWD}/empty-control" "${PWD}/seed" "${PWD}/seed.json"
```

The initializer is not a timing sample. The generator requires Python 3, SQLite and Pillow. It copies only the table and index definitions from the closed schema-v4 file created by the control, not the database pages, source preferences or application directories. It refuses an open journal, a session or an existing source catalog, creates new tables, inserts the synthetic rows under `bili_424242` and closes a truncated WAL. The seed contains no session, transport, downloaded chapters, reading anchor or private key. The only SHA-256 operations derive filenames from the application cover URLs and the category key for `store:all`; neither is an integrity check. All cover URLs use the reserved `.invalid` domain.

The device's synthetic `last_account` preference selects the same twenty catalog rows even without an identity. Keep one private copy without a session as the **signed-out Shelf negative control**: those images can be prepared, but the login prompt prevents their display. Use the separate qualified, in-memory mock identity setup only on a second disposable copy; no real authentication, credentials or purchases are involved. A picture from Store is not evidence that the signed-in Shelf displayed its covers. Never archive a copy containing the mock session. Both app arms use fresh copies of the same closed, qualified input with `--offline`, without `--demo`.

## Build and use the isolated seed helper

The helper is a separate, unpublished Cargo package, not an example or feature added to either application. In the private `test-env` build directory mounted as `/work`, stage a **separate copy of arm A** at `control-source/`: extract `git archive 9175002` there, then overlay only `src/app/catalog.rs`, `src/store.rs` and `src/store/catalog_cache_tests.rs` from the study checkout. Do not copy B's startup changes. From this document's directory, copy `seed_profile.rs` and `seed-profile.Cargo.toml` into a new mode-0700 `seed-helper/`, naming the latter `Cargo.toml`. These are the only helper sources. The absolute paths in its manifest refer to that isolated mount.

After saving both formal executables, run the following on `test-env`, with `BUILD_ROOT` set to that private host directory. The helper shares this run's `/work/target` dependency cache, not a global target or either arm's manifest or lockfile. Offline lock resolution adds only `bilicomics-offline-seed-profile` to the copy of the control lock; stop if other packages change.

```sh
unshare --net bash -ec 'ip link set lo up; exec "$@"' isolated \
  /opt/kindle-toolchain/ubuntu-run --build "$BUILD_ROOT" bash -ec '
    set -eu; umask 077; cd /work/seed-helper
    cp /work/control-source/Cargo.lock Cargo.lock
    export CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/work/target
    cargo update --offline --workspace
    cargo build --release --locked --offline -j1 --bin seed-profile
    install -m 700 /work/target/release/seed-profile ./seed-profile'
```

Copy only `seed-helper/seed-profile` to the desktop runner's private `/work/seed-profile`. Follow [the desktop protocol](PROTOCOL.md) in a loopback-only namespace: `ordinary_pairs.py bootstrap`, then `ordinary_pairs.py make-seed`, **instead of** generating the standalone `seed` above. The latter creates the fixed `/work/direct-png-fixture/.fixture-only` marker and `data/`, runs `seed-profile encode`, copies the encoded data to `guest/` **before** any identity exists, and only then runs `seed-profile identity` on `data/`. The encoder compares all decoded RGBA bytes; the identity phase uses one closed in-process fixture request, normal validation and private `SessionFile` save/reload. Neither phase accepts a path or credentials. Keep all seed and per-launch copies on the worker, never commit or archive them, and remove them after verification. Only the [non-sensitive qualification receipt](seed-qualification.json) is retained here.

## Positive visual evidence

The ordinary Scribe grid has two Shelf pages, no resume card, and ten covers on page one. Each static PNG is 320 by 416, straight RGBA8, without interlacing. The ten prepared images occupy 5,324,800 RGBA bytes, below the 16 MiB preparation limit. With the existing 4 MiB attachment budget, the first chunk handles eight covers, the next two. The separately available guest Store fixture displays 15 covers on its first page and is not the Shelf endpoint.

Each cover encodes its one-based position in five large opaque patches centred on the horizontal midline. Left to right, the patches are bits 4 through 0, with dark 32 for zero and light 224 for one (all three color channels). Per-cover source coordinates are in `covers[].marker.samples` in the generated receipt. A second patch at `(160,100)` is translucent `(45,170,114,112)`; the rest of the artwork varies independently in color and alpha. Check the opaque patches inside each actual displayed tile away from its outlines/badges, not just the final route, page count or cache filenames. Oracle screenshots from the actual control must show the expected distinct markers; a separate signed-out or missing-cover control must not meet that positive condition.
