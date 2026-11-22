#!/usr/bin/env python3
"""Exercise formal Library packaging guards only on ssh test-env.

Synthetic ARM headers exercise packaging identity and architecture guards; no
fixture is a linked application or an executed ARM binary. Full-package negative
cases stop before external ELF tools. Native catalog behavior remains separate.
"""

import hashlib
import json
import os
from pathlib import Path
import shutil
import stat
import struct
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch
import zipfile
import zlib

from rebundle_launcher import base_entries, rebundle
from run_suite import SOURCE_TREES, manifest_text
from verify_package import (ALLOWED_FILES, EXECUTABLE_FILES, FIXED_TIMESTAMP,
                            LEGACY_ALLOWED_FILES, LIBRARY_COVER, LIBRARY_COVER_SOURCE,
                            LIBRARY_DOCUMENT, LIBRARY_DOCUMENT_SOURCE, LIBRARY_ENTRY_COMMIT,
                            LIBRARY_ENTRY_NOTICES, LIBRARY_ENTRY_PREFIX,
                            LIBRARY_ENTRY_REFERENCE, LIBRARY_ENTRY_REVIEWED_MAIN,
                            LIBRARY_ENTRY_SOURCES, SPATIAL_DITHER_NOTICES,
                            SPATIAL_DITHER_PREFIX, SPATIAL_DITHER_REFERENCE,
                            STARTUP_NOTICES, STARTUP_PREFIX, STARTUP_REFERENCE,
                            STARTUP_SEQUENCE_NOTICES, STARTUP_SEQUENCE_PREFIX,
                            STARTUP_SEQUENCE_REFERENCE, STARTUP_SEQUENCE_SOURCES,
                            STARTUP_SOURCES, check_library_document,
                            check_library_entry_notices, check_package)


APPLICATION = "bilicomics/app/bilicomics"
JPEG_WORKER = "bilicomics/app/kindle-jpeg-worker"
LAUNCHER = "bilicomics/launch.sh"
LIBRARY_COMMAND = b"exec sh /mnt/us/bilicomics/launch.sh --library"
LIBRARY_ICON = b"# Icon: /mnt/us/bilicomics/assets/bilicomics-library-cover.png"
DOCUMENT_FIXTURE = (b"#!/bin/sh\n# Name: BiliComics for Kindle\n"
                    + LIBRARY_ICON + b"\n" + LIBRARY_COMMAND + b"\n")


def digest(value):
    return hashlib.sha256(value).hexdigest()


def arm_header_fixture(label, machine=40):
    value = bytearray(52)
    value[:7] = b"\x7fELF\x01\x01\x01"
    struct.pack_into("<H", value, 18, machine)
    struct.pack_into("<I", value, 36, (5 << 24) | 0x400)
    return bytes(value) + label


def png_fixture(width, height, *, bit_depth=8, color_type=0, transparency=b""):
    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))

    channels = 3 if color_type == 2 else 1
    row = b"\0" * (1 + (width * channels * bit_depth + 7) // 8)
    header = struct.pack(">IIBBBBB", width, height, bit_depth, color_type, 0, 0, 0)
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", header)
            + (chunk(b"tRNS", transparency) if transparency else b"")
            + chunk(b"IDAT", zlib.compress(row * height)) + chunk(b"IEND", b""))


