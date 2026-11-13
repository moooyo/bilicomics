"""Exercise production native-identity scripts in a real host shell and fake files.

Run only on test-env with BILICOMICS_REMOTE_VERIFICATION=1. These cases never
open a real proc record, device, framebuffer, transport, or shared lease.
"""

import hashlib
import os
from pathlib import Path
from subprocess import CompletedProcess, run
import sys
import tempfile
import unittest

import physical_device as physical


def setUpModule():
    if sys.platform != "linux" or os.environ.get("BILICOMICS_REMOTE_VERIFICATION") != "1":
        raise RuntimeError("Run native-identity shell contracts only through ssh test-env")


class NativeIdentityShellTests(unittest.TestCase):
    def setUp(self):
        self.workspace = tempfile.TemporaryDirectory(prefix="bilicomics-native-identity-shell-")
        self.addCleanup(self.workspace.cleanup)
        self.root = Path(self.workspace.name) / "fixture with spaces"
        self.proc = self.root / "proc"
        self.process = self.proc / "1001"
        self.lock = self.root / "lock"
        self.process.mkdir(parents=True)
        self.lock.mkdir()
        self.binary = self.root / "synthetic executable"
        self.binary.write_bytes(b"A non-executable host byte fixture\n")
        self.token, self.nonce = "a" * 64, "b" * 32
        self.stat = "1001 (bilicomics) " + " ".join(["S"] + ["0"] * 18 + ["1234"]) + "\n"
        self.status = b"Name:\tbilicomics\nSeccomp:\t2\nNoNewPrivs:\t0\nUid:\t0 0 0 0\nGid:\t0 0 0 0\n"
        self.arguments = b"/fixture/bilicomics\0--kindle\0--offline\0"
        self.seed()

    def seed(self):
        (self.process / "stat").write_text(self.stat)
        (self.process / "status").write_bytes(self.status)
        executable = self.process / "exe"
        if os.path.lexists(executable):
            executable.unlink()
        executable.symlink_to(self.binary)
        (self.process / "cmdline").write_bytes(self.arguments)
        (self.process / "environ").write_bytes(
            ("BILICOMICS_PHYSICAL_TOKEN=" + self.token + "\0BILICOMICS_LAUNCH_TOKEN=" + self.nonce + "\0").encode("ascii"))
        (self.lock / "generation").write_text("1:" + self.nonce + "\n")
        (self.lock / "nonce").write_text(self.nonce + "\n")
        (self.lock / "native-portrait-orientation").write_text("1:" + self.nonce + ":U:1\n")

    def execute(self, script):
        self.assertIn("\n", script)
        self.assertNotIn("\0", script)
        return run(["sh", "-eu", "-s"], input=script.encode("utf-8"), capture_output=True, timeout=5)

    def basic(self, fixture_mode=True):
        return physical.native_identity_basic_script("1001", self.token, fixture_mode, proc_root=str(self.proc))

    def extra(self):
        return physical.native_identity_extra_script("1001", proc_root=str(self.proc), lock_dir=str(self.lock))

    def assert_rejection(self, result, phase, stage, code):
        self.assertEqual(result.returncode, code)
        self.assertEqual(result.stderr, ("BILICOMICS_NATIVE_IDENTITY_STAGE=" + stage + "\n").encode("ascii"))
        with self.assertRaises(RuntimeError) as rejected:
            physical.native_identity_output(result, phase)
        self.assertIn(stage, str(rejected.exception))
        self.assertNotIn(self.token, str(rejected.exception))
        self.assertNotIn(self.nonce, str(rejected.exception))
        self.assertNotIn("/fixture/bilicomics", str(rejected.exception))

    def test_basic_success_preserves_the_original_three_line_protocol(self):
        result = self.execute(self.basic())
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stderr, b"")
        rows = result.stdout.decode("utf-8").splitlines()
        self.assertEqual(rows[:2], [self.stat.rstrip("\n"), str(self.binary)])
        self.assertEqual(len(rows), 3)
        self.assertEqual(rows[2].split()[0], hashlib.sha256(self.binary.read_bytes()).hexdigest())
        self.assertEqual(physical.native_identity_output(result, "basic"), result.stdout)

    def test_basic_read_token_and_required_flag_failures_have_distinct_stages(self):
        cases = (("stat", "basic.stat_read", 41), ("exe", "basic.exe_read", 42),
                 ("digest", "basic.digest_read", 43), ("environ", "basic.environ_read", 44),
                 ("token", "basic.physical_token", 45), ("cmdline", "basic.cmdline_read", 46),
                 ("demo", "basic.flag_demo", 47), ("offline", "basic.flag_offline", 48),
                 ("kindle", "basic.flag_kindle", 49))
        for fault, stage, code in cases:
            with self.subTest(fault=fault):
                self.seed()
                if fault in ("stat", "exe", "environ", "cmdline"):
                    (self.process / fault).unlink()
                elif fault == "digest":
                    (self.process / "exe").unlink()
                    (self.process / "exe").symlink_to(self.root / "missing executable")
                elif fault == "token":
                    (self.process / "environ").write_bytes(b"BILICOMICS_PHYSICAL_TOKEN=foreign\0")
                elif fault in ("offline", "kindle"):
                    (self.process / "cmdline").write_bytes(self.arguments.replace(("--" + fault + "\0").encode(), b""))
                self.assert_rejection(self.execute(self.basic(fault != "demo")), "basic", stage, code)

    def test_extra_success_preserves_binary_delimiters_and_owned_portrait(self):
        for direction in ("U", "D", "FIXED"):
            for rotation in range(4):
                with self.subTest(direction=direction, rotation=rotation):
                    payload = f"{direction}:{rotation}\n".encode("ascii")
                    (self.lock / "native-portrait-orientation").write_bytes(
                        ("1:" + self.nonce + ":").encode("ascii") + payload)
                    result = self.execute(self.extra())
                    self.assertEqual(result.returncode, 0, result.stderr)
                    self.assertEqual(result.stderr, b"")
                    self.assertEqual(result.stdout, self.status + b"\0" + self.arguments + b"\0" + payload)
                    status, tail = result.stdout.split(b"\0", 1)
                    arguments, portrait = tail.rsplit(b"\0\0", 1)
                    self.assertEqual(status, self.status)
                    self.assertEqual(arguments, self.arguments[:-1])
                    self.assertEqual(portrait, payload)

    def test_extra_read_generation_portrait_and_launch_token_failures_remain_rejections(self):
        cases = (("status", "extra.status_read", 61), ("cmdline", "extra.cmdline_read", 63),
                 ("generation", "extra.generation_read", 65), ("nonce", "extra.nonce_read", 66),
                 ("native-portrait-orientation", "extra.portrait_read", 67),
                 ("wrong-generation", "extra.generation_binding", 68),
                 ("wrong-portrait", "extra.portrait_binding", 69),
                 ("environ", "extra.environ_read", 70), ("wrong-token", "extra.launch_token_binding", 71))
        for fault, stage, code in cases:
            with self.subTest(fault=fault):
                self.seed()
                if fault in ("status", "cmdline", "environ"):
                    (self.process / fault).unlink()
                elif fault in ("generation", "nonce", "native-portrait-orientation"):
                    (self.lock / fault).unlink()
                elif fault == "wrong-generation":
                    (self.lock / "generation").write_text("2:" + self.nonce + "\n")
                elif fault == "wrong-portrait":
                    (self.lock / "native-portrait-orientation").write_text("1:" + self.nonce + ":L:1\n")
                elif fault == "wrong-token":
                    (self.process / "environ").write_bytes(b"BILICOMICS_LAUNCH_TOKEN=foreign\0")
                self.assert_rejection(self.execute(self.extra()), "extra", stage, code)

    def test_portrait_records_reject_foreign_context_noncanonical_codes_and_resume_schema(self):
        prefix = "1:" + self.nonce + ":"
        for payload in ("U:4", "D:01", "D:1:extra", "FIXED:-1", "3:CALIBRATED", "U", "UNKNOWN:1"):
            with self.subTest(payload=payload):
                self.seed()
                (self.lock / "native-portrait-orientation").write_text(prefix + payload + "\n")
                self.assert_rejection(self.execute(self.extra()), "extra", "extra.portrait_binding", 69)
        for record in ("2:" + self.nonce + ":D:1\n", "1:" + "c" * 32 + ":D:1\n"):
            with self.subTest(record=record):
                self.seed()
                (self.lock / "native-portrait-orientation").write_text(record)
                self.assert_rejection(self.execute(self.extra()), "extra", "extra.portrait_binding", 69)
        for ending in ("", "\n\n"):
            with self.subTest(ending=ending):
                self.seed()
                (self.lock / "native-portrait-orientation").write_text(prefix + "D:1" + ending)
                self.assert_rejection(self.execute(self.extra()), "extra", "extra.portrait_framing", 73)

    def test_transport_or_untrusted_stderr_never_exposes_process_data(self):
        secret = b"private-token-or-raw-environment"
        for stderr in (secret, b"BILICOMICS_NATIVE_IDENTITY_STAGE=untrusted." + secret + b"\n",
                       b"BILICOMICS_NATIVE_IDENTITY_STAGE=basic.physical_token\n" + secret):
            with self.subTest(stderr=stderr), self.assertRaises(RuntimeError) as rejected:
                physical.native_identity_output(CompletedProcess([], 1, secret, stderr), "extra")
            self.assertNotIn(secret.decode(), str(rejected.exception))
            self.assertIn("extra.transport_or_unlabelled", str(rejected.exception))


if __name__ == "__main__":
    unittest.main()
