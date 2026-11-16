#!/usr/bin/env python3
"""Host-only regression fixtures for the aggregate startup coordinator."""

import contextlib
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import tarfile
import tempfile
import time
import unittest
import zipfile


HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("startup_ab", HERE / "startup_ab.py")
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


def fingerprint(value):
    return hashlib.sha256(value).hexdigest()


def fixture_launcher():
    return b'''#!/bin/sh
DATA_DIR=$BILICOMICS_HOME/data
if [ "${1:-}" != --detached ]; then
    if command -v setsid >/dev/null 2>&1; then
        setsid sh "$SCRIPT_PATH" --detached >/dev/null 2>&1 </dev/null &
    else
        nohup sh "$SCRIPT_PATH" --detached >/dev/null 2>&1 </dev/null &
    fi
    exit 0
fi
run_application() {
    "$APP_DIR/bilicomics" --kindle --device "$PROFILE" --data-dir "$DATA_DIR" &
    runner_child=$!
}
'''


class PreparationTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="bili-startup-tools-", dir="/root")
        self.directory = Path(self.temporary.name)
        self.batch = self.directory.with_name(self.directory.name + "-batch")

    def tearDown(self):
        if self.batch.exists():
            self.assertEqual(self.batch.parent, Path("/root"))
            self.assertTrue(self.batch.name.startswith("bili-startup-tools-"))
            shutil.rmtree(self.batch)
        self.temporary.cleanup()

    def configuration(self):
        elf = bytearray(80)
        elf[:7] = b"\x7fELF\x01\x01\x01"
        elf[18:20] = b"(\0"
        elf = bytes(elf)
        package = self.directory / "fixture.zip"
        launcher = fixture_launcher()
        with zipfile.ZipFile(package, "w") as archive:
            archive.writestr("bilicomics/app/bilicomics", elf)
            archive.writestr("bilicomics/app/kindle-jpeg-worker", elf)
            archive.writestr("bilicomics/launch.sh", launcher)
        face = self.directory / "helper"
        face.write_bytes(elf)
        data = self.directory / "data"
        data.mkdir()
        (data / "state.sqlite3").write_bytes(b"not a live database; preparation fixture")
        fixture = [{"path": "state.sqlite3", "bytes": (data / "state.sqlite3").stat().st_size,
                    "sha256": fingerprint((data / "state.sqlite3").read_bytes())}]
        transport = self.directory / "transport.json"
        transport.write_text("{}")
        guard = self.directory / "guard.py"
        guard.write_text("raise RuntimeError('Preparation must never execute transport')\n")
        lease = self.directory / "device_lease.py"
        lease.write_text("raise RuntimeError('Preparation must never access the lease')\n")
        record = {"package": str(package), "package_sha256": fingerprint(package.read_bytes()),
                  "source_commit": "06ad90b02edb28db43abeed3031932a91dc2f8df",
                  "source_fingerprint": "a" * 64, "compiler_fingerprint": "b" * 64,
                  "binary_sha256": fingerprint(elf), "worker_sha256": fingerprint(elf),
                  "launcher_sha256": fingerprint(launcher)}
        config = {"owner_thread": "01a1d11a-1833-7703-aa40-167b49276737", "profile": "scribe",
                  "firmware": "5.19.5", "fixture_scope": "isolated-demo-offline", "fixture_data": str(data),
                  "fixture_fingerprint": fingerprint((json.dumps(fixture, sort_keys=True, separators=(",", ":")) + "\n").encode()),
                  "startup_clock": str(face), "startup_clock_sha256": fingerprint(elf),
                  "physical_framebuffer": str(face), "physical_framebuffer_sha256": fingerprint(elf),
                  "variants": {"baseline": dict(record), "candidate": {**record, "source_commit": "1" * 40}},
                  "guarded_transport": str(guard), "lease_utility": str(lease), "transport_template": str(transport),
                  "preserve_paths": ["/mnt/us/bilicomics/app/bilicomics"]}
        return config

    def prepare(self, config):
        path = self.directory / "config.json"
        path.write_text(json.dumps(config))
        with contextlib.redirect_stdout(io.StringIO()):
            module.prepare(path, self.batch)

    def test_prepare_seals_twelve_cases_without_transport_or_lease_access(self):
        self.prepare(self.configuration())
        self.assertFalse((self.batch / "private/lease-token").exists())
        config = json.loads((self.batch / "private/config.json").read_bytes())
        self.assertEqual(len(config["cases"]), 12)
        self.assertEqual(config["cases"][:6], ["baseline-warmup", "candidate-warmup", "baseline-01", "candidate-01", "candidate-02", "baseline-02"])
        with tarfile.open(self.batch / "inputs/payload.tar.gz") as archive:
            names = archive.getnames()
            owners = [archive.extractfile(name).read() for name in names if name.startswith("out/") and name.endswith("/owner")]
            self.assertEqual(len(owners), 12)
            self.assertEqual(len(set(owners)), 12)
            for name in names:
                if name.endswith(".sh"):
                    subprocess.run(["sh", "-n"], input=archive.extractfile(name).read(), check=True)
        self.assertFalse(any((self.batch / "evidence").glob("lease-*")))

    def test_prepare_rejects_mismatched_compiler_before_any_device_access(self):
        config = self.configuration()
        config["variants"]["candidate"]["compiler_fingerprint"] = "c" * 64
        with self.assertRaisesRegex(RuntimeError, "Compiler"):
            self.prepare(config)
        self.assertFalse((self.batch / "private/lease-token").exists())

    def test_prepare_rejects_a_changed_source_bound_package(self):
        config = self.configuration()
        Path(config["variants"]["candidate"]["package"]).write_bytes(b"changed")
        with self.assertRaisesRegex(RuntimeError, "package identity"):
            self.prepare(config)

    def test_prepare_rejects_a_linked_demo_fixture(self):
        config = self.configuration()
        (Path(config["fixture_data"]) / "linked").symlink_to(self.directory / "helper")
        with self.assertRaisesRegex(RuntimeError, "fixture link"):
            self.prepare(config)

    def test_prepare_rejects_a_linked_fixture_root(self):
        config = self.configuration()
        linked = self.directory / "linked-root"
        linked.symlink_to(config["fixture_data"], target_is_directory=True)
        config["fixture_data"] = str(linked)
        with self.assertRaisesRegex(RuntimeError, "fixture root"):
            self.prepare(config)

    def test_instrumentation_keeps_native_reader_and_adds_only_ui_arguments(self):
        original = fixture_launcher() + b'helper="$APP_DIR/bilicomics --capture-native-display"\n'
        actual = module.instrument_launcher(original)
        self.assertIn(b'helper="$APP_DIR/bilicomics --capture-native-display"', actual)
        self.assertEqual(actual.count(b"--performance-trace"), 1)
        self.assertIn(b"--demo --offline --screen shelf", actual)
        self.assertIn(b"launch_trace research_application_started", actual)

    def test_ambiguous_launcher_anchor_is_rejected(self):
        with self.assertRaisesRegex(RuntimeError, "anchor"):
            module.instrument_launcher(fixture_launcher() * 2)


