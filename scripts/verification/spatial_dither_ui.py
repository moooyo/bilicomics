#!/usr/bin/env python3
"""Capture the CPU dithering choices using real XTest input on test-env.

Only the existing isolated comic demonstration is used. Normal page captures
must preserve native-code membership without requiring a particular tone count.
Fast pages must remain binary while
normal-mode choices are saved. These desktop results do not exercise a native
driver, prove a hardware waveform, or establish physical-device acceptance.
"""

import argparse
import datetime
import json
import os
from pathlib import Path
import re
import shutil
import sys
import time
import traceback

import fast_mode_ui as fast
from capture_uiux_baseline import SCALE, write_json
from harness_signals import install_interruption_handlers
from native_media_settings import verify_source
from pointer_v2 import INPUT_DELIVERY_PROTOCOL, Pointer
from ui_refinement import PROFILES


METHODS = ("off", "bayer8", "floyd_steinberg")
NOTE_KEYS = {
    "off": "spatial_dither_off_note", "bayer8": "spatial_dither_bayer8_note",
    "floyd_steinberg": "spatial_dither_floyd_note", "fast": "spatial_dither_fast_saved_note",
    "pending": "spatial_dither_pending_note",
}


class SpatialSession(fast.FastSession):
    def __init__(self, arguments, pointer, profile, name, route, landscape=False):
        super().__init__(arguments, pointer, profile, name, route, landscape)
        self.comic = None
        self.reference = None
        self.storage = None
        self.report["selection_steps"] = []

    def capture_evidence(self, name):
        self.sequence += 1
        stem = str(self.sequence).zfill(3) + "-" + name
        samples_root = self.directory / ("settle-" + stem)
        samples_root.mkdir(exist_ok=False)
        started = time.monotonic()
        previous, identical, samples = None, 0, []
        while True:
            fast.require(self.process.poll() is None, "The application exited before stable pixel evidence")
            frame = samples_root / (str(len(samples)).zfill(3) + ".png")
            capture = self.pointer.capture(self.window, frame)
            digest = capture["sha256"]
            identical = identical + 1 if digest == previous else 1
            previous = digest
            elapsed = time.monotonic() - started
            samples.append({"file": str(frame.relative_to(self.output)), "sha256": digest,
                            "elapsed_seconds": round(elapsed, 3)})
            if elapsed >= 1.5 and identical >= 3:
                break
            fast.require(elapsed < 30, "The actual spatial-dithering pixels did not stabilize within 30 seconds")
            time.sleep(0.35)
        target = self.evidence / (stem + ".png")
        shutil.copyfile(frame, target)
        metadata = {**capture, "path": str(target), "file": str(target.relative_to(self.output)),
                    "id": name, "profile": self.profile, "session": self.name,
                    "process_id": self.process.pid, "width": self.window["width"],
                    "height": self.window["height"], "logical_size": self.logical_size,
                    "scale_factor": SCALE, "binary_sha256": self.arguments.expected_sha256,
                    "captured_source": self.arguments.source_identity,
                    "captured_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
                    "pixel_processing": "None; copied untouched stable X11 RGB pixels",
                    "stability": {"minimum_settle_seconds": 1.5, "identical_samples": identical,
                                  "samples": samples},
                    "baseline_capture": False, "demo": True, "physical_device_used": False,
                    "live_account_used": False, "hardware_waveform_proof": False}
        write_json(target.with_suffix(".json"), metadata)
        self.report["evidence"].append(metadata)
        return target


def load_notes(source_root):
    labels = {}
    for path in sorted((source_root / "assets/i18n").glob("*.json")):
        values = json.loads(path.read_text(encoding="utf-8-sig"))
        for key in NOTE_KEYS.values():
            if key in values:
                fast.require(key not in labels, "A spatial-dithering explanation has duplicate source identities")
                fast.require(isinstance(values[key], str) and values[key], "A spatial-dithering explanation is empty")
                labels[key] = values[key]
    fast.require(set(labels) == set(NOTE_KEYS.values()), "The source translations omit a spatial-dithering explanation")
    return {name: labels[key] for name, key in NOTE_KEYS.items()}


