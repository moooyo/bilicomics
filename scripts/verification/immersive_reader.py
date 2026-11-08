#!/usr/bin/env python3
"""Verify quiet full-canvas reading in the real offline App with XTest input.

Run only on ssh test-env in a network namespace containing loopback alone. The
harness owns its Xvfb server, generates public synthetic corner-marked pages,
and seeds the normal guest cache and durable reading anchor. It never uses demo
mode, scripted intents, direct view-model setters, a live account, or a Kindle.
Pixel equality is against generated source artwork, not a previous app capture.
"""

import argparse
import datetime
import hashlib
import json
import os
from pathlib import Path
import re
import select
import signal
import socket
import sqlite3
import subprocess
import sys
import time
import traceback

from PIL import Image, ImageChops, ImageDraw

from harness_signals import install_interruption_handlers
from reader_guest_fixtures import SCHEMA, check_isolation
from reader_v2_runtime import CONTEXT_KEYS, PROFILES, Pointer, sha256, trace_records, write_json


ACCOUNT = "guest"
COMIC = "900000002"
EPISODE = "90000000201"
REVISION = "immersive-synthetic-v1"
PAGE_COUNT = 5
GEOMETRY_KEYS = (
    "comic_id", "episode_id", "revision", "page_id", "page_index", "page_number",
    "mode", "fit", "direction", "landscape_layout", "cover_alone", "effective_spread",
    "source_x", "source_y", "zoom_ratio", "rotation", "crop_index", "crop_count",
    "viewport_width", "viewport_height", "frame_width", "frame_height",
    "second_frame_width", "second_frame_height", "left_page_number", "right_page_number",
    "focus_x", "focus_y", "focus_width", "focus_height", "transition",
)
ANCHOR_KEYS = (
    "episode_id", "revision", "page_id", "page_index", "source_x", "source_y",
    "zoom_ratio", "rotation", "mode", "finished",
)


def require(value, message):
    if not value:
        raise AssertionError(message)


def read_anchor(database):
    with sqlite3.connect(database.as_uri() + "?mode=ro", uri=True, timeout=5) as connection:
        row = connection.execute(
            "SELECT payload FROM reading_anchors WHERE account_key=? AND comic_id=?",
            (ACCOUNT, COMIC),
        ).fetchone()
    require(row is not None, "The real guest reader has no durable reading anchor")
    anchor = json.loads(row[0])
    return {key: anchor[key] for key in ANCHOR_KEYS}


def read_preferences(database):
    with sqlite3.connect(database.as_uri() + "?mode=ro", uri=True, timeout=5) as connection:
        row = connection.execute(
            "SELECT payload FROM preferences WHERE account_key=? AND key='preferences'", (ACCOUNT,),
        ).fetchone()
    require(row is not None, "The guest preferences disappeared")
    return json.loads(row[0])


