# Unattended Scribe Shelf-render batch

This controller uses the existing native Library card exactly once. It observes an isolated demo/offline application launched by the [reversible route](route/README.md); it does not install an application, replace the Library document, open a real account, click inside Shelf, take a screenshot, or require a person at the Kindle. The only possible native inputs are switching between the recognized Home/Library tabs, opening the unique public BiliComics card, and returning the saved pointer. A single diagnostic launch is not a production or optical baseline.

## Prepare without device access

- Complete the Linux/mock, emulator and ARM admission gates before starting. The site operator must supply `BILICOMICS_STUDY_OWNER` from the private approval identifying the designated main conversation, independently of this shell, the plan and the evidence. Forward the actual `CODEX_THREAD_ID` via the operator's existing `test-env` SSH configuration and agent. Preparation, start and recovery on that host require both UUIDs to match; never substitute a delegated conversation. Preparation is offline; the historical transport helper only resolves the pinned configuration with `ssh -G` at this stage.
- Assemble a fresh [route kit](route/README.md) with a new `bili-shelf-` + 32 lowercase hexadecimal ID. Use the approved ARM hard-float binaries, exact historical shipping/document launchers and the built route helper. The closed diagnostic and production evidence is not a substitute for these input bytes.
- The native detector is the previously exercised `paired-kit.tar.gz` plus its adjacent `paired-kit.json` receipt, not a new binary or a bare recognizer. Use the existing owner-only rollout tooling directory containing `rollout.py`, `device_lease.py`, `run-pair.py`, `prepare-pair.py` and their reviewed dependencies. The normal host layout places the detector below `/var/tmp/bilicomics-click-20261120.ZP9wXtaX/detector/`; supply the exact paths on the verified host. Only these trusted, root-controlled inputs are accepted.

```bash
study=/absolute/path/to/startup-interactive-kindle-20261121
python3 -IB "$study/test_operator.py"
batch=$(python3 -IB "$study/shelf_operator.py" prepare \
  --kit "$prepared_route" --native-kit "$paired_archive" --rollout-root "$rollout_tools")
```

The printed private batch contains both prepacked payloads, the materialized native guard and a manifest bound to this run, host boot and main conversation. The credential prompt helper contains only code, not a password. Preparation does not reserve the device or create a lease token.

## Start or recover

Use a private foreground PTY with tracing off. The main coordinator supplies the already-authorized device credential at the tool's no-echo prompt before starting the one acquisition attempt. Do not put credentials in command arguments, a shell command, a transcript, a file in the kit, or a request to the human. Do not start a parallel operator.

```bash
python3 -IB "$study/shelf_operator.py" start --batch "$batch"
```

The controller uses the real shared host lease, a fresh private token and the pinned root-only Kindle SSH destination with connection sharing disabled. It checks active native control, model/firmware and available `/mnt/us` space (expanded payloads plus 64 MiB) before staging. Only the two fresh run-owned media directories are eligible for removal. A busy lease means no device contact, no queue, no polling and no access to the other owner. Keep its nonsecret owner/run/batch identity available for necessary recovery.

Activation first requires the exact installed production paths, document entry and supported filesystem operation. The host writes click intent before dispatching the one automatically recognized card. After dispatch, idleness alone is **not** authorization to undo the route: the exact picked entry must have retired and the guard must have confirmed native control. If normal retirement cannot be verified it can attempt one scoped orderly stop; no fallback batch or generic process kill is permitted. The controller restores the original launcher and native tab/pointer, projects only anonymous allowed records, removes the two stages, finishes its SSH operations, and immediately releases the exact owned lease. Analysis runs after release.

On an interrupted device batch or unconfirmed recovery, **do not acquire a new lease**, edit code, replace an owner record or rerun `start`. Only the actual owner may continue necessary recovery, using the original host boot, run, private token and private batch:

```bash
python3 -IB "$study/shelf_operator.py" recover --batch "$batch"
```

