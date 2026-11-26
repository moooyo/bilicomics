#!/usr/bin/env python3
"""Verify an ordinary Kindle development ZIP by direct byte comparison, without hashes."""

import argparse
import json
import os
from pathlib import Path
import re
import stat
import struct
import subprocess
import sys
import tomllib
import zipfile
import zlib

from verify_package import (
    ALLOWED_FILES, EXECUTABLE_FILES, FIXED_TIMESTAMP, LIBRARY_COVER, LIBRARY_COVER_SOURCE,
    LIBRARY_DOCUMENT, LIBRARY_DOCUMENT_SOURCE, LIBRARY_ENTRY_REFERENCE, SPATIAL_DITHER_REFERENCE,
    STARTUP_REFERENCE, STARTUP_SEQUENCE_REFERENCE, library_cover_has_expected_format, require,
)


LAUNCHER = "bilicomics/launch.sh"
ALLOWLIST = "bilicomics/data/device-allowlist"
PROFILE = "bilicomics/data/device-profile"
BINARIES = {"bilicomics/app/bilicomics", "bilicomics/app/kindle-jpeg-worker"}
SCRIPTS = {LAUNCHER, LIBRARY_DOCUMENT}
TEMPLATES = SCRIPTS | {ALLOWLIST, PROFILE, LIBRARY_COVER}
CHUNK_SIZE = 1024 * 1024

# Independently bind the builder's staged files to the selected source checkout.
SOURCE_FILES = {
    "README.md": "packaging/kindle/README.md",
    "THIRD_PARTY_NOTICES.md": "THIRD_PARTY_NOTICES.md",
    LAUNCHER: "packaging/kindle/launch.sh",
    ALLOWLIST: "packaging/kindle/device-allowlist.example",
    LIBRARY_DOCUMENT: LIBRARY_DOCUMENT_SOURCE,
    LIBRARY_COVER: LIBRARY_COVER_SOURCE,
    "notices/Cargo.lock": "Cargo.lock",
    "notices/protocol/README.md": "vendor/bili-protocol/README.md",
    "notices/protocol/wasm3-LICENSE": "vendor/bili-protocol/wasm3/LICENSE",
    "notices/protocol/cjson-LICENSE": "vendor/bili-protocol/cjson/LICENSE",
    "notices/protocol/mbedtls-LICENSE": "vendor/bili-protocol/legacy_v5/vendor/mbedtls/LICENSE",
    "notices/jpeg/0001-strict-entropy-completeness.patch":
        "vendor/zune-jpeg/patches/0001-strict-entropy-completeness.patch",
}
NOTICE_DIRS = {
    "backend": "vendor/slint-backend-kindle",
    "fonts": "assets/fonts",
    "pinyin": "assets/pinyin",
    "jpeg": "vendor/zune-jpeg",
    "image-triangle": "vendor/image-triangle-reference-0.25.10",
    "aidoku-native-media": "vendor/aidoku-native-media",
    "spatial-dither": SPATIAL_DITHER_REFERENCE,
    "aidoku-startup": STARTUP_REFERENCE,
    "aidoku-startup-sequence": STARTUP_SEQUENCE_REFERENCE,
    "aidoku-native-library-entry": LIBRARY_ENTRY_REFERENCE,
}


def input_directory(path):
    require(stat.S_ISDIR(path.lstat().st_mode), f"Not a plain input directory: {path}")
    return path.resolve(strict=True)


def source_file(root, relative):
    parts = Path(relative).parts
    path = root
    for part in parts[:-1]:
        path /= part
        require(stat.S_ISDIR(path.lstat().st_mode), f"Not a plain source directory: {path}")
    path /= parts[-1]
    require(stat.S_ISREG(path.lstat().st_mode), f"Not a regular source file: {path}")
    return path


def selected_source(app_root, release_dir, name):
    if name == PROFILE:
        return None  # The official staging script generates this template.
    if name in BINARIES:
        return source_file(release_dir, Path(name).name)
    if name in SOURCE_FILES:
        return source_file(app_root, SOURCE_FILES[name])
    _, group, leaf = name.split("/", 2)
    require(group in NOTICE_DIRS, f"No source binding for package member: {name}")
    return source_file(app_root, f"{NOTICE_DIRS[group]}/{leaf}")


def verify_version(app_root, archive_path):
    with source_file(app_root, "Cargo.toml").open("rb") as stream:
        package = tomllib.load(stream).get("package", {})
    version = package.get("version")
    require(package.get("name") == "bilicomics" and isinstance(version, str)
            and re.fullmatch(r"[A-Za-z0-9._-]+", version), "Invalid BiliComics Cargo package version")
    require(archive_path.name == f"bilicomics-kindle-{version}-dev.zip",
            "The archive filename does not match the BiliComics Cargo package version")
    with source_file(app_root, "Cargo.lock").open("rb") as stream:
        packages = tomllib.load(stream).get("package", [])
    require([p.get("version") for p in packages if p.get("name") == "bilicomics"] == [version],
            "Cargo.lock does not identify the selected BiliComics package version")
    return version


