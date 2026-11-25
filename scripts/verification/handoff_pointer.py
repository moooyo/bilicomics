#!/usr/bin/env python3
"""Exercise new controls in actual Xvfb windows using XTest, not scripted intents."""

import argparse
import json
import os
from pathlib import Path
import signal
import sqlite3
import subprocess
import time

from PIL import ImageGrab
from handoff_scenes import PROFILES, verdict_value
from pointer_v2 import Pointer
from ui_refinement import stop_owned_group, trace_records


class Window:
    def __init__(self, args, profile, route, name=None):
        self.profile = profile
        self.route = route
        self.raw = args.output / "raw" / profile
        self.raw.mkdir(parents=True, exist_ok=True)
        self.work = args.output / "work" / profile / (name or route)
        self.work.mkdir(parents=True)
        self.trace = self.work / "ui.jsonl"
        self.pointer = Pointer()
        self.process = None
        self.log = (self.work / "application.log").open("w")
        self.env = {key: value for key, value in os.environ.items() if not key.startswith(("BILICOMICS_", "SLINT_"))}
        self.env.update(SLINT_BACKEND="winit-software", SLINT_SCALE_FACTOR="2")
        self.command = [str(args.binary), "--demo", "--device", profile, "--screen", route,
                        "--data-dir", str(self.work / "data"), "--lifecycle-trace", str(self.trace),
                        "--exit-after-ms", "120000"]

    def __enter__(self):
        self.process = subprocess.Popen(self.command, env=self.env, stdout=self.log, stderr=self.log,
                                        stdin=subprocess.DEVNULL, start_new_session=True)
        try:
            self.window = self.wait("mapped window", self.pointer.window)
            assert (self.window["width"], self.window["height"]) == PROFILES[self.profile]
            self.width, self.height = (value // 2 for value in PROFILES[self.profile])
            self.state(route=self.route)
        except BaseException:
            self.__exit__(None, None, None)
            raise
        return self

    def wait(self, label, condition, timeout=16):
        end = time.monotonic() + timeout
        while time.monotonic() < end:
            if self.process.poll() is not None:
                raise RuntimeError(f"Application exited waiting for {label}: {self.process.returncode}")
            observed = condition()
            if observed:
                return observed
            time.sleep(0.06)
        raise RuntimeError(f"{self.profile}: timeout waiting for {label}: {self.latest()}")

    def latest(self):
        return next((record["state"] for record in reversed(trace_records(self.trace))
                     if record.get("event") == "ui-state" and record.get("process_id") == self.process.pid), {})

    def state(self, **expected):
        def checked():
            state = self.latest()
            return state if state and all(verdict_value(state, key) == value for key, value in expected.items()) else None
        return self.wait(expected, checked)

    def click(self, x, y, **expected):
        self.pointer.click(self.window, (round(x * 2), round(y * 2)))
        if expected:
            self.state(**expected)
        else:
            time.sleep(0.15)

    def grab(self):
        box = self.window
        return ImageGrab.grab(bbox=(box["x"], box["y"], box["x"] + box["width"], box["y"] + box["height"]))

    def reader_frame(self, **expected):
        def checked():
            reader = next((row.get("reader_state") for row in reversed(trace_records(self.trace))
                           if row.get("event") == "reader-ready" and row.get("process_id") == self.process.pid), None)
            return reader if reader and all(reader.get(key) == value for key, value in expected.items()) else None
        return self.wait(expected, checked)

    def assert_quiet_binary(self, *, different_from=None):
        self.state(route="reader", **{"reader.menu": False, "reader.sheet": "",
                                      "reader.presented_fast_mode": True, "reader.transition": False})
        self.reader_frame(reader_refresh_scene="fast_content", presented_dither_algorithm="binary_bayer4_v1")

        previous_pixels = different_from.tobytes() if different_from else None

        def binary_sample():
            image = self.grab().convert("RGB")
            colors = image.getcolors(maxcolors=2)
            if colors and {color for _, color in colors} == {(0, 0, 0), (255, 255, 255)}:
                if previous_pixels is None or image.tobytes() != previous_pixels:
                    return image
            return None

        first = self.wait("actual binary reader pixels", binary_sample)
        for _ in range(3):
            time.sleep(0.54)
            assert self.grab().convert("RGB").tobytes() == first.tobytes(), "Idle Fast frame changed"
        return first

    def capture(self, name):
        time.sleep(0.5)
        previous = self.grab()
        for _ in range(12):
            time.sleep(0.18)
            image = self.grab()
            if image.tobytes() == previous.tobytes():
                target = self.raw / (name + ".png")
                with target.open("xb") as handle:
                    image.save(handle, format="PNG")
                target.with_suffix(".json").write_text(json.dumps({"profile": self.profile, "scope": "XTest pointer", "state": self.latest()}, ensure_ascii=False, indent=2) + "\n")
                print(f"{self.profile}/{name}: captured after pointer input", flush=True)
                return
            previous = image
        raise RuntimeError(f"Pixels did not settle: {self.profile}/{name}")

    def __exit__(self, *_):
        if self.process is not None and self.process.poll() is None:
            self.process.send_signal(signal.SIGTERM)
        stop_owned_group(self.process)
        self.log.close()
        self.pointer.close()


def reading(args, profile):
    wide = profile == "scribe"
    with Window(args, profile, "shelf") as window:
        width, height = window.width, window.height
        window.click(100 if wide else 36, 130 if wide else 64, route="reader", **{"reader.loading": False, "reader.page": 18})
        window.click(width / 2, height / 2, **{"reader.menu": True, "reader.sheet": ""})
        window.click(width / 8, height - 42, **{"reader.sheet": "toc", "reader.page": 18})
        window.capture("directory-pointer")
        window.click(45, 55, **{"reader.sheet": "", "reader.menu": False, "reader.page": 18})
        window.click(width / 2, height / 2, **{"reader.menu": True})
        window.click(width * 3 / 8, height - 42, **{"reader.sheet": "display"})
        if wide:
            window.capture("reader-settings")
        else:
            window.capture("reader-layout")
            window.click(96, height - 376 + 76)
            window.capture("reader-display-normal")
            window.click(500, height - 376 + 136, **{"reader.refresh_mode": "fast"})
            window.capture("reader-display")
            window.click(246, height - 376 + 256)
            window.state(**{"reader.dither_method": "bayer8", "reader.dither": True})
            window.click(230, height - 376 + 136, **{"reader.refresh_mode": "normal"})
        window.click(width - (92 if wide else 46), height - (610 if wide else 376) + 29,
                     **{"reader.sheet": "", "reader.menu": True})
        window.click(width * 7 / 8, height - 42, **{"reader.sheet": "light"})
        window.capture("reader-light-pointer")
        window.click(width / 2, height / 4, **{"reader.sheet": "", "reader.menu": False})
        window.click(width / 2, height / 2, **{"reader.menu": True})
        window.click(44, 52, route="shelf")


def display_preferences(args, profile):
    wide = profile == "scribe"
    with Window(args, profile, "shelf", "display-preferences") as window:
        width, height = window.width, window.height
        mode_y, clear_y, dither_y = (height - 420, height - 317, height - 214) if wide else (
            height - 240, height - 180, height - 120)
        mode_normal, mode_fast = (574, 794) if wide else (253, 493)
        dither_off, dither_floyd = (540, 816) if wide else (212, 532)

        def open_display():
            window.click(width / 2, height / 2, **{"reader.menu": True, "reader.sheet": ""})
            window.click(width * 3 / 8, height - 42, **{"reader.sheet": "display"})
            if not wide:
                window.click(96, height - 376 + 76)

        def close_display():
            window.click(width - (92 if wide else 46), height - (610 if wide else 376) + 29,
                         **{"reader.sheet": "", "reader.menu": True})
            window.click(width / 2, height / 4, **{"reader.sheet": "", "reader.menu": False})

        window.click(100 if wide else 36, 130 if wide else 64, route="reader", **{"reader.page": 18})
        window.reader_frame(page_number=18, presented_dither_algorithm="bayer8_v1", reader_refresh_scene="normal")
        open_display()
        window.click(dither_off, dither_y, **{"reader.dither": False})
        window.reader_frame(presented_dither_algorithm="off", page_number=18)
        window.click(dither_floyd, dither_y, **{"reader.dither": True, "reader.dither_method": "floyd_steinberg"})
        window.reader_frame(presented_dither_algorithm="floyd_steinberg_q8_v1", page_number=18)
        window.click(mode_fast, mode_y, **{"reader.refresh_mode": "fast", "reader.presented_fast_mode": True})
        window.reader_frame(reader_refresh_scene="fast_overlay", hardware_y4_requested=False)
        window.click(dither_off, dither_y)
        window.state(**{"reader.dither": True, "reader.dither_method": "floyd_steinberg"})
        window.click(mode_fast, clear_y)
        close_display()
        fast = window.assert_quiet_binary()
        window.click(width / 2, height / 2, **{"reader.menu": True})
        window.click(width * 5 / 8, height - 42, **{"reader.menu": False, "reader.sheet": ""})
        assert window.assert_quiet_binary().tobytes() == fast.tobytes(), "Manual clearing changed the Fast page"
        window.click(width * 5 / 6, height / 2, **{"reader.page": 19, "reader.loading": False})
        window.reader_frame(page_number=19)
        window.assert_quiet_binary(different_from=fast)
        open_display()
        window.click(mode_normal, mode_y, **{"reader.refresh_mode": "normal", "reader.presented_fast_mode": False,
                                            "reader.dither": True, "reader.dither_method": "floyd_steinberg",
                                            "reader.presented_cpu_dither": True, "reader.page": 19})
        window.reader_frame(presented_dither_algorithm="floyd_steinberg_q8_v1", reader_refresh_scene="normal",
                            hardware_y4_requested=False, page_number=19)
        close_display()
        database = (window.work / "data/demo/state.sqlite3").as_uri()
        connection = sqlite3.connect(database + "?mode=ro", uri=True)
        try:
            for key in ("preferences", "reader_preferences_28601"):
                payload, = connection.execute("SELECT payload FROM preferences WHERE account_key = ? AND key = ?",
                                              ("design-demo", key)).fetchone()
                saved = json.loads(payload)
                assert saved["every"] == "smart" and saved["refresh_mode"] == "normal"
                assert saved["dither"] and saved["dither_method"] == "floyd_steinberg"
        finally:
            connection.close()
        print(f"{profile}: Fast stays binary through idle, manual clearing and a page turn; Normal restores its saved choices", flush=True)


def account_settings(args, profile):
    wide = profile == "scribe"
    with Window(args, profile, "account") as window:
        if not wide:
            window.click(126, 204, **{"account.section": "reading"})
        window.click(680 if wide else 500, 340 if wide else 223, **{"account.dialog": "picker"})
        window.capture("acct-picker")
        window.click(window.width / 2, 495 if wide else 322, **{"account.dialog": "", "account.fit": "original"})
        if wide:
            window.click(122, 340, **{"account.section": "download"})
            window.click(690, 412, **{"account.dialog": "picker"})
            window.capture("acct-picker-conc")
            window.click(35, 535, **{"account.dialog": ""})
            window.click(120, 554, **{"account.section": "account"})
        else:
            window.click(44, 52, **{"account.section": "home"})
            window.click(123, 269, **{"account.section": "display"})
            window.click(500, 223, **{"account.dialog": "picker"})
            window.capture("acct-picker-rpol")
            window.click(601, 790, **{"account.dialog": ""})
            window.click(44, 52, **{"account.section": "home"})
            window.click(125, 534, **{"account.section": "account"})
        window.click(420 if wide else 98, 479 if wide else 355, **{"account.dialog": "signout"})
        window.capture("account-signout")


def download_panel(args, profile):
    wide = profile == "scribe"
    with Window(args, profile, "downloads") as window:
        window.click(window.width / 2, 218 if wide else 170, **{"downloads.sheet_open": True})
        window.capture("downloads-sheet-pointer")
        window.click(window.width / 2, 120, **{"downloads.sheet_open": False})
        window.click(844 if wide else 590, 67 if wide else 55, **{"downloads.selecting": True})
        window.click(window.width / 2, 218 if wide else 170, **{"downloads.selected_count": 1})
        window.capture("downloads-selection-pointer")


def catalog_and_purchase(args, profile):
    wide = profile == "scribe"
    with Window(args, profile, "shelf", "catalog-and-purchase") as window:
        window.click(830 if wide else 576, 366 if wide else 136, **{"catalog.shelf_menu": True})
        window.capture("shelf-filter-pointer")
        window.click(140, 490 if wide else 280, **{"catalog.shelf_menu": False})
        window.click(580 if wide else 170, 266 if wide else 63, route="detail", **{"catalog.detail_id": "28601"})
        window.click(470 if wide else 370, 1042 if wide else 311, route="purchase", **{"purchase.phase": "review", "purchase.included": 1})
        window.click(window.width / 2, 362 if wide else 243, **{"purchase.included": 5})
        window.capture("purchase-range-pointer")
        window.click(52, 61, route="detail")
        window.click(45, 52, route="shelf")
        window.click(window.width * 3 / 8, window.height - 24, route="store")
        window.click(125, 58, route="search", **{"catalog.keyboard": True})
        window.click(134, 275 if wide else 230, **{"catalog.keyboard": False})
        window.capture("search-history-pointer")
        if wide:
            # The strip to the right of both buttons still belongs to the first result row.
            window.click(window.width - 20, 365, route="detail", **{"catalog.detail_back": "搜索"})
            window.click(64, 63, route="search")
            window.click(window.width - 132, 322, route="reader", **{"reader.return_label": "搜索"})
            window.state(**{"reader.loading": False})
            window.click(window.width / 2, window.height / 2, **{"reader.menu": True})
            window.click(60, 66, route="search", **{"catalog.query": "灯塔", "catalog.search_results": 3})


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    flows = (reading, display_preferences, account_settings, download_panel, catalog_and_purchase)
    parser.add_argument("--only", choices=tuple(flow.__name__ for flow in flows))
    args = parser.parse_args()
    args.binary = args.binary.resolve(strict=True)
    args.output = args.output.resolve()
    args.output.mkdir(mode=0o700, parents=True, exist_ok=False)
    for profile in PROFILES:
        for flow in flows:
            if args.only is None or flow.__name__ == args.only:
                flow(args, profile)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
