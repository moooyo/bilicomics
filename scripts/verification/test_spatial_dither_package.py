#!/usr/bin/env python3
"""Check retained spatial-dither notices without building or executing an ARM ELF.

Run only on the designated Linux worker through ssh test-env. Positive cases
exercise the production notice checker directly; invalid package fixtures also
prove that the full checker reaches that guard before its independent ELF checks.
"""

import hashlib
import json
from pathlib import Path
import stat
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch
import zipfile

from verify_package import (ALLOWED_FILES, EXECUTABLE_FILES, FIXED_TIMESTAMP,
                            SPATIAL_DITHER_NOTICES, SPATIAL_DITHER_PREFIX,
                            SPATIAL_DITHER_REFERENCE, check_package,
                            check_spatial_dither_notices)


class SpatialDitherPackageTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="bilicomics-spatial-dither-package-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.source = Path(__file__).resolve().parents[2]
        self.archive = self.root / "candidate.zip"
        self.notices = {SPATIAL_DITHER_PREFIX + name:
                        (self.source / SPATIAL_DITHER_REFERENCE / name).read_bytes()
                        for name in SPATIAL_DITHER_NOTICES}

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

    def check_notices(self, members):
        self.write_archive(members)
        report = {"checks": []}
        with zipfile.ZipFile(self.archive) as archive:
            check_spatial_dither_notices(archive, self.source, report)
        return report

    def test_complete_original_notice_bytes_pass_without_any_elf_mock(self):
        report = self.check_notices(self.notices)
        self.assertEqual(len(report["checks"]), 1)
        check = report["checks"][0]
        self.assertEqual(check["name"], "spatial_dither_notice_provenance")
        self.assertEqual(check["status"], "passed")
        self.assertEqual(check["source_repository"], "AidokuForKindle")
        self.assertEqual(check["source_snapshot_sha256"],
                         "25df24627f3db1dc3a334b1af6f30246744e3fda9a679191e15711f43c4fde97")
        self.assertIs(check["reference_images_bundled"], False)
        with zipfile.ZipFile(self.archive) as archive:
            for name, expected in self.notices.items():
                self.assertEqual(archive.read(name), expected)

    def test_every_required_notice_is_required(self):
        for missing in self.notices:
            with self.subTest(missing=missing):
                members = {name: content for name, content in self.notices.items() if name != missing}
                with self.assertRaisesRegex(ValueError, "notice group is incomplete"):
                    self.check_notices(members)

    def test_replacing_any_notice_is_rejected_despite_a_valid_zip_crc(self):
        for changed in self.notices:
            with self.subTest(changed=changed):
                members = dict(self.notices)
                members[changed] = b"A replaced source or license record.\n"
                with self.assertRaisesRegex(ValueError, "notice differs from the frozen source"):
                    self.check_notices(members)

    def test_research_images_and_algorithm_sources_are_not_notice_members(self):
        expected = {"notices/spatial-dither/LICENSE.AIDOKU", "notices/spatial-dither/README.md",
                    "notices/spatial-dither/UPSTREAM_FILES.json"}
        self.assertEqual({name for name in ALLOWED_FILES if name.startswith(SPATIAL_DITHER_PREFIX)}, expected)
        for extra in ("expected-off.png", "input-gray8.png", "spatial_dither.rs"):
            with self.subTest(extra=extra):
                members = dict(self.notices)
                members[SPATIAL_DITHER_PREFIX + extra] = b"Excluded research input.\n"
                with self.assertRaisesRegex(ValueError, "contains research inputs"):
                    self.check_notices(members)

    def test_the_full_package_checker_wires_notice_provenance_before_elf_checks(self):
        application = self.root / "bilicomics"
        worker = self.root / "kindle-jpeg-worker"
        application.write_bytes(b"Selected application fixture, never executed.\n")
        worker.write_bytes(b"Selected JPEG worker fixture, never executed.\n")
        members = {name: b"Fixture placeholder.\n" for name in ALLOWED_FILES}
        members.update(self.notices)
        members["bilicomics/app/bilicomics"] = application.read_bytes()
        members["bilicomics/app/kindle-jpeg-worker"] = worker.read_bytes()
        members["notices/spatial-dither/README.md"] = b"Replaced provenance with a recomputed ZIP manifest.\n"
        self.write_archive(members, with_manifest=True)
        arguments = SimpleNamespace(archive=self.archive, binary=application, worker=worker,
                                    app_root=self.source, output=self.root / "verification.json",
                                    expected_binary_sha256=hashlib.sha256(application.read_bytes()).hexdigest(),
                                    expected_worker_sha256=hashlib.sha256(worker.read_bytes()).hexdigest(),
                                    expected_archive_sha256=None)
        # A missing production call must fail this test at a later unrelated
        # guard; mocked ELF success is never used to prove notice acceptance.
        with patch("verify_package.subprocess.run", side_effect=AssertionError("Unexpected ELF-tool invocation")):
            with self.assertRaisesRegex(ValueError, "spatial-dither notice differs from the frozen source"):
                check_package(arguments, {"checks": []})


if __name__ == "__main__":
    unittest.main()
