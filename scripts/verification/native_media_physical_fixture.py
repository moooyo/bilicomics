#!/usr/bin/env python3
"""Prepare synthetic receipt-backed Reader data on test-env, without device I/O.

The seed is a real offline guest account, never DemoData. Its version-zero
receipts require the application to perform its normal immutable read, digest,
validation, and LocalPageResolver recheck. This tool neither opens codec devices
nor starts, deploys, or controls an application. Physical execution belongs to
the owning operator and requires the separate reviewed launcher/network guard.
"""

import argparse
from contextlib import closing
import hashlib
import io
import json
import math
from pathlib import Path
import re
import sqlite3
import struct
import sys

import reader_guest_fixtures as guest


KIND = "native_media_offline_guest_fixture"
CONTINUATION_VARIANT = "app_sqlite_continuation_v1"
GRAY_ONLY_VARIANT = "gray_only_production_v1"
EXACT_TUPLES_VARIANT = "gray_exact_tuples_v1"
INITIAL_MANIFEST_SHA256 = "419493db7dbe1103ea769411294261dd91dfd0422e41e2bf32e4e11737b2c29d"
INITIAL_DATABASE_SHA256 = "79c030c760972c87c969417a10a163997f7f068a1b6914b82f7af3e88696ac52"
GRAY_ONLY_MANIFEST_SHA256 = "1a0e972ef6a8f1e6252669c16cab02a12eebe525a583931269f7df8435e2bade"
CONTINUATION_SOURCE_MANIFEST_SHA256 = "ff860e5511934e04a0983de3c72268154d4a46dbb500afd93b4d84daa4aba6b6"
MEASURED_GRAY_TUPLES = ((1920, 2560, 1860, 2480), (1920, 1280, 1860, 1240))
ACCOUNT = "guest"
COMIC = "900000015"
EPISODE = "90000001501"
REVISION = "native-media-physical-v1"
MAX_CONTENT_BYTES = 64 * 1024 * 1024
MAX_PAGE_BYTES = 8 * 1024 * 1024
STAMP = 1_700_000_000
VIEWPORT = (1860, 2480)
FITS = ("original", "page", "height", "width")
Y4_KEYS = ("dither", "dither_method", "hardware_y4_available", "hardware_dither",
           "target_hardware_dither", "hardware_y4_requested")
ANCHOR_KEYS = ("comic_id", "episode_id", "revision", "page_id", "page_index", "crop_index",
              "source_x", "source_y", "zoom_ratio", "rotation", "core_mode", "fit", "direction")
WORKLOADS = (
    ("gray-large", 1920, 2560, "gray", None, False),
    ("color422-large", 1920, 2560, "color", 1, False),
    ("color444-large", 1920, 2560, "color", 0, False),
    ("color420-fallback", 1920, 2560, "color", 2, False),
    ("progressive-fallback", 1920, 2560, "gray", None, True),
    ("png-fallback", 1920, 2560, "png", None, False),
    ("wide-gray", 1920, 1280, "gray", None, False),
    ("tall-gray", 1280, 2560, "gray", None, False),
    ("small-gray", 100, 50, "gray", None, False),
    ("identity-gray", 1860, 2480, "gray", None, False),
)
SAFETY = {"demo": False, "live_account": False, "session_seeded": False, "credentials_seeded": False,
          "source_urls": [], "remote_writes": False, "downloads_seeded": False,
          "transport_enforcement": "external-launch-guard-required", "application_decode_proof_claimed": False}
GLOBAL_PREFERENCE_KEYS = {"mode", "fit", "direction", "every", "landscape", "cover_alone", "dither", "dither_method",
                          "jpeg_acceleration", "footer", "smart", "ahead", "concurrency", "preload", "wifi", "cache_limit",
                          "launch", "sleep_cover", "flashui", "orientation", "onboarded", "reader_guidance_seen"}
READER_PREFERENCE_KEYS = {"mode", "fit", "direction", "every", "landscape", "cover_alone", "dither", "dither_method", "jpeg_acceleration", "footer"}


def require(value, reason):
    if not value:
        raise ValueError(reason)


def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=True, allow_nan=False)


def digest(content):
    return hashlib.sha256(content).hexdigest()


def read_json(path):
    require(path.is_file() and not path.is_symlink() and path.stat().st_size <= 256 * 1024,
            "The selected JSON input is not a bounded regular file")
    return json.loads(path.read_text(encoding="utf-8"))


def write_new(path, content):
    with path.open("xb") as stream:
        stream.write(content)
    path.chmod(0o600)


def rounded(value):
    return math.floor(value + 0.5)


