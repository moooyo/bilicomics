#!/usr/bin/env python3
"""Check a development archive and its ARM ELF without executing the program."""

import argparse
import datetime
import hashlib
import json
import os
from pathlib import Path
import re
import stat
import struct
import subprocess
import sys
import zipfile


ALLOWED_FILES = {
    "README.md",
    "THIRD_PARTY_NOTICES.md",
    "bilicomics/app/bilicomics",
    "bilicomics/launch.sh",
    "bilicomics/data/device-allowlist",
    "bilicomics/data/device-profile",
    "notices/Cargo.lock",
    "notices/backend/LICENSE-MIT",
    "notices/backend/LICENSE-APACHE",
    "notices/backend/UPSTREAM_REVISION",
    "notices/backend/PATCHES.md",
    "notices/fonts/startup-manifest.json",
    "notices/fonts/OFL.txt",
    "notices/fonts/NotoSansCJK-LICENSE.txt",
    "notices/fonts/NotoSerifCJK-LICENSE.txt",
    "notices/fonts/NOTICES.txt",
    "notices/pinyin/LICENSE-AOSP.txt",
    "notices/pinyin/README.md",
    "notices/protocol/README.md",
    "notices/protocol/wasm3-LICENSE",
    "notices/protocol/cjson-LICENSE",
    "notices/protocol/mbedtls-LICENSE",
    "notices/jpeg/LICENSE-APACHE",
    "notices/jpeg/LICENSE-MIT",
    "notices/jpeg/LICENSE-ZLIB",
    "notices/jpeg/UPSTREAM.md",
    "notices/jpeg/PATCHES.md",
    "notices/jpeg/0001-strict-entropy-completeness.patch",
}
EXECUTABLE_FILES = {"bilicomics/app/bilicomics", "bilicomics/launch.sh"}
FIXED_TIMESTAMP = (1980, 1, 1, 0, 0, 0)


def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def require(condition, message):
    if not condition:
        raise ValueError(message)


def record(report, name, **details):
    report["checks"].append({"name": name, "status": "passed", **details})


