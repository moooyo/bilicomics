#!/usr/bin/env python3
"""Real offline guest reader fixtures; execute only after the final build gate."""

import argparse
import ctypes
import ctypes.util
import hashlib
import json
import os
from pathlib import Path
import re
import signal
import sqlite3
import struct
import subprocess
import sys
import time
import traceback
import zlib

ACCOUNT = "guest"
COMIC = "900000001"
EPISODE = "90000000101"
REVISION = "local-reader-fixture-v1"
FAULT_TRIGGER = "reader_fixture_revoke_failure"
CASES = ("guide-dismiss", "missing", "corrupt", "revoke-error", "loading")

SCHEMA = """
PRAGMA journal_mode=WAL;
PRAGMA synchronous=FULL;
BEGIN IMMEDIATE;
CREATE TABLE comics(account_key TEXT NOT NULL,comic_id TEXT NOT NULL,payload TEXT NOT NULL,updated_at INTEGER NOT NULL,last_read_at INTEGER NOT NULL,PRIMARY KEY(account_key,comic_id));
CREATE INDEX comics_recent ON comics(account_key,last_read_at DESC,updated_at DESC);
CREATE TABLE comic_details(account_key TEXT NOT NULL,comic_id TEXT NOT NULL,payload TEXT NOT NULL,PRIMARY KEY(account_key,comic_id));
CREATE TABLE catalog_members(account_key TEXT NOT NULL,scope TEXT NOT NULL,comic_id TEXT NOT NULL,position INTEGER NOT NULL,PRIMARY KEY(account_key,scope,comic_id));
CREATE TABLE image_indices(account_key TEXT NOT NULL,episode_id TEXT NOT NULL,revision TEXT NOT NULL,payload TEXT NOT NULL,created_at INTEGER NOT NULL,PRIMARY KEY(account_key,episode_id,revision));
CREATE TABLE reading_anchors(account_key TEXT NOT NULL,comic_id TEXT NOT NULL,episode_id TEXT NOT NULL,revision TEXT NOT NULL,payload TEXT NOT NULL,updated_at INTEGER NOT NULL,PRIMARY KEY(account_key,comic_id));
CREATE INDEX anchors_recent ON reading_anchors(account_key,updated_at DESC);
CREATE TABLE preferences(account_key TEXT NOT NULL,key TEXT NOT NULL,payload TEXT NOT NULL,PRIMARY KEY(account_key,key));
CREATE TABLE downloads(account_key TEXT NOT NULL,task_id TEXT NOT NULL,episode_id TEXT NOT NULL,revision TEXT NOT NULL,status TEXT NOT NULL,payload TEXT NOT NULL,updated_at INTEGER NOT NULL,PRIMARY KEY(account_key,task_id));
CREATE INDEX downloads_episode ON downloads(account_key,episode_id,revision);
CREATE TABLE page_receipts(account_key TEXT NOT NULL,episode_id TEXT NOT NULL,revision TEXT NOT NULL,page_id TEXT NOT NULL,page_index INTEGER NOT NULL,payload TEXT NOT NULL,PRIMARY KEY(account_key,episode_id,revision,page_id));
CREATE INDEX pages_order ON page_receipts(account_key,episode_id,revision,page_index);
CREATE TABLE page_commit_journal(account_key TEXT NOT NULL,task_id TEXT NOT NULL,page_id TEXT NOT NULL,generation INTEGER NOT NULL,payload TEXT NOT NULL,PRIMARY KEY(account_key,task_id,page_id));
CREATE TABLE page_identity_history(account_key TEXT NOT NULL,episode_id TEXT NOT NULL,revision TEXT NOT NULL,page_id TEXT NOT NULL,sha256 TEXT NOT NULL,bytes INTEGER NOT NULL,PRIMARY KEY(account_key,episode_id,revision,page_id));
PRAGMA user_version=2;
COMMIT;
"""


def require(condition, message):
    if not condition:
        raise AssertionError(message)