class SupervisorTests(unittest.TestCase):
    def scenario(self, mode):
        with tempfile.TemporaryDirectory(prefix="bili-startup-supervisor-") as temporary:
            root = Path(temporary)
            stage, runtime = root / "stage-test", root / "runtime-test"
            out, app, lock = stage / "out/baseline-01", stage / "baseline/app", root / "launcher.lock"
            for path in (out, app, runtime, stage / "cases/baseline-01/data", root / "bin"):
                path.mkdir(parents=True, exist_ok=True)
            token = "d" * 64
            for path, value in ((stage / "owner", "fixture"), (runtime / "owner", "fixture"), (out / "owner", token)):
                path.write_text(value + "\n")
            shutil.copyfile("/usr/bin/sleep", app / "bilicomics")
            (app / "bilicomics").chmod(0o755)
            native = subprocess.Popen(["sleep", "120"])
            process = None
            try:
                fields = Path(f"/proc/{native.pid}/stat").read_text().rsplit(") ", 1)[1].split()
                recorded_birth = int(fields[19]) + (1 if mode == "native-replaced" else 0)
                (out / "native-before.tsv").write_text(f"{native.pid} {recorded_birth} {Path('/proc/' + str(native.pid) + '/exe').resolve()}\n")
                (out / "statusbar-before.txt").write_text("statusbar start/running, process 123\n")
                (root / "status-state").write_text("running\n")
                (root / "bin/status").write_text(f"#!/bin/sh\ncase $(cat {module.q(str(root / 'status-state'))}) in running) printf 'statusbar start/running, process 123\\n';; *) printf 'statusbar stop/waiting\\n';; esac\n")
                (root / "bin/status").chmod(0o755)
                (root / "bin/start").write_text(f"#!/bin/sh\nprintf 'running\\n' >{module.q(str(root / 'status-state'))}\n")
                (root / "bin/start").chmod(0o755)
                (root / "bin/usleep").write_text("#!/bin/sh\nexec sleep 0.1\n")
                (root / "bin/usleep").chmod(0o755)
                launcher = stage / "baseline/launch.sh"
                nonce = "e" * 32
                duration = "120" if mode == "hang" else "3"
                exit_behavior = ""
                if mode == "orphan-native":
                    exit_behavior = f"trap - 0\nkill -STOP {native.pid}\nprintf 'stopped\\n' >{module.q(str(root / 'status-state'))}\n"
                signal_behavior = "trap '' TERM" if mode == "hang" else "trap 'exit 143' TERM"
                launcher.write_text(f'''#!/bin/sh
set -eu
lock={module.q(str(lock))}
mkdir "$lock"
printf '%s\\n' "$$" >"$lock/pid"
printf '%s\\n' {nonce} >"$lock/nonce"
printf '%s\\n' 1:{nonce} >"$lock/generation"
child=''
cleanup() {{
    if [ -n "$child" ]; then kill -TERM "$child" 2>/dev/null || :; wait "$child" 2>/dev/null || :; fi
    if [ "$(cat "$lock/nonce")" = {nonce} ]; then
        rm -f "$lock/pid" "$lock/nonce" "$lock/generation" "$lock/application-pid"
        rmdir "$lock"
    fi
}}
trap cleanup 0
{signal_behavior}
"$BILICOMICS_HOME/app/bilicomics" {duration} &
child=$!
printf '%s\\n' "$child" >"$lock/application-pid"
printf '%s\\n' '{{"kind":"startup","stage":"ui_ready"}}' '{{"kind":"native","stage":"frame","data":{{"rendered":true,"refresh_accepted":true}}}}' >"$BILICOMICS_RESEARCH_OUT/performance.jsonl"
wait "$child"
{exit_behavior}
''')
                supervisor = module.SUPERVISOR.replace("/mnt/us/bilicomics-startup-*", module.q(str(root / "stage-")) + "*")
                supervisor = supervisor.replace("/tmp/bilicomics-startup-*", module.q(str(root / "runtime-")) + "*")
                supervisor = supervisor.replace("lock=/tmp/bilicomics-launcher.lock", "lock=" + module.q(str(lock)))
                if mode == "hang":
                    supervisor = supervisor.replace("now_value + 45", "now_value + 2")
                if mode in ("hang", "foreign-lock", "native-replaced"):
                    supervisor = supervisor.replace("now_value + 25", "now_value + 2")
                path = stage / "supervise.sh"
                path.write_text(supervisor)
                environment = {**os.environ, "PATH": str(root / "bin") + ":" + os.environ["PATH"]}
                process = subprocess.Popen(["sh", str(path), str(stage), str(runtime), "baseline-01", token, "fixture"],
                                           env={**environment, "BILICOMICS_STARTUP_CASE_TOKEN": token},
                                           start_new_session=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
                if mode in ("abort", "foreign-lock"):
                    deadline = time.monotonic() + 5
                    while not (out / "launcher-lock.identity").exists():
                        self.assertLess(time.monotonic(), deadline)
                        time.sleep(0.02)
                    if mode == "abort":
                        batch = object.__new__(module.Batch)
                        batch.stage, batch.runtime, batch.run_id, batch.active_case = str(stage), str(runtime), "fixture", "baseline-01"
                        def command(script, name, timeout, required):
                            result = subprocess.run(["sh", "-c", "set -eu\n" + script], env=environment,
                                                    stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=timeout)
                            return result.stdout if result.returncode == 0 else None
                        batch.command = command
                        batch.abort_active()
                        self.assertIsNone(batch.active_case)
                    else:
                        (lock / "nonce").write_text("f" * 32 + "\n")
                output, error = process.communicate(timeout=40)
                report = json.loads((out / "recovery.json").read_bytes())
                if mode == "normal":
                    self.assertEqual(process.returncode, 0, error.decode())
                    self.assertEqual(report["launcher_exit_status"], 0)
                    self.assertTrue(report["passed"], report)
                    self.assertTrue(report["owned_processes_gone"], report)
                    self.assertTrue(report["native_restored"], report)
                    self.assertFalse(lock.exists())
                elif mode == "abort":
                    self.assertFalse(report["passed"], report)
                    self.assertTrue(report["owned_processes_gone"], report)
                    self.assertTrue(report["native_restored"], report)
                    self.assertFalse(lock.exists())
                elif mode == "orphan-native":
                    self.assertFalse(report["passed"], report)
                    self.assertEqual(report["fallback_attempted"], 1)
                    self.assertTrue(report["native_restored"], report)
                    self.assertTrue(report["owned_processes_gone"], report)
                    self.assertFalse(lock.exists())
                    self.assertEqual((root / "status-state").read_text(), "running\n")
                elif mode == "hang":
                    self.assertFalse(report["passed"], report)
                    self.assertFalse(report["owned_processes_gone"], report)
                    self.assertTrue(lock.exists())
                elif mode == "foreign-lock":
                    self.assertFalse(report["passed"], report)
                    self.assertFalse(report["native_restored"], report)
                    self.assertEqual((lock / "nonce").read_text(), "f" * 32 + "\n")
                elif mode == "native-replaced":
                    self.assertFalse(report["passed"], report)
                    self.assertFalse(report["native_restored"], report)
                    self.assertFalse(lock.exists())
            finally:
                if process is not None:
                    # This private host fixture created its own process session.
                    # No real device, production lock or system service is used.
                    try:
                        os.killpg(process.pid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass
                    process.wait(timeout=5)
                native.send_signal(signal.SIGCONT)
                native.terminate()
                native.wait(timeout=5)

    def test_normal_exit_reaps_once_and_restores_all_owned_processes(self):
        self.scenario("normal")

    def test_abort_recognizes_initial_case_token_and_restores_native(self):
        self.scenario("abort")

    def test_hanging_owned_launcher_does_not_restore_over_a_live_worker(self):
        self.scenario("hang")

    def test_foreign_lock_nonce_is_preserved(self):
        self.scenario("foreign-lock")

    def test_replaced_native_identity_blocks_acceptance(self):
        self.scenario("native-replaced")

    def test_drained_orphan_native_state_has_bounded_guarded_recovery(self):
        self.scenario("orphan-native")


class ProofFailureTests(unittest.TestCase):
    def ownership_probe(self, recorded, live_pid, proc_result, token_result):
        with tempfile.TemporaryDirectory(prefix="bili-startup-proof-") as temporary:
            root = Path(temporary)
            (root / "proc").mkdir()
            if live_pid is not None:
                (root / "proc" / str(live_pid)).mkdir()
            (root / "owned-processes.tsv").write_text(recorded)
            start = module.SUPERVISOR.index("owned_gone() {\n")
            end = module.SUPERVISOR.index("\n}\n", start) + len("\n}\n")
            function = module.SUPERVISOR[start:end].replace("/proc/", str(root / "proc") + "/")
            script = f'''set -eu
out={module.q(str(root))}
proc_read() {{ {proc_result}; }}
has_case_token() {{ return {token_result}; }}
{function}
if owned_gone; then exit 0; else exit 1; fi
'''
            return subprocess.run(["sh", "-c", script], stdout=subprocess.PIPE, stderr=subprocess.PIPE).returncode

    def test_existing_recorded_process_with_unreadable_stat_is_not_gone(self):
        self.assertEqual(self.ownership_probe("123 456 1 1 1 /fixture\n", 123, "return 1", 1), 1)

    def test_unknown_existing_process_with_unreadable_environment_is_not_gone(self):
        self.assertEqual(self.ownership_probe("", 789, "return 1", 2), 1)

    def test_verified_missing_recorded_process_is_gone(self):
        self.assertEqual(self.ownership_probe("123 456 1 1 1 /fixture\n", None, "return 1", 1), 0)

    def test_verified_reused_pid_has_no_old_birth_ownership(self):
        self.assertEqual(self.ownership_probe("123 456 1 1 1 /fixture\n", 123, "proc_birth=999; return 0", 1), 0)

    def test_transport_timeout_rejects_a_mismatched_birth(self):
        process = subprocess.Popen(["sleep", "30"], start_new_session=True)
        try:
            fields = Path(f"/proc/{process.pid}/stat").read_text().rsplit(") ", 1)[1].split()
            batch = object.__new__(module.Batch)
            self.assertFalse(batch.signal_transport(process, int(fields[19]) + 1, signal.SIGTERM))
            self.assertIsNone(process.poll())
        finally:
            process.terminate()
            process.wait(timeout=5)

    def test_kernel_flag_exclusion_requires_a_verified_flag(self):
        with tempfile.TemporaryDirectory(prefix="bili-startup-kernel-proof-") as temporary:
            root = Path(temporary)
            process = root / "proc/123"
            process.mkdir(parents=True)
            functions = module.PROC_FUNCTIONS.replace("/proc/", str(root / "proc") + "/")
            for flags, expected in ((2097152, 1), (0, 2)):
                # Both tasks have I state and an unreadable/missing environment.
                # Only a successfully parsed PF_KTHREAD proves kernel ownership.
                tail = ["I", "2", "0", "0", "0", "-1", str(flags)] + ["0"] * 12 + ["12"]
                (process / "stat").write_text("123 (untrusted name) " + " ".join(tail) + "\n")
                script = functions + "\ntoken=fixture\nhas_case_token 123\nexit $?\n"
                result = subprocess.run(["sh", "-c", script], capture_output=True)
                self.assertEqual(result.returncode, expected)

    def test_service_helper_with_later_unreadable_stat_is_not_waited_or_signalled(self):
        with tempfile.TemporaryDirectory(prefix="bili-startup-service-proof-") as temporary:
            root = Path(temporary)
            (root / "bin").mkdir()
            (root / "owned-processes.tsv").write_text("")
            status = root / "bin/status"
            status.write_text("#!/bin/sh\nexec sleep 1.5\n")
            status.chmod(0o755)
            # Keep the first real stat observation, then emulate an existing
            # task whose later identity reads are denied. Waiting for the live
            # helper would take its full 1.5 seconds and violate this fixture.
            functions = module.PROC_FUNCTIONS.replace("proc_read() {", "original_proc_read() {", 1)
            script = (f"out={module.q(str(root))}\ntoken=fixture\n" + functions + "\n" + module.NATIVE_RECOVERY + r'''
reads=0
proc_read() {
    reads=$((reads + 1))
    if [ "$reads" -eq 1 ]; then original_proc_read "$1"; else return 1; fi
}
now_seconds
deadline=$((now_value + 20))
if fallback_service_call 3 status statusbar; then exit 99; else exit 0; fi
''')
            started = time.monotonic()
            result = subprocess.run(["sh", "-c", script],
                                    env={**os.environ, "PATH": str(root / "bin") + ":" + os.environ["PATH"]},
                                    capture_output=True, timeout=3)
            self.assertEqual(result.returncode, 0, result.stderr.decode())
            self.assertLess(time.monotonic() - started, 0.9)
            record = (root / "owned-processes.tsv").read_text().split()
            self.assertTrue(record)
            process = Path("/proc") / record[0]
            # Let this deliberately un-signalled host fixture finish naturally.
            limit = time.monotonic() + 3
            while process.exists() and time.monotonic() < limit:
                try:
                    state = (process / "stat").read_text().rsplit(") ", 1)[1].split()[0]
                    if state in ("Z", "X"):
                        break
                except FileNotFoundError:
                    break
                time.sleep(0.02)


class CancellationTests(unittest.TestCase):
    def test_deferral_does_not_pass_blocked_control_signals_to_a_child(self):
        code = "import signal; print(','.join(str(int(x)) for x in signal.pthread_sigmask(signal.SIG_BLOCK, [])))"
        with module.Cancellation():
            with module.defer_control_signals():
                output = subprocess.check_output([sys.executable, "-I", "-c", code], text=True)
        inherited = {int(value) for value in output.strip().split(",") if value}
        self.assertFalse(inherited.intersection(int(value) for value in module.CONTROL_SIGNALS))

    def test_first_signal_raises_once_and_later_recovery_signals_do_not_interrupt(self):
        control = module.Cancellation()
        with self.assertRaises(module.BatchCancelled):
            control.handle(signal.SIGTERM, None)
        control.handle(signal.SIGHUP, None)
        control.recovering = True
        control.handle(signal.SIGINT, None)
        self.assertEqual(control.received, [signal.SIGTERM, signal.SIGHUP, signal.SIGINT])

    def test_command_closes_its_actual_owned_transport_after_keyboard_interrupt(self):
        from unittest.mock import patch
        with tempfile.TemporaryDirectory(prefix="bili-startup-cancel-") as temporary:
            root = Path(temporary)
            (root / "tools").mkdir()
            (root / "evidence").mkdir()
            (root / "tools/guarded-device-ssh.py").write_text("import time\ntime.sleep(30)\n")
            batch = object.__new__(module.Batch)
            batch.root, batch.transport = root, root / "unused-private-config.json"
            batch.marker, batch.transports = "host-fixture", []
            batch.record = {"commands": []}
            original = subprocess.Popen.communicate
            invoked = []
            def interrupted_once(process, *arguments, **keywords):
                invoked.append(process.pid)
                if len(invoked) == 1:
                    raise KeyboardInterrupt("controlled host fixture")
                return original(process, *arguments, **keywords)
            with patch.object(subprocess.Popen, "communicate", interrupted_once):
                with self.assertRaises(KeyboardInterrupt):
                    batch.command("true", "cancelled-command")
            self.assertEqual(len(batch.transports), 1)
            self.assertFalse(Path(f"/proc/{batch.transports[0]['pid']}").exists())
            self.assertTrue(batch.record["commands"][0]["interrupted"])


if __name__ == "__main__":
    if sys.platform != "linux":
        raise SystemExit("Run this fixture on test-env")
    unittest.main()