def staged_files(root):
    dirs = {parent.as_posix() for name in ALLOWED_FILES for parent in Path(name).parents}
    files = {}
    for path in root.rglob("*"):
        name = path.relative_to(root).as_posix()
        entry = path.lstat()
        if stat.S_ISDIR(entry.st_mode):
            require(name in dirs, f"Unexpected staging directory: {name}")
        else:
            require(stat.S_ISREG(entry.st_mode) and entry.st_nlink == 1,
                    f"The staging member is not a plain, unlinked file: {name}")
            files[name] = (path, entry.st_mode)
    require(files.keys() == ALLOWED_FILES, "Staging has missing or unexpected package members")
    return files


def stock_payload(name, data):
    if name in SCRIPTS:
        data = data.replace(b"\r\n", b"\n")
        require(data.startswith(b"#!/bin/sh\n") and data.endswith(b"\n") and b"\r" not in data and b"\0" not in data,
                f"Invalid POSIX shell package input: {name}")
    if name == LIBRARY_DOCUMENT:
        require(data.splitlines() == [b"#!/bin/sh", b"# Name: BiliComics for Kindle",
                                     f"# Icon: /mnt/us/{LIBRARY_COVER}".encode("ascii"),
                                     b"set -eu", b"exec sh /mnt/us/bilicomics/launch.sh --library"],
                "The Library document is not the unchanged stock card")
    elif name == LIBRARY_COVER:
        require(library_cover_has_expected_format(data), "The stock Library cover is not a 600 x 800 grayscale PNG")
    elif name == LAUNCHER:
        invocations = [line.lstrip() for line in data.splitlines()
                       if line.lstrip().startswith(b"BILICOMICS_LIFECYCLE_DIR=$LOCK_DIR ")]
        require(len(invocations) == 1 and re.fullmatch(
            rb'BILICOMICS_LIFECYCLE_DIR=\$LOCK_DIR BILICOMICS_LAUNCH_GENERATION=\$APP_GENERATION '
            rb'BILICOMICS_LAUNCH_TOKEN=\$LAUNCH_TOKEN "\$APP_DIR/bilicomics" --kindle '
            rb'--device "\$PROFILE" --data-dir "\$DATA_DIR" >&6 2>&1 3>&- 4>&- 5>&- 6>&- &', invocations[0]),
            "The canonical launcher does not use the ordinary application invocation")
    elif name == ALLOWLIST:
        require(all(not line.strip() or line.lstrip().startswith(b"#") for line in data.splitlines()),
                "The development allowlist must not admit any device/firmware pair")
    return data


def compare_member(archive, info, staged, source):
    name = info.filename
    staged_error = f"Staged bytes differ from the selected input: {name}"
    archive_error = f"ZIP bytes differ from the selected input: {name}"
    if name in TEMPLATES:
        raw = source.read_bytes() if source is not None else b"pw5\n"
        require(staged.read_bytes() == raw, staged_error)
        expected = stock_payload(name, raw)
        require(info.file_size == len(expected) and archive.read(info) == expected, archive_error)
    else:
        # Reading the member through EOF validates its ZIP CRC as it is compared.
        with source.open("rb") as expected, staged.open("rb") as copied, archive.open(info) as embedded:
            require(info.file_size == os.fstat(expected.fileno()).st_size, archive_error)
            for chunk in iter(lambda: expected.read(CHUNK_SIZE), b""):
                require(copied.read(len(chunk)) == chunk, staged_error)
                require(embedded.read(len(chunk)) == chunk, archive_error)
            require(not copied.read(1), staged_error)
            require(not embedded.read(1), archive_error)


