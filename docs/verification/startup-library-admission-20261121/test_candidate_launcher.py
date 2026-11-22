"""Regression cases for the archived worker-only launcher, never the shipping default."""

from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest import mock


HERE = Path(__file__).resolve().parent
sys.path[:0] = [str(HERE), str(HERE.parents[2] / 'packaging/tests')]
import launcher_test
import native_series


class WorkerOnlyLauncherTests(launcher_test.LauncherLifecycleTests):
    def setUp(self):
        directory = tempfile.TemporaryDirectory(prefix='bilicomics-worker-only-')
        self.addCleanup(directory.cleanup)
        candidate = Path(directory.name) / 'launch.sh'
        candidate.write_bytes(native_series.edited_stock(launcher_test.SOURCE.read_bytes()))
        replacement = mock.patch.object(launcher_test, 'SOURCE', candidate)
        replacement.start()
        self.addCleanup(replacement.stop)
        super().setUp()

    def test_library_preflights_before_the_display_lock(self):
        self.assert_library_preflights(['--detached'])

    def test_library_worker_reports_the_current_preflight_refusal_without_takeover(self):
        self.env.update(BILICOMICS_APPROVED_PROFILE="pw6", BILICOMICS_APPROVED_FIRMWARE="5.20.1")
        wrapper = self.bin / "setsid"
        anchor = "  record_entry_worker\n"
        contents = wrapper.read_text()
        self.assertEqual(contents.count(anchor), 1)
        wrapper.write_text(contents.replace(anchor, anchor + '  printf "pw6 0\\n" >"$BILICOMICS_HOME/data/device-allowlist"\n'))
        status, output, error = self.run_launcher(admission="--library")
        self.assertEqual(status, 1, error)
        self.assertEqual(output, "")
        self.assertEqual(error, "bilicomics: The configured profile/firmware combination has not been accepted: pw6 5.20.1.\n")
        self.assertEqual(self.starts(), [])
        self.assertEqual(self.trace(), [])
        self.assertFalse((self.home / "data" / "logs").exists())
        self.assertFalse((self.root / "lock").exists())

    def test_library_preflight_diagnostics_end_before_worker_lock_errors(self):
        lock = self.root / "lock"
        lock.mkdir()
        probe = self.root / "preflight-channel"
        anchor = 'mkdir "$LOCK_DIR" 2>/dev/null || fail'
        source = self.launcher.read_text()
        self.assertEqual(source.count(anchor), 1)
        self.launcher.write_text(source.replace(anchor,
            'if [ -e /proc/self/fd/8 ] || [ -n "${BILICOMICS_LIBRARY_PREFLIGHT_STDERR:-}" ]; then\n'
            f'    printf "leaked\\n" >"{probe}"\nelse\n    printf "closed\\n" >"{probe}"\nfi\n' + anchor))
        status, output, error = self.run_launcher(admission="--library")
        self.assertEqual((status, output, error), (1, "", ""))
        self.assertEqual(probe.read_text(), "closed\n")
        self.assertEqual(list(lock.iterdir()), [])
        self.assertEqual(self.starts(), [])
        self.assertEqual(self.trace(), [])

    def test_library_with_closed_stderr_still_starts_its_independent_worker(self):
        result = subprocess.run(["sh", "-c", 'exec 2>&-; exec sh "$1" --library', "sh", str(self.launcher)],
                                env=dict(self.env, TEST_ADMISSION_KEY="closed-stderr"), capture_output=True, text=True, timeout=18)
        self.assertEqual((result.returncode, result.stdout, result.stderr), (0, "", ""))
        self.assertEqual(len(self.starts()), 1)
        self.assertEqual(self.trace(), ["stop:statusbar", "start:statusbar", "xrefresh:"])
        self.assertFalse((self.root / "lock").exists())

    def test_library_preflight_error_reaches_an_open_unlinked_stderr(self):
        (self.home / "data" / "device-profile").unlink()
        sink = self.root / "stderr"
        with sink.open("x+b") as output:
            sink.unlink()
            result = subprocess.run(["sh", str(self.launcher), "--library"],
                                    env=dict(self.env, TEST_ADMISSION_KEY="open-unlinked-stderr"),
                                    stdout=subprocess.PIPE, stderr=output, timeout=6)
            output.seek(0)
            self.assertEqual(output.read(), b"bilicomics: Configure data/device-profile before launching.\n")
        self.assertEqual((result.returncode, result.stdout), (1, b""))
        self.assertEqual(self.starts(), [])
        self.assertFalse((self.root / "lock").exists())

    def test_direct_worker_keeps_an_unrelated_caller_descriptor(self):
        sink = self.root / "unrelated-output"
        anchor = 'mkdir "$LOCK_DIR" 2>/dev/null || fail'
        source = self.launcher.read_text()
        self.assertEqual(source.count(anchor), 1)
        self.launcher.write_text(source.replace(anchor, 'printf "preserved\\n" >&8\n' + anchor))
        (self.root / "lock").mkdir()
        result = subprocess.run(["sh", "-c", 'exec 8>"$2"; exec sh "$1" --detached', "sh", str(self.launcher), str(sink)],
                                env=self.env, capture_output=True, text=True, timeout=6)
        self.assertEqual(result.returncode, 1)
        self.assertEqual(result.stderr, "bilicomics: Another launcher owns the display lock.\n")
        self.assertEqual(sink.read_text(), "preserved\n")
        self.assertEqual(self.starts(), [])


def load_tests(loader, tests, pattern):
    return unittest.TestSuite(WorkerOnlyLauncherTests(name) for name in WorkerOnlyLauncherTests.__dict__ if name.startswith('test_'))


if __name__ == '__main__':
    unittest.main()