def write_json(path, value):
    path.write_text(json.dumps(value, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")


def sql(database):
    connection = sqlite3.connect(database, timeout=5)
    connection.execute("PRAGMA busy_timeout=5000")
    return connection


def wait_for(description, predicate, process=None, timeout=30):
    deadline = time.monotonic() + timeout
    last_error = None
    while time.monotonic() < deadline:
        if process is not None:
            require(process.poll() is None, f"The real application exited while waiting for {description}")
        try:
            value = predicate()
            if value:
                return value
        except (FileNotFoundError, json.JSONDecodeError, sqlite3.Error) as error:
            last_error = str(error)
        time.sleep(0.04)
    raise AssertionError(f"Timed out waiting for {description}; last read error: {last_error}")


def trace_records(path):
    if not path.exists():
        return []
    return [json.loads(line) for line in path.read_text().splitlines(keepends=True)
            if line.endswith("\n") and line.strip()]


def read_preferences(database):
    with sql(database) as connection:
        row = connection.execute(
            "SELECT payload FROM preferences WHERE account_key=? AND key='preferences'", (ACCOUNT,)
        ).fetchone()
    return json.loads(row[0]) if row else None


def receipt_count(database):
    with sql(database) as connection:
        return connection.execute(
            "SELECT COUNT(*) FROM page_receipts WHERE account_key=? AND episode_id=? AND revision=?",
            (ACCOUNT, EPISODE, REVISION),
        ).fetchone()[0]


def check_isolation(data, database):
    require(not (data / "account" / "session.json").exists(), "The fixture unexpectedly created a session file")
    with sql(database) as connection:
        keys = [row[0] for row in connection.execute("SELECT DISTINCT account_key FROM preferences")]
        forbidden = connection.execute(
            "SELECT COUNT(*) FROM preferences WHERE key IN ('purchase_journal','recharge_intent') "
            "OR (account_key='device' AND key='last_account')"
        ).fetchone()[0]
    require(set(keys) <= {ACCOUNT}, "The fixture escaped the guest preference namespace")
    require(forbidden == 0, "A session selector or transaction journal appeared unexpectedly")


def png_dimensions(content):
    require(content[:8] == b"\x89PNG\r\n\x1a\n" and content[12:16] == b"IHDR",
            "The supplied design asset is not a PNG with an IHDR")
    return struct.unpack(">II", content[16:24])


def corrupt_idat(content):
    result = bytearray(content)
    offset = 8
    while offset + 12 <= len(result):
        length = struct.unpack(">I", result[offset:offset + 4])[0]
        if result[offset + 4:offset + 8] == b"IDAT" and length > 0:
            result[offset + 8 + length // 2] ^= 0x40
            return bytes(result)
        offset += length + 12
    raise AssertionError("The actual PNG has no IDAT chunk to damage")


def seed_data(output, case, asset_root, origin_page):
    data = output / "data"
    data.mkdir(mode=0o700)
    database = data / "state.sqlite3"
    manifest = json.loads((asset_root / "manifest.json").read_text())
    require(manifest["demo_only"] is True, "Only the supplied fictional design artwork is allowed")
    page = next(value for value in manifest["pages"] if value["number"] == origin_page)
    original_path = (asset_root / page["filename"]).resolve()
    require(original_path.is_relative_to(asset_root.resolve()), "The asset escaped its fixture root")
    original = original_path.read_bytes()
    width, height = png_dimensions(original)
    expected_size = [value * page["scale_factor"] for value in page["logical_size"]]
    require([width, height] == expected_size, "The PNG does not match the real design manifest")
    digest = hashlib.sha256(original).hexdigest()
    filename = digest + ".png"
    pages = data / "accounts" / hashlib.sha256(ACCOUNT.encode()).hexdigest() / "pages"
    pages.mkdir(parents=True, mode=0o700)
    page_path = pages / filename
    page_path.write_bytes(original)
    stamp = int(time.time())
    comic = {
        "id": COMIC, "title": "Local Guest Reader Fixture", "author": "Local fixture",
        "cover_url": "", "description": "Isolated local-cache behavior fixture",
        "genre": "", "latest_episode_id": EPISODE, "latest_episode_title": "Local chapter",
        "latest_order": 1.0, "chapter_count": 1, "favorite": True, "finished": False,
        "updated_at": stamp, "last_episode_id": EPISODE, "last_read_at": stamp,
        "extra": {"fixture_only": True, "origin_manifest_page": origin_page},
    }
    episode = {
        "id": EPISODE, "comic_id": COMIC, "title": "Local chapter", "short_title": "",
        "order": 1.0, "access": "free", "expires_at": None, "pay_gold": None,
        "read": False, "extra": {"fixture_only": True, "page_count": 1},
    }
    detail = {"comic": comic, "episodes": [episode], "extra": {"fixture_only": True}}
    descriptor = {"id": digest, "index": 0, "width": width, "height": height}
    index = {"episode_id": EPISODE, "revision": REVISION, "images": [descriptor],
             "extra": {"fixture_only": True, "source_urls": []}}
    receipt = {
        "episode_id": EPISODE, "revision": REVISION, "page_id": digest, "page_index": 0,
        "file_name": filename, "sha256": digest, "bytes": len(original), "width": width,
        "height": height, "retained": True, "verified_at": 0,
    }
    preferences = {"onboarded": True, "reader_guidance_seen": case != "guide-dismiss",
                   "preload": 0, "launch": "shelf", "mode": "page", "fit": "page", "direction": "ltr"}
    with sql(database) as connection:
        connection.executescript(SCHEMA)
        connection.execute("INSERT INTO comics VALUES(?,?,?,?,?)",
                           (ACCOUNT, COMIC, json.dumps(comic), stamp, stamp))
        connection.execute("INSERT INTO comic_details VALUES(?,?,?)",
                           (ACCOUNT, COMIC, json.dumps(detail)))
        connection.execute("INSERT INTO catalog_members VALUES(?,?,?,?)", (ACCOUNT, "shelf", COMIC, 0))
        connection.execute("INSERT INTO image_indices VALUES(?,?,?,?,?)",
                           (ACCOUNT, EPISODE, REVISION, json.dumps(index), stamp))
        connection.execute("INSERT INTO page_receipts VALUES(?,?,?,?,?,?)",
                           (ACCOUNT, EPISODE, REVISION, digest, 0, json.dumps(receipt)))
        connection.execute("INSERT INTO page_identity_history VALUES(?,?,?,?,?,?)",
                           (ACCOUNT, EPISODE, REVISION, digest, digest, len(original)))
        connection.execute("INSERT INTO preferences VALUES(?,?,?)",
                           (ACCOUNT, "preferences", json.dumps(preferences)))
        if case == "loading":
            record = {
                "id": "local-interrupted-task", "comic_id": COMIC, "episode_id": EPISODE,
                "revision": REVISION, "title": "Local interrupted task", "comic_title": comic["title"],
                "status": "running", "total_pages": 1, "completed_pages": 1, "bytes": len(original),
                "error_code": None, "generation": 1, "priority": 0, "created_at": stamp,
                "updated_at": stamp, "retained": True,
            }
            connection.execute("INSERT INTO downloads VALUES(?,?,?,?,?,?,?)",
                               (ACCOUNT, record["id"], EPISODE, REVISION, "running", json.dumps(record), stamp))
    if case == "missing":
        page_path.unlink()
    elif case in ("corrupt", "revoke-error"):
        page_path.write_bytes(corrupt_idat(original))
    if case == "revoke-error":
        with sql(database) as connection:
            connection.executescript(f"""
                CREATE TRIGGER {FAULT_TRIGGER} BEFORE DELETE ON page_receipts
                WHEN OLD.account_key='{ACCOUNT}' AND OLD.episode_id='{EPISODE}'
                BEGIN SELECT RAISE(FAIL,'controlled_fixture_receipt_revoke_failure'); END;
            """)
    proof = {
        "data": str(data), "database": str(database), "account_key": ACCOUNT,
        "comic_id": COMIC, "episode_id": EPISODE, "revision": REVISION,
        "synthetic_page_index": 0, "synthetic_page_count": 1,
        "origin_manifest_page_number": origin_page, "origin_path": str(original_path),
        "origin_sha256": digest, "origin_bytes": len(original), "origin_dimensions": [width, height],
        "cached_path": str(page_path), "cache_exists": page_path.exists(),
        "cached_sha256": hashlib.sha256(page_path.read_bytes()).hexdigest() if page_path.exists() else None,
        "receipt": receipt, "receipt_origin": "Controlled local fixture; verified_at=0 is not upstream validation",
        "session_seeded": False, "credentials_seeded": False, "source_response_simulated": False,
        "case": case,
    }
    write_json(output / "seed.json", proof)
    check_isolation(data, database)
    return data, database, proof


class XImageHead(ctypes.Structure):
    _fields_ = [
        ("width", ctypes.c_int), ("height", ctypes.c_int), ("xoffset", ctypes.c_int),
        ("format", ctypes.c_int), ("data", ctypes.c_void_p), ("byte_order", ctypes.c_int),
        ("bitmap_unit", ctypes.c_int), ("bitmap_bit_order", ctypes.c_int), ("bitmap_pad", ctypes.c_int),
        ("depth", ctypes.c_int), ("bytes_per_line", ctypes.c_int), ("bits_per_pixel", ctypes.c_int),
        ("red_mask", ctypes.c_ulong), ("green_mask", ctypes.c_ulong), ("blue_mask", ctypes.c_ulong),
    ]


def write_png(path, width, height, rgb):
    def chunk(kind, value):
        return struct.pack(">I", len(value)) + kind + value + struct.pack(">I", zlib.crc32(kind + value))
    rows = b"".join(b"\0" + rgb[offset:offset + width * 3] for offset in range(0, len(rgb), width * 3))
    path.write_bytes(b"\x89PNG\r\n\x1a\n" +
                     chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)) +
                     chunk(b"IDAT", zlib.compress(rows, 6)) + chunk(b"IEND", b""))


class X11:
    def __init__(self):
        require(os.environ.get("DISPLAY", "").startswith(":"), "Use a private local Xvfb display")
        x11_library = os.environ.get("BILICOMICS_X11_LIBRARY") or ctypes.util.find_library("X11")
        xtest_library = os.environ.get("BILICOMICS_XTEST_LIBRARY") or ctypes.util.find_library("Xtst")
        require(x11_library and xtest_library,
                "X11/XTest libraries are required; isolated runtimes can supply explicit library paths")
        self.lib = ctypes.CDLL(x11_library)
        self.test = ctypes.CDLL(xtest_library)
        self.lib.XOpenDisplay.argtypes = [ctypes.c_char_p]
        self.lib.XOpenDisplay.restype = ctypes.c_void_p
        self.lib.XDefaultScreen.argtypes = [ctypes.c_void_p]
        self.lib.XDefaultScreen.restype = ctypes.c_int
        self.lib.XFlush.argtypes = [ctypes.c_void_p]
        self.lib.XSetInputFocus.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_int, ctypes.c_ulong]
        self.lib.XCloseDisplay.argtypes = [ctypes.c_void_p]
        self.lib.XGetImage.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_int, ctypes.c_int,
                                     ctypes.c_uint, ctypes.c_uint, ctypes.c_ulong, ctypes.c_int]
        self.lib.XGetImage.restype = ctypes.POINTER(XImageHead)
        self.lib.XDestroyImage.argtypes = [ctypes.POINTER(XImageHead)]
        self.lib.XDestroyImage.restype = ctypes.c_int
        self.test.XTestFakeMotionEvent.argtypes = [ctypes.c_void_p, ctypes.c_int, ctypes.c_int,
                                                  ctypes.c_int, ctypes.c_ulong]
        self.test.XTestFakeButtonEvent.argtypes = [ctypes.c_void_p, ctypes.c_uint, ctypes.c_int, ctypes.c_ulong]
        self.display = self.lib.XOpenDisplay(None)
        require(self.display, "Cannot open the private Xvfb display")

    def window(self, process):
        tree = subprocess.check_output(["xwininfo", "-root", "-tree"], text=True)
        for candidate in re.finditer(r'(0x[0-9a-fA-F]+)\s+"BiliComics"', tree):
            identifier = candidate.group(1)
            try:
                properties = subprocess.check_output(["xprop", "-id", identifier, "_NET_WM_PID"],
                                                     text=True, stderr=subprocess.DEVNULL)
            except subprocess.CalledProcessError:
                continue
            pid = re.search(r"_NET_WM_PID\(CARDINAL\)\s*=\s*(\d+)", properties)
            if pid is None or int(pid.group(1)) != process.pid:
                continue
            details = subprocess.check_output(["xwininfo", "-id", identifier], text=True)
            if "Map State: IsViewable" not in details:
                continue
            fields = {}
            for key, pattern in {
                "x": r"Absolute upper-left X:\s*(-?\d+)", "y": r"Absolute upper-left Y:\s*(-?\d+)",
                "width": r"Width:\s*(\d+)", "height": r"Height:\s*(\d+)",
            }.items():
                fields[key] = int(re.search(pattern, details).group(1))
            fields["id"] = identifier
            fields["process_id"] = int(pid.group(1))
            return fields
        return None

    def click(self, window, relative_x, relative_y):
        self.lib.XSetInputFocus(self.display, int(window["id"], 16), 2, 0)
        self.test.XTestFakeMotionEvent(self.display, self.lib.XDefaultScreen(self.display),
                                      window["x"] + int(relative_x), window["y"] + int(relative_y), 0)
        self.lib.XFlush(self.display)
        self.lib.XSync(self.display, 0)
        time.sleep(0.08)
        self.test.XTestFakeButtonEvent(self.display, 1, 1, 0)
        self.lib.XFlush(self.display)
        time.sleep(0.08)
        self.test.XTestFakeButtonEvent(self.display, 1, 0, 0)
        self.lib.XFlush(self.display)

    def capture(self, window, path):
        width, height = window["width"], window["height"]
        pointer = self.lib.XGetImage(self.display, int(window["id"], 16), 0, 0, width, height,
                                    ctypes.c_ulong(-1).value, 2)
        require(pointer, "XGetImage did not return the actual mapped window")
        try:
            head = pointer.contents
            require(head.bits_per_pixel == 32 and head.byte_order == 0 and
                    (head.red_mask, head.green_mask, head.blue_mask) == (0xff0000, 0xff00, 0xff),
                    "Unsupported Xvfb pixel layout")
            packed = ctypes.string_at(head.data, head.bytes_per_line * height)
            rows = []
            for row in range(height):
                source = packed[row * head.bytes_per_line:row * head.bytes_per_line + width * 4]
                target = bytearray(width * 3)
                target[0::3], target[1::3], target[2::3] = source[2::4], source[1::4], source[0::4]
                rows.append(target)
            rgb = b"".join(rows)
            write_png(path, width, height, rgb)
            return {"path": str(path), "rgb_sha256": hashlib.sha256(rgb).hexdigest(),
                    "size": [width, height], "origin": "Actual X11 window pixels",
                    "window_id": window["id"], "window_process_id": window["process_id"]}
        finally:
            self.lib.XDestroyImage(pointer)

    def stable_capture(self, window, path, process, timeout=4):
        deadline = time.monotonic() + timeout
        previous = None
        equal = 0
        while time.monotonic() < deadline:
            require(process.poll() is None, "The application exited before the real window capture")
            result = self.capture(window, path)
            equal = equal + 1 if result["rgb_sha256"] == previous else 0
            previous = result["rgb_sha256"]
            if equal >= 2:
                return result
            time.sleep(0.08)
        raise AssertionError("The actual X11 view did not settle within the capture window")

    def close(self):
        self.lib.XCloseDisplay(self.display)


