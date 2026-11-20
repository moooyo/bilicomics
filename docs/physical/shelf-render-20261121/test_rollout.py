"""Host-only state-machine, input-boundary and recovery checks for the production rollout."""

import contextlib
import os
from pathlib import Path
import sqlite3
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import rollout


def navigation(tab="home", pointer=(899, 852)):
    return {"screen": [1860, 2480], "pointer": list(pointer), "buttons_clear": True, "title": {"count": 0},
            "home": {"count": 1, "box": [100, 850, 200, 980], "underline_pct": 80 if tab == "home" else 0},
            "library": {"count": 1, "box": [300, 850, 405, 980], "underline_pct": 80 if tab == "library" else 0}}


class Fixture(rollout.Rollout):
    def __init__(self, path):
        self.path, self.run = path, path.name
        self.tab, self.pointer = "home", (899, 852)
        self.installed, self.clicked, self.stopped = False, False, False
        self.steps = []
        self.request, self.password, self.askpass = object(), "", path / "askpass"
        self.info = {"payload_bytes": 4}

    def native_inventory(self, installed):
        if self.installed != installed or (self.clicked and not self.stopped):
            raise ValueError("Not the expected native state")
        return {"catalog_rows": int(installed)}

    def stage_payload(self, data):
        self.steps.append("stage")
        assert data == b"demo"
        self.flag("staging-intended")

    def control(self, op, *args, deadline=45):
        self.steps.append(op)
        if op == "install":
            self.installed = True
        if op == "observe" and not self.clicked:
            raise ValueError("Unqualified launch")
        if op == "stop":
            self.stopped = True
        if op == "rollback":
            self.installed = False
            self.stopped = True

    def probe(self, mode, *args):
        self.steps.append(mode)
        if mode == "navigation":
            return navigation(self.tab, self.pointer)
        if mode.startswith("nav-"):
            self.tab = mode[4:]
            self.pointer = (405, 915)
            return {"ok": True, "buttons_clear_after": True}
        if mode == "card":
            return navigation(self.tab, self.pointer) | {"title": {"count": 1}}
        if mode == "open-card":
            self.clicked = True
            return {"ok": True, "buttons_clear_after": True, "target": "open-card"}
        if mode == "restore-pointer":
            self.pointer = args
            return {"restored": True}
        raise AssertionError(mode)

    def remove_stage(self):
        self.steps.append("remove-stage")


