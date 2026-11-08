#!/usr/bin/env python3
"""Replace only the launcher in a selected development archive without rebuilding."""

import argparse
import copy
import datetime
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import stat
import struct
import sys
import zipfile

from run_suite import SOURCE_TREES, manifest_text
from verify_package import ALLOWED_FILES, EXECUTABLE_FILES, FIXED_TIMESTAMP


LAUNCHER = "bilicomics/launch.sh"
BINARY = "bilicomics/app/bilicomics"
MAX_ENTRY_BYTES = 128 * 1024 * 1024
MAX_TOTAL_BYTES = 256 * 1024 * 1024


def require(condition, message):
    if not condition:
        raise ValueError(message)


def absolute_path(value):
    path = Path(value)
    if not path.is_absolute():
        raise argparse.ArgumentTypeError("An absolute path is required")
    return path


def digest_argument(value):
    if re.fullmatch(r"[0-9a-fA-F]{64}", value) is None:
        raise argparse.ArgumentTypeError("A full SHA-256 digest is required")
    return value.lower()


def digest(value):
    return hashlib.sha256(value).hexdigest()


def stream_digest(stream):
    stream.seek(0)
    value = hashlib.sha256()
    for block in iter(lambda: stream.read(1024 * 1024), b""):
        value.update(block)
    stream.seek(0)
    return value.hexdigest()


def regular_input(path, maximum=None):
    descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK | os.O_CLOEXEC)
    try:
        metadata = os.fstat(descriptor)
        require(stat.S_ISREG(metadata.st_mode), "An input is not a regular file")
        if maximum is not None:
            require(metadata.st_size <= maximum, "An input exceeds its size bound")
        return os.fdopen(descriptor, "rb")
    except BaseException:
        os.close(descriptor)
        raise


def read_regular(path, maximum):
    with regular_input(path, maximum) as stream:
        value = stream.read(maximum + 1)
    require(len(value) <= maximum, "An input grew beyond its size bound")
    return value


def source_pairs(value):
    result = {}
    for line in value.splitlines():
        match = re.fullmatch(r"([0-9a-f]{64})  (.+)", line)
        require(match is not None, "The binary source manifest has an invalid record")
        identity, name = match.groups()
        path = PurePosixPath(name)
        require(not path.is_absolute() and ".." not in path.parts and "\\" not in name
                and path.as_posix() == name and name not in result,
                "The binary source manifest has an unsafe or duplicate path")
        result[name] = identity
    require(bool(result), "The binary source manifest is empty")
    return result


def launcher_bytes(source):
    value = read_regular(source / "packaging/kindle/launch.sh", 512 * 1024).replace(b"\r\n", b"\n")
    require(value.startswith(b"#!/bin/sh\n") and value.endswith(b"\n")
            and b"\r" not in value and b"\0" not in value,
            "The selected launcher is not a normalized LF POSIX shell script")
    return value


def check_arm_header(value):
    require(len(value) >= 52 and value[:7] == b"\x7fELF\x01\x01\x01",
            "The selected embedded binary is not ELF32 little-endian version 1")
    require(struct.unpack_from("<H", value, 18)[0] == 40, "The selected embedded binary is not ARM")
    flags = struct.unpack_from("<I", value, 36)[0]
    require(flags >> 24 == 5 and flags & 0x400 and not flags & 0x200,
            "The selected embedded binary is not ARM EABI5 hard-float")


