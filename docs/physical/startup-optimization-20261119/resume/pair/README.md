# Paired Kindle startup: source-only handoff

**Historical-input controller, not a device package or performance result.**
It implements the [predeclared comparison](../../PLAN.md). The selector has
since passed independent host tests. That does not qualify the original
`operator.py` on a worker without its historical inputs. See the
[rebuilt-worker path](../rebuilt/README.md). Do not run these sources from the checkout.

## Explicit inputs

On a rebuilt worker without the historical click workspace, the selector
alone can be qualified by combining the [standalone host runner and fixtures](../../../library-pairs-20261120/selector/)
with this directory's unchanged `select-arms.in.sh` and `pair-fs.c`. Run
`sh build-selector.sh --standalone` in a fresh private directory with
`BILI_PAIR_WORK` and `BILI_PAIR_TEST_HOST`. This
exercises the directory switch and recovery against synthetic payloads,
including two warmups and six alternating pairs with identical payloads.
It does not run `operator.py`, arm a click, or authorize device access. Do
not invent a historical workspace to get past the physical runner's checks.

On Linux `test-env`, copy the five source files into a new root-owned mode-0700
workspace and work there. Supply:

| Environment | Meaning |
| --- | --- |
| `BILI_PAIR_WORK` | Absolute, canonical path of that new workspace; also the current directory. |
| `BILI_PAIR_HISTORY` | Separate existing read-only click research root with the original pinned `fresh.py`, `fresh_click.py`, `click-r2/admission.json` and `click-r2/plugin/`. |
| `BILI_PAIR_TEST_HOST` | Actual verification-host name from trusted inventory, as returned by its hostname API, not an arbitrary SSH alias. |
| `BILI_PAIR_OWNER` | Actual ID of the **receiving main conversation**. Forward its real `CODEX_THREAD_ID` into `test-env` separately as `CODEX_THREAD_ID`; the two must match. Never use a previous owner. |
| `BILI_PAIR_DEVICE` | Canonical IPv4 address of the explicitly authorized Kindle, from the operator. Must match the existing private, host-key-pinned transport template. |

The original R7 tools, baseline package, fixture, detector and private SSH trust
store must still exist at their verified locations. Relocating the history by
editing pinned scripts is unsupported. If a dependency or pin differs, stop.

## Order of operations

1. Review the sources and any interrupted batch. A missing `/tmp` lease after a
   host restart is not a device handoff.
2. Outside the lease, run `sh build-selector.sh` using trusted compilers. Review
   the host fixtures and ARM ELF checks; QEMU smoke is conditional. Construct a
   **dry-only** batch:

   ```sh
   python3 -IB operator.py prepare --batch "$BILI_PAIR_WORK/dry-01" --fixture
   ```

3. After successful regressions, supply the independently pinned candidate
   build JSON. It must attest
   `complete`, `package`, `package_sha256`, `binary_sha256`, `worker_sha256`,
   `launcher_sha256`, `document_sha256`, `source_fingerprint`,
   `build_receipt_path` and `build_receipt_sha256`. Use the admission SHA
   returned by **this** fresh preparation:

   ```sh
   python3 -IB operator.py prepare --batch "$BILI_PAIR_WORK/run-01" \
     --candidate-receipt "$CANDIDATE_JSON" --candidate-receipt-sha "$CANDIDATE_SHA"
   python3 -IB operator.py check --batch "$BILI_PAIR_WORK/run-01" --admission-sha "$ADMISSION_SHA"
   ```

4. Only the bound main conversation may replace `check` above with `run`, just
   before continuous device access. Only `run` acquires a lease. Admission
   binds owner, verifier host/boot, device/trust store, paths, run and program
   identities. Changed bindings require a new batch, never an old token.
5. Require both warmups, six alternating pairs of verified card clicks, native
   restoration, closed transports and exact release. Preserve failures and their
   exact recovery identities. Use the [paired reader](../../project_pairs.py)
   only on closed evidence, never replacing a click with a URI launch.