class Recovery(unittest.TestCase):
    def setUp(self):
        area = tempfile.TemporaryDirectory()
        self.addCleanup(area.cleanup)
        path = Path(area.name) / "bili-prod-00000001"
        path.mkdir()
        (path / "payload.tar.gz").write_bytes(b"demo")
        self.fixture = Fixture(path)
        stack = contextlib.ExitStack()
        self.addCleanup(stack.close)
        self.acquire = stack.enter_context(patch.object(rollout.device_lease, "acquire", return_value={"ok": True}))
        self.status = stack.enter_context(patch.object(rollout.device_lease, "status", return_value={"state": "held"}))
        self.release = stack.enter_context(patch.object(rollout.device_lease, "release", return_value={"state": "released"}))
        stack.enter_context(patch.object(rollout.getpass, "getpass", return_value="fixture"))
        stack.enter_context(patch.object(rollout.time, "sleep", return_value=None))
        stack.enter_context(patch.object(rollout, "roster", return_value=[]))
        self.fixture.info["stage_files"] = []

    def test_normal_card_smoke_keeps_install_after_restore(self):
        self.fixture.execute(False)
        self.assertTrue(self.fixture.installed)
        self.assertEqual(self.fixture.tab, "home")
        self.assertEqual(self.fixture.pointer, (899, 852))
        self.assertEqual(self.fixture.steps.count("open-card"), 1)
        self.assertLess(self.fixture.steps.index("stop"), self.fixture.steps.index("commit"))
        self.assertLess(self.fixture.steps.index("restore-pointer"), self.fixture.steps.index("commit"))
        self.assertNotIn("rollback", self.fixture.steps)
        self.assertEqual(self.fixture.steps[-1], "remove-stage")
        self.release.assert_called_once()

    def test_busy_lease_does_not_touch_device_or_clean_other_owners(self):
        self.acquire.side_effect = ValueError("busy")
        with self.assertRaisesRegex(ValueError, "busy"):
            self.fixture.execute(False)
        self.assertEqual(self.fixture.steps, [])
        self.release.assert_not_called()
        self.assertFalse(self.fixture.askpass.exists())

    def test_failed_runtime_observation_rolls_back_before_restore(self):
        original = self.fixture.control
        def action(op, *args, **kwargs):
            if op == "observe":
                raise ValueError("Controlled runtime failure")
            return original(op, *args, **kwargs)
        with patch.object(self.fixture, "control", action), self.assertRaisesRegex(ValueError, "Controlled runtime"):
            self.fixture.execute(False)
        self.assertFalse(self.fixture.installed)
        self.assertEqual(self.fixture.steps.count("open-card"), 1)
        self.assertLess(self.fixture.steps.index("rollback"), self.fixture.steps.index("restore-pointer"))
        self.assertNotIn("commit", self.fixture.steps)
        self.release.assert_called_once()

    def test_uncertain_cleanup_keeps_exact_lease_until_recovery_succeeds(self):
        with patch.object(self.fixture, "remove_stage", side_effect=ValueError("cleanup")):
            with self.assertRaisesRegex(ValueError, "cleanup"):
                self.fixture.execute(False)
        self.release.assert_not_called()
        self.assertFalse(self.fixture.askpass.exists())
        self.fixture.execute(True)
        self.status.assert_called_once()
        self.acquire.assert_called_once()
        self.release.assert_called_once()
        self.assertEqual(self.fixture.steps.count("open-card"), 1)

    def test_wrong_owner_recovery_never_connects(self):
        self.status.side_effect = ValueError("wrong owner")
        with self.assertRaisesRegex(ValueError, "wrong owner"):
            self.fixture.execute(True)
        self.assertEqual(self.fixture.steps, [])
        self.acquire.assert_not_called()
        self.release.assert_not_called()