class Run:
    def __init__(self, binary, output, data, profile, exit_after_ms=60000):
        output.mkdir()
        self.output, self.data = output, data
        self.trace = output / "lifecycle.jsonl"
        self.capture = output / "final.png"
        self.log = (output / "application.log").open("w")
        self.command = ["unshare", "-n", "--", str(binary), "--offline", "--device", profile,
                        "--screen", "reader", "--data-dir", str(data),
                        "--lifecycle-trace", str(self.trace), "--capture", str(self.capture),
                        "--exit-after-ms", str(exit_after_ms)]
        self.process = subprocess.Popen(self.command, stdout=self.log, stderr=subprocess.STDOUT)
        self.parent_netns = os.readlink("/proc/self/ns/net")
        try:
            self.netns = wait_for("the private application network namespace",
                                 lambda: self.network_namespace(), process=self.process, timeout=5)
        except Exception:
            self.cleanup()
            raise

    def network_namespace(self):
        value = os.readlink(f"/proc/{self.process.pid}/ns/net")
        return value if value != self.parent_netns else None

    def event(self, name):
        return next((value for value in reversed(trace_records(self.trace))
                     if value.get("event") == name and value.get("process_id") == self.process.pid), None)

    def ready(self):
        return wait_for("the real application ready event", lambda: self.event("ready"),
                        process=self.process, timeout=90)

    def frame_ready(self):
        return wait_for("a completed real local reader frame", lambda: next((
            value for value in reversed(trace_records(self.trace))
            if value.get("event") == "reader-ready" and value.get("process_id") == self.process.pid
            and value.get("reader_loading") is False
            and len(value.get("frame_size", [])) == 2 and min(value["frame_size"]) > 0
            and (value.get("reader_state") or {}).get("comic_id") == COMIC), None),
            process=self.process, timeout=30)

    def stop(self):
        if self.process.poll() is None:
            self.process.send_signal(signal.SIGTERM)
        require(self.process.wait(timeout=20) == 0, "The real application did not exit cleanly")
        self.log.close()
        metadata = json.loads(self.capture.with_suffix(".json").read_text())
        require(metadata["process_id"] == self.process.pid, "The capture belongs to another process")
        require(metadata["route"] == "reader" and metadata["demo"] is False
                and metadata["offline"] is True and not metadata["startup"]["booting"],
                "The real process did not remain an isolated offline guest reader")
        require(metadata["reader"]["pages"] == 1, "The fixture claimed more than its one local descriptor")
        return metadata

    def cleanup(self):
        if self.process.poll() is None:
            self.process.terminate()
            try:
                self.process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait(timeout=5)
        self.log.close()