def marked_page(width, height, serial):
    # Every tone is an exact member of the Reader's sixteen-level gray palette.
    # The dense non-white body prevents a synthetic gutter from being invented.
    tones = (51, 85, 119, 153, 187)
    image = Image.new("L", (width, height), tones[serial % len(tones)])
    draw = ImageDraw.Draw(image)
    for band, top in enumerate(range(0, height, 96)):
        draw.rectangle((0, top, width - 1, min(height - 1, top + 31)),
                       fill=tones[(serial + band + 1) % len(tones)])
    edge = 8
    draw.rectangle((0, 0, width - 1, height - 1), outline=17, width=edge)
    marker = min(64, width // 6, height // 6)
    for box, tone in (
        ((0, 0, marker - 1, marker - 1), 0),
        ((width - marker, 0, width - 1, marker - 1), 68),
        ((0, height - marker, marker - 1, height - 1), 136),
        ((width - marker, height - marker, width - 1, height - 1), 204),
    ):
        draw.rectangle(box, fill=tone)
    # A page-specific binary stripe distinguishes pages with the same base tone.
    for bit in range(8):
        if serial & (1 << bit):
            left = marker + bit * 12
            draw.rectangle((left, 12, left + 7, 35), fill=34)
    return image


def source_pages(size, mode, spread, thumbnail_workload_scale=1):
    width, height = size
    width = width // 2 if spread else width
    pages = []
    for index in range(PAGE_COUNT):
        serial = index + 1
        if mode == "panel":
            source = Image.new("L", (width, height * 2 + 12), 255)
            source.paste(marked_page(width, height, serial), (0, 0))
            source.paste(marked_page(width, height, serial + 32), (0, height + 12))
        elif mode == "strip":
            source = marked_page(width, height * 3, serial)
        else:
            source = marked_page(width, height // 2 if index == 3 and not spread else height, serial)
        if thumbnail_workload_scale > 1 and index != 2:
            source = source.resize((source.width * thumbnail_workload_scale,
                                    source.height * thumbnail_workload_scale), Image.Resampling.NEAREST)
        pages.append(source)
    return pages


def seed_data(directory, size, mode, direction, spread, thumbnail_workload_scale=1):
    data = directory / "data"
    data.mkdir(mode=0o700)
    database = data / "state.sqlite3"
    pages_path = data / "accounts" / hashlib.sha256(ACCOUNT.encode()).hexdigest() / "pages"
    pages_path.mkdir(mode=0o700, parents=True)
    pages = source_pages(size, mode, spread, thumbnail_workload_scale)
    stamp = int(time.time())
    descriptors, receipts, page_proofs = [], [], []
    for index, image in enumerate(pages):
        temporary = directory / ("generated-source-" + str(index + 1) + ".png")
        image.save(temporary)
        digest = sha256(temporary)
        target = pages_path / (digest + ".png")
        target.write_bytes(temporary.read_bytes())
        width, height = image.size
        descriptor = {"id": digest, "index": index, "width": width, "height": height}
        receipt = {
            "episode_id": EPISODE, "revision": REVISION, "page_id": digest, "page_index": index,
            "file_name": target.name, "sha256": digest, "bytes": target.stat().st_size,
            "width": width, "height": height, "retained": True, "verified_at": 0,
        }
        descriptors.append(descriptor)
        receipts.append(receipt)
        page_proofs.append({"descriptor": descriptor, "receipt": receipt,
                            "source": str(temporary), "source_sha256": digest,
                            "cache": str(target), "cache_sha256": sha256(target)})
    comic = {
        "id": COMIC, "title": "Immersive Synthetic Fixture", "author": "Local fixture",
        "cover_url": "", "description": "Public generated corner-marked geometry fixture",
        "genre": "", "latest_episode_id": EPISODE, "latest_episode_title": "Synthetic chapter",
        "latest_order": 1.0, "chapter_count": 1, "favorite": True, "finished": False,
        "updated_at": stamp, "last_episode_id": EPISODE, "last_read_at": stamp,
        "extra": {"fixture_only": True},
    }
    episode = {
        "id": EPISODE, "comic_id": COMIC, "title": "Synthetic chapter", "short_title": "",
        "order": 1.0, "access": "free", "expires_at": None, "pay_gold": None,
        "read": False, "extra": {"fixture_only": True, "page_count": PAGE_COUNT},
    }
    detail = {"comic": comic, "episodes": [episode], "extra": {"fixture_only": True}}
    image_index = {"episode_id": EPISODE, "revision": REVISION, "images": descriptors,
                   "extra": {"fixture_only": True, "source_urls": []}}
    initial_page = 0 if spread else 2
    anchor = {
        "episode_id": EPISODE, "revision": REVISION, "page_id": descriptors[initial_page]["id"],
        "page_index": initial_page, "source_x": 0.0, "source_y": 0.0, "zoom_ratio": 1.0,
        "rotation": 0, "mode": mode, "finished": False, "updated_at": stamp,
    }
    preferences = {
        "onboarded": True, "reader_guidance_seen": True, "preload": 0, "launch": "resume",
        "mode": mode, "fit": "page", "direction": direction, "dither": False,
        "landscape": "spread" if spread else "single", "cover_alone": True,
        "footer": True,
    }
    with sqlite3.connect(database, timeout=5) as connection:
        connection.executescript(SCHEMA)
        connection.execute("INSERT INTO comics VALUES(?,?,?,?,?)",
                           (ACCOUNT, COMIC, json.dumps(comic), stamp, stamp))
        connection.execute("INSERT INTO comic_details VALUES(?,?,?)", (ACCOUNT, COMIC, json.dumps(detail)))
        connection.execute("INSERT INTO catalog_members VALUES(?,?,?,?)", (ACCOUNT, "shelf", COMIC, 0))
        connection.execute("INSERT INTO image_indices VALUES(?,?,?,?,?)",
                           (ACCOUNT, EPISODE, REVISION, json.dumps(image_index), stamp))
        connection.execute("INSERT INTO reading_anchors VALUES(?,?,?,?,?,?)",
                           (ACCOUNT, COMIC, EPISODE, REVISION, json.dumps(anchor), stamp))
        connection.execute("INSERT INTO preferences VALUES(?,?,?)", (ACCOUNT, "preferences", json.dumps(preferences)))
        for descriptor, receipt in zip(descriptors, receipts):
            connection.execute("INSERT INTO page_receipts VALUES(?,?,?,?,?,?)",
                               (ACCOUNT, EPISODE, REVISION, descriptor["id"], descriptor["index"], json.dumps(receipt)))
            connection.execute("INSERT INTO page_identity_history VALUES(?,?,?,?,?,?)",
                               (ACCOUNT, EPISODE, REVISION, descriptor["id"], descriptor["id"], receipt["bytes"]))
    proof = {
        "account_key": ACCOUNT, "comic_id": COMIC, "episode_id": EPISODE, "revision": REVISION,
        "data": str(data), "database": str(database), "pages": page_proofs,
        "initial_anchor": anchor, "initial_preferences": preferences,
        "fixture_origin": "Deterministic locally generated grayscale PNGs; no acquired comic content",
        "receipt_origin": "Controlled guest cache seed; verified_at=0 is not upstream validation",
        "demo": False, "session_seeded": False, "credentials_seeded": False,
        "remote_writes": False, "scripted_intents": False,
        "non_current_page_thumbnail_workload_scale": thumbnail_workload_scale,
    }
    write_json(directory / "seed.json", proof)
    check_isolation(data, database)
    return data, database, pages, proof


def stop_owned_group(process, timeout=10):
    if process is None:
        return {"started": False}
    signaled = False
    try:
        os.killpg(process.pid, signal.SIGTERM)
        signaled = True
    except ProcessLookupError:
        pass
    try:
        process.wait(timeout=timeout)
    except subprocess.TimeoutExpired:
        os.killpg(process.pid, signal.SIGKILL)
        process.wait(timeout=5)
    remaining = False
    try:
        os.killpg(process.pid, 0)
        remaining = True
        os.killpg(process.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    return {"started": True, "pid": process.pid, "group_termination_sent": signaled,
            "return_code": process.returncode, "owned_group_remained_after_parent_reaped": remaining}


class PrivateDisplay:
    def __init__(self, directory):
        self.process = None
        self.log = (directory / "xvfb.log").open("w")
        self.previous = os.environ.get("DISPLAY")
        self.identity = {}
        read_fd, write_fd = os.pipe()
        try:
            command = ["Xvfb", "-displayfd", str(write_fd), "-screen", "0", "1600x1600x24",
                       "-nolisten", "tcp", "-noreset", "-ac"]
            self.process = subprocess.Popen(command, stdout=self.log, stderr=subprocess.STDOUT,
                                            pass_fds=(write_fd,), start_new_session=True)
            os.close(write_fd)
            write_fd = None
            line = b""
            deadline = time.monotonic() + 15
            while time.monotonic() < deadline and b"\n" not in line:
                require(self.process.poll() is None, "The private Xvfb server exited during startup")
                if select.select([read_fd], [], [], 0.1)[0]:
                    chunk = os.read(read_fd, 64)
                    require(chunk, "Xvfb closed its display-number pipe without a number")
                    line += chunk
            require(re.fullmatch(rb"\d+\n", line), "Xvfb did not allocate a private display in time")
            self.name = ":" + line.decode().strip()
            os.environ["DISPLAY"] = self.name
            self.identity = {"display": self.name, "process_id": self.process.pid, "command": command,
                             "ownership": "Created and reaped by this harness", "tcp_listening": False}
        except BaseException:
            self.close()
            raise
        finally:
            os.close(read_fd)
            if write_fd is not None:
                os.close(write_fd)

    def close(self):
        cleanup = stop_owned_group(self.process)
        if self.previous is None:
            os.environ.pop("DISPLAY", None)
        else:
            os.environ["DISPLAY"] = self.previous
        if not self.log.closed:
            self.log.close()
        self.identity["cleanup"] = cleanup
        return cleanup


def mapped_window(process):
    tree = subprocess.check_output(["xwininfo", "-root", "-tree"], text=True, timeout=5)
    matches = []
    for candidate in re.finditer(r'(0x[0-9a-fA-F]+)\s+"BiliComics"', tree):
        identifier = candidate.group(1)
        owner = subprocess.check_output(["xprop", "-id", identifier, "_NET_WM_PID"], text=True, timeout=5)
        pid = re.search(r"_NET_WM_PID\(CARDINAL\)\s*=\s*(\d+)", owner)
        if pid is None or int(pid.group(1)) != process.pid:
            continue
        details = subprocess.check_output(["xwininfo", "-id", identifier], text=True, timeout=5)
        if "Map State: IsViewable" not in details:
            continue
        fields = {key: int(re.search(pattern, details).group(1)) for key, pattern in {
            "x": r"Absolute upper-left X:\s*(-?\d+)", "y": r"Absolute upper-left Y:\s*(-?\d+)",
            "width": r"Width:\s*(\d+)", "height": r"Height:\s*(\d+)",
        }.items()}
        matches.append({"id": identifier, "process_id": int(pid.group(1)), **fields})
    require(len(matches) <= 1, "The selected process owns multiple mapped application windows")
    return matches[0] if matches else None


class ReaderCase:
    def __init__(self, arguments, pointer, profile, landscape, mode, direction="ltr", spread=False, scenario=None):
        self.arguments, self.pointer = arguments, pointer
        self.profile, self.landscape, self.mode = profile, landscape, mode
        self.direction, self.spread = direction, spread
        self.scenario = scenario
        self.size = tuple(reversed(PROFILES[profile])) if landscape else PROFILES[profile]
        self.name = ("landscape" if landscape else "portrait") + "-" + ("spread-" + direction if spread else mode)
        if scenario:
            self.name += "-" + scenario
        self.directory = arguments.output / (profile + "-" + self.name)
        self.directory.mkdir(mode=0o700)
        self.trace, self.capture = self.directory / "lifecycle.jsonl", self.directory / "final.png"
        self.process, self.window, self.log, self.state = None, None, None, None
        self.pointer_held = False
        self.deadline = time.monotonic() + arguments.case_timeout
        self.data, self.database, self.pages, self.seed = seed_data(
            self.directory, self.size, mode, direction, spread, 3 if scenario else 1,
        )
        self.result = {
            "name": self.name, "profile": profile, "landscape": landscape, "mode": mode,
            "direction": direction, "spread": spread, "passed": False, "runtime_passed": False,
            "interaction_scenario": scenario,
            "observations": [], "clicks": [], "screenshots": [], "assertions": [],
            "fixture": str(self.directory / "seed.json"), "visual_review_required": [],
            "legacy_footer_preference": True, "intent_delivery": "Actual XTest pointer events only",
        }
        self.last_quiet = None

    def wait(self, description, predicate, timeout=30):
        deadline = min(self.deadline, time.monotonic() + timeout)
        last_error = None
        while time.monotonic() < deadline:
            if self.process is not None:
                require(self.process.poll() is None, "The real App exited while waiting for " + description)
            try:
                value = predicate()
                if value:
                    return value
            except (FileNotFoundError, json.JSONDecodeError, sqlite3.Error) as error:
                last_error = str(error)
            time.sleep(0.04)
        raise AssertionError("Timed out waiting for " + description + "; last read error: " + str(last_error))

    def observe(self, description, expected=None, after=0):
        expected = expected or {}

        def find():
            for record in reversed(trace_records(self.trace)[after:]):
                state = record.get("reader_state")
                if not (record.get("event") == "reader-ready" and record.get("process_id") == self.process.pid
                        and record.get("route") == "reader" and record.get("reader_loading") is False
                        and isinstance(state, dict) and state.get("rendering") is False):
                    continue
                if all(state.get(key) == value for key, value in expected.items()):
                    return record
            return None

        record = self.wait(description, find, self.arguments.startup_timeout if self.state is None else 30)
        self.state = record["reader_state"]
        require(all(key in self.state for key in GEOMETRY_KEYS + ("menu", "sheet", "render_version", "rendering")),
                "The actual lifecycle snapshot lacks a required immersive geometry observable")
        require(self.state["comic_id"] == COMIC and self.state["episode_id"] == EPISODE
                and self.state["revision"] == REVISION, "The live Reader escaped its seeded guest identity")
        self.result["observations"].append({"description": description, "expected": expected, "record": record})
        self.assert_viewport()
        return self.state

    def assert_viewport(self):
        expected = (self.size[0] // 2 if self.spread else self.size[0], self.size[1])
        require((self.state["viewport_width"], self.state["viewport_height"]) == expected,
                "The production Reader viewport reserves an artificial reading margin")
        require(self.state["effective_spread"] is self.spread, "The effective spread mode differs from the real canvas")
        if self.spread:
            page = self.state["page_number"]
            base = 2 + ((page - 2) // 2) * 2
            slots = [0, 1] if page == 1 else [base, base + 1]
            if self.direction == "rtl":
                slots.reverse()
            require([self.state["left_page_number"], self.state["right_page_number"]] == slots,
                    "The actual physical spread slots differ from the declared LTR or RTL source order")
        if not self.spread or self.state["left_page_number"] in range(1, PAGE_COUNT + 1):
            require((self.state["frame_width"], self.state["frame_height"]) == expected,
                    "The actual prepared primary frame does not fill the synthetic source viewport")
        else:
            require((self.state["frame_width"], self.state["frame_height"]) == (1, 1),
                    "The empty cover slot lost its source-independent placeholder")
        if self.spread and self.state["right_page_number"] in range(1, PAGE_COUNT + 1):
            require((self.state["second_frame_width"], self.state["second_frame_height"]) == expected,
                    "The occupied secondary leaf does not use its full half-canvas")

    def snapshot(self, name):
        previous, stable = None, 0
        capture = None
        deadline = min(self.deadline, time.monotonic() + 5)
        while time.monotonic() < deadline:
            require(self.process.poll() is None, "The real App exited before its X11 capture")
            capture = self.pointer.capture(self.window, self.directory / (name + ".png"))
            stable = stable + 1 if capture["rgb_sha256"] == previous else 0
            previous = capture["rgb_sha256"]
            if stable >= 2:
                capture.update({"name": name, "window_id": self.window["id"],
                                "window_process_id": self.window["process_id"]})
                self.result["screenshots"].append(capture)
                return capture
            time.sleep(0.08)
        raise AssertionError("The actual X11 pixels did not settle for " + name)

    def tap(self, name, point):
        require(0 <= point[0] < self.size[0] and 0 <= point[1] < self.size[1],
                "The planned XTest event lies outside the checked application window")
        require(self.window["process_id"] == self.process.pid, "The target window changed process identity")
        prior = len(trace_records(self.trace))
        started = time.monotonic()
        self.pointer.click(self.window, point)
        self.result["clicks"].append({"name": name, "client": list(point), "window_id": self.window["id"],
                                      "window_process_id": self.process.pid, "trace_records_before": prior,
                                      "input_started_monotonic_seconds": started,
                                      "input_finished_monotonic_seconds": time.monotonic()})
        return prior

    def click(self, name, point, expected):
        prior = self.tap(name, point)
        self.observe(name, expected, after=prior)
        return self.snapshot(name)

    def press(self, name, point):
        require(not self.pointer_held, "An earlier fixture press is still held")
        require(0 <= point[0] < self.size[0] and 0 <= point[1] < self.size[1]
                and self.window["process_id"] == self.process.pid,
                "The held XTest press is outside the checked application window")
        self.pointer.xtst.XTestFakeMotionEvent(
            self.pointer.display, self.pointer.x11.XDefaultScreen(self.pointer.display),
            self.window["x"] + point[0], self.window["y"] + point[1], 0,
        )
        self.pointer.x11.XFlush(self.pointer.display)
        self.pointer.x11.XSync(self.pointer.display, 0)
        time.sleep(0.08)
        prior = len(trace_records(self.trace))
        require(self.pointer.xtst.XTestFakeButtonEvent(self.pointer.display, 1, 1, 0),
                "XTest rejected the real button-down event")
        self.pointer_held = True
        self.pointer_down_at = time.monotonic()
        self.pointer.x11.XFlush(self.pointer.display)
        self.pointer.x11.XSync(self.pointer.display, 0)
        self.result.setdefault("input_phases", []).append({
            "name": name, "phase": "button-down", "client": list(point),
            "window_id": self.window["id"], "window_process_id": self.process.pid,
            "monotonic_seconds": self.pointer_down_at, "trace_records_before": prior,
        })
        return prior

    def release(self, name):
        require(self.pointer_held, "There is no held fixture press to release")
        prior = len(trace_records(self.trace))
        require(self.pointer.xtst.XTestFakeButtonEvent(self.pointer.display, 1, 0, 0),
                "XTest rejected the real button-up event")
        released = time.monotonic()
        self.pointer_held = False
        self.pointer.x11.XFlush(self.pointer.display)
        self.pointer.x11.XSync(self.pointer.display, 0)
        self.result.setdefault("input_phases", []).append({
            "name": name, "phase": "button-up", "window_id": self.window["id"],
            "window_process_id": self.process.pid, "monotonic_seconds": released,
            "hold_seconds": released - self.pointer_down_at, "trace_records_before": prior,
        })
        return prior

    def pending_worker(self, description):
        record = next((record for record in reversed(trace_records(self.trace))
                       if record.get("event") == "reader-ready" and record.get("process_id") == self.process.pid
                       and record.get("route") == "reader"), None)
        require(record is not None, "The real thumbnail worker has no current lifecycle observation")
        state = record.get("reader_state") or {}
        expected = {"menu": True, "sheet": "grid", "thumbnails_count": PAGE_COUNT,
                    "thumbnails_loaded_count": 0, "thumbnails_loading": True, "thumbnails_rendering": True}
        proof = {"description": description, "record": record,
                 "observed_monotonic_seconds": time.monotonic(), "expected": expected}
        self.result.setdefault("worker_observations", []).append(proof)
        require(all(state.get(key) == value for key, value in expected.items()),
                "The pending thumbnail observation window closed before the real press; "
                "this run does not establish loading-input coverage")
        return proof

    def assert_unchanged(self, before, description, render_version=True):
        keys = GEOMETRY_KEYS + (("render_version",) if render_version else ())
        changed = {key: [before[key], self.state[key]] for key in keys if before[key] != self.state[key]}
        require(not changed, description + " changed source identity, anchor, or raster geometry: " + json.dumps(changed))
        self.result["assertions"].append({"description": description, "stable_keys": list(keys), "passed": True})

    def assert_page_action(self, before, description):
        require(self.state["render_version"] > before["render_version"],
                description + " closed the menu without executing the actual page-selection intent")
        self.result["assertions"].append({"description": description + " reached the real controller",
                                           "render_version_before": before["render_version"],
                                           "render_version_after": self.state["render_version"], "passed": True})

    def assert_pixels(self, name, capture, expected):
        expected = expected.convert("RGB")
        expected_path = self.directory / (name + "-expected.png")
        expected.save(expected_path)
        with Image.open(capture["path"]) as image:
            actual = image.convert("RGB")
        require(actual.size == expected.size, "The actual window differs from the expected full-canvas size")
        expected_hash = hashlib.sha256(expected.tobytes()).hexdigest()
        equal = hashlib.sha256(actual.tobytes()).hexdigest() == expected_hash
        proof = {"description": name, "passed": equal, "method": "Exact full-window RGB equality",
                 "actual": capture["path"], "expected": str(expected_path), "expected_rgb_sha256": expected_hash,
                 "dimensions": list(expected.size), "artwork_origin": "Generated synthetic source PNG"}
        if not equal:
            difference = ImageChops.difference(actual, expected)
            difference_path = self.directory / (name + "-difference.png")
            difference.save(difference_path)
            proof.update({"difference": str(difference_path), "difference_bounds": difference.getbbox()})
        self.result["assertions"].append(proof)
        require(equal, "The quiet full-window pixels differ from the expected source artwork: " + str(expected_path))
        self.last_quiet = capture

    def expected_image(self):
        width, height = self.size
        if self.spread:
            canvas = Image.new("L", self.size, 255)
            for position, field in enumerate(("left_page_number", "right_page_number")):
                page = self.state[field]
                if page in range(1, PAGE_COUNT + 1):
                    source = self.pages[page - 1]
                    require(source.size == (width // 2, height), "The spread fixture unexpectedly needs resampling")
                    canvas.paste(source, (position * (width // 2), 0))
            return canvas
        source = self.pages[self.state["page_index"]]
        if self.mode == "panel":
            require(self.state["crop_index"] in (0, 1), "The two-panel source produced an unexpected region")
            top = 0 if self.state["crop_index"] == 0 else height + 12
            return source.crop((0, top, width, top + height))
        if self.mode == "strip":
            top = round(self.state["source_y"] * source.height)
            require(source.width == width and top + height <= source.height,
                    "The selected strip evidence unexpectedly needs resampling or padding")
            return source.crop((0, top, width, top + height))
        require(source.width == width and source.height <= height, "The whole-page fixture unexpectedly needs resampling")
        canvas = Image.new("L", self.size, 255)
        canvas.paste(source, (0, (height - source.height) // 2))
        return canvas

    def quiet(self, name, capture=None):
        require(self.state["menu"] is False and self.state["sheet"] == "" and self.state["transition"] is False,
                "The asserted quiet state still contains an active overlay or episode transition")
        capture = capture or self.snapshot(name)
        self.assert_pixels(name, capture, self.expected_image())
        require(read_preferences(self.database)["footer"] is True,
                "The legacy footer preference was changed to hide a persistent HUD regression")
        require(self.state.get("footer") is True,
                "The actual Reader snapshot no longer exercises the legacy enabled footer preference")
        return capture

    def open_menu(self, name):
        before, anchor = dict(self.state), read_anchor(self.database)
        quiet = self.last_quiet
        capture = self.click(name, (self.size[0] // 2, self.size[1] // 2), {"menu": True, "sheet": ""})
        self.assert_unchanged(before, "Opening the contextual menu")
        require(read_anchor(self.database) == anchor, "Opening the contextual menu mutated the durable source anchor")
        top = (98 if self.profile == "scribe" else 86) + 2
        bottom = self.size[1] - (208 if self.profile == "scribe" else 182) - 2
        box = (0, top, self.size[0], bottom)
        with Image.open(quiet["path"]) as image:
            body_before = image.convert("RGB").crop(box).tobytes()
        with Image.open(capture["path"]) as image:
            body_after = image.convert("RGB").crop(box).tobytes()
        require(body_before == body_after, "The contextual menu moved, resized, dimmed, or cropped the uncovered artwork")
        require(capture["rgb_sha256"] != quiet["rgb_sha256"], "The center tap did not visibly reveal contextual controls")
        self.result["assertions"].append({"description": "The menu overlays the original raster",
                                           "uncovered_body": list(box), "exact_pixels_preserved": True,
                                           "durable_anchor_preserved": True, "passed": True})
        self.result["visual_review_required"].append({
            "artifact": capture["path"], "status": "pending",
            "checks": ["The contextual header shows chapter, cache, reading mode, and progress information.",
                       "The progress control and menu tools are visible and contained in the viewport.",
                       "The menu overlays the full-canvas artwork without moving or resizing it."],
        })
        return before, anchor, quiet

    def outside_round_trip(self, name):
        before, anchor, quiet = self.open_menu(name + "-open")
        closed = self.click(name + "-outside-close", (self.size[0] // 2, self.size[1] // 2),
                            {"menu": False, "sheet": ""})
        self.assert_unchanged(before, "Closing the menu outside its controls")
        require(read_anchor(self.database) == anchor, "Closing the contextual menu mutated the durable source anchor")
        require(closed["rgb_sha256"] == quiet["rgb_sha256"], "Outside close did not restore the exact quiet window pixels")
        self.quiet(name + "-quiet-restored", closed)

    def same_page_seek(self):
        before, anchor, quiet = self.open_menu("progress-open")
        require(before["page_number"] == 3 and not self.spread, "The progress fixture must start at source page 3 of 5")
        # With five source pages, the middle of the visible seek track selects
        # source page 3. This point lies within its broad middle target interval.
        point = (self.size[0] // 2, self.size[1] - (134 if self.profile == "scribe" else 112))
        closed = self.click("progress-select-current-page", point,
                            {"menu": False, "sheet": "", "page_number": 3, "crop_index": 0})
        self.assert_unchanged(before, "Selecting the current source page from progress", render_version=False)
        self.assert_page_action(before, "Selecting the current progress position")
        require(read_anchor(self.database) == anchor, "Selecting the current progress position changed its durable anchor")
        require(closed["rgb_sha256"] == quiet["rgb_sha256"], "The progress selection changed the quiet raster")
        self.quiet("progress-quiet-restored", closed)

    def wait_current_thumbnail(self, point):
        # A stable placeholder is not a ready thumbnail. The generated current
        # page has dense non-white pixels here, while its loading placeholder
        # is white. Observing painted artwork also waits for the worker's model
        # replacement before the actual down/up pair targets that model.
        patch = (point[0] - 8, point[1] - 8, point[0] + 9, point[1] + 9)
        previous, stable = None, 0
        deadline = min(self.deadline, time.monotonic() + 30)
        last_fraction = 0.0
        while time.monotonic() < deadline:
            require(self.process.poll() is None, "The real App exited while waiting for the current thumbnail")
            capture = self.pointer.capture(self.window, self.directory / "overview-thumbnails-ready.png")
            with Image.open(capture["path"]) as image:
                pixels = image.convert("L").crop(patch).tobytes()
            last_fraction = sum(value < 245 for value in pixels) / len(pixels)
            digest = hashlib.sha256(pixels).hexdigest()
            painted = last_fraction >= 0.95
            stable = stable + 1 if painted and digest == previous else 0
            previous = digest if painted else None
            if stable >= 2:
                capture.update({"name": "overview-thumbnails-ready", "window_id": self.window["id"],
                                "window_process_id": self.window["process_id"]})
                self.result["screenshots"].append(capture)
                self.result["thumbnail_readiness"] = {
                    "passed": True,
                    "method": "Actual X11 current-thumbnail artwork pixels, not a timer or unchanged placeholder",
                    "artifact": capture["path"], "source_page": 3, "click_target": list(point),
                    "patch": list(patch), "non_white_fraction": last_fraction,
                    "stable_painted_observations": stable + 1, "patch_gray_sha256": digest,
                    "observed_monotonic_seconds": time.monotonic(),
                }
                return
            time.sleep(0.08)
        self.result["thumbnail_readiness"] = {
            "passed": False, "patch": list(patch), "click_target": list(point),
            "artifact": str(self.directory / "overview-thumbnails-ready.png"),
            "last_non_white_fraction": last_fraction,
        }
        raise AssertionError("The actual current thumbnail did not paint stable source artwork before selection")

    def grid_point(self, grid):
        with Image.open(grid["path"]) as image:
            gray = image.convert("L")
            rows = []
            for y in range(0, self.size[1] - 100):
                if sum(value <= 34 for value in gray.crop((0, y, self.size[0], y + 1)).getdata()) >= self.size[0] * 0.98:
                    rows.append(y)
        require(rows, "The real overview sheet has no detectable full-width top rule")
        # A compact landscape sheet can reach the menu header. Its top rule is
        # the last full-width black rule, possibly touching the header's rule.
        grid_top = rows[-1]
        columns, edge, bar = (5, 48, 64) if self.profile == "scribe" else (4, 24, 56)
        cell_width = (self.size[0] - 2 * edge - (columns - 1) * 14) / columns
        point = (round(edge + 2 * (cell_width + 14) + cell_width / 2), grid_top + bar + 14 + 24)
        return point, grid_top

    def grid_tool_point(self):
        left, right = (32, 48) if self.profile == "scribe" else (12, 24)
        tool_width = (self.size[0] - left - right - 24) / 5
        return (round(left + (tool_width + 6) + tool_width / 2),
                self.size[1] - (54 if self.profile == "scribe" else 44))

    def same_page_grid(self):
        before, anchor, quiet = self.open_menu("overview-open")
        grid = self.click("overview-show-current-source-page", self.grid_tool_point(), {"menu": True, "sheet": "grid"})
        self.assert_unchanged(before, "Opening the source-page overview")
        point, _ = self.grid_point(grid)
        self.wait_current_thumbnail(point)
        closed = self.click("overview-select-current-source-page", point,
                            {"menu": False, "sheet": "", "page_number": 3, "crop_index": 0})
        self.assert_unchanged(before, "Selecting the current source-page thumbnail", render_version=False)
        self.assert_page_action(before, "Selecting the current source-page thumbnail")
        require(read_anchor(self.database) == anchor, "Selecting the current source page changed its durable anchor")
        require(closed["rgb_sha256"] == quiet["rgb_sha256"], "The source-page selection changed the quiet raster")
        self.quiet("overview-quiet-restored", closed)

    def loading_grid(self):
        prior = self.tap("race-overview-show", self.grid_tool_point())
        self.observe("the actual pending thumbnail worker", {
            "menu": True, "sheet": "grid", "thumbnails_count": PAGE_COUNT,
            "thumbnails_loaded_count": 0, "thumbnails_loading": True, "thumbnails_rendering": True,
        }, after=prior)
        deadline = min(self.deadline, time.monotonic() + 3)
        while time.monotonic() < deadline:
            self.pending_worker("pending before the loading-grid capture")
            capture = self.pointer.capture(self.window, self.directory / "race-loading-grid.png")
            point, grid_top = self.grid_point(capture)
            # These optional race fixtures use PW portrait. Its overview rule
            # must be below the header before the placeholder can be targeted.
            if grid_top > 88:
                patch = (point[0] - 8, point[1] - 8, point[0] + 9, point[1] + 9)
                with Image.open(capture["path"]) as image:
                    gray = image.convert("L")
                    pixels = gray.crop(patch).tobytes()
                    cell_width = (self.size[0] - 48 - 3 * 14) / 4
                    current_left = round(24 + 2 * (cell_width + 14))
                    current_top = grid_top + 56 + 14
                    outline = gray.crop((current_left + 1, current_top + 8,
                                         current_left + 3, current_top + 40)).tobytes()
                # The actual current-row frame must also be present. A stale
                # menu footer can otherwise resemble the sheet's black rule.
                framed = sum(value <= 34 for value in outline) / len(outline) >= 0.95
                if framed and sum(value >= 245 for value in pixels) / len(pixels) >= 0.95:
                    capture.update({"name": "race-loading-grid", "window_id": self.window["id"],
                                    "window_process_id": self.window["process_id"]})
                    self.result["screenshots"].append(capture)
                    self.result["loading_target"] = {"client": list(point), "patch": list(patch),
                                                     "placeholder_is_white": True, "current_frame_outline_observed": True,
                                                     "artifact": capture["path"]}
                    self.pending_worker("pending after the actual placeholder was painted")
                    return point
            time.sleep(0.02)
        raise AssertionError("The actual loading thumbnail placeholder did not become targetable in time")

    def interaction_race(self):
        require(self.mode == "page" and not self.landscape and not self.spread
                and self.profile in ("pw5", "pw6"), "Interaction races require the real PW portrait page fixture")
        before, anchor, quiet = self.open_menu("race-menu-open")
        point = self.loading_grid()
        self.assert_unchanged(before, "Opening the real loading source-page overview")
        self.press("loading-current-thumbnail", point)
        self.pending_worker("actual worker state while the XTest button is down")
        if self.scenario == "loading-hold":
            self.observe("the real thumbnail worker completed during the held press", {
                "menu": True, "sheet": "grid", "thumbnails_count": PAGE_COUNT,
                "thumbnails_loaded_count": PAGE_COUNT, "thumbnails_loading": False, "thumbnails_rendering": False,
            })
            self.result.setdefault("worker_observations", []).append({
                "description": "actual worker completion while the XTest button remains down",
                "record": self.result["observations"][-1]["record"],
                "observed_monotonic_seconds": time.monotonic(), "pointer_held": self.pointer_held,
            })
            require(self.pointer_held, "The loading-hold fixture released before real worker completion")
            self.wait_current_thumbnail(point)
            self.assert_unchanged(before, "Completing thumbnails during a held source-page press")
        else:
            require(self.scenario == "loading-tap", "The selected interaction race is unknown")
            remaining = 0.08 - (time.monotonic() - self.pointer_down_at)
            if remaining > 0:
                time.sleep(remaining)
        prior = self.release(self.scenario + "-current-thumbnail")
        self.observe(self.scenario + " selected the real current source page", {
            "menu": False, "sheet": "", "page_number": 3, "crop_index": 0,
        }, after=prior)
        closed = self.snapshot(self.scenario + "-selected")
        self.assert_unchanged(before, "Selecting the current thumbnail through " + self.scenario, render_version=False)
        self.assert_page_action(before, "The " + self.scenario + " current-thumbnail selection")
        require(read_anchor(self.database) == anchor, "The loading-input case changed the durable source anchor")
        require(closed["rgb_sha256"] == quiet["rgb_sha256"], "The loading-input selection changed the quiet raster")
        self.quiet(self.scenario + "-quiet-restored", closed)
        self.result["source_page_sequence"] = [3, 3]
        self.result["loading_input_scope"] = (
            "Real XTest press spans actual production thumbnail-worker completion and painted artwork"
            if self.scenario == "loading-hold" else "Real 80 ms XTest tap starts while the production thumbnail worker is pending"
        )

    def turn(self, name, forward, expected):
        fraction = 0.85 if (forward != (self.direction == "rtl")) else 0.15
        capture = self.click(name, (round(self.size[0] * fraction), self.size[1] // 2),
                             {"menu": False, "sheet": "", "transition": False, **expected})
        self.quiet(name + "-quiet", capture)

    def start(self):
        environment = os.environ.copy()
        for key in CONTEXT_KEYS:
            environment.pop(key, None)
        environment.pop("WAYLAND_DISPLAY", None)
        environment.pop("WAYLAND_SOCKET", None)
        environment["WINIT_UNIX_BACKEND"] = "x11"
        environment["SLINT_SCALE_FACTOR"] = "1"
        command = [str(self.arguments.binary), "--offline", "--device", self.profile, "--screen", "reader",
                   "--data-dir", str(self.data), "--lifecycle-trace", str(self.trace),
                   "--capture", str(self.capture), "--exit-after-ms", str((self.arguments.case_timeout + 20) * 1000)]
        if self.landscape:
            command.append("--landscape")
        self.log = (self.directory / "application.log").open("w")
        self.process = subprocess.Popen(command, env=environment, stdout=self.log,
                                        stderr=subprocess.STDOUT, start_new_session=True)
        self.result["command"] = command
        self.result["process_id"] = self.process.pid
        self.wait("the actual App ready event", lambda: next((record for record in trace_records(self.trace)
                  if record.get("event") == "ready" and record.get("process_id") == self.process.pid), None),
                  timeout=self.arguments.startup_timeout)
        executable = (Path("/proc") / str(self.process.pid) / "exe").resolve(strict=True)
        require(sha256(executable) == self.arguments.expected_binary_sha256,
                "The running process executable differs from the selected build")
        self.result["live_executable"] = {"path": str(executable), "sha256": sha256(executable)}
        self.observe("the real cached guest Reader frame", {
            "mode": self.mode, "page_number": 1 if self.spread else 3,
            "direction": self.direction, "menu": False, "sheet": "", "transition": False,
        })
        self.window = self.wait("the PID-checked mapped application window", lambda: mapped_window(self.process))
        require((self.window["width"], self.window["height"]) == self.size,
                "The actual window differs from the selected portrait or landscape design profile")
        self.result["window"] = self.window
        self.quiet("initial-quiet")

    def drive(self):
        if self.scenario:
            self.interaction_race()
            return
        self.outside_round_trip("initial-menu")
        if self.spread:
            sequence = [1]
            for name, forward, page in (
                ("cover-to-first-pair", True, 2), ("first-to-second-pair", True, 4),
                ("second-back-to-first-pair", False, 2), ("first-pair-back-to-cover", False, 1),
            ):
                self.turn(name, forward, {"page_number": page})
                sequence.append(page)
                if page == 2 and forward:
                    self.outside_round_trip("paired-menu")
            self.result["source_page_sequence"] = sequence
        elif self.mode == "page":
            self.same_page_seek()
            self.same_page_grid()
            self.turn("whole-page-with-letterbox", True, {"page_number": 4, "crop_index": 0})
            require(self.state["focus_width"] == 1 and self.state["focus_height"] == 1,
                    "Whole-page fit cropped the mismatched-aspect source image")
            self.outside_round_trip("letterbox-menu")
            self.turn("letterbox-back-to-full-canvas", False, {"page_number": 3, "crop_index": 0})
            self.result["source_page_sequence"] = [3, 4, 3]
        else:
            require(self.state["crop_count"] >= 2, "The real panel or strip source has no next viewport")
            before_page = self.state["page_id"]
            self.turn("next-real-source-region", True, {"page_number": 3, "crop_index": 1})
            require(self.state["page_id"] == before_page and self.state["source_y"] > 0,
                    "The panel or strip turn invented a source page instead of advancing its anchor")
            self.outside_round_trip("advanced-region-menu")
            self.turn("previous-real-source-region", False, {"page_number": 3, "crop_index": 0})
            self.result["source_page_sequence"] = [3, 3, 3]

    def finish(self):
        os.kill(self.process.pid, signal.SIGTERM)
        require(self.process.wait(timeout=20) == 0, "The real App did not capture and exit cleanly")
        metadata = json.loads(self.capture.with_suffix(".json").read_text(encoding="utf-8"))
        reader = metadata.get("reader", {})
        require(metadata.get("process_id") == self.process.pid and metadata.get("demo") is False
                and metadata.get("offline") is True and metadata.get("route") == "reader"
                and metadata.get("backend") == "desktop-software" and metadata.get("profile") == self.profile
                and metadata.get("landscape") is self.landscape and metadata.get("snapshot_size") == list(self.size)
                and metadata.get("startup", {}).get("booting") is False
                and metadata.get("startup", {}).get("error") is False and metadata.get("network_prompt") is False,
                "The final capture did not remain the original healthy offline guest App")
        require(reader.get("menu") is False and reader.get("sheet") == "" and reader.get("loading") is False
                and reader.get("transition") is False and reader.get("page") == self.state["page_number"]
                and reader.get("pages") == PAGE_COUNT and reader.get("mode") == self.mode,
                "The final view model differs from the actual pointer-controlled quiet Reader")
        with Image.open(self.capture) as image:
            final_rgb = hashlib.sha256(image.convert("RGB").tobytes()).hexdigest()
        require(final_rgb == self.last_quiet["rgb_sha256"], "The final app capture differs from the last quiet X11 window")
        check_isolation(self.data, self.database)
        require(read_preferences(self.database)["footer"] is True, "The legacy footer setting was silently rewritten")
        self.result.update({"passed": True, "runtime_passed": True, "final_reader_state": self.state,
                            "final_capture": str(self.capture), "final_capture_sha256": sha256(self.capture),
                            "final_metadata": metadata, "final_durable_anchor": read_anchor(self.database),
                            "persistent_hud_absence": "Proven by exact generated-source full-window RGB equality"})

    def close(self):
        if self.pointer_held:
            try:
                self.release("owned-fixture-cleanup")
            except Exception as error:
                self.result["passed"] = self.result["runtime_passed"] = False
                self.result["pointer_cleanup_error"] = str(error)
        cleanup = stop_owned_group(self.process)
        self.result["process_cleanup"] = cleanup
        if cleanup.get("owned_group_remained_after_parent_reaped"):
            self.result["passed"] = self.result["runtime_passed"] = False
            self.result.setdefault("error", "The private App left an owned child process group behind")
        if self.log is not None and not self.log.closed:
            self.log.close()
        self.result["trace_path"] = str(self.trace)
        write_json(self.directory / "result.json", self.result)


def selected_cases(arguments):
    for profile in arguments.profiles:
        for landscape in (False, True):
            for mode in arguments.modes:
                yield profile, landscape, mode, "ltr", False, None
    if "page" in arguments.modes and "scribe" in arguments.profiles:
        for direction in ("ltr", "rtl"):
            yield "scribe", True, "page", direction, True, None
    if arguments.interaction_races:
        profile = next(name for name in arguments.profiles if name in ("pw5", "pw6"))
        for scenario in ("loading-tap", "loading-hold"):
            yield profile, False, "page", "ltr", False, scenario


def main():
    require(sys.platform.startswith("linux") and os.environ.get("BILICOMICS_REMOTE_VERIFICATION") == "1",
            "Run through ssh test-env with BILICOMICS_REMOTE_VERIFICATION=1; local verification is not authorized")
    os.umask(0o077)
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--workspace", type=Path, required=True)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--expected-binary-sha256", required=True)
    parser.add_argument("--source-manifest", type=Path, required=True)
    parser.add_argument("--source-sha256", required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--profiles", nargs="+", choices=list(PROFILES), default=["pw5", "scribe"])
    parser.add_argument("--modes", nargs="+", choices=["page", "panel", "strip"], default=["page", "panel", "strip"])
    parser.add_argument("--interaction-races", action="store_true",
                        help="Add two real PW portrait loading-input regressions without production delay hooks")
    parser.add_argument("--startup-timeout", type=int, default=90)
    parser.add_argument("--case-timeout", type=int, default=150)
    arguments = parser.parse_args()
    require({name for _, name in socket.if_nameindex()} == {"lo"},
            "Run the entire harness under unshare --net with no external network interface")
    workspace = arguments.workspace.resolve(strict=True)
    arguments.binary = arguments.binary.resolve(strict=True)
    arguments.source_manifest = arguments.source_manifest.resolve(strict=True)
    arguments.expected_binary_sha256 = arguments.expected_binary_sha256.lower()
    arguments.source_sha256 = arguments.source_sha256.lower()
    require(arguments.binary.is_file() and os.access(arguments.binary, os.X_OK), "The selected App binary is unavailable")
    require(re.fullmatch(r"[a-f0-9]{64}", arguments.expected_binary_sha256)
            and sha256(arguments.binary) == arguments.expected_binary_sha256, "The selected App build SHA-256 differs")
    require(re.fullmatch(r"[a-f0-9]{64}", arguments.source_sha256)
            and sha256(arguments.source_manifest) == arguments.source_sha256,
            "The frozen source-manifest SHA-256 differs from the build input declaration")
    output = arguments.output.resolve()
    require(output.is_relative_to(workspace) and output != workspace,
            "All synthetic guest state and evidence must stay inside the isolated remote workspace")
    require(arguments.case_timeout > arguments.startup_timeout > 0, "The case timeout must exceed the startup timeout")
    require(not arguments.interaction_races or any(name in ("pw5", "pw6") for name in arguments.profiles),
            "The optional loading-input fixtures require a selected PW profile")
    arguments.output = output
    arguments.output.mkdir(mode=0o700, parents=True, exist_ok=False)
    report = {
        "passed": False, "runtime_passed": False,
        "started_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        "coverage": "Real Rust/Slint App, normal offline guest cache, real ReaderCore raster, XTest pointer controls",
        "binary": str(arguments.binary), "binary_sha256": arguments.expected_binary_sha256,
        "source_manifest": str(arguments.source_manifest), "source_sha256": arguments.source_sha256,
        "harness_sha256": sha256(Path(__file__)),
        "helper_sha256": {name: sha256(Path(__file__).parent / name) for name in
                          ("reader_v2_runtime.py", "reader_guest_fixtures.py", "harness_signals.py")},
        "network_interfaces": ["lo"], "network_namespace": os.readlink("/proc/self/ns/net"),
        "demo": False, "live_account_used": False, "remote_transactions_performed": False,
        "physical_device": False, "firmware_emulator": False,
        "interaction_races_requested": arguments.interaction_races,
        "input_delivery": {"method": "Actual XTest motion and button events", "window_pid_checked": True,
                           "pre_down_event_loop_slot_ms": 80, "tap_hold_ms": 80,
                           "held_press_timing": "Optional loading-hold spans the real worker; exact duration is recorded per case",
                           "scripted_intents_used": False, "direct_view_model_setters_used": False},
        "cases": [], "visual_review": {"status": "pending", "required": True},
        "limitations": ["Synthetic offline guest results do not establish live-account, firmware, or hardware support.",
                        "Layout and wording in contextual menu screenshots still require human visual review.",
                        "Loading, error, guide, and episode-ended coverage remains in the existing dedicated harnesses."],
    }
    display, pointer = None, None
    try:
        display = PrivateDisplay(arguments.output)
        report["private_display"] = display.identity
        pointer = Pointer()
        for profile, landscape, mode, direction, spread, scenario in selected_cases(arguments):
            case = ReaderCase(arguments, pointer, profile, landscape, mode, direction, spread, scenario)
            try:
                case.start()
                case.drive()
                case.finish()
            except Exception as error:
                case.result.update({"error": str(error), "traceback": traceback.format_exc()})
                if case.window is not None and case.process is not None and case.process.poll() is None:
                    try:
                        case.snapshot("failure")
                    except Exception:
                        pass
            finally:
                case.close()
                report["cases"].append(case.result)
                write_json(arguments.output / "immersive-reader-result.json", report)
            print(json.dumps({"profile": profile, "case": case.name, "runtime_passed": case.result["runtime_passed"],
                              "error": case.result.get("error"), "directory": str(case.directory)}), flush=True)
        require(sha256(arguments.binary) == arguments.expected_binary_sha256,
                "The selected binary changed during the immersive runtime run")
        require(sha256(arguments.source_manifest) == arguments.source_sha256,
                "The frozen source manifest changed during the immersive runtime run")
        report["runtime_passed"] = bool(report["cases"]) and all(case["runtime_passed"] for case in report["cases"])
        report["passed"] = report["runtime_passed"]
        report["visual_review"]["artifacts"] = [item for case in report["cases"] for item in case["visual_review_required"]]
    except (Exception, KeyboardInterrupt) as error:
        report.update({"error": str(error), "traceback": traceback.format_exc()})
    finally:
        if pointer is not None:
            pointer.close()
        if display is not None:
            cleanup = display.close()
            if cleanup.get("owned_group_remained_after_parent_reaped"):
                report["passed"] = report["runtime_passed"] = False
                report.setdefault("error", "The private Xvfb server left an owned process group behind")
        report["finished_at"] = datetime.datetime.now(datetime.timezone.utc).isoformat()
        write_json(arguments.output / "immersive-reader-result.json", report)
    print(json.dumps({"runtime_passed": report["runtime_passed"],
                      "cases": len(report["cases"]), "result": str(arguments.output / "immersive-reader-result.json")}), flush=True)
    return 0 if report["runtime_passed"] else 1


if __name__ == "__main__":
    install_interruption_handlers()
    sys.exit(main())