class LibraryPackageFixture(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="bilicomics-library-package-")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.repository = Path(__file__).resolve().parents[2]
        self.source = self.root / "source"
        self.source.mkdir()
        self.archive = self.root / "candidate.zip"
        self.document_source = self.source / LIBRARY_DOCUMENT_SOURCE
        self.cover_source = self.source / LIBRARY_COVER_SOURCE
        self.write_source(LIBRARY_DOCUMENT_SOURCE, DOCUMENT_FIXTURE)
        self.write_source("src/main.rs", b"fn main() {}\n")
        for name in set(LIBRARY_ENTRY_NOTICES) | set(LIBRARY_ENTRY_SOURCES):
            self.copy_source(LIBRARY_ENTRY_REFERENCE + "/" + name)
        self.packaging_sources = {
            LAUNCHER: "packaging/kindle/launch.sh",
            "README.md": "packaging/kindle/README.md",
            "THIRD_PARTY_NOTICES.md": "THIRD_PARTY_NOTICES.md",
            LIBRARY_DOCUMENT: LIBRARY_DOCUMENT_SOURCE,
            LIBRARY_COVER: LIBRARY_COVER_SOURCE,
            **{LIBRARY_ENTRY_PREFIX + name: LIBRARY_ENTRY_REFERENCE + "/" + name
               for name in LIBRARY_ENTRY_NOTICES},
        }
        for name, relative in self.packaging_sources.items():
            if name != LIBRARY_DOCUMENT:
                self.copy_source(relative)
        self.packaging = {
            name: (self.source / relative).read_bytes().replace(b"\r\n", b"\n")
            if name in (LAUNCHER, LIBRARY_DOCUMENT)
            else (self.source / relative).read_bytes()
            for name, relative in self.packaging_sources.items()
        }
        self.notices = {LIBRARY_ENTRY_PREFIX + name:
                        (self.source / LIBRARY_ENTRY_REFERENCE / name).read_bytes()
                        for name in LIBRARY_ENTRY_NOTICES}
        self.application_bytes = arm_header_fixture(b"Selected application fixture.\n")
        self.worker_bytes = arm_header_fixture(b"Selected JPEG worker fixture.\n")

    def write_source(self, relative, content):
        path = self.source / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(content)
        return path

    def copy_source(self, relative):
        target = self.source / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(self.repository / relative, target)

    def write_archive(self, members, path=None, with_manifest=False, modes=None):
        path = path or self.archive
        modes = modes or {}
        records = []
        with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as archive:
            for name, content in sorted(members.items()):
                info = zipfile.ZipInfo(name, date_time=FIXED_TIMESTAMP)
                info.create_system = 3
                info.compress_type = zipfile.ZIP_DEFLATED
                mode = 0o755 if name in EXECUTABLE_FILES else 0o644
                info.external_attr = (stat.S_IFREG | modes.get(name, mode)) << 16
                archive.writestr(info, content)
                records.append({"path": name, "bytes": len(content), "sha256": digest(content)})
        sidecar = {"development_candidate": True, "device_verified": False, "files": records}
        if with_manifest:
            path.with_suffix(".manifest.json").write_text(json.dumps(sidecar) + "\n", encoding="utf-8")
        return sidecar

    def document_members(self, document=DOCUMENT_FIXTURE):
        return {LIBRARY_DOCUMENT: document, LIBRARY_COVER: self.packaging[LIBRARY_COVER]}

    def check_document(self, members=None):
        self.write_archive(self.document_members() if members is None else members)
        report = {"checks": []}
        with zipfile.ZipFile(self.archive) as archive:
            check_library_document(archive, self.source, report)
        return report

    def check_notices(self, members=None):
        self.write_archive(self.notices if members is None else members)
        report = {"checks": []}
        with zipfile.ZipFile(self.archive) as archive:
            check_library_entry_notices(archive, self.source, report)
        return report


