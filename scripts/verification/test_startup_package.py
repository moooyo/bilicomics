#!/usr/bin/env python3
"""Exercise startup provenance guards on ssh test-env without executing ARM code."""

import hashlib
import json
from pathlib import Path
import shutil
import stat
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch
import zipfile

from verify_package import (ALLOWED_FILES, EXECUTABLE_FILES, FIXED_TIMESTAMP,
                            SPATIAL_DITHER_NOTICES, SPATIAL_DITHER_PREFIX,
                            SPATIAL_DITHER_REFERENCE, STARTUP_COMMITS,
                            STARTUP_NOTICES, STARTUP_PREFIX, STARTUP_REFERENCE,
                            STARTUP_SOURCES, check_package, check_startup_notices)


class StartupPackageTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="bilicomics-startup-package-")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.source = Path(__file__).resolve().parents[2]
        self.archive = self.root / "candidate.zip"
        self.notices = {STARTUP_PREFIX + name:
                        (self.source / STARTUP_REFERENCE / name).read_bytes()
                        for name in STARTUP_NOTICES}

    def write_archive(self, members, with_manifest=False):
        records = []
        with zipfile.ZipFile(self.archive, "w", zipfile.ZIP_DEFLATED) as archive:
            for name, content in sorted(members.items()):
                info = zipfile.ZipInfo(name, date_time=FIXED_TIMESTAMP)
                info.create_system = 3
                info.compress_type = zipfile.ZIP_DEFLATED
                mode = 0o755 if name in EXECUTABLE_FILES else 0o644
                info.external_attr = (stat.S_IFREG | mode) << 16
                archive.writestr(info, content)
                records.append({"path": name, "bytes": len(content),
                                "sha256": hashlib.sha256(content).hexdigest()})
        if with_manifest:
            self.archive.with_suffix(".manifest.json").write_text(json.dumps({
                "development_candidate": True, "device_verified": False, "files": records,
            }) + "\n", encoding="utf-8")

    def check_notices(self, members, source=None):
        self.write_archive(members)
        report = {"checks": []}
        with zipfile.ZipFile(self.archive) as archive:
            check_startup_notices(archive, source or self.source, report)
        return report

    def copy_reference(self):
        source = self.root / "source"
        shutil.copytree(self.source / STARTUP_REFERENCE, source / STARTUP_REFERENCE)
        return source

    def test_complete_original_records_and_all_selected_snapshots_pass(self):
        report = self.check_notices(self.notices)
        self.assertEqual(len(report["checks"]), 1)
        check = report["checks"][0]
        self.assertEqual(check["name"], "startup_implementation_notice_provenance")
        self.assertEqual(check["status"], "passed")
        self.assertEqual(check["source_repository"], "AidokuForKindle")
        self.assertEqual(check["source_commits"], list(STARTUP_COMMITS))
        self.assertEqual(check["source_snapshots"], 9)
        self.assertIs(check["source_code_bundled"], False)

    def test_every_required_notice_is_required(self):
        for missing in self.notices:
            with self.subTest(missing=missing):
                members = {name: content for name, content in self.notices.items() if name != missing}
                with self.assertRaisesRegex(ValueError, "startup notice group is incomplete"):
                    self.check_notices(members)

    def test_replacing_any_notice_is_rejected_with_valid_zip_crc(self):
        for changed in self.notices:
            with self.subTest(changed=changed):
                members = dict(self.notices)
                members[changed] = b"Replaced provenance or license.\n"
                with self.assertRaisesRegex(ValueError, "startup notice differs from the frozen source"):
                    self.check_notices(members)

    def test_only_the_three_notice_records_are_package_members(self):
        expected = {STARTUP_PREFIX + name for name in STARTUP_NOTICES}
        self.assertEqual({name for name in ALLOWED_FILES if name.startswith(STARTUP_PREFIX)}, expected)
        for extra in ("device.log", "capture.png", "upstream/input_discovery.rs"):
            with self.subTest(extra=extra):
                members = dict(self.notices)
                members[STARTUP_PREFIX + extra] = b"Excluded input.\n"
                with self.assertRaisesRegex(ValueError, "contains source inputs"):
                    self.check_notices(members)

    def test_replaced_source_snapshot_is_rejected_without_changing_the_notices(self):
        source = self.copy_reference()
        relative = next(path for path in STARTUP_SOURCES if path.endswith("input_discovery.rs"))
        (source / STARTUP_REFERENCE / relative).write_bytes(b"Replaced input source.\n")
        with self.assertRaisesRegex(ValueError, "startup source differs from its manifest"):
            self.check_notices(self.notices, source)

    def test_recomputed_records_cannot_rebind_the_pinned_commits(self):
        source = self.copy_reference()
        path = source / STARTUP_REFERENCE / "UPSTREAM_FILES.json"
        manifest = json.loads(path.read_bytes())
        manifest["source_commits"][0] = "0" * 40
        content = (json.dumps(manifest, indent=2) + "\n").encode("utf-8")
        path.write_bytes(content)
        members = dict(self.notices)
        members[STARTUP_PREFIX + "UPSTREAM_FILES.json"] = content
        with self.assertRaisesRegex(ValueError, "pinned source identities"):
            self.check_notices(members, source)

    def test_recomputed_license_record_cannot_replace_the_original_license(self):
        source = self.copy_reference()
        reference = source / STARTUP_REFERENCE
        content = (reference / "LICENSE.AIDOKU").read_bytes() + b"Changed license.\n"
        (reference / "LICENSE.AIDOKU").write_bytes(content)
        manifest = json.loads((reference / "UPSTREAM_FILES.json").read_bytes())
        item = next(item for item in manifest["files"] if item["path"] == "LICENSE.AIDOKU")
        item["bytes"] = len(content)
        item["sha256"] = hashlib.sha256(content).hexdigest()
        manifest_content = (json.dumps(manifest, indent=2) + "\n").encode("utf-8")
        (reference / "UPSTREAM_FILES.json").write_bytes(manifest_content)
        members = dict(self.notices)
        members[STARTUP_PREFIX + "LICENSE.AIDOKU"] = content
        members[STARTUP_PREFIX + "UPSTREAM_FILES.json"] = manifest_content
        with self.assertRaisesRegex(ValueError, "original GPLv3 text"):
            self.check_notices(members, source)

    def test_full_checker_reaches_startup_provenance_before_elf_checks(self):
        application = self.root / "bilicomics"
        worker = self.root / "kindle-jpeg-worker"
        application.write_bytes(b"Selected application fixture, never executed.\n")
        worker.write_bytes(b"Selected JPEG worker fixture, never executed.\n")
        members = {name: b"Fixture placeholder.\n" for name in ALLOWED_FILES}
        members.update({SPATIAL_DITHER_PREFIX + name:
                        (self.source / SPATIAL_DITHER_REFERENCE / name).read_bytes()
                        for name in SPATIAL_DITHER_NOTICES})
        members.update(self.notices)
        members["bilicomics/app/bilicomics"] = application.read_bytes()
        members["bilicomics/app/kindle-jpeg-worker"] = worker.read_bytes()
        members[STARTUP_PREFIX + "README.md"] = b"Replaced notice with recomputed ZIP manifest.\n"
        self.write_archive(members, with_manifest=True)
        arguments = SimpleNamespace(archive=self.archive, binary=application, worker=worker,
                                    app_root=self.source, output=self.root / "verification.json",
                                    expected_binary_sha256=hashlib.sha256(application.read_bytes()).hexdigest(),
                                    expected_worker_sha256=hashlib.sha256(worker.read_bytes()).hexdigest(),
                                    expected_archive_sha256=None)
        with patch("verify_package.subprocess.run", side_effect=AssertionError("Unexpected ELF invocation")):
            with self.assertRaisesRegex(ValueError, "startup notice differs from the frozen source"):
                check_package(arguments, {"checks": []})


if __name__ == "__main__":
    unittest.main()