def geometry(width, height, fit):
    """Independent expected metadata for the approved centered, zoom-one fits."""
    vw, vh = VIEWPORT
    crop = [0, 0, width, height]
    if fit == "page":
        target, origin, sampled = [vw, vh], [0.0, 0.0], False
    else:
        scale = {"original": min(1.0, vw / width, vh / height),
                 "height": vh / height, "width": vw / width}[fit]
        visible = [min(width, vw / scale), min(height, vh / scale)]
        sampled = scale > 1.0 and (visible[0] < width or visible[1] < height)
        if sampled:
            origin = [(width - visible[0]) / 2.0, (height - visible[1]) / 2.0]
            def snap(value):
                return float(rounded(value)) if abs(value - rounded(value)) <= 1e-9 else value
            left, top = (math.floor(snap(value)) for value in origin)
            right, bottom = (math.ceil(snap(origin[i] + visible[i])) for i in range(2))
            crop = [left, top, right - left, bottom - top]
            target = [min(vw, max(1, rounded(width * scale))), min(vh, max(1, rounded(height * scale)))]
        else:
            cw, ch = max(1, min(width, rounded(visible[0]))), max(1, min(height, rounded(visible[1])))
            crop = [(width - cw) // 2, (height - ch) // 2, cw, ch]
            origin = [float(crop[0]), float(crop[1])]
            target = [min(vw, max(1, rounded(cw * scale))), min(vh, max(1, rounded(ch * scale)))]
    downscale = (crop == [0, 0, width, height] and not sampled and target != [width, height]
                 and target[0] <= width and target[1] <= height and min(target) >= 16)
    return {"crop": crop, "target": target, "canvas": list(VIEWPORT), "sampled_crop": sampled,
            "origin": origin, "anchor": [origin[0] / width, origin[1] / height],
            "canvas_offset": [(VIEWPORT[i] - target[i]) // 2 for i in range(2)],
            "whole_source_downscale": downscale}


def jpeg_header(content):
    require(content.startswith(b"\xff\xd8"), "The fixture is not a JPEG")
    offset = 2
    while offset < len(content):
        require(content[offset] == 255, "Invalid synthetic JPEG marker")
        marker = content[offset + 1]
        length = struct.unpack(">H", content[offset + 2:offset + 4])[0]
        body = content[offset + 4:offset + 2 + length]
        if marker in (0xc0, 0xc2):
            return {"coding": "baseline" if marker == 0xc0 else "progressive", "precision": body[0],
                    "width": struct.unpack(">H", body[3:5])[0], "height": struct.unpack(">H", body[1:3])[0],
                    "components": [{"id": body[6 + i * 3], "h": body[7 + i * 3] >> 4,
                                    "v": body[7 + i * 3] & 15} for i in range(body[5])]}
        offset += 2 + length
    raise ValueError("The synthetic JPEG has no frame header")


def artwork(name, width, height, color):
    from PIL import Image, ImageDraw

    horizontal = Image.linear_gradient("L").resize((height, width)).transpose(Image.Transpose.ROTATE_90)
    vertical = Image.linear_gradient("L").resize((width, height))
    image = Image.merge("RGB", (horizontal, vertical, horizontal.transpose(Image.Transpose.FLIP_LEFT_RIGHT))) if color else vertical
    draw = ImageDraw.Draw(image)
    ink, paper = ((16, 16, 16), (245, 245, 245)) if color else (16, 245)
    edge = max(2, min(width, height) // 60)
    draw.rectangle((edge, edge, width - edge - 1, height - edge - 1), outline=ink, width=max(1, edge // 2))
    for band in range(16):
        left, right = band * width // 16, (band + 1) * width // 16 - 1
        tone = band * 17
        draw.rectangle((left, height // 8, right, height // 4), fill=(tone, tone, tone) if color else tone)
    if width >= 200:
        draw.rectangle((width // 12, height // 3, width * 5 // 12, height * 2 // 3), fill=paper, outline=ink, width=edge)
        draw.ellipse((width * 7 // 12, height // 3, width * 11 // 12, height * 2 // 3), fill=paper, outline=ink, width=edge)
        for line in range(24):
            y = height * 3 // 4 + line * max(2, height // 160)
            draw.line((width // 10, y, width * 9 // 10, y), fill=ink, width=1 + line % 4)
        draw.text((edge * 2, edge * 2), "Synthetic offline fixture: " + name, fill=ink, stroke_width=1)
    return image


def encode_workload(spec):
    name, width, height, kind, subsampling, progressive = spec
    image = artwork(name, width, height, kind == "color")
    output = io.BytesIO()
    if kind == "png":
        image.save(output, format="PNG", optimize=False, compress_level=6)
        header, extension = {"coding": "png", "mode": image.mode, "width": width, "height": height}, "png"
    else:
        options = {"format": "JPEG", "quality": 88, "optimize": False, "progressive": progressive}
        if subsampling is not None:
            options["subsampling"] = subsampling
        image.save(output, **options)
        header, extension = jpeg_header(output.getvalue()), "jpg"
        require(len(output.getvalue()) <= 2 * 1024 * 1024, "Synthetic JPEG exceeds the qualified encoded budget")
        expected = [(1, 1)] if kind == "gray" else [[(1, 1), (2, 1), (2, 2)][subsampling], (1, 1), (1, 1)]
        require([(v["h"], v["v"]) for v in header["components"]] == expected, "JPEG encoder changed the requested sampling")
    return output.getvalue(), extension, header


def state_rows(pages, dithering):
    comic = {"id": COMIC, "title": "Native Media Synthetic Fixture", "author": "Local fixture", "cover_url": "",
             "description": "Generated shapes and gradients; no acquired content", "genre": "", "latest_episode_id": EPISODE,
             "latest_episode_title": "Synthetic chapter", "latest_order": 1.0, "chapter_count": 1, "favorite": True,
             "finished": False, "updated_at": STAMP, "last_episode_id": EPISODE, "last_read_at": STAMP,
             "extra": {"fixture_only": True}}
    episode = {"id": EPISODE, "comic_id": COMIC, "title": "Synthetic chapter", "short_title": "", "order": 1.0,
               "access": "free", "expires_at": None, "pay_gold": None, "read": False,
               "extra": {"fixture_only": True, "page_count": len(pages)}}
    index = {"episode_id": EPISODE, "revision": REVISION,
             "images": [{"id": p["page_id"], "index": p["index"], "width": p["width"], "height": p["height"]} for p in pages],
             "extra": {"fixture_only": True, "source_urls": []}}
    anchor = {"episode_id": EPISODE, "revision": REVISION, "page_id": pages[0]["page_id"], "page_index": 0,
              "source_x": 0.0, "source_y": 0.0, "zoom_ratio": 1.0, "rotation": 0, "mode": "page", "finished": False, "updated_at": STAMP}
    preferences = {"onboarded": True, "reader_guidance_seen": True, "preload": 0, "launch": "resume", "mode": "page",
                   "fit": "page", "direction": "ltr", "landscape": "single", "cover_alone": True, "footer": True,
                   "dither": dithering != "off", "dither_method": "hardware" if dithering == "hardware" else "software",
                   "jpeg_acceleration": False, "orientation": "portrait"}
    rows = {"comics": [(ACCOUNT, COMIC, canonical(comic), STAMP, STAMP)],
            "comic_details": [(ACCOUNT, COMIC, canonical({"comic": comic, "episodes": [episode], "extra": {"fixture_only": True}}))],
            "catalog_members": [(ACCOUNT, "shelf", COMIC, 0)],
            "image_indices": [(ACCOUNT, EPISODE, REVISION, canonical(index), STAMP)],
            "reading_anchors": [(ACCOUNT, COMIC, EPISODE, REVISION, canonical(anchor), STAMP)],
            "preferences": [(ACCOUNT, "preferences", canonical(preferences)),
                            (ACCOUNT, "reader_preferences_" + COMIC, canonical(preferences))],
            "downloads": [], "page_commit_journal": [], "page_receipts": [], "page_identity_history": []}
    for page in pages:
        receipt = {"episode_id": EPISODE, "revision": REVISION, "page_id": page["page_id"], "page_index": page["index"],
                   "file_name": Path(page["file"]).name, "sha256": page["encoded_sha256"], "bytes": page["bytes"],
                   "width": page["width"], "height": page["height"], "retained": True, "verified_at": 0, "decode_validation_version": 0}
        rows["page_receipts"].append((ACCOUNT, EPISODE, REVISION, page["page_id"], page["index"], canonical(receipt)))
        rows["page_identity_history"].append((ACCOUNT, EPISODE, REVISION, page["page_id"], page["encoded_sha256"], page["bytes"]))
    return rows, anchor, preferences


def actions(sequence):
    return [{"at_ms": 8_000 + i * 12_000, "section": "reader", "action": action, "value": value}
            for i, (action, value) in enumerate(sequence)]


def action_sets():
    return {"data/media-actions.json": actions([("setting", "jpeg-acceleration:on"), ("source-page", "2"),
                ("source-page", "3"), ("source-page", "4"), ("source-page", "5"), ("source-page", "6"),
                ("source-page", "1"), ("setting", "fit:original"), ("setting", "fit:height"), ("setting", "fit:width"),
                ("source-page", "7"), ("setting", "fit:height"), ("source-page", "8"), ("setting", "fit:width"),
                ("source-page", "10")])}


def phase_plan(pages):
    schedule = ((1, "page", "combined", 1), (2, "page", "combined", 2), (3, "page", "combined", 3),
                (4, "page", "software", 3), (5, "page", "software", 3), (6, "page", "software", 3),
                (1, "page", "processed_cache", 3), (1, "original", "combined", 4), (1, "height", "combined", 5),
                (1, "width", "combined", 6), (7, "width", "combined", 7), (7, "height", "software", 7),
                (8, "height", "combined", 8), (8, "width", "software", 8), (10, "width", "software", 8))
    result = [{"phase": 0, "at_ms": 0, "page_number": 1, "page_name": pages[0]["name"], "fit": "page",
               "requested": False, "outcome": "software", "cumulative_native": {"attempts": 0, "completed": 0, "resized": 0}}]
    for i, (number, fit, outcome, count) in enumerate(schedule):
        result.append({"phase": i + 1, "at_ms": 8_000 + i * 12_000, "page_number": number, "page_name": pages[number - 1]["name"],
                       "fit": fit, "requested": True, "outcome": outcome,
                       "cumulative_native": {key: count for key in ("attempts", "completed", "resized")},
                       "geometry": pages[number - 1]["fits"][fit]})
    return result


def gray_only_actions():
    sequence = ((30000, "setting", "jpeg-acceleration:on"), (90000, "setting", "jpeg-acceleration:off"),
                (98000, "setting", "jpeg-acceleration:on"), (106000, "setting", "fit:original"),
                (114000, "setting", "fit:height"), (122000, "setting", "fit:width"),
                (130000, "source-page", "2"), (138000, "source-page", "3"), (146000, "source-page", "4"),
                (154000, "source-page", "5"), (162000, "source-page", "6"), (170000, "source-page", "7"),
                (180000, "setting", "fit:height"), (190000, "source-page", "8"),
                (200000, "setting", "fit:width"), (210000, "source-page", "9"), (220000, "source-page", "10"))
    return {"data/media-gray-actions.json": [{"at_ms": stamp, "section": "reader", "action": action, "value": value}
                                            for stamp, action, value in sequence]}


def gray_only_phase_plan(pages):
    schedule = ((0, 1, "page", False, "software", 0), (30000, 1, "page", True, "combined", 1),
                (90000, 1, "page", False, "software", 1), (98000, 1, "page", True, "processed_cache", 1),
                (106000, 1, "original", True, "combined", 2), (114000, 1, "height", True, "combined", 3),
                (122000, 1, "width", True, "combined", 4), (130000, 2, "width", True, "software", 4),
                (138000, 3, "width", True, "software", 4), (146000, 4, "width", True, "software", 4),
                (154000, 5, "width", True, "software", 4), (162000, 6, "width", True, "software", 4),
                (170000, 7, "width", True, "combined", 5), (180000, 7, "height", True, "software", 5),
                (190000, 8, "height", True, "combined", 6), (200000, 8, "width", True, "software", 6),
                (210000, 9, "width", True, "software", 6), (220000, 10, "width", True, "software", 6))
    return [{"phase": i, "at_ms": stamp, "page_number": number, "page_name": pages[number - 1]["name"],
             "fit": fit, "requested": requested, "outcome": outcome,
             "cumulative_native": {key: count for key in ("attempts", "completed", "resized")},
             "geometry": pages[number - 1]["fits"][fit]}
            for i, (stamp, number, fit, requested, outcome, count) in enumerate(schedule)]


def gray_only_pages():
    pages = [page_record(i, spec, *encode_workload(spec)) for i, spec in enumerate(WORKLOADS)]
    for page in pages:
        if page["header"].get("components") and len(page["header"]["components"]) != 1:
            page["qualified_input"] = False
            for plan in page["fits"].values():
                plan["expect_native_when_enabled"] = False
    return pages


def exact_tuple_actions():
    return {"data/media-tuple-actions.json": next(iter(gray_only_actions().values()))}


def exact_tuple_pages():
    pages = gray_only_pages()
    for page in pages:
        for plan in page["fits"].values():
            plan["expect_native_when_enabled"] = plan["expect_native_when_enabled"] and (
                page["width"], page["height"], *plan["target"]) in MEASURED_GRAY_TUPLES
        page["qualified_input"] = any(plan["expect_native_when_enabled"] for plan in page["fits"].values())
    return pages


def exact_tuple_phase_plan(pages):
    result = gray_only_phase_plan(pages)
    for phase in result:
        if phase["outcome"] == "combined" and not phase["geometry"]["expect_native_when_enabled"]:
            phase["outcome"] = "software"
        phase["cumulative_native"] = {key: min(count, 5) for key, count in phase["cumulative_native"].items()}
    return result


def restart_actions():
    return {"data/media-restart-actions.json": [
        {"at_ms": 8000, "section": "reader", "action": "source-page", "value": "1"},
        {"at_ms": 40000, "section": "reader", "action": "setting", "value": "jpeg-acceleration:off"},
    ]}


def restart_phase_plan(pages):
    return [{"phase": 0, "at_ms": 0, "page_number": 10, "page_name": pages[9]["name"], "fit": "width",
             "requested": True, "outcome": "software", "cumulative_native": {key: 0 for key in ("attempts", "completed", "resized")}},
            {"phase": 1, "at_ms": 8000, "page_number": 1, "page_name": pages[0]["name"], "fit": "width",
             "requested": True, "outcome": "combined", "cumulative_native": {key: 1 for key in ("attempts", "completed", "resized")}},
            {"phase": 2, "at_ms": 40000, "page_number": 1, "page_name": pages[0]["name"], "fit": "width",
             "requested": False, "outcome": "software", "cumulative_native": {key: 1 for key in ("attempts", "completed", "resized")}}]


def migrated_schema():
    """Use the repository's exact v2-to-v4 migration SQL, without opening App."""
    source = Path(__file__).resolve().parents[2] / "src/store.rs"
    text = source.read_text(encoding="utf-8")
    require(re.search(r"const SCHEMA_VERSION: i64 = 4;", text) is not None, "The application schema version changed")
    start = text.index("fn migrate_resource_schema(")
    match = re.search(r'transaction\.execute_batch\(\s*"(.*?)"\s*\)', text[start:], re.DOTALL)
    require(match is not None and "\\" not in match.group(1), "The resource migration SQL is not the reviewed literal")
    sql = "ALTER TABLE page_identity_history ADD COLUMN decode_validation_version INTEGER NOT NULL DEFAULT 0;\n" + match.group(1)
    return guest.SCHEMA + "\n" + sql + "\nPRAGMA user_version=4;"


def readonly_database(path):
    require(path.is_file() and not path.is_symlink() and 0 < path.stat().st_size <= 2 * 1024 * 1024, "The collected database is not bounded regular data")
    for suffix in ("-wal", "-shm", "-journal"):
        sidecar = Path(str(path) + suffix)
        require(not sidecar.exists() or sidecar.is_file() and not sidecar.is_symlink() and sidecar.stat().st_size == 0,
                "A nonempty SQLite sidecar prevents an exact stopped-database continuation")
    connection = sqlite3.connect(path.as_uri() + "?mode=ro&immutable=1", uri=True)
    try:
        connection.execute("PRAGMA query_only=ON")
        connection.execute("PRAGMA trusted_schema=OFF")
    except sqlite3.Error:
        connection.close()
        raise
    return connection


def validate_app_database(database, pages):
    """Read and retain actual App state; never promote receipts or write rows."""
    initial_rows, _, _ = state_rows(pages, "software")
    with closing(readonly_database(database)) as connection, closing(sqlite3.connect(":memory:")) as reference:
        reference.executescript(migrated_schema())
        query = "SELECT type,name,tbl_name,sql FROM sqlite_schema ORDER BY type,name"
        normalize = lambda rows: [(a, b, c, re.sub(r"\s+", " ", d).strip() if d else d) for a, b, c, d in rows]
        require(normalize(connection.execute(query).fetchall()) == normalize(reference.execute(query).fetchall()),
                "The collected database has an unexpected schema, table, index, trigger, or view")
        require(connection.execute("PRAGMA user_version").fetchone() == (4,) and connection.execute("PRAGMA quick_check").fetchall() == [("ok",)],
                "The collected database is not an intact App-migrated v4 database")
        for table in ("comics", "comic_details", "catalog_members", "image_indices"):
            require(sorted(connection.execute("SELECT * FROM " + table).fetchall()) == sorted(initial_rows[table]),
                    "The application database changed fixed synthetic content in " + table)
        for table in ("downloads", "page_commit_journal", "download_event_outbox", "sqlite_sequence"):
            require(connection.execute("SELECT COUNT(*) FROM " + table).fetchone() == (0,), "The continuation contains unexpected pending or transaction state")
        for (table,) in connection.execute("SELECT name FROM sqlite_schema WHERE type='table'"):
            columns = [row[1] for row in connection.execute("PRAGMA table_info(" + table + ")")]
            if "account_key" in columns:
                require(all(row[0] == ACCOUNT for row in connection.execute("SELECT DISTINCT account_key FROM " + table)), "The continuation escaped the guest account")
        preference_rows = connection.execute("SELECT key,payload FROM preferences WHERE account_key=?", (ACCOUNT,)).fetchall()
        preferences = {key: json.loads(value) for key, value in preference_rows}
        allowed = {"preferences", "reader_preferences_" + COMIC, "queued_download_intents", "queued_download_priorities",
                   "catalog_shelf_initialized", "catalog_cache_recency"}
        require(set(preferences) <= allowed and {"preferences", "reader_preferences_" + COMIC} <= set(preferences), "The continuation contains an unexpected preference or journal")
        require("queued_download_intents" not in preferences or preferences["queued_download_intents"] == [],
                "The continuation contains deferred download intents or an invalid Vec value")
        require("queued_download_priorities" not in preferences or preferences["queued_download_priorities"] == {},
                "The continuation contains deferred download priorities or an invalid map value")
        require("catalog_shelf_initialized" not in preferences or type(preferences["catalog_shelf_initialized"]) is bool,
                "The guest catalog flag is not a boolean")
        require("catalog_cache_recency" not in preferences or preferences["catalog_cache_recency"]
                == {"scopes": [], "comics": [COMIC], "details": [COMIC]}, "The catalog recency escaped the fixed synthetic comic")
        global_preferences = preferences["preferences"]
        saved = preferences["reader_preferences_" + COMIC]
        require(set(global_preferences) == GLOBAL_PREFERENCE_KEYS and set(saved) == READER_PREFERENCE_KEYS,
                "The persisted preference schema is not the reviewed App schema")
        for value in (global_preferences, saved):
            require(value.get("jpeg_acceleration") is True and value.get("fit") == "width" and value.get("mode") == "page"
                    and value.get("direction") == "ltr" and value.get("landscape") == "single", "The first App did not persist its final requested reader settings")
            require(type(value.get("dither")) is bool and value.get("dither_method") in ("software", "hardware"), "The persisted Y4 preference is invalid")
            require(all(type(value.get(key)) is bool for key in ("cover_alone", "footer")), "The saved reader boolean preferences are invalid")
            require(value.get("every") in ("smart", "0", "1", "6"), "The saved refresh preference is invalid")
        require(all(type(global_preferences.get(key)) is bool for key in ("smart", "sleep_cover", "flashui", "onboarded", "reader_guidance_seen"))
                and all(type(global_preferences.get(key)) is int and 0 <= global_preferences[key] <= 65536
                        for key in ("ahead", "concurrency", "preload", "cache_limit")), "The global preference scalar types are invalid")
        require(all(isinstance(global_preferences.get(key), str) and 0 < len(global_preferences[key]) <= 32
                    and all(character.isprintable() for character in global_preferences[key]) for key in ("wifi", "launch", "orientation")),
                "The global preference string types are invalid")
        require(global_preferences.get("preload") == 0 and global_preferences.get("launch") == "resume"
                and global_preferences.get("onboarded") is True and global_preferences.get("reader_guidance_seen") is True,
                "The continuation would change the controlled cold-start route or speculative policy")
        require((global_preferences["dither"], global_preferences["dither_method"]) == (saved["dither"], saved["dither_method"]),
                "Global and per-comic Y4 preferences differ")
        anchors = connection.execute("SELECT account_key,comic_id,episode_id,revision,payload,updated_at FROM reading_anchors").fetchall()
        require(len(anchors) == 1 and anchors[0][:4] == (ACCOUNT, COMIC, EPISODE, REVISION), "The collected database has an unexpected reading anchor")
        anchor = json.loads(anchors[0][4])
        expected = {"episode_id": EPISODE, "revision": REVISION, "page_id": pages[9]["page_id"], "page_index": 9,
                    "source_x": 0.0, "source_y": 0.0, "zoom_ratio": 1.0, "rotation": 0, "mode": "page", "finished": False}
        require(set(anchor) == set(expected) | {"updated_at"} and all(anchor.get(key) == value for key, value in expected.items())
                and type(anchor["updated_at"]) is int and anchor["updated_at"] > STAMP and anchors[0][5] == anchor["updated_at"],
                "The first App did not persist its actual final page-ten reading anchor")
        require(type(anchor["page_index"]) is int and type(anchor["rotation"]) is int and type(anchor["finished"]) is bool
                and all(type(anchor[key]) in (int, float) and math.isfinite(anchor[key]) for key in ("source_x", "source_y", "zoom_ratio")),
                "The collected anchor types cannot be read by the real App")
        receipts = connection.execute("SELECT account_key,episode_id,revision,page_id,page_index,payload FROM page_receipts ORDER BY page_index").fetchall()
        histories = connection.execute("SELECT account_key,episode_id,revision,page_id,sha256,bytes,decode_validation_version FROM page_identity_history ORDER BY page_id").fetchall()
        require(len(receipts) == len(pages) and len(histories) == len(pages), "The collected receipt inventory differs")
        histories = {row[3]: row for row in histories}
        versions = []
        for page, row in zip(pages, receipts):
            require(row[:5] == (ACCOUNT, EPISODE, REVISION, page["page_id"], page["index"]), "The collected receipt identity differs")
            receipt = json.loads(row[5])
            original = json.loads(initial_rows["page_receipts"][page["index"]][5])
            require(set(receipt) == set(original) and all(receipt[key] == value for key, value in original.items()
                if key not in ("verified_at", "decode_validation_version")), "A collected receipt differs from original encoded content or dimensions")
            version, stamp = receipt["decode_validation_version"], receipt["verified_at"]
            require(type(version) is int and version in (0, 1) and type(stamp) is int
                    and (stamp > STAMP if version else stamp == 0), "The collected application proof metadata is invalid")
            require(version == 1, "A first-batch displayed page lacks its actual App validation upgrade")
            require(histories.get(page["page_id"]) == (ACCOUNT, EPISODE, REVISION, page["page_id"], page["encoded_sha256"], page["bytes"], version),
                    "The retained history and actual receipt upgrade differ")
            versions.append(version)
    return {"initial_preferences": global_preferences, "initial_reader_preferences": saved, "initial_anchor": anchor,
            "receipt_validation_versions": versions, "schema_version": 4, "proofs_preserved_without_mutation": True}


def page_record(position, spec, content, extension, header):
    sha = digest(content)
    qualified = spec[3] != "png" and not spec[5] and spec[4] != 2 and spec[1] * spec[2] >= 256 * 256
    page = {"index": position, "number": position + 1, "name": spec[0], "page_id": sha, "encoded_sha256": sha,
            "file": "data/accounts/" + digest(ACCOUNT.encode("ascii")) + "/pages/" + sha + "." + extension,
            "bytes": len(content), "width": spec[1], "height": spec[2], "header": header, "qualified_input": qualified,
            "fits": {fit: geometry(spec[1], spec[2], fit) for fit in FITS}}
    for plan in page["fits"].values():
        plan["expect_native_when_enabled"] = qualified and plan["whole_source_downscale"]
    return page


def generate(output, dithering):
    require(output.is_absolute() and output.parent.is_dir() and not output.exists() and not output.is_symlink(),
            "The output must be a new absolute directory under an existing parent")
    require(output.parent.resolve() == output.parent and ".." not in output.parts, "The output path must be canonical")
    output.mkdir(mode=0o700)
    directory = "data/accounts/" + digest(ACCOUNT.encode("ascii")) + "/pages"
    for relative in ("data", "data/accounts", str(Path(directory).parent), directory):
        (output / relative).mkdir(mode=0o700)
    pages = []
    for position, spec in enumerate(WORKLOADS):
        content, extension, header = encode_workload(spec)
        page = page_record(position, spec, content, extension, header)
        write_new(output / page["file"], content)
        pages.append(page)
    rows, anchor, preferences = state_rows(pages, dithering)
    database = output / "data/state.sqlite3"
    with closing(sqlite3.connect(database)) as connection:
        connection.executescript(guest.SCHEMA)
        with connection:
            for table, records in rows.items():
                for record in records:
                    connection.execute("INSERT INTO " + table + " VALUES(" + ",".join("?" for _ in record) + ")", record)
        connection.execute("PRAGMA wal_checkpoint(TRUNCATE)")
        connection.execute("PRAGMA journal_mode=DELETE")
    database.chmod(0o600)
    scripts = action_sets()
    for name, value in scripts.items():
        write_new(output / name, (canonical(value) + "\n").encode("ascii"))
    files = [{"path": "data/state.sqlite3", "sha256": digest(database.read_bytes()), "bytes": database.stat().st_size}]
    files += [{"path": p["file"], "sha256": p["encoded_sha256"], "bytes": p["bytes"]} for p in pages]
    files += [{"path": name, "sha256": digest((canonical(value) + "\n").encode("ascii")),
               "bytes": len((canonical(value) + "\n").encode("ascii"))} for name, value in scripts.items()]
    manifest = {"kind": KIND, "format_version": 1, "account_key": ACCOUNT, "comic_id": COMIC, "episode_id": EPISODE,
                "revision": REVISION, "schema_version": 2, "database": "data/state.sqlite3", "data_directory": "data",
                "schema_source": "scripts/verification/reader_guest_fixtures.py:SCHEMA", "schema_sha256": digest(guest.SCHEMA.encode()),
                "logical_viewport": [930, 1240], "scale_factor": 2, "viewport": list(VIEWPORT),
                "page_count": len(pages), "initial_anchor": anchor, "initial_preferences": preferences, "pages": pages, "files": files,
                "launch_arguments": ["--offline", "--device", "scribe", "--screen", "reader", "--data-dir", "DATA"],
                "action_files": list(scripts), "phase_plan": phase_plan(pages),
                "action_clock": "Milliseconds after real application readiness; real runtime completion must be polled; no automatic exit",
                "device_scope": "Execution requires root's current exact device lease and native launcher; this tool does not deploy",
                "native_expectation": "Cold distinct target: attempts/completed/resized increase; cache reuse may have zero attempts",
                "fallback_expectation": "420/progressive/PNG/crop/upscale/identity: zero additional worker attempts",
                "safety": SAFETY}
    manifest["content_total_bytes"] = sum(record["bytes"] for record in files)
    write_new(output / "manifest.json", (canonical(manifest) + "\n").encode("ascii"))
    return validate(output)


def generate_gray_only(source_fixture, output):
    """Publish a new immutable safe-policy seed; preserve all historical inputs."""
    source_fixture, output = Path(source_fixture), Path(output)
    original = validate_fixture(source_fixture, INITIAL_MANIFEST_SHA256)["manifest"]
    require("variant" not in original, "Only the untouched historical r2 supplies original source images")
    require(output.is_absolute() and output.parent.is_dir() and output.parent.resolve() == output.parent
            and not output.exists() and not output.is_symlink() and ".." not in output.parts
            and not output.is_relative_to(source_fixture), "The Gray-only output must be a new independent canonical directory")
    pages = gray_only_pages()
    output.mkdir(mode=0o700)
    page_directory = str(Path(pages[0]["file"]).parent)
    for relative in ("data", "data/accounts", str(Path(page_directory).parent), page_directory):
        (output / relative).mkdir(mode=0o700)
    database_bytes = (source_fixture / "data/state.sqlite3").read_bytes()
    require(digest(database_bytes) == INITIAL_DATABASE_SHA256, "The original default-off database changed")
    write_new(output / "data/state.sqlite3", database_bytes)
    for page in pages:
        write_new(output / page["file"], (source_fixture / page["file"]).read_bytes())
    for name, value in gray_only_actions().items():
        write_new(output / name, (canonical(value) + "\n").encode("ascii"))
    files = [{"path": "data/state.sqlite3", "sha256": INITIAL_DATABASE_SHA256, "bytes": len(database_bytes)}]
    files += [{"path": page["file"], "sha256": page["encoded_sha256"], "bytes": page["bytes"]} for page in pages]
    files += [{"path": name, "sha256": digest((canonical(value) + "\n").encode("ascii")),
               "bytes": len((canonical(value) + "\n").encode("ascii"))} for name, value in gray_only_actions().items()]
    manifest = {**original, "variant": GRAY_ONLY_VARIANT, "hardware_policy": "gray_only_v2",
                "source_fixture_manifest_sha256": INITIAL_MANIFEST_SHA256,
                "pages": pages, "files": files, "action_files": list(gray_only_actions()),
                "phase_plan": gray_only_phase_plan(pages), "content_total_bytes": sum(value["bytes"] for value in files),
                "native_expectation": "Only baseline single-component Gray JPEG downscales may submit; a processed-cache hit does not resubmit",
                "fallback_expectation": "All color, progressive, PNG, crop, upscale, and identity cases add zero hardware attempts",
                "capture_window": {"first_enable_at_ms": 30000, "first_disable_at_ms": 90000,
                                   "nominal_native_stability_ms": 60000, "final_action_at_ms": 220000,
                                   "completion_requirement": "Poll actual reader-ready, native statistics, and kernel diagnostics; clocks are not acceptance"}}
    write_new(output / "manifest.json", (canonical(manifest) + "\n").encode("ascii"))
    return validate_fixture(output)


def generate_exact_tuples(source_fixture, output):
    """Publish the two measured tuples without rewriting r2, r3, or device evidence."""
    source_fixture, output = Path(source_fixture), Path(output)
    original = validate_fixture(source_fixture, GRAY_ONLY_MANIFEST_SHA256)["manifest"]
    require(original.get("variant") == GRAY_ONLY_VARIANT, "The exact-tuple source must be the frozen r3 corpus")
    require(output.is_absolute() and output.parent.is_dir() and output.parent.resolve() == output.parent
            and not output.exists() and not output.is_symlink() and ".." not in output.parts
            and not output.is_relative_to(source_fixture), "The exact-tuple output must be a new independent canonical directory")
    pages = exact_tuple_pages()
    output.mkdir(mode=0o700)
    page_directory = str(Path(pages[0]["file"]).parent)
    for relative in ("data", "data/accounts", str(Path(page_directory).parent), page_directory):
        (output / relative).mkdir(mode=0o700)
    database_bytes = (source_fixture / "data/state.sqlite3").read_bytes()
    require(digest(database_bytes) == INITIAL_DATABASE_SHA256, "The original default-off database changed")
    write_new(output / "data/state.sqlite3", database_bytes)
    for page in pages:
        write_new(output / page["file"], (source_fixture / page["file"]).read_bytes())
    for name, value in exact_tuple_actions().items():
        write_new(output / name, (canonical(value) + "\n").encode("ascii"))
    files = [{"path": "data/state.sqlite3", "sha256": INITIAL_DATABASE_SHA256, "bytes": len(database_bytes)}]
    files += [{"path": page["file"], "sha256": page["encoded_sha256"], "bytes": page["bytes"]} for page in pages]
    files += [{"path": name, "sha256": digest((canonical(value) + "\n").encode("ascii")),
               "bytes": len((canonical(value) + "\n").encode("ascii"))} for name, value in exact_tuple_actions().items()]
    manifest = {**original, "variant": EXACT_TUPLES_VARIANT, "hardware_policy": "gray_exact_tuples_v3",
                "source_fixture_manifest_sha256": GRAY_ONLY_MANIFEST_SHA256,
                "image_origin_manifest_sha256": INITIAL_MANIFEST_SHA256,
                "measured_gray_tuples": [list(value) for value in MEASURED_GRAY_TUPLES],
                "pages": pages, "files": files, "action_files": list(exact_tuple_actions()),
                "phase_plan": exact_tuple_phase_plan(pages), "content_total_bytes": sum(value["bytes"] for value in files),
                "native_expectation": "Only the two explicitly measured Gray source/target tuples may submit",
                "fallback_expectation": "Every other geometry, all color, progressive, PNG, crop, upscale, and identity cases add zero attempts"}
    write_new(output / "manifest.json", (canonical(manifest) + "\n").encode("ascii"))
    return validate_fixture(output)


def validate_gray_only(directory, manifest, expected_manifest_sha256):
    """Validate the new Gray-only plan without relaxing the historical r2 gate."""
    from PIL import Image

    manifest_sha = digest((directory / "manifest.json").read_bytes())
    require(expected_manifest_sha256 is None or manifest_sha == expected_manifest_sha256, "The pinned Gray-only manifest digest differs")
    exact = manifest.get("variant") == EXACT_TUPLES_VARIANT
    variant = EXACT_TUPLES_VARIANT if exact else GRAY_ONLY_VARIANT
    policy = "gray_exact_tuples_v3" if exact else "gray_only_v2"
    source_sha = GRAY_ONLY_MANIFEST_SHA256 if exact else INITIAL_MANIFEST_SHA256
    scripts = exact_tuple_actions() if exact else gray_only_actions()
    require(manifest.get("variant") == variant and manifest.get("hardware_policy") == policy
            and manifest.get("kind") == KIND and manifest.get("format_version") == 1, "Unknown Gray-only fixture policy")
    require((manifest.get("account_key"), manifest.get("comic_id"), manifest.get("episode_id"), manifest.get("revision"), manifest.get("page_count"))
            == (ACCOUNT, COMIC, EPISODE, REVISION, len(WORKLOADS)), "The Gray-only synthetic content identity differs")
    require(manifest.get("source_fixture_manifest_sha256") == source_sha and manifest.get("schema_version") == 2
            and manifest.get("viewport") == list(VIEWPORT) and manifest.get("safety") == SAFETY,
            "The Gray-only source, schema, viewport, or isolation contract differs")
    require(manifest.get("schema_sha256") == digest(guest.SCHEMA.encode())
            and manifest.get("database") == "data/state.sqlite3" and manifest.get("data_directory") == "data", "The Gray-only seed schema or data path changed")
    pages = exact_tuple_pages() if exact else gray_only_pages()
    phases = exact_tuple_phase_plan(pages) if exact else gray_only_phase_plan(pages)
    require(manifest.get("pages") == pages and manifest.get("action_files") == list(scripts)
            and manifest.get("phase_plan") == phases, "A Gray-only source, target, or fixed action plan changed")
    if exact:
        require(manifest.get("measured_gray_tuples") == [list(value) for value in MEASURED_GRAY_TUPLES]
                and manifest.get("image_origin_manifest_sha256") == INITIAL_MANIFEST_SHA256,
                "The explicit measured tuple evidence changed")
    require(manifest.get("capture_window") == {"first_enable_at_ms": 30000, "first_disable_at_ms": 90000,
            "nominal_native_stability_ms": 60000, "final_action_at_ms": 220000,
            "completion_requirement": "Poll actual reader-ready, native statistics, and kernel diagnostics; clocks are not acceptance"},
            "The minimum stable capture window changed")
    database = directory / "data/state.sqlite3"
    require(database.is_file() and not database.is_symlink() and database.stat().st_size <= 2 * 1024 * 1024
            and digest(database.read_bytes()) == INITIAL_DATABASE_SHA256, "The original default-off seed database changed")
    expected_files = [{"path": "data/state.sqlite3", "sha256": INITIAL_DATABASE_SHA256, "bytes": database.stat().st_size}]
    expected_files += [{"path": page["file"], "sha256": page["encoded_sha256"], "bytes": page["bytes"]} for page in pages]
    expected_files += [{"path": name, "sha256": digest((canonical(value) + "\n").encode("ascii")),
                        "bytes": len((canonical(value) + "\n").encode("ascii"))} for name, value in scripts.items()]
    require(manifest.get("files") == expected_files and manifest.get("content_total_bytes") == sum(v["bytes"] for v in expected_files)
            and manifest["content_total_bytes"] <= MAX_CONTENT_BYTES, "The Gray-only data inventory changed")
    expected_inventory = {"manifest.json", *(v["path"] for v in expected_files)}
    expected_directories = {str(parent) for v in expected_files for parent in Path(v["path"]).parents if str(parent) != "."}
    seen_files, seen = set(), 0
    for path in directory.rglob("*"):
        seen += 1
        require(seen <= len(expected_inventory) + len(expected_directories) and not path.is_symlink(), "The Gray-only tree exceeds its budget or contains symlinks")
        relative = path.relative_to(directory).as_posix()
        if path.is_file():
            seen_files.add(relative)
        else:
            require(path.is_dir() and relative in expected_directories, "Unexpected Gray-only directory or object")
    require(seen_files == expected_inventory, "The Gray-only fixture contains unexpected or missing files")
    for record in expected_files:
        path = directory / record["path"]
        require(path.is_file() and not path.is_symlink() and path.resolve().is_relative_to(directory)
                and path.stat().st_size == record["bytes"] <= MAX_PAGE_BYTES
                and digest(path.read_bytes()) == record["sha256"], "A frozen Gray-only data file changed")
    for name, value in scripts.items():
        require((directory / name).read_bytes() == (canonical(value) + "\n").encode("ascii"), "The fixed Gray-only intents changed")
    rows, anchor, preferences = state_rows(pages, "software")
    require(manifest.get("initial_preferences") == preferences and manifest.get("initial_anchor") == anchor, "The Gray-only initial default-off state changed")
    with closing(readonly_database(database)) as connection, closing(sqlite3.connect(":memory:")) as reference:
        reference.executescript(guest.SCHEMA)
        query = "SELECT type,name,tbl_name,sql FROM sqlite_schema ORDER BY type,name"
        require(connection.execute(query).fetchall() == reference.execute(query).fetchall()
                and connection.execute("PRAGMA user_version").fetchone() == (2,)
                and connection.execute("PRAGMA quick_check").fetchall() == [("ok",)], "The Gray-only seed schema changed")
        for table, records in rows.items():
            require(sorted(connection.execute("SELECT * FROM " + table).fetchall()) == sorted(records), "Unexpected Gray-only seed rows in " + table)
    for page in pages:
        with Image.open(directory / page["file"]) as image:
            image.load()
            require(image.size == (page["width"], page["height"]), "A Gray-only fixture image changed geometry")
    return {"kind": KIND, "variant": variant, "status": "passed", "passed": True, "manifest": manifest,
            "manifest_sha256": manifest_sha, "database_sha256": INITIAL_DATABASE_SHA256, "directory": str(directory),
            "file_count": len(seen_files), "content_total_bytes": manifest["content_total_bytes"], "page_count": len(pages),
            "jpeg_acceleration": False, "receipt_validation_version": 0, "device_accessed": False, "native_codec_opened": False}


def validate_provenance(content, raw_database_sha256):
    value = json.loads(content.decode("utf-8"))
    required = {"kind", "schema_version", "database_sha256", "source_fixture_manifest_sha256", "app_sha256", "worker_sha256",
                "app_process_exited", "native_worker_drained", "session_absent"}
    allowed = required | {"run_id", "process_id", "clean_exit_evidence_sha256"}
    require(isinstance(value, dict) and required <= set(value) <= allowed, "The clean-exit provenance is not the reviewed closed JSON record")
    require(value.get("kind") == "bilicomics_clean_app_exit_database" and value.get("schema_version") == 1,
            "The supplied provenance does not identify a collected clean App exit")
    require(value.get("database_sha256") == raw_database_sha256
            and value.get("source_fixture_manifest_sha256") == CONTINUATION_SOURCE_MANIFEST_SHA256,
            "The provenance does not bind this exact database and approved Gray-only fixture")
    require(all(value.get(key) is True for key in ("app_process_exited", "native_worker_drained", "session_absent")),
            "The supplied collection evidence does not establish a stopped, drained, session-free App")
    for key in ("database_sha256", "source_fixture_manifest_sha256", "app_sha256", "worker_sha256", "clean_exit_evidence_sha256"):
        require(key not in value or isinstance(value[key], str) and re.fullmatch(r"[0-9a-f]{64}", value[key]), "Invalid provenance digest")
    require("run_id" not in value or isinstance(value["run_id"], str) and re.fullmatch(r"[a-z0-9-]{1,64}", value["run_id"]), "Invalid source run identity")
    require("process_id" not in value or type(value["process_id"]) is int and value["process_id"] > 1, "Invalid source App process identity")
    return value


def generate_continuation(source_fixture, database, provenance, output):
    """Preserve a collected SQLite file verbatim; never synthesize an App proof."""
    source_fixture, database, provenance, output = map(Path, (source_fixture, database, provenance, output))
    original = validate_fixture(source_fixture, CONTINUATION_SOURCE_MANIFEST_SHA256)["manifest"]
    require(original.get("variant") == EXACT_TUPLES_VARIANT, "Only the approved measured-tuple fixture may supply continuation state")
    require(output.is_absolute() and output.parent.resolve() == output.parent and output.parent.is_dir()
            and not output.exists() and not output.is_symlink() and ".." not in output.parts, "The continuation output must be a new canonical directory")
    require(not output.is_relative_to(source_fixture), "The continuation must not alter the frozen original fixture tree")
    require(database.is_absolute() and database.resolve() == database, "The collected database path must be canonical")
    with closing(readonly_database(database)):
        pass
    require(not (database.parent / "account/session.json").exists(), "A source session tree cannot supply guest continuation state")
    require(provenance.is_file() and not provenance.is_symlink() and provenance.stat().st_size <= 16 * 1024, "The supplied provenance is not bounded regular JSON")
    raw_database, provenance_bytes = database.read_bytes(), provenance.read_bytes()
    raw_sha = digest(raw_database)
    validate_provenance(provenance_bytes, raw_sha)
    # Validate the actual received bytes before publishing a new fixture. This
    # read-only connection never upgrades receipts or changes preference rows.
    actual = validate_app_database(database, original["pages"])
    require(digest(database.read_bytes()) == raw_sha, "The collected database changed during validation")
    output.mkdir(mode=0o700)
    page_directory = str(Path(original["pages"][0]["file"]).parent)
    for relative in ("data", "data/accounts", str(Path(page_directory).parent), page_directory):
        (output / relative).mkdir(mode=0o700)
    write_new(output / "data/state.sqlite3", raw_database)
    for page in original["pages"]:
        write_new(output / page["file"], (source_fixture / page["file"]).read_bytes())
    script_files = restart_actions()
    for name, value in script_files.items():
        write_new(output / name, (canonical(value) + "\n").encode("ascii"))
    write_new(output / "source-db-provenance.json", provenance_bytes)
    write_new(output / "source-manifest.json", (source_fixture / "manifest.json").read_bytes())
    files = [{"path": "data/state.sqlite3", "sha256": raw_sha, "bytes": len(raw_database)}]
    files += [{"path": p["file"], "sha256": p["encoded_sha256"], "bytes": p["bytes"]} for p in original["pages"]]
    files += [{"path": name, "sha256": digest((canonical(value) + "\n").encode("ascii")),
               "bytes": len((canonical(value) + "\n").encode("ascii"))} for name, value in script_files.items()]
    manifest = {**original, **actual, "variant": CONTINUATION_VARIANT,
                "source_fixture_manifest_sha256": CONTINUATION_SOURCE_MANIFEST_SHA256,
                "image_origin_manifest_sha256": INITIAL_MANIFEST_SHA256,
                "initial_page_index": 9, "schema_migration_source": "src/store.rs:migrate_resource_schema",
                "schema_migration_sha256": digest(migrated_schema().encode("utf-8")),
                "files": files, "content_total_bytes": sum(value["bytes"] for value in files),
                "action_files": list(script_files), "phase_plan": restart_phase_plan(original["pages"]),
                "continuation": {"raw_database_sha256": raw_sha, "raw_database_bytes": len(raw_database),
                    "source_fixture_manifest_sha256": CONTINUATION_SOURCE_MANIFEST_SHA256,
                    "provenance_file": "source-db-provenance.json", "provenance_sha256": digest(provenance_bytes),
                    "provenance_bytes": len(provenance_bytes), "source_manifest_file": "source-manifest.json",
                    "definition": "A new ordinary App process using unchanged collected SQLite; no Kindle reboot is claimed",
                    "application_state_written_by_generator": False}}
    write_new(output / "manifest.json", (canonical(manifest) + "\n").encode("ascii"))
    return validate_fixture(output)


def validate_continuation(directory, manifest, expected_manifest_sha256):
    """A separate variant policy; the initial version-zero seed stays strict."""
    from PIL import Image

    manifest_sha = digest((directory / "manifest.json").read_bytes())
    require(expected_manifest_sha256 is None or manifest_sha == expected_manifest_sha256, "The pinned continuation manifest digest differs")
    require(manifest.get("variant") == CONTINUATION_VARIANT and manifest.get("kind") == KIND and manifest.get("format_version") == 1,
            "Unknown continuation kind or format")
    require((manifest.get("account_key"), manifest.get("comic_id"), manifest.get("episode_id"), manifest.get("revision"), manifest.get("page_count"))
            == (ACCOUNT, COMIC, EPISODE, REVISION, len(WORKLOADS)), "The continuation source identity differs")
    require(manifest.get("schema_version") == 4 and manifest.get("viewport") == list(VIEWPORT) and manifest.get("safety") == SAFETY
            and manifest.get("hardware_policy") == "gray_exact_tuples_v3"
            and manifest.get("source_fixture_manifest_sha256") == CONTINUATION_SOURCE_MANIFEST_SHA256
            and manifest.get("image_origin_manifest_sha256") == INITIAL_MANIFEST_SHA256,
            "The continuation schema, viewport, or isolation contract differs")
    require(manifest.get("schema_migration_source") == "src/store.rs:migrate_resource_schema"
            and manifest.get("schema_migration_sha256") == digest(migrated_schema().encode("utf-8"))
            and manifest.get("initial_page_index") == 9, "The continuation migration or initial-page identity differs")
    require(manifest.get("database") == "data/state.sqlite3" and manifest.get("data_directory") == "data", "The continuation database path differs")
    expected_pages = exact_tuple_pages()
    require(manifest.get("pages") == expected_pages, "The continuation does not use the fixed original synthetic images")
    origin = manifest.get("continuation", {})
    require(origin.get("provenance_file") == "source-db-provenance.json" and origin.get("source_manifest_file") == "source-manifest.json"
            and origin.get("source_fixture_manifest_sha256") == CONTINUATION_SOURCE_MANIFEST_SHA256
            and origin.get("application_state_written_by_generator") is False, "The continuation provenance identity differs")
    provenance = directory / "source-db-provenance.json"
    require(provenance.is_file() and not provenance.is_symlink() and provenance.stat().st_size <= 16 * 1024, "The preserved provenance is not bounded regular JSON")
    provenance_bytes = provenance.read_bytes()
    require(len(provenance_bytes) == origin.get("provenance_bytes") and digest(provenance_bytes) == origin.get("provenance_sha256"), "The received provenance bytes changed")
    original = directory / "source-manifest.json"
    require(original.is_file() and not original.is_symlink() and original.stat().st_size <= 256 * 1024
            and digest(original.read_bytes()) == CONTINUATION_SOURCE_MANIFEST_SHA256, "The preserved approved Gray-only manifest changed")
    database = directory / "data/state.sqlite3"
    with closing(readonly_database(database)):
        pass
    raw_sha, raw_bytes = digest(database.read_bytes()), database.stat().st_size
    require(raw_sha == origin.get("raw_database_sha256") and raw_bytes == origin.get("raw_database_bytes"), "The copied raw App database changed")
    validate_provenance(provenance_bytes, raw_sha)
    actual = validate_app_database(database, expected_pages)
    require(all(manifest.get(key) == value for key, value in actual.items()), "The actual preserved App state differs from the declared continuation")
    expected_files = [{"path": "data/state.sqlite3", "sha256": raw_sha, "bytes": raw_bytes}]
    expected_files += [{"path": p["file"], "sha256": p["encoded_sha256"], "bytes": p["bytes"]} for p in expected_pages]
    expected_files += [{"path": name, "sha256": digest((canonical(value) + "\n").encode("ascii")),
                        "bytes": len((canonical(value) + "\n").encode("ascii"))} for name, value in restart_actions().items()]
    require(manifest.get("files") == expected_files and manifest.get("action_files") == list(restart_actions())
            and manifest.get("phase_plan") == restart_phase_plan(expected_pages), "The continuation data/action/phase inventory differs")
    require(manifest.get("content_total_bytes") == sum(v["bytes"] for v in expected_files)
            and manifest["content_total_bytes"] <= MAX_CONTENT_BYTES, "The continuation content budget differs")
    expected_inventory = {"manifest.json", "source-db-provenance.json", "source-manifest.json", *(v["path"] for v in expected_files)}
    expected_directories = {str(parent) for v in expected_files for parent in Path(v["path"]).parents if str(parent) != "."}
    seen_files, seen = set(), 0
    for path in directory.rglob("*"):
        seen += 1
        require(seen <= len(expected_inventory) + len(expected_directories) and not path.is_symlink(), "The continuation tree exceeds its count budget or contains symlinks")
        relative = path.relative_to(directory).as_posix()
        if path.is_file():
            seen_files.add(relative)
        else:
            require(path.is_dir() and relative in expected_directories, "Unexpected continuation directory or file object")
    require(seen_files == expected_inventory, "The continuation contains unexpected or missing files")
    for record in expected_files:
        path = directory / record["path"]
        require(path.is_file() and not path.is_symlink() and path.resolve().is_relative_to(directory)
                and path.stat().st_size == record["bytes"] <= MAX_PAGE_BYTES
                and digest(path.read_bytes()) == record["sha256"], "A pinned continuation data file changed")
    for name, value in restart_actions().items():
        require((directory / name).read_bytes() == (canonical(value) + "\n").encode("ascii"), "The fixed continuation intents changed")
    for page in expected_pages:
        with Image.open(directory / page["file"]) as image:
            image.load()
            require(image.size == (page["width"], page["height"]), "A continuation source image changed geometry")
    return {"kind": KIND, "variant": CONTINUATION_VARIANT, "status": "passed", "passed": True, "manifest": manifest,
            "manifest_sha256": manifest_sha, "database_sha256": raw_sha, "source_database_sha256": origin["raw_database_sha256"],
            "provenance_sha256": origin["provenance_sha256"], "directory": str(directory), "file_count": len(seen_files),
            "content_total_bytes": manifest["content_total_bytes"], "page_count": len(expected_pages), "jpeg_acceleration": True,
            "receipt_validation_versions": actual["receipt_validation_versions"], "device_accessed": False, "native_codec_opened": False,
            "cold_start_verified": False, "cold_start_requires_real_second_app_process": True}


def validate_fixture(directory, expected_manifest_sha256=None):
    from PIL import Image

    directory = Path(directory)
    require(directory.is_absolute() and directory.is_dir() and not directory.is_symlink(), "The fixture root must be an absolute real directory")
    require(directory.resolve() == directory, "The fixture root must be canonical")
    manifest = read_json(directory / "manifest.json")
    if "variant" in manifest:
        if manifest.get("variant") in (GRAY_ONLY_VARIANT, EXACT_TUPLES_VARIANT):
            return validate_gray_only(directory, manifest, expected_manifest_sha256)
        require(manifest.get("variant") == CONTINUATION_VARIANT, "Unknown media fixture variant")
        return validate_continuation(directory, manifest, expected_manifest_sha256)
    require(manifest.get("kind") == KIND and manifest.get("account_key") == ACCOUNT, "Wrong fixture identity")
    manifest_sha = digest((directory / "manifest.json").read_bytes())
    require(expected_manifest_sha256 is None or manifest_sha == expected_manifest_sha256, "The pinned manifest digest differs")
    require((manifest.get("comic_id"), manifest.get("episode_id"), manifest.get("revision"), manifest.get("page_count"))
            == (COMIC, EPISODE, REVISION, len(WORKLOADS)), "The fixed synthetic content identity differs")
    require(manifest.get("viewport") == list(VIEWPORT) and manifest.get("schema_version") == 2,
            "The selected Scribe geometry or seed schema differs")
    require(manifest.get("safety") == SAFETY and manifest.get("format_version") == 1, "The isolated guest safety contract differs")
    require(manifest.get("schema_sha256") == digest(guest.SCHEMA.encode()), "The seed schema changed")
    expected_pages = []
    for position, spec in enumerate(WORKLOADS):
        content, extension, header = encode_workload(spec)
        expected_pages.append(page_record(position, spec, content, extension, header))
    require(manifest.get("pages") == expected_pages, "The source images or expected plans are not the fixed generated artwork")
    database = directory / "data/state.sqlite3"
    require(database.is_file() and not database.is_symlink() and database.stat().st_size <= 2 * 1024 * 1024,
            "The seed database is not bounded regular data")
    expected_files = [{"path": "data/state.sqlite3", "sha256": digest(database.read_bytes()), "bytes": database.stat().st_size}]
    expected_files += [{"path": p["file"], "sha256": p["encoded_sha256"], "bytes": p["bytes"]} for p in expected_pages]
    expected_files += [{"path": name, "sha256": digest((canonical(value) + "\n").encode("ascii")),
                        "bytes": len((canonical(value) + "\n").encode("ascii"))} for name, value in action_sets().items()]
    require(manifest.get("files") == expected_files and manifest.get("content_total_bytes") == sum(v["bytes"] for v in expected_files),
            "The declared data inventory differs")
    require(manifest["content_total_bytes"] <= MAX_CONTENT_BYTES and len(manifest["files"]) == len(WORKLOADS) + 2,
            "The fixture content budget is exceeded")
    expected_inventory = {"manifest.json", *action_sets(), *(record["path"] for record in expected_files)}
    expected_directories = {str(parent) for record in expected_files for parent in Path(record["path"]).parents if str(parent) != "."}
    inventory = set()
    seen = 0
    for path in directory.rglob("*"):
        seen += 1
        require(seen <= len(expected_inventory) + len(expected_directories), "The fixture inventory exceeds its count budget")
        require(not path.is_symlink(), "The fixture tree contains a symlink")
        if path.is_file():
            inventory.add(path.relative_to(directory).as_posix())
        else:
            require(path.is_dir() and path.relative_to(directory).as_posix() in expected_directories,
                    "The fixture contains an unexpected directory or non-file object")
    require(inventory == expected_inventory, "The fixture tree contains unexpected or missing files")
    require(manifest.get("action_files") == list(action_sets()), "The planned action inventory differs")
    require(manifest.get("phase_plan") == phase_plan(expected_pages), "The expected runtime phase plan changed")
    for name, value in action_sets().items():
        require((directory / name).read_bytes() == (canonical(value) + "\n").encode("ascii"), "The reviewable action plan changed")
    for record in manifest["files"]:
        path = directory / record["path"]
        require(not path.is_symlink() and path.resolve().is_relative_to(directory.resolve()) and path.is_file(), "A fixture file escaped its directory")
        require(path.stat().st_size <= MAX_PAGE_BYTES, "A fixture file exceeds its bounded size")
        content = path.read_bytes()
        require(len(content) == record["bytes"] and digest(content) == record["sha256"], "A frozen fixture file changed")
    for page in manifest["pages"]:
        with Image.open(directory / page["file"]) as image:
            image.load()
            require(image.size == (page["width"], page["height"]), "Decoded synthetic image geometry differs")
        if page["header"]["coding"] != "png":
            require(jpeg_header((directory / page["file"]).read_bytes()) == page["header"], "Synthetic JPEG sampling changed")
    expected, _, _ = state_rows(manifest["pages"], "off" if not manifest["initial_preferences"]["dither"]
                                else manifest["initial_preferences"]["dither_method"])
    require(manifest.get("database") == "data/state.sqlite3" and manifest.get("data_directory") == "data", "The database path differs")
    require(canonical(manifest["initial_preferences"]) == expected["preferences"][0][2], "The initial acceleration preference must remain off")
    with closing(sqlite3.connect(database.as_uri() + "?mode=ro&immutable=1", uri=True)) as connection:
        connection.execute("PRAGMA query_only=ON")
        connection.execute("PRAGMA trusted_schema=OFF")
        with closing(sqlite3.connect(":memory:")) as reference:
            reference.executescript(guest.SCHEMA)
            query = "SELECT type,name,tbl_name,sql FROM sqlite_schema ORDER BY type,name"
            require(connection.execute(query).fetchall() == reference.execute(query).fetchall(), "The database schema contains an unexpected object")
        require(connection.execute("PRAGMA user_version").fetchone() == (2,), "The prelaunch seed was migrated or altered")
        require(connection.execute("PRAGMA quick_check").fetchall() == [("ok",)], "The seed database failed integrity checking")
        for table, records in expected.items():
            require(sorted(connection.execute("SELECT * FROM " + table).fetchall()) == sorted(records), "Unexpected seed rows in " + table)
    require(not (directory / "data/account/session.json").exists(), "A session file appeared in the guest seed")
    return {"kind": KIND, "status": "passed", "passed": True, "manifest": manifest,
            "file_count": len(inventory), "content_total_bytes": manifest["content_total_bytes"],
            "directory": str(directory), "manifest_sha256": manifest_sha,
            "database_sha256": digest(database.read_bytes()), "page_count": len(manifest["pages"]), "jpeg_acceleration": False,
            "receipt_validation_version": 0, "device_accessed": False, "native_codec_opened": False}


def validate(directory):
    return validate_fixture(directory)


def reader_snapshot(records, manifest, kind):
    """Validate real lifecycle records; compatible with the physical harness API."""
    require(manifest.get("kind") == KIND and kind in ("ready", "reader", "suspended", "resumed"), "Unknown media checkpoint")
    require(any(value.get("event") == "ready" for value in records), "The real application is not ready")
    ready_index = next((i for i in range(len(records) - 1, -1, -1) if records[i].get("event") == "reader-ready"), None)
    require(ready_index is not None, "The media reader has no completed frame")
    ready = records[ready_index]
    require(not any(value.get("event") in ("reader-closed", "quit") for value in records[ready_index + 1:]), "The reader was closed after its last frame")
    power = next((value for value in reversed(records) if value.get("event") in ("suspend", "resume")), None)
    suspended = power is not None and power.get("event") == "suspend"
    require(suspended == (kind == "suspended"), "The media checkpoint power state differs")
    if kind == "resumed":
        require(power is not None and power.get("event") == "resume" and power.get("suspended") is False, "The media reader has not resumed")
    state_record = next((value for value in reversed(records) if isinstance(value.get("reader_state"), dict)), None)
    require(state_record is not None and ready.get("reader_loading") is False and ready.get("route") == "reader", "No complete real media reader state")
    reader = state_record["reader_state"]
    page_index = reader.get("page_index")
    require(type(page_index) is int and 0 <= page_index < len(manifest["pages"]), "Unknown media fixture page index")
    page = manifest["pages"][page_index]
    require((reader.get("comic_id"), reader.get("episode_id"), reader.get("revision"), reader.get("page_id"), reader.get("page_number"))
            == (COMIC, EPISODE, REVISION, page["page_id"], page_index + 1), "The media reader source identity differs")
    fit = reader.get("fit")
    require(fit in FITS and reader.get("presented_fit") == reader.get("target_fit") == fit, "The media fit is not completely presented")
    require(reader.get("route") == "reader" and reader.get("core_mode") == reader.get("mode") == "page"
            and reader.get("direction") == "ltr" and reader.get("landscape_layout") == "single"
            and reader.get("rotation") == 0 and reader.get("zoom_ratio") == 1.0
            and reader.get("crop_index") == 0 and reader.get("crop_count") == 1
            and all(reader.get(key) is False for key in ("demo_strip", "effective_spread", "transition", "menu", "rendering"))
            and reader.get("sheet") == "", "The media checkpoint is not a quiet upright complete page")
    require(all(type(reader.get(key)) is int and reader[key] > 0 for key in ("reader_generation", "render_version")), "Missing media frame lifecycle identity")
    require([reader.get("viewport_width"), reader.get("viewport_height")] == list(VIEWPORT)
            and [reader.get("frame_width"), reader.get("frame_height")] == list(VIEWPORT)
            and ready.get("frame_size") == list(VIEWPORT)
            and reader.get("second_frame_width") == reader.get("second_frame_height") == 0, "The native media viewport differs")
    plan = page["fits"][fit]
    expected = [plan["crop"][0] / page["width"], plan["crop"][1] / page["height"],
                plan["crop"][2] / page["width"], plan["crop"][3] / page["height"]]
    require(all(abs(reader.get(key, -100.0) - value) <= 1e-6 for key, value in zip(
        ("focus_x", "focus_y", "focus_width", "focus_height"), expected)), "The media crop differs from the accepted fitting plan")
    require(all(abs(reader.get(key, -100.0) - value) <= 1e-9 for key, value in zip(("source_x", "source_y"), plan["anchor"])), "The media anchor geometry differs")
    policy = "decode_and_resize" if reader.get("jpeg_acceleration") is True and reader.get("media_availability") == "available" else "off"
    require(type(reader.get("jpeg_acceleration")) is bool and all(reader.get(key) == policy for key in
        ("media_acceleration", "presented_media_acceleration", "target_media_acceleration")), "The media preference and complete frame policy differ")
    require(all(key in reader for key in Y4_KEYS) and all(type(reader["native_media"].get(key)) is int and reader["native_media"][key] >= 0
        for key in ("attempts", "completed", "resized", "software_fallbacks")), "The media/Y4 diagnostics are incomplete")
    require(reader.get("dither_method") in ("software", "hardware") and all(type(reader.get(key)) is bool for key in Y4_KEYS
        if key != "dither_method"), "The Y4 diagnostics are not typed preferences/capabilities")
    require(all(ready.get("reader_state", {}).get(key) == reader.get(key) for key in ANCHOR_KEYS +
        ("render_version", "reader_generation", "menu", "sheet", "rendering", "media_acceleration")), "The completed media frame record is stale")
    resources = ready.get("reader_resources") or {}
    require(resources.get("reader_active") is True and resources.get("reader_generation") == reader["reader_generation"]
            and resources.get("page_size") == {"width": VIEWPORT[0], "height": VIEWPORT[1]}, "The media resource snapshot differs from its frame")
    return {"reader_state": reader, "ready": ready, "state_record": state_record, "latest_power": power, "page": page}


def latest_reader(path):
    content = path.read_text(encoding="utf-8")
    require(len(content) <= 16 * 1024 * 1024, "The snapshot input exceeds the bounded size")
    try:
        value = json.loads(content)
        records = value if isinstance(value, list) else [value]
    except json.JSONDecodeError:
        records = [json.loads(line) for line in content.splitlines() if line.strip()]
    records = [value for value in records if isinstance(value, dict) and isinstance(value.get("reader_state"), dict)]
    require(records, "No real reader_state snapshot was supplied")
    record = records[-1]
    require(record.get("event") == "reader-ready" and record.get("reader_loading") is False,
            "The final snapshot is not a complete real reader-ready frame")
    return record, record["reader_state"]


def check_snapshots(directory, before_path, after_path, name, fit, expect, enabled, preserve_anchor):
    manifest = read_json(directory / "manifest.json")
    require(manifest.get("kind") == KIND, "Wrong snapshot fixture kind")
    page = next((value for value in manifest["pages"] if value["name"] == name), None)
    require(page is not None, "Unknown expected fixture page")
    before_record, before = latest_reader(before_path)
    after_record, after = latest_reader(after_path)
    require(before_record.get("process_id") == after_record.get("process_id") and isinstance(after_record.get("process_id"), int),
            "Worker counter deltas require snapshots from one application process")
    require((after.get("comic_id"), after.get("episode_id"), after.get("revision"), after.get("page_id"))
            == (COMIC, EPISODE, REVISION, page["page_id"]), "The current real reader does not own the selected synthetic page")
    require(after.get("fit") == after.get("presented_fit") == after.get("target_fit") == fit, "A fitting change has not reached complete presentation")
    require([after.get("viewport_width"), after.get("viewport_height")] == list(VIEWPORT)
            and [after.get("frame_width"), after.get("frame_height")] == list(VIEWPORT), "The native Scribe viewport is not 1860x2480")
    require(after.get("rotation") == 0 and after.get("zoom_ratio") == 1.0 and after.get("core_mode") == "page"
            and after.get("crop_count") == 1 and not after.get("rendering") and not after.get("effective_spread"),
            "The snapshot is outside the quiet upright single-page plan")
    require(all(key in before and key in after and before[key] == after[key] for key in Y4_KEYS), "Media processing changed a Y4 invariant")
    if preserve_anchor:
        require(all(key in before and key in after and before[key] == after[key] for key in ANCHOR_KEYS), "The media toggle changed the retained reading anchor")
    plan = page["fits"][fit]
    expected_focus = [plan["crop"][0] / page["width"], plan["crop"][1] / page["height"],
                      plan["crop"][2] / page["width"], plan["crop"][3] / page["height"]]
    require(all(abs(after.get(key, -100.0) - value) <= 1e-6 for key, value in zip(
        ("focus_x", "focus_y", "focus_width", "focus_height"), expected_focus)), "Centered crop metadata changed")
    require(all(abs(after.get(key, -100.0) - value) <= 1e-9 for key, value in zip(("source_x", "source_y"), plan["anchor"])),
            "The source anchor differs from the accepted fitting geometry")
    require(after.get("jpeg_acceleration") is enabled, "The requested acceleration preference differs")
    expected_policy = "decode_and_resize" if enabled and after.get("media_availability") == "available" else "off"
    require(after.get("media_acceleration") == after.get("presented_media_acceleration") == after.get("target_media_acceleration") == expected_policy,
            "The presented frame and effective native policy are inconsistent")
    keys = ("attempts", "completed", "resized", "software_fallbacks")
    old, new = before["native_media"], after["native_media"]
    require(all(type(new.get(key)) is int and type(old.get(key)) is int and new[key] >= old[key] for key in keys), "Invalid native statistics identity")
    delta = {key: new[key] - old[key] for key in keys}
    if expect == "native":
        require(enabled and plan["expect_native_when_enabled"] and expected_policy == "decode_and_resize", "The selected workload cannot enter qualified hardware")
        require(all(delta[key] >= 1 for key in ("attempts", "completed", "resized")) and delta["software_fallbacks"] == 0,
                "No cold successful combined hardware transaction was observed")
    else:
        require(not enabled or not plan["expect_native_when_enabled"] or expected_policy == "off",
                "A qualified processed-cache hit cannot be labeled a software fallback")
        require(all(delta[key] == 0 for key in ("attempts", "completed", "resized")), "A software fallback entered the hardware worker")
    return {"passed": True, "page": name, "page_number": page["number"], "fit": fit, "expected": expect,
            "target": plan["target"], "crop": plan["crop"], "statistics_delta": delta, "y4_preserved": True,
            "anchor_preserved": preserve_anchor, "source_identity_verified": True, "physical_panel_update_claimed": False}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    create = commands.add_parser("generate")
    create.add_argument("--output", type=Path, required=True)
    create.add_argument("--dithering", choices=("off", "software", "hardware"), default="software")
    gray = commands.add_parser("generate-gray-only")
    gray.add_argument("--source-fixture", type=Path, required=True)
    gray.add_argument("--output", type=Path, required=True)
    exact = commands.add_parser("generate-exact-tuples")
    exact.add_argument("--source-fixture", type=Path, required=True)
    exact.add_argument("--output", type=Path, required=True)
    continuation = commands.add_parser("generate-continuation")
    continuation.add_argument("--source-fixture", type=Path, required=True)
    continuation.add_argument("--database", type=Path, required=True)
    continuation.add_argument("--provenance", type=Path, required=True)
    continuation.add_argument("--output", type=Path, required=True)
    check = commands.add_parser("validate")
    check.add_argument("--directory", type=Path, required=True)
    compare = commands.add_parser("check-snapshots")
    compare.add_argument("--directory", type=Path, required=True)
    compare.add_argument("--before", type=Path, required=True)
    compare.add_argument("--after", type=Path, required=True)
    compare.add_argument("--page", choices=tuple(value[0] for value in WORKLOADS), required=True)
    compare.add_argument("--fit", choices=FITS, default="page")
    compare.add_argument("--expect", choices=("native", "software"), required=True)
    compare.add_argument("--policy", choices=("on", "off"), default="on")
    compare.add_argument("--preserve-anchor", action="store_true")
    arguments = parser.parse_args()
    require(sys.platform == "linux", "Run fixture generation and verification only on test-env")
    if arguments.command == "generate":
        report = generate(arguments.output, arguments.dithering)
    elif arguments.command == "generate-gray-only":
        report = generate_gray_only(arguments.source_fixture, arguments.output)
    elif arguments.command == "generate-exact-tuples":
        report = generate_exact_tuples(arguments.source_fixture, arguments.output)
    elif arguments.command == "generate-continuation":
        report = generate_continuation(arguments.source_fixture, arguments.database, arguments.provenance, arguments.output)
    elif arguments.command == "validate":
        report = validate(arguments.directory)
    else:
        report = check_snapshots(arguments.directory, arguments.before, arguments.after, arguments.page,
                                 arguments.fit, arguments.expect, arguments.policy == "on", arguments.preserve_anchor)
    print(json.dumps({key: value for key, value in report.items() if key != "manifest"}, sort_keys=True, indent=2))


if __name__ == "__main__":
    try:
        main()
    except (ValueError, KeyError, OSError, sqlite3.Error) as error:
        print(json.dumps({"passed": False, "error": str(error), "device_accessed": False}), file=sys.stderr)
        raise SystemExit(1)