class LibraryDocumentPackageTests(LibraryPackageFixture):
    def test_production_template_preserves_title_and_shared_handler_splash_policy(self):
        content = (self.repository / LIBRARY_DOCUMENT_SOURCE).read_bytes().replace(b"\r\n", b"\n")
        lines = content.splitlines()
        self.assertEqual(LIBRARY_DOCUMENT, "documents/bilicomics.sh")
        self.assertEqual([line for line in lines[:6] if line.startswith(b"# Name:")],
                         [b"# Name: BiliComics for Kindle"])
        self.assertEqual(lines[2], LIBRARY_ICON)
        self.assertEqual([line for line in lines if line.lstrip().lower().startswith(b"# icon:")],
                         [LIBRARY_ICON])
        self.assertNotIn(b"dontusefbink", content.lower())
        self.assertFalse(any(line.lstrip().lower().startswith(b"# author:") for line in lines))
        self.assertEqual(lines[-1], LIBRARY_COMMAND)
        self.assertEqual(lines.count(LIBRARY_COMMAND), 1)
        for line in lines:
            if line.lstrip().startswith(b"#"):
                continue
            with self.subTest(command=line):
                self.assertNotIn(b">", line)
                self.assertNotIn(b"<", line)

    def test_selected_thin_document_passes_with_a_windows_source_checkout(self):
        self.document_source.write_bytes(DOCUMENT_FIXTURE.replace(b"\n", b"\r\n"))
        report = self.check_document()
        check = report["checks"][0]
        self.assertEqual(check["name"], "formal_library_document_source")
        self.assertEqual(check["document"], LIBRARY_DOCUMENT)
        self.assertEqual(check["cover"], LIBRARY_COVER)
        self.assertEqual(check["cover_source"], LIBRARY_COVER_SOURCE)
        self.assertEqual(check["cover_dimensions"], [600, 800])
        self.assertEqual(check["argument"], "--library")
        self.assertIs(check["shared_handler_changed"], False)

    def test_archive_document_must_match_the_selected_lf_bytes(self):
        for content in (DOCUMENT_FIXTURE.replace(b"--library", b"--detached"),
                        DOCUMENT_FIXTURE.replace(b"\n", b"\r\n"),
                        DOCUMENT_FIXTURE + b"\n", DOCUMENT_FIXTURE + b"\0"):
            with self.subTest(content=content):
                with self.assertRaisesRegex(ValueError, "Library document differs from the selected thin-wrapper source"):
                    self.check_document(self.document_members(content))

    def test_source_cannot_rebind_the_entry_to_a_noncanonical_invocation(self):
        for content in (DOCUMENT_FIXTURE.replace(b"--library", b"--detached"),
                        DOCUMENT_FIXTURE + LIBRARY_COMMAND + b"\n",
                        DOCUMENT_FIXTURE + b"# Trailing command displacement.\n"):
            with self.subTest(content=content):
                self.document_source.write_bytes(content)
                with self.assertRaisesRegex(ValueError, "must invoke the canonical launcher with --library"):
                    self.check_document({LIBRARY_DOCUMENT: content})

    def test_source_with_bare_cr_or_nul_is_not_a_shell_document(self):
        for content in (DOCUMENT_FIXTURE.replace(b"# Name:", b"#\r Name:"),
                        DOCUMENT_FIXTURE.replace(b"# Name:", b"#\0 Name:")):
            with self.subTest(content=content):
                self.document_source.write_bytes(content)
                with self.assertRaisesRegex(ValueError, "not a normalized LF POSIX shell script"):
                    self.check_document({LIBRARY_DOCUMENT: content})

    def test_the_document_namespace_cannot_be_missing_or_contain_an_alias(self):
        cover = {LIBRARY_COVER: self.packaging[LIBRARY_COVER]}
        for members in (cover, {**self.document_members(), "documents/unexpected-entry.sh": DOCUMENT_FIXTURE}):
            with self.subTest(paths=sorted(members)):
                with self.assertRaisesRegex(ValueError, "exactly the selected Library document"):
                    self.check_document(members)

    def test_only_the_canonical_third_line_icon_declaration_is_accepted(self):
        for label, content in (
            ("missing", DOCUMENT_FIXTURE.replace(LIBRARY_ICON + b"\n", b"")),
            ("wrong path", DOCUMENT_FIXTURE.replace(LIBRARY_ICON, b"# Icon: /mnt/us/bilicomics/alternate.png")),
            ("data URI", DOCUMENT_FIXTURE.replace(LIBRARY_ICON, b"# Icon: data:image/png;base64,AAAA")),
            ("displaced", DOCUMENT_FIXTURE.replace(LIBRARY_ICON, b"\n" + LIBRARY_ICON)),
            ("duplicate", DOCUMENT_FIXTURE.replace(LIBRARY_ICON, LIBRARY_ICON + b"\n" + LIBRARY_ICON)),
            ("ambiguous duplicate", DOCUMENT_FIXTURE.replace(
                LIBRARY_ICON, LIBRARY_ICON + b"\n # iCoN : /tmp/alternate.png")),
        ):
            with self.subTest(label=label):
                self.document_source.write_bytes(content)
                with self.assertRaisesRegex(ValueError, "must declare exactly one canonical Icon header on line 3"):
                    self.check_document(self.document_members(content))

    def test_the_archive_cover_cannot_be_missing_or_staged_at_another_path(self):
        document = {LIBRARY_DOCUMENT: DOCUMENT_FIXTURE}
        for members in (document, {**document, "bilicomics/assets/alternate-cover.png": self.packaging[LIBRARY_COVER]}):
            with self.subTest(paths=sorted(members)):
                with self.assertRaisesRegex(ValueError, "must contain exactly one Library cover"):
                    self.check_document(members)

    def test_duplicate_cover_members_are_rejected(self):
        self.write_archive(self.document_members())
        with zipfile.ZipFile(self.archive, "a") as archive:
            with self.assertWarnsRegex(UserWarning, "Duplicate name"):
                archive.writestr(LIBRARY_COVER, self.packaging[LIBRARY_COVER])
        with zipfile.ZipFile(self.archive) as archive:
            with self.assertRaisesRegex(ValueError, "must contain exactly one Library cover"):
                check_library_document(archive, self.source, {"checks": []})

    def test_the_selected_cover_source_must_be_a_regular_file(self):
        self.cover_source.unlink()
        with self.assertRaisesRegex(ValueError, "selected Library cover source is unavailable"):
            self.check_document()
        self.cover_source.symlink_to(self.repository / LIBRARY_COVER_SOURCE)
        with self.assertRaisesRegex(ValueError, "selected Library cover source is unavailable"):
            self.check_document()

    def test_matching_source_and_archive_must_contain_an_opaque_grayscale_cover(self):
        for label, cover in (
            ("wrong size", png_fixture(800, 600)),
            ("wrong depth", png_fixture(600, 800, bit_depth=4)),
            ("RGB", png_fixture(600, 800, color_type=2)),
            ("grayscale with transparency", png_fixture(600, 800, transparency=b"\0\0")),
            ("truncated", self.packaging[LIBRARY_COVER][:-1]),
            ("not a PNG", b"Not a PNG.\n"),
        ):
            with self.subTest(label=label):
                self.cover_source.write_bytes(cover)
                members = {**self.document_members(), LIBRARY_COVER: cover}
                with self.assertRaisesRegex(ValueError, "Library cover must be an opaque 600 x 800 8-bit grayscale PNG"):
                    self.check_document(members)


