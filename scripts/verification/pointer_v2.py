#!/usr/bin/env python3
"""Verify selected v2 Catalog and Downloads controls using real XTest clicks."""

import argparse
import ctypes
import ctypes.util
import datetime
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

INPUT_DELIVERY_PROTOCOL = {
    "method": "XTest",
    "motion_separate_flush_and_sync": True,
    "pre_down_event_loop_slot_ms": 80,
    "tap_hold_ms": 80,
    "focus_or_raise": False,
    "scope": "Headless input-delivery precondition; not a production application fix",
}
CONTEXT_KEYS = ("BILICOMICS_LIFECYCLE_DIR", "BILICOMICS_LAUNCH_TOKEN", "BILICOMICS_LAUNCH_GENERATION")
PROFILES = {
    "pw5": {"size": (618, 824), "status": 28, "header": 53, "margin": 24,
            "search_start": 86, "search_back": (70, 54), "hide": (562, 54),
            "field_left": (26, 119), "field_right": (536, 119),
            "n": (432, 720), "i": (461, 604), "candidate_next": (586, 540),
            "candidate_previous": (530, 540), "candidate_first": (110, 540),
            "detail_enter": (392, 201), "detail_row": (309, 327),
            "detail_cancel": (604, 54), "detail_next": (603, 794), "detail_locate": (481, 268),
            "no_history_cover": (309, 575), "toggle_all": (478, 54),
            "download_first": (309, 174), "download_third": (309, 342),
            "task_primary": (173, 436), "sheet_done": (566, 268)},
    "scribe": {"size": (930, 1240), "status": 32, "header": 73, "margin": 48,
               "search_start": 128, "search_back": (100, 68), "hide": (844, 68),
               "field_left": (50, 153), "field_right": (816, 153),
               "n": (648, 1112), "i": (692, 960), "candidate_next": (890, 878),
               "candidate_previous": (834, 878), "candidate_first": (146, 878),
               "detail_enter": (592, 341), "detail_row": (465, 559),
               "detail_cancel": (896, 68), "detail_next": (895, 1200), "detail_locate": (748, 440),
               "no_history_cover": (807, 618), "toggle_all": (720, 68),
               "download_first": (465, 220), "download_third": (465, 428),
               "task_primary": (664, 678), "sheet_done": (854, 518)},
}


def require(value, message):
    if not value:
        raise AssertionError(message)