def verify_arm_elf(path):
    with path.open("rb") as stream:
        header = stream.read(52)
    require(len(header) == 52 and header.startswith(b"\x7fELF\x01\x01\x01"),
            f"Not an ELF32 little-endian executable: {path}")
    require(struct.unpack_from("<HHI", header, 16) == (2, 40, 1) and struct.unpack_from("<I", header, 24)[0],
            f"Not an executable ARM ELF: {path}")
    flags, = struct.unpack_from("<I", header, 36)
    require(flags >> 24 == 5 and flags & 0x600 == 0x400, f"Not ARM EABI5 hard-float: {path}")
    result = subprocess.run(["readelf", "-W", "-l", "-d", "-A", str(path)], capture_output=True,
                            text=True, env={**os.environ, "LC_ALL": "C"}, timeout=30)
    require(result.returncode == 0 and not result.stderr.strip(), f"readelf failed for {path}: {result.stderr.strip()}")
    require(re.search(r"^\s*Tag_CPU_arch:[ \t]+v7$", result.stdout, re.MULTILINE)
            and re.search(r"^\s*Tag_CPU_arch_profile:[ \t]+Application$", result.stdout, re.MULTILINE)
            and re.search(r"^\s*Tag_ABI_VFP_args:[ \t]+VFP registers$", result.stdout, re.MULTILINE),
            f"Missing ARMv7-A hard-float attributes: {path}")
    require(re.search(r"^\s*LOAD\s", result.stdout, re.MULTILINE)
            and not re.search(r"^\s*(?:INTERP|DYNAMIC)\s", result.stdout, re.MULTILINE)
            and "(NEEDED)" not in result.stdout, f"Not a statically linked loadable ARM executable: {path}")
    return {"elf_class": 32, "byte_order": "little", "architecture": "ARMv7-A", "eabi": 5,
            "hard_float": True, "static": True}


def verify_payload(archive_path, app_root, staging, release_dir):
    require(stat.S_ISREG(archive_path.lstat().st_mode), f"Not a regular ZIP file: {archive_path}")
    archive_path = archive_path.resolve(strict=True)
    app_root, staging, release_dir = map(input_directory, (app_root, staging, release_dir))
    version = verify_version(app_root, archive_path)
    staged = staged_files(staging)
    members = []
    elf = {}
    with zipfile.ZipFile(archive_path) as archive:
        infos = archive.infolist()
        require([info.orig_filename for info in infos] == sorted(ALLOWED_FILES),
                "The ZIP has duplicate, missing, unexpected, or out-of-order package members")
        require(not archive.comment, "Unexpected ZIP archive comment")
        for info in infos:
            name = info.filename
            mode = info.external_attr >> 16
            require(info.create_system == 3 and stat.S_ISREG(mode) and not info.flag_bits
                    and not info.extra and not info.comment and not info.external_attr & 0xFFFF,
                    f"The ZIP member is not an ordinary, unencrypted regular file: {name}")
            require(info.date_time == FIXED_TIMESTAMP and info.compress_type == zipfile.ZIP_DEFLATED,
                    f"Unexpected ZIP timestamp or compression: {name}")
            path, staged_mode = staged[name]
            require(mode == staged_mode, f"ZIP mode differs from staging: {name}")
            if name in EXECUTABLE_FILES:
                require(stat.S_IMODE(mode) == 0o755, f"The executable mode is not 0755: {name}")
            if name == LIBRARY_COVER:
                require(stat.S_IMODE(mode) == 0o644, f"The Library cover mode is not 0644: {name}")
            source = selected_source(app_root, release_dir, name)
            compare_member(archive, info, path, source)
            if name in BINARIES:
                elf[name] = verify_arm_elf(source)
            members.append({"path": name, "bytes": info.file_size, "mode": f"{stat.S_IMODE(mode):04o}",
                            "source": str(source) if source is not None else "generated device profile"})
    return {
        "schema": "bilicomics-kindle-dev-byte-verification-v1",
        "status": "passed", "development_candidate": True, "device_verified": False,
        "version": version, "archive": str(archive_path), "archive_bytes": archive_path.stat().st_size,
        "member_count": len(members), "zip_crc_valid": True, "fixed_timestamp": list(FIXED_TIMESTAMP),
        "device_profile": "pw5", "admitted_device_firmware_pairs": 0,
        "files": members, "elf": elf,
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--archive", type=Path, required=True)
    parser.add_argument("--app-root", type=Path, required=True, help="The source checkout used for the build")
    parser.add_argument("--staging", type=Path, required=True, help="The official builder's kindle-package directory")
    parser.add_argument("--release-dir", type=Path, required=True, help="The selected ARM Cargo release output directory")
    parser.add_argument("--output", type=Path, required=True, help="A new JSON byte-verification receipt")
    args = parser.parse_args()
    os.umask(0o077)
    try:
        require(not args.output.exists() and not args.output.is_symlink(), "The byte receipt path already exists")
        report = verify_payload(args.archive, args.app_root, args.staging, args.release_dir)
        with args.output.open("x", encoding="utf-8") as stream:
            json.dump(report, stream, indent=2)
            stream.write("\n")
    except (OSError, ValueError, EOFError, zipfile.BadZipFile, zlib.error, subprocess.SubprocessError) as error:
        print(f"Development ZIP verification failed: {error}", file=sys.stderr)
        return 1
    print(json.dumps({"status": "passed", "version": report["version"], "members": report["member_count"],
                      "archive": report["archive"], "output": str(args.output.resolve())}))
    return 0


if __name__ == "__main__":
    sys.exit(main())
