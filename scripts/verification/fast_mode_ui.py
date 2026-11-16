#!/usr/bin/env python3
"""Capture Normal/Fast settings with real XTest input on isolated test-env.

All preferences are changed through the real application controls. Closed demo
data is copied only to verify an ordinary cold restart. Actual X11 captures are
never thresholded or retouched. Pixel and idle checks do not prove DU, GC16,
panel updates, or physical-device compatibility.
"""

import argparse
import datetime
import hashlib
import json
import math
import os
from pathlib import Path
import re
import shutil
import sqlite3
import sys
import time
import traceback

from capture_uiux_baseline import SCALE, Session, write_json
from harness_signals import install_interruption_handlers
from native_media_settings import verify_source
from pointer_v2 import INPUT_DELIVERY_PROTOCOL, Pointer
from ui_refinement import PROFILES


ACCOUNT = "design-demo"
ANCHOR_KEYS = (
    "comic_id", "episode_id", "revision", "page_id", "page_index", "crop_index",
    "source_x", "source_y", "zoom_ratio", "rotation", "core_mode", "fit", "direction",
    "landscape_layout", "cover_alone",
)
DATABASE_ANCHOR_KEYS = (
    "episode_id", "revision", "page_id", "page_index", "source_x", "source_y",
    "zoom_ratio", "rotation", "mode",
)
SAVED_KEYS = ("every", "dither", "dither_method")
READER_KEYS = ANCHOR_KEYS + (
    "refresh_mode", "fast_mode", "presented_fast_mode", "target_fast_mode", "reader_refresh_scene",
    "reader_generation", "render_version",
    "rendering", "menu", "sheet", "dither", "dither_method", "effective_spread",
    "viewport_width", "viewport_height", "frame_width", "frame_height",
    "second_frame_width", "second_frame_height",
)


def require(value, message):
    if not value:
        raise AssertionError(message)