def write_json(path, value):
    path.write_text(json.dumps(value, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")


def trace_records(path):
    if not path.exists():
        return []
    result = []
    for line in path.read_text(encoding="utf-8").splitlines(keepends=True):
        if line.endswith("\n") and line.strip():
            result.append(json.loads(line))
    return result


def normalized(record):
    for key in ("ui", "state", "snapshot"):
        if isinstance(record.get(key), dict):
            return record[key]
    return record


def field(state, path):
    alternatives = {
        "catalog.keyboard": ("catalog.keyboard", "catalog.keyboard_visible"),
        "catalog.download_selecting": ("catalog.download_selecting", "catalog.selecting"),
        "catalog.detail_id": ("catalog.detail_id", "catalog.detail.id"),
        "downloads.task_primary_action": ("downloads.task_primary_action", "downloads.task.primary_action"),
        "downloads.sheet_id": ("downloads.sheet_id", "downloads.sheet.id"),
        "downloads.sheet_open": ("downloads.sheet_open", "downloads.sheet.open"),
    }
    for candidate in alternatives.get(path, (path,)):
        value = state
        for part in candidate.split("."):
            value = value.get(part) if isinstance(value, dict) else None
        if value is not None:
            return value
    return None


def expect(state, expected):
    return all(field(state, key) == value for key, value in expected.items())


def item(state, identifier):
    return next((row for row in field(state, "downloads.items") or [] if str(row.get("id")) == identifier), None)


def ids(state):
    return [str(row["id"]) for row in field(state, "downloads.items") or []]


class XImage(ctypes.Structure):
    _fields_ = [
        ("width", ctypes.c_int), ("height", ctypes.c_int), ("xoffset", ctypes.c_int),
        ("format", ctypes.c_int), ("data", ctypes.c_void_p), ("byte_order", ctypes.c_int),
        ("bitmap_unit", ctypes.c_int), ("bitmap_bit_order", ctypes.c_int),
        ("bitmap_pad", ctypes.c_int), ("depth", ctypes.c_int), ("bytes_per_line", ctypes.c_int),
        ("bits_per_pixel", ctypes.c_int), ("red_mask", ctypes.c_ulong),
        ("green_mask", ctypes.c_ulong), ("blue_mask", ctypes.c_ulong),
    ]


def png_chunk(kind, data):
    return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)


class Pointer:
    def __init__(self):
        self.x11 = ctypes.CDLL(ctypes.util.find_library("X11"))
        self.xtst = ctypes.CDLL(ctypes.util.find_library("Xtst"))
        self.x11.XOpenDisplay.argtypes = [ctypes.c_char_p]
        self.x11.XOpenDisplay.restype = ctypes.c_void_p
        self.x11.XDefaultScreen.argtypes = [ctypes.c_void_p]
        self.x11.XDefaultScreen.restype = ctypes.c_int
        self.x11.XFlush.argtypes = [ctypes.c_void_p]
        self.x11.XSync.argtypes = [ctypes.c_void_p, ctypes.c_int]
        self.x11.XCloseDisplay.argtypes = [ctypes.c_void_p]
        self.xtst.XTestFakeMotionEvent.argtypes = [ctypes.c_void_p, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_ulong]
        self.xtst.XTestFakeButtonEvent.argtypes = [ctypes.c_void_p, ctypes.c_uint, ctypes.c_int, ctypes.c_ulong]
        self.x11.XGetImage.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_int, ctypes.c_int,
                                     ctypes.c_uint, ctypes.c_uint, ctypes.c_ulong, ctypes.c_int]
        self.x11.XGetImage.restype = ctypes.POINTER(XImage)
        self.x11.XDestroyImage.argtypes = [ctypes.POINTER(XImage)]
        self.display = self.x11.XOpenDisplay(None)
        require(self.display, "Run this harness inside its isolated Xvfb display")

    def window(self):
        tree = subprocess.check_output(["xwininfo", "-root", "-tree"], text=True)
        matches = re.findall(r'(0x[0-9a-fA-F]+) "BiliComics - Design Preview"', tree)
        require(len(matches) <= 1, "The isolated display contains multiple application windows")
        if not matches:
            return None
        details = subprocess.check_output(["xwininfo", "-id", matches[0]], text=True)
        if "Map State: IsViewable" not in details:
            return None
        expressions = {"x": r"Absolute upper-left X:\s*(-?\d+)", "y": r"Absolute upper-left Y:\s*(-?\d+)",
                       "width": r"Width:\s*(\d+)", "height": r"Height:\s*(\d+)"}
        return {"id": matches[0], **{key: int(re.search(expression, details).group(1))
                                    for key, expression in expressions.items()}}

    def click(self, window, point):
        x, y = window["x"] + point[0], window["y"] + point[1]
        self.xtst.XTestFakeMotionEvent(self.display, self.x11.XDefaultScreen(self.display), x, y, 0)
        self.x11.XFlush(self.display)
        self.x11.XSync(self.display, 0)
        time.sleep(0.08)
        self.xtst.XTestFakeButtonEvent(self.display, 1, 1, 0)
        self.x11.XFlush(self.display)
        time.sleep(0.08)
        self.xtst.XTestFakeButtonEvent(self.display, 1, 0, 0)
        self.x11.XSync(self.display, 0)

    def capture(self, window, output):
        self.x11.XSync(self.display, 0)
        image = self.x11.XGetImage(self.display, int(window["id"], 16), 0, 0, window["width"],
                                  window["height"], ctypes.c_ulong(-1).value, 2)
        require(bool(image), "The X11 window image could not be captured")
        try:
            meta = image.contents
            require(meta.bits_per_pixel == 32 and meta.byte_order == 0
                    and (meta.red_mask, meta.green_mask, meta.blue_mask) == (0xFF0000, 0xFF00, 0xFF),
                    "The isolated Xvfb window has an unsupported pixel format")
            raw = ctypes.string_at(meta.data, meta.height * meta.bytes_per_line)
            scanlines = []
            for y in range(meta.height):
                start, stop = y * meta.bytes_per_line, y * meta.bytes_per_line + meta.width * 4
                row = bytearray(meta.width * 3)
                row[0::3] = raw[start + 2:stop:4]
                row[1::3] = raw[start + 1:stop:4]
                row[2::3] = raw[start:stop:4]
                scanlines.append(b"\0" + bytes(row))
            payload = b"\x89PNG\r\n\x1a\n"
            payload += png_chunk(b"IHDR", struct.pack(">IIBBBBB", meta.width, meta.height, 8, 2, 0, 0, 0))
            payload += png_chunk(b"IDAT", zlib.compress(b"".join(scanlines), 6))
            payload += png_chunk(b"IEND", b"")
            output.write_bytes(payload)
        finally:
            self.x11.XDestroyImage(image)
        return {"path": str(output), "sha256": hashlib.sha256(payload).hexdigest(),
                "source": "Actual X11 application window"}

    def close(self):
        self.x11.XCloseDisplay(self.display)