def remove_trigger(database):
    with sql(database) as connection:
        connection.execute(f"DROP TRIGGER IF EXISTS {FAULT_TRIGGER}")


def guide_case(binary, output, data, database, profile, x11):
    first = Run(binary, output / "first", data, profile)
    try:
        first.ready()
        ready = first.frame_ready()
        window = wait_for("the mapped real reader window", lambda: x11.window(first.process),
                          process=first.process)
        require(read_preferences(database)["reader_guidance_seen"] is False,
                "The real guide was cleared before the click")
        before = x11.stable_capture(window, output / "guide-before-click.png", first.process)
        x11.click(window, window["width"] * 0.5, window["height"] * 0.5)
        wait_for("real guidance preference persistence", lambda:
                 read_preferences(database).get("reader_guidance_seen") is True, process=first.process)
        after = x11.stable_capture(window, output / "guide-after-click.png", first.process)
        require(before["rgb_sha256"] != after["rgb_sha256"], "The real Guide view did not change after its click")
        first_metadata = first.stop()
        require(first_metadata["reader"]["menu"] is False and first_metadata["network_prompt"] is False,
                "The first Guide tap opened another UI instead of only dismissing guidance")
        check_isolation(data, database)
    finally:
        first.cleanup()
    second = Run(binary, output / "restart", data, profile)
    try:
        second.ready()
        second_ready = second.frame_ready()
        window = wait_for("the restarted real reader window", lambda: x11.window(second.process),
                          process=second.process)
        reopened = x11.stable_capture(window, output / "guide-after-restart.png", second.process)
        require(reopened["rgb_sha256"] == after["rgb_sha256"],
                "The restarted actual reader differs from the dismissed Guide view")
        second_metadata = second.stop()
        require(second_metadata["reader"]["menu"] is False and second_metadata["network_prompt"] is False,
                "The restarted local reader did not remain unobscured")
        require(read_preferences(database)["reader_guidance_seen"] is True, "Guide dismissal did not persist")
        check_isolation(data, database)
        return {"passed": True, "status": "observed", "before": before, "after": after,
                "reopened": reopened, "first_ready": ready, "restart_ready": second_ready,
                "first_metadata": first_metadata, "restart_metadata": second_metadata,
                "network_namespaces": [first.netns, second.netns],
                "controller_dismissal_persisted": True,
                "guidance_flag_exported_by_capture": False}
    finally:
        second.cleanup()