class LibraryEntryNoticePackageTests(LibraryPackageFixture):
    def test_complete_selected_sources_and_original_license_pass(self):
        report = self.check_notices()
        check = report["checks"][0]
        self.assertEqual(check["name"], "library_entry_notice_provenance")
        self.assertEqual(check["source_commits"], [LIBRARY_ENTRY_COMMIT])
        self.assertEqual(check["reviewed_main_commit"], LIBRARY_ENTRY_REVIEWED_MAIN)
        self.assertEqual(check["source_snapshots"], len(LIBRARY_ENTRY_SOURCES) - 1)
        self.assertIs(check["source_code_bundled"], False)

    def test_the_notice_group_contains_only_the_three_distribution_records(self):
        expected = {LIBRARY_ENTRY_PREFIX + name for name in LIBRARY_ENTRY_NOTICES}
        self.assertEqual({name for name in ALLOWED_FILES if name.startswith(LIBRARY_ENTRY_PREFIX)}, expected)
        for missing in expected:
            with self.subTest(missing=missing):
                members = {name: content for name, content in self.notices.items() if name != missing}
                with self.assertRaisesRegex(ValueError, "Library-entry notice group is incomplete"):
                    self.check_notices(members)
        members = {**self.notices, LIBRARY_ENTRY_PREFIX + "upstream/launch.sh": b"Excluded source.\n"}
        with self.assertRaisesRegex(ValueError, "contains source inputs"):
            self.check_notices(members)

    def test_replacing_any_notice_is_rejected_despite_a_valid_zip_crc(self):
        for changed in self.notices:
            with self.subTest(changed=changed):
                members = {**self.notices, changed: b"Replaced provenance or license.\n"}
                with self.assertRaisesRegex(ValueError, "Library-entry notice differs from the frozen source"):
                    self.check_notices(members)

    def test_recomputed_records_cannot_rebind_the_reviewed_upstream_identity(self):
        path = self.source / LIBRARY_ENTRY_REFERENCE / "UPSTREAM_FILES.json"
        manifest = json.loads(path.read_bytes())
        manifest["reviewed_main_commit"] = "0" * 40
        content = (json.dumps(manifest, indent=2) + "\n").encode("utf-8")
        path.write_bytes(content)
        members = {**self.notices, LIBRARY_ENTRY_PREFIX + "UPSTREAM_FILES.json": content}
        with self.assertRaisesRegex(ValueError, "Library-entry provenance differs from the pinned source identities"):
            self.check_notices(members)

    def test_a_recomputed_snapshot_record_cannot_replace_the_pinned_source(self):
        path = self.source / LIBRARY_ENTRY_REFERENCE / "UPSTREAM_FILES.json"
        manifest = json.loads(path.read_bytes())
        item = next(item for item in manifest["files"] if item["path"].endswith("packaging/kindle/launch.sh"))
        content = b"Replaced retained launcher source.\n"
        (self.source / LIBRARY_ENTRY_REFERENCE / item["path"]).write_bytes(content)
        item["bytes"] = len(content)
        item["sha256"] = digest(content)
        manifest_content = (json.dumps(manifest, indent=2) + "\n").encode("utf-8")
        path.write_bytes(manifest_content)
        members = {**self.notices, LIBRARY_ENTRY_PREFIX + "UPSTREAM_FILES.json": manifest_content}
        with self.assertRaisesRegex(ValueError, "Library-entry source differs from its pinned bytes"):
            self.check_notices(members)