class Case:
    def __init__(self, arguments, profile, name, screen, pointer):
        self.arguments, self.profile, self.name, self.screen, self.pointer = arguments, profile, name, screen, pointer
        self.geometry = PROFILES[profile]
        self.directory = arguments.output / (profile + "-" + name)
        self.directory.mkdir(mode=0o700, parents=True, exist_ok=False)
        self.trace = self.directory / "ui.jsonl"
        self.capture = self.directory / "final.png"
        self.process = None
        self.window = None
        self.state = None
        self.result = {"name": name, "profile": profile, "passed": False, "input_delivery_protocol": INPUT_DELIVERY_PROTOCOL, "clicks": [], "observations": [],
                       "intent_delivery": "Actual XTest mouse input only; no --actions or named UI action calls"}

    def wait(self, description, predicate, timeout=30):
        deadline = time.monotonic() + timeout
        last_error = None
        while time.monotonic() < deadline:
            if self.process is not None:
                require(self.process.poll() is None, f"Application exited while waiting for {description}")
            try:
                value = predicate()
                if value:
                    return value
            except (FileNotFoundError, json.JSONDecodeError, sqlite3.Error) as error:
                last_error = str(error)
            time.sleep(0.04)
        raise AssertionError(f"Timed out waiting for {description}; last read error: {last_error}")

    def observe(self, description, expected=None, predicate=None, after=0):
        def match():
            for record in reversed(trace_records(self.trace)[after:]):
                if (record.get("event") != "ui-state" or record.get("process_id") != self.process.pid
                    or record.get("demo") is not True or record.get("view_model_fixture_only") is not False):
                    continue
                state = normalized(record)
                if (expected is None or expect(state, expected)) and (predicate is None or predicate(state)):
                    return record
            return None
        record = self.wait(description, match)
        self.state = normalized(record)
        self.result["observations"].append({"description": description, "expected": expected,
                                            "record": record})
        return self.state

    def snapshot(self, name):
        time.sleep(0.18)
        value = self.pointer.capture(self.window, self.directory / (name + ".png"))
        self.result.setdefault("screenshots", []).append(value)
        return value

    def click(self, name, point, expected=None, predicate=None):
        require(0 <= point[0] < self.window["width"] and 0 <= point[1] < self.window["height"],
                "The planned pointer coordinate is outside the actual application window")
        prior = len(trace_records(self.trace))
        self.pointer.click(self.window, point)
        self.result["clicks"].append({"name": name, "client": list(point), "window_id": self.window["id"],
                                     "monotonic_seconds": time.monotonic()})
        self.observe(name, expected, predicate, prior)
        self.snapshot(name)
        return self.state

    def start(self):
        environment = os.environ.copy()
        for key in CONTEXT_KEYS:
            environment.pop(key, None)
        environment["SLINT_SCALE_FACTOR"] = "1"
        command = [str(self.arguments.binary), "--demo", "--device", self.profile, "--screen", self.screen,
                   "--data-dir", str(self.directory / "data"), "--lifecycle-trace", str(self.trace),
                   "--capture", str(self.capture), "--exit-after-ms", "120000"]
        self.result["command"] = command
        self.log = (self.directory / "application.log").open("w")
        self.process = subprocess.Popen(command, env=environment, stdout=self.log, stderr=subprocess.STDOUT,
                                        start_new_session=True)
        self.result["process_id"] = self.process.pid
        self.wait("the real App ready trace", lambda: next((record for record in trace_records(self.trace)
                  if record.get("event") == "ready" and record.get("process_id") == self.process.pid), None), timeout=80)
        self.observe("the initial live UI snapshot", {"route": self.screen})
        self.window = self.wait("the mapped window after App readiness", self.pointer.window)
        require((self.window["width"], self.window["height"]) == self.geometry["size"],
                "The application window does not match the frozen logical geometry")
        self.result["window"] = self.window
        self.result["readiness_gate"] = "Real ready trace, matching process ID, initial live ui-state; then X11 mapping"
        self.snapshot("initial")

    def finish(self):
        os.kill(self.process.pid, signal.SIGTERM)
        require(self.process.wait(timeout=20) == 0, "The real Rust application failed during graceful capture")
        self.log.close()
        metadata = json.loads(self.capture.with_suffix(".json").read_text())
        require(metadata["demo"] is True and metadata["process_id"] == self.process.pid
                and metadata["startup"]["booting"] is False and metadata["startup"]["error"] is False,
                "The final capture did not come from the original healthy demo application")
        require(metadata["network_prompt"] is False, "A pointer flow unexpectedly opened the network prompt")
        require(metadata["route"] == field(self.state, "route") and metadata["profile"] == self.profile
                and metadata["backend"] == "desktop-software", "The final captured scene differs from the observed pointer result")
        self.result.update({"passed": True, "final_metadata": metadata, "final_capture": str(self.capture),
                            "final_ui_state": self.state})

    def close(self):
        if self.process is not None and self.process.poll() is None:
            self.process.terminate()
            try:
                self.process.wait(timeout=15)
            except subprocess.TimeoutExpired:
                os.killpg(self.process.pid, signal.SIGKILL)
                self.process.wait(timeout=5)
        if self.process is not None:
            try:
                os.killpg(self.process.pid, 0)
            except ProcessLookupError:
                self.result["process_group_cleaned"] = True
            else:
                os.killpg(self.process.pid, signal.SIGKILL)
                self.result["process_group_cleaned"] = False
                self.result["passed"] = False
                self.result.setdefault("error", "The demo application left an owned child process behind")
        if hasattr(self, "log") and not self.log.closed:
            self.log.close()
        self.result["trace_path"] = str(self.trace)
        write_json(self.directory / "result.json", self.result)


