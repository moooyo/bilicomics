# Host lease for one installed-app observation

**Historical manual procedure, not used for the current test.** This is the host-side procedure for the [superseded manual launch](MANUAL-HISTORICAL.md), not a standing reservation or an automated device runner. For the automated render-only test see the [current study](README.md). No lease or device sample resulted from preparing this manual procedure.

The [unmodified repository helper](../../../scripts/verification/device_lease.py) was staged and compared with `cmp` at `test-env:/var/tmp/bilicomics-shelf-optical-helper-20261121.GeQkI7MA/device_lease.py`. This preparation directory was removed after the automated render study; the historical commands below are not an active handoff. It held no run, token, or Kindle connection. The real shared lease is `/tmp/codex-kindle-physical-test.lock` in the **host** namespace; do not use `ubuntu-run`, a container, a Windows path, or a substitute lock.

## Start gate

1. Finish preparation outside the lease. Wait for any known current owner to confirm native restoration and release. The operator must confirm readiness **for this manual BiliComics run**: they are beside the Kindle, can perform the launch, filter tap and normal exit immediately, and can stay through recovery. Only the optional optical measurement needs a camera; an eyewitness is enough for the functional check, and device-event timing has its own evidence requirements. Readiness for another app or an earlier report that the device was free is not a start signal. Do not query the lock, poll, reserve it, create a token, or start a background helper while waiting.
2. The owner is the approved **main** conversation, not a subagent, a project label, or a previous owner. Obtain `BILICOMICS_STUDY_OWNER` independently from the private operator approval; never set it from the current thread, a plan, or a batch receipt. Use the existing pinned `test-env` SSH alias and agent. Verify both IDs in the main conversation before forwarding them; neither is a credential.

   ```powershell
   if ($env:BILICOMICS_STUDY_OWNER -cnotmatch '\A[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}\z' -or $env:CODEX_THREAD_ID -cne $env:BILICOMICS_STUDY_OWNER) { throw 'Missing approval or wrong main conversation' }
   ssh -o StrictHostKeyChecking=yes -o BatchMode=yes -t test-env "env BILICOMICS_STUDY_OWNER=$($env:BILICOMICS_STUDY_OWNER) CODEX_THREAD_ID=$($env:CODEX_THREAD_ID) bash --noprofile --norc -i"
   ```

3. Use the following foreground host Bash blocks **only at admission**. The first block creates a fresh nonsecret run ID and a new private evidence directory. Keep the printed owner/run/batch tuple in the main conversation for recovery. No token is supplied on a command line or in an environment variable. The helper creates the token itself only on a successful reservation path; never pre-create the token file.

   ```bash
   set +x
   set -eu
   umask 077
   owner=${BILICOMICS_STUDY_OWNER:?Missing independently approved main conversation}
   test "${CODEX_THREAD_ID:?Missing main conversation identity}" = "$owner"
   helper=/var/tmp/bilicomics-shelf-optical-helper-20261121.GeQkI7MA/device_lease.py
   run="bili-shelf-$(/usr/bin/python3 -IB -c 'import uuid; print(uuid.uuid4().hex)')"
   batch="/var/tmp/bilicomics-optical-$run"
   mkdir -m 700 -- "$batch"
   printf 'owner=%s run=%s batch=%s\n' "$owner" "$run" "$batch"
   lease=(/usr/bin/python3 -IB "$helper" --lease-dir /tmp/codex-kindle-physical-test.lock
          --token-file "$batch/private-token" --owner-thread "$owner" --run-id "$run")
   "${lease[@]}" --output "$batch/acquired.json" acquire >/dev/null
   "${lease[@]}" --output "$batch/owned.json" status >/dev/null
   ```

   This is exactly **one** acquisition attempt. Global options, including `--output`, precede the subcommand. Each new receipt is exclusively created. `status` verifies only this run against its own private token; it is not an occupancy query. A nonzero exit, missing receipt, or partial operation is never a start signal. Check the two receipts privately before telling the human to begin; do not display the full JSON:

   ```bash
   /usr/bin/python3 -IB - "$batch" "$owner" "$run" <<'PY'
   import json, sys
   from pathlib import Path
   directory, owner, run = sys.argv[1:]
   rows = [json.loads((Path(directory) / name).read_text()) for name in ('acquired.json', 'owned.json')]
   for row, operation in zip(rows, ('acquire', 'status')):
       if not (row.get('ok') is True and row.get('operation') == operation and row.get('state') == 'held'
               and row.get('project') == 'BiliComics' and row.get('owner_thread') == owner and row.get('run_id') == run):
           sys.exit('Stop: held lease not established')
   fingerprint = rows[0].get('lease_token_sha256')
   if not fingerprint or rows[1].get('lease_token_sha256') != fingerprint or rows[1].get('ownership_verified') is not True:
       sys.exit('Stop: ownership not verified')
   print('Owned lease verified; the coordinator may now give the manual start signal.')
   PY
   ```