def check_package(args, report):
    archive_path = args.archive.resolve(strict=True)
    binary = args.binary.resolve(strict=True)
    app_root = args.app_root.resolve(strict=True)
    manifest_path = archive_path.with_suffix(".manifest.json")
    require(app_root.is_dir(), "The application source root is not a directory")
    require(archive_path.is_file() and binary.is_file(), "The archive and binary must be regular files")
    require(manifest_path.is_file() and not manifest_path.is_symlink(), "The archive manifest is unavailable or is a symlink")
    binary_digest = sha256(binary)
    archive_digest = sha256(archive_path)
    require(binary_digest == args.expected_binary_sha256.lower(), "The ARM binary SHA-256 differs from the selected input")
    if args.expected_archive_sha256:
        require(archive_digest == args.expected_archive_sha256.lower(), "The archive SHA-256 differs from the selected input")
    report["inputs"] = {
        "archive": str(archive_path),
        "archive_bytes": archive_path.stat().st_size,
        "archive_sha256": archive_digest,
        "manifest": str(manifest_path),
        "manifest_sha256": sha256(manifest_path),
        "binary": str(binary),
        "binary_bytes": binary.stat().st_size,
        "binary_sha256": binary_digest,
        "app_root": str(app_root),
    }
    record(report, "selected_input_digests", archive_digest_explicitly_selected=bool(args.expected_archive_sha256))

    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    require(manifest.get("development_candidate") is True and manifest.get("device_verified") is False,
            "The sidecar must identify an unverified development candidate")
    files = manifest.get("files")
    require(isinstance(files, list) and len(files) == len(ALLOWED_FILES), "The manifest does not match the exact allowed entry count")
    require(all(isinstance(item, dict) and isinstance(item.get("path"), str) for item in files),
            "The manifest contains an invalid entry")
    manifest_names = [item["path"] for item in files]
    require(len(set(manifest_names)) == len(files) and set(manifest_names) == ALLOWED_FILES,
            "The manifest has duplicate, missing, or unexpected paths")

    with zipfile.ZipFile(archive_path) as archive:
        names = archive.namelist()
        require(len(names) == len(set(names)) == len(ALLOWED_FILES) and set(names) == ALLOWED_FILES,
                "The archive has duplicate, missing, or unexpected paths")
        require(archive.testzip() is None, "The archive failed its CRC check")
        record(report, "archive_crc_and_exact_allowlist", entries=len(names), runtime_account_files_present=False)
        entries = []
        for item in files:
            name = item["path"]
            info = archive.getinfo(name)
            mode = (info.external_attr >> 16) & 0xFFFF
            require(not info.is_dir() and not stat.S_ISLNK(mode), f"The archive entry is not a regular file: {name}")
            require(info.date_time == FIXED_TIMESTAMP, f"The archive entry timestamp is not fixed: {name}")
            data = archive.read(name)
            digest = hashlib.sha256(data).hexdigest()
            require(len(data) == item.get("bytes") and digest == item.get("sha256"),
                    f"The archive entry differs from its manifest: {name}")
            if name in EXECUTABLE_FILES:
                require(stat.S_IMODE(mode) == 0o755, f"The executable mode is not 0755: {name}")
            entries.append({"path": name, "bytes": len(data), "sha256": digest,
                            "mode": oct(stat.S_IMODE(mode)), "fixed_timestamp": True})
        report["archive_entries"] = entries
        record(report, "entry_sizes_digests_timestamps_and_modes", executable_modes="0755")

        embedded = archive.read("bilicomics/app/bilicomics")
        require(len(embedded) == binary.stat().st_size and hashlib.sha256(embedded).hexdigest() == binary_digest,
                "The packaged executable differs from the selected ARM binary")
        record(report, "packaged_binary_matches_selected_binary")
        accepted = [line for line in archive.read("bilicomics/data/device-allowlist").decode("utf-8").splitlines()
                    if line.strip() and not line.lstrip().startswith("#")]
        require(not accepted, "The development archive unexpectedly accepts a device/firmware combination")
        record(report, "device_allowlist_empty", accepted_entries=0)

        startup_manifest = json.loads(archive.read("notices/fonts/startup-manifest.json"))
        startup_font = app_root / "assets/fonts/StartupSansSC.otf"
        output = startup_manifest.get("output", {})
        require(output.get("sha256") == sha256(startup_font) and output.get("bytes") == startup_font.stat().st_size,
                "The startup font differs from the packaged provenance")
        retained = startup_manifest.get("retained_license_files")
        require(isinstance(retained, dict) and retained, "The startup font has no retained license provenance")
        for name, identity in retained.items():
            data = archive.read("notices/fonts/" + name)
            require(isinstance(identity, dict) and len(data) == identity.get("bytes")
                    and hashlib.sha256(data).hexdigest() == identity.get("sha256"),
                    f"A retained startup-font notice differs from its provenance: {name}")
        record(report, "startup_font_and_notice_provenance", font_sha256=sha256(startup_font),
               font_bytes=startup_font.stat().st_size, retained_notices=sorted(retained))

        jpeg_notices = {
            "LICENSE-APACHE": "LICENSE-APACHE", "LICENSE-MIT": "LICENSE-MIT", "LICENSE-ZLIB": "LICENSE-ZLIB",
            "UPSTREAM.md": "UPSTREAM.md", "PATCHES.md": "PATCHES.md",
            "0001-strict-entropy-completeness.patch": "patches/0001-strict-entropy-completeness.patch",
        }
        for packaged, source in jpeg_notices.items():
            require(archive.read("notices/jpeg/" + packaged) == (app_root / "vendor/zune-jpeg" / source).read_bytes(),
                    f"A retained JPEG decoder notice differs from the selected source: {packaged}")
        record(report, "jpeg_decoder_notice_provenance", retained_files=sorted(jpeg_notices))

    with binary.open("rb") as stream:
        header = stream.read(64)
    require(len(header) >= 52 and header[:7] == b"\x7fELF\x01\x01\x01", "The executable is not ELF32 little-endian version 1")
    machine = struct.unpack_from("<H", header, 18)[0]
    flags = struct.unpack_from("<I", header, 36)[0]
    require(machine == 40, "The executable is not an ARM ELF")
    require(flags >> 24 == 5, "The executable does not declare ARM EABI5")
    require(flags & 0x400 and not flags & 0x200, "The executable does not declare the ARM hard-float ABI")
    record(report, "arm_elf_header", elf_class=32, byte_order="little", machine="ARM", eabi_version=5, hard_float=True)

    readelf_parts = []
    environment = os.environ.copy()
    environment["LC_ALL"] = "C"
    for option in ("-h", "-l", "-d"):
        command = ["readelf", option, str(binary)]
        completed = subprocess.run(command, capture_output=True, text=True, env=environment, timeout=30)
        readelf_parts.append("$ " + " ".join(command) + "\n" + completed.stdout + completed.stderr)
        require(completed.returncode == 0, f"readelf {option} failed: {completed.stderr.strip()}")
        if option == "-l":
            require(not re.search(r"^\s*INTERP\s", completed.stdout, re.MULTILINE), "The executable requests a dynamic interpreter")
        if option == "-d":
            require("(NEEDED)" not in completed.stdout, "The executable requires a shared library")
    readelf_path = args.output.with_suffix(".readelf.txt")
    with readelf_path.open("x", encoding="utf-8") as stream:
        stream.write("\n".join(readelf_parts))
    record(report, "no_dynamic_interpreter_or_required_shared_libraries", evidence=str(readelf_path),
           readelf_sha256=sha256(readelf_path))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--archive", type=Path, required=True)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--app-root", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True, help="A new JSON evidence file")
    parser.add_argument("--expected-binary-sha256", required=True)
    parser.add_argument("--expected-archive-sha256")
    args = parser.parse_args()
    for name in ("expected_binary_sha256", "expected_archive_sha256"):
        value = getattr(args, name)
        if value is not None and not re.fullmatch(r"[0-9a-fA-F]{64}", value):
            parser.error(f"--{name.replace('_', '-')} must be a SHA-256 hex digest")
    os.umask(0o077)
    args.output = args.output.resolve()
    args.output.parent.mkdir(parents=True, exist_ok=True)
    if args.output.exists() or args.output.with_suffix(".readelf.txt").exists():
        parser.error("The evidence paths already exist; select a new output file")
    report = {
        "schema_version": 1,
        "status": "failed",
        "passed": False,
        "started_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        "coverage": "Static development-archive and ARM ELF checks only",
        "build_executed_by_checker": False,
        "arm_binary_executed": False,
        "device_verified": False,
        "checks": [],
    }
    try:
        check_package(args, report)
        report.update(status="passed", passed=True)
    except Exception as error:
        report["error"] = str(error)
    report["finished_at"] = datetime.datetime.now(datetime.timezone.utc).isoformat()
    with args.output.open("x", encoding="utf-8") as stream:
        json.dump(report, stream, indent=2)
        stream.write("\n")
    print(json.dumps({"status": report["status"], "output": str(args.output), "error": report.get("error")}))
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    sys.exit(main())