def search_controls(case):
    g = case.geometry
    width = g["size"][0]
    case.click("store-search-left-top-edge", (g["search_start"] + 2, g["status"] + 2),
               {"route": "search", "catalog.keyboard": True})
    case.click("search-back-to-store", g["search_back"], {"route": "store"})
    case.click("store-search-right-bottom-edge", (width - g["margin"] - 2, g["status"] + g["header"] - 3),
               {"route": "search", "catalog.keyboard": True})
    case.click("pinyin-n", g["n"], {"catalog.composition": "n"})
    case.click("pinyin-ni", g["i"], {"catalog.composition": "ni", "catalog.candidate_page": 0},
               lambda state: (field(state, "catalog.candidate_pages") or 0) > 1)
    first_candidates = list(field(case.state, "catalog.candidates") or [])
    require(first_candidates, "The real Pinyin input has no visible first-page candidates")
    case.click("next-candidates", g["candidate_next"], {"catalog.candidate_page": 1})
    second_candidates = list(field(case.state, "catalog.candidates") or [])
    require(second_candidates and second_candidates != first_candidates, "The real next button did not change candidate contents")
    case.click("previous-candidates", g["candidate_previous"], {"catalog.candidate_page": 0})
    require(field(case.state, "catalog.candidates") == first_candidates, "The real previous button did not restore the candidate page")
    case.click("next-candidates-again", g["candidate_next"], {"catalog.candidate_page": 1})
    chosen = field(case.state, "catalog.candidates")[0]
    case.click("commit-visible-candidate", g["candidate_first"],
               {"catalog.query": chosen, "catalog.composition": "", "catalog.candidate_page": 0})
    case.click("hide-keyboard", g["hide"], {"catalog.keyboard": False, "catalog.query": chosen})
    case.click("search-field-left-edge", g["field_left"], {"catalog.keyboard": True, "catalog.query": chosen})
    case.click("hide-keyboard-again", g["hide"], {"catalog.keyboard": False, "catalog.query": chosen})
    case.click("search-field-right-usable-edge", g["field_right"], {"catalog.keyboard": True, "catalog.query": chosen})
    case.result["candidate_evidence"] = {"first_page": first_candidates, "second_page": second_candidates,
                                        "chosen_visible_candidate": chosen}


