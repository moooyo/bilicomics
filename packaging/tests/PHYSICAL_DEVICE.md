# Supervised physical-device harness

Run `scripts/verification/physical_device.py` on the designated Linux verification host. The coordinator alone obtains device access, performs all physical-device commands, asks the operator to touch or press power, and returns the exclusive lease. The harness neither acquires nor removes the shared lease.

The host lease directory must contain a regular, nonsymlink `owner.json` with `project: "BiliComics"`, the exact coordinator `owner_thread`, `state: "held"`, and a 48-character hexadecimal `run_token`. Supply the same token through a separate private `--lease-token-file`. Public reports retain only its SHA-256. Do not copy passwords, private keys, SSH control credentials, or account content into the repository or evidence archive.

Use an existing noninteractive SSH route with a pinned host key. `--ssh-helper` names an executable accepting one remote shell command as its sole argument and forwarding standard input. Alternatively use `--ssh-target` and repeated `--ssh-option` values; the harness enforces strict host-key checking and batch authentication. A Windows operator may coordinate the route, but this Python harness refuses execution on Windows.

All invocations require these common arguments:

```text
--run-id RUN_ID --output NEW_HOST_EVIDENCE_DIRECTORY
--lease-dir SHARED_LEASE_DIRECTORY --lease-owner COORDINATOR_THREAD_ID
--lease-token-file PRIVATE_EXPECTED_LEASE_TOKEN_FILE
--ssh-helper PINNED_DEVICE_SSH_HELPER
```

The run ID permits only lowercase letters, digits and hyphens, with a maximum length of 48. Each device run uses a new `/mnt/us/bilicomics-physical-RUN_ID` directory. Existing installations and data directories are never updated. Each deployed root can be started once; use a new run ID and evidence directory for a later reader session.

## Phases

1. `preflight --profile scribe --expected-firmware EXACT_VERSION --operator-model CONFIRMED_MODEL` reads firmware, framebuffer sysfs values and native service baselines. It refuses an existing production launcher lock. The model label is an explicit operator confirmation, not an automatic model detection result. Ioctl geometry follows after deploying the pinned read-only helper.
2. `deploy --package ZIP --expected-package-sha256 SHA --expected-binary-sha256 SHA --framebuffer-helper ARM_HELPER --expected-helper-sha256 SHA --wrapper scripts/verification/physical/physical-wrapper.sh --watchdog scripts/verification/physical/physical-watchdog.sh --event-wrapper scripts/verification/physical/physical-event-wrapper.sh --approve-runtime-pair` verifies selected hashes, stages retained notices, and checks every deployed file digest. The repository package allowlist must remain empty. The exact runtime profile/firmware entry applies only to this supervised run. The scripts are normalized to LF; the normalized deployment hashes are recorded.
3. `start --screen shelf --exit-after-ms 30000 --init-timeout 90 --session-timeout 180` arms the independent watchdog and starts the unchanged production launcher. Application duration begins at UI readiness. Initialization and absolute deadlines remain independent of application timer progress, including sleep. A later assisted reader run may use `--screen reader --assisted` with a longer explicitly bounded duration; the absolute watchdog maximum is 600 seconds.
4. `checkpoint --name shelf-ready --require ready --wait-seconds 90 --capture` waits for an actual app trace event, verifies the live ARM process and its demonstration/offline/native arguments, and optionally captures framebuffer memory. A reader checkpoint additionally requires current route/loading state and a valid `reader-ready` image.
5. `stop` or `release` requests graceful shutdown through the run token, waits for watchdog recovery, collects safe logs, and checks the saved native service/process baseline and absence of the production lock. These commands do not return or delete the shared lease. `collect` can also retrieve evidence after the automatic smoke exit or a failed operation.
6. `confirm --observer OBSERVER --note OBSERVATION --home --touch` records direct observation of native Home and native touch after successful process recovery. Panel observation uses `--panel --checkpoint NAME`. An assisted physical input observation uses `--input-source finger-touch`, `physical-power-button`, or `physical-sleep-and-wake`, with `--checkpoint NAME`. A trace change alone never proves which physical input was used.