def observe_choice(case, mode, enabled, method):
    record = case.latest("ui-state")
    model = record.get("state", {}).get(case.route, {}) if record else {}
    required = ("dither", "dither_method", "dither_note", "refresh_mode")
    fast.require(all(key in model for key in required), "The real UI omits typed spatial-dithering fields")
    expected = {case.route + ".refresh_mode": mode, case.route + ".dither": enabled,
                case.route + ".dither_method": method}
    if case.route == "reader":
        fast.require("presented_cpu_dither" in model and type(model["presented_cpu_dither"]) is bool,
                     "The Reader UI omits its actual presented CPU-dithering flag")
        # Off still writes nearest native tones through the CPU raster path.
        # This actual presentation flag is independent of saved enablement.
        expected["reader.presented_cpu_dither"] = mode == "normal"
    record = case.observe(expected)
    model = record["state"][case.route]
    fast.require(type(model["dither"]) is bool and isinstance(model["dither_method"], str),
                 "The selected dithering preference has an invalid type")
    if case.route == "reader":
        state = case.reader_ready({"dither": enabled, "dither_method": method})["reader_state"]
        state = fast.reader_state(case, mode)
        fast.require(state["dither"] is enabled and state["dither_method"] == method,
                     "The loaded Reader differs from its saved CPU-dithering choice")
        for key in ("hardware_y4_requested", "hardware_dither", "target_hardware_dither"):
            fast.require(key in state and state[key] is False, "CPU dithering requested a hardware Y4 path: " + key)
        versioned = ("binary_bayer4_v1" if mode == "fast" else "off" if not enabled
                     else "bayer8_v1" if method == "bayer8" else "floyd_steinberg_q8_v1")
        fast.require(state.get("presented_dither_algorithm") == versioned
                     and state.get("target_dither_algorithm") == versioned,
                     "The actual presented or target algorithm has another processing identity")
        fast.require(type(state.get("raster_version")) is int and state["raster_version"] >= 2,
                     "The Reader omits its versioned final-raster identity")
        fast.require(fast.anchor(state) == case.reference, "Changing CPU dithering moved the live reading anchor")
    key = "fast" if mode == "fast" else method if enabled else "off"

    def settled_explanation():
        current = case.latest("ui-state")
        visible = current.get("state", {}).get(case.route, {}) if current else {}
        note = visible.get("dither_note")
        if (visible.get("refresh_mode") == mode and visible.get("dither") is enabled
                and visible.get("dither_method") == method and isinstance(note, str)
                and note.startswith(case.arguments.notes[key])
                and case.arguments.notes["pending"] not in note):
            return current
        return None

    return case.wait("the settled algorithm or Fast-preservation explanation", settled_explanation)


def storage(case, mode, enabled, method):
    current = fast.database_snapshot(case, case.comic)
    fast.require(all(value == mode for value in current["modes"].values()), "The durable refresh mode differs")
    fast.require(all(value["dither"] is enabled and value["dither_method"] == method
                     for value in current["saved"].values()), "The durable CPU choice or remembered method differs")
    if case.storage is not None:
        fast.require(current["anchors"] == case.storage["anchors"], "CPU choices changed a durable source anchor")
        fast.require({key: value["every"] for key, value in current["saved"].items()}
                     == {key: value["every"] for key, value in case.storage["saved"].items()},
                     "CPU choices changed the saved normal-mode clearing interval")
    return current


def choice_fill(path, box, selected):
    pixels = fast.rgb(path).convert("L")
    inner = box["right"] - box["left"] - 4 * SCALE
    observed = []
    for index in range(3):
        # The long Floyd label has two centered lines. Sample the flat inner
        # corner, never its first line of white or black text.
        x = round(box["left"] + 2 * SCALE + index * inner / 3 + 6 * SCALE)
        y = box["top"] + 6 * SCALE
        low, high = pixels.crop((x, y, x + 2, y + 2)).getextrema()
        fast.require(high < 30 if index == selected else low > 240,
                     "Actual CPU selector backgrounds do not show the selected option")
        observed.append({"index": index, "minimum": low, "maximum": high})
    return observed


def has_three_options(path, box):
    pixels = fast.rgb(path).convert("L")
    y = box["top"] + 4 * SCALE
    for index in (1, 2):
        x = round(box["left"] + 2 * SCALE + index * (box["right"] - box["left"] - 4 * SCALE) / 3) + SCALE
        if pixels.getpixel((x, y)) >= 225 or max(pixels.getpixel((x + offset * SCALE, y)) for offset in (-4, 4)) <= 240:
            return False
    return True