## Stop and return

- After the explicit start signal, note the original native view, then perform only the [one-launch protocol](README.md). If a different route opens or the target is unrecognizable, do not retry, change settings, or force Shelf. Normal exit and restoration still apply. Keep private library titles and video off the host and out of shared records.
- Close any filter menu without changing the selection. Use the ordinary application exit (the borderless X on Account home when available), return to the original native Home/Library view, and confirm with one harmless, reversible **native** touch that the normal UI responds; return it to its original state. The human must confirm both restoration and responsiveness. A still frame or the application's disappearance alone is insufficient. No Kindle transport is opened in this manual protocol; verify that no owned device connection remains. Do not postpone release for video playback, transcription, analysis, or acknowledgement from another requester.
- Only after this confirmation, the **same main conversation** uses the **same** tuple and the same private token file. If reconnecting to `test-env`, repeat the main identity check and reconstruct the Bash `lease` array from the recorded tuple; **do not** generate a new ID, rerun `mkdir`, or reacquire. The release guard is a truthful attestation, not a way to cause restoration.

  ```bash
  "${lease[@]}" --output "$batch/released.json" release --native-restored-and-connections-closed >/dev/null
  /usr/bin/python3 -IB - "$batch" "$owner" "$run" <<'PY'
  import json, sys
  from pathlib import Path
  directory, owner, run = sys.argv[1:]
  acquired, released = [json.loads((Path(directory) / name).read_text()) for name in ('acquired.json', 'released.json')]
  if not (released.get('ok') is True and released.get('operation') == 'release' and released.get('state') == 'released'
          and released.get('project') == 'BiliComics' and released.get('owner_thread') == owner and released.get('run_id') == run
          and acquired.get('lease_token_sha256') and released.get('lease_token_sha256') == acquired['lease_token_sha256']
          and released.get('owner_and_token_checked') is True and released.get('native_restored_and_connections_closed') is True
          and released.get('only_owned_owner_json_removed_then_rmdir') is True):
      sys.exit('Stop: exact release was not confirmed')
  print('Native control returned; the exact owned lease was released.')
  PY
  ```

The helper rechecks owner, run, file identities and private token on release, unlinks only its own `owner.json`, then removes the empty shared directory. It retains the private external token file; after successful release and receipt verification, remove **only this run's** token from its private batch directory. Do not display, copy, transmit or reuse it. The helper already supplies a public token fingerprint; compare its recorded values privately, do not recalculate it or publish it in chat. If there is a known requester actually waiting, notify them after release; do not block release on their acknowledgement.

## Refusals and interruption

- `lease_exists` or `operation_busy` with **no** partial progress means no access. Do not read another owner's record, remove anything, retry in a loop, or leave a pending acquire. Tell the operator not to touch the Kindle; coordinate a new handoff through an authorized channel. A later batch needs fresh operator readiness, run ID and token.
- If acquisition partially created anything, a check failed after acquisition, transport was interrupted, or release was partial, retain the original private tuple, token if present, and receipts. Check only the helper's safe failure code and progress flags, never print the raw token or owner record. Do not claim a release, manufacture state, delete an ambiguous lock, or attach the old case to a new lease. The original owner handles only the necessary recovery, with the operator; a missing lock after a reboot is not proof of native restoration.
- If the app has started and native control cannot yet be confirmed, stop measurement and keep the exact owned batch only for necessary recovery. Never pass the restoration flag on speculation. If nobody touched the device and the batch is abandoned after acquisition, confirm the unchanged native state and absence of owned device connections, then release immediately. Analyze evidence only after the lease is returned.
