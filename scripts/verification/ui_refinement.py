#!/usr/bin/env python3
"""Exercise the approved header and reader refinement with real XTest input.

Run only on test-env against a freshly built desktop executable. The harness
creates its own Xvfb server, fresh private demo data, and evidence directory.
It does not inject named UI actions, contact a live account, or access a Kindle.
Screenshots are actual X11 window captures; typography and clipping still need
human review. No screenshot is presented as physical-device acceptance.
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
import sqlite3
import subprocess
import sys
import time
import traceback

from harness_signals import install_interruption_handlers
from pointer_v2 import CONTEXT_KEYS, INPUT_DELIVERY_PROTOCOL, Pointer


PROFILES = {
    "scribe": {
        "size": (930, 1240), "status": 32, "margin": 48,
        "filter": (854, 405), "filter_outside": (50, 200),
        "shelf_next": (830, 1124), "shelf_previous": (90, 1124),
        "nav_store": (349, 1201), "nav_downloads": (581, 1201),
        "nav_account": (814, 1201), "search_left": (52, 68),
        "search_right": (878, 68), "search_back": (100, 68),
        "download_pause": (684, 64), "download_select": (834, 64),
        "download_first": (465, 202), "exit": (854, 92),
        "center": (465, 620), "zoom_y": 1130, "tool_y": 1198,
        "choice_x": (152, 361, 569, 778),
        "directory_back": (118, 68), "directory_body": 224,
        "directory_row": 68, "directory_capacity": 14,
        "directory_filter_y": 146, "directory_filters": (92, 178, 266, 352),
        "directory_locate": (733, 146), "directory_sort": (844, 146),
        "directory_previous": (100, 1208), "directory_next": (830, 1208),
    },
    "pw6": {
        "size": (632, 840), "status": 28, "margin": 24,
        "filter": (572, 58), "filter_outside": (50, 500),
        "shelf_next": (584, 750), "shelf_previous": (492, 750),
        "nav_store": (237, 809), "nav_downloads": (395, 809),
        "nav_account": (553, 809), "search_left": (28, 56),
        "search_right": (604, 56), "search_back": (70, 54),
        "download_pause": (468, 54), "download_select": (574, 54),
        "download_first": (316, 170), "exit": (584, 74),
        "center": (316, 420), "zoom_y": 742, "tool_y": 802,
        "choice_x": (97, 243, 389, 535),
        "directory_back": (70, 60), "directory_body": 168,
        "directory_row": 60, "directory_capacity": 10,
        "directory_filter_y": 128, "directory_filters": (59, 128, 198, 267),
        "directory_locate": (490, 128), "directory_sort": (581, 128),
        "directory_previous": (70, 804), "directory_next": (562, 804),
    },
}
FIXTURE_COMIC = "28601"
FIXTURE_EPISODE = "2860112"
FIXTURE_PAGE_INDEX = 17
ANCHOR_KEYS = (
    "comic_id", "episode_id", "revision", "page_id", "page_index", "page_number",
    "source_x", "source_y", "zoom_ratio", "rotation", "mode", "fit",
    "reader_generation",
)


def require(value, message):
    if not value:
        raise AssertionError(message)


def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def write_json(path, value):
    path.write_text(json.dumps(value, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")


def trace_records(path):
    if not path.exists():
        return []
    records = []
    for line in path.read_text(encoding="utf-8").splitlines(keepends=True):
        if line.endswith("\n") and line.strip():
            records.append(json.loads(line))
    return records


def field(state, path):
    value = state
    for part in path.split("."):
        value = value.get(part) if isinstance(value, dict) else None
    return value


def matches(state, expected):
    return all(field(state, key) == value for key, value in (expected or {}).items())


def stop_owned_group(process, timeout=15):
    if process is None:
        return {"started": False}
    sent = False
    if process.poll() is None:
        try:
            os.killpg(process.pid, signal.SIGTERM)
            sent = True
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
    return {"started": True, "process_id": process.pid, "group_termination_sent": sent,
            "return_code": process.returncode, "owned_group_remained_after_parent_reaped": remaining}


class PrivateDisplay:
    def __init__(self, directory):
        self.process = None
        self.previous = os.environ.get("DISPLAY")
        self.log = (directory / "xvfb.log").open("w", encoding="utf-8")
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
        self.identity["cleanup"] = stop_owned_group(self.process)
        if self.previous is None:
            os.environ.pop("DISPLAY", None)
        else:
            os.environ["DISPLAY"] = self.previous
        if not self.log.closed:
            self.log.close()


class Session:
    def __init__(self, arguments, pointer, profile, name, screen):
        self.arguments, self.pointer, self.profile = arguments, pointer, profile
        self.name, self.screen, self.geometry = name, screen, PROFILES[profile]
        self.directory = arguments.output / (profile + "-" + name)
        self.directory.mkdir(mode=0o700, exist_ok=False)
        self.trace = self.directory / "lifecycle.jsonl"
        self.capture = self.directory / "final.png"
        self.data = self.directory / "data"
        self.database = self.data / "demo/state.sqlite3"
        self.process = None
        self.log = None
        self.window = None
        self.ui_record = None
        self.reader_record = None
        self.state = {}
        self.reader = {}
        self.result = {
            "name": name, "profile": profile, "passed": False, "runtime_passed": False,
            "binary_sha256": arguments.expected_sha256, "clicks": [], "observations": [],
            "screenshots": [], "visual_review_required": [],
            "input_delivery_protocol": INPUT_DELIVERY_PROTOCOL,
            "intent_delivery": "Actual XTest pointer input only; no named or scripted UI actions",
            "fixture": {"demo": True, "data": str(self.data), "live_account_used": False},
        }

    def records(self):
        return trace_records(self.trace)

    def wait(self, description, predicate, timeout=None):
        deadline = time.monotonic() + (timeout or self.arguments.step_timeout)
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

    def latest(self, event, after=0):
        return next((record for record in reversed(self.records()[after:])
                     if record.get("event") == event and record.get("process_id") == self.process.pid), None)

    def observe_ui(self, description, expected=None, predicate=None, after=0):
        def find():
            record = self.latest("ui-state", after)
            if not (record and record.get("demo") is True
                    and record.get("view_model_fixture_only") is False):
                return None
            state = record.get("state")
            if not isinstance(state, dict):
                return None
            return record if matches(state, expected) and (predicate is None or predicate(state)) else None
        self.ui_record = self.wait(description, find)
        self.state = self.ui_record["state"]
        self.result["observations"].append({"description": description, "kind": "live_ui",
                                            "expected": expected, "record": self.ui_record})
        return self.state

    def observe_reader(self, description, expected=None, predicate=None, after=0):
        def find():
            record = self.latest("reader-ready", after)
            if not record or record.get("route") != "reader" or record.get("reader_loading") is not False:
                return None
            state, frame = record.get("reader_state"), record.get("frame_size")
            if not (isinstance(state, dict) and isinstance(frame, list) and len(frame) == 2
                    and min(frame) > 0 and state.get("frame_width", 0) > 0
                    and state.get("frame_height", 0) > 0 and state.get("rendering") is False):
                return None
            return record if matches(state, expected) and (predicate is None or predicate(state)) else None
        self.reader_record = self.wait(description, find)
        self.reader = self.reader_record["reader_state"]
        require(self.reader.get("comic_id") == FIXTURE_COMIC
                and self.reader.get("episode_id") == FIXTURE_EPISODE,
                "The actual reader identity differs from the frozen canonical demo fixture")
        require(self.reader.get("page_index") == FIXTURE_PAGE_INDEX,
                "A reader control unexpectedly changed the source page")
        self.result["observations"].append({"description": description, "kind": "ready_reader",
                                            "expected": expected, "record": self.reader_record})
        return self.reader

    def start(self):
        require(sha256(self.arguments.binary) == self.arguments.expected_sha256,
                "The selected executable changed before session launch")
        environment = os.environ.copy()
        for key in CONTEXT_KEYS:
            environment.pop(key, None)
        environment["SLINT_SCALE_FACTOR"] = "1"
        command = [str(self.arguments.binary), "--demo", "--device", self.profile, "--screen", self.screen,
                   "--data-dir", str(self.data), "--lifecycle-trace", str(self.trace),
                   "--capture", str(self.capture), "--exit-after-ms", "300000"]
        self.result["command"] = command
        self.log = (self.directory / "application.log").open("w", encoding="utf-8")
        self.process = subprocess.Popen(command, env=environment, stdout=self.log,
                                        stderr=subprocess.STDOUT, start_new_session=True)
        self.result["process_id"] = self.process.pid
        ready = self.wait("the same-process App ready event", lambda: self.latest("ready"),
                          timeout=self.arguments.startup_timeout)
        self.result["ready_record"] = ready
        self.observe_ui("initial live UI after App readiness", {"route": self.screen})
        if self.screen == "reader":
            self.observe_reader("initial source page 18 with a completed frame",
                                {"page_index": FIXTURE_PAGE_INDEX, "fit": "page", "presented_fit": "page",
                                 "menu": False, "sheet": ""})
        self.window = self.wait("the mapped private application window", self.pointer.window)
        require((self.window["width"], self.window["height"]) == self.geometry["size"],
                "The actual window differs from the frozen logical profile")
        self.result["window"] = self.window
        self.result["readiness_gate"] = "Matching PID ready and live ui-state; reader cases also require a completed real source frame"
        self.snapshot("initial")

    def snapshot(self, name):
        time.sleep(0.22)
        value = self.pointer.capture(self.window, self.directory / (name + ".png"))
        value.update({"name": name, "dimensions": list(self.geometry["size"]),
                      "process_id": self.process.pid, "binary_sha256": self.arguments.expected_sha256})
        metadata = {"name": name, "captured_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
                    "profile": self.profile, "window": self.window, "screenshot": value,
                    "ui_record": self.ui_record, "reader_record": self.reader_record,
                    "scope": "Actual private desktop-software demo application window"}
        sidecar = self.directory / (name + ".json")
        write_json(sidecar, metadata)
        value["metadata"] = str(sidecar)
        self.result["screenshots"].append(value)
        return value

    def visual_review(self, name, checks):
        image = next(row for row in self.result["screenshots"] if row["name"] == name)
        self.result["visual_review_required"].append({"status": "pending", "artifact": image["path"],
                                                       "checks": checks})

    def click(self, name, point):
        require(0 <= point[0] < self.window["width"] and 0 <= point[1] < self.window["height"],
                "The planned pointer coordinate is outside the actual application client area")
        prior = len(self.records())
        self.pointer.click(self.window, point)
        self.result["clicks"].append({"name": name, "client": list(point), "window_id": self.window["id"],
                                     "monotonic_seconds": time.monotonic(), "trace_records_before": prior})
        return prior

    def click_ui(self, name, point, expected=None, predicate=None):
        prior = self.click(name, point)
        self.observe_ui(name, expected, predicate, prior)
        self.snapshot(name)
        return self.state

    def click_reader(self, name, point, expected, ui_expected=None, predicate=None):
        prior = self.click(name, point)
        ui_expected = {"route": "reader", "reader.loading": False, "reader.page": 18,
                       **(ui_expected or {})}
        self.observe_ui(name + " live UI", ui_expected, after=prior)
        self.observe_reader(name + " completed source frame", expected, predicate, prior)
        self.snapshot(name)
        return self.reader

    def preference(self, key):
        with sqlite3.connect(self.database.as_uri() + "?mode=ro", uri=True, timeout=5) as connection:
            row = connection.execute("SELECT payload FROM preferences WHERE account_key=? AND key=?",
                                     ("design-demo", key)).fetchone()
        return json.loads(row[0]) if row else None

    def assert_saved_fit(self, fit):
        def saved():
            defaults = self.preference("preferences")
            comic = self.preference("reader_preferences_" + FIXTURE_COMIC)
            return {"defaults": defaults, "comic": comic} if defaults and comic and (
                defaults.get("fit") == fit and comic.get("fit") == fit) else None
        proof = self.wait("durable default and per-comic fit " + fit, saved)
        self.result.setdefault("saved_fit_evidence", []).append({"fit": fit, "database": str(self.database),
                                                                 "access": "Read only", "preferences": proof})

    def finish(self, exited_by_x=False):
        if not exited_by_x:
            require(self.process.poll() is None, "The application exited before graceful final capture")
            os.kill(self.process.pid, signal.SIGTERM)
        require(self.process.wait(timeout=20) == 0, "The application did not exit cleanly")
        self.log.close()
        metadata = json.loads(self.capture.with_suffix(".json").read_text(encoding="utf-8"))
        require(metadata.get("process_id") == self.process.pid and metadata.get("demo") is True
                and metadata.get("profile") == self.profile and metadata.get("backend") == "desktop-software"
                and metadata.get("startup", {}).get("booting") is False
                and metadata.get("startup", {}).get("error") is False
                and metadata.get("network_prompt") is False
                and metadata.get("snapshot_size") == list(self.geometry["size"]),
                "The final capture is not the original healthy private desktop demo application")
        require(metadata.get("route") == field(self.state, "route"),
                "The final capture route differs from the latest observed pointer result")
        if self.screen == "reader":
            require(metadata.get("reader", {}).get("page") == 18
                    and metadata.get("reader", {}).get("fit") == "page"
                    and metadata.get("reader", {}).get("loading") is False,
                    "The final reader capture lost source page 18 or its selected whole-page fit")
        self.result.update({"passed": True, "runtime_passed": True, "final_metadata": metadata,
                            "final_capture": str(self.capture), "final_capture_sha256": sha256(self.capture),
                            "final_ui_state": self.state,
                            "exited_by_actual_x_click": exited_by_x})

    def close(self):
        self.result["cleanup"] = stop_owned_group(self.process)
        if self.result["cleanup"].get("owned_group_remained_after_parent_reaped"):
            self.result["passed"] = False
            self.result["runtime_passed"] = False
            self.result.setdefault("error", "The application left an owned child process behind")
        if self.log is not None and not self.log.closed:
            self.log.close()
        self.result["trace_path"] = str(self.trace)
        write_json(self.directory / "result.json", self.result)


def tab_controls(case):
    g = case.geometry
    require(field(case.state, "catalog.shelf_view") == "grid"
            and field(case.state, "catalog.shelf_page") == 1,
            "The fresh shelf did not use its expected grid and first-page state")
    case.visual_review("initial", ["No repeated primary-tab heading", "The content filter belongs to the shelf content",
                                   "The book grid ends above the separate bottom pager"])
    case.click_ui("shelf-content-filter", g["filter"], {"catalog.shelf_menu": True})
    case.click_ui("close-shelf-content-filter", g["filter_outside"], {"catalog.shelf_menu": False})
    require(field(case.state, "catalog.shelf_pages") > 1, "The canonical shelf needs multiple pages for pager coverage")
    case.click_ui("shelf-bottom-next", g["shelf_next"], {"catalog.shelf_page": 2})
    case.visual_review("shelf-bottom-next", ["Bottom pagination remains visible and the grid does not overlap its hit area"])
    case.click_ui("shelf-bottom-previous", g["shelf_previous"], {"catalog.shelf_page": 1})
    case.click_ui("store-tab", g["nav_store"], {"route": "store"})
    case.visual_review("store-tab", ["The search field spans the header content width", "There is no repeated store title"])
    case.click_ui("store-search-left-edge", g["search_left"], {"route": "search", "catalog.keyboard": True})
    case.click_ui("search-back-to-store", g["search_back"], {"route": "store"})
    case.click_ui("store-search-right-edge", g["search_right"], {"route": "search", "catalog.keyboard": True})
    case.click_ui("search-back-again", g["search_back"], {"route": "store"})
    case.click_ui("downloads-tab", g["nav_downloads"], {"route": "downloads", "downloads.selecting": False},
                  lambda state: bool(field(state, "downloads.status_summary")) and bool(field(state, "downloads.summary")))
    case.visual_review("downloads-tab", ["Task status appears in the first header row", "Storage information appears on the next row",
                                         "There is no repeated downloads title"])
    status_before = field(case.state, "downloads.status_summary")
    require(field(case.state, "downloads.has_active") is True,
            "The canonical fixture has no active task for the dynamic header test")
    case.click_ui("pause-all-from-status-header", g["download_pause"], {"downloads.all_paused": True},
                  lambda state: bool(field(state, "downloads.status_summary"))
                  and field(state, "downloads.status_summary") != status_before)
    status_paused = field(case.state, "downloads.status_summary")
    case.click_ui("resume-all-from-status-header", g["download_pause"], {"downloads.all_paused": False},
                  lambda state: field(state, "downloads.status_summary") == status_before)
    case.result["dynamic_download_header"] = {"active": status_before, "paused": status_paused}
    case.click_ui("download-select", g["download_select"], {"downloads.selecting": True, "downloads.selected_count": 0})
    case.click_ui("select-first-download", g["download_first"], {"downloads.selecting": True, "downloads.selected_count": 1})
    case.click_ui("cancel-download-selection", g["download_select"], {"downloads.selecting": False, "downloads.selected_count": 0})
    case.click_ui("account-tab", g["nav_account"], {"route": "account", "account.section": "account"})
    case.visual_review("account-tab", ["Profile and wallet have priority", "The exit control is a borderless X with a usable hit area",
                                       "Logout remains a separate account row", "There is no repeated account title"])
    case.click("account-borderless-x-exit", g["exit"])
    case.finish(exited_by_x=True)


def assert_anchor(case, expected):
    observed = {key: case.reader.get(key) for key in ANCHOR_KEYS}
    require(observed == expected, "A directory or secondary control changed the live reading anchor or reader generation")
    case.result.setdefault("anchor_invariance", []).append({"expected": expected, "observed": observed})


def directory_rows(case, filter_key="all", descending=False):
    rows = field(case.state, "reader.chapter_rows")
    capacity = field(case.state, "reader.chapter_capacity")
    require(isinstance(rows, list) and capacity == case.geometry["directory_capacity"] and len(rows) <= capacity,
            "The full-screen directory has an invalid capacity or too many visible rows")
    require(field(case.state, "reader.chapter_filter") == filter_key
            and field(case.state, "reader.chapter_descending") is descending,
            "Directory filter or ordering differs from the selected control")
    values = [float(row["order"]) for row in rows]
    require(values == sorted(values, reverse=descending), "The visible directory rows are out of numeric chapter order")
    if filter_key == "unread":
        require(all(row.get("read") is False and row.get("current") is False for row in rows),
                "The unread directory contains a read or current chapter")
    elif filter_key == "readable":
        require(all(row.get("readable") is True for row in rows), "The readable directory contains an inaccessible chapter")
    elif filter_key == "downloaded":
        require(all(row.get("downloaded") is True for row in rows), "The downloaded directory contains an unretained chapter")
    require(field(case.state, "reader.thumbnail_count") == 0
            and field(case.state, "reader.thumbnails_loading") is False,
            "The directory unexpectedly started the retired page-overview workload")
    return rows


def reader_controls(case):
    g = case.geometry
    case.click_reader("open-reading-controls", g["center"], {"menu": True, "sheet": ""},
                      {"reader.menu": True, "reader.sheet": ""})
    case.visual_review("open-reading-controls", ["The reader header has one title row and no network or cache caption",
                                                 "The bottom control area has progress, four fit choices, and exactly four tools",
                                                 "No mode caption or page-overview tool is present"])
    for index, fit in enumerate(("original", "page", "height", "width", "page")):
        fit_index = ("original", "page", "height", "width").index(fit)
        name = "fit-" + fit + ("-restored" if index == 4 else "")
        case.click_reader(name, (g["choice_x"][fit_index], g["zoom_y"]),
                          {"fit": fit, "presented_fit": fit, "target_fit": fit,
                           "page_index": FIXTURE_PAGE_INDEX, "menu": True, "sheet": ""},
                          {"reader.fit": fit, "reader.menu": True, "reader.sheet": ""})
        case.assert_saved_fit(fit)
        case.visual_review(name, ["The selected fit is visibly highlighted",
                                 "Page fills both viewport axes and may change the artwork aspect ratio" if fit == "page"
                                 else "Artwork preserves its aspect ratio and is centered"])
    anchor = {key: case.reader.get(key) for key in ANCHOR_KEYS}

    def open_directory(name):
        case.click_reader(name, (g["choice_x"][0], g["tool_y"]), {"menu": True, "sheet": "toc"},
                          {"reader.menu": True, "reader.sheet": "toc",
                           "reader.chapter_capacity": g["directory_capacity"]})
        rows = directory_rows(case)
        require(field(case.state, "reader.chapter_count") == 32 and any(row["current"] for row in rows),
                "The full directory did not open on the canonical current chapter")
        assert_anchor(case, anchor)
        case.visual_review(name, ["Directory covers the complete viewport", "No comic frame or intermediate half-height sheet is visible",
                                  "Chapter rows, access, progress, and pagination fit without clipping"])
        return rows

    rows = open_directory("open-full-chapter-directory")
    current_index = next(index for index, row in enumerate(rows) if row["current"])
    require(rows[current_index]["order"] == "12", "The highlighted current chapter differs from the canonical fixture")
    point = (g["size"][0] // 2, round(g["directory_body"] + (current_index + 0.5) * g["directory_row"]))
    case.click_reader("current-chapter-returns-to-page-18", point, {"menu": False, "sheet": ""},
                      {"reader.menu": False, "reader.sheet": ""})
    assert_anchor(case, anchor)
    case.click_reader("reopen-reading-controls", g["center"], {"menu": True, "sheet": ""},
                      {"reader.menu": True, "reader.sheet": ""})
    open_directory("reopen-complete-directory")

    while field(case.state, "reader.chapter_page") > 1:
        page = field(case.state, "reader.chapter_page") - 1
        case.click_reader("directory-back-to-page-" + str(page), g["directory_previous"], {"sheet": "toc"},
                          {"reader.sheet": "toc", "reader.chapter_page": page})
    full_rows = list(directory_rows(case))
    while field(case.state, "reader.chapter_page") < field(case.state, "reader.chapter_pages"):
        page = field(case.state, "reader.chapter_page") + 1
        case.click_reader("directory-forward-to-page-" + str(page), g["directory_next"], {"sheet": "toc"},
                          {"reader.sheet": "toc", "reader.chapter_page": page})
        full_rows.extend(directory_rows(case))
        assert_anchor(case, anchor)
    full_orders = [row["order"] for row in full_rows]
    require(full_orders == [str(index) for index in range(1, 33)],
            "The complete directory pager did not expose every fixture chapter exactly once")
    case.result["complete_directory_orders"] = full_orders
    case.result["complete_directory_flags"] = full_rows
    page = field(case.state, "reader.chapter_page") - 1
    case.click_reader("directory-previous-from-last-page", g["directory_previous"], {"sheet": "toc"},
                      {"reader.sheet": "toc", "reader.chapter_page": page})
    case.click_reader("directory-locate-current", g["directory_locate"], {"sheet": "toc"},
                      {"reader.sheet": "toc", "reader.chapter_filter": "all", "reader.chapter_descending": False})
    require(any(row["current"] for row in directory_rows(case)), "Locate did not expose the current chapter")
    case.click_reader("directory-descending", g["directory_sort"], {"sheet": "toc"},
                      {"reader.chapter_descending": True, "reader.chapter_page": 1})
    rows = directory_rows(case, descending=True)
    require([row["order"] for row in rows] == list(reversed(full_orders))[:g["directory_capacity"]],
            "Descending order did not expose the expected final chapters on the first page")
    case.click_reader("directory-ascending", g["directory_sort"], {"sheet": "toc"},
                      {"reader.chapter_descending": False, "reader.chapter_page": 1})
    rows = directory_rows(case)
    require([row["order"] for row in rows] == full_orders[:g["directory_capacity"]],
            "Ascending order did not restore the expected first chapters")
    for index, key in enumerate(("unread", "readable", "downloaded"), start=1):
        case.click_reader("directory-filter-" + key, (g["directory_filters"][index], g["directory_filter_y"]),
                          {"sheet": "toc"}, {"reader.sheet": "toc", "reader.chapter_filter": key,
                                               "reader.chapter_page": 1})
        rows = directory_rows(case, key)
        expected = [row for row in full_rows if (
            not row["read"] and not row["current"] if key == "unread" else row[key])]
        require(expected and rows, "The nonempty canonical fixture filter unexpectedly returned no chapters")
        require([row["order"] for row in rows] == [row["order"] for row in expected[:g["directory_capacity"]]]
                and field(case.state, "reader.chapter_pages") == max(
                    1, (len(expected) + g["directory_capacity"] - 1) // g["directory_capacity"]),
                "The directory filter does not match the complete fixture chapter set and page count")
        assert_anchor(case, anchor)
    case.click_reader("directory-locate-resets-filter", g["directory_locate"], {"sheet": "toc"},
                      {"reader.sheet": "toc", "reader.chapter_filter": "all", "reader.chapter_descending": False})
    require(any(row["current"] for row in directory_rows(case)), "Locate after filtering lost the current chapter")
    case.click_reader("directory-back-preserves-page-18", g["directory_back"], {"menu": False, "sheet": ""},
                      {"reader.menu": False, "reader.sheet": ""})
    assert_anchor(case, anchor)

    for name, tool_index, sheet in (("reading-settings", 1, "display"), ("frontlight", 3, "light")):
        case.click_reader("controls-before-" + name, g["center"], {"menu": True, "sheet": ""},
                          {"reader.menu": True, "reader.sheet": ""})
        case.click_reader(name + "-tool", (g["choice_x"][tool_index], g["tool_y"]),
                          {"menu": True, "sheet": sheet}, {"reader.menu": True, "reader.sheet": sheet})
        case.click_reader("close-" + name, (g["size"][0] // 2, 160), {"menu": False, "sheet": ""},
                          {"reader.menu": False, "reader.sheet": ""})
        assert_anchor(case, anchor)
    case.click_reader("controls-before-full-refresh", g["center"], {"menu": True, "sheet": ""},
                      {"reader.menu": True, "reader.sheet": ""})
    case.click_reader("full-refresh-tool", (g["choice_x"][2], g["tool_y"]), {"menu": False, "sheet": ""},
                      {"reader.menu": False, "reader.sheet": ""})
    assert_anchor(case, anchor)
    case.assert_saved_fit("page")
    case.finish()


def main():
    require(sys.platform.startswith("linux"), "This verification harness runs only on remote Linux test-env")
    os.umask(0o077)
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--expected-sha256", required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--profiles", nargs="+", choices=list(PROFILES), default=list(PROFILES))
    parser.add_argument("--cases", nargs="+", choices=["tabs", "reader"], default=["tabs", "reader"])
    parser.add_argument("--startup-timeout", type=float, default=90)
    parser.add_argument("--step-timeout", type=float, default=30)
    arguments = parser.parse_args()
    require(re.fullmatch(r"[0-9a-f]{64}", arguments.expected_sha256), "Supply the exact lowercase executable SHA-256")
    arguments.binary = arguments.binary.resolve(strict=True)
    require(arguments.binary.is_file() and os.access(arguments.binary, os.X_OK), "The selected desktop executable is unavailable")
    require(sha256(arguments.binary) == arguments.expected_sha256, "The executable does not match the selected build SHA-256")
    require(arguments.output.is_absolute(), "Supply an absolute fresh evidence output directory")
    arguments.output = arguments.output.resolve()
    arguments.output.mkdir(mode=0o700, parents=True, exist_ok=False)
    manifest_path = arguments.output / "ui-refinement-result.json"
    manifest = {
        "suite": "header_reader_refinement_20261113", "passed": False, "runtime_passed": False,
        "started_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        "binary": str(arguments.binary), "binary_sha256": arguments.expected_sha256,
        "harness": str(Path(__file__).resolve()), "harness_sha256": sha256(Path(__file__)),
        "pointer_dependency": str(Path(__file__).with_name("pointer_v2.py").resolve()),
        "pointer_dependency_sha256": sha256(Path(__file__).with_name("pointer_v2.py")),
        "profiles": arguments.profiles, "cases": [], "live_account_used": False,
        "named_ui_actions_used": False, "physical_device_used": False, "firmware_emulator_used": False,
        "coverage": "Real Rust desktop-software application and XTest controls with isolated bundled demo fixtures",
        "input_delivery_protocol": INPUT_DELIVERY_PROTOCOL,
        "visual_review_status": "pending",
        "limitations": ["Screenshots require human visual review for typography, header hierarchy, and clipping",
                        "No physical Kindle or firmware-emulator acceptance is established",
                        "Demo logout is a preview-only toast and does not establish signed-out layout acceptance",
                        "Sleep/wake and restart acceptance are covered by separate lifecycle verification"],
    }
    display = None
    pointer = None
    write_json(manifest_path, manifest)
    try:
        display = PrivateDisplay(arguments.output)
        manifest["display"] = display.identity
        pointer = Pointer()
        for profile in arguments.profiles:
            for name in arguments.cases:
                case = Session(arguments, pointer, profile, name, "shelf" if name == "tabs" else "reader")
                try:
                    case.start()
                    (tab_controls if name == "tabs" else reader_controls)(case)
                except Exception as error:
                    case.result.update({"passed": False, "runtime_passed": False,
                                        "error": str(error), "traceback": traceback.format_exc()})
                    if case.window is not None and case.process is not None and case.process.poll() is None:
                        try:
                            case.snapshot("failure")
                        except Exception as capture_error:
                            case.result["failure_capture_error"] = str(capture_error)
                finally:
                    case.close()
                    manifest["cases"].append(case.result)
                    write_json(manifest_path, manifest)
                print(json.dumps({"profile": profile, "case": name, "passed": case.result["passed"],
                                  "directory": str(case.directory), "error": case.result.get("error")}), flush=True)
        require(sha256(arguments.binary) == arguments.expected_sha256, "The selected executable changed during verification")
        manifest["passed"] = bool(manifest["cases"]) and all(case["passed"] for case in manifest["cases"])
        manifest["runtime_passed"] = manifest["passed"]
    except BaseException as error:
        manifest.update({"passed": False, "runtime_passed": False, "error": str(error), "traceback": traceback.format_exc()})
        if isinstance(error, KeyboardInterrupt):
            manifest["interrupted"] = True
    finally:
        if pointer is not None:
            pointer.close()
        if display is not None:
            display.close()
            manifest["display"] = display.identity
        manifest["finished_at"] = datetime.datetime.now(datetime.timezone.utc).isoformat()
        write_json(manifest_path, manifest)
    return 0 if manifest["passed"] else 1


if __name__ == "__main__":
    install_interruption_handlers()
    sys.exit(main())