def base_entries(stream, sidecar, expected_binary):
    require(sidecar.get("development_candidate") is True and sidecar.get("device_verified") is False,
            "The base sidecar must identify an unverified development candidate")
    records = sidecar.get("files")
    require(isinstance(records, list) and len(records) == len(ALLOWED_FILES),
            "The base sidecar does not contain the exact entry count")
    require(all(isinstance(row, dict) and isinstance(row.get("path"), str) for row in records),
            "The base sidecar contains an invalid entry")
    names = [row["path"] for row in records]
    require(len(set(names)) == len(names) and set(names) == ALLOWED_FILES,
            "The base sidecar has duplicate, missing, or unexpected paths")
    indexed = {row["path"]: row for row in records}
    with zipfile.ZipFile(stream) as archive:
        infos = archive.infolist()
        actual = [info.filename for info in infos]
        require(len(actual) == len(set(actual)) == len(ALLOWED_FILES) and set(actual) == ALLOWED_FILES,
                "The base archive has duplicate, missing, or unexpected entries")
        require(all(0 <= info.file_size <= MAX_ENTRY_BYTES for info in infos)
                and sum(info.file_size for info in infos) <= MAX_TOTAL_BYTES,
                "The base archive exceeds its bounded payload size")
        for info in infos:
            mode = (info.external_attr >> 16) & 0xFFFF
            require(not info.is_dir() and stat.S_ISREG(mode) and not info.flag_bits & 1,
                    "A base archive entry is not an unencrypted regular file")
            require(info.date_time == FIXED_TIMESTAMP, "A base archive timestamp is not fixed")
            if info.filename in EXECUTABLE_FILES:
                require(stat.S_IMODE(mode) == 0o755, "A base executable mode is not 0755")
        require(archive.testzip() is None, "The base archive failed its CRC check")
        selected = {}
        for info in infos:
            row = indexed[info.filename]
            value = archive.read(info)
            require(type(row.get("bytes")) is int and len(value) == row["bytes"]
                    and row.get("sha256") == digest(value),
                    "A base archive entry differs from its sidecar")
            selected[info.filename] = (copy.copy(info), value)
    binary = selected[BINARY][1]
    require(digest(binary) == expected_binary, "The embedded binary differs from the selected ARM digest")
    check_arm_header(binary)
    return selected


