#!/usr/bin/env python3
"""Verify native JPEG preferences with isolated desktop fixtures on test-env.

The six baseline captures use actual XTest input at device-profile resolution.
Separate seeded-restart and scripted-intent cases exercise policy boundaries;
they do not establish native-driver or physical-device acceptance.
"""

import argparse
import datetime
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import sqlite3
import subprocess
import sys
import time
import traceback

from capture_uiux_baseline import SCALE, Session, write_json
from harness_signals import install_interruption_handlers
from pointer_v2 import CONTEXT_KEYS, INPUT_DELIVERY_PROTOCOL, Pointer
from ui_refinement import PROFILES, stop_owned_group, trace_records


ACCOUNT = "design-demo"
ANCHOR_KEYS = (
    "comic_id", "episode_id", "revision", "page_id", "page_index", "crop_index",
    "source_x", "source_y", "zoom_ratio", "rotation", "core_mode", "fit", "direction",
)
Y4_KEYS = (
    "dither", "dither_method", "hardware_y4_available", "hardware_dither",
    "target_hardware_dither", "hardware_y4_requested",
)


def require(value, message):
    if not value:
        raise AssertionError(message)


def sha256(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def verify_source(arguments):
    manifest = arguments.source_manifest.read_bytes()
    require(hashlib.sha256(manifest).hexdigest() == arguments.source_fingerprint,
            "The selected source manifest differs from its fingerprint")
    identities = []
    for line in manifest.decode("utf-8").splitlines():
        match = re.fullmatch(r"([0-9a-f]{64})  (.+)", line)
        require(match is not None, "The source manifest contains an invalid entry")
        digest, relative = match.groups()
        name = Path(relative)
        require(not name.is_absolute() and ".." not in name.parts and "\\" not in relative,
                "A source input escapes the selected source root")
        path = arguments.source_root / name
        require(not path.is_symlink() and path.resolve(strict=True).is_relative_to(arguments.source_root)
                and path.is_file(), "A source input is not a contained regular file: " + relative)
        require(sha256(path) == digest, "A selected source input changed: " + relative)
        identities.append(relative)
    require(identities and len(set(identities)) == len(identities),
            "The source manifest is empty or contains duplicate inputs")
    return {"base_commit": arguments.source_commit,
            "source_fingerprint": arguments.source_fingerprint,
            "source_manifest_sha256": hashlib.sha256(manifest).hexdigest(),
            "source_input_count": len(identities), "worktree_clean_claimed": False}


def preferences(database):
    with sqlite3.connect(database, timeout=5) as connection:
        rows = connection.execute(
            "SELECT key,payload FROM preferences WHERE account_key=?", (ACCOUNT,)
        ).fetchall()
    return {key: json.loads(value) for key, value in rows
            if key == "preferences" or key.startswith("reader_preferences_")}


def assert_software_record(record, requested=None):
    require(record.get("event") == "ui-state" and record.get("demo") is True
            and record.get("view_model_fixture_only") is False,
            "The observation is not the real isolated desktop application")
    state = record["state"]
    stats = state.get("native_media", {})
    require(all(stats.get(key) == 0 for key in ("attempts", "completed", "resized")),
            "An unsupported desktop profile attempted native JPEG work")
    capability = state.get("account", {}).get("media_acceleration", {})
    require(capability.get("reason") == "platform", "The desktop capability reason is incorrect")
    if state.get("route") == "account":
        require(capability.get("available") is False and bool(capability.get("note")),
                "The unsupported platform lacks a disabled explanation")
        if requested is not None:
            require(capability.get("requested") is requested,
                    "The global requested preference changed unexpectedly")
    if state.get("route") == "reader":
        reader = state.get("reader", {})
        require(reader.get("jpeg_acceleration_available") is False
                and bool(reader.get("jpeg_acceleration_note")),
                "The Reader lacks its disabled acceleration explanation")
        if requested is not None:
            require(reader.get("jpeg_acceleration") is requested,
                    "The per-comic requested preference changed unexpectedly")
        capability = {"requested": reader.get("jpeg_acceleration"), "available": False,
                      "reason": capability["reason"], "note": reader["jpeg_acceleration_note"]}
    return {"capability": capability, "statistics": stats}


def reader_invariants(case, requested=False):
    record = case.reader_ready()
    state = record["reader_state"]
    require(all(key in state for key in ANCHOR_KEYS + Y4_KEYS),
            "The Reader omits an anchor or Y4 invariant")
    require(state.get("jpeg_acceleration") is requested
            and state.get("media_acceleration") == "off"
            and state.get("presented_media_acceleration") == "off"
            and state.get("target_media_acceleration") == "off",
            "An unsupported desktop Reader retained an effective hardware policy")
    assert_software_record(case.latest("ui-state"), requested)
    return {"anchor": {key: state[key] for key in ANCHOR_KEYS},
            "y4": {key: state[key] for key in Y4_KEYS}}


def find_disabled_track(path, profile):
    """Find the right-aligned rectangular switch in the captured X11 pixels."""
    from PIL import Image

    with Image.open(path) as source:
        pixels = source.convert("L")
    width, height = pixels.size
    edge = (48 if profile == "scribe" else 24) * SCALE
    left, right = width - edge - 60 * SCALE, width - edge
    bands = []
    for y in range(70 * SCALE, height - 8 * SCALE):
        row = list(pixels.crop((left + 2 * SCALE, y, right - 2 * SCALE, y + 1)).getdata())
        if row and 40 < min(row) <= max(row) < 245 and max(row) - min(row) <= 3:
            if bands and bands[-1][-1] + 1 == y:
                bands[-1].append(y)
            else:
                bands.append([y])
    candidates = []
    for top, bottom in zip(bands, bands[1:]):
        if (len(top) <= 4 * SCALE and len(bottom) <= 4 * SCALE
                and abs(bottom[0] - top[0] - 30 * SCALE) <= 2 * SCALE):
            middle = (top[0] + bottom[-1]) // 2
            if (pixels.getpixel((left, middle)) < 245
                    and pixels.getpixel((right - 1, middle)) < 245):
                candidates.append((top[0], bottom[-1], middle))
    require(len(candidates) == 1, "The actual pixels do not identify exactly one disabled switch")
    top, bottom, middle = candidates[0]
    return {"left": left, "right": right, "top": top, "bottom": bottom,
            "point": ((left + right) / (2 * SCALE), middle / SCALE),
            "source": "Actual X11 pixels; rectangular switch border"}


def click_disabled(case, capture, requested=False):
    before_reader = reader_invariants(case, requested) if case.route == "reader" else None
    before = preferences(case.directory / "data/demo/state.sqlite3")
    initial = len(case.records())
    track = find_disabled_track(capture, case.profile)
    case.click("Attempt disabled JPEG acceleration", track["point"])
    # A disabled control correctly emits no new UI event. Observe a bounded
    # quiet interval, current pixels and persistence instead of awaiting one.
    time.sleep(0.65)
    require(case.process.poll() is None, "The application exited after a disabled click")
    current = case.pointer.capture(case.window, case.directory / "disabled-click-after.png")
    from PIL import Image, ImageChops
    with Image.open(capture) as before_pixels, Image.open(current["path"]) as after_pixels:
        # The device-profile status clock can advance during a quiet interval.
        top = (32 if case.profile == "scribe" else 28) * SCALE
        box = (0, top, before_pixels.width, before_pixels.height)
        require(ImageChops.difference(before_pixels.convert("RGB").crop(box),
                                      after_pixels.convert("RGB").crop(box)).getbbox() is None,
                "A disabled switch changed the visible application body")
    require(preferences(case.directory / "data/demo/state.sqlite3") == before,
            "A disabled switch changed durable preferences")
    observations = []
    for record in case.records()[initial:]:
        if record.get("process_id") == case.process.pid and record.get("event") == "ui-state":
            observations.append(assert_software_record(record, requested))
    observations.append(assert_software_record(case.latest("ui-state"), requested))
    if before_reader is not None:
        require(reader_invariants(case, requested) == before_reader,
                "A disabled media switch changed the reading anchor or Y4")
    case.report["disabled_switch"] = {
        "track": track, "no_new_ui_event_required": True, "quiet_seconds": 0.65,
        "body_pixels_unchanged": True, "status_clock_excluded_from_comparison": True,
        "preferences_unchanged": True,
        "anchor_and_y4_unchanged": before_reader is not None,
        "statistics": observations[-1]["statistics"],
    }


def open_reader_settings(case, requested=False):
    before = reader_invariants(case, requested)
    width, height = case.logical_size
    edge = 48 if case.profile == "scribe" else 24
    case.click("Open reading controls", (width / 2, height / 2),
               {"reader.menu": True, "reader.sheet": ""})
    point = (edge + (width - 2 * edge) * 3 / 8,
             height - (42 if case.profile == "scribe" else 38))
    case.click("Open reading settings", point, {"reader.sheet": "display"},
               reader={"sheet": "display"})
    require(reader_invariants(case, requested) == before,
            "Opening settings changed the reading anchor or Y4")
    return before


def capture_case(arguments, pointer, profile, kind):
    landscape = kind == "reader-landscape"
    route = "account" if kind == "account" else "reader"
    case = Session(arguments, arguments.output, pointer, profile, kind, route,
                   landscape=landscape, offline=True)
    case.report.update({"passed": False, "input_scope": "Actual XTest input only"})
    try:
        case.start()
        assert_software_record(case.latest("ui-state"), False)
        if kind == "account":
            point = (257, 354) if profile == "scribe" else (case.logical_size[0] / 2, 202)
            case.click("Open default reading settings", point, {"account.section": "reading"})
            scene = "10-account-reading"
        else:
            case.report["reader_invariants"] = open_reader_settings(case)
            scene = "27-reader-settings-landscape" if landscape else "20-reader-settings"
        case.snapshot(scene)
        capture = case.raw / (scene + ".png")
        metadata_path = capture.with_suffix(".json")
        metadata = json.loads(metadata_path.read_text(encoding="utf-8"))
        metadata["captured_source"] = arguments.source_identity
        write_json(metadata_path, metadata)
        case.report["screenshots"][-1] = metadata
        click_disabled(case, capture)
        case.finish()
        case.report["passed"] = True
    except Exception as error:
        case.report.update({"error": str(error), "traceback": traceback.format_exc()})
    finally:
        case.close()
    return case


def seed_requested(source, destination, enabled):
    shutil.copytree(source, destination)
    require(not list(destination.rglob("session.json")), "The fixture unexpectedly contains a session file")
    database = destination / "demo/state.sqlite3"
    with sqlite3.connect(database, timeout=5) as connection:
        rows = connection.execute(
            "SELECT key,payload FROM preferences WHERE account_key=?", (ACCOUNT,)
        ).fetchall()
        selected = {key: json.loads(value) for key, value in rows
                    if key == "preferences" or key.startswith("reader_preferences_")}
        selected.setdefault("preferences", {})
        comic = connection.execute(
            "SELECT comic_id FROM reading_anchors WHERE account_key=? ORDER BY updated_at DESC LIMIT 1",
            (ACCOUNT,),
        ).fetchone()
        require(comic is not None, "The closed Reader fixture has no persisted anchor")
        selected.setdefault("reader_preferences_" + comic[0], {})
        for key, value in selected.items():
            value["jpeg_acceleration"] = enabled
            connection.execute("INSERT OR REPLACE INTO preferences VALUES(?,?,?)",
                               (ACCOUNT, key, json.dumps(value)))
    return {"keys": sorted(selected), "requested": enabled,
            "modified_before_application_start": True}


def persisted_case(arguments, pointer, source):
    case = Session(arguments, arguments.output, pointer, source.profile,
                   "persisted-requested-on", "reader", offline=True)
    case.report.update({"passed": False, "input_scope": "Prelaunch preference seed; ordinary restart"})
    try:
        seed = seed_requested(source.directory / "data", case.directory / "data", True)
        case.report["seed"] = seed
        case.start()
        case.observe({"reader.jpeg_acceleration": True, "reader.jpeg_acceleration_available": False})
        invariant = reader_invariants(case, True)
        require(invariant == source.report["reader_invariants"],
                "Restart with a hardware preference changed the reading anchor or Y4")
        open_reader_settings(case, True)
        case.report["observation"] = {"requested": True, "effective": "off",
                                      "invariants": invariant,
                                      **assert_software_record(case.latest("ui-state"), True)}
        case.finish()
        saved = preferences(case.directory / "data/demo/state.sqlite3")
        require(all(saved[key].get("jpeg_acceleration") is True for key in seed["keys"]),
                "A disabled platform discarded the durable requested preference")
        case.report["requested_preserved_on_restart"] = True
        case.report["anchor_and_y4_preserved_on_restart"] = True
        case.report["passed"] = True
    except Exception as error:
        case.report.update({"error": str(error), "traceback": traceback.format_exc()})
    finally:
        case.close()
    return case.report


def forged_case(arguments, source, route):
    directory = arguments.output / "scripted-policy" / (source.profile + "-" + route)
    directory.mkdir(parents=True, mode=0o700, exist_ok=False)
    result = {"profile": source.profile, "route": route, "passed": False,
              "input_scope": "Scripted intents; excluded from XTest capture acceptance"}
    process, log = None, None
    try:
        result["seed"] = seed_requested(source.directory / "data", directory / "data", False)
        actions = ([{"at_ms": 600, "section": "account", "action": "jpeg-acceleration", "value": "on"},
                    {"at_ms": 1200, "section": "account", "action": "section", "value": "storage"},
                    {"at_ms": 1800, "section": "account", "action": "section", "value": "reading"}]
                   if route == "account" else
                   [{"at_ms": 600, "section": "reader", "action": "setting", "value": "jpeg-acceleration:on"},
                    {"at_ms": 1200, "section": "reader", "action": "menu", "value": "open"},
                    {"at_ms": 1800, "section": "reader", "action": "sheet", "value": "display"}])
        write_json(directory / "actions.json", actions)
        environment = os.environ.copy()
        for key in CONTEXT_KEYS:
            environment.pop(key, None)
        environment["SLINT_SCALE_FACTOR"] = str(SCALE)
        command = [str(arguments.binary), "--demo", "--offline", "--device", source.profile,
                   "--screen", route, "--data-dir", str(directory / "data"),
                   "--actions", str(directory / "actions.json"),
                   "--lifecycle-trace", str(directory / "lifecycle.jsonl"),
                   "--capture", str(directory / "final.png"), "--exit-after-ms", "6000"]
        result["command"] = command
        log = (directory / "application.log").open("w", encoding="utf-8")
        process = subprocess.Popen(command, env=environment, stdout=log, stderr=subprocess.STDOUT,
                                   start_new_session=True)
        require(process.wait(timeout=90) == 0, "The scripted policy case did not exit cleanly")
        records = [row for row in trace_records(directory / "lifecycle.jsonl")
                   if row.get("process_id") == process.pid]
        states = [row for row in records if row.get("event") == "ui-state"]
        require(states, "The scripted policy case produced no live UI observations")
        for state in states:
            assert_software_record(state, False)
        final = json.loads((directory / "final.json").read_text(encoding="utf-8"))
        require(final.get("demo") is True and final.get("backend") == "desktop-software"
                and not final.get("failures"), "The scripted case has a failed or unexpected runtime")
        if route == "account":
            require(any(row["state"]["account"]["section"] == "storage" for row in states)
                    and states[-1]["state"]["account"]["section"] == "reading",
                    "The post-forgery account dispatch milestones were not observed")
        else:
            ready = [row["reader_state"] for row in records if row.get("event") == "reader-ready"
                     and row.get("reader_loading") is False]
            require(ready and ready[-1].get("sheet") == "display",
                    "The post-forgery Reader dispatch milestone was not observed")
            require(all(all(key in row for key in ANCHOR_KEYS + Y4_KEYS) for row in ready),
                    "A forged-intent observation omits an anchor or Y4 invariant")
            first = {key: ready[0].get(key) for key in ANCHOR_KEYS + Y4_KEYS}
            require(all({key: row.get(key) for key in ANCHOR_KEYS + Y4_KEYS} == first for row in ready),
                    "A forged media request changed the reading anchor or Y4")
            require(all(all(row.get(key) == "off" for key in
                            ("media_acceleration", "presented_media_acceleration", "target_media_acceleration"))
                        for row in ready),
                    "A forged media request activated the Reader hardware policy")
        saved = preferences(directory / "data/demo/state.sqlite3")
        require(all(not value.get("jpeg_acceleration", False) for value in saved.values()),
                "A forged request enabled a durable acceleration preference")
        result.update({"passed": True, "final_observation": assert_software_record(states[-1], False),
                       "capability_gate_rejected_on": True, "persistent_preferences": saved})
    except Exception as error:
        result.update({"error": str(error), "traceback": traceback.format_exc()})
    finally:
        result["cleanup"] = stop_owned_group(process)
        if log is not None:
            log.close()
        write_json(directory / "result.json", result)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--expected-sha256", "--expected-binary-sha256", dest="expected_sha256", required=True)
    parser.add_argument("--source-commit", required=True, help="Base commit; no clean-worktree claim is made")
    parser.add_argument("--source-root", type=Path, default=Path(__file__).resolve().parents[2])
    parser.add_argument("--source-manifest", type=Path, required=True)
    parser.add_argument("--source-fingerprint", required=True, help="SHA-256 of the exact build source manifest")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--profiles", nargs="+", choices=list(PROFILES), default=["pw6", "scribe"])
    arguments = parser.parse_args()
    require(sys.platform.startswith("linux") and os.environ.get("BILICOMICS_REMOTE_VERIFICATION") == "1",
            "Run only on designated Linux test-env with BILICOMICS_REMOTE_VERIFICATION=1")
    require(re.fullmatch(r"[0-9a-f]{40}", arguments.source_commit), "Supply the full lowercase base commit")
    require(all(re.fullmatch(r"[0-9a-f]{64}", value) for value in
                (arguments.expected_sha256, arguments.source_fingerprint)), "Supply exact lowercase SHA-256 identities")
    arguments.binary = arguments.binary.resolve(strict=True)
    arguments.source_root = arguments.source_root.resolve(strict=True)
    arguments.source_manifest = arguments.source_manifest.resolve(strict=True)
    require(arguments.binary.is_file() and os.access(arguments.binary, os.X_OK)
            and sha256(arguments.binary) == arguments.expected_sha256, "The selected desktop binary differs")
    require(arguments.output.is_absolute(), "Supply a fresh absolute output directory")
    arguments.output = arguments.output.resolve()
    os.umask(0o077)
    arguments.output.mkdir(parents=True, mode=0o700, exist_ok=False)
    arguments.source_identity = verify_source(arguments)
    shutil.copyfile(arguments.source_manifest, arguments.output / "source-inputs.sha256")
    install_interruption_handlers()
    pointer = None
    report = {"suite": "native_media_desktop_settings", "passed": False,
              "started_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
              "source": arguments.source_identity, "binary_sha256": arguments.expected_sha256,
              "harness_sha256": sha256(Path(__file__)),
              "harness_dependencies": {name: sha256(Path(__file__).with_name(name)) for name in
                                       ("capture_uiux_baseline.py", "pointer_v2.py", "ui_refinement.py",
                                        "harness_signals.py")},
              "capture_host": "test-env", "scale_factor": SCALE,
              "input_protocol": INPUT_DELIVERY_PROTOCOL, "visual_review_status": "pending",
              "scope": "Desktop preference gates, persistence, anchor/Y4 invariants and untouched X11 captures",
              "physical_device_used": False, "native_driver_exercised": False, "live_account_used": False,
              "hardware_compatibility_claimed": False, "sessions": [], "policy_cases": [], "errors": []}
    try:
        pointer = Pointer()
        for profile in dict.fromkeys(arguments.profiles):
            cases = [capture_case(arguments, pointer, profile, kind)
                     for kind in ("account", "reader-portrait", "reader-landscape")]
            report["sessions"].extend(case.report for case in cases)
            source = cases[1]
            if source.report.get("passed"):
                report["policy_cases"].append(persisted_case(arguments, pointer, source))
                for route in ("account", "reader"):
                    report["policy_cases"].append(forged_case(arguments, source, route))
            else:
                report["errors"].append({"profile": profile, "error": "Reader seed failed; policy cases skipped"})
            write_json(arguments.output / "native-media-settings-result.json", report)
        require(sha256(arguments.binary) == arguments.expected_sha256, "The selected binary changed during verification")
        verify_source(arguments)
        selected = report["sessions"] + report["policy_cases"]
        report["passed"] = bool(selected) and not report["errors"] and all(case.get("passed") for case in selected)
    except BaseException as error:
        report["errors"].append({"error": str(error), "traceback": traceback.format_exc()})
    finally:
        if pointer is not None:
            pointer.close()
        report["finished_at"] = datetime.datetime.now(datetime.timezone.utc).isoformat()
        report["capture_count"] = sum(len(case.get("screenshots", [])) for case in report["sessions"])
        write_json(arguments.output / "native-media-settings-result.json", report)
    print(json.dumps({"passed": report["passed"], "captures": report["capture_count"],
                      "policy_cases": len(report["policy_cases"]), "visual_review_status": "pending"}), flush=True)
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