def wheel(case, direction):
    before = case.capture_evidence("account-before-scroll")
    body = fast.account_body(case)
    x, y = round(case.window["width"] / 2), round((body[1] + body[3]) / 2)
    pointer = case.pointer
    pointer.xtst.XTestFakeMotionEvent(pointer.display, pointer.x11.XDefaultScreen(pointer.display),
                                     case.window["x"] + x, case.window["y"] + y, 0)
    pointer.x11.XFlush(pointer.display)
    pointer.x11.XSync(pointer.display, 0)
    time.sleep(0.08)
    for _ in range(3):
        pointer.xtst.XTestFakeButtonEvent(pointer.display, 5 if direction == "down" else 4, 1, 0)
        pointer.xtst.XTestFakeButtonEvent(pointer.display, 5 if direction == "down" else 4, 0, 0)
        pointer.x11.XSync(pointer.display, 0)
        time.sleep(0.03)
    after = case.capture_evidence("account-after-scroll")
    fast.require(fast.fixed_account_pixels(case, before) == fast.fixed_account_pixels(case, after),
                 "CPU-option scrolling moved the fixed Account header or navigation")
    changed = fast.rectangle_hash(fast.rgb(before), body) != fast.rectangle_hash(fast.rgb(after), body)
    case.report["scrolls"].append({"method": "Actual XTest wheel buttons", "direction": direction, "ticks": 3,
                                   "body_pixels_changed": changed, "header_and_navigation_unchanged": True,
                                   "before": str(before.relative_to(case.output)), "after": str(after.relative_to(case.output))})
    return after, changed


def account_top(case):
    for _ in range(12):
        path, changed = wheel(case, "up")
        if not changed:
            fast.account_mode_box(case, path)
            return path
    raise AssertionError("The Account reading area did not return to its bounded scroll top")


def dither_control(case):
    if case.route == "reader":
        path = case.capture_evidence("reader-cpu-controls")
        controls = fast.reader_controls(case, path)
        box = controls["choices"][2 if case.landscape else 5]
        fast.require(has_three_options(path, box), "The Reader CPU selector does not have three actual option cells")
        return path, box
    width, _ = case.logical_size
    edge = 48 if case.profile == "scribe" else 24
    body = fast.account_body(case)
    for attempt in range(13):
        path = case.capture_evidence("locate-account-cpu-selector")
        boxes = fast.choice_boxes(path, edge, width - edge, 64 if case.profile == "scribe" else 52,
                                  body[1] / SCALE, body[3] / SCALE)
        boxes = [box for box in boxes if has_three_options(path, box)]
        fast.require(len(boxes) <= 1, "The full-width three-option CPU selector is ambiguous")
        if boxes:
            return path, boxes[0]
        fast.require(attempt < 12, "The Account CPU selector is not reachable in its clipped reading area")
        _, changed = wheel(case, "down")
        fast.require(changed, "Account scrolling stopped before its CPU selector became fully visible")
    raise AssertionError("Unreachable CPU selector")


def open_settings(case, mode):
    if case.route == "reader":
        fast.open_reader_settings(case, mode, case.reference)
    else:
        width, _ = case.logical_size
        point = ((48 + 1.5 * (width - 96) / 6, 354) if case.profile == "scribe" else (width / 2, 202))
        case.click("Open Account reading defaults", point, {"account.section": "reading"})
        path = case.capture_evidence("account-reading-entry")
        fast.account_mode_box(case, path)


def select_choice(case, choice, mode, previous_method, *, baseline=False):
    _, box = dither_control(case)
    enabled, method = choice != "off", previous_method if choice == "off" else choice
    case.click("Select CPU dithering " + choice, fast.option_point(box, METHODS.index(choice), 3))
    observe_choice(case, mode, enabled, method)
    current = case.wait("the actual CPU selection in durable preferences",
                        lambda: saved_choice_or_none(case, mode, enabled, method))
    if case.storage is None:
        case.storage = current
    path, box = dither_control(case)
    samples = choice_fill(path, box, METHODS.index(choice))
    if baseline:
        direction = "landscape" if case.landscape else "portrait"
        name = ("20-reader" if case.route == "reader" else "10-account") + "-cpu-" + choice.replace("_", "-") + "-" + direction
        path = case.baseline(name, mode)
        box = (fast.reader_controls(case, path)["choices"][2 if case.landscape else 5]
               if case.route == "reader" else box)
        choice_fill(path, box, METHODS.index(choice))
    case.report["selection_steps"].append({"choice": choice, "refresh_mode": mode, "enabled": enabled,
                                           "remembered_method": method, "file": str(path.relative_to(case.output)),
                                           "background_samples": samples, "durable": current})
    return method


