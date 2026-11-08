#!/usr/bin/env python3
"""Exercise targeted v2 Reader behavior in a private remote desktop demo session.

Named intents only establish fixture state. All behavior under examination uses
XTEST input delivered to the real Rust/Slint application window. Screenshots
require a separate human review; passing state assertions do not approve layout.
"""

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

CONTEXT_KEYS = (
    "BILICOMICS_LIFECYCLE_DIR",
    "BILICOMICS_LAUNCH_TOKEN",
    "BILICOMICS_LAUNCH_GENERATION",
)
PROFILES = {
    "pw5": (618, 824),
    "pw6": (632, 840),
    "scribe": (930, 1240),
}


def require(value, message):
    if not value:
        raise AssertionError(message)


def write_json(path, value):
    path.write_text(json.dumps(value, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")


def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def trace_records(path):
    if not path.exists():
        return []
    records = []
    for line in path.read_text(encoding="utf-8").splitlines(keepends=True):
        if line.endswith("\n") and line.strip():
            records.append(json.loads(line))
    return records


def matching(state, expected):
    return all(state.get(key) == value for key, value in (expected or {}).items())


def png_chunk(kind, data):
    return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)


class XImage(ctypes.Structure):
    _fields_ = [
        ("width", ctypes.c_int), ("height", ctypes.c_int), ("xoffset", ctypes.c_int),
        ("format", ctypes.c_int), ("data", ctypes.c_void_p), ("byte_order", ctypes.c_int),
        ("bitmap_unit", ctypes.c_int), ("bitmap_bit_order", ctypes.c_int),
        ("bitmap_pad", ctypes.c_int), ("depth", ctypes.c_int), ("bytes_per_line", ctypes.c_int),
        ("bits_per_pixel", ctypes.c_int), ("red_mask", ctypes.c_ulong),
        ("green_mask", ctypes.c_ulong), ("blue_mask", ctypes.c_ulong),
    ]


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
        self.xtst.XTestFakeMotionEvent.argtypes = [
            ctypes.c_void_p, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_ulong
        ]
        self.xtst.XTestFakeButtonEvent.argtypes = [
            ctypes.c_void_p, ctypes.c_uint, ctypes.c_int, ctypes.c_ulong
        ]
        self.x11.XGetImage.argtypes = [
            ctypes.c_void_p, ctypes.c_ulong, ctypes.c_int, ctypes.c_int,
            ctypes.c_uint, ctypes.c_uint, ctypes.c_ulong, ctypes.c_int,
        ]
        self.x11.XGetImage.restype = ctypes.POINTER(XImage)
        self.x11.XDestroyImage.argtypes = [ctypes.POINTER(XImage)]
        self.display = self.x11.XOpenDisplay(None)
        require(self.display, "Run this harness inside its own Xvfb display")

    def window(self, process=None):
        tree = subprocess.check_output(["xwininfo", "-root", "-tree"], text=True, timeout=5)
        identifiers = re.findall(r'(0x[0-9a-fA-F]+) "BiliComics - Design Preview"', tree)
        require(len(identifiers) <= 1, "The isolated display contains multiple application windows")
        if not identifiers:
            return None
        owner = subprocess.check_output(
            ["xprop", "-id", identifiers[0], "_NET_WM_PID"], text=True, timeout=5,
        )
        pid = re.search(r"_NET_WM_PID\(CARDINAL\)\s*=\s*(\d+)", owner)
        require(pid is not None, "The selected application window has no process identity")
        if process is not None:
            require(int(pid.group(1)) == process.pid, "The selected application window belongs to another process")
        details = subprocess.check_output(["xwininfo", "-id", identifiers[0]], text=True, timeout=5)
        if "Map State: IsViewable" not in details:
            return None
        patterns = {
            "x": r"Absolute upper-left X:\s*(-?\d+)",
            "y": r"Absolute upper-left Y:\s*(-?\d+)",
            "width": r"Width:\s*(\d+)",
            "height": r"Height:\s*(\d+)",
        }
        return {"id": identifiers[0], "process_id": int(pid.group(1)), **{
            name: int(re.search(pattern, details).group(1)) for name, pattern in patterns.items()
        }}

    def click(self, window, point):
        self.xtst.XTestFakeMotionEvent(
            self.display, self.x11.XDefaultScreen(self.display),
            window["x"] + point[0], window["y"] + point[1], 0,
        )
        self.x11.XFlush(self.display)
        self.x11.XSync(self.display, 0)
        time.sleep(0.08)
        self.xtst.XTestFakeButtonEvent(self.display, 1, 1, 0)
        self.x11.XFlush(self.display)
        time.sleep(0.08)
        self.xtst.XTestFakeButtonEvent(self.display, 1, 0, 0)
        self.x11.XSync(self.display, 0)

    def capture(self, window, path):
        self.x11.XSync(self.display, 0)
        image = self.x11.XGetImage(
            self.display, int(window["id"], 16), 0, 0, window["width"], window["height"],
            ctypes.c_ulong(-1).value, 2,
        )
        require(bool(image), "The actual X11 application image could not be captured")
        try:
            meta = image.contents
            require(
                meta.bits_per_pixel == 32 and meta.byte_order == 0
                and (meta.red_mask, meta.green_mask, meta.blue_mask) == (0xFF0000, 0xFF00, 0xFF),
                "The isolated display has an unsupported pixel format",
            )
            raw = ctypes.string_at(meta.data, meta.height * meta.bytes_per_line)
            scanlines = []
            pixels = hashlib.sha256()
            for y in range(meta.height):
                start, stop = y * meta.bytes_per_line, y * meta.bytes_per_line + meta.width * 4
                row = bytearray(meta.width * 3)
                row[0::3] = raw[start + 2:stop:4]
                row[1::3] = raw[start + 1:stop:4]
                row[2::3] = raw[start:stop:4]
                pixels.update(row)
                scanlines.append(b"\0" + bytes(row))
            payload = b"\x89PNG\r\n\x1a\n"
            payload += png_chunk(b"IHDR", struct.pack(">IIBBBBB", meta.width, meta.height, 8, 2, 0, 0, 0))
            payload += png_chunk(b"IDAT", zlib.compress(b"".join(scanlines), 6))
            payload += png_chunk(b"IEND", b"")
            path.write_bytes(payload)
            return {
                "path": str(path), "sha256": hashlib.sha256(payload).hexdigest(),
                "rgb_sha256": pixels.hexdigest(), "dimensions": [meta.width, meta.height],
                "source": "Actual X11 application window",
            }
        finally:
            self.x11.XDestroyImage(image)

    def close(self):
        self.x11.XCloseDisplay(self.display)


class Case:
    def __init__(self, arguments, pointer, profile, name, landscape, actions, expected):
        self.arguments = arguments
        self.pointer = pointer
        self.profile = profile
        self.name = name
        self.landscape = landscape
        self.actions = actions
        self.expected = expected
        self.size = tuple(reversed(PROFILES[profile])) if landscape else PROFILES[profile]
        self.directory = arguments.output / (profile + "-" + name)
        self.directory.mkdir(mode=0o700, parents=True, exist_ok=False)
        self.trace = self.directory / "lifecycle.jsonl"
        self.capture = self.directory / "final.png"
        self.process = None
        self.window = None
        self.state = None
        self.final_expected = {"menu": False, "sheet": ""}
        self.result = {
            "name": name, "profile": profile, "landscape": landscape,
            "passed": False, "runtime_passed": False, "clicks": [], "observations": [],
            "screenshots": [], "visual_review_required": [],
            "fixture_setup": actions,
            "intent_delivery": "Named fixture setup only; behavior under test uses actual XTEST mouse input",
        }

    def wait(self, description, predicate, timeout=30):
        deadline = time.monotonic() + timeout
        last_error = None
        while time.monotonic() < deadline:
            if self.process is not None:
                require(self.process.poll() is None, "Application exited while waiting for " + description)
            try:
                value = predicate()
                if value:
                    return value
            except (FileNotFoundError, json.JSONDecodeError, sqlite3.Error) as error:
                last_error = str(error)
            time.sleep(0.04)
        raise AssertionError("Timed out waiting for " + description + "; last read error: " + str(last_error))

    def observe_reader(self, description, expected=None, predicate=None, after=0):
        def match():
            for record in reversed(trace_records(self.trace)[after:]):
                state = record.get("reader_state")
                frame = record.get("frame_size")
                if not (
                    record.get("event") == "reader-ready"
                    and record.get("process_id") == self.process.pid
                    and record.get("route") == "reader"
                    and record.get("reader_loading") is False
                    and isinstance(state, dict)
                    and isinstance(frame, list) and len(frame) == 2 and min(frame) > 0
                    and state.get("frame_width", 0) > 0 and state.get("frame_height", 0) > 0
                ):
                    continue
                if matching(state, expected) and (predicate is None or predicate(state)):
                    return record
            return None
        record = self.wait(description, match)
        self.state = record["reader_state"]
        require(
            self.state["comic_id"] == self.arguments.fixture_comic
            and self.state["episode_id"] == self.arguments.fixture_episode,
            "The live Reader identity differs from the frozen canonical demo fixture",
        )
        self.result["observations"].append({
            "description": description, "expected": expected,
            "record_index": trace_records(self.trace).index(record), "record": record,
        })
        return self.state

    def snapshot(self, name):
        time.sleep(0.22)
        value = self.pointer.capture(self.window, self.directory / (name + ".png"))
        value["name"] = name
        self.result["screenshots"].append(value)
        return value

    def click(self, name, point):
        require(
            0 <= point[0] < self.window["width"] and 0 <= point[1] < self.window["height"],
            "The planned click is outside the actual application client area",
        )
        prior = len(trace_records(self.trace))
        self.pointer.click(self.window, point)
        self.result["clicks"].append({
            "name": name, "client": list(point), "window_id": self.window["id"],
            "monotonic_seconds": time.monotonic(), "trace_records_before": prior,
        })
        return prior

    def click_reader(self, name, point, expected=None, predicate=None):
        prior = self.click(name, point)
        self.observe_reader(name, expected, predicate, prior)
        self.snapshot(name)
        return self.state

    def turn_point(self, forward):
        fraction = 0.85 if (forward != (self.state["direction"] == "rtl")) else 0.15
        return (round(self.size[0] * fraction), self.size[1] // 2)

    def turn(self, name, forward, expected):
        return self.click_reader(name, self.turn_point(forward), expected)

    def last_view_point(self):
        inset = 56 if self.profile == "scribe" else 28
        bottom = 48 if self.profile == "scribe" else 28
        height = 60 if self.profile == "scribe" else 52
        width = (self.size[0] - 2 * inset - 20) / 3
        return (round(inset + width / 2), round(self.size[1] - bottom - height / 2))

    def assert_optional_layout(self, spread, left=None, right=None):
        observed = {}
        for key in ("effective_spread", "is_spread"):
            if key in self.state:
                observed[key] = self.state[key]
                require(self.state[key] is spread, "Effective spread metadata does not match the canvas")
        for keys, expected in (
            (("left_page", "left_page_index", "left_page_number"), left),
            (("right_page", "right_page_index", "right_page_number"), right),
        ):
            if expected is not None:
                for key in keys:
                    if key in self.state:
                        observed[key] = self.state[key]
                        require(self.state[key] == expected, "The physical spread slot metadata differs")
        for key in ("effective_fit", "second_frame_width", "second_frame_height", "second_frame_size"):
            if key in self.state:
                observed[key] = self.state[key]
        self.result.setdefault("optional_layout_observables", []).append(observed)

    def start(self):
        environment = os.environ.copy()
        for key in CONTEXT_KEYS:
            environment.pop(key, None)
        environment["SLINT_SCALE_FACTOR"] = "1"
        actions_path = self.directory / "fixture-actions.json"
        write_json(actions_path, self.actions)
        command = [
            str(self.arguments.binary), "--demo", "--device", self.profile, "--screen", "reader",
            "--data-dir", str(self.directory / "data"), "--actions", str(actions_path),
            "--lifecycle-trace", str(self.trace), "--capture", str(self.capture),
            "--exit-after-ms", "180000",
        ]
        if self.landscape:
            command.append("--landscape")
        self.result["command"] = command
        self.log = (self.directory / "application.log").open("w")
        self.process = subprocess.Popen(
            command, env=environment, stdout=self.log, stderr=subprocess.STDOUT, start_new_session=True,
        )
        self.result["process_id"] = self.process.pid
        self.wait(
            "the real App ready trace",
            lambda: next((
                record for record in trace_records(self.trace)
                if record.get("event") == "ready" and record.get("process_id") == self.process.pid
            ), None),
            timeout=self.arguments.startup_timeout,
        )
        self.observe_reader("loaded Reader after fixture setup", self.expected)
        self.window = self.wait(
            "the mapped Reader window after actual frame readiness", lambda: self.pointer.window(self.process),
        )
        require(
            (self.window["width"], self.window["height"]) == self.size,
            "The actual application window differs from the frozen design profile",
        )
        self.result["window"] = self.window
        self.result["readiness_gate"] = (
            "Matching PID event=ready followed by event=reader-ready, route=reader, "
            "reader_loading=false, positive frame size and expected real reader_state; then X11 mapping. "
            "The SC LTR cover placeholder is a legitimate positive 1x1 primary frame."
        )
        self.assert_private_index()
        self.snapshot("fixture-ready")

    def assert_private_index(self):
        database = self.directory / "data/demo/state.sqlite3"
        with sqlite3.connect(database.as_uri() + "?mode=ro", uri=True, timeout=5) as connection:
            rows = connection.execute(
                "SELECT revision,payload FROM image_indices WHERE account_key=? AND episode_id=?",
                ("design-demo", self.arguments.fixture_episode),
            ).fetchall()
        counts = [{"revision": revision, "page_count": len(json.loads(payload)["images"])} for revision, payload in rows]
        require(counts and all(row["page_count"] == self.arguments.fixture_pages for row in counts),
                "The private demo image index disagrees with the frozen source fixture")
        self.result["private_index_evidence"] = {
            "database": str(database), "access": "Read only", "episode_id": self.arguments.fixture_episode,
            "source_fixture_page_count": self.arguments.fixture_pages, "indices": counts,
        }

    def visual_review(self, screenshot_name, checks):
        screenshot = next(item for item in self.result["screenshots"] if item["name"] == screenshot_name)
        self.result["visual_review_required"].append({
            "status": "pending", "artifact": screenshot["path"], "checks": checks,
        })

    def finish(self):
        os.kill(self.process.pid, signal.SIGTERM)
        require(self.process.wait(timeout=20) == 0, "The Rust application failed during graceful final capture")
        self.log.close()
        metadata = json.loads(self.capture.with_suffix(".json").read_text(encoding="utf-8"))
        reader = metadata.get("reader", {})
        require(
            metadata.get("process_id") == self.process.pid and metadata.get("demo") is True
            and metadata.get("backend") == "desktop-software" and metadata.get("profile") == self.profile
            and metadata.get("route") == "reader" and metadata.get("landscape") is self.landscape
            and metadata.get("startup", {}).get("booting") is False
            and metadata.get("startup", {}).get("error") is False
            and metadata.get("network_prompt") is False,
            "The final capture is not the original healthy private desktop demo Reader",
        )
        require(metadata.get("snapshot_size") == list(self.size), "The final capture dimensions changed")
        expected = {
            "page": self.state["page_number"], "loading": False,
            "mode": self.state["mode"], "direction": self.state["direction"],
            "transition": self.state["transition"], **self.final_expected,
        }
        if self.state["mode"] == "strip":
            expected["strip_screen"] = self.state["crop_index"] + 1
            expected["pages"] = 1
        else:
            expected["pages"] = self.arguments.fixture_pages
        require(matching(reader, expected), "The final Reader view model differs from the observed pointer result")
        self.result.update({
            "passed": True, "runtime_passed": True, "final_reader_state": self.state,
            "final_capture": str(self.capture), "final_capture_sha256": sha256(self.capture),
            "final_metadata": metadata, "expected_final_reader": expected,
        })

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
                self.result["runtime_passed"] = False
                self.result.setdefault("error", "The private demo application left an owned child process behind")
        if hasattr(self, "log") and not self.log.closed:
            self.log.close()
        self.result["trace_path"] = str(self.trace)
        self.result["last_trace_records"] = trace_records(self.trace)[-4:]
        write_json(self.directory / "result.json", self.result)


def fixture_actions(settings, page=None):
    intents = [("setting", value) for value in settings]
    if page is not None:
        intents.append(("page", str(page)))
    return [
        {"at_ms": 200 * (index + 1), "section": "reader", "action": action, "value": value}
        for index, (action, value) in enumerate(intents)
    ]


def page_setup(page, direction="ltr"):
    settings = ["mode:page", "fit:page", "landscape-layout:spread", "cover-alone:on", "direction:" + direction]
    return fixture_actions(settings, page), {
        "mode": "page", "fit": "page", "landscape_layout": "spread",
        "cover_alone": True, "direction": direction, "page_number": page, "transition": False,
    }


def beginning_closes_menu(case):
    width, height = case.size
    require(not (case.landscape and width >= 1100), "The SC landscape reference has no beginning control")
    case.click("open-reading-menu", (width // 2, height // 2))
    case.snapshot("menu-before-beginning")
    point = (89, height - 136) if case.profile == "scribe" else (61, height - 114)
    case.click_reader("beginning-with-menu-open", point, {
        "page_number": 1, "crop_index": 0, "transition": False, "menu": False, "sheet": "",
    })
    case.final_expected = {"menu": False, "sheet": ""}
    case.visual_review("beginning-with-menu-open", [
        "Returning to page 1 closes the menu and restores quiet full-canvas reading.",
        "Source page 1 is visible without a persistent footer, badge, progress label, or menu sheet.",
    ])
    case.result["assertion"] = (
        "A real center tap opened the menu, a real beginning tap moved page 5 to page 1, "
        "and the final live ReaderModel must report menu=false and an empty sheet."
    )
    case.result["authorized_behavior_change"] = (
        "The immersive Reader replaces the former keep-menu-open expectation with an explicit close "
        "after a page or progress selection. Historical verification outputs are not overwritten."
    )


def strip_last_view(case):
    require(case.state["mode"] == "strip" and case.state["demo_strip"] is True,
            "This case requires the independent strip fixture")
    require(case.state["crop_count"] > 1, "The loaded strip did not create multiple real viewports")
    turns = 0
    while case.state["crop_index"] + 1 < case.state["crop_count"]:
        turns += 1
        require(turns <= 96, "The targeted strip loop exceeded its safety bound")
        target = case.state["crop_index"] + 1
        case.turn("strip-next-" + str(target + 1), True, {"crop_index": target, "transition": False})
    before = dict(case.state)
    before_image = case.snapshot("strip-final-viewport-before-end")
    require(before["source_y"] > 0, "The final strip viewport is still at the source top")
    case.turn("strip-enter-episode-transition", True, {"transition": True})
    case.visual_review("strip-enter-episode-transition", [
        "The actual episode transition is visible after the final strip viewport.",
        "No additional source page was invented for the independent strip.",
    ])
    case.click_reader("strip-return-last-view", case.last_view_point(), {
        "transition": False, "crop_index": before["crop_index"], "crop_count": before["crop_count"],
        "source_x": before["source_x"], "source_y": before["source_y"], "page_id": before["page_id"],
    })
    after_image = case.snapshot("strip-final-viewport-after-return")
    require(before_image["rgb_sha256"] == after_image["rgb_sha256"],
            "Returning from the transition did not restore the exact rendered final strip viewport")
    require(case.state["crop_index"] + 1 == case.state["crop_count"],
            "The last-view action returned to an earlier strip viewport")
    case.result["strip_evidence"] = {
        "actual_pointer_turns_to_last_viewport": turns,
        "before": before, "after": dict(case.state),
        "before_rgb_sha256": before_image["rgb_sha256"],
        "after_rgb_sha256": after_image["rgb_sha256"], "exact_rendered_pixels_restored": True,
    }
    case.visual_review("strip-final-viewport-after-return", [
        "The same final strip viewport is restored; its source bottom is visible.",
    ])


def cover_slots(case):
    require(case.profile == "scribe" and case.landscape and case.state["page_number"] == 1,
            "This case requires the first SC landscape source page")
    direction = case.state["direction"]
    expected_slots = [0, 1] if direction == "ltr" else [1, 0]
    case.assert_optional_layout(True, expected_slots[0], expected_slots[1])
    if direction == "ltr":
        require((case.state["frame_width"], case.state["frame_height"]) == (1, 1),
                "The first LTR cover must retain an empty physical primary slot")
    else:
        require(min(case.state["frame_width"], case.state["frame_height"]) > 1,
                "The first RTL cover must place source page 1 in the physical primary slot")
    case.result["cover_slot_expectation"] = {
        "direction": direction, "physical_slots": expected_slots,
        "meaning": "0 denotes the empty cover-alone slot; 1 denotes actual source page 1",
        "primary_frame": [case.state["frame_width"], case.state["frame_height"]],
        "slot_placement_requires_visual_review": True,
    }
    case.visual_review("fixture-ready", [
        "LTR has an empty left slot and source page 1 in the right slot." if direction == "ltr"
        else "RTL has source page 1 in the left slot and an empty right slot.",
        "The empty slot is plain paper without duplicated artwork, a dashed frame, or a cover-alone hint.",
        "Actual source page 1 is visibly rendered in the occupied slot.",
    ])
    case.turn("cover-to-first-pair", True, {"page_number": 2, "transition": False})
    case.assert_optional_layout(True, 2 if direction == "ltr" else 3, 3 if direction == "ltr" else 2)
    case.turn("first-pair-to-second-pair", True, {"page_number": 4, "transition": False})
    case.turn("second-pair-back-to-first-pair", False, {"page_number": 2, "transition": False})
    case.turn("first-pair-back-to-cover", False, {"page_number": 1, "transition": False})
    case.assert_optional_layout(True, expected_slots[0], expected_slots[1])
    case.result["source_page_sequence"] = [1, 2, 4, 2, 1]


def small_landscape_single(case):
    require(case.profile in ("pw5", "pw6") and case.landscape and case.size[0] < 1100,
            "This case requires an ineligible small landscape canvas")
    before = dict(case.state)
    require(before["landscape_layout"] == "spread" and before["mode"] == "page" and before["fit"] == "page",
            "The fixture must exercise the spread preference on a small landscape canvas")
    case.assert_optional_layout(False)
    expected_frame = list(case.size)
    require([before["frame_width"], before["frame_height"]] == expected_frame,
            "The small landscape Reader did not render the single-page viewport")
    case.turn("small-landscape-next-source-page", True, {
        "page_number": before["page_number"] + 1, "transition": False,
    })
    require(case.state["page_index"] - before["page_index"] == 1,
            "The actual small landscape turn skipped a source page as a spread")
    case.assert_optional_layout(False)
    case.result["small_canvas_evidence"] = {
        "requested_layout": "spread", "canvas": list(case.size),
        "single_page_frame": expected_frame,
        "before_page_index": before["page_index"], "after_page_index": case.state["page_index"],
        "actual_source_page_increment": 1,
    }
    case.visual_review("small-landscape-next-source-page", [
        "PW landscape displays one source page without paired page labels, a second leaf, or a center gutter.",
        "The single source page fits the actual small landscape canvas.",
    ])


def end_controls(case):
    require(case.landscape and case.state["page_number"] == case.arguments.fixture_pages,
            "This case must start on the real last page from the frozen demo image index")
    case.turn("landscape-enter-episode-transition", True, {"transition": True})
    case.visual_review("landscape-enter-episode-transition", [
        "The ended-episode heading, next-episode information, price card, primary purchase/continue action, "
        "and explanatory text have separate visible areas.",
        "The three bottom controls are fully visible, mutually separate, and contained in the viewport.",
        "The primary action and explanatory text do not overlap the bottom controls.",
    ])
    case.click_reader("landscape-return-last-view", case.last_view_point(), {
        "transition": False, "page_number": case.arguments.fixture_pages, "crop_index": 0,
    })
    case.result["end_evidence"] = {
        "last_source_page_from_private_index": case.arguments.fixture_pages,
        "last_view_control_activated_by": "Actual XTEST click at the visible bottom-left control",
        "returned_to_same_episode": True,
    }


def plans(arguments):
    result = []
    if "beginning" in arguments.cases:
        for profile in arguments.profiles:
            actions, expected = page_setup(5)
            result.append((profile, "portrait-beginning-closes-menu", False, actions, expected, beginning_closes_menu))
    if "strip" in arguments.cases:
        profile = next((name for name in arguments.profiles if name in ("pw5", "pw6")), arguments.profiles[0])
        actions = fixture_actions(["mode:strip"])
        expected = {"mode": "strip", "demo_strip": True, "transition": False}
        result.append((profile, "portrait-strip-last-view", False, actions, expected, strip_last_view))
    if "cover" in arguments.cases and "scribe" in arguments.profiles:
        for direction in ("ltr", "rtl"):
            actions, expected = page_setup(1, direction)
            result.append(("scribe", "landscape-cover-" + direction, True, actions, expected, cover_slots))
    if "single" in arguments.cases:
        for profile in (name for name in arguments.profiles if name in ("pw5", "pw6")):
            for direction in ("ltr", "rtl"):
                actions, expected = page_setup(5, direction)
                result.append((profile, "landscape-single-" + direction, True, actions, expected, small_landscape_single))
    if "end" in arguments.cases:
        for profile in arguments.profiles:
            actions, expected = page_setup(arguments.fixture_pages)
            result.append((profile, "landscape-end-controls", True, actions, expected, end_controls))
    return result


def main():
    os.umask(0o077)
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--workspace", type=Path, required=True)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--app-root", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--expected-binary-sha256", required=True)
    parser.add_argument("--source-sha256", required=True)
    parser.add_argument("--source-manifest", type=Path)
    parser.add_argument("--profiles", nargs="+", choices=list(PROFILES), default=["pw5", "scribe"])
    parser.add_argument("--cases", nargs="+", choices=["beginning", "strip", "cover", "single", "end"],
                        default=["beginning", "strip", "cover", "single", "end"])
    parser.add_argument("--startup-timeout", type=int, default=90)
    arguments = parser.parse_args()
    require(os.environ.get("DISPLAY"), "Run this remote harness through a dedicated xvfb-run display")
    workspace = arguments.workspace.resolve(strict=True)
    require(arguments.binary.is_file() and os.access(arguments.binary, os.X_OK), "The selected desktop binary is unavailable")
    require(arguments.output.resolve().is_relative_to(workspace) and arguments.output.resolve() != workspace,
            "All fixture data and evidence must stay inside the new isolated remote workspace")
    digest = sha256(arguments.binary)
    require(digest == arguments.expected_binary_sha256.lower(), "The binary differs from the coordinator's selected build")
    require(re.fullmatch(r"[a-fA-F0-9]{64}", arguments.source_sha256),
            "A declared frozen source SHA-256 identity is required")
    if arguments.source_manifest:
        require(sha256(arguments.source_manifest) == arguments.source_sha256.lower(),
                "The source manifest digest differs from the declared frozen build input")
    fixture_path = arguments.app_root / "assets/demo/catalog.json"
    fixture = json.loads(fixture_path.read_text(encoding="utf-8-sig"))
    resume = fixture["continue_reading"]
    arguments.fixture_comic = str(resume["comic_id"])
    arguments.fixture_episode = str(resume["episode_id"])
    arguments.fixture_pages = int(resume["page_count"])
    require(arguments.fixture_pages > 5, "The canonical demo fixture has too few actual source pages")
    arguments.output.mkdir(mode=0o700, parents=True, exist_ok=False)
    manifest = {
        "passed": False, "runtime_passed": False,
        "started_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        "binary": str(arguments.binary), "binary_sha256": digest,
        "declared_build_input_source_sha256": arguments.source_sha256.lower(),
        "source_manifest": str(arguments.source_manifest) if arguments.source_manifest else None,
        "fixture_source": str(fixture_path), "fixture_source_sha256": sha256(fixture_path),
        "canonical_demo_episode": arguments.fixture_episode, "actual_fixture_page_count": arguments.fixture_pages,
        "harness": str(Path(__file__).absolute()), "harness_sha256": sha256(Path(__file__)),
        "coverage": "Real Rust/Slint desktop Reader with XTEST pointer controls and loaded-frame lifecycle evidence",
        "input_delivery_protocol": {"method": "XTest", "motion_separate_flush_and_sync": True,
                                    "pre_down_event_loop_slot_ms": 80, "tap_hold_ms": 80,
                                    "scope": "Headless harness timing; no production touch fix is claimed"},
        "cases": [], "live_account_used": False, "remote_transactions_performed": False,
        "named_intents_used_for": "Fixture state only; no behavior under examination",
        "visual_review": {"status": "pending", "required": True,
                          "policy": "Runtime assertions do not approve screenshot layout or physical page placement"},
        "limitations": [
            "These are isolated demo fixture results.",
            "No live account, firmware emulator, or physical Kindle result is established.",
            "The current production snapshot may omit effective spread/fit, physical slot indices, "
            "and second-frame dimensions; missing fields are not fabricated.",
        ],
    }
    pointer = None
    try:
        pointer = Pointer()
        selected = plans(arguments)
        require(selected, "No targeted Reader cases match the selected profiles")
        for profile, name, landscape, actions, expected, drive in selected:
            case = Case(arguments, pointer, profile, name, landscape, actions, expected)
            try:
                case.start()
                drive(case)
                case.finish()
            except Exception as error:
                case.result.update({"passed": False, "runtime_passed": False,
                                    "error": str(error), "traceback": traceback.format_exc()})
                if case.window is not None and case.process is not None and case.process.poll() is None:
                    try:
                        case.snapshot("failure")
                    except Exception:
                        pass
            finally:
                case.close()
            manifest["cases"].append(case.result)
            write_json(arguments.output / "reader-v2-runtime-result.json", manifest)
            print(json.dumps({"profile": profile, "case": name, "runtime_passed": case.result["runtime_passed"],
                              "error": case.result.get("error"), "directory": str(case.directory)}), flush=True)
        manifest["runtime_passed"] = bool(manifest["cases"]) and all(
            item["runtime_passed"] for item in manifest["cases"]
        )
        manifest["passed"] = manifest["runtime_passed"]
        manifest["visual_review"]["artifacts"] = [
            review for item in manifest["cases"] for review in item["visual_review_required"]
        ]
    except Exception as error:
        manifest["error"] = str(error)
        manifest["traceback"] = traceback.format_exc()
    finally:
        if pointer is not None:
            pointer.close()
        manifest["finished_at"] = datetime.datetime.now(datetime.timezone.utc).isoformat()
        write_json(arguments.output / "reader-v2-runtime-result.json", manifest)
    return 0 if manifest["runtime_passed"] else 1


if __name__ == "__main__":
    from harness_signals import install_interruption_handlers
    install_interruption_handlers()
    sys.exit(main())
