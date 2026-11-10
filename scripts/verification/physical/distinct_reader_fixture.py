#!/usr/bin/env python3
"""Generate and validate a bounded, synthetic, offline guest reader seed.

This utility prepares data only. It never launches the application or accesses a
device. Run verification on the designated remote test environment. The real
application must upgrade the version-zero receipts through complete decoding;
this seed is not an application validation proof. Launching it requires the
separate inherited AF_UNIX-only network guard.

Public API: generate_fixture(output, anchor_index=0) returns the manifest;
validate_fixture(directory) returns a machine-readable prelaunch report and
raises FixtureError on rejection. Validation intentionally rejects data after
the application has migrated or modified it.
"""

import argparse
import ast
from contextlib import closing
from functools import lru_cache
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import sqlite3
import stat
import struct
import sys
import zlib


KIND = "distinct_offline_guest_fixture"
FORMAT_VERSION = 1
ACCOUNT = "guest"
ACCOUNT_HASH = hashlib.sha256(ACCOUNT.encode("ascii")).hexdigest()
COMIC = "900000012"
EPISODE = "90000001201"
REVISION = "physical-distinct-reader-v1"
STAMP = 1_700_000_000
PAGE_COUNT = 12
WIDTH = 1700
HEIGHT = 2360
SCHEMA_VERSION = 2
DATABASE = "data/state.sqlite3"
PAGE_DIRECTORY = f"data/accounts/{ACCOUNT_HASH}/pages"
EXPECTED_DIRECTORIES = {"data", "data/accounts", f"data/accounts/{ACCOUNT_HASH}", PAGE_DIRECTORY}
MAX_PNG_BYTES = 8 * 1024 * 1024
MAX_DATABASE_BYTES = 2 * 1024 * 1024
MAX_MANIFEST_BYTES = 128 * 1024
MAX_TOTAL_BYTES = 64 * 1024 * 1024
MAX_FILES = PAGE_COUNT + 2
PNG_SIGNATURE = b"\x89PNG\r\n\x1a\n"
PATTERN_NAMES = (
    "vertical-bands", "horizontal-bands", "checkerboard",
    "ascending-diagonal-bands", "descending-diagonal-bands",
    "concentric-rectangles", "concentric-circles", "staggered-bricks",
    "diamond-rings", "radial-wedges", "wide-cross", "hourglass",
)
SEGMENTS = {
    "0": "abcedf", "1": "bc", "2": "abged", "3": "abgcd", "4": "fgbc",
    "5": "afgcd", "6": "afgecd", "7": "abc", "8": "abcdefg", "9": "abfgcd",
}


class FixtureError(ValueError):
    """The supplied fixture does not satisfy the prelaunch contract."""


def require(condition, message):
    if not condition:
        raise FixtureError(message)


def canonical_json(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=True, allow_nan=False)


def _json_object(pairs):
    result = {}
    for key, value in pairs:
        require(key not in result, "JSON contains a duplicate key")
        result[key] = value
    return result


def _invalid_constant(value):
    raise FixtureError("JSON contains a non-finite numeric constant")


def _parse_json(content):
    try:
        return json.loads(content, object_pairs_hook=_json_object, parse_constant=_invalid_constant)
    except (UnicodeError, json.JSONDecodeError, RecursionError) as error:
        raise FixtureError("The manifest is not bounded valid UTF-8 JSON") from error


@lru_cache(maxsize=1)
def _guest_schema():
    # Read the existing constant without importing the runtime fixture harness.
    source = Path(__file__).resolve().parents[1] / "reader_guest_fixtures.py"
    tree = ast.parse(source.read_text(encoding="utf-8"), filename=str(source))
    matches = [node for node in tree.body if isinstance(node, ast.Assign)
               and any(isinstance(target, ast.Name) and target.id == "SCHEMA" for target in node.targets)]
    require(len(matches) == 1, "The existing guest schema constant is ambiguous")
    schema = ast.literal_eval(matches[0].value)
    require(isinstance(schema, str), "The existing guest schema is not a literal string")
    return schema