def detail_controls(case):
    g = case.geometry
    require(field(case.state, "catalog.episode_page") == 2, "The fresh canonical detail did not locate the current chapter")
    case.click("enter-detail-selection", g["detail_enter"], {"catalog.download_selecting": True,
                                                           "catalog.selection_count": 0})
    case.click("select-visible-readable-chapter", g["detail_row"], {"catalog.selection_count": 1})
    case.click("cancel-detail-selection-right-edge", g["detail_cancel"], {"catalog.download_selecting": False,
                                                                        "catalog.selection_count": 0})
    case.click("next-detail-page", g["detail_next"], {"catalog.episode_page": 3})
    case.click("locate-current-chapter", g["detail_locate"], {"catalog.episode_page": 2,
                                                           "catalog.episode_filter": "all"})
    require(field(case.state, "catalog.detail_id") == "28601", "The pointer detail flow changed the canonical comic")


def no_history_cover(case):
    database = case.directory / "data/demo/state.sqlite3"
    def anchor_count():
        with sqlite3.connect(database.as_uri() + "?mode=ro", uri=True, timeout=5) as connection:
            return connection.execute(
                "SELECT COUNT(*) FROM reading_anchors WHERE account_key='design-demo' AND comic_id='28606'").fetchone()[0]
    require(anchor_count() == 0, "The no-history fixture already has a reader anchor")
    case.click("no-history-moon-cover-to-detail", case.geometry["no_history_cover"],
               {"route": "detail", "catalog.detail_id": "28606"})
    require(anchor_count() == 0, "The no-history cover click created a reader anchor")
    require(not any(record.get("event") == "reader-ready" for record in trace_records(case.trace)),
            "The no-history cover click entered the reader")
    case.result["sqlite_anchor_evidence"] = {"comic_id": "28606", "before": 0, "after": 0,
                                            "database": str(database), "access": "Read only"}


def download_controls(case):
    g = case.geometry
    before_deep, before_snow = item(case.state, "28602"), item(case.state, "28603")
    require(before_deep and before_snow and before_deep["active"] is True, "The fresh demo download rows do not match the fixture")
    case.click("pause-all", g["toggle_all"], {"downloads.all_paused": True},
               lambda state: item(state, "28602") and item(state, "28602")["active"] is False
               and item(state, "28602").get("has_bar") is True
               and item(state, "28602")["progress"] == before_deep["progress"])
    paused_deep, paused_snow = item(case.state, "28602"), item(case.state, "28603")
    case.result["pause_progress_evidence"] = {"before": before_deep["progress"],
                                               "paused": paused_deep["progress"],
                                               "paused_has_bar": paused_deep["has_bar"]}
    require(paused_deep["status"] != before_deep["status"] and paused_snow["status"] != before_snow["status"],
            "The real pause button did not change both active and queued row statuses")
    case.click("resume-all", g["toggle_all"], {"downloads.all_paused": False},
               lambda state: item(state, "28602") and item(state, "28602")["active"] is True
               and item(state, "28602")["status"] == before_deep["status"]
               and item(state, "28603")["status"] == before_snow["status"])
    case.click("open-failed-crane", g["download_first"], {"downloads.sheet_open": True,
                                                        "downloads.sheet_id": "28607",
                                                        "downloads.task_primary_action": "retry"})
    case.click("retry-crane-creates-second-queued-task", g["task_primary"],
               {"downloads.task_primary_action": "priority"}, lambda state: ids(state)[:3] == ["28602", "28607", "28603"])
    case.click("close-crane-sheet", g["sheet_done"], {"downloads.sheet_open": False},
               lambda state: ids(state)[:3] == ["28602", "28607", "28603"])
    before_priority = ids(case.state)
    case.click("open-queued-snow", g["download_third"], {"downloads.sheet_open": True,
                                                       "downloads.sheet_id": "28603",
                                                       "downloads.task_primary_action": "priority"})
    case.click("prioritize-queued-snow", g["task_primary"], {"downloads.sheet_id": "28603"},
               lambda state: ids(state)[:3] == ["28602", "28603", "28607"])
    case.click("close-prioritized-sheet", g["sheet_done"], {"downloads.sheet_open": False},
               lambda state: ids(state)[:3] == ["28602", "28603", "28607"])
    case.result["local_priority_feedback"] = {"before_ids": before_priority, "after_ids": ids(case.state),
                                             "proof": "The real queued-task priority click changed the visible local row order",
                                             "second_queued_task_created_by": "Actual retry control click on the failed demo task",
                                             "remote_download_started": False, "sqlite_priority_claimed": False}


