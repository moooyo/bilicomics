#!/usr/bin/env python3
"""Exercise the isolated guest seed and its prelaunch rejection boundaries.

Run only in the designated remote verification environment, for example:
python3 -m unittest discover -s scripts/verification -p test_distinct_reader_fixture.py
"""

from contextlib import closing, redirect_stdout
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import shutil
import sqlite3
import struct
import sys
import tempfile
import unittest


sys.dont_write_bytecode = True
MODULE_PATH = Path(__file__).parent / "physical" / "distinct_reader_fixture.py"
SPEC = importlib.util.spec_from_file_location("distinct_reader_fixture", MODULE_PATH)
fixture = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(fixture)


class DistinctReaderFixtureTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.workspace = tempfile.TemporaryDirectory(prefix="bilicomics-distinct-fixture-tests-")
        cls.workspace_path = Path(cls.workspace.name)
        cls.source = cls.workspace_path / "canonical"
        cls.manifest = fixture.generate_fixture(cls.source)

    @classmethod
    def tearDownClass(cls):
        cls.workspace.cleanup()

    def setUp(self):
        self.case = self.workspace_path / self._testMethodName
        shutil.copytree(self.source, self.case)

    def load_manifest(self):
        return json.loads((self.case / "manifest.json").read_text(encoding="utf-8"))

    def write_manifest(self, value):
        (self.case / "manifest.json").write_text(fixture.canonical_json(value) + "\n", encoding="ascii")

    def rebind_database(self):
        manifest = self.load_manifest()
        content = (self.case / fixture.DATABASE).read_bytes()
        for record in manifest["files"]:
            if record["path"] == fixture.DATABASE:
                record.update(bytes=len(content), sha256=hashlib.sha256(content).hexdigest())
        manifest["content_total_bytes"] = sum(record["bytes"] for record in manifest["files"])
        self.write_manifest(manifest)

    def execute_database(self, sql, parameters=()):
        with closing(sqlite3.connect(self.case / fixture.DATABASE)) as connection:
            with connection:
                connection.execute(sql, parameters)
        self.rebind_database()

    def assert_rejected(self, message=None):
        if message is None:
            with self.assertRaises((fixture.FixtureError, OSError, sqlite3.Error)):
                fixture.validate_fixture(self.case)
        else:
            with self.assertRaisesRegex(fixture.FixtureError, message):
                fixture.validate_fixture(self.case)

    def test_guest_seed_has_twelve_distinct_pages_and_unverified_receipts(self):
        report = fixture.validate_fixture(self.case)
        self.assertEqual(report["status"], "passed")
        self.assertEqual(report["file_count"], 14)
        self.assertEqual(report["encoded_page_count"], 12)
        self.assertEqual(report["decoded_page_count"], 12)
        self.assertFalse(report["application_decode_proof_claimed"])
        self.assertTrue(report["external_network_guard_required"])
        self.assertEqual(report["manifest_sha256"], hashlib.sha256((self.case / "manifest.json").read_bytes()).hexdigest())
        self.assertFalse((self.case / "data/account").exists())
        self.assertTrue((self.case / fixture.PAGE_DIRECTORY).is_dir())
        self.assertEqual(self.manifest["initial_preferences"]["preload"], 3)
        self.assertEqual(self.manifest["initial_anchor"]["page_index"], 0)
        with closing(sqlite3.connect(self.case / fixture.DATABASE)) as connection:
            self.assertEqual(connection.execute("PRAGMA user_version").fetchone()[0], 2)
            self.assertEqual(len(connection.execute("PRAGMA table_info(page_identity_history)").fetchall()), 6)
            receipts = [json.loads(row[0]) for row in connection.execute("SELECT payload FROM page_receipts")]
        self.assertEqual(len(receipts), 12)
        self.assertTrue(all(value["verified_at"] == 0 and value["decode_validation_version"] == 0 for value in receipts))

    def test_generation_is_repeatable_and_accepts_only_nonterminal_anchors(self):
        repeated = self.workspace_path / "repeat"
        manifest = fixture.generate_fixture(repeated)
        self.assertEqual(manifest, self.manifest)
        self.assertEqual((repeated / "manifest.json").read_bytes(), (self.source / "manifest.json").read_bytes())
        for record in manifest["files"]:
            self.assertEqual((repeated / record["path"]).read_bytes(), (self.source / record["path"]).read_bytes())
        second = self.workspace_path / "second-anchor"
        fixture.generate_fixture(second, anchor_index=1)
        self.assertEqual(fixture.validate_fixture(second)["manifest"]["initial_anchor"]["page_index"], 1)
        with self.assertRaises(fixture.FixtureError):
            fixture.generate_fixture(self.workspace_path / "last-anchor", anchor_index=11)
        with self.assertRaises(fixture.FixtureError):
            fixture.generate_fixture(self.source)

    def test_machine_markers_are_large_independent_page_codes(self):
        codes = set()
        for page in self.manifest["pages"]:
            pixels = fixture._decode_png((self.case / page["file"]).read_bytes())
            marker = page["machine_marker"]
            self.assertGreaterEqual(marker["block_radius"], 24)
            for sample in marker["samples"]:
                for dx, dy in ((0, 0), (-24, -24), (24, 24), (-24, 24), (24, -24)):
                    self.assertEqual(pixels[(sample["y"] + dy) * fixture.WIDTH + sample["x"] + dx], sample["luma"])
            code = sum((pixels[marker["samples"][sample]["y"] * fixture.WIDTH
                               + marker["samples"][sample]["x"]] == 255) << bit
                       for bit, sample in enumerate(marker["bit_sample_indices"]))
            self.assertEqual(code, page["page_number"])
            codes.add(code)
        self.assertEqual(codes, set(range(1, 13)))

    def test_rejects_the_real_credential_tree_even_when_empty(self):
        (self.case / "data/account").mkdir()
        self.assert_rejected("credential account tree")
        private = self.case / "data/account/private"
        private.mkdir()
        (private / "session.json").write_text("{}", encoding="ascii")
        self.assert_rejected("credential account tree")

    def test_rejects_traversal_and_extra_account_content(self):
        manifest = self.load_manifest()
        manifest["pages"][0]["file"] = "../outside.png"
        self.write_manifest(manifest)
        self.assert_rejected("traversal")
        self.write_manifest(self.manifest)
        (self.case / "data/accounts/not-guest").mkdir()
        self.assert_rejected("unexpected directory")

    def test_rejects_symbolic_and_hard_links(self):
        page = self.case / self.manifest["pages"][0]["file"]
        outside = self.workspace_path / "link-source.png"
        shutil.copyfile(page, outside)
        page.unlink()
        os.symlink(outside, page)
        self.assert_rejected("links")
        page.unlink()
        os.link(outside, page)
        self.assert_rejected("hard links")

    def test_rejects_file_size_overflow_and_unknown_files(self):
        unexpected = self.case / "data/state.sqlite3-wal"
        unexpected.write_bytes(b"not a permitted sidecar")
        self.assert_rejected("unexpected file")
        unexpected.unlink()
        with (self.case / self.manifest["pages"][0]["file"]).open("wb") as stream:
            stream.truncate(fixture.MAX_PNG_BYTES + 1)
        self.assert_rejected("size")

    def test_rejects_repeated_pixels_even_after_all_page_metadata_is_replaced(self):
        manifest = self.load_manifest()
        repeated = dict(manifest["pages"][0])
        repeated.update(index=1, page_index=1, page_number=2)
        manifest["pages"][1] = repeated
        self.write_manifest(manifest)
        self.assert_rejected("full-page pattern")

    def test_rejects_corruption_dimensions_and_manifest_identity_changes(self):
        page = self.case / self.manifest["pages"][0]["file"]
        original = page.read_bytes()
        damaged = bytearray(original)
        damaged[len(damaged) // 2] ^= 0x40
        page.write_bytes(damaged)
        self.assert_rejected("CRC")
        wrong_header = fixture._png_chunk(b"IHDR", struct.pack(">IIBBBBB", fixture.WIDTH + 1, fixture.HEIGHT, 8, 0, 0, 0, 0))
        page.write_bytes(fixture.PNG_SIGNATURE + wrong_header + original[33:])
        self.assert_rejected("1700 by 2360")
        page.write_bytes(original)
        manifest = self.load_manifest()
        manifest["pages"][0]["encoded_sha256"] = "0" * 64
        self.write_manifest(manifest)
        self.assert_rejected("identity")

    def test_rejects_extra_schema_objects_after_database_rehash(self):
        self.execute_database("CREATE VIEW unexpected_view AS SELECT account_key FROM preferences")
        self.assert_rejected("schema")
        self.execute_database("DROP VIEW unexpected_view")
        self.execute_database("CREATE TRIGGER unexpected_trigger AFTER INSERT ON preferences BEGIN SELECT 1; END")
        self.assert_rejected("schema")
        self.execute_database("DROP TRIGGER unexpected_trigger")
        self.execute_database("CREATE TABLE unexpected_table(value TEXT)")
        self.assert_rejected("schema")

    def test_rejects_extra_accounts_transactions_tasks_and_source_urls_after_rehash(self):
        changes = (
            ("INSERT INTO preferences VALUES(?,?,?)", ("other-account", "preferences", "{}")),
            ("INSERT INTO preferences VALUES(?,?,?)", ("guest", "purchase_journal", "{}")),
            ("INSERT INTO downloads VALUES(?,?,?,?,?,?,?)", ("guest", "task", fixture.EPISODE, fixture.REVISION, "queued", "{}", 0)),
            ("INSERT INTO page_commit_journal VALUES(?,?,?,?,?)", ("guest", "task", "page", 0, "{}")),
            ("UPDATE image_indices SET payload=?", ("{\"extra\":{\"source_urls\":[\"https://example.invalid/page\"]}}",)),
        )
        for sql, parameters in changes:
            with self.subTest(sql=sql):
                shutil.copyfile(self.source / fixture.DATABASE, self.case / fixture.DATABASE)
                self.write_manifest(self.manifest)
                self.execute_database(sql, parameters)
                self.assert_rejected("guest-only seed")

    def test_rejects_mismatched_receipt_history_and_anchor_after_rehash(self):
        changes = (
            "UPDATE page_receipts SET page_index=99 WHERE page_index=0",
            "UPDATE page_identity_history SET bytes=1 WHERE page_id=(SELECT page_id FROM page_receipts LIMIT 1)",
            "UPDATE reading_anchors SET payload='{}'",
            "UPDATE page_receipts SET payload=json_set(payload,'$.decode_validation_version',1) WHERE page_index=0",
        )
        for sql in changes:
            with self.subTest(sql=sql):
                shutil.copyfile(self.source / fixture.DATABASE, self.case / fixture.DATABASE)
                self.write_manifest(self.manifest)
                self.execute_database(sql)
                self.assert_rejected("guest-only seed")

    def test_cli_returns_machine_readable_success_and_rejection(self):
        success = io.StringIO()
        with redirect_stdout(success):
            status = fixture.main(["validate", str(self.case)])
        self.assertEqual(status, 0)
        self.assertEqual(json.loads(success.getvalue())["status"], "passed")
        failure = io.StringIO()
        with redirect_stdout(failure):
            status = fixture.main(["generate", "--output", str(self.case)])
        self.assertEqual(status, 2)
        self.assertEqual(json.loads(failure.getvalue())["status"], "rejected")


if __name__ == "__main__":
    unittest.main()