class MemberPaths(unittest.TestCase):
    def test_no_absolute_links_or_hidden_traversal(self):
        self.assertTrue(rollout.safe_member("notices/backend/PATCHES.md"))
        for name in ("/mnt/us/bilicomics", ".", "../file", "notices/../secret", "a//b", "a b", "a\nb"):
            self.assertFalse(rollout.safe_member(name), name)

    def test_inventory_refuses_existing_install_and_unrelated_occupant(self):
        boot = "01234567-89ab-cdef-0123-456789abcdef"
        run = "bili-prod-00000001"
        fixed = {"firmware": "5.19.5", "catalog_selected_rows": "0", "fb_bits_per_pixel": "8", "fb_virtual_size": "1872,4960",
                 "doc_spelling": "absent", "doc_sidecar": "absent", "doc_lower_id": "absent", "doc_upper_id": "absent",
                 "doc_lower_bytes": "absent", "doc_upper_bytes": "absent", "doc_index": "0:0", "kindle_boot": boot, "doc_volume": "41"}
        for sample in ("before", "after"):
            fixed.update({sample + "_" + key: value for key, value in (("front", "native"), ("power", "active"),
                          ("window_manager", "running"), ("statusbar", "running"), ("lock_bilicomics", "absent"),
                          ("lock_weread", "absent"), ("lock_aidoku", "absent"))})
        paths = ["bilicomics", "bilicomics/data", "bilicomics/launch.sh", "bilicomics/app/bilicomics",
                 "bilicomics/app/kindle-jpeg-worker", "documents/bilicomics.sh", "documents/BiliComics.sh",
                 "documents/bilicomics.sdr", "documents/BiliComics.sdr", "documents/bilicomics.sh.sdr", "documents/BiliComics.sh.sdr"]
        fixed.update({"path:/mnt/us/" + path: "absent" for path in paths})
        def encode(changes):
            return "\n".join(f"{key}={value}" for key, value in (fixed | changes).items()).encode("ascii")
        self.assertEqual(rollout.inventory(encode({}), False)["catalog_rows"], 0)
        for changes in ({"after_lock_weread": "directory"}, {"before_front": "other"}, {"path:/mnt/us/bilicomics": "directory"},
                        {"catalog_selected_rows": "1"}, {"path:/mnt/us/documents/BiliComics.sh": "file"},
                        {"fb_virtual_size": "unsupported"}, {"firmware": "different"}):
            with self.subTest(changes=changes), self.assertRaises(ValueError):
                rollout.inventory(encode(changes), False)
        installed = {"catalog_selected_rows": "1", "doc_index": "1:1", "doc_spelling": "bilicomics.sh",
                     "doc_lower_id": "41:198:1", "doc_lower_bytes": "exact"}
        installed.update({"path:/mnt/us/" + path: "directory" if path in ("bilicomics", "bilicomics/data") else "file"
                          for path in paths[:6]})
        upper = "path:/mnt/us/documents/BiliComics.sh"
        self.assertFalse(rollout.inventory(encode(installed), True)["uppercase_lookup_is_alias"])
        alias = installed | {upper: "file", "doc_upper_id": "41:198:1", "doc_upper_bytes": "exact"}
        self.assertTrue(rollout.inventory(encode(alias), True)["uppercase_lookup_is_alias"])
        for changes in ({"doc_spelling": "unexpected"}, {"doc_lower_id": "absent"}, {"doc_upper_id": "42:198:1"},
                        {"doc_upper_id": "41:199:1"}, {"doc_sidecar": "present"}, {"catalog_selected_rows": "2"},
                        {"path:/mnt/us/documents/bilicomics.sdr": "directory"}, {upper: "link"}, {upper: "absent"},
                        {"doc_upper_id": "41:198:2", "doc_lower_id": "41:198:2"}):
            with self.subTest(changes=changes), self.assertRaises(ValueError):
                rollout.inventory(encode(alias | changes), True)
        fuse = alias | {"doc_upper_id": "41:199:1"}
        witness = {"run": run, "kindle_boot": boot, "doc_volume": "41", "doc_lower_id": "41:198:1",
                   "doc_upper_id": "41:199:1", "doc_alias_mode": "casefold-proven"}
        def verify(changes, supplied):
            return rollout.inventory(encode(fuse | changes), True, run=run, witness=supplied)
        self.assertTrue(verify({}, witness)["uppercase_lookup_is_alias"])
        for changes in ({"doc_index": "2:1"}, {"doc_index": "1:0"}, {"doc_lower_bytes": "unknown"}, {"doc_upper_bytes": "absent"},
                        {"doc_volume": "42"}, {"doc_lower_id": "41:197:1"}, {"doc_upper_id": "42:199:1"},
                        {"doc_spelling": "unexpected"}, {"doc_sidecar": "present"}):
            with self.subTest(changes=changes), self.assertRaises(ValueError):
                verify(changes, witness)
        for changes in ({"run": "bili-prod-00000002"}, {"kindle_boot": "01234567-89ab-cdef-0123-456789abcdee"},
                        {"doc_volume": "42"}, {"doc_lower_id": "41:197:1"}, {"doc_upper_id": "41:199:2"},
                        {"doc_alias_mode": "same-inode"}, {"doc_alias_mode": "absent"}):
            with self.subTest(witness=changes), self.assertRaises(ValueError):
                verify({}, witness | changes)
        with self.assertRaises(ValueError):
            verify({}, None)

    def test_exact_package_document_check_uses_bytes_and_exact_single_catalog_row(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            expected = "#!/bin/sh\n# Name: BiliComics for Kindle\nexec sh /mnt/us/bilicomics/launch.sh --library\n"
            lower = root / "bilicomics.sh"
            lower.write_bytes(expected.encode("ascii"))
            sqlite = root / "sqlite3"
            catalog = root / "fixture.sqlite3"
            insert = "INSERT INTO Entries VALUES(?, 'text/x-shellscript','BiliComics for Kindle')"
            with sqlite3.connect(catalog) as database:
                database.execute("CREATE TABLE Entries(p_location,p_mimeType,p_titles_0_nominal)")
                database.execute(insert, ("/mnt/us/documents/bilicomics.sh",))
            sqlite.write_text(f'''#!/usr/bin/env python3
import sqlite3, sys
assert sys.argv[1:5] == ['-readonly', '-cmd', '.timeout 1500', '/var/local/cc.db']
with sqlite3.connect({(catalog.as_uri() + '?mode=ro')!r}, uri=True) as db:
    for row in db.execute(sys.argv[5]): print(*row)
''')
            sqlite.chmod(0o700)
            env = dict(os.environ, PATH=str(root) + ":" + os.defpath)
            scan = rollout.DOCUMENT_SCAN.replace(b"/mnt/us/documents", os.fsencode(root)) + rollout.document_comparisons(expected)
            first = subprocess.run(["sh", "-eu", "-s"], input=scan, capture_output=True, check=True, env=env)
            self.assertIn(b"doc_lower_bytes=exact\ndoc_upper_bytes=absent\ndoc_index=1:1\n", first.stdout)
            with sqlite3.connect(catalog) as database:
                database.execute(insert, ("/mnt/us/documents/BILICOMICS.SH",))
            duplicate = subprocess.run(["sh", "-eu", "-s"], input=scan, capture_output=True, check=True, env=env)
            self.assertIn(b"doc_index=2:1\n", duplicate.stdout)
            modified = expected.replace("# Name:", "# Fake:")
            self.assertEqual(len(modified), len(expected))
            lower.write_bytes(modified.encode("ascii"))
            self.assertNotEqual(subprocess.run(["sh", "-eu", "-s"], input=scan, capture_output=True, env=env).returncode, 0)
            lower.unlink()
            lower.symlink_to(sqlite)
            self.assertNotEqual(subprocess.run(["sh", "-eu", "-s"], input=scan, capture_output=True, env=env).returncode, 0)

    def test_device_spelling_scan_does_not_export_other_library_entries(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "private-title.txt").touch()
            scan = rollout.DOCUMENT_SCAN.replace(b"/mnt/us/documents", os.fsencode(root))
            def sample():
                result = subprocess.run(["sh", "-eu", "-s"], input=scan, capture_output=True, check=True).stdout
                self.assertNotIn(b"private-title", result)
                return dict(line.split("=", 1) for line in result.decode().splitlines())
            self.assertEqual(sample()["doc_spelling"], "absent")
            lower = root / "bilicomics.sh"
            lower.touch()
            self.assertEqual(sample()["doc_spelling"], "bilicomics.sh")
            self.assertEqual(sample()["doc_upper_id"], "absent")
            upper = root / "BiliComics.sh"
            os.link(lower, upper)
            sample2 = sample()
            self.assertEqual(sample2["doc_spelling"], "unexpected")
            self.assertEqual(sample2["doc_lower_id"], sample2["doc_upper_id"])
            upper.unlink()
            (root / "BiLiComics.Sh.SDR").mkdir()
            self.assertEqual(sample()["doc_sidecar"], "present")

    def test_long_cleanup_goes_over_stdin_and_checks_owned_objects(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            names = [f"probe/dependency-{index}-" + "x" * 80 for index in range(110)]
            current = object.__new__(rollout.Rollout)
            current.run = "bili-prod-00000001"
            current.stage, current.priv = str(root / "media"), str(root / "private")
            current.info = {"stage_files": names}
            paths = [root / "media", root / "private"]
            for path in paths:
                path.mkdir()
                (path / "owner").write_text(current.run + "\n")
            (paths[1] / "state").mkdir()
            (paths[1] / "state/document-volume").write_text("run-scoped fixture witness\n")
            (paths[0] / "probe").mkdir()
            for name in names:
                (paths[0] / name).touch()
            def local_fixture(command, data, deadline):
                self.assertEqual(command, "sh -s")
                self.assertGreater(len(data), 8192)
                subprocess.run(["sh", "-s"], input=data, capture_output=True, check=True)
            current.remote = local_fixture
            (paths[0] / "owner").write_text("another-run\n")
            with self.assertRaises(subprocess.CalledProcessError):
                current.remove_stage()
            self.assertTrue((paths[0] / names[-1]).exists())
            (paths[0] / "owner").write_text(current.run + "\n")
            extra = paths[0] / "foreign"
            extra.touch()
            with self.assertRaises(subprocess.CalledProcessError):
                current.remove_stage()
            self.assertTrue(extra.exists())
            extra.unlink()
            current.remove_stage()
            self.assertTrue(all(not path.exists() for path in paths))


if __name__ == "__main__":
    unittest.main()
