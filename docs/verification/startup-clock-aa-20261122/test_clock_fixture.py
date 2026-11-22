"""Exercise the marker on Linux using prebuilt BILI_CLOCK_MARKS_HOST.

Set BILI_CLOCK_MARKS_ARM and BILI_CLOCK_MARKS_QEMU for the optional ARM
user-mode check. No firmware, framebuffer, network, or Kindle is required.
"""

import json
import os
from pathlib import Path
import stat
import subprocess
import tempfile
import time
import unittest


RUN = "bili-shelf-0123456789abcdef0123456789abcdef"
STAGES = {"shim_entry": "shim-entry.json", "app_preinvoke": "app-preinvoke.json"}


class ClockFixture(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name).resolve()
        self.picked = self.root / RUN / "picked"
        self.picked.mkdir(parents=True, mode=0o700)
        self.helper = Path(os.environ["BILI_CLOCK_MARKS_HOST"]).resolve(strict=True)

    def arguments(self, stage, *command):
        return [RUN, stage, self.picked / STAGES[stage], "--", *command]

    def examine(self, stage, process_id, before, after):
        record = (self.picked / STAGES[stage]).read_bytes()
        parsed = json.loads(record)
        self.assertIs(type(parsed["monotonic_ns"]), int)
        self.assertLessEqual(before, parsed["monotonic_ns"])
        self.assertLessEqual(parsed["monotonic_ns"], after)
        expected = {"schema": "bili-startup-clock-mark-v1", "run_id": RUN,
                    "clock": "CLOCK_MONOTONIC", "stage": stage, "pid": process_id,
                    "monotonic_ns": parsed["monotonic_ns"]}
        self.assertEqual(record, (json.dumps(expected, separators=(",", ":")) + "\n").encode("ascii"))
        self.assertEqual(stat.S_IMODE((self.picked / STAGES[stage]).stat().st_mode), 0o600)

    def test_same_process_preserves_argv_environment_descriptors_and_exit(self):
        for stage in STAGES:
            with self.subTest(stage=stage), tempfile.TemporaryFile() as inherited:
                inherited.write(b"fixture fd\n")
                inherited.seek(0)
                environment = {**os.environ, "FIXTURE_PAYLOAD": "fixture env",
                               "FIXTURE_FD": str(inherited.fileno())}
                command = ("/bin/sh", "-c", 'IFS= read -r value <&"$FIXTURE_FD" || exit 89; '
                           'printf "%s|%s|%s|%s|%s\\n" "$$" "$0" "$1" "$FIXTURE_PAYLOAD" "$value"; exit 23',
                           "fixture argv zero", "literal ; * argument")
                before = time.monotonic_ns()
                with subprocess.Popen([self.helper, *self.arguments(stage, *command)], env=environment,
                                      pass_fds=(inherited.fileno(),), stdout=subprocess.PIPE,
                                      stderr=subprocess.PIPE, umask=0o077) as child:
                    output, errors = child.communicate(timeout=5)
                    self.assertEqual(child.returncode, 23)
                    self.assertEqual(output, f"{child.pid}|fixture argv zero|literal ; * argument|fixture env|fixture fd\n".encode())
                    self.assertEqual(errors, b"")
                    self.examine(stage, child.pid, before, time.monotonic_ns())

    def test_nonexclusive_or_unsafe_record_does_not_change_command_exit(self):
        stage = "shim_entry"
        record = self.picked / STAGES[stage]
        foreign = self.root / "foreign-record"
        foreign.write_bytes(b"foreign")
        args = [self.helper, *self.arguments(stage, "/bin/sh", "-c", "exit 42")]
        record.write_bytes(b"existing")
        original = record.stat()
        result = subprocess.run(args, capture_output=True, timeout=5)
        self.assertEqual(result.returncode, 42)
        self.assertEqual(result.stderr, b"clock-marks: diagnostic mark unavailable\n")
        self.assertEqual(record.read_bytes(), b"existing")
        self.assertEqual(record.stat().st_ino, original.st_ino)

        record.unlink()
        record.symlink_to(foreign)
        result = subprocess.run(args, capture_output=True, timeout=5)
        self.assertEqual(result.returncode, 42)
        self.assertEqual(result.stderr, b"clock-marks: diagnostic mark unavailable\n")
        self.assertEqual(foreign.read_bytes(), b"foreign")

    def test_record_directory_cannot_be_a_symlink(self):
        self.picked.rmdir()
        self.picked.symlink_to(self.root, target_is_directory=True)
        stage = "app_preinvoke"
        result = subprocess.run([self.helper, *self.arguments(stage, "/bin/sh", "-c", "exit 17")],
                                capture_output=True, timeout=5)
        self.assertEqual(result.returncode, 17)
        self.assertEqual(result.stderr, b"clock-marks: diagnostic mark unavailable\n")
        self.assertFalse((self.root / STAGES[stage]).exists())

    def test_malformed_bindings_do_not_run_the_command(self):
        canonical = self.arguments("shim_entry", "/bin/sh", "-c", "printf proceeded")
        changes = {0: RUN.upper(), 1: "arbitrary", 2: self.picked / "app-preinvoke.json",
                   3: "--invalid", 4: "sh"}
        for index, value in changes.items():
            with self.subTest(index=index):
                args = canonical.copy()
                args[index] = value
                result = subprocess.run([self.helper, *args], capture_output=True, timeout=5)
                self.assertEqual(result.returncode, 64)
                self.assertEqual(result.stdout, b"")
                self.assertEqual(result.stderr, b"clock-marks: invalid diagnostic invocation\n")
        self.assertEqual(list(self.picked.iterdir()), [])

    @unittest.skipUnless(os.environ.get("BILI_CLOCK_MARKS_ARM") and os.environ.get("BILI_CLOCK_MARKS_QEMU"),
                         "ARM helper and QEMU not supplied")
    def test_arm_user_mode_uses_the_host_monotonic_domain(self):
        helper = Path(os.environ["BILI_CLOCK_MARKS_ARM"]).resolve(strict=True)
        qemu = Path(os.environ["BILI_CLOCK_MARKS_QEMU"]).resolve(strict=True)
        stage = "app_preinvoke"
        before = time.monotonic_ns()
        with subprocess.Popen([qemu, helper, *self.arguments(stage, self.root / "absent")],
                              stdout=subprocess.PIPE, stderr=subprocess.PIPE, umask=0o077) as child:
            output, errors = child.communicate(timeout=5)
            self.assertEqual(child.returncode, 127)
            self.assertEqual(output, b"")
            self.assertEqual(errors, b"clock-marks: cannot execute original command\n")
            self.examine(stage, child.pid, before, time.monotonic_ns())


if __name__ == "__main__":
    unittest.main()
