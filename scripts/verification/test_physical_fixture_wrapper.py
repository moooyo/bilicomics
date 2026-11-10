"""Exercise only the opt-in fixture wrapper routing on the Linux test host.

The guard here is a recorded exec stub, not a seccomp implementation. These
tests prove argument routing, same-PID exec and fail-closed wrapper behavior.
The real guard's socket isolation is covered by its separate component tests.
No Kindle or shared device lease is used. Importing the existing helper module
does not rediscover or rerun its original physical harness test cases.
"""

import hashlib
import json
import subprocess
import time
import unittest

import test_physical_harness as _harness


class DistinctFixtureWrapperTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        # This helper enforces Linux plus BILICOMICS_REMOTE_VERIFICATION=1.
        _harness.PhysicalHarnessTests.setUpClass()

    @classmethod
    def tearDownClass(cls):
        _harness.PhysicalHarnessTests.tearDownClass()

    def fixture(self):
        fixture = _harness.PhysicalHarnessTests("test_relay_entry_preserves_native_identity_and_arguments")
        self.addCleanup(fixture.doCleanups)
        fixture.setUp()
        return fixture

    def configure_guard(self, fixture, *, reject=False):
        (fixture.logs / "content-mode").write_text("distinct_fixture\n")
        (fixture.logs / "performance-trace-enabled").write_text("1\n")
        fixture.environment["TEST_GUARD_RECORD"] = str(fixture.root / "guard-record")
        fixture.environment["TEST_GUARD_ARGUMENTS"] = str(fixture.root / "guard-arguments")
        fixture.environment["TEST_GUARD_PROOF_PATH"] = str(fixture.logs / "ui-socket-guard.json")
        temporary = fixture.run_root / "app/guard-stub"
        fixture.write_script(temporary,
            'printf "%s\\n" "$$" >>"$TEST_GUARD_RECORD"\n'
            'printf "%s\\n" "$@" >"$TEST_GUARD_ARGUMENTS"\n'
            '[ "$1" = --proof-file ] && [ "$2" = "$TEST_GUARD_PROOF_PATH" ]\n'
            'proof_path=$2\nshift 2\n[ "$1" = -- ]\nshift\n' +
            ('exit 125' if reject else
             'printf \'{"stub_only":true,"process_id":%s}\\n\' "$$" >"$proof_path"\nexec "$@"'))
        digest = hashlib.sha256(temporary.read_bytes()).hexdigest()
        guard = temporary.with_name("offline-socket-guard-" + digest)
        temporary.rename(guard)
        fixture_digest = "f" * 64
        (fixture.logs / "socket-guard-name").write_text(guard.name + "\n")
        (fixture.logs / "socket-guard-sha256").write_text(digest + "\n")
        (fixture.logs / "fixture-sha256").write_text(fixture_digest + "\n")
        (fixture.logs / "socket-guard-approved").write_text(fixture.token + "|" + digest + "|" + fixture_digest + "\n")
        return guard

    def assert_rejected_before_ui(self, fixture, *, guard_invoked):
        fixture.watchdog_log = (fixture.root / "watchdog-output.log").open("w")
        fixture.watchdog = subprocess.Popen(
            ["sh", str(fixture.run_root / "physical-watchdog.sh"), str(fixture.run_root), fixture.token, "30", "90"],
            env=fixture.environment, stdout=fixture.watchdog_log, stderr=subprocess.STDOUT, start_new_session=True)
        fixture.watchdog_identity = _harness.identity(fixture.watchdog.pid)
        deadline = time.monotonic() + 40
        while not (fixture.logs / "watchdog.done").exists() and time.monotonic() < deadline:
            fixture.remember_observer_if_owned()
            time.sleep(0.03)
        self.assertTrue((fixture.logs / "watchdog.done").is_file(),
                        "The rejected fixture UI did not finish its bounded native recovery")
        fields = (fixture.logs / "watchdog.done").read_text().strip().split("|")
        self.assertEqual(len(fields), 4)
        self.assertEqual(fields[0], fixture.token)
        self.assertEqual(fields[2], "1", "Rejecting the guard did not restore native control")
        fixture.watchdog.wait(timeout=5)
        self.assertEqual(fixture.watchdog.returncode, 0)
        executions = (fixture.root / "executions").read_text().splitlines()
        self.assertTrue(any(line.startswith("native-capture:") for line in executions),
                        "The isolated launcher never reached its real native-helper startup boundary")
        self.assertFalse(any(line.startswith("application:") for line in executions),
                         "The rejected guard fell back to an unguarded or demonstration UI")
        record = fixture.root / "guard-record"
        self.assertEqual(record.exists(), guard_invoked)
        if guard_invoked:
            self.assertEqual(len(record.read_text().splitlines()), 1)
        self.assertFalse(fixture.lock.exists(), "Rejection retained the owned launcher lock")
        self.assertIn("start/running", (fixture.root / "statusbar-state").read_text())
        for child, before, stopped in fixture.native_baseline:
            after = _harness.identity(child.pid)
            self.assertIsNotNone(after)
            self.assertEqual(after["birth"], before["birth"])
            self.assertEqual(after["state"] in ("T", "t"), stopped)
        fixture.completed = True

    def test_ui_uses_guard_once_and_native_helpers_bypass_it(self):
        fixture = self.fixture()
        self.configure_guard(fixture)
        fixture.start_fixture()
        application_args = (fixture.arguments / f"application-{fixture.application['pid']}").read_text().splitlines()
        self.assertEqual((fixture.root / "guard-record").read_text().splitlines(), [str(fixture.application["pid"])])
        self.assertEqual((fixture.root / "guard-arguments").read_text().splitlines(),
                         ["--proof-file", str(fixture.logs / "ui-socket-guard.json"), "--", *application_args])
        self.assertEqual(json.loads((fixture.logs / "ui-socket-guard.json").read_text()),
                         {"stub_only": True, "process_id": fixture.application["pid"]})
        self.assertNotIn("--demo", application_args)
        self.assertNotIn("--actions", application_args)
        self.assertEqual(application_args.count("--offline"), 1)
        self.assertEqual(application_args.count("--data-dir"), 1)
        index = application_args.index("--data-dir")
        self.assertEqual(application_args[index + 1], str(fixture.run_root / "data"))
        self.assertEqual(application_args.count("--performance-trace"), 1)
        executions = (fixture.root / "executions").read_text().splitlines()
        expected = {
            "relay": [str(fixture.fixture_binary), "--relay-power-events"],
            "native-capture": [str(fixture.fixture_binary), "--capture-native-display", "--kindle", "--device", "scribe"],
            "portrait-publish": [str(fixture.fixture_binary), "--publish-native-portrait-orientation",
                                 "FIXED:3", "--kindle", "--device", "scribe"],
        }
        for role, arguments in expected.items():
            pids = [int(line.split(":", 1)[1]) for line in executions if line.startswith(role + ":")]
            self.assertEqual(len(pids), 1, "A typed native helper was missing or duplicated")
            self.assertNotEqual(pids[0], fixture.application["pid"])
            self.assertEqual((fixture.arguments / f"{role}-{pids[0]}").read_text().splitlines(), arguments)
        fixture.finish_fixture()

    def test_missing_or_changed_guard_is_rejected_without_fallback(self):
        for fault in ("missing", "changed-digest"):
            with self.subTest(fault=fault):
                fixture = self.fixture()
                guard = self.configure_guard(fixture)
                if fault == "missing":
                    guard.unlink()
                else:
                    with guard.open("a") as stream:
                        stream.write("\n# Changed after the approved digest was recorded.\n")
                self.assert_rejected_before_ui(fixture, guard_invoked=False)

    def test_guard_exec_failure_never_falls_back_to_demo(self):
        fixture = self.fixture()
        self.configure_guard(fixture, reject=True)
        self.assert_rejected_before_ui(fixture, guard_invoked=True)


if __name__ == "__main__":
    unittest.main(verbosity=2)