def error_case(binary, output, data, database, case, profile, x11):
    run = Run(binary, output / "application", data, profile)
    try:
        run.ready()
        window = wait_for("the mapped real reader error window", lambda: x11.window(run.process),
                          process=run.process)
        if case != "revoke-error":
            wait_for("real receipt invalidation", lambda: receipt_count(database) == 0, process=run.process)
        time.sleep(0.5)
        before = x11.stable_capture(window, output / "before-sheet-close.png", run.process)
        if case != "revoke-error":
            # The point is outside the actual bottom sheet and is inert on the failed reader.
            x11.click(window, window["width"] * 0.5, window["height"] * 0.08)
        after = x11.stable_capture(window, output / "reader-error.png", run.process)
        metadata = run.stop()
        require(metadata["reader"]["loading"] is False and metadata["reader"]["transition"] is False,
                "The fault capture remained in loading or chapter transition")
        require(metadata["network_prompt"] is False, "The actual NetSheet still covers the reader error")
        require(not any(value.get("event") == "reader-ready" for value in trace_records(run.trace)),
                "The broken local cache unexpectedly produced a ready page")
        if case != "revoke-error":
            require(receipt_count(database) == 0, "The broken page receipt was not revoked")
            require(before["rgb_sha256"] != after["rgb_sha256"],
                    "The actual NetSheet did not visibly close")
        else:
            require(receipt_count(database) == 1, "The controlled revoke failure did not retain its receipt")
        check_isolation(data, database)
        return {
            "passed": True, "status": "captured_requires_visual_review",
            "before_sheet_close": before, "after_sheet_close": after, "metadata": metadata,
            "network_namespace": run.netns,
            "source_derived_outcome": "Failed after a real receipt-revocation storage error" if case == "revoke-error"
                                      else "Missing after real cached-file validation failed",
            "pure_png_decode_error_coverage": False,
            "fault_class": "Corrupt file plus receipt-revocation SQL failure" if case == "revoke-error" else case,
            "reader_failed_flag_exported_by_capture": False,
            "error_text_exported_by_capture": False,
            "visual_review_required": True,
        }
    finally:
        remove_trigger(database)
        run.cleanup()