def sha256(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def rgb(path):
    from PIL import Image

    with Image.open(path) as source:
        return source.convert("RGB")


def rectangle_hash(pixels, box):
    return hashlib.sha256(pixels.crop(box).tobytes()).hexdigest()


def database_snapshot(case, comic=None):
    database = (case.directory / "data/demo/state.sqlite3").resolve(strict=True)
    with sqlite3.connect(database.as_uri() + "?mode=ro", uri=True, timeout=5) as connection:
        rows = connection.execute(
            "SELECT key,payload FROM preferences WHERE account_key=?", (ACCOUNT,)
        ).fetchall()
        anchors = connection.execute(
            "SELECT comic_id,payload FROM reading_anchors WHERE account_key=?", (ACCOUNT,)
        ).fetchall()
    selected = {"preferences"}
    if comic is not None:
        selected.add("reader_preferences_" + comic)
    preferences = {key: json.loads(payload) for key, payload in rows if key in selected}
    require(set(preferences) == selected, "The real controls have not persisted the selected preferences")
    saved = {}
    modes = {}
    for key, payload in preferences.items():
        require(all(name in payload for name in SAVED_KEYS + ("refresh_mode",)),
                "A durable preference omits refresh_mode, every, or dithering: " + key)
        saved[key] = {name: payload[name] for name in SAVED_KEYS}
        modes[key] = payload["refresh_mode"]
    positions = {}
    for key, payload in anchors:
        anchor = json.loads(payload)
        require(all(name in anchor for name in DATABASE_ANCHOR_KEYS), "A saved anchor omits a position field")
        positions[key] = {name: anchor[name] for name in DATABASE_ANCHOR_KEYS}
    require(positions, "The isolated demonstration has no source anchor")
    return {"saved": saved, "modes": modes, "anchors": positions}


def assert_durable(case, reference, mode, comic=None):
    current = database_snapshot(case, comic)
    require(current["saved"] == reference["saved"], "A refresh-mode operation changed saved every or dithering")
    require(current["anchors"] == reference["anchors"], "A refresh-mode operation changed the durable reading anchor")
    require(all(value == mode for value in current["modes"].values()), "The selected refresh mode was not retained")
    return current


def assert_ui(case, mode):
    record = case.latest("ui-state")
    require(record and record.get("demo") is True and record.get("view_model_fixture_only") is False,
            "A same-process real demonstration UI observation is required")
    state = record.get("state", {})
    model = state.get(case.route, {})
    require("refresh_mode" in model and "refresh_mode_note" in model,
            "The UI trace omits refresh_mode or refresh_mode_note for " + case.route)
    require(model["refresh_mode"] == mode and bool(model["refresh_mode_note"]),
            "The visible model has a different refresh mode or lacks its explanation")
    if case.route == "reader":
        require("presented_fast_mode" in model and type(model["presented_fast_mode"]) is bool,
                "The real UI model omits its typed actual presented-frame mode")
    return record


def reader_state(case, mode):
    record = case.reader_ready()
    state = record["reader_state"]
    missing = [key for key in READER_KEYS if key not in state]
    require(not missing, "The detailed reader-ready contract omits: " + ", ".join(missing))
    expected = {"refresh_mode": mode, "fast_mode": mode == "fast", "presented_fast_mode": mode == "fast",
                "target_fast_mode": mode == "fast"}
    record = case.reader_ready(expected)
    state = record["reader_state"]
    require(all(type(state[key]) is bool for key in ("fast_mode", "presented_fast_mode", "target_fast_mode")),
            "Reader Fast flags must be typed booleans")
    require(state["reader_refresh_scene"] in ("normal", "fast_content", "fast_overlay"),
            "The detailed Reader observation has an unknown refresh scene")
    ui = assert_ui(case, mode)
    require(ui["state"]["reader"]["presented_fast_mode"] is (mode == "fast"),
            "The actual UI image sampling mode differs from the presented Reader frame")
    return state


def anchor(state):
    return {key: state[key] for key in ANCHOR_KEYS}


def assert_reader_anchor(case, reference, mode):
    state = reader_state(case, mode)
    require(anchor(state) == reference, "A refresh-mode operation moved the live source anchor")
    return state


class FastSession(Session):
    def __init__(self, arguments, pointer, profile, name, route, landscape=False):
        super().__init__(arguments, arguments.output, pointer, profile, name, route,
                         landscape=landscape, offline=True)
        self.evidence = self.directory / "evidence"
        self.evidence.mkdir()
        self.sequence = 0
        self.report.update({"passed": False, "input_scope": "Actual XTest input only",
                            "named_ui_actions_used": False, "evidence": [], "scrolls": []})

    def capture_evidence(self, name):
        self.sequence += 1
        path = self.evidence / (str(self.sequence).zfill(3) + "-" + name + ".png")
        require(not path.exists(), "Refusing to overwrite an existing evidence capture")
        capture = self.pointer.capture(self.window, path)
        metadata = {**capture, "id": name, "profile": self.profile, "session": self.name,
                    "file": str(path.relative_to(self.output)), "process_id": self.process.pid,
                    "width": self.window["width"], "height": self.window["height"],
                    "logical_size": self.logical_size, "scale_factor": SCALE,
                    "binary_sha256": self.arguments.expected_sha256,
                    "captured_source": self.arguments.source_identity,
                    "captured_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
                    "pixel_processing": "None; untouched X11 RGB pixels", "baseline_capture": False,
                    "demo": True, "physical_device_used": False, "live_account_used": False}
        write_json(path.with_suffix(".json"), metadata)
        self.report["evidence"].append(metadata)
        return path

    def baseline(self, name, mode):
        assert_ui(self, mode)
        expected = {self.route + ".refresh_mode": mode}
        reader = ({"refresh_mode": mode, "fast_mode": mode == "fast", "target_fast_mode": mode == "fast"}
                  if self.route == "reader" else None)
        self.snapshot(name, expected=expected, reader=reader)
        path = self.raw / (name + ".png")
        metadata = json.loads(path.with_suffix(".json").read_text(encoding="utf-8"))
        metadata.update({"captured_source": self.arguments.source_identity,
                         "baseline_id": self.arguments.baseline_id, "refresh_mode": mode,
                         "visual_review_status": "pending", "hardware_waveform_proof": False})
        write_json(path.with_suffix(".json"), metadata)
        self.report["screenshots"][-1] = metadata
        return path


def bands(gray, left, right, top, bottom, limit=225):
    found = []
    for y in range(max(0, top), min(gray.height, bottom)):
        low, high = gray.crop((left, y, right, y + 1)).getextrema()
        if high < limit and high - low <= 3:
            if found and found[-1][-1] + 1 == y:
                found[-1].append(y)
            else:
                found.append([y])
    return found


def choice_boxes(path, left, right, height, top, bottom):
    pixels = rgb(path).convert("L")
    left, right, height = round(left * SCALE), round(right * SCALE), round(height * SCALE)
    thin = [band for band in bands(pixels, left + 2 * SCALE, right - 2 * SCALE,
                                  round(top * SCALE), round(bottom * SCALE)) if len(band) <= 4 * SCALE]
    result = []
    for first in thin:
        for last in thin:
            actual = last[-1] + 1 - first[0]
            if last[0] <= first[-1] or abs(actual - height) > 2 * SCALE:
                continue
            sample_y = (first[-1] + 4 * SCALE, (first[0] + last[-1]) // 2, last[0] - 4 * SCALE)
            if not all(pixels.getpixel((left + 1, y)) < 240
                       and pixels.getpixel((right - 2, y)) < 240 for y in sample_y):
                continue
            # This distinguishes a real left border from a crop through a
            # selected black option in a larger, full-width control.
            if left > 0 and not all(pixels.getpixel((left - 1, y)) > 240 for y in sample_y):
                continue
            box = {"left": left, "right": right, "top": first[0], "bottom": last[-1] + 1,
                   "border_luma": pixels.getpixel(((left + right) // 2, first[0]))}
            if not any(abs(box["top"] - old["top"]) <= 2 * SCALE for old in result):
                result.append(box)
    return sorted(result, key=lambda value: value["top"])


def option_point(box, index, count):
    return ((box["left"] + (box["right"] - box["left"]) * (index + 0.5) / count) / SCALE,
            (box["top"] + box["bottom"]) / (2 * SCALE))


def assert_selection(path, box, index, count):
    pixels = rgb(path).convert("L")
    y = box["top"] + 7 * SCALE
    observed = [pixels.getpixel((round((box["left"] + (box["right"] - box["left"]) * (item + 0.5) / count)), y))
                for item in range(count)]
    require(observed[index] < 30 and all(value > 240 for item, value in enumerate(observed) if item != index),
            "The actual selector pixels do not show the requested enabled choice")


def four_options(path, box):
    pixels = rgb(path).convert("L")
    y = box["top"] + 7 * SCALE
    for index in (1, 2, 3):
        x = round(box["left"] + 2 * SCALE + index * (box["right"] - box["left"] - 4 * SCALE) / 4) + SCALE
        middle = pixels.getpixel((x, y))
        neighbors = [pixels.getpixel((x + offset * SCALE, y)) for offset in (-4, 4)]
        if middle >= 225 or max(neighbors) <= 240:
            return False
    return True


def reader_controls(case, path):
    width, height = case.logical_size
    edge = 48 if case.profile == "scribe" else 24
    label = 96 if case.profile == "scribe" else 76
    gray = rgb(path).convert("L")
    tops = [band for band in bands(gray, SCALE, gray.width - SCALE, 70 * SCALE,
                                  gray.height - 4 * SCALE, 30) if len(band) <= 4 * SCALE]
    require(tops, "Actual pixels do not identify the Reader settings sheet top")
    sheet_top = tops[-1][0] / SCALE
    column = edge + ((width - 2 * edge + 32) / 2 if case.landscape else 0)
    rows = choice_boxes(path, column + label + 12, width - edge,
                        60 if case.profile == "scribe" else 52, sheet_top + 40, height)
    require(len(rows) == (3 if case.landscape else 6),
            "The visible Reader settings do not match the seven-row portrait or two-column landscape contract")
    return {"sheet_top": sheet_top, "choices": rows,
            "mode": rows[0 if case.landscape else 3], "every": rows[1 if case.landscape else 4],
            "source": "Actual X11 rectangular choice borders"}


def open_reader_settings(case, mode, reference):
    assert_reader_anchor(case, reference, mode)
    width, height = case.logical_size
    edge = 48 if case.profile == "scribe" else 24
    case.click("Open Reader controls", (width / 2, height / 2),
               {"reader.menu": True, "reader.sheet": ""})
    case.click("Open Reader settings", (edge + (width - 2 * edge) * 3 / 8,
                                        height - (42 if case.profile == "scribe" else 38)),
               {"reader.sheet": "display"}, reader={"sheet": "display"})
    state = assert_reader_anchor(case, reference, mode)
    require(state["reader_refresh_scene"] == ("fast_overlay" if mode == "fast" else "normal"),
            "Open Reader settings do not use the expected grayscale overlay scene")
    path = case.capture_evidence("reader-settings-controls")
    return path, reader_controls(case, path)


def close_reader_settings(case, mode, reference):
    path = case.capture_evidence("reader-settings-before-close")
    controls = reader_controls(case, path)
    width, _ = case.logical_size
    edge = 48 if case.profile == "scribe" else 24
    case.click("Close Reader settings with its visible Done control",
               (width - edge - (34 if case.profile == "scribe" else 31),
                controls["sheet_top"] + 2 + (32 if case.profile == "scribe" else 28)),
               {"reader.menu": False, "reader.sheet": ""}, reader={"menu": False, "sheet": ""})
    return assert_reader_anchor(case, reference, mode)


def quiet_reader_state(case, mode):
    state = reader_state(case, mode)
    require(state["menu"] is False and state["sheet"] == "" and state["rendering"] is False,
            "A quiet full-page capture requires closed controls and a completed frame")
    require(state["reader_refresh_scene"] == ("fast_content" if mode == "fast" else "normal"),
            "The quiet Reader is not in its actual content refresh scene")
    require(assert_ui(case, mode)["state"]["reader"]["presented_fast_mode"] is (mode == "fast"),
            "The quiet UI image uses a different presented-frame mode")
    width, height = case.window["width"], case.window["height"]
    slots = 2 if state["effective_spread"] else 1
    require(state["frame_width"] * slots == width and state["frame_height"] == height,
            "The loaded primary frame does not cover its complete logical viewport")
    if slots == 2:
        require(state["second_frame_width"] == state["frame_width"] and state["second_frame_height"] == height,
                "The second spread slot does not cover its complete logical viewport")
    return state


def quiet_frame(case, mode, name, *, settle=False, expected_sha256=None):
    started = time.monotonic()
    deadline = started + 30
    previous = None
    identical = 0
    samples = []
    while True:
        state = quiet_reader_state(case, mode)
        path = case.capture_evidence(name)
        digest = sha256(path)
        colors = rgb(path).getcolors(maxcolors=3) if mode == "fast" else None
        binary = (colors is not None
                  and {color for _, color in colors} == {(0, 0, 0), (255, 255, 255)})
        pixels_match = (mode != "fast" or binary) and (expected_sha256 is None or digest == expected_sha256)
        if not settle:
            require(mode != "fast" or binary,
                    "The untouched quiet Fast capture is not a nonempty binary black/white page")
            require(expected_sha256 is None or digest == expected_sha256,
                    "The untouched quiet page differs from its original pixel identity")
            return path, state
        identical = identical + 1 if pixels_match and digest == previous else 1 if pixels_match else 0
        previous = digest if pixels_match else None
        elapsed = time.monotonic() - started
        samples.append({"file": str(path.relative_to(case.output)), "sha256": digest,
                        "elapsed_seconds": round(elapsed, 3), "pixel_condition_passed": pixels_match,
                        "render_version": state["render_version"]})
        if elapsed >= 1.5 and identical >= 3:
            case.report.setdefault("quiet_paint_boundaries", []).append({
                "name": name, "minimum_settle_seconds": 1.5, "identical_samples": identical,
                "samples": samples, "refresh_mode": mode, "binary_required": mode == "fast",
                "expected_sha256": expected_sha256,
                "actual_presented_scene_checked": True, "pixels_retouched": False,
            })
            return path, state
        require(time.monotonic() < deadline,
                "The actual quiet page did not reach its required stable pixel identity within 30 seconds")
        time.sleep(0.35)


def idle_reader(case, reference, durable, first):
    before = reader_state(case, "fast")
    version = before["render_version"]
    generation = before["reader_generation"]
    started = time.monotonic()
    initial = len(case.records())
    expected_hash = sha256(first)
    samples = []
    while True:
        path, state = quiet_frame(case, "fast", "fast-idle")
        require(anchor(state) == reference and state["reader_generation"] == generation
                and state["render_version"] == version, "An idle Fast page changed its anchor or render version")
        require(sha256(path) == expected_hash, "An idle Fast page changed the untouched X11 pixels")
        assert_durable(case, durable, "fast", before["comic_id"])
        samples.append({"file": str(path.relative_to(case.output)), "sha256": expected_hash,
                        "elapsed_seconds": round(time.monotonic() - started, 3), "render_version": version})
        if time.monotonic() - started >= case.arguments.idle_seconds and len(samples) >= 3:
            break
        time.sleep(0.35)
    for row in case.records()[initial:]:
        if row.get("process_id") == case.process.pid and row.get("event") == "reader-ready":
            require(row.get("reader_state", {}).get("render_version") == version,
                    "An intervening idle Reader observation changed render version")
    return {"samples": samples, "minimum_seconds": case.arguments.idle_seconds,
            "pixels_and_render_version_unchanged": True, "hardware_waveform_proof": False,
            "scope": "Desktop image and state stability; no DU/GC16 or panel-update conclusion"}


def disabled_every(case, path, box, durable, mode, reference=None):
    gray = rgb(path).convert("L")
    require(100 <= box["border_luma"] < 225, "The periodic-refresh control is not visibly gray")
    require(gray.crop((box["left"] + 3 * SCALE, box["top"] + 3 * SCALE,
                       box["right"] - 3 * SCALE, box["bottom"] - 3 * SCALE)).getextrema()[0] >= 100,
            "The disabled periodic-refresh options retain an enabled black treatment")
    before = reader_state(case, mode) if case.route == "reader" else None
    comic = before["comic_id"] if before else None
    assert_durable(case, durable, mode, comic)
    initial = len(case.records())
    case.click("Attempt the disabled every-page option", option_point(box, 1, 4))
    # Silence is the correct response to a disabled control. Never wait for a
    # new UI event; compare real pixels, current observations and SQLite.
    time.sleep(0.65)
    require(case.process.poll() is None, "The application exited after the disabled periodic-refresh click")
    after_path = case.capture_evidence("disabled-every-after")
    top = (32 if case.profile == "scribe" else 28) * SCALE
    from PIL import ImageChops
    old, new = rgb(path), rgb(after_path)
    body = (0, top, old.width, old.height)
    require(ImageChops.difference(old.crop(body), new.crop(body)).getbbox() is None,
            "The disabled periodic-refresh click changed visible body pixels")
    assert_durable(case, durable, mode, comic)
    assert_ui(case, mode)
    if before is not None:
        after = assert_reader_anchor(case, reference, mode)
        require(after["render_version"] == before["render_version"],
                "The disabled periodic-refresh click changed the Reader render version")
    return {"box": box, "before": str(path.relative_to(case.output)),
            "after": str(after_path.relative_to(case.output)), "quiet_seconds": 0.65,
            "new_ui_event_required": False, "new_trace_records": len(case.records()) - initial,
            "body_pixels_unchanged": True, "status_clock_excluded": True,
            "durable_preferences_and_anchor_unchanged": True}


def account_body(case):
    width, height = case.logical_size
    status = 32 if case.profile == "scribe" else 28
    root_top = 193 if case.profile == "scribe" else 72
    nav = 80 if case.profile == "scribe" else 64
    bottom_gap = 72 if case.profile == "scribe" else 12
    return (0, (status + root_top) * SCALE, width * SCALE, (height - nav - bottom_gap) * SCALE)


def fixed_account_pixels(case, path):
    pixels = rgb(path)
    body = account_body(case)
    status = (32 if case.profile == "scribe" else 28) * SCALE
    nav = (80 if case.profile == "scribe" else 64) * SCALE
    return {"header": rectangle_hash(pixels, (0, status, pixels.width, body[1])),
            "navigation": rectangle_hash(pixels, (0, pixels.height - nav, pixels.width, pixels.height))}


def stable_account_frame(case, name, require_selector=False):
    # A model observation can precede its X11 presentation. Require actual
    # Reading controls on entry, then three unchanged raw frames before using
    # either side of a wheel operation as fixed-region evidence.
    previous = None
    identical = 0
    samples = []
    started = time.monotonic()
    for _ in range(24):
        require(case.process.poll() is None, "The application exited while settling account pixels")
        record = case.latest("ui-state")
        state = record.get("state", {}) if record else {}
        require(state.get("route") == "account" and state.get("account", {}).get("section") == "reading",
                "Account pixel stability requires the Reading route and section")
        mode = state["account"].get("refresh_mode")
        require(mode in ("normal", "fast"), "The account pixel observation omits its refresh mode")
        path = case.capture_evidence(name + "-settle")
        if require_selector:
            try:
                assert_selection(path, account_mode_box(case, path), 1 if mode == "fast" else 0, 2)
            except AssertionError:
                previous = None
                identical = 0
                time.sleep(0.2)
                continue
        pixels = rgb(path)
        status = (32 if case.profile == "scribe" else 28) * SCALE
        digest = rectangle_hash(pixels, (0, status, pixels.width, pixels.height))
        identical = identical + 1 if digest == previous else 1
        previous = digest
        samples.append({"file": str(path.relative_to(case.output)), "body_rgb_sha256": digest,
                        "elapsed_seconds": round(time.monotonic() - started, 3)})
        if identical >= 3:
            proof = {"name": name, "identical_samples": identical, "sampling_interval_seconds": 0.2,
                     "actual_reading_selector_required": require_selector,
                     "comparison_box": [0, status, pixels.width, pixels.height],
                     "status_clock_excluded": True, "samples": samples}
            case.report.setdefault("frame_stability", []).append(proof)
            return path, proof
        time.sleep(0.2)
    raise AssertionError("The actual Reading pixels did not stabilize before " + name)


def wheel(case, direction, name):
    body = account_body(case)
    x, y = round(case.window["width"] / 2), round((body[1] + body[3]) / 2)
    before, before_stability = stable_account_frame(case, "before-wheel")
    pointer = case.pointer
    pointer.xtst.XTestFakeMotionEvent(pointer.display, pointer.x11.XDefaultScreen(pointer.display),
                                     case.window["x"] + x, case.window["y"] + y, 0)
    pointer.x11.XFlush(pointer.display)
    pointer.x11.XSync(pointer.display, 0)
    time.sleep(0.08)
    button = 5 if direction == "down" else 4
    for _ in range(3):
        pointer.xtst.XTestFakeButtonEvent(pointer.display, button, 1, 0)
        pointer.xtst.XTestFakeButtonEvent(pointer.display, button, 0, 0)
        pointer.x11.XSync(pointer.display, 0)
        time.sleep(0.03)
    after, after_stability = stable_account_frame(case, "after-wheel")
    require(fixed_account_pixels(case, before) == fixed_account_pixels(case, after),
            "Account scrolling moved the fixed header or bottom navigation")
    changed = rectangle_hash(rgb(before), body) != rectangle_hash(rgb(after), body)
    case.report["scrolls"].append({"name": name, "method": "Actual XTest wheel buttons", "direction": direction,
                                   "ticks": 3, "physical_point": [x, y], "body_pixels_changed": changed,
                                   "header_and_navigation_unchanged": True,
                                   "before_stability": before_stability, "after_stability": after_stability,
                                   "before": str(before.relative_to(case.output)),
                                   "after": str(after.relative_to(case.output))})
    return after, changed


def account_top(case):
    for _ in range(12):
        path, changed = wheel(case, "up", "Restore the account reading scroll position")
        if not changed:
            return path
    raise AssertionError("The account reading area did not reach its bounded scroll top")


def account_mode_box(case, path):
    width, _ = case.logical_size
    edge = 48 if case.profile == "scribe" else 24
    body = account_body(case)
    boxes = choice_boxes(path, edge + (142 if case.profile == "scribe" else 108) + 12,
                         width - edge, 60 if case.profile == "scribe" else 52,
                         body[1] / SCALE, body[3] / SCALE)
    require(len(boxes) == 1, "Actual pixels do not identify exactly one account refresh-mode selector")
    return boxes[0]


def account_seek(case, kind):
    width, _ = case.logical_size
    edge = 48 if case.profile == "scribe" else 24
    body = account_body(case)
    for attempt in range(13):
        path = case.capture_evidence("locate-account-" + kind)
        if kind == "every":
            candidates = choice_boxes(path, edge, width - edge, 64 if case.profile == "scribe" else 52,
                                      body[1] / SCALE, body[3] / SCALE)
            candidates = [box for box in candidates if four_options(path, box)]
        else:
            candidates = choice_boxes(path, width - edge - 60, width - edge, 32,
                                      body[1] / SCALE, body[3] / SCALE)
        require(len(candidates) <= 1, "The visible account control has ambiguous rectangular pixel evidence")
        if candidates:
            return path, candidates[0]
        require(attempt < 12, "The account " + kind + " control is not reachable inside the bounded reading area")
        _, changed = wheel(case, "down", "Reach the account " + kind + " control")
        require(changed, "Account scrolling stopped before its " + kind + " control became fully visible")
    raise AssertionError("Unreachable account control")


def open_account_reading(case, mode):
    width, _ = case.logical_size
    point = ((48 + 1.5 * (width - 96) / 6, 354) if case.profile == "scribe" else (width / 2, 202))
    case.click("Open account default reading settings", point, {"account.section": "reading"})
    assert_ui(case, mode)
    stable_account_frame(case, "reading-route-entry", require_selector=True)


def wait_every_six(case, comic=None):
    def saved():
        try:
            current = database_snapshot(case, comic)
        except AssertionError:
            return None
        return current if all(value["every"] == "6" for value in current["saved"].values()) else None
    return case.wait("the actual every-six selection in SQLite", saved)


def close_case(case, error=None):
    if error is not None:
        case.report.update({"error": str(error), "traceback": traceback.format_exc()})
    case.close()


def reader_case(arguments, pointer, profile, landscape):
    direction = "landscape" if landscape else "portrait"
    case = FastSession(arguments, pointer, profile, "reader-" + direction, "reader", landscape)
    try:
        case.start()
        state = reader_state(case, "normal")
        reference = anchor(state)
        path, controls = open_reader_settings(case, "normal", reference)
        case.click("Select Normal using the actual Reader selector", option_point(controls["mode"], 0, 2))
        case.click("Store every-six while Normal is enabled", option_point(controls["every"], 2, 4))
        durable = wait_every_six(case, state["comic_id"])
        close_reader_settings(case, "normal", reference)
        normal_path, _ = quiet_frame(case, "normal", "normal-quiet-reference")
        time.sleep(0.6)
        normal_path, _ = quiet_frame(case, "normal", "normal-quiet-reference-settled", settle=True)
        path, controls = open_reader_settings(case, "normal", reference)
        assert_selection(path, controls["mode"], 0, 2)
        assert_selection(path, controls["every"], 2, 4)
        prefix = "27" if landscape else "20"
        case.baseline(prefix + "-reader-settings-normal-" + direction, "normal")
        case.click("Select Fast using the actual Reader selector", option_point(controls["mode"], 1, 2),
                   {"reader.refresh_mode": "fast"}, reader={"refresh_mode": "fast", "fast_mode": True,
                                                            "target_fast_mode": True})
        assert_reader_anchor(case, reference, "fast")
        assert_durable(case, durable, "fast", state["comic_id"])
        path = case.baseline(prefix + "-reader-settings-fast-" + direction, "fast")
        controls = reader_controls(case, path)
        assert_selection(path, controls["mode"], 1, 2)
        case.report["disabled_every"] = disabled_every(case, path, controls["every"], durable, "fast", reference)
        close_reader_settings(case, "fast", reference)
        quiet_name = ("25" if landscape else "14") + "-reader-fast-" + direction
        path = case.baseline(quiet_name, "fast")
        quiet_frame(case, "fast", "fast-quiet-binary-proof")
        case.report["idle"] = idle_reader(case, reference, durable, path)
        case.report.update({"reader_anchor": reference, "durable_reference": durable,
                            "normal_quiet_sha256": sha256(normal_path), "quiet_fast_binary": True})
        case.finish()
        assert_durable(case, durable, "fast", state["comic_id"])
        case.report["passed"] = True
    except Exception as error:
        close_case(case, error)
    else:
        close_case(case)
    return case


def account_case(arguments, pointer, profile, landscape):
    direction = "landscape" if landscape else "portrait"
    case = FastSession(arguments, pointer, profile, "account-" + direction, "account", landscape)
    try:
        case.start()
        open_account_reading(case, "normal")
        path = account_top(case)
        box = account_mode_box(case, path)
        case.click("Select Normal using the actual account selector", option_point(box, 0, 2))
        path, every = account_seek(case, "every")
        case.click("Store every-six in account defaults", option_point(every, 2, 4))
        durable = wait_every_six(case)
        path = account_top(case)
        box = account_mode_box(case, path)
        assert_selection(path, box, 0, 2)
        case.baseline("10-account-reading-normal-" + direction, "normal")
        case.click("Select Fast using the actual account selector", option_point(box, 1, 2),
                   {"account.refresh_mode": "fast"})
        assert_durable(case, durable, "fast")
        path = case.baseline("10-account-reading-fast-" + direction, "fast")
        assert_selection(path, account_mode_box(case, path), 1, 2)
        path, every = account_seek(case, "every")
        case.report["disabled_every"] = disabled_every(case, path, every, durable, "fast")
        path, jpeg = account_seek(case, "jpeg")
        case.report["jpeg_reachability"] = {"file": str(path.relative_to(case.output)), "box": jpeg,
                                              "source": "Actual XTest scrolling and X11 switch pixels"}
        if landscape:
            require(any(row["body_pixels_changed"] for row in case.report["scrolls"]),
                    "The landscape account case did not exercise actual reading-area scrolling")
        case.report["durable_reference"] = durable
        case.finish()
        assert_durable(case, durable, "fast")
        case.report["passed"] = True
    except Exception as error:
        close_case(case, error)
    else:
        close_case(case)
    return case


def restart_case(arguments, pointer, source):
    case = FastSession(arguments, pointer, source.profile, source.name + "-cold-restart", source.route,
                       source.landscape)
    try:
        require(source.process.poll() is not None and source.report.get("complete"),
                "Cold restart requires a gracefully closed source application")
        data = source.directory / "data"
        require(not list(data.rglob("session.json")), "The private demo data unexpectedly contains a session file")
        shutil.copytree(data, case.directory / "data")
        case.report["restart_source"] = str(source.directory.relative_to(arguments.output))
        case.report["database_modified_by_harness"] = False
        case.start()
        durable = source.report["durable_reference"]
        if case.route == "reader":
            reference = source.report["reader_anchor"]
            state = assert_reader_anchor(case, reference, "fast")
            assert_durable(case, durable, "fast", state["comic_id"])
            path, _ = quiet_frame(case, "fast", "cold-restart-fast-page", settle=True)
            case.report["idle"] = idle_reader(case, reference, durable, path)
            _, controls = open_reader_settings(case, "fast", reference)
            case.click("Restore Normal through the actual Reader selector", option_point(controls["mode"], 0, 2),
                       {"reader.refresh_mode": "normal"}, reader={"refresh_mode": "normal", "fast_mode": False,
                                                                  "target_fast_mode": False})
            assert_durable(case, durable, "normal", state["comic_id"])
            close_reader_settings(case, "normal", reference)
            time.sleep(0.6)
            path, _ = quiet_frame(case, "normal", "normal-page-restored", settle=True,
                                  expected_sha256=source.report["normal_quiet_sha256"])
            require(sha256(path) == source.report["normal_quiet_sha256"],
                    "Restoring Normal after cold restart did not restore the original actual page pixels")
            comic = state["comic_id"]
        else:
            open_account_reading(case, "fast")
            assert_durable(case, durable, "fast")
            path = account_top(case)
            box = account_mode_box(case, path)
            assert_selection(path, box, 1, 2)
            case.click("Restore Normal through the actual account selector", option_point(box, 0, 2),
                       {"account.refresh_mode": "normal"})
            assert_durable(case, durable, "normal")
            path, every = account_seek(case, "every")
            require(every["border_luma"] < 30, "Normal did not restore the enabled periodic-refresh border")
            assert_selection(path, every, 2, 4)
            comic = None
        case.finish()
        assert_durable(case, durable, "normal", comic)
        case.report.update({"passed": True, "cold_restart_retained_fast": True,
                            "normal_restored_by_xtest": True, "every_dither_and_anchor_preserved": True})
    except Exception as error:
        close_case(case, error)
    else:
        close_case(case)
    return case.report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--expected-sha256", "--expected-binary-sha256", dest="expected_sha256", required=True)
    parser.add_argument("--source-commit", required=True)
    parser.add_argument("--source-root", type=Path, default=Path(__file__).resolve().parents[2])
    parser.add_argument("--source-manifest", type=Path, required=True)
    parser.add_argument("--source-fingerprint", required=True)
    parser.add_argument("--baseline-id", required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--profiles", nargs="+", choices=list(PROFILES), default=["pw6", "scribe"])
    parser.add_argument("--idle-seconds", type=float, default=2.4)
    arguments = parser.parse_args()
    require(sys.platform.startswith("linux") and os.environ.get("BILICOMICS_REMOTE_VERIFICATION") == "1",
            "Run only on designated Linux test-env with BILICOMICS_REMOTE_VERIFICATION=1")
    require(os.environ.get("DISPLAY"), "Run inside a fresh isolated Xvfb display")
    require(re.fullmatch(r"[0-9a-f]{40}", arguments.source_commit), "Supply the full lowercase base commit")
    require(all(re.fullmatch(r"[0-9a-f]{64}", value) for value in
                (arguments.expected_sha256, arguments.source_fingerprint)), "Supply exact lowercase SHA-256 identities")
    require(re.fullmatch(r"[a-z0-9][a-z0-9-]{0,95}", arguments.baseline_id), "Supply a bounded new baseline identity")
    require(math.isfinite(arguments.idle_seconds) and 1.2 <= arguments.idle_seconds <= 15,
            "Use a bounded idle observation between 1.2 and 15 seconds")
    arguments.binary = arguments.binary.resolve(strict=True)
    arguments.source_root = arguments.source_root.resolve(strict=True)
    arguments.source_manifest = arguments.source_manifest.resolve(strict=True)
    require(arguments.binary.is_file() and os.access(arguments.binary, os.X_OK)
            and sha256(arguments.binary) == arguments.expected_sha256, "The selected desktop binary differs")
    require(arguments.output.is_absolute(), "Supply a fresh absolute evidence directory")
    arguments.output = arguments.output.resolve()
    os.umask(0o077)
    arguments.output.mkdir(parents=True, mode=0o700, exist_ok=False)
    arguments.source_identity = verify_source(arguments)
    shutil.copyfile(arguments.source_manifest, arguments.output / "source-inputs.sha256")
    install_interruption_handlers()
    profiles = list(dict.fromkeys(arguments.profiles))
    report = {"suite": "fast_mode_desktop_ui", "baseline_id": arguments.baseline_id, "passed": False,
              "started_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
              "source": arguments.source_identity, "binary_sha256": arguments.expected_sha256,
              "harness_sha256": sha256(Path(__file__)),
              "harness_dependencies": {name: sha256(Path(__file__).with_name(name)) for name in
                                       ("capture_uiux_baseline.py", "pointer_v2.py", "ui_refinement.py",
                                        "harness_signals.py", "native_media_settings.py")},
              "capture_host": "test-env", "scale_factor": SCALE, "input_protocol": INPUT_DELIVERY_PROTOCOL,
              "visual_review_status": "pending", "expected_capture_count": 10 * len(profiles),
              "scope": "Real XTest settings, persistence, complete quiet binary pages and desktop idle stability",
              "physical_device_used": False, "native_driver_exercised": False, "live_account_used": False,
              "hardware_waveform_proof": False, "hardware_compatibility_claimed": False,
              "idle_limit": "Stable screenshots cannot prove actual DU/GC16 requests, absence of panel refreshes or physical acceptance",
              "sessions": [], "restart_cases": [], "errors": []}
    pointer = None
    try:
        pointer = Pointer()
        for profile in profiles:
            sources = []
            for landscape in (False, True):
                sources.append(reader_case(arguments, pointer, profile, landscape))
                sources.append(account_case(arguments, pointer, profile, landscape))
            report["sessions"].extend(case.report for case in sources)
            for source in sources:
                if source.report.get("passed"):
                    report["restart_cases"].append(restart_case(arguments, pointer, source))
                else:
                    report["errors"].append({"profile": profile, "session": source.name,
                                             "error": "The source case failed; its cold restart was skipped"})
            write_json(arguments.output / "fast-mode-ui-result.json", report)
        require(sha256(arguments.binary) == arguments.expected_sha256, "The selected binary changed during verification")
        verify_source(arguments)
        selected = report["sessions"] + report["restart_cases"]
        capture_count = sum(len(case.get("screenshots", [])) for case in report["sessions"])
        report["passed"] = (not report["errors"] and len(selected) == 8 * len(profiles)
                            and capture_count == report["expected_capture_count"]
                            and all(case.get("passed") for case in selected))
    except BaseException as error:
        report["errors"].append({"error": str(error), "traceback": traceback.format_exc()})
    finally:
        if pointer is not None:
            pointer.close()
        report["finished_at"] = datetime.datetime.now(datetime.timezone.utc).isoformat()
        report["capture_count"] = sum(len(case.get("screenshots", [])) for case in report["sessions"])
        write_json(arguments.output / "fast-mode-ui-result.json", report)
    print(json.dumps({"passed": report["passed"], "captures": report["capture_count"],
                      "restart_cases": len(report["restart_cases"]), "visual_review_status": "pending"}), flush=True)
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