Recovery never clicks the card or republishes the route. An SSH failure before any device mutation is reported as `no-device-change`, not as evidence of launch or active native control. A scope/identity mismatch, unproven late dispatch, altered original, unfamiliar residual file, or ambiguous lease operation fails closed. An incomplete **acquisition** is not proof of ownership; use `recover` only if the original tuple can actually pass the exact helper's `status`. A confirmed release remains final even if another requester immediately acquires the shared resource.

## Results

Exit `0` requires an audited primary renderer interval. Exit `2` denotes refusal before operation (including a busy lease), `3` an unconfirmed lease/recovery state, and `4` a closed batch without an automatically accepted primary sample. Exit `3` includes the phase and the allowlisted reason when known. For example, `stock-restore: Scoped SSH failed (1; unrecognized-route)` identifies the failing gate; it does not identify which protected attribute changed, authorize bypassing the gate, or confirm release. Unknown diagnostic text is withheld.

If measurement fails inside a navigation or card probe and the exact lease is subsequently released, `probe-failure.json` may accompany the unchanged `failure.json`. Its fixed labels identify the action and whether the failure was at the native guard, command request, response validation, or after a validated receipt. `tab_request` and `card_request` describe the measurement attempt, not later cleanup: `not-requested`, `dispatch-possible` (no validated acknowledgement), or `receipt-confirmed` (accepted by the host, not necessarily persisted as an origin). These labels are a host-side snapshot, not a new device call. `255` does not identify a transport cause and `dispatch-possible` does not mean a click happened or authorize retry. The recovery logic continues to use the existing intent, process and release records. There are no coordinates, wire output, commands or credentials in the optional file. It is not written while the lease is held, and a missing file supplies no evidence.

## Replay a closed batch offline

The private plan/closure readers that check a study owner also use `BILICOMICS_STUDY_OWNER`. On a replay host without `CODEX_THREAD_ID`, supply the **original** designated owner from the independent operator approval, not from the plan, admission, token or release being checked. Without that external identity these readers refuse the replay. Pure timing or public aggregate auditors do not attest ownership.

Recovery does not calculate timings or repeat the launch. A recovery refusal **after** the observed launch ended need not invalidate that observation. Before using the existing two-file auditor, verify the following in the **same original private batch**; the auditor cannot establish these facts from its two inputs:

1. `released.json` records `state: released`, `native_restored_and_connections_closed: true`, `owner_and_token_checked: true`, `device_disposition: original-native-restored`, and the owner/run from `admission.json`. Do not contact the Kindle, reacquire its lease or reuse the batch.
2. The recorded sequence establishes that only restoration or cleanup was interrupted. The original card release and independently witnessed natural retirement had already completed: `retired.json` has `natural: true` and a positive integer `app_pid`, and there is no `stop-intended` marker. Do not take a PID from the trace header or substitute a fresh click.
3. `card.json` has that same PID, the admitted run ID, the boot from `origin.json`, and the original release bracket from `observed-click.json`. The anonymous `<run>.jsonl` exists and there is no `partial-trace` marker. A missing projection, disputed launch, forced stop or uncertain chronology is not a qualified sample.

Once these checks pass, point the auditor at the original projections on the host. This reads two files and prints JSON; it never invokes the device operator or opens SSH:

```bash
run=$(basename "$batch")
run=${run#bilicomics-interactive-}
python3 -IB "$study/log_bounds.py" "$batch/card.json" "$batch/$run.jsonl"
```

Report a primary interval only if the audit succeeds **and** `renderer_return_interval_ns` is not null. The interval brackets native release to the first qualifying renderer return; covers may be placeholders. Accepted SEND and same-marker driver completion are separate endpoints and may be absent. Identify an eligible recovered observation as one diagnostic demo/offline measurement (`n=1`, cleanup recovered), not a new run, a median, a speedup, clickability or optical panel timing. If a check fails or an endpoint is missing, report that limitation instead. Keep the small anonymous evidence; do not publish process details, library content, databases or launcher/application logs.