def write_json(stream, value):
    stream.write((json.dumps(value, indent=2) + "\n").encode("utf-8"))
    stream.flush()
    os.fsync(stream.fileno())


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source-root", type=absolute_path, required=True)
    parser.add_argument("--base-archive", type=absolute_path, required=True)
    parser.add_argument("--expected-base-archive-sha256", type=digest_argument, required=True)
    parser.add_argument("--expected-binary-sha256", type=digest_argument, required=True)
    parser.add_argument("--binary-source-manifest", type=absolute_path, required=True)
    parser.add_argument("--output", type=absolute_path, required=True)
    parser.add_argument("--provenance-output", type=absolute_path, required=True)
    args = parser.parse_args()
    require(sys.platform == "linux" and os.environ.get("BILICOMICS_REMOTE_VERIFICATION") == "1",
            "Run only through the designated remote verification worker")
    os.umask(0o077)
    source = args.source_root.resolve(strict=True)
    require(source.is_dir() and not args.source_root.is_symlink(), "The source root is not a regular directory")
    output = args.output
    sidecar_output = output.with_suffix(".manifest.json")
    outputs = (output, sidecar_output, args.provenance_output)
    require(output.suffix == ".zip" and len({path.resolve() for path in outputs}) == len(outputs),
            "Archive, sidecar, and provenance outputs must be distinct new paths")
    for path in outputs:
        require(path.parent.is_dir() and not path.exists() and not path.is_symlink(),
                "Output parents must exist and output files must be new")
    manifest_raw = read_regular(args.binary_source_manifest, 2 * 1024 * 1024)
    previous_source = source_pairs(manifest_raw.decode("utf-8"))
    initial_manifest = manifest_text(source, SOURCE_TREES)
    current_source = source_pairs(initial_manifest)
    require(current_source == previous_source,
            "The complete source inputs differ from the selected binary build manifest")
    launcher = launcher_bytes(source)
    base_sidecar_path = args.base_archive.with_suffix(".manifest.json")
    base_sidecar_raw = read_regular(base_sidecar_path, 2 * 1024 * 1024)
    base_sidecar = json.loads(base_sidecar_raw)
    with regular_input(args.base_archive) as base:
        require(stream_digest(base) == args.expected_base_archive_sha256,
                "The base archive differs from its selected digest")
        entries = base_entries(base, base_sidecar, args.expected_binary_sha256)
        require(stream_digest(base) == args.expected_base_archive_sha256,
                "The pinned base archive changed during inspection")
    require(manifest_text(source, SOURCE_TREES) == initial_manifest and launcher_bytes(source) == launcher,
            "The frozen source or launcher changed during inspection")
    original_launcher = entries[LAUNCHER][1]
    entries[LAUNCHER] = (entries[LAUNCHER][0], launcher)
    new_manifest = {"development_candidate": True, "device_verified": False,
                    "files": [{"path": name, "bytes": len(value), "sha256": digest(value)}
                              for name, (_, value) in sorted(entries.items())]}
    # Exclusive creation never replaces an existing artifact. An I/O failure
    # can leave new partial files; those are not successful development bundles.
    with output.open("x+b") as bundle, sidecar_output.open("xb") as sidecar, args.provenance_output.open("xb") as provenance:
        with zipfile.ZipFile(bundle, "w") as archive:
            for name, (info, value) in sorted(entries.items()):
                archive.writestr(info, value)
        bundle.flush()
        os.fsync(bundle.fileno())
        bundle.seek(0)
        with zipfile.ZipFile(bundle) as archive:
            for name, (info, value) in entries.items():
                actual = archive.getinfo(name)
                require(archive.read(name) == value and actual.date_time == info.date_time
                        and actual.external_attr == info.external_attr,
                        "A rebundled entry differs from the selected bytes or retained metadata")
        require(manifest_text(source, SOURCE_TREES) == initial_manifest and launcher_bytes(source) == launcher,
                "The frozen source or launcher changed while writing the archive")
        archive_sha = stream_digest(bundle)
        write_json(sidecar, new_manifest)
        report = {
            "schema_version": 1, "passed": True,
            "completed_at_utc": datetime.datetime.now(datetime.timezone.utc).isoformat(),
            "operation": "replace_only_launcher_in_development_archive",
            "source_root": str(source), "source_file_count": len(current_source),
            "source_manifest_sha256": digest(initial_manifest.encode("utf-8")),
            "binary_source_manifest": str(args.binary_source_manifest),
            "binary_source_manifest_sha256": digest(manifest_raw),
            "complete_source_pairs_identical": True,
            "base_archive": str(args.base_archive),
            "base_archive_sha256": args.expected_base_archive_sha256,
            "base_sidecar_sha256": digest(base_sidecar_raw),
            "arm_binary_sha256": args.expected_binary_sha256,
            "binary_mapping": "Unchanged complete source inputs and identical embedded ARM bytes from the selected base archive",
            "changed_entry": LAUNCHER, "old_launcher_sha256": digest(original_launcher),
            "new_launcher_sha256": digest(launcher),
            "launcher_normalization": "CRLF converted to LF; bare CR and NUL rejected",
            "retained_entries_byte_identical": len(entries) - 1,
            "entry_modes_and_timestamps_preserved": True,
            "output_archive": str(output), "output_archive_sha256": archive_sha,
            "output_sidecar": str(sidecar_output),
            "output_sidecar_sha256": digest((json.dumps(new_manifest, indent=2) + "\n").encode("utf-8")),
            "build_performed": False, "arm_binary_executed": False,
            "physical_device_accessed": False, "device_verified": False,
            "followup": "Run verify_package.py on this exact archive and selected ARM binary before deployment",
        }
        write_json(provenance, report)
    print(json.dumps({"passed": True, "archive_sha256": archive_sha,
                      "unchanged_arm_binary_sha256": args.expected_binary_sha256,
                      "provenance": str(args.provenance_output)}))


if __name__ == "__main__":
    try:
        main()
    except Exception as error:
        print(json.dumps({"passed": False, "error": str(error),
                          "outputs": "Any newly created partial files are not successful bundles"}), file=sys.stderr)
        raise SystemExit(1)