class LibraryFullPackageGuardTests(LibraryPackageFixture):
    def complete_members(self):
        members = {name: b"Package fixture placeholder.\n" for name in ALLOWED_FILES}
        members.update(self.packaging)
        for prefix, reference, notices, sources in (
            (SPATIAL_DITHER_PREFIX, SPATIAL_DITHER_REFERENCE, SPATIAL_DITHER_NOTICES, ()),
            (STARTUP_PREFIX, STARTUP_REFERENCE, STARTUP_NOTICES, STARTUP_SOURCES),
            (STARTUP_SEQUENCE_PREFIX, STARTUP_SEQUENCE_REFERENCE, STARTUP_SEQUENCE_NOTICES,
             STARTUP_SEQUENCE_SOURCES),
        ):
            for name in set(notices) | set(sources):
                self.copy_source(reference + "/" + name)
            members.update({prefix + name: (self.source / reference / name).read_bytes() for name in notices})
        members[APPLICATION] = self.application_bytes
        members[JPEG_WORKER] = self.worker_bytes
        members["bilicomics/data/device-allowlist"] = b"# No approved runtime pair.\n"
        return members

    def assert_full_checker_rejects(self, members, reason):
        application = self.root / "selected-application"
        worker = self.root / "selected-worker"
        application.write_bytes(self.application_bytes)
        worker.write_bytes(self.worker_bytes)
        self.write_archive(members, with_manifest=True)
        arguments = SimpleNamespace(archive=self.archive, binary=application, worker=worker,
                                    app_root=self.source, output=self.root / "verification.json",
                                    expected_binary_sha256=digest(self.application_bytes),
                                    expected_worker_sha256=digest(self.worker_bytes), expected_archive_sha256=None)
        with patch("verify_package.subprocess.run", side_effect=AssertionError("Unexpected ELF-tool invocation")):
            with self.assertRaisesRegex(ValueError, reason):
                check_package(arguments, {"checks": []})

    def test_full_checker_reaches_document_binding_before_external_elf_tools(self):
        members = self.complete_members()
        members[LIBRARY_DOCUMENT] = DOCUMENT_FIXTURE.replace(b"--library", b"--detached")
        self.assert_full_checker_rejects(members, "Library document differs from the selected thin-wrapper source")

    def test_full_checker_reaches_cover_binding_with_a_recomputed_zip_manifest(self):
        members = self.complete_members()
        members[LIBRARY_COVER] = png_fixture(600, 800)
        self.assert_full_checker_rejects(members, "Library cover differs from the selected source")

    def test_full_checker_reaches_notice_binding_with_a_recomputed_zip_manifest(self):
        members = self.complete_members()
        members[LIBRARY_ENTRY_PREFIX + "README.md"] = b"Replaced notice.\n"
        self.assert_full_checker_rejects(members, "Library-entry notice differs from the frozen source")

    def test_full_checker_does_not_pair_the_document_with_a_stale_launcher(self):
        members = self.complete_members()
        members[LAUNCHER] = b"#!/bin/sh\nexit 0\n"
        self.assert_full_checker_rejects(members, "canonical launcher differs from the selected source")


