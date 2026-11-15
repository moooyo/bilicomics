"""Check physical-run recovery evidence with owned Linux host processes only.

Run only on test-env. The harness transport is replaced by local read-only
process snapshots; no Kindle connection or shared device lease is accessed.
"""

import json
import os
from pathlib import Path
import shlex
import shutil
import subprocess
import sys
import tempfile
import time
from types import SimpleNamespace
import unittest

import physical_device as physical


def setUpModule():
    if sys.platform != "linux" or os.environ.get("BILICOMICS_REMOTE_VERIFICATION") != "1":
        raise RuntimeError("Run physical recovery fixtures only through ssh test-env")


class PhysicalRecoveryTests(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory(prefix="bilicomics-physical-recovery-")
        self.addCleanup(directory.cleanup)
        self.output = Path(directory.name)
        self.process = subprocess.Popen(
            [sys.executable, "-c", "import sys; sys.stdin.readline()"],
            stdin=subprocess.PIPE, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        )
        self.addCleanup(self.close_process)
        stat = Path(f"/proc/{self.process.pid}/stat").read_text().rsplit(") ", 1)[1].split()
        self.birth = stat[19]
        self.executable = os.readlink(f"/proc/{self.process.pid}/exe")
        self.owned = f"{self.process.pid}|{self.birth}|{self.executable}|0|application\n".encode()
        self.done = {"recovery_complete": True}
        self.logs = {
            "statusbar-before.txt": b"statusbar start/running, process 123\n",
            "statusbar-after.txt": b"statusbar start/running, process 456\n",
            "native-baseline.tsv": b"awesome|789|41|/fixture/awesome|S\n",
            "awesome-after.tsv": b"789|41|/fixture/awesome|S\n",
            "owned-processes.tsv": self.owned,
        }
        self.snapshots = []

    def close_process(self):
        if self.process.poll() is None:
            self.process.communicate(b"finished\n", timeout=10)
        elif self.process.stdin is not None:
            self.process.stdin.close()

    def stopped_process(self):
        self.process.communicate(b"finished\n", timeout=10)

    def harness(self, content_mode="demo"):
        harness = physical.Harness.__new__(physical.Harness)
        harness.args = SimpleNamespace()
        harness.output = self.output
        harness.state_path = self.output / "run.json"
        harness.logs = "/fixture/device/logs"
        harness.state = {"phase": "running", "content_mode": content_mode,
                         "run_id": "recovery-fixture", "observations": []}
        harness.read = lambda path, required=True: self.logs.get(path.rsplit("/", 1)[-1], b"")
        harness.done = lambda required=False: dict(self.done)

        def remote(script, *, data=None, check=True, timeout=20):
            self.assertIsNone(data)
            if script.startswith("[ ! -e "):
                # The production lock pathname is never accessed on this host.
                return subprocess.CompletedProcess([], 0, stdout=b"", stderr=b"")
            self.assertIn("snapshot_owned()", script)
            self.assertIn(str(self.process.pid), script)
            self.assertIn(self.birth, script)
            self.assertNotIn("kill ", script)
            self.snapshots.append(script)
            return subprocess.run(["sh", "-c", script], capture_output=True,
                                  check=check, timeout=timeout)

        harness.remote = remote
        return harness

    def test_demo_restoration_includes_the_observed_owned_process_snapshot(self):
        self.stopped_process()
        harness = self.harness()
        result = harness.collect()
        self.assertEqual(len(self.snapshots), 1)
        self.assertTrue(result["recovery_checks"]["passed"])
        self.assertTrue(result["recovery_checks"]["owned_process_snapshot_passed"])
        saved = json.loads((self.output / "post-restore-processes.json").read_text())
        self.assertTrue(saved["passed"])
        self.assertEqual(saved["processes"][0]["pid"], self.process.pid)
        self.assertEqual(saved["processes"][0]["expected_start_ticks"], int(self.birth))
        self.assertIn(saved["processes"][0]["state"], ("gone", "pid-reused", "zombie", "dead"))
        self.assertFalse(saved["signals_sent"])
        self.assertFalse(saved["waited_for_zombies"])
        self.assertTrue(json.loads(harness.state_path.read_text())["recovery_checks"]["passed"])

    def test_demo_with_an_active_owned_process_is_not_ready_for_lease_release(self):
        harness = self.harness()
        result = harness.collect()
        self.assertEqual(len(self.snapshots), 1)
        self.assertFalse(result["recovery_checks"]["passed"])
        self.assertFalse(result["recovery_checks"]["owned_process_snapshot_passed"])
        observed = harness.state["post_restore_process_snapshot"]
        self.assertEqual(observed["processes"][0]["state"], "active-match")
        self.assertFalse(observed["signals_sent"])

    def test_distinct_fixture_keeps_the_same_real_snapshot_boundary(self):
        self.stopped_process()
        harness = self.harness("distinct_fixture")
        result = harness.collect()
        self.assertEqual(len(self.snapshots), 1)
        self.assertTrue(result["recovery_checks"]["owned_process_snapshot_passed"])
        self.assertTrue(result["recovery_checks"]["passed"])

    def test_incomplete_native_recovery_does_not_manufacture_process_closure(self):
        for failure in ("watchdog", "statusbar", "window-manager"):
            with self.subTest(failure=failure):
                self.snapshots.clear()
                self.done["recovery_complete"] = failure != "watchdog"
                self.logs["statusbar-after.txt"] = (
                    b"statusbar stop/waiting\n" if failure == "statusbar"
                    else b"statusbar start/running, process 456\n"
                )
                self.logs["awesome-after.tsv"] = (
                    b"789|41|/fixture/awesome|T\n" if failure == "window-manager"
                    else b"789|41|/fixture/awesome|S\n"
                )
                harness = self.harness()
                result = harness.collect()
                self.assertFalse(result["recovery_checks"]["passed"])
                self.assertNotIn("owned_process_snapshot_passed", result["recovery_checks"])
                self.assertEqual(self.snapshots, [])
                self.assertNotIn("post_restore_process_snapshot", harness.state)


class PhysicalMediaWorkerRecoveryTests(unittest.TestCase):
    FUNCTIONS = (
        "read_one", "now_seconds", "proc_fields", "proc_ticks", "proc_parent", "proc_group_session", "proc_state",
        "proc_exe", "has_token", "same_process", "signal_record", "recorded_observer_anchor",
        "recorded_signal_role", "recorded_observer_member", "recorded_application_parent", "recorded_ui_child",
        "signal_descendants", "recorded_media_worker", "process_birth_exited", "media_producers_gone", "wait_for_media_workers",
        "observer_group_has_media_worker", "stop_owned_observer_group", "publish_cleanup_result", "cleanup",
    )

    def setUp(self):
        directory = tempfile.TemporaryDirectory(prefix="bilicomics-media-recovery-")
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)
        self.logs = self.root / "logs"
        self.logs.mkdir()
        self.app = self.root / "app"
        self.app.mkdir()
        self.lock = self.root / "launcher.lock"
        self.lock.mkdir()
        (self.lock / "control").write_text("owned control must remain until recovery\n")
        self.receipt = self.root / "elf.receipt"
        self.receipt.write_text("owned verification receipt\n")
        self.token = "e" * 64
        self.owned = self.logs / "owned-processes.tsv"
        self.owned.write_text("")
        self.source = (Path(__file__).with_name("physical") / "physical-watchdog.sh").read_text()

    def function(self, name):
        lines = self.source.splitlines()
        start = next(index for index, line in enumerate(lines) if line.startswith(name + "() {"))
        end = start if lines[start].endswith(" }") else next(
            index for index in range(start + 1, len(lines)) if lines[index] == "}"
        )
        return "\n".join(lines[start:end + 1]) + "\n"

    def close_process(self, process):
        if process.stdin is not None:
            process.stdin.close()
            process.stdin = None
        process.wait(timeout=5)

    def process(self, relative="app/kindle-jpeg-worker", *, token=None):
        executable = self.root / relative
        executable.parent.mkdir(parents=True, exist_ok=True)
        # A copied host cat has a real, exact /proc executable identity and
        # exits naturally on stdin EOF. No codec or device node is involved.
        shutil.copyfile(Path(shutil.which("cat")).resolve(), executable)
        executable.chmod(0o700)
        environment = os.environ.copy()
        environment["BILICOMICS_PHYSICAL_TOKEN"] = self.token if token is None else token
        process = subprocess.Popen([str(executable)], stdin=subprocess.PIPE, stdout=subprocess.DEVNULL,
                                   stderr=subprocess.DEVNULL, env=environment, start_new_session=True)
        self.addCleanup(self.close_process, process)
        fields = Path(f"/proc/{process.pid}/stat").read_text().rsplit(") ", 1)[1].split()
        return process, fields[19], str(executable)

    def record(self, process, birth, executable, role="descendant", parent=None):
        with self.owned.open("a") as stream:
            stream.write(f"{process.pid}|{birth}|{executable}|{os.getpid() if parent is None else parent}|{role}\n")

    def close_forker(self, process):
        if process.poll() is None:
            process.stdin.write(b"exec\nfinish\n")
            process.stdin.flush()
            process.stdin.close()
            process.stdin = None
            process.wait(timeout=5)
        for stream in (process.stdout, process.stderr):
            if stream is not None:
                stream.close()

    def forking_application(self):
        executable = self.app / ("bilicomics-" + "a" * 64)
        shutil.copyfile(Path(sys.executable).resolve(), executable)
        executable.chmod(0o700)
        (self.logs / "binary-name").write_text(executable.name + "\n")
        worker = self.app / "kindle-jpeg-worker"
        shutil.copyfile(Path(shutil.which("cat")).resolve(), worker)
        worker.chmod(0o700)
        code = r'''
import os, sys, threading
gate_read, gate_write = os.pipe()
input_read, input_write = os.pipe()
def spawn():
    child = os.fork()
    if child == 0:
        os.close(gate_write)
        os.close(input_write)
        os.dup2(input_read, 0)
        os.close(input_read)
        os.read(gate_read, 1)
        os.close(gate_read)
        os.execv(sys.argv[1], [sys.argv[1]])
    os.close(gate_read)
    os.close(input_read)
    print(child, flush=True)
    os.waitpid(child, 0)
thread = threading.Thread(target=spawn)
thread.start()
try:
    for instruction in sys.stdin:
        if instruction.strip() == "exec" and gate_write is not None:
            os.write(gate_write, b"x")
            os.close(gate_write)
            gate_write = None
        if instruction.strip() == "finish":
            break
finally:
    if gate_write is not None:
        os.write(gate_write, b"x")
        os.close(gate_write)
    os.close(input_write)
    thread.join()
'''
        environment = os.environ.copy()
        environment["BILICOMICS_PHYSICAL_TOKEN"] = self.token
        process = subprocess.Popen([str(executable), "-S", "-c", code, str(worker)], stdin=subprocess.PIPE,
                                   stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=environment,
                                   start_new_session=True)
        self.addCleanup(self.close_forker, process)
        child = int(process.stdout.readline())
        birth = Path(f"/proc/{process.pid}/stat").read_text().rsplit(") ", 1)[1].split()[19]
        child_birth = Path(f"/proc/{child}/stat").read_text().rsplit(") ", 1)[1].split()[19]
        self.record(process, birth, str(executable), "application")
        return process, child, child_birth, str(executable), str(worker)

    def script(self, body, extra=""):
        globals_ = {
            "run_root": str(self.root), "logs": str(self.logs), "lock": str(self.lock), "token": self.token,
            "signal_log": str(self.root / "signals"), "operation_log": str(self.root / "operations"),
            "receipt": str(self.receipt), "launcher_pid": "", "launcher_ticks": "", "launcher_exe": "",
            "observer_anchor_pid": "", "observer_group_proven": "0", "cleanup_started": "0",
            "recovery_complete": "1", "media_worker_recovery_blocked": "0", "media_worker_wait_deadline": "",
            "reason": "operator-stop",
        }
        stubs = r'''
discover_children() { :; }
record_observer_group_members() { :; }
observer_group_target_matches() { return 1; }
launcher_child_matches() { return 1; }
restore_native() { printf 'restore-native\n' >>"$operation_log"; }
remove_owned_lock() { printf 'remove-lock\n' >>"$operation_log"; rm "$lock/control"; rmdir "$lock"; }
remove_verified_elf_receipt() { printf 'remove-receipt\n' >>"$operation_log"; rm "$receipt"; }
kill() { printf '%s\n' "$*" >>"$signal_log"; command kill "$@"; }
sleep() { command sleep 0.02; }
'''
        return "set -eu\n" + "".join(self.function(name) for name in self.FUNCTIONS) + \
            "".join(key + "=" + shlex.quote(value) + "\n" for key, value in globals_.items()) + stubs + extra + body

    def run_script(self, body, extra=""):
        result = subprocess.run(["sh", "-c", self.script(body, extra)], capture_output=True, text=True, timeout=15)
        self.assertEqual(result.returncode, 0, result.stderr)
        return result

    def assert_no_signals(self):
        self.assertFalse((self.root / "signals").exists(), "Recovery sent a signal to the protected worker")

    def assert_recovery_blocked(self):
        fields = (self.logs / "watchdog.done").read_text().strip().split("|")
        self.assertEqual(fields, [self.token, "media-worker-recovery-blocked", "0", "unknown"])
        self.assertTrue((self.lock / "control").is_file())
        self.assertTrue(self.receipt.is_file())
        self.assertFalse((self.root / "operations").exists(), "Blocked recovery performed native cleanup")
        self.assertEqual((self.logs / "launcher-lock-after.txt").read_text(), "present\n")
        self.assert_no_signals()

    def test_exact_owned_worker_is_never_sent_leaf_term_or_kill(self):
        process, birth, executable = self.process()
        self.record(process, birth, executable)
        self.run_script("signal_descendants TERM\nsignal_descendants KILL\n" +
                        f"signal_record {process.pid} {birth} {shlex.quote(executable)} TERM\n" +
                        f"signal_record {process.pid} {birth} {shlex.quote(executable)} KILL\n")
        self.assertIsNone(process.poll())
        self.assert_no_signals()
        log = (self.logs / "recovery.log").read_text()
        self.assertIn("not TERM", log)
        self.assertIn("not KILL", log)

    def test_worker_deadline_blocks_final_cleanup_and_preserves_control(self):
        process, birth, executable = self.process()
        self.record(process, birth, executable)
        self.run_script("media_worker_wait_deadline=$(now_seconds)\nif cleanup; then exit 90; fi\n")
        self.assertIsNone(process.poll())
        self.assert_recovery_blocked()
        self.assertIn("natural-exit deadline expired", (self.logs / "recovery.log").read_text())
        self.assertIn("token-matched", (self.logs / "recovery.log").read_text())

    def test_natural_worker_exit_allows_the_existing_native_cleanup(self):
        process, birth, executable = self.process()
        self.record(process, birth, executable)
        shell = subprocess.Popen(["sh", "-c", self.script("cleanup\n")], stdout=subprocess.PIPE,
                                 stderr=subprocess.PIPE, text=True)
        self.addCleanup(lambda: shell.wait(timeout=15))
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline:
            log = self.logs / "recovery.log"
            if log.exists() and "Waiting for exact media worker natural exit" in log.read_text():
                break
            self.assertIsNone(shell.poll(), "Cleanup finished before observing the live worker")
            time.sleep(0.01)
        else:
            self.fail("The production drain boundary did not observe the owned worker")
        self.close_process(process)
        stdout, stderr = shell.communicate(timeout=15)
        self.assertEqual(shell.returncode, 0, stdout + stderr)
        self.assert_no_signals()
        self.assertEqual((self.logs / "watchdog.done").read_text().strip().split("|")[2], "1")
        self.assertEqual((self.root / "operations").read_text().splitlines(),
                         ["restore-native", "remove-lock", "remove-receipt"])
        self.assertFalse(self.lock.exists())
        self.assertFalse(self.receipt.exists())

    def test_unproven_lineage_or_changed_token_cannot_bypass_recovery(self):
        for fault in ("unrecorded", "changed-token", "changed-birth", "unrecorded-token"):
            with self.subTest(fault=fault):
                process, birth, executable = self.process(
                    token="f" * 64 if fault in ("changed-token", "unrecorded-token") else None
                )
                if fault == "changed-token":
                    self.record(process, birth, executable)
                elif fault == "changed-birth":
                    self.record(process, str(int(birth) + 1), executable)
                self.run_script("if cleanup; then exit 90; fi\n")
                self.assertIsNone(process.poll())
                self.assert_recovery_blocked()
                self.assertIn("unproven", (self.logs / "recovery.log").read_text())
                self.close_process(process)
                for path in (self.logs / "watchdog.done", self.logs / "launcher-lock-after.txt"):
                    path.unlink()
                self.owned.write_text("")

    def test_producer_executable_or_token_drift_is_not_producer_exit(self):
        for fault in ("deleted-executable", "changed-token"):
            with self.subTest(fault=fault):
                process, birth, executable = self.process(
                    "app/producer-" + fault, token="f" * 64 if fault == "changed-token" else None
                )
                self.record(process, birth, executable, "application")
                if fault == "deleted-executable":
                    Path(executable).unlink()
                self.run_script("media_worker_wait_deadline=$(now_seconds)\nif cleanup; then exit 90; fi\n")
                self.assertIsNone(process.poll())
                self.assert_recovery_blocked()
                self.assertIn("Waiting for recorded media producer exit", (self.logs / "recovery.log").read_text())
                self.close_process(process)
                self.owned.write_text("")

    def test_recorded_worker_executable_drift_is_not_codec_drain(self):
        process, birth, executable = self.process()
        self.record(process, birth, executable)
        Path(executable).unlink()
        self.run_script("if cleanup; then exit 90; fi\n")
        self.assertIsNone(process.poll())
        self.assert_recovery_blocked()
        self.assertIn("identity is unproven", (self.logs / "recovery.log").read_text())

    def test_non_main_thread_fork_exec_child_never_acquires_signal_authority(self):
        process, child, child_birth, application, worker = self.forking_application()
        extra = "".join(self.function(name) for name in ("record_process", "known_parent", "discover_children")) + \
            "observe_owned_lock() { :; }\nobserve_observer_group() { :; }\n" + \
            "record_persistent_identity() { :; }\n" + f"launcher_pid={process.pid}\n"
        self.run_script("discover_children\nsignal_descendants TERM\nsignal_descendants KILL\n", extra)
        before = f"{child}|{child_birth}|{application}|{process.pid}|descendant"
        self.assertIn(before, self.owned.read_text().splitlines())
        self.assertIsNone(process.poll())
        self.assertEqual(os.readlink(f"/proc/{child}/exe"), application)
        self.assert_no_signals()
        process.stdin.write(b"exec\n")
        process.stdin.flush()
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline and os.readlink(f"/proc/{child}/exe") != worker:
            time.sleep(0.01)
        self.assertEqual(os.readlink(f"/proc/{child}/exe"), worker)
        self.run_script("discover_children\nsignal_descendants TERM\nsignal_descendants KILL\n" +
                        "media_worker_wait_deadline=$(now_seconds)\nif wait_for_media_workers; then exit 90; fi\n", extra)
        self.assertIn(f"{child}|{child_birth}|{worker}|{process.pid}|descendant", self.owned.read_text().splitlines())
        self.assert_no_signals()
        self.close_forker(process)
        self.run_script("wait_for_media_workers\n")

    def test_exact_worker_also_prevents_an_observer_group_kill(self):
        process, birth, executable = self.process()
        self.record(process, birth, executable)
        self.run_script("stop_owned_observer_group\n[ \"$recovery_complete\" -eq 0 ]\n",
                        f"observer_anchor_pid={process.pid}\nobserver_group_proven=1\n"
                        "observer_group_target_matches() { return 0; }\n")
        self.assertIsNone(process.poll())
        self.assert_no_signals()
        self.assertIn("Observer group signal withheld", (self.logs / "recovery.log").read_text())

    def test_other_path_with_the_same_basename_keeps_the_normal_signal_paths(self):
        for operation, signal_number in (("leaf", 15), ("group", 9)):
            with self.subTest(operation=operation):
                process, birth, executable = self.process("other-" + operation + "/kindle-jpeg-worker")
                self.record(process, birth, executable, "runner" if operation == "leaf" else "relay")
                if operation == "leaf":
                    self.run_script("signal_descendants TERM\n")
                else:
                    self.run_script("stop_owned_observer_group\n",
                                    f"observer_anchor_pid={process.pid}\nobserver_group_proven=1\n"
                                    "observer_group_target_matches() { return 0; }\n")
                self.assertEqual(process.wait(timeout=5), -signal_number)
                expected = f"-TERM {process.pid}" if operation == "leaf" else f"-KILL -{process.pid}"
                self.assertIn(expected, (self.root / "signals").read_text().splitlines())


if __name__ == "__main__":
    unittest.main()