def saved_choice_or_none(case, mode, enabled, method):
    try:
        return storage(case, mode, enabled, method)
    except AssertionError:
        return None


def page(case, mode, enabled, method, name, expected_sha=None):
    observe_choice(case, mode, enabled, method)
    state = fast.reader_state(case, mode)
    fast.require(state["menu"] is False and state["sheet"] == "" and state["rendering"] is False,
                 "A page observation requires a quiet complete Reader frame")
    fast.require(state["reader_refresh_scene"] == ("fast_content" if mode == "fast" else "normal"),
                 "The actual Reader page is not in its expected content-only refresh scene")
    slots = 2 if state["effective_spread"] else 1
    width, height = case.window["width"], case.window["height"]
    fast.require(state["frame_width"] * slots == width and state["frame_height"] == height
                 and state["viewport_width"] * slots == width and state["viewport_height"] == height,
                 "The Reader frame and viewport do not cover the complete physical window")
    if slots == 2:
        fast.require(state["second_frame_width"] == state["frame_width"] and state["second_frame_height"] == height,
                     "The second Reader page does not cover its complete physical slot")
    path = case.capture_evidence(name)
    observe_choice(case, mode, enabled, method)
    if mode == "fast":
        colors = fast.rgb(path).getcolors(maxcolors=3)
        fast.require(colors is not None and {color for _, color in colors} == {(0, 0, 0), (255, 255, 255)},
                     "The actual stable Fast page is not nonempty binary black and white")
    else:
        colors = fast.rgb(path).getcolors(maxcolors=17)
        fast.require(colors is not None and all(r == g == b and r % 17 == 0
                     for _, (r, g, b) in colors),
                     "The final CPU page was interpolated outside the sixteen native gray codes")
    if expected_sha is not None:
        fast.require(fast.sha256(path) == expected_sha, "Restored Normal or preserved Fast page pixels differ")
    return {"file": str(path.relative_to(case.output)), "sha256": fast.sha256(path),
            "physical_size": [width, height], "tone_count_asserted": mode == "fast",
            "native_code_membership_asserted": True,
            "hardware_y4_requested": False, "pixel_processing": "None"}


def set_mode(case, mode, enabled, method):
    if case.route == "account":
        path = account_top(case)
        box = fast.account_mode_box(case, path)
    else:
        path = case.capture_evidence("reader-refresh-mode-control")
        box = fast.reader_controls(case, path)["mode"]
    case.click("Select refresh mode " + mode, fast.option_point(box, 1 if mode == "fast" else 0, 2),
               {case.route + ".refresh_mode": mode})
    observe_choice(case, mode, enabled, method)
    storage(case, mode, enabled, method)


def source_case(arguments, pointer, profile, route, landscape):
    direction = "landscape" if landscape else "portrait"
    case = SpatialSession(arguments, pointer, profile, route + "-cpu-" + direction, route, landscape)
    try:
        case.start()
        if route == "reader":
            state = fast.reader_state(case, "normal")
            case.reference, case.comic = fast.anchor(state), state["comic_id"]
        observe_choice(case, "normal", True, "bayer8")
        open_settings(case, "normal")
        method, normal_pages = "bayer8", {}
        for choice in METHODS:
            method = select_choice(case, choice, "normal", method, baseline=True)
            if route == "reader":
                fast.close_reader_settings(case, "normal", case.reference)
                normal_pages[choice] = page(case, "normal", choice != "off", method, "normal-page-" + choice)
                open_settings(case, "normal")
        method = select_choice(case, "off", "normal", method)
        fast.require(method == "floyd_steinberg", "Off discarded the last Floyd algorithm")
        case.report["off_last_method"] = {"passed": True, "enabled": False, "remembered_method": method}
        case.report.update({"storage_reference": case.storage, "reader_anchor": case.reference,
                            "comic": case.comic, "normal_pages": normal_pages})
        case.finish()
        storage(case, "normal", False, "floyd_steinberg")
        case.report["passed"] = True
    except Exception as error:
        case.report.update({"error": str(error), "traceback": traceback.format_exc()})
    finally:
        case.close()
    return case


