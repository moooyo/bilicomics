import io
import os
from pathlib import Path
import subprocess
import tarfile
import tempfile
import unittest

from project_launcher_stages import MAX_TRACE, SEQUENCE, TRACE, project


ROOT = Path(__file__).resolve().parents[3]
CONTROL = ROOT / "docs/physical/startup-optimization-20261119/resume/rebuilt/device-pair.sh"


class StageTests(unittest.TestCase):
    def setUp(self):
        scratch = tempfile.TemporaryDirectory()
        self.addCleanup(scratch.cleanup)
        self.work = Path(scratch.name)
        self.run = "lib-p-ab123456"
        self.stage = self.work / "media" / ("bilicomics-" + self.run)
        self.private = self.work / "private" / ("bilicomics-" + self.run)
        for root in (self.stage, self.private):
            root.mkdir(parents=True)
            (root / "owner").write_text(self.run + "\n")
        self.case = self.private / "a01"
        self.case.mkdir()
        self.leaves = {"entry.pid", "worker.pid", "entry.status", "performance.jsonl",
                       "lifecycle.jsonl", "final.json", "final.png", "guard-done"}
        for leaf in self.leaves:
            (self.case / leaf).touch()
        (self.case / "worker.pid").write_text("812 112345\n")
        self.logs = self.stage / "cases/a01/data/logs"
        self.logs.mkdir(parents=True)
        self.raw = self.logs / "launcher-timing.tsv"
        self.control = self.work / "control.sh"
        self.control.write_text(CONTROL.read_text().replace("/mnt/us", str(self.stage.parent))
                                .replace("/var/tmp", str(self.private.parent)))
        self.lines = [
            f"123.{index:02}\t{event}\tpid=812\tphase=none\tsleep=0\tcapture=0\t"
            f"probe=0\tlifecycle=0\tmode={'Y8' if index >= 5 else 'unknown'}\tread_sequence={sequence}\n"
            for index, (event, sequence) in enumerate(SEQUENCE)
        ]
        self.raw.write_text("".join(self.lines))

    def collect(self, op="collect-stages", ok=True):
        result = subprocess.run(["sh", self.control, self.run, op, "a01"], capture_output=True, timeout=5)
        self.assertEqual(result.returncode == 0, ok, result.stderr)
        if not ok:
            self.assertEqual(result.stdout, b"")
            return
        with tarfile.open(fileobj=io.BytesIO(result.stdout)) as archive:
            self.assertEqual(set(archive.getnames()), self.leaves if op == "collect" else self.leaves | {TRACE})
            if op != "collect":
                return archive.extractfile(TRACE).read()

    def test_sanitized_stages_from_closed_stream(self):
        self.lines.insert(-1, self.lines[-1].replace("native_take_done", "awesome_paused"))
        self.raw.write_text("private fixture header\n" + "".join(self.lines) + "unused-private-trailer\n")
        data = self.collect()
        self.assertNotIn(b"private", data)
        self.assertNotIn(b"awesome_paused", data)
        self.assertNotIn(b"mode=", data)
        self.assertEqual(data.splitlines()[0], b"123.00\tstartup_logging_ready\t812\t0")
        self.assertFalse((self.case / TRACE).exists())
        (self.case / TRACE).write_bytes(data)
        report = project(self.case)
        self.assertEqual(report["interval_ms"], {"logging_ready_to_sample_begin": 10,
                         "sample_begin_to_sample_ready": 40, "sample_ready_to_take_begin": 70,
                         "take_begin_to_take_done": 10})
        self.assertEqual(report["nested_read_ms"]["display_sample"], {"go_to_ready": 10, "ready_to_reaped": 10})

    def test_non_numeric_source_fields_and_incomplete_records_never_leave_the_worker(self):
        for old, new in (("read_sequence=1", "read_sequence=private"), ("pid=812", "pid=13"),
                         ("phase=none", "phase=private"), ("sleep=0", "sleep=1"), ("mode=unknown", "mode=private"),
                         ("native_read_ready", "private"), ("native_take_done", "other_event")):
            with self.subTest(field=old):
                self.raw.write_text("".join(self.lines).replace(old, new))
                self.collect(ok=False)
                self.assertFalse((self.case / TRACE).exists())

    def test_only_bounded_regular_files_in_case_directory(self):
        private_file = self.work / "secret"
        private_file.write_text("not a timing log")
        self.raw.unlink()
        self.collect(ok=False)
        self.raw.symlink_to(private_file)
        self.collect(ok=False)
        self.raw.unlink()
        os.mkfifo(self.raw)
        self.collect(ok=False)
        self.raw.unlink()
        self.raw.write_bytes(b"x" * 65537)
        self.collect(ok=False)
        self.raw.write_text("".join(self.lines))
        logs_backup = self.logs.with_name("elsewhere")
        self.logs.rename(logs_backup)
        self.logs.symlink_to(logs_backup, target_is_directory=True)
        self.collect(ok=False)
        self.assertEqual(private_file.read_text(), "not a timing log")

    def test_existing_destination_or_invalid_identity_never_overwritten(self):
        destination = self.case / TRACE
        destination.write_text("preserve")
        self.collect(ok=False)
        self.assertEqual(destination.read_text(), "preserve")
        destination.unlink()
        (self.case / "worker.pid").write_text("812 12 extra\n")
        self.collect(ok=False)

    def test_closed_host_case_rejects_reordering_wrong_pid_sequence_and_clock(self):
        rows = self.collect().splitlines(keepends=True)
        for change in (rows[:1] + rows[2:], rows[:5] + [rows[5], rows[5]] + rows[6:],
                       rows[:2] + [rows[3], rows[2]] + rows[4:],
                       rows[:1] + [rows[1].replace(b"\t812\t", b"\t101\t")] + rows[2:],
                       rows[:3] + [rows[3].replace(b"\t1\n", b"\t2\n")] + rows[4:],
                       rows[:3] + [rows[3].replace(b"123.03", b"122.00")] + rows[4:],
                       rows[:-1] + [rows[-1].rstrip()]):
            with self.subTest():
                (self.case / TRACE).write_bytes(b"".join(change))
                with self.assertRaises(ValueError):
                    project(self.case)
        (self.case / TRACE).write_bytes(b"x" * (MAX_TRACE + 1))
        with self.assertRaises(ValueError):
            project(self.case)

    def test_older_collect_has_no_launcher_stage_requirement(self):
        self.raw.unlink()
        self.collect("collect")
        with self.assertRaises(FileNotFoundError):
            project(self.case)


if __name__ == "__main__":
    unittest.main()