def loading_validation_state(data, database, seed):
    # Read proofs written by the real App. The fixture never promotes a receipt
    # or changes the schema itself after the original legacy seed is created.
    with sqlite3.connect(database.as_uri() + "?mode=ro", uri=True, timeout=5) as connection:
        version = connection.execute("PRAGMA user_version").fetchone()[0]
        require(version == 4, "The real warmup did not migrate the metadata schema to version 4")
        receipts = connection.execute(
            "SELECT page_id,payload FROM page_receipts WHERE account_key=? AND episode_id=? AND revision=?",
            (ACCOUNT, EPISODE, REVISION),
        ).fetchall()
        require(len(receipts) == 1, "The loading fixture must retain exactly its one real page receipt")
        receipt = json.loads(receipts[0][1])
        history = connection.execute(
            "SELECT sha256,bytes,decode_validation_version FROM page_identity_history "
            "WHERE account_key=? AND episode_id=? AND revision=? AND page_id=?",
            (ACCOUNT, EPISODE, REVISION, receipts[0][0]),
        ).fetchone()
    require(receipts[0][0] == seed["origin_sha256"] and receipt["sha256"] == seed["origin_sha256"]
            and receipt["bytes"] == seed["origin_bytes"]
            and [receipt["width"], receipt["height"]] == seed["origin_dimensions"],
            "The real warmup changed the seeded image identity, bytes, or geometry")
    require(receipt.get("decode_validation_version") == 1 and receipt["verified_at"] > 0,
            "The real warmup did not persist a complete image decode proof")
    require(history == (seed["origin_sha256"], seed["origin_bytes"], 1),
            "The real warmup did not bind the complete proof to retained image history")
    cached = data / "accounts" / hashlib.sha256(ACCOUNT.encode()).hexdigest() / "pages" / receipt["file_name"]
    require(cached == Path(seed["cached_path"]) and cached.is_file()
            and hashlib.sha256(cached.read_bytes()).hexdigest() == seed["origin_sha256"],
            "The complete validation proof no longer identifies the original cached bytes")
    return {"schema_version": version, "receipt": receipt,
            "identity_history": {"sha256": history[0], "bytes": history[1], "decode_validation_version": history[2]},
            "cached_path": str(cached), "cached_sha256": seed["origin_sha256"],
            "proof_origin": "Real App migration and complete cached-image validation; read-only fixture observation"}