def restart_and_fast_case(arguments, pointer, source):
    case = SpatialSession(arguments, pointer, source.profile, source.name + "-restart", source.route, source.landscape)
    try:
        fast.require(source.process.poll() is not None and source.report.get("complete"),
                     "Cold restart requires a gracefully closed source process")
        data = source.directory / "data"
        fast.require(not list(data.rglob("session.json")), "The private demo data unexpectedly contains a session")
        shutil.copytree(data, case.directory / "data")
        case.reference, case.comic, case.storage = source.reference, source.comic, source.storage
        case.start()
        observe_choice(case, "normal", False, "floyd_steinberg")
        storage(case, "normal", False, "floyd_steinberg")
        open_settings(case, "normal")
        path, box = dither_control(case)
        choice_fill(path, box, 0)
        method = select_choice(case, "floyd_steinberg", "normal", "floyd_steinberg")
        if case.route == "reader":
            fast.close_reader_settings(case, "normal", case.reference)
            page(case, "normal", True, method, "cold-normal-floyd-restored",
                 source.report["normal_pages"]["floyd_steinberg"]["sha256"])
            open_settings(case, "normal")
        case.report["cold_restart"] = {"passed": True, "off_and_last_floyd_retained": True,
                                       "normal_floyd_restored_by_xtest": True,
                                       "database_modified_by_harness": False}
        set_mode(case, "fast", True, method)
        fast_reference = None
        if case.route == "reader":
            fast.close_reader_settings(case, "fast", case.reference)
            fast_reference = page(case, "fast", True, method, "fast-page-before-cpu-choices")
            open_settings(case, "fast")
        steps = []
        for choice in ("bayer8", "floyd_steinberg", "off"):
            method = select_choice(case, choice, "fast", method)
            if case.route == "reader":
                fast.close_reader_settings(case, "fast", case.reference)
                result = page(case, "fast", choice != "off", method, "fast-page-after-" + choice,
                              fast_reference["sha256"])
                open_settings(case, "fast")
            else:
                result = {"scope": "Account defaults only; no active Reader or hardware-Y4 observation"}
            steps.append({"choice": choice, "saved": storage(case, "fast", choice != "off", method),
                          "page": result})
        set_mode(case, "normal", False, method)
        if case.route == "reader":
            fast.close_reader_settings(case, "normal", case.reference)
            page(case, "normal", False, method, "normal-off-restored-after-fast",
                 source.report["normal_pages"]["off"]["sha256"])
            open_settings(case, "normal")
        select_choice(case, "floyd_steinberg", "normal", method)
        if case.route == "reader":
            fast.close_reader_settings(case, "normal", case.reference)
            page(case, "normal", True, "floyd_steinberg", "normal-floyd-restored-after-fast",
                 source.report["normal_pages"]["floyd_steinberg"]["sha256"])
        case.report["fast_preservation"] = {"passed": True, "steps": steps,
                                            "off_retained_last_floyd": True,
                                            "normal_choices_restored_by_xtest": True,
                                            "no_hardware_y4_claim_for_account_defaults": True}
        case.finish()
        storage(case, "normal", True, "floyd_steinberg")
        case.report["passed"] = True
    except Exception as error:
        case.report.update({"error": str(error), "traceback": traceback.format_exc()})
    finally:
        case.close()
    return case.report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--expected-sha256", "--expected-binary-sha256", dest="expected_sha256", required=True)
    parser.add_argument("--source-root", type=Path, default=Path(__file__).resolve().parents[2])
    parser.add_argument("--source-commit", required=True)
    parser.add_argument("--source-manifest", type=Path, required=True)
    parser.add_argument("--source-fingerprint", required=True)
    parser.add_argument("--baseline-id", required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--profiles", nargs="+", choices=list(PROFILES), default=["pw6", "scribe"])
    arguments = parser.parse_args()
    fast.require(sys.platform.startswith("linux") and os.environ.get("BILICOMICS_REMOTE_VERIFICATION") == "1",
                 "Run only on designated Linux test-env with BILICOMICS_REMOTE_VERIFICATION=1")
    fast.require(os.environ.get("DISPLAY"), "Run inside a fresh isolated Xvfb display")
    fast.require(re.fullmatch(r"[0-9a-f]{40}", arguments.source_commit), "Supply the full lowercase base commit")
    fast.require(all(re.fullmatch(r"[0-9a-f]{64}", value) for value in
                     (arguments.expected_sha256, arguments.source_fingerprint)), "Supply exact lowercase SHA-256 identities")
    fast.require(re.fullmatch(r"[a-z0-9][a-z0-9-]{0,95}", arguments.baseline_id), "Supply a bounded new baseline identity")
    arguments.binary = arguments.binary.resolve(strict=True)
    arguments.source_root = arguments.source_root.resolve(strict=True)
    arguments.source_manifest = arguments.source_manifest.resolve(strict=True)
    fast.require(arguments.binary.is_file() and os.access(arguments.binary, os.X_OK)
                 and fast.sha256(arguments.binary) == arguments.expected_sha256, "The selected desktop binary differs")
    fast.require(arguments.output.is_absolute(), "Supply a fresh absolute evidence directory")
    arguments.output = arguments.output.resolve()
    arguments.source_identity = verify_source(arguments)
    arguments.notes = load_notes(arguments.source_root)
    os.umask(0o077)
    arguments.output.mkdir(parents=True, mode=0o700, exist_ok=False)
    shutil.copyfile(arguments.source_manifest, arguments.output / "source-inputs.sha256")
    install_interruption_handlers()
    profiles = list(dict.fromkeys(arguments.profiles))
    report = {"suite": "spatial_dither_desktop_ui", "baseline_id": arguments.baseline_id, "passed": False,
              "started_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
              "source": arguments.source_identity, "binary_sha256": arguments.expected_sha256,
              "harness_sha256": fast.sha256(Path(__file__)),
              "harness_dependencies": {name: fast.sha256(Path(__file__).with_name(name)) for name in
                                       ("fast_mode_ui.py", "capture_uiux_baseline.py", "pointer_v2.py",
                                        "ui_refinement.py", "harness_signals.py", "native_media_settings.py")},
              "capture_host": "test-env", "scale_factor": SCALE, "input_protocol": INPUT_DELIVERY_PROTOCOL,
              "visual_review_status": "pending", "expected_capture_count": 12 * len(profiles),
              "scope": "Actual Off/Bayer/Floyd UI, existing comic pages, persistence and saved Fast-mode choices",
              "normal_page_tone_count_asserted": False, "physical_device_used": False,
              "native_driver_exercised": False, "live_account_used": False, "hardware_waveform_proof": False,
              "hardware_compatibility_claimed": False, "named_ui_actions_used": False,
              "sessions": [], "restart_and_fast_cases": [], "errors": []}
    pointer = None
    try:
        pointer = Pointer()
        for profile in profiles:
            for landscape in (False, True):
                for route in ("reader", "account"):
                    source = source_case(arguments, pointer, profile, route, landscape)
                    report["sessions"].append(source.report)
                    if source.report.get("passed"):
                        report["restart_and_fast_cases"].append(restart_and_fast_case(arguments, pointer, source))
                    else:
                        report["errors"].append({"profile": profile, "session": source.name,
                                                 "error": "The Normal source case failed; restart and Fast checks were skipped"})
                    write_json(arguments.output / "spatial-dither-ui-result.json", report)
        fast.require(fast.sha256(arguments.binary) == arguments.expected_sha256, "The selected binary changed during verification")
        verify_source(arguments)
        selected = report["sessions"] + report["restart_and_fast_cases"]
        captures = sum(len(case.get("screenshots", [])) for case in report["sessions"])
        report["passed"] = (not report["errors"] and len(selected) == 8 * len(profiles)
                            and captures == report["expected_capture_count"]
                            and all(case.get("passed") for case in selected))
    except BaseException as error:
        report["errors"].append({"error": str(error), "traceback": traceback.format_exc()})
    finally:
        if pointer is not None:
            pointer.close()
        report["finished_at"] = datetime.datetime.now(datetime.timezone.utc).isoformat()
        report["capture_count"] = sum(len(case.get("screenshots", [])) for case in report["sessions"])
        write_json(arguments.output / "spatial-dither-ui-result.json", report)
    print(json.dumps({"passed": report["passed"], "captures": report["capture_count"],
                      "restart_and_fast_cases": len(report["restart_and_fast_cases"]),
                      "visual_review_status": "pending"}), flush=True)
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