def machine_marker(page_number):
    tones = [0] + [255 if page_number & (1 << bit) else 0 for bit in range(4)] + [255]
    return {
        "encoding": "binary-lsb-first-v1", "value": page_number, "block_radius": 48,
        "samples": [{"x": x, "y": 1820, "luma": tone}
                    for x, tone in zip((350, 550, 750, 950, 1150, 1350), tones)],
        "bit_sample_indices": [1, 2, 3, 4],
        "reference_sample_indices": {"black": 0, "white": 5},
    }


def _pattern(index):
    return {
        "geometry_version": 1, "name": PATTERN_NAMES[index], "page_number": index + 1,
        "dimensions": [WIDTH, HEIGHT], "digit_style": "seven-segment-v1",
        "palette_step": 17, "machine_marker": machine_marker(index + 1),
    }


def _rectangle(pixels, x, y, width, height, tone):
    require(0 <= x < x + width <= WIDTH and 0 <= y < y + height <= HEIGHT,
            "An internal artwork rectangle is out of bounds")
    row = bytes([tone]) * width
    for top in range(y, y + height):
        start = top * WIDTH + x
        pixels[start:start + width] = row


def _render_pixels(index):
    """Render integer-only geometry and font-independent large page numerals."""
    require(type(index) is int and 0 <= index < PAGE_COUNT, "The artwork index is invalid")
    pixels = bytearray(WIDTH * HEIGHT)
    x_values = range(WIDTH)
    centered_x = [2 * x - (WIDTH - 1) for x in x_values]
    x_squares = [value * value for value in centered_x]
    rows = {}
    for y in range(HEIGHT):
        cy = 2 * y - (HEIGHT - 1)
        if index == 0:
            key = 0
            if key not in rows:
                rows[key] = bytes(17 if x // 170 % 2 else 221 for x in x_values)
            row = rows[key]
        elif index == 1:
            row = bytes([34 if y // 236 % 2 else 238]) * WIDTH
        elif index == 2:
            key = y // 236 % 2
            if key not in rows:
                rows[key] = bytes(0 if (x // 170 + key) % 2 else 204 for x in x_values)
            row = rows[key]
        elif index in (3, 4):
            key = y % 360
            if key not in rows:
                sign = 1 if index == 3 else -1
                rows[key] = bytes(51 if ((x + sign * key) // 180) % 2 else 238 for x in x_values)
            row = rows[key]
        elif index == 5:
            row = bytes(17 if max(abs(cx) * HEIGHT // WIDTH, abs(cy)) // 280 % 2 else 187
                        for cx in centered_x)
        elif index == 6:
            # Squared-radius bands use only integer arithmetic.
            row = bytes(68 if (square + cy * cy) // 260_000 % 2 else 238 for square in x_squares)
        elif index == 7:
            key = (y // 236 % 2, y % 236 < 32)
            if key not in rows:
                rows[key] = bytes(255 if key[1] or (x + key[0] * 170) % 340 < 32
                                  else (34 if (x + key[0] * 170) // 340 % 2 else 153)
                                  for x in x_values)
            row = rows[key]
        elif index == 8:
            row = bytes((0, 119, 238)[(abs(cx) * HEIGHT // WIDTH + abs(cy)) // 340 % 3]
                        for cx in centered_x)
        elif index == 9:
            row = bytes((17 if cx < 0 else 204) if abs(cx) * HEIGHT > abs(cy) * WIDTH
                        else (68 if cy < 0 else 238) for cx in centered_x)
        elif index == 10:
            key = (abs(cy) < 280, cy < 0)
            if key not in rows:
                rows[key] = bytes(0 if key[0] or abs(cx) < 280 else (187 if key[1] else 255)
                                  for cx in centered_x)
            row = rows[key]
        else:
            row = bytes(34 if abs(cx) * HEIGHT < abs(cy) * WIDTH else 221 for cx in centered_x)
        pixels[y * WIDTH:(y + 1) * WIDTH] = row

    # The number panel occupies less than one quarter of the page area.
    _rectangle(pixels, 390, 750, 920, 860, 0)
    _rectangle(pixels, 402, 762, 896, 836, 255)
    segment_boxes = {
        "a": (0, 0, 330, 75), "b": (255, 0, 75, 350),
        "c": (255, 350, 75, 350), "d": (0, 625, 330, 75),
        "e": (0, 350, 75, 350), "f": (0, 0, 75, 350),
        "g": (0, 312, 330, 76),
    }
    for digit_index, digit in enumerate(f"{index + 1:02d}"):
        for segment in SEGMENTS[digit]:
            x, y, width, height = segment_boxes[segment]
            _rectangle(pixels, 470 + digit_index * 430 + x, 830 + y, width, height, 0)

    # Six large guarded samples remain readable after viewport scaling.
    _rectangle(pixels, 280, 1730, 1140, 180, 255)
    marker = machine_marker(index + 1)
    radius = marker["block_radius"]
    for sample in marker["samples"]:
        _rectangle(pixels, sample["x"] - radius, sample["y"] - radius,
                   radius * 2 + 1, radius * 2 + 1, sample["luma"])
    _rectangle(pixels, 0, 0, WIDTH, 16, 0)
    _rectangle(pixels, 0, HEIGHT - 16, WIDTH, 16, 0)
    _rectangle(pixels, 0, 0, 16, HEIGHT, 0)
    _rectangle(pixels, WIDTH - 16, 0, 16, HEIGHT, 0)
    return bytes(pixels)


def _png_chunk(kind, payload):
    return (struct.pack(">I", len(payload)) + kind + payload
            + struct.pack(">I", zlib.crc32(kind + payload) & 0xFFFFFFFF))


def _encode_png(pixels):
    require(len(pixels) == WIDTH * HEIGHT, "The internal pixel buffer has the wrong size")
    rows = b"".join(b"\0" + pixels[y * WIDTH:(y + 1) * WIDTH] for y in range(HEIGHT))
    return (PNG_SIGNATURE
            + _png_chunk(b"IHDR", struct.pack(">IIBBBBB", WIDTH, HEIGHT, 8, 0, 0, 0, 0))
            + _png_chunk(b"IDAT", zlib.compress(rows, 9)) + _png_chunk(b"IEND", b""))


@lru_cache(maxsize=PAGE_COUNT)
def _artwork(index):
    pixels = _render_pixels(index)
    return _encode_png(pixels), hashlib.sha256(pixels).hexdigest()


def _decode_png(content):
    """Decode the fixture's restricted PNG dialect with bounded inflation."""
    require(content.startswith(PNG_SIGNATURE), "A page does not have a PNG signature")
    chunks = []
    offset = len(PNG_SIGNATURE)
    while offset < len(content):
        require(len(chunks) < 3 and offset + 12 <= len(content), "A PNG has extra or truncated chunks")
        length = struct.unpack(">I", content[offset:offset + 4])[0]
        require(length <= MAX_PNG_BYTES and offset + 12 + length <= len(content), "A PNG chunk exceeds its bounds")
        kind = content[offset + 4:offset + 8]
        payload = content[offset + 8:offset + 8 + length]
        crc = struct.unpack(">I", content[offset + 8 + length:offset + 12 + length])[0]
        require(crc == zlib.crc32(kind + payload) & 0xFFFFFFFF, "A PNG chunk CRC is invalid")
        chunks.append((kind, payload))
        offset += length + 12
    require([kind for kind, _ in chunks] == [b"IHDR", b"IDAT", b"IEND"],
            "Only one IHDR, one IDAT, and one IEND are allowed")
    require(chunks[0][1] == struct.pack(">IIBBBBB", WIDTH, HEIGHT, 8, 0, 0, 0, 0),
            "A PNG is not the required 1700 by 2360 non-interlaced L8 image")
    require(not chunks[2][1], "The PNG IEND is not empty")
    expected_size = (WIDTH + 1) * HEIGHT
    inflater = zlib.decompressobj()
    try:
        rows = inflater.decompress(chunks[1][1], expected_size + 1)
    except zlib.error as error:
        raise FixtureError("A PNG IDAT stream is invalid") from error
    require(len(rows) == expected_size and inflater.eof and not inflater.unused_data
            and not inflater.unconsumed_tail, "A PNG decoded size or stream boundary is invalid")
    require(all(rows[y * (WIDTH + 1)] == 0 for y in range(HEIGHT)),
            "The fixture only permits unfiltered PNG rows")
    return b"".join(rows[y * (WIDTH + 1) + 1:(y + 1) * (WIDTH + 1)] for y in range(HEIGHT))


def _is_link(metadata):
    return (stat.S_ISLNK(metadata.st_mode)
            or bool(getattr(metadata, "st_file_attributes", 0) & 0x400))


def _absolute_unlinked_directory(path):
    path = Path(path)
    require(".." not in path.parts, "Directory traversal is not allowed")
    path = path.absolute()
    for component in [*reversed(path.parents), path]:
        metadata = component.lstat()
        require(not _is_link(metadata) and stat.S_ISDIR(metadata.st_mode),
                "Fixture directories and their ancestors must be real directories without links")
    return path


def _read_file(path, limit):
    flags = os.O_RDONLY | getattr(os, "O_BINARY", 0) | getattr(os, "O_NOFOLLOW", 0)
    descriptor = os.open(path, flags)
    with os.fdopen(descriptor, "rb") as stream:
        before = os.fstat(stream.fileno())
        require(not _is_link(before) and stat.S_ISREG(before.st_mode) and before.st_nlink == 1,
                "Fixture files must be regular files without symbolic or hard links")
        require(0 < before.st_size <= limit, "A fixture file is empty or exceeds its size limit")
        content = stream.read(limit + 1)
        after = os.fstat(stream.fileno())
    require(len(content) == before.st_size and
            (before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns, before.st_ctime_ns)
            == (after.st_dev, after.st_ino, after.st_size, after.st_mtime_ns, after.st_ctime_ns),
            "A fixture file changed while it was being read")
    return content


def _write_new(path, content):
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_BINARY", 0) | getattr(os, "O_NOFOLLOW", 0)
    descriptor = os.open(path, flags, 0o600)
    with os.fdopen(descriptor, "wb") as stream:
        stream.write(content)


def _inventory(root):
    files = {}
    directories = set()
    pending = [(root, "")]
    total = 0
    while pending:
        directory, prefix = pending.pop()
        with os.scandir(directory) as entries:
            for entry in entries:
                relative = prefix + entry.name
                require(relative != "data/account", "The credential account tree is forbidden before launch")
                metadata = entry.stat(follow_symlinks=False)
                require(not _is_link(metadata), "Fixture links and Windows reparse points are forbidden")
                if stat.S_ISDIR(metadata.st_mode):
                    require(relative in EXPECTED_DIRECTORIES, "The fixture contains an unexpected directory")
                    directories.add(relative)
                    pending.append((Path(entry.path), relative + "/"))
                else:
                    require(stat.S_ISREG(metadata.st_mode) and metadata.st_nlink == 1,
                            "Fixture objects must be regular files without hard links")
                    if relative == "manifest.json":
                        limit = MAX_MANIFEST_BYTES
                    elif relative == DATABASE:
                        limit = MAX_DATABASE_BYTES
                    else:
                        require(relative.startswith(PAGE_DIRECTORY + "/")
                                and re.fullmatch(r"[0-9a-f]{64}\.png", entry.name),
                                "The fixture contains an unexpected file")
                        limit = MAX_PNG_BYTES
                    require(0 < metadata.st_size <= limit, "A fixture file exceeds its permitted size")
                    files[relative] = (Path(entry.path), limit)
                    total += metadata.st_size
                    require(len(files) <= MAX_FILES and total <= MAX_TOTAL_BYTES,
                            "The fixture exceeds the file-count or total-byte limit")
    require(directories == EXPECTED_DIRECTORIES and len(files) == MAX_FILES,
            "The fixture directory or file inventory is incomplete")
    require("manifest.json" in files and DATABASE in files, "The fixture manifest or database is missing")
    return files, total


def _safe_relative(value):
    require(isinstance(value, str) and len(value) <= 256 and "\\" not in value and ":" not in value,
            "Manifest file names must be bounded POSIX relative paths")
    path = PurePosixPath(value)
    require(not path.is_absolute() and path.as_posix() == value and
            all(part not in ("", ".", "..") for part in path.parts), "A manifest path contains traversal")
    return value


def _page_record(index, encoded, pixel_digest):
    digest = hashlib.sha256(encoded).hexdigest()
    pattern = _pattern(index)
    return {
        "index": index, "page_index": index, "page_number": index + 1,
        "page_id": digest, "source_id": f"synthetic-distinct-{index + 1:02d}",
        "pattern": pattern, "pattern_signature": hashlib.sha256(canonical_json(pattern).encode("ascii")).hexdigest(),
        "file": f"{PAGE_DIRECTORY}/{digest}.png", "bytes": len(encoded),
        "encoded_sha256": digest, "pixel_sha256": pixel_digest,
        "width": WIDTH, "height": HEIGHT, "mode": "L", "machine_marker": machine_marker(index + 1),
    }


def _state(pages, anchor_index):
    comic = {
        "id": COMIC, "title": "Distinct Reader Fixture", "author": "Local fixture", "cover_url": "",
        "description": "Synthetic geometric pages for an isolated offline guest run", "genre": "",
        "latest_episode_id": EPISODE, "latest_episode_title": "Synthetic chapter", "latest_order": 1.0,
        "chapter_count": 1, "favorite": True, "finished": False, "updated_at": STAMP,
        "last_episode_id": EPISODE, "last_read_at": STAMP, "extra": {"fixture_only": True},
    }
    episode = {
        "id": EPISODE, "comic_id": COMIC, "title": "Synthetic chapter", "short_title": "", "order": 1.0,
        "access": "free", "expires_at": None, "pay_gold": None, "read": False,
        "extra": {"fixture_only": True, "page_count": PAGE_COUNT},
    }
    descriptors = [{"id": page["page_id"], "index": page["index"], "width": WIDTH, "height": HEIGHT}
                   for page in pages]
    detail = {"comic": comic, "episodes": [episode], "extra": {"fixture_only": True}}
    image_index = {"episode_id": EPISODE, "revision": REVISION, "images": descriptors,
                   "extra": {"fixture_only": True, "source_urls": []}}
    anchor = {
        "episode_id": EPISODE, "revision": REVISION, "page_id": pages[anchor_index]["page_id"],
        "page_index": anchor_index, "source_x": 0.0, "source_y": 0.0, "zoom_ratio": 1.0,
        "rotation": 0, "mode": "page", "finished": False, "updated_at": STAMP,
    }
    preferences = {
        "onboarded": True, "reader_guidance_seen": True, "preload": 3, "launch": "resume",
        "mode": "page", "fit": "page", "direction": "ltr", "dither": False,
        "landscape": "single", "cover_alone": True, "footer": True,
    }
    rows = {
        "comics": [(ACCOUNT, COMIC, canonical_json(comic), STAMP, STAMP)],
        "comic_details": [(ACCOUNT, COMIC, canonical_json(detail))],
        "catalog_members": [(ACCOUNT, "shelf", COMIC, 0)],
        "image_indices": [(ACCOUNT, EPISODE, REVISION, canonical_json(image_index), STAMP)],
        "reading_anchors": [(ACCOUNT, COMIC, EPISODE, REVISION, canonical_json(anchor), STAMP)],
        "preferences": [(ACCOUNT, "preferences", canonical_json(preferences))],
        "downloads": [], "page_receipts": [], "page_commit_journal": [], "page_identity_history": [],
    }
    for page in pages:
        receipt = {
            "episode_id": EPISODE, "revision": REVISION, "page_id": page["page_id"], "page_index": page["index"],
            "file_name": PurePosixPath(page["file"]).name, "sha256": page["encoded_sha256"], "bytes": page["bytes"],
            "width": WIDTH, "height": HEIGHT, "retained": True, "verified_at": 0, "decode_validation_version": 0,
        }
        rows["page_receipts"].append((ACCOUNT, EPISODE, REVISION, page["page_id"], page["index"], canonical_json(receipt)))
        rows["page_identity_history"].append((ACCOUNT, EPISODE, REVISION, page["page_id"],
                                              page["encoded_sha256"], page["bytes"]))
    return rows, anchor, preferences


def _file_record(relative, content):
    return {"path": relative, "bytes": len(content), "sha256": hashlib.sha256(content).hexdigest()}


def _manifest(pages, database_bytes, anchor_index):
    _, anchor, preferences = _state(pages, anchor_index)
    files = [_file_record(DATABASE, database_bytes)]
    files.extend({"path": page["file"], "bytes": page["bytes"], "sha256": page["encoded_sha256"]} for page in pages)
    files.sort(key=lambda value: value["path"])
    return {
        "kind": KIND, "format_version": FORMAT_VERSION, "account_key": ACCOUNT,
        "comic_id": COMIC, "episode_id": EPISODE, "revision": REVISION,
        "schema_version": SCHEMA_VERSION, "schema_source": "scripts/verification/reader_guest_fixtures.py:SCHEMA",
        "schema_sha256": hashlib.sha256(_guest_schema().encode("utf-8")).hexdigest(),
        "data_directory": "data", "database": DATABASE, "page_count": PAGE_COUNT,
        "initial_page_index": anchor_index, "initial_anchor": anchor, "initial_preferences": preferences,
        "image_dimensions": [WIDTH, HEIGHT], "image_mode": "L", "pages": pages, "files": files,
        "content_total_bytes": sum(value["bytes"] for value in files),
        "safety": {
            "classification": "fixture", "demo": False, "live_account": False,
            "session_seeded": False, "credentials_seeded": False, "transactions_seeded": False,
            "downloads_seeded": False, "network_source_urls": [], "source_response_simulated": False,
            "remote_writes_performed": False, "transport_enforcement": "external-launch-guard-required",
            "receipt_origin": "Synthetic guest seed; verified_at=0 and decode_validation_version=0 require real application decoding",
            "application_decode_proof_claimed": False,
        },
    }


def generate_fixture(output, *, anchor_index=0):
    """Write a new fixture below an existing real parent; never overwrite data."""
    require(type(anchor_index) is int and anchor_index in (0, 1), "The initial page index must be 0 or 1")
    output = Path(output)
    require(".." not in output.parts, "Directory traversal is not allowed")
    output = output.absolute()
    _absolute_unlinked_directory(output.parent)
    require(not os.path.lexists(output), "The output directory must be new")
    output.mkdir(mode=0o700)
    for directory in sorted(EXPECTED_DIRECTORIES, key=lambda value: (value.count("/"), value)):
        (output / directory).mkdir(mode=0o700)
    pages = []
    for index in range(PAGE_COUNT):
        encoded, pixel_digest = _artwork(index)
        page = _page_record(index, encoded, pixel_digest)
        require(len(encoded) <= MAX_PNG_BYTES, "Generated artwork exceeds the encoded page limit")
        _write_new(output / page["file"], encoded)
        pages.append(page)
    rows, _, _ = _state(pages, anchor_index)
    database = output / DATABASE
    with closing(sqlite3.connect(database, timeout=5)) as connection:
        connection.executescript(_guest_schema())
        with connection:
            for table, records in rows.items():
                for record in records:
                    connection.execute(f"INSERT INTO {table} VALUES({','.join('?' for _ in record)})", record)
        connection.execute("PRAGMA wal_checkpoint(TRUNCATE)")
        connection.execute("PRAGMA journal_mode=DELETE")
    database.chmod(0o600)
    manifest = _manifest(pages, _read_file(database, MAX_DATABASE_BYTES), anchor_index)
    _write_new(output / "manifest.json", (canonical_json(manifest) + "\n").encode("ascii"))
    return manifest


def _schema_rows(connection):
    return connection.execute("SELECT type,name,tbl_name,sql FROM sqlite_schema ORDER BY type,name").fetchall()


@lru_cache(maxsize=1)
def _expected_schema_rows():
    with closing(sqlite3.connect(":memory:")) as connection:
        connection.executescript(_guest_schema())
        return _schema_rows(connection)


def _validate_database(database, expected_bytes, pages, anchor_index):
    require(expected_bytes.startswith(b"SQLite format 3\0"), "The state file is not a SQLite database")
    expected_rows, _, _ = _state(pages, anchor_index)
    uri = database.as_uri() + "?mode=ro&immutable=1"
    with closing(sqlite3.connect(uri, uri=True, timeout=5)) as connection:
        connection.execute("PRAGMA query_only=ON")
        connection.execute("PRAGMA trusted_schema=OFF")
        budget = [0]

        def progress():
            budget[0] += 1
            return int(budget[0] > 2000)

        connection.set_progress_handler(progress, 1000)
        require(connection.execute("PRAGMA user_version").fetchone() == (SCHEMA_VERSION,),
                "The fixture must retain the original schema version 2 before launch")
        require(_schema_rows(connection) == _expected_schema_rows(),
                "The database schema contains an altered table, index, trigger, or view")
        require(connection.execute("PRAGMA quick_check").fetchall() == [("ok",)],
                "The fixture database failed its bounded integrity check")
        for table, expected in expected_rows.items():
            # Names come only from the fixed trusted schema, never from input JSON.
            actual = connection.execute(f"SELECT * FROM {table}").fetchmany(PAGE_COUNT + 1)
            require(len(actual) == len(expected) and set(actual) == set(expected),
                    f"The {table} rows differ from the exact guest-only seed")
    require(_read_file(database, MAX_DATABASE_BYTES) == expected_bytes,
            "The fixture database changed during validation")


def _validate_fixture(directory):
    root = _absolute_unlinked_directory(directory)
    inventory, total_bytes = _inventory(root)
    manifest_bytes = _read_file(root / "manifest.json", MAX_MANIFEST_BYTES)
    manifest = _parse_json(manifest_bytes)
    require(isinstance(manifest, dict), "The manifest root must be an object")
    anchor_index = manifest.get("initial_page_index")
    require(type(anchor_index) is int and anchor_index in (0, 1), "The initial page index must be 0 or 1")
    declared_pages = manifest.get("pages")
    declared_files = manifest.get("files")
    require(isinstance(declared_pages, list) and len(declared_pages) == PAGE_COUNT,
            "The manifest must describe exactly twelve pages")
    require(isinstance(declared_files, list) and len(declared_files) == PAGE_COUNT + 1,
            "The manifest must bind exactly the database and twelve page files")
    for record in declared_files:
        require(isinstance(record, dict), "A manifest file record is not an object")
        _safe_relative(record.get("path"))
    pages = []
    encoded_digests = set()
    pixel_digests = set()
    for index, declared in enumerate(declared_pages):
        require(isinstance(declared, dict), "A manifest page record is not an object")
        relative = _safe_relative(declared.get("file"))
        require(relative in inventory and relative.startswith(PAGE_DIRECTORY + "/"),
                "A page does not refer to an allowed guest content object")
        encoded = _read_file(*inventory[relative])
        pixels = _decode_png(encoded)
        pixel_digest = hashlib.sha256(pixels).hexdigest()
        require(pixel_digest == _artwork(index)[1], "A decoded page differs from its declared full-page pattern")
        page = _page_record(index, encoded, pixel_digest)
        require(canonical_json(declared) == canonical_json(page), "A page identity or artwork declaration differs")
        require(page["encoded_sha256"] not in encoded_digests and pixel_digest not in pixel_digests,
                "Encoded and decoded page identities must each be unique")
        encoded_digests.add(page["encoded_sha256"])
        pixel_digests.add(pixel_digest)
        pages.append(page)
    database_bytes = _read_file(root / DATABASE, MAX_DATABASE_BYTES)
    expected_manifest = _manifest(pages, database_bytes, anchor_index)
    require(canonical_json(manifest) == canonical_json(expected_manifest),
            "The manifest file bindings, preferences, or safety contract differ")
    expected_inventory = {"manifest.json", *[value["path"] for value in expected_manifest["files"]]}
    require(set(inventory) == expected_inventory, "The file inventory differs from the manifest")
    _validate_database(root / DATABASE, database_bytes, pages, anchor_index)
    final_inventory, final_total = _inventory(root)
    require(set(final_inventory) == set(inventory) and final_total == total_bytes
            and _read_file(root / "manifest.json", MAX_MANIFEST_BYTES) == manifest_bytes,
            "The fixture changed during validation")
    return {
        "status": "passed", "kind": KIND, "prelaunch": True, "fixture_directory": str(root),
        "manifest_sha256": hashlib.sha256(manifest_bytes).hexdigest(), "manifest": manifest,
        "file_count": len(inventory), "content_file_count": len(declared_files), "total_bytes": total_bytes,
        "content_total_bytes": expected_manifest["content_total_bytes"],
        "encoded_page_count": len(encoded_digests), "decoded_page_count": len(pixel_digests),
        "application_decode_proof_claimed": False, "external_network_guard_required": True,
    }


def validate_fixture(directory):
    """Reject any credential tree, extra data, schema change, or mismatched page."""
    try:
        return _validate_fixture(directory)
    except FixtureError:
        raise
    except (OSError, sqlite3.Error, ValueError, RecursionError) as error:
        raise FixtureError(f"Fixture validation failed: {error}") from error


class _Parser(argparse.ArgumentParser):
    def error(self, message):
        raise FixtureError(message)


def main(argv=None):
    try:
        parser = _Parser(description=__doc__)
        commands = parser.add_subparsers(dest="command", required=True, parser_class=_Parser)
        generate = commands.add_parser("generate", help="Create a new isolated guest fixture directory")
        generate.add_argument("--output", type=Path, required=True)
        generate.add_argument("--anchor-index", type=int, default=0, choices=(0, 1))
        validate = commands.add_parser("validate", help="Validate an untouched fixture before launch")
        validate.add_argument("directory", type=Path)
        args = parser.parse_args(argv)
        if args.command == "generate":
            generate_fixture(args.output, anchor_index=args.anchor_index)
            report = validate_fixture(args.output)
        else:
            report = validate_fixture(args.directory)
        report["operation"] = args.command
        print(canonical_json(report), flush=True)
        return 0
    except (FixtureError, OSError, sqlite3.Error, ValueError, RecursionError) as error:
        print(canonical_json({"status": "rejected", "error": str(error)}), flush=True)
        return 2


if __name__ == "__main__":
    sys.exit(main())