class LibraryEntryRebundleTests(LibraryPackageFixture):
    def setUp(self):
        super().setUp()
        self.base_archive = self.root / "base.zip"
        self.binary_manifest = self.root / "binary-source.sha256"
        self.binary_manifest.write_text(manifest_text(self.source, SOURCE_TREES), encoding="utf-8")

    def base_members(self, names=LEGACY_ALLOWED_FILES):
        members = {name: ("Retained fixture: " + name + "\n").encode("utf-8") for name in names}
        members[APPLICATION] = self.application_bytes
        members[JPEG_WORKER] = self.worker_bytes
        members["bilicomics/data/device-profile"] = b"scribe\n"
        members["bilicomics/data/device-allowlist"] = b"# No accepted firmware in a development bundle.\n"
        return members

    def arguments(self, members, modes=None):
        self.write_archive(members, path=self.base_archive, with_manifest=True, modes=modes)
        return SimpleNamespace(
            source_root=self.source, base_archive=self.base_archive,
            expected_base_archive_sha256=digest(self.base_archive.read_bytes()),
            expected_binary_sha256=digest(self.application_bytes), expected_worker_sha256=digest(self.worker_bytes),
            binary_source_manifest=self.binary_manifest,
            expected_binary_source_manifest_sha256=digest(self.binary_manifest.read_bytes()),
            output=self.archive, provenance_output=self.root / "rebundle.json",
        )

    def invoke_rebundle(self, arguments):
        previous_umask = os.umask(0o077)
        try:
            with patch.dict(os.environ, {"BILICOMICS_REMOTE_VERIFICATION": "1"}):
                return rebundle(arguments)
        finally:
            os.umask(previous_umask)

    def selected_base_entries(self, members, expected_worker=None, modes=None, sidecar=None):
        generated = self.write_archive(members, path=self.base_archive, modes=modes)
        with self.base_archive.open("rb") as stream:
            return base_entries(stream, generated if sidecar is None else sidecar,
                                digest(self.application_bytes),
                                digest(self.worker_bytes) if expected_worker is None else expected_worker)

    def assert_refreshed_package(self, members, schema):
        arguments = self.arguments(members, modes={"README.md": 0o640})
        with zipfile.ZipFile(self.base_archive) as base:
            original_metadata = {info.filename: (info.external_attr, info.date_time) for info in base.infolist()}
        report = self.invoke_rebundle(arguments)
        with zipfile.ZipFile(self.archive) as archive:
            self.assertEqual(set(archive.namelist()), ALLOWED_FILES)
            self.assertEqual(len(archive.namelist()), len(ALLOWED_FILES))
            output = {name: archive.read(name) for name in archive.namelist()}
            for name, metadata in original_metadata.items():
                with self.subTest(metadata=name):
                    info = archive.getinfo(name)
                    self.assertEqual((info.external_attr, info.date_time), metadata)
            for name in set(members) - set(self.packaging):
                with self.subTest(retained=name):
                    self.assertEqual(output[name], members[name])
            for name, content in self.packaging.items():
                self.assertEqual(output[name], content)
            document_mode = (archive.getinfo(LIBRARY_DOCUMENT).external_attr >> 16) & 0xFFFF
            self.assertEqual(stat.S_IMODE(document_mode), 0o755)
            cover_mode = (archive.getinfo(LIBRARY_COVER).external_attr >> 16) & 0xFFFF
            self.assertEqual(stat.S_IMODE(cover_mode), 0o644)
        self.assertEqual(report["base_schema"], schema)
        self.assertEqual(report["output_schema"], "library-entry-cover-v2")
        self.assertEqual(report["base_entry_count"], len(members))
        self.assertEqual(report["output_entry_count"], len(ALLOWED_FILES))
        self.assertEqual(report["added_entries"], sorted(set(self.packaging) - set(members)))
        self.assertEqual(report["updated_entries"], sorted(name for name, value in self.packaging.items()
                                                         if name in members and members[name] != value))
        self.assertEqual(report["retained_entries_byte_identical"],
                         sum(output[name] == value for name, value in members.items()))
        self.assertEqual(report["arm_binary_sha256"], digest(self.application_bytes))
        self.assertEqual(report["jpeg_worker_sha256"], digest(self.worker_bytes))
        self.assertIs(report["existing_entry_modes_and_timestamps_preserved"], True)
        self.assertIs(report["build_performed"], False)
        self.assertIs(report["arm_binary_executed"], False)
        self.assertIs(report["physical_device_accessed"], False)
        sidecar = json.loads(self.archive.with_suffix(".manifest.json").read_bytes())
        self.assertEqual(sidecar, {"development_candidate": True, "device_verified": False,
                                   "files": [{"path": name, "bytes": len(value), "sha256": digest(value)}
                                             for name, value in sorted(output.items())]})
        self.assertEqual(json.loads(arguments.provenance_output.read_bytes()), report)
        self.assertEqual(report["output_archive_sha256"], digest(self.archive.read_bytes()))

    def test_legacy_package_adds_the_document_cover_and_notices_without_rebuilding(self):
        self.assert_refreshed_package(self.base_members(), "pre-library-entry-v1")

    def test_current_schema_refreshes_selected_packaging_without_duplicate_documents(self):
        self.assert_refreshed_package(self.base_members(ALLOWED_FILES), "library-entry-cover-v2")

    def test_identical_current_packaging_counts_all_existing_entries_as_retained(self):
        members = self.base_members(ALLOWED_FILES)
        members.update(self.packaging)
        self.assert_refreshed_package(members, "library-entry-cover-v2")

    def test_partial_upgrades_are_not_a_supported_package_schema(self):
        notices = {LIBRARY_ENTRY_PREFIX + name for name in LIBRARY_ENTRY_NOTICES}
        for additions in ({LIBRARY_DOCUMENT}, {LIBRARY_COVER}, notices, notices | {LIBRARY_DOCUMENT}):
            with self.subTest(additions=sorted(additions)):
                members = self.base_members(LEGACY_ALLOWED_FILES | additions)
                with self.assertRaisesRegex(ValueError, "not an exact supported package schema"):
                    self.selected_base_entries(members)

    def test_archive_and_sidecar_cannot_select_different_supported_schemas(self):
        sidecar = self.write_archive(self.base_members(), path=self.base_archive)
        with self.assertRaisesRegex(ValueError, "base archive has duplicate, missing, or unexpected entries"):
            self.selected_base_entries(self.base_members(ALLOWED_FILES), sidecar=sidecar)

    def test_worker_digest_is_independent_of_the_selected_application(self):
        members = self.base_members()
        members[JPEG_WORKER] = arm_header_fixture(b"Replacement JPEG worker.\n")
        with self.assertRaisesRegex(ValueError, "embedded JPEG worker differs from the selected ARM digest"):
            self.selected_base_entries(members)

    def test_selected_worker_digest_does_not_bypass_its_arm_header_guard(self):
        members = self.base_members()
        members[JPEG_WORKER] = arm_header_fixture(b"Non-ARM worker fixture.\n", machine=62)
        with self.assertRaisesRegex(ValueError, "embedded JPEG worker is not ARM"):
            self.selected_base_entries(members, expected_worker=digest(members[JPEG_WORKER]))

    def test_worker_must_retain_executable_permission_in_the_base_package(self):
        with self.assertRaisesRegex(ValueError, "base executable mode is not 0755"):
            self.selected_base_entries(self.base_members(), modes={JPEG_WORKER: 0o644})

    def test_binary_build_manifest_requires_its_independently_selected_digest(self):
        arguments = self.arguments(self.base_members())
        arguments.expected_binary_source_manifest_sha256 = "0" * 64
        with self.assertRaisesRegex(ValueError, "binary source manifest differs from its selected digest"):
            self.invoke_rebundle(arguments)

    def test_changed_runtime_source_cannot_reuse_the_selected_binary_manifest(self):
        arguments = self.arguments(self.base_members())
        self.write_source("src/main.rs", b"fn main() { panic!(\"Changed fixture.\"); }\n")
        with self.assertRaisesRegex(ValueError, "complete source inputs differ from the selected binary build manifest"):
            self.invoke_rebundle(arguments)

    def test_existing_outputs_are_never_replaced(self):
        arguments = self.arguments(self.base_members())
        self.archive.write_bytes(b"Existing output must survive.\n")
        with self.assertRaisesRegex(ValueError, "output files must be new"):
            self.invoke_rebundle(arguments)
        self.assertEqual(self.archive.read_bytes(), b"Existing output must survive.\n")
        self.assertFalse(arguments.provenance_output.exists())
        self.assertFalse(self.archive.with_suffix(".manifest.json").exists())


if __name__ == "__main__":
    unittest.main()