def loading_case(binary, output, data, database, profile, x11):
    blocker, warmup, run = None, None, None
    warmup_evidence, reset_evidence, lock_evidence = {}, {}, {}
    phase = "warmup"
    try:
        seed = json.loads((output / "seed.json").read_text())
        # Schema migration and legacy-proof promotion legitimately write the
        # database. Let the production App finish both before blocking only its
        # later offline pause worker.
        warmup = Run(binary, output / "warmup", data, profile)
        warmup_ready = warmup.ready()
        warmup_frame = warmup.frame_ready()
        warmup_window = wait_for("the mapped warmup reader window", lambda: x11.window(warmup.process),
                                 process=warmup.process)
        warmup_capture = x11.stable_capture(warmup_window, output / "reader-before-loading-attempt.png", warmup.process)
        warmup_metadata = warmup.stop()
        require(warmup_metadata["reader"]["loading"] is False
                and warmup_metadata["reader"]["page"] == 1 and warmup_metadata["reader"]["menu"] is False
                and warmup_metadata["reader"]["sheet"] == "" and warmup_metadata["network_prompt"] is False,
                "The real warmup did not finish on the unobscured cached source page")
        validation = loading_validation_state(data, database, seed)
        check_isolation(data, database)
        warmup_evidence = {"ready": warmup_ready, "frame": warmup_frame, "capture": warmup_capture,
                           "metadata": warmup_metadata, "network_namespace": warmup.netns,
                           "process_id": warmup.process.pid, "validation": validation,
                           "normal_exit_verified": warmup.process.returncode == 0}
        write_json(output / "warmup.json", warmup_evidence)

        phase = "reset-interrupted-task"
        stamp = int(time.time())
        with sql(database) as connection:
            row = connection.execute(
                "SELECT status,payload FROM downloads WHERE account_key=? AND task_id=?",
                (ACCOUNT, "local-interrupted-task"),
            ).fetchone()
            require(row is not None, "The real warmup lost the controlled interrupted task")
            previous = json.loads(row[1])
            require(row[0] == "paused" and previous["status"] == "paused" and previous["generation"] == 2,
                    "The real warmup did not finish its first offline pause generation")
            changed = connection.execute(
                "UPDATE downloads SET status='running', "
                "payload=json_set(payload,'$.status','running','$.generation',1,'$.error_code',NULL,'$.updated_at',?), "
                "updated_at=? WHERE account_key=? AND task_id=? AND episode_id=? AND revision=? "
                "AND status='paused' AND json_extract(payload,'$.status')='paused' "
                "AND json_extract(payload,'$.generation')=2",
                (stamp, stamp, ACCOUNT, "local-interrupted-task", EPISODE, REVISION),
            ).rowcount
            require(changed == 1, "The controlled loading reset must affect exactly one warmed download task")
        with sql(database) as connection:
            row = connection.execute(
                "SELECT status,payload FROM downloads WHERE account_key=? AND task_id=?",
                (ACCOUNT, "local-interrupted-task"),
            ).fetchone()
        reset = json.loads(row[1])
        require(row[0] == "running" and reset["status"] == "running" and reset["generation"] == 1,
                "The controlled interrupted task was not reset to Running generation 1")
        require(all(previous.get(key) == value for key, value in reset.items()
                    if key not in ("status", "generation", "error_code", "updated_at")),
                "The controlled loading reset changed unrelated task data")
        require(loading_validation_state(data, database, seed) == validation,
                "The controlled loading reset changed an application-written validation proof")
        reset_evidence = {"updated_rows": changed, "before": previous, "after": reset,
                          "write_scope": "Only the interrupted download task status, generation, error, and update time",
                          "schema_and_image_proofs_modified": False}
        write_json(output / "task-reset.json", reset_evidence)

        phase = "blocked-offline-pause"
        blocker = sql(database)
        blocker.execute("BEGIN IMMEDIATE")
        acquired = time.monotonic()
        pending_deadline = acquired + 2.5
        lock_evidence = {"acquired_monotonic_seconds": acquired, "capture_deadline_monotonic_seconds": pending_deadline,
                         "capture_budget_seconds": 2.5}
        ready, window, pending, attempted_capture, observation_gap = None, None, None, None, None
        try:
            run = Run(binary, output / "application", data, profile)
            remaining = pending_deadline - time.monotonic()
            if remaining <= 0.3:
                observation_gap = "The bounded writer-lock window elapsed before the real ready observation"
            else:
                try:
                    ready = wait_for("the real ready event inside the writer-lock window", lambda: run.event("ready"),
                                     process=run.process, timeout=remaining)
                except AssertionError as error:
                    if run.process.poll() is not None:
                        raise
                    observation_gap = str(error)
            if ready is not None:
                if ready.get("reader_loading") is not True:
                    observation_gap = "The real ready event did not observe loading; no model field was forced"
                else:
                    require(ready.get("frame_size") == [0, 0], "The pending view already had a completed page frame")
                    remaining = pending_deadline - time.monotonic()
                    if remaining <= 0.3:
                        observation_gap = "The real loading view had no remaining bounded capture window"
                    else:
                        try:
                            window = wait_for("the mapped real pending reader window", lambda: x11.window(run.process),
                                              process=run.process, timeout=min(1.0, remaining - 0.1))
                            remaining = pending_deadline - time.monotonic()
                            if remaining <= 0.3:
                                observation_gap = "The mapped loading window appeared too late for a bounded capture"
                            else:
                                attempted_capture = x11.stable_capture(
                                    window, output / "reader-loading.png", run.process, timeout=min(1.2, remaining - 0.05))
                                lock_evidence["capture_finished_monotonic_seconds"] = time.monotonic()
                                if time.monotonic() < pending_deadline:
                                    pending = attempted_capture
                                else:
                                    observation_gap = "The actual loading capture exceeded the bounded writer-lock window"
                        except AssertionError as error:
                            if run.process.poll() is not None:
                                raise
                            observation_gap = str(error)
        finally:
            blocker.rollback()
            lock_evidence["released_monotonic_seconds"] = time.monotonic()
            lock_evidence["held_seconds"] = lock_evidence["released_monotonic_seconds"] - acquired
            blocker.close()
            blocker = None

        phase = "recovery"
        if ready is None:
            ready = run.ready()
        completed = run.frame_ready()
        if window is None:
            window = wait_for("the mapped recovered reader window", lambda: x11.window(run.process), process=run.process)
        final = x11.stable_capture(window, output / "reader-after-local-recovery.png", run.process)
        metadata = run.stop()
        with sql(database) as connection:
            row = connection.execute("SELECT status,payload FROM downloads WHERE account_key=? AND task_id=?",
                                     (ACCOUNT, "local-interrupted-task")).fetchone()
        require(row is not None, "The recovered offline pause worker lost its controlled task")
        record = json.loads(row[1])
        require(row[0] == "paused" and record["status"] == "paused" and record["generation"] == 2,
                "The real offline pause worker did not finish its deferred write")
        require(metadata["reader"]["loading"] is False and metadata["reader"]["page"] == 1,
                "The local recovery did not deliver the real cached source page")
        require(final["rgb_sha256"] == warmup_capture["rgb_sha256"],
                "The real recovered page pixels differ from the healthy pre-lock warmup page")
        require(loading_validation_state(data, database, seed) == validation,
                "The blocked offline pause or recovery changed a completed image validation proof")
        check_isolation(data, database)
        evidence = {"ready": ready, "completed": completed, "final_capture": final, "metadata": metadata,
                    "warmup": warmup_evidence, "task_reset": reset_evidence, "writer_lock": lock_evidence,
                    "network_namespace": run.netns, "offline_task": record,
                    "pending_origin": "Actual manager lease while SQLite blocked its offline pause write",
                    "production_hook_added": False, "reader_model_field_forced": False,
                    "same_loaded_page_pixels_after_recovery": True}
        if pending is None:
            return {**evidence, "passed": None, "status": "observability_gap", "reason": observation_gap,
                    "unaccepted_pending_capture": attempted_capture, "loading_model_observed": False,
                    "loading_pixel_acceptance_verified": False}
        require(pending["rgb_sha256"] != final["rgb_sha256"], "The actual pending view did not become the real page")
        return {**evidence, "passed": True, "status": "captured_requires_visual_review", "pending_capture": pending,
                "loading_model_observed": True, "loading_pixel_acceptance_verified": False,
                "visual_review_required": True}
    except Exception as error:
        return {"passed": False, "status": "failed", "error": str(error),
                "traceback": traceback.format_exc(), "failure_class": "loading_fixture_attempt_failure",
                "failure_phase": phase, "warmup": warmup_evidence, "task_reset": reset_evidence,
                "writer_lock": lock_evidence,
                "production_hook_added": False, "reader_model_field_forced": False}
    finally:
        if blocker is not None:
            blocker.rollback()
            blocker.close()
        if run is not None:
            run.cleanup()
        if warmup is not None:
            warmup.cleanup()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--workspace", type=Path, required=True)
    parser.add_argument("--expected-binary-sha256", required=True)
    parser.add_argument("--output", type=Path, required=True, help="A new isolated fixture output directory")
    parser.add_argument("--profile", default="pw5", choices=("pw5", "pw6", "scribe", "scribe2025"))
    parser.add_argument("--assets-root", type=Path, required=True)
    parser.add_argument("--origin-page", type=int, default=18, choices=(18, 19))
    parser.add_argument("--case", action="append", choices=CASES)
    parser.add_argument("--include-loading", action="store_true")
    args = parser.parse_args()
    os.umask(0o077)
    args.output = args.output.resolve()
    require(args.output.is_relative_to(args.workspace.resolve(strict=True)) and args.output != args.workspace.resolve(),
            "Output must be a new directory inside the isolated workspace")
    args.output.mkdir(parents=True, exist_ok=False)
    args.binary = args.binary.resolve()
    require(args.binary.is_file(), "The final compiled application binary is missing")
    digest = hashlib.sha256(args.binary.read_bytes()).hexdigest()
    require(digest == args.expected_binary_sha256.lower(), "The selected desktop binary digest differs")
    selected = args.case or ["guide-dismiss", "missing", "corrupt", "revoke-error"]
    if args.include_loading and "loading" not in selected:
        selected.append("loading")
    selected = list(dict.fromkeys(selected))
    x11 = X11()
    records = []
    try:
        for case in selected:
            output = args.output / case
            output.mkdir()
            result = {"name": case, "class": "fixture", "view_model_fixture_only": False,
                      "auth_verified": False, "live_account_verified": False,
                      "credentials_seeded": False, "actual_transaction_executed": False,
                      "source_success_simulated": False, "production_source_modified": False,
                      "http_network_policy": "The actual app process runs in a fresh unshare -n namespace",
                      "hardware_verified": False}
            try:
                data, database, proof = seed_data(output, case, args.assets_root.resolve(), args.origin_page)
                result["seed"] = proof
                if case == "guide-dismiss":
                    result.update(guide_case(args.binary, output, data, database, args.profile, x11))
                elif case == "loading":
                    result.update(loading_case(args.binary, output, data, database, args.profile, x11))
                else:
                    result.update(error_case(args.binary, output, data, database, case, args.profile, x11))
            except Exception as error:
                result.update({"passed": False, "status": "failed", "error": str(error),
                               "traceback": traceback.format_exc()})
            write_json(output / "result.json", result)
            records.append(result)
            print(json.dumps({"name": case, "passed": result.get("passed"), "status": result.get("status"),
                              "error": result.get("error"), "reason": result.get("reason")}), flush=True)
    finally:
        x11.close()
    summary = {"class": "fixture", "binary_sha256": digest, "view_model_fixture_only": False, "auth_verified": False,
               "input_delivery_protocol": {"method": "XTest", "focus_existing_pid_verified_window": True,
                                           "motion_separate_flush_and_sync": True,
                                           "pre_down_event_loop_slot_ms": 80, "tap_hold_ms": 80},
               "actual_transactions_executed": False, "cases": records,
               "passed": all(value.get("passed") is not False for value in records),
               "loading_observability_gaps": sum(value["status"] == "observability_gap" for value in records),
               "error_visual_review_required": True,
               "manual_visual_review_pending": [value["name"] for value in records if value.get("visual_review_required")],
               "all_selected_automated_invariants_passed": all(value.get("passed") is True for value in records)}
    write_json(args.output / "results.json", summary)
    return 0 if summary["passed"] else 1


if __name__ == "__main__":
    from harness_signals import install_interruption_handlers
    install_interruption_handlers()
    sys.exit(main())