For an assisted reader acceptance sequence, capture a ready reader checkpoint, ask the operator for a real page-turn touch, then use `checkpoint --name reader-turned --require reader --compare-to reader-before --expect next-page --capture`. Capture the selected reader position before sleep. Observe `--require suspended` without framebuffer capture; after the operator wakes and unlocks the device use `--require resumed --compare-to reader-before-sleep --expect retained --capture`. The comparisons require the same PID, process birth time, executable hash, reader identity, settings and source anchor. Input observations are recorded separately.

The supervised wrapper sources are maintained in `scripts/verification/physical/`
and exercised by `scripts/verification/test_physical_harness.py`. Select all three
from that directory together. They account for the production launcher's separate
power relay and the launcher, observer, source-runner, and native-event lineage.
The deployment harness uses the supplied paths exactly; it does not substitute
newer wrappers automatically.

## Ownership and evidence limits

The watchdog saves exact awesome PID/birth/executable/state records and the statusbar baseline before starting the launcher. It first terminates its recorded launcher, allows bounded production cleanup, and then signals only individually verified owned identities. The app wrapper and native power-event wrapper persist their original lineage before `exec`, allowing recovery after reparenting. The event wrapper preserves the original `lipc-wait-event` arguments and output; it does not generate events. Unknown token-bearing orphans are reported for operator review without signalling an unproven process.

Before starting the launcher, the watchdog hashes the selected fixed ELF once and checks its stable file identity before and after the complete read. It stores a same-run receipt in `/tmp/bilicomics-physical-RUN_ID-elf/verified-elf.receipt` with private directory/file permissions. Portrait metadata and publication helpers compare that receipt, issuer PID/birth, fixed path, owner, mode, link count, size, device/inode, and full `stat` mtime/ctime before and after their unchanged native-reader ancestry checks. Unsupported identity fields, missing private permissions, changed metadata, or a foreign receipt reject the helper. UI and power-relay entry points keep their existing full hashes. The production two-second read budget and ARM executable are unchanged. This receipt is an optimization for an unchanged isolated deployment, not a security boundary against a same-UID or root actor; the underlying filesystem timestamp resolution still applies.

The known `/tmp` base resolves only to `/tmp` or Kindle's `/var/tmp` alias. The watchdog and helpers use that same canonical base and recheck it with the private receipt directory's inode identity. Other temporary-base resolutions and symlinks at receipt directory/file leaves remain rejected. The alias fixture uses an isolated mock resolution rather than changing the host's `/tmp`.

Recovery resumes only recorded awesome processes that were originally running. It starts statusbar only when the saved baseline was running and this run's display context remains established. A current foreign launcher lock blocks restoration. Lock cleanup requires this run's PID, nonce and directory inode, accepts only the known entry names/types, and uses individual removal followed by `rmdir`. It never uses process-name killing, removes a foreign lock, or manages WeRead. Native service commands and cleanup waits have independent bounds. Evidence-storage errors do not skip remaining restoration attempts.

The production launcher retains its own reviewed behavior and original package hash. This harness adds independent supervision; its PID/birth/executable checks do not certify every internal production-launcher signal path.

Cross-compile `scripts/verification/physical_framebuffer.c` using the selected ARM toolchain and pin the resulting SHA-256. It opens `/dev/fb0` read-only, issues only `FBIOGET_FSCREENINFO` and `FBIOGET_VSCREENINFO`, checks geometry/stride/offset/memory bounds, and reads visible rows with bounded `pread`. `--info` emits JSON. `--capture` emits a JSON header followed by visible packed pixel bytes. The host converts confirmed grayscale or declared color bitfields into PNGs. Capture is permitted only while the live isolated demo owns the display, powerd is active, statusbar is stopped, recorded awesome instances are paused, and the current trace is not suspended. Capture races discard the payload and stop the run.

A framebuffer PNG proves memory pixels only. Physical panel refresh, startup visibility, touch behavior and Home restoration need explicit observation. Do not capture native Home, native unlock screens, account content or serial identifiers. Fixture, firmware-emulator and physical-device results remain separate. This harness does not accept a hardware support pair, select a release license, publish an artifact, reboot firmware, unplug USB, or make account writes.