def main():
    os.umask(0o077)
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--workspace", type=Path, required=True)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--expected-binary-sha256", required=True)
    parser.add_argument("--profiles", nargs="+", choices=list(PROFILES), default=list(PROFILES))
    parser.add_argument("--cases", nargs="+", choices=["search", "detail", "no-history", "downloads"],
                        default=["search", "detail", "no-history", "downloads"])
    arguments = parser.parse_args()
    require(os.environ.get("DISPLAY"), "Run this harness with isolated xvfb-run")
    workspace = arguments.workspace.resolve(strict=True)
    require(arguments.binary.is_file() and os.access(arguments.binary, os.X_OK), "The selected desktop binary is unavailable")
    require(arguments.output.resolve().is_relative_to(workspace) and arguments.output.resolve() != workspace,
            "Output must be a new directory inside the isolated workspace")
    digest = hashlib.sha256(arguments.binary.read_bytes()).hexdigest()
    if arguments.expected_binary_sha256:
        require(digest == arguments.expected_binary_sha256, "The binary hash does not match the selected new build")
    arguments.output.mkdir(mode=0o700, parents=True, exist_ok=False)
    manifest = {"passed": False, "suite": "promoted_design_v2_pointer", "input_delivery_protocol": INPUT_DELIVERY_PROTOCOL, "harness_path": str(Path(__file__).resolve()), "harness_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(), "started_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
                "binary": str(arguments.binary), "binary_sha256": digest, "cases": [],
                "live_account_used": False, "named_ui_actions_used": False,
                "requires": "A demo-only event=ui-state JSONL snapshot with Catalog and Downloads observable fields",
                "coverage": "Real Rust desktop UI and XTest pointer controls",
                "limitations": ["No physical Kindle or firmware-emulator acceptance is established"]}
    pointer = Pointer()
    plans = {"search": ("store", search_controls), "detail": ("detail", detail_controls),
             "no-history": ("shelf", no_history_cover), "downloads": ("downloads", download_controls)}
    try:
        for name in arguments.cases:
            for profile in arguments.profiles:
                screen, drive = plans[name]
                case = Case(arguments, profile, name, screen, pointer)
                try:
                    case.start()
                    drive(case)
                    case.finish()
                except Exception as error:
                    case.result.update({"passed": False, "error": str(error), "traceback": traceback.format_exc()})
                finally:
                    case.close()
                manifest["cases"].append(case.result)
                write_json(arguments.output / "pointer-v2-result.json", manifest)
                print(json.dumps({"profile": profile, "case": name, "passed": case.result["passed"],
                                  "error": case.result.get("error"), "directory": str(case.directory)}), flush=True)
        manifest["passed"] = all(case["passed"] for case in manifest["cases"]) and bool(manifest["cases"])
    finally:
        pointer.close()
        manifest["finished_at"] = datetime.datetime.now(datetime.timezone.utc).isoformat()
        write_json(arguments.output / "pointer-v2-result.json", manifest)
    return 0 if manifest["passed"] else 1


if __name__ == "__main__":
    from harness_signals import install_interruption_handlers
    install_interruption_handlers()
    sys.exit(main())

