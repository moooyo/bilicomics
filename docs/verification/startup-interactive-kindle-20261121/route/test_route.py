"""Rehearse the same route state machine in a disposable Linux directory."""

import fcntl
import importlib.util
import os
from pathlib import Path
import shlex
import shutil
import subprocess
import tempfile
import unittest


HERE = Path(__file__).resolve().parent
REPO = HERE.parents[3]
SPEC = importlib.util.spec_from_file_location("prepare_route", HERE / "prepare_route.py")
PREPARE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(PREPARE)
RUN = "bili-shelf-" + "a" * 32


class RouteTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.temporary = tempfile.TemporaryDirectory(prefix="bili-route-rehearsal-")
        cls.addClassCleanup(cls.temporary.cleanup)
        cls.root = Path(cls.temporary.name).resolve()
        cls.media = cls.root / "media"
        cls.tool = cls.root / "native-route-fs"
        subprocess.run([os.environ.get("CC", "cc"), "-std=c11", "-O2", "-Wall", "-Wextra", "-Werror",
                        f'-DROUTE_MEDIA="{cls.media}"', str(HERE / "route-fs.c"), "-o", str(cls.tool)], check=True)

    def setUp(self):
        self.addCleanup(self.clean_fixture)
        self.home = self.media / "bilicomics"
        self.stage = self.media / RUN
        self.doc = self.media / "documents/bilicomics.sh"
        self.stock = self.home / "launch.sh"
        self.ready = self.stage / "entry-ready.sh"
        self.retained = self.stage / "retained-launch.sh"
        self.journal = self.stage / "prepared"
        self.pending = self.stage / "prepared.pending"
        self.armed = self.stage / "armed"
        self.picked = self.stage / "picked"
        for directory in (self.home / "app", self.home / "data", self.stage / "app", self.stage / "data/demo",
                          self.doc.parent, self.root / "bin", self.root / "system"):
            directory.mkdir(parents=True, exist_ok=True)
        (self.root / "system/firmware").write_text("Kindle 5.19.5\n", encoding="ascii")
        (self.root / "system/bpp").write_text("8\n", encoding="ascii")
        (self.root / "system/size").write_text("1872,4960\n", encoding="ascii")
        (self.root / "system/framebuffer").touch()
        self.shim = PREPARE.entry(RUN).replace(b"/mnt/us", os.fsencode(self.media))
        self.shipping = (REPO / "packaging/kindle/launch.sh").read_bytes()
        self.stock.write_bytes(self.shipping)
        (self.stage / "shipping-launcher.sh").write_bytes(self.shipping)
        self.stock_inode = self.stock.stat().st_ino
        self.doc.write_bytes(PREPARE.DOCUMENT)
        (self.stage / "document-launcher.sh").write_bytes(PREPARE.DOCUMENT)
        self.doc_before = self.doc.stat()
        for base in (self.home, self.stage):
            (base / "data/device-profile").write_bytes(b"scribe\n")
            (base / "data/device-allowlist").write_bytes(b"scribe 5.19.5\n")
            for name, size in (("bilicomics", 74340832), ("kindle-jpeg-worker", 426672)):
                path = base / "app" / name
                with path.open("xb") as executable:
                    executable.truncate(size if base == self.home else 100)
                path.chmod(0o700)
        (self.stage / "data/demo/state.sqlite3").write_bytes(b"synthetic metadata")
        (self.stage / "run-id").write_text(RUN + "\n", encoding="ascii")
        (self.stage / "cleanup-assets").write_bytes(PREPARE.demo_roster())
        (self.stage / "entry.sh").write_bytes(self.shim)
        self.ready.write_bytes(self.shim)
        (self.stage / "launch.sh").write_bytes(PREPARE.diagnostic_launcher(self.shipping, RUN))
        target = (HERE / "switch.sh").read_text(encoding="ascii")
        for source, fixture in (
            ("/mnt/us", self.media), ("/etc/prettyversion.txt", self.root / "system/firmware"),
            ("/sys/class/graphics/fb0/bits_per_pixel", self.root / "system/bpp"),
            ("/sys/class/graphics/fb0/virtual_size", self.root / "system/size"),
            ("-c /dev/fb0", f'-f "{self.root}/system/framebuffer"'),
            ('"/tmp/$application-launcher.lock"', f'"{self.root}/system/$application-launcher.lock"'),
            ('= 0:1 ]; }', f'= {os.geteuid()}:1 ]; }}'),
        ):
            self.assertIn(source, target)
            target = target.replace(source, str(fixture))
        self.switch = self.stage / "switch.sh"
        self.switch.write_text(target, encoding="ascii")
        self.command(self.root / "bin/lipc-get-prop",
                     'if [ "$1" = -s ]; then echo active; else echo com.lab126.KPPMainApp; fi')
        self.command(self.root / "bin/status", "echo 'statusbar start/running, process 42'")
        self.command(self.root / "bin/pidof", f"echo {os.getpid()}")
        self.command(self.root / "bin/sqlite3", 'test "$1" = -readonly && echo 1:1')
        self.command(self.stage / "route-fs", f'''
if [ "$1" = move ] && [ "$3" = "${{STOP_AFTER:-none}}" ]; then
    {shlex.quote(str(self.tool))} "$@" || exit 1
    kill -KILL "$PPID"
    exit 0
fi
exec {shlex.quote(str(self.tool))} "$@"''')

    def clean_fixture(self):
        for item in (self.media, self.root / "bin", self.root / "system"):
            if item.exists():
                shutil.rmtree(item)

    def command(self, path, source):
        path.write_text("#!/bin/sh\n" + source + "\n", encoding="ascii")
        path.chmod(0o700)

    def invoke(self, operation, state=None, stop=None):
        result = subprocess.run(["sh", self.switch, RUN, operation], capture_output=True, text=True, timeout=15,
                                env={**os.environ, "PATH": f"{self.root}/bin:{os.defpath}", "STOP_AFTER": stop or ""})
        if state is None:
            self.assertNotEqual(result.returncode, 0, result.stdout)
        else:
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(result.stdout, f"run={RUN}\nroute={state}\n")

    def check_stock(self):
        self.assertEqual(self.stock.stat().st_ino, self.stock_inode)
        self.assertEqual(self.stock.read_bytes(), self.shipping)
        self.assertEqual(self.doc.stat().st_ino, self.doc_before.st_ino)
        self.assertEqual(self.doc.read_bytes(), PREPARE.DOCUMENT)
        self.assertFalse(self.retained.exists())
        self.assertFalse(self.armed.exists())

    def test_install_restore_and_clean_only_this_stage(self):
        self.invoke("inspect", "stock")
        self.invoke("activate", "armed")
        self.assertEqual(self.stock.read_bytes(), self.shim)
        self.assertEqual(self.retained.stat().st_ino, self.stock_inode)
        self.invoke("activate")
        self.invoke("cleanup")
        self.invoke("restore", "stock")
        self.check_stock()
        self.invoke("restore", "stock")
        self.invoke("cleanup", "removed")
        self.assertFalse(self.stage.exists())
        self.check_stock()

    def test_only_the_prepared_arm_can_activate_and_restore(self):
        launch = self.stage / 'launch.sh'
        default = PREPARE.diagnostic_launcher(self.shipping, RUN)
        serial = PREPARE.diagnostic_launcher(self.shipping, RUN, True)
        self.assertEqual(serial, default.replace(b'--exit-after-ms 2500 >&6', b'--exit-after-ms 2500 --serial-shelf-render >&6'))
        launch.write_bytes(serial)
        self.invoke('activate')
        self.check_stock()
        self.switch.write_bytes(PREPARE.diagnostic_switch(self.switch.read_bytes(), True))
        for rejected in (default, serial.replace(b'--serial-shelf-render', b'--serial-shelf-render --screen shelf')):
            launch.write_bytes(rejected)
            self.invoke('activate')
            self.assertFalse(self.journal.exists())
            self.check_stock()
        launch.write_bytes(serial)
        self.invoke('activate', 'armed')
        self.invoke('restore', 'stock')
        self.invoke('cleanup', 'removed')
        self.check_stock()

    def test_approval_and_parallel_guards(self):
        descriptor = os.open(self.home, os.O_DIRECTORY | os.O_RDONLY)
        try:
            fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
            self.invoke("activate")
        finally:
            os.close(descriptor)
        (self.root / "system/aidoku-launcher.lock").mkdir()
        self.invoke("activate")
        (self.root / "system/aidoku-launcher.lock").rmdir()
        (self.home / "data/device-allowlist").write_bytes(b"scribe 5.19.4\n")
        self.invoke("activate")
        self.check_stock()

    def test_native_card_touch_does_not_prevent_restoration(self):
        self.invoke("activate", "armed")
        touched = self.doc_before.st_mtime_ns + 4_000_000_000
        os.utime(self.doc, ns=(self.doc_before.st_atime_ns, touched))
        self.invoke("inspect", "armed")
        self.invoke("restore", "stock")
        self.check_stock()
        self.assertEqual(self.doc.stat().st_mtime_ns, touched)

    def test_native_card_replacement_with_matching_bytes_is_refused(self):
        self.invoke("activate", "armed")
        original = self.root / "system/original-document"
        self.doc.rename(original)
        shutil.copy2(original, self.doc)
        self.assertNotEqual(self.doc.stat().st_ino, self.doc_before.st_ino)
        self.invoke("restore")
        self.assertEqual(self.stock.read_bytes(), self.shim)
        self.assertEqual(self.retained.stat().st_ino, self.stock_inode)
        self.doc.unlink()
        original.rename(self.doc)
        self.invoke("restore", "stock")
        self.check_stock()

    def test_production_allowlist_touch_still_prevents_restoration(self):
        self.invoke("activate", "armed")
        allowlist = self.home / "data/device-allowlist"
        before = allowlist.stat()
        os.utime(allowlist, ns=(before.st_atime_ns, before.st_mtime_ns + 4_000_000_000))
        self.invoke("restore")
        self.assertEqual(self.stock.read_bytes(), self.shim)
        self.assertEqual(self.retained.stat().st_ino, self.stock_inode)
        os.utime(allowlist, ns=(before.st_atime_ns, before.st_mtime_ns))
        self.invoke("restore", "stock")
        self.check_stock()

    def test_retained_original_with_matching_bytes_but_different_inode(self):
        self.invoke("activate", "armed")
        old = self.root / "system/original"
        self.retained.rename(old)
        self.retained.write_bytes(old.read_bytes())
        self.invoke("restore")
        self.assertEqual(self.stock.read_bytes(), self.shim)
        self.assertEqual(old.stat().st_ino, self.stock_inode)
        self.retained.unlink()
        old.rename(self.retained)
        self.invoke("restore", "stock")
        self.check_stock()

    def test_foreign_canonical_path_is_not_overwritten(self):
        self.invoke("activate", stop="park")
        outside = self.root / "system/foreign"
        outside.write_bytes(b"not part of this run")
        self.stock.symlink_to(outside)
        self.invoke("restore")
        self.assertTrue(self.stock.is_symlink())
        self.assertEqual(outside.read_bytes(), b"not part of this run")
        self.assertEqual(self.retained.stat().st_ino, self.stock_inode)
        self.stock.unlink()
        self.invoke("restore", "stock")

    def test_unsafe_stage_is_not_cleaned(self):
        self.invoke("activate", "armed")
        self.invoke("restore", "stock")
        (self.stage / "data/logs").mkdir()
        sentinel = self.stage / "data/logs/foreign.log"
        sentinel.write_bytes(b"foreign")
        self.invoke("cleanup")
        self.assertEqual(sentinel.read_bytes(), b"foreign")
        self.assertTrue(self.switch.is_file())
        sentinel.unlink()
        self.invoke("cleanup", "removed")
        self.check_stock()

    def test_missing_journal_after_pending_write(self):
        self.pending.write_bytes(b"interrupted record")
        self.invoke("restore", "stock")
        self.invoke("cleanup", "removed")
        self.check_stock()

    def test_interrupted_dummy_probe_never_moved_production(self):
        probe = self.stage / "rename-probe"
        (probe / "left").mkdir(parents=True)
        (probe / "right").mkdir()
        (probe / "right/dummy").write_text(RUN + "\n", encoding="ascii")
        self.invoke("activate")
        self.assertFalse(self.journal.exists())
        self.invoke("restore", "stock")
        self.invoke("cleanup", "removed")
        self.check_stock()

    def test_cleanup_of_bound_runtime_files_and_partial_demo_publish(self):
        self.invoke("activate", "armed")
        self.picked.mkdir()
        picked_entry = self.picked / "entry"
        picked_entry.write_text(f"{RUN} 9999999 1\n", encoding="ascii")
        self.invoke("restore", "stock")
        for name in (f"data/{RUN}.jsonl", "data/logs/launcher.log", "data/logs/application.log.1",
                     "data/demo/state.sqlite3-wal", "data/demo/fixtures/catalog.json",
                     "data/demo/fixtures/covers/cover-28601.png", "data/demo/fixtures/pages/.demo-62-123-45.pending"):
            leaf = self.stage / name
            leaf.parent.mkdir(parents=True, exist_ok=True)
            leaf.write_bytes(b"diagnostic runtime file")
        (self.stage / "data/fonts").mkdir()
        bad = self.stage / "data/demo/fixtures/pages/.demo-foreign.pending"
        bad.touch()
        self.invoke("cleanup")
        self.assertTrue(bad.is_file())
        self.assertTrue(picked_entry.is_file())
        self.assertTrue(self.switch.is_file())
        bad.unlink()
        self.invoke("cleanup", "removed")
        self.check_stock()

    def test_journal_published_then_disconnect(self):
        self.invoke("activate", stop="journal")
        self.assertTrue(self.journal.is_file())
        self.invoke("activate")
        self.invoke("restore", "stock")
        self.check_stock()

    def test_first_activation_rename_then_disconnect(self):
        self.invoke("activate", stop="park")
        self.invoke("inspect", "parked")
        self.assertFalse(self.stock.exists())
        self.invoke("restore", "stock")
        self.check_stock()

    def test_second_activation_rename_then_disconnect(self):
        self.invoke("activate", stop="publish")
        self.invoke("inspect", "unarmed")
        self.assertNotEqual(subprocess.run(["sh", self.stock, "--library"], timeout=5).returncode, 0)
        self.invoke("restore", "stock")
        self.check_stock()

    def test_first_restoration_rename_then_disconnect(self):
        self.invoke("activate", "armed")
        self.invoke("restore", stop="unpublish")
        self.invoke("inspect", "parked")
        self.invoke("restore", "stock")
        self.check_stock()

    def test_second_restoration_rename_then_disconnect(self):
        self.invoke("activate", "armed")
        self.invoke("restore", stop="return")
        self.invoke("inspect", "stock")
        self.invoke("restore", "stock")
        self.check_stock()

    def test_admission_records_process_and_refuses_second_pick(self):
        stage_launch = self.stage / "launch.sh"
        self.invoke("activate", "armed")
        replacement = self.root / "system/record-shell"
        self.command(replacement, 'printf "%s %s\n" "$$" "$1"')
        saved = self.root / "system/original-launch"
        stage_launch.rename(saved)
        os.link(replacement, stage_launch)
        try:
            first = subprocess.run(["sh", self.stock, "--library"], capture_output=True, text=True, timeout=5)
            self.assertEqual(first.returncode, 0, first.stderr)
            pid, argument = first.stdout.split()
            self.assertEqual(argument, "--library")
            run, recorded_pid, birth = (self.picked / "entry").read_text(encoding="ascii").split()
            self.assertEqual((run, recorded_pid, birth.isdecimal()), (RUN, pid, True))
            second = subprocess.run(["sh", self.stock, "--library"], capture_output=True, timeout=5)
            self.assertNotEqual(second.returncode, 0)
        finally:
            stage_launch.unlink()
            saved.rename(stage_launch)
        self.invoke("inspect", "used")
        self.invoke("restore", "stock")
        self.check_stock()

    def test_live_entry_disarms_but_does_not_move_launcher(self):
        self.invoke("activate", "armed")
        self.picked.mkdir()
        pid = os.getpid()
        birth = Path(f"/proc/{pid}/stat").read_text(encoding="ascii").rsplit(") ", 1)[1].split()[19]
        (self.picked / "entry").write_text(f"{RUN} {pid} {birth}\n", encoding="ascii")
        self.invoke("restore")
        self.assertFalse(self.armed.exists())
        self.assertEqual(self.stock.read_bytes(), self.shim)
        (self.picked / "entry").write_text(f"{RUN} {pid} {int(birth) + 1}\n", encoding="ascii")
        self.invoke("restore", "stock")
        self.check_stock()


if __name__ == "__main__":
    unittest.main()
