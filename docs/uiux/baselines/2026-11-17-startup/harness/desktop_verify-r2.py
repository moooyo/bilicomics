#!/usr/bin/env python3
"""Compare source-bound desktop startup frames on the remote Linux worker.

Each case owns fresh offline demonstration data and its own Xvfb server. The
tool never opens a Kindle, authenticates an account, or injects input. Timings
contain a controlled delay and are not startup-performance measurements.
"""

import argparse
import concurrent.futures
import datetime
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import signal
import subprocess
import sys
import time
import traceback
import uuid

from PIL import Image, ImageChops


PROFILES = {"pw6": (632, 840), "scribe": (930, 1240)}
SCALE = 2
CASE_MARKER = "BILICOMICS_DESKTOP_VERIFY_CASE"
VARIANTS = ("baseline", "candidate")
SCENARIOS = ("normal", "failure")
SOURCE_FIELDS = (
    "binary", "binary_sha256", "source_root", "source_commit",
    "source_manifest", "source_manifest_sha256", "source_archive",
    "source_archive_sha256",
)
REQUIRED_SOURCE_FILES = (
    "src/main.rs", "src/fonts.rs", "src/app/startup.rs", "ui/app.slint",
    "ui/startup.slint", "assets/i18n/startup.json",
    "assets/fonts/StartupSansSC.otf", "assets/fonts/startup-manifest.json",
)


def require(value, message):
    if not value:
        raise AssertionError(message)


def digest(path):
    result = hashlib.sha256()
    with Path(path).open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            result.update(chunk)
    return result.hexdigest()


def write_json(path, value):
    with Path(path).open("x", encoding="utf-8", newline="\n") as output:
        json.dump(value, output, ensure_ascii=False, indent=2)
        output.write("\n")


def read_json(path):
    return json.loads(Path(path).read_text(encoding="utf-8"))


def load_records(path):
    if not path.is_file():
        return []
    return [json.loads(line) for line in path.read_text(encoding="utf-8").splitlines(keepends=True)
            if line.endswith("\n") and line.strip()]


def source_inventory(record):
    root = Path(record["source_root"]).resolve(strict=True)
    manifest = Path(record["source_manifest"]).resolve(strict=True)
    require(digest(manifest) == record["source_manifest_sha256"], "Source inventory hash mismatch")
    result = {}
    for line in manifest.read_text(encoding="utf-8").splitlines():
        match = re.fullmatch(r"([0-9a-f]{64})  (.+)", line)
        require(match, "Malformed source inventory line")
        expected, name = match.groups()
        relative = PurePosixPath(name)
        require(not relative.is_absolute() and ".." not in relative.parts and "\\" not in name,
                "Unsafe source inventory path")
        require(name not in result, "Duplicate source inventory path")
        path = root.joinpath(*relative.parts)
        require(path.is_file() and not path.is_symlink() and path.resolve().is_relative_to(root),
                "Missing or unsafe source input: " + name)
        require(digest(path) == expected, "Source input changed: " + name)
        result[name] = expected
    require(set(REQUIRED_SOURCE_FILES).issubset(result), "The selected source inventory is incomplete")
    require(digest(record["source_archive"]) == record["source_archive_sha256"],
            "Selected source archive changed")
    return {"files": len(result), "required_inputs": {name: result[name] for name in REQUIRED_SOURCE_FILES}}


def font_coverage(record):
    from fontTools.pens.boundsPen import BoundsPen
    from fontTools.ttLib import TTFont

    root = Path(record["source_root"])
    labels = read_json(root / "assets/i18n/startup.json")
    manifest = read_json(root / "assets/fonts/startup-manifest.json")
    font_path = root / "assets/fonts/StartupSansSC.otf"
    require(digest(font_path) == manifest["output"]["sha256"], "Bootstrap font provenance mismatch")
    require(all(isinstance(value, str) and value for value in labels.values()), "Empty startup localization")
    codepoints = {ord(character) for value in labels.values() for character in value if not character.isspace()}
    font = TTFont(io.BytesIO(font_path.read_bytes()), recalcTimestamp=False)
    try:
        cmap = font.getBestCmap()
        require(codepoints.issubset(cmap), "The bootstrap font omits a localized startup character")
        glyphs = font.getGlyphSet()
        for codepoint in sorted(codepoints):
            name = cmap[codepoint]
            require(name != ".notdef", "A localized startup character resolves to .notdef")
            pen = BoundsPen(glyphs)
            glyphs[name].draw(pen)
            require(pen.bounds is not None, "A localized startup character has no visible outline")
        family = font["name"].getDebugName(1)
        require(family == "BiliComics Startup", "The bootstrap font family changed")
    finally:
        font.close()
    return {"family": family, "font_sha256": digest(font_path), "required_codepoints": len(codepoints),
            "missing_codepoints": [], "empty_required_outlines": [],
            "localization_keys": sorted(labels), "text_and_clipping_visual_review": "pending"}


def image_record(path, source):
    with Image.open(path) as image:
        require(image.convert("RGBA").getchannel("A").getextrema() == (255, 255),
                "A supposedly opaque application frame has transparent pixels")
        rgb = image.convert("RGB")
        return {"file": path.name, "sha256": digest(path), "size": list(rgb.size),
                "rgb_sha256": hashlib.sha256(rgb.tobytes()).hexdigest(), "source": source,
                "pixel_processing": "None; original PNG retained unchanged"}


def region_box(profile, name):
    width, height = PROFILES[profile]
    content_width = min(440, width - 48)
    left = (width - content_width) / 2
    top = max(24, height / 2 - 150)
    rectangles = {
        "title": (left, top, left + content_width, top + 56),
        "loading": (left, top + 80, left + content_width, top + 120),
        "phase": (left, top + 132, left + content_width, top + 180),
        "failure_body": (left, top + 132, left + content_width, top + 204),
        "indicator": ((width - 120) / 2, top + 212, (width + 120) / 2, top + 224),
    }
    return tuple(round(value * SCALE) for value in rectangles[name])


def ink_count(image, box):
    return sum(min(pixel) < 245 for pixel in image.crop(box).convert("RGB").getdata())


def inspect_case(config, returncode, app_pid, ready, stable):
    directory = Path(config["directory"])
    first = read_json(directory / "startup-0.json")
    second = read_json(directory / "startup-1.json")
    final = read_json(directory / "final.json")
    expected_size = tuple(value * SCALE for value in PROFILES[config["profile"]])
    require(first["process_id"] == second["process_id"] == final["process_id"] == app_pid,
            "Startup and final evidence do not belong to the same application process")
    require(first["worker_started"] is False and second["worker_started"] is True,
            "The first startup frame did not precede worker admission")
    require(first["booting"] is True and second["booting"] is True,
            "Startup capture occurred after application attachment")
    require(first["animation_frame"] != second["animation_frame"], "The startup indicator did not advance")
    require(not first["error"] and not second["error"], "The activity evidence already shows failure")
    require(final["profile"] == config["profile"] and final["backend"] == "desktop-software",
            "The final capture has another profile or backend")
    require(final["demo"] is True and final["offline"] is True and final["offline_requested"] is True,
            "The case escaped isolated offline demonstration mode")
    require(final["logical_size"] == list(PROFILES[config["profile"]])
            and final["window_size"] == list(expected_size), "The live window did not use scale factor two")
    require(final["native_epdc_mode"] is None, "A desktop case acquired native output state")
    pictures = {}
    for name in ("startup-0.png", "startup-1.png", "final.png"):
        path = directory / name
        picture = image_record(path, "Real Rust/Slint window.take_snapshot")
        require(tuple(picture["size"]) == expected_size, "A captured PNG has unexpected geometry")
        pictures[name] = picture
    with Image.open(directory / "startup-0.png") as initial, Image.open(directory / "startup-1.png") as activity:
        initial = initial.convert("RGB")
        activity = activity.convert("RGB")
        box = region_box(config["profile"], "indicator")
        require(ImageChops.difference(initial.crop(box), activity.crop(box)).getbbox() is not None,
                "Indicator metadata advanced without changed rendered pixels")
        text_regions = {}
        for name in ("title", "loading", "phase"):
            count = ink_count(initial, region_box(config["profile"], name))
            require(count > 20, "The initial startup text region is empty: " + name)
            text_regions[name] = {"ink_pixels": count, "box": list(region_box(config["profile"], name))}
    if config["scenario"] == "failure":
        actual_ready = [row for row in load_records(directory / "lifecycle.jsonl")
                        if row.get("event") == "ready" and row.get("process_id") == app_pid]
        require(returncode != 0 and ready is None and not actual_ready,
                "Controlled startup failure was reported as ready")
        error_log = (directory / "application.log").read_text(encoding="utf-8")
        require("Application preparation failed" in error_log
                and "Create the application data directory" in error_log
                and "Not a directory" in error_log,
                "The error view did not result from the controlled non-directory input")
        require(final["route"] == "startup" and final["startup"]["booting"] is True
                and final["startup"]["error"] is True, "Failure did not retain the real startup error view")
        with Image.open(directory / "final.png") as failure:
            require(ink_count(failure, region_box(config["profile"], "indicator")) == 0,
                    "The activity indicator survived in the failure view")
            require(ink_count(failure, region_box(config["profile"], "failure_body")) > 20,
                    "The failure explanation did not render")
    else:
        require(returncode == 0 and ready is not None, "The normal startup did not complete successfully")
        require(final["route"] == "shelf" and final["startup"]["booting"] is False
                and final["startup"]["error"] is False, "The normal case did not attach the shelf")
        require(stable and stable["identical_samples"] >= 3
                and stable["minimum_settle_seconds"] >= 1.5, "No stable actual X11 frame was recorded")
        pictures["final-x11.png"] = image_record(directory / "final-x11.png", "Actual X11 application window")
    return {"passed": True, "exit_code": returncode, "application_pid": app_pid,
            "first_frame": first, "activity_frame": second, "final_state": final,
            "startup_text_regions": text_regions, "indicator_pixels_changed": True,
            "stable_x11": stable, "images": pictures, "ready_record": ready,
            "visual_review": {"status": "pending", "checks": [
                "All Chinese startup and error text is legible and unclipped",
                "The final shelf is complete and has the expected typography and covers"]}}


def stop_owned_process(process):
    if process is not None and process.poll() is None:
        # This unreaped direct child shares the private outer case group.
        process.terminate()
        try:
            process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=5)


def process_identity(pid):
    try:
        raw = Path("/proc", str(pid), "stat").read_bytes()
    except (FileNotFoundError, ProcessLookupError):
        return None
    head, separator, tail = raw.rpartition(b") ")
    fields = tail.split()
    require(separator and len(fields) >= 20, "Malformed process ownership receipt")
    return {"pid": pid, "state": fields[0].decode("ascii"),
            "group": int(fields[2]), "birth": int(fields[19])}


def has_case_marker(pid, nonce):
    try:
        environment = Path("/proc", str(pid), "environ").read_bytes().split(b"\0")
    except (FileNotFoundError, ProcessLookupError):
        return False
    return (CASE_MARKER + "=" + nonce).encode("ascii") in environment


def owned_group_members(group, nonce):
    members = []
    for path in Path("/proc").iterdir():
        if not path.name.isdigit():
            continue
        record = process_identity(int(path.name))
        if record is None or record["group"] != group or record["state"] == "Z":
            continue
        if not has_case_marker(record["pid"], nonce):
            deadline = time.monotonic() + 1
            while True:
                current = process_identity(record["pid"])
                if (current is None or current["state"] == "Z" or current["birth"] != record["birth"]
                        or current["group"] != group):
                    record = None
                    break
                if has_case_marker(record["pid"], nonce):
                    break
                require(time.monotonic() < deadline,
                        "A live member of the private case group lacks its ownership marker")
                time.sleep(0.01)
            if record is None:
                continue
        members.append(record)
    return members


def signal_owned_member(record, nonce, selected_signal):
    try:
        descriptor = os.pidfd_open(record["pid"])
    except ProcessLookupError:
        return
    try:
        current = process_identity(record["pid"])
        if (current is None or current["state"] == "Z" or current["birth"] != record["birth"]
                or current["group"] != record["group"]):
            return
        deadline = time.monotonic() + 1
        while not has_case_marker(current["pid"], nonce):
            current = process_identity(record["pid"])
            if (current is None or current["state"] == "Z" or current["birth"] != record["birth"]
                    or current["group"] != record["group"]):
                return
            require(time.monotonic() < deadline, "Process ownership marker changed before cleanup")
            time.sleep(0.01)
        try:
            signal.pidfd_send_signal(descriptor, selected_signal)
        except ProcessLookupError:
            pass
    finally:
        os.close(descriptor)


def close_case_group(process, config):
    group = process.pid
    nonce = config["case_nonce"]
    receipt = {"group": group, "case_id": config["case_id"], "initial_members": [],
               "signalled": [], "remaining_members": None, "passed": False}
    try:
        first = owned_group_members(group, nonce)
        receipt["initial_members"] = first
        for record in first:
            signal_owned_member(record, nonce, signal.SIGTERM)
            receipt["signalled"].append({**record, "signal": "SIGTERM"})
        deadline = time.monotonic() + 5
        remaining = owned_group_members(group, nonce)
        while remaining and time.monotonic() < deadline:
            time.sleep(0.05)
            remaining = owned_group_members(group, nonce)
        for record in remaining:
            signal_owned_member(record, nonce, signal.SIGKILL)
            receipt["signalled"].append({**record, "signal": "SIGKILL"})
        process.wait(timeout=5)
        deadline = time.monotonic() + 5
        remaining = owned_group_members(group, nonce)
        while remaining and time.monotonic() < deadline:
            time.sleep(0.05)
            remaining = owned_group_members(group, nonce)
        receipt["remaining_members"] = remaining
        require(not remaining, "The private Xvfb case group did not fully close")
        receipt["passed"] = True
        return receipt
    except Exception as error:
        receipt["error"] = str(error)
        raise
    finally:
        write_json(Path(config["directory"]) / "outer-cleanup.json", receipt)


def wait_for_wrapper_identity(process, nonce):
    deadline = time.monotonic() + 2
    observed = []
    while True:
        identity = process_identity(process.pid)
        marker = identity is not None and has_case_marker(process.pid, nonce)
        observed.append({"identity": identity, "marker_seen": marker})
        if identity is not None and identity["group"] == process.pid and marker:
            return {**identity, "observations": observed}
        require(process.poll() is None and time.monotonic() < deadline,
                "The private Xvfb wrapper did not establish its owned process group: " + json.dumps(observed))
        time.sleep(0.01)


def internal_case(path):
    config = read_json(path)
    directory = Path(config["directory"])
    process, pointer = None, None
    report = {"case_id": config["case_id"], "variant": config["variant"], "profile": config["profile"],
              "scenario": config["scenario"], "passed": False, "input_injected": False,
              "physical_device_used": False, "live_account_used": False,
              "binary_sha256": config["source"]["binary_sha256"],
              "source_commit": config["source"]["source_commit"], "harness_sha256": config["harness_sha256"],
              "source_manifest_sha256": config["source"]["source_manifest_sha256"],
              "source_archive_sha256": config["source"]["source_archive_sha256"],
              "case_manifest_sha256": digest(path)}
    try:
        require(sys.platform == "linux", "This verification is restricted to the remote Linux worker")
        require(digest(__file__) == config["harness_sha256"], "The selected harness changed")
        require(digest(config["source"]["binary"]) == config["source"]["binary_sha256"], "The selected binary changed")
        support = Path(config["support_script"])
        require(digest(support) == config["support_script_sha256"], "The X11 capture helper changed")
        spec = importlib.util.spec_from_file_location("selected_pointer_v2", support)
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        pointer = module.Pointer()
        environment = os.environ.copy()
        for name in list(environment):
            if name.startswith(("BILICOMICS_", "AIDOKU_", "SLINT_")):
                environment.pop(name, None)
        environment["SLINT_SCALE_FACTOR"] = str(SCALE)
        environment[CASE_MARKER] = config["case_nonce"]
        for name, leaf in (("XDG_DATA_HOME", "xdg-data"), ("XDG_CONFIG_HOME", "xdg-config"),
                           ("XDG_CACHE_HOME", "xdg-cache")):
            environment[name] = str(directory / leaf)
        data = directory / "data"
        if config["scenario"] == "failure":
            data.write_text("Controlled non-directory demonstration data input.\n", encoding="utf-8")
        command = ["unshare", "--net", "--", config["source"]["binary"], "--demo", "--offline",
                   "--device", config["profile"], "--screen", "shelf", "--data-dir", str(data),
                   "--startup-delay-ms", str(config["delay_ms"]),
                   "--capture-startup", str(directory / "startup.png"),
                   "--capture", str(directory / "final.png"),
                   "--lifecycle-trace", str(directory / "lifecycle.jsonl"), "--exit-after-ms", "6000"]
        report["command"] = command
        report["network_isolation"] = "Fresh unshare --net namespace plus --demo --offline"
        started = time.monotonic()
        ready, stable = None, None
        with (directory / "application.log").open("x", encoding="utf-8") as log:
            process = subprocess.Popen(command, stdout=log, stderr=subprocess.STDOUT,
                                       env=environment)
            if config["scenario"] == "normal":
                while time.monotonic() - started < 35:
                    require(process.poll() is None, "The application exited before real readiness")
                    ready = next((row for row in reversed(load_records(directory / "lifecycle.jsonl"))
                                  if row.get("event") == "ready" and row.get("process_id") == process.pid), None)
                    if ready:
                        break
                    time.sleep(0.05)
                require(ready is not None, "No same-process ready record appeared")
                ready_observed = time.monotonic()
                window = None
                while time.monotonic() - ready_observed < 5:
                    window = pointer.window()
                    if window:
                        break
                    time.sleep(0.05)
                require(window is not None, "The private X11 application window was not mapped")
                require((window["width"], window["height"]) == tuple(value * SCALE for value in PROFILES[config["profile"]]),
                        "The actual X11 window has unexpected geometry")
                samples, previous, identical = [], None, 0
                (directory / "settle").mkdir()
                time.sleep(max(0, 1.5 - (time.monotonic() - ready_observed)))
                while time.monotonic() - ready_observed < 5:
                    require(process.poll() is None, "The application exited before stable X11 evidence")
                    target = directory / "settle" / (str(len(samples)).zfill(3) + ".png")
                    captured = pointer.capture(window, target)
                    identical = identical + 1 if captured["sha256"] == previous else 1
                    previous = captured["sha256"]
                    samples.append({"file": str(target.relative_to(directory)), "sha256": previous,
                                    "seconds_after_ready_observed": round(time.monotonic() - ready_observed, 6)})
                    if identical >= 3:
                        shutil.copyfile(target, directory / "final-x11.png")
                        stable = {"minimum_settle_seconds": 1.5, "identical_samples": identical,
                                  "window": window, "samples": samples}
                        break
                    time.sleep(0.35)
                require(stable is not None, "The actual X11 shelf did not stabilize before the bounded exit")
            returncode = process.wait(timeout=max(1, 45 - (time.monotonic() - started)))
        report.update(inspect_case(config, returncode, process.pid, ready, stable))
        report["elapsed_seconds_including_controlled_delay"] = round(time.monotonic() - started, 6)
        for image in directory.rglob("*.png"):
            image.chmod(0o444)
        write_json(directory / "result.json", report)
        return 0
    except Exception as error:
        report["error"] = str(error)
        report["traceback"] = traceback.format_exc()
        write_json(directory / "failure.json", report)
        return 1
    finally:
        stop_owned_process(process)
        if pointer is not None:
            pointer.close()


def compare_images(output, left, right, reason):
    with Image.open(output / left) as first, Image.open(output / right) as second:
        require(first.size == second.size, "Compared PNG geometry differs: " + reason)
        difference = ImageChops.difference(first.convert("RGB"), second.convert("RGB"))
        exact = difference.getbbox() is None
        result = {"left": str(left), "right": str(right), "reason": reason, "pixel_exact": exact,
                  "left_sha256": digest(output / left), "right_sha256": digest(output / right),
                  "compared_pixels": first.width * first.height,
                  "difference_bbox": difference.getbbox()}
    require(exact, "Original rendered RGB pixels differ: " + reason)
    return result


def main(arguments):
    require(sys.platform == "linux", "Run this verification only on test-env")
    require(hasattr(os, "pidfd_open") and hasattr(signal, "pidfd_send_signal"),
            "Owned case cleanup requires Linux pidfd support in this Python runtime")
    require(digest(__file__) == arguments.expected_harness_sha256, "The selected harness hash differs")
    require(1200 <= arguments.delay_ms <= 2000, "Use a controlled delay between 1200 and 2000 ms")
    inputs = read_json(arguments.inputs)
    require(set(inputs) == set(VARIANTS), "Select exactly baseline and candidate inputs")
    source_checks, font_checks = {}, {}
    for variant, source in inputs.items():
        require(set(source) == set(SOURCE_FIELDS), "Input fields are missing or unexpected: " + variant)
        require(re.fullmatch(r"[0-9a-f]{40}", source["source_commit"]), "A full source commit is required")
        for name in ("binary_sha256", "source_manifest_sha256", "source_archive_sha256"):
            require(re.fullmatch(r"[0-9a-f]{64}", source[name]), "Invalid selected SHA-256: " + name)
        require(digest(source["binary"]) == source["binary_sha256"], "Selected executable hash mismatch")
        source_checks[variant] = source_inventory(source)
        font_checks[variant] = font_coverage(source)
    require(font_checks["baseline"]["font_sha256"] == font_checks["candidate"]["font_sha256"],
            "The baseline and candidate bootstrap fonts are not identical")
    support = Path(inputs["candidate"]["source_root"]) / "scripts/verification/pointer_v2.py"
    support_hash = digest(support)
    output = arguments.output.resolve()
    output.mkdir(mode=0o700, parents=True, exist_ok=False)
    report = {"schema_version": 1, "passed": False,
              "created_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
              "scope": "Real Rust/Slint offline demo desktop startup and raw RGB comparison",
              "physical_device_used": False, "live_account_used": False, "input_injected": False,
              "startup_performance_benchmark": False, "scale_factor": SCALE,
              "controlled_state_worker_delay_ms": arguments.delay_ms,
              "harness_sha256": arguments.expected_harness_sha256,
              "selection_sha256": digest(arguments.inputs), "inputs": inputs,
              "source_verification": source_checks, "bootstrap_font_coverage": font_checks,
              "capture_dependency": {"path": str(support), "sha256": support_hash},
              "cases": [], "comparisons": [],
              "limitations": ["No native driver, waveform, SEND, or panel completion was measured",
                              "Controlled-delay desktop durations are not physical startup performance",
                              "Character outlines and pixel identity do not replace visual clipping review"]}
    configs = []
    for profile in PROFILES:
        for scenario in SCENARIOS:
            for variant in VARIANTS:
                case_id = "-".join((variant, profile, scenario))
                directory = output / case_id
                directory.mkdir(mode=0o700)
                config = {"case_id": case_id, "variant": variant, "profile": profile, "scenario": scenario,
                          "directory": str(directory), "source": inputs[variant], "delay_ms": arguments.delay_ms,
                          "support_script": str(support), "support_script_sha256": support_hash,
                          "harness_sha256": arguments.expected_harness_sha256, "case_nonce": uuid.uuid4().hex}
                path = directory / "case.json"
                write_json(path, config)
                configs.append(path)

    def run_case(path):
        config = read_json(path)
        command = ["xvfb-run", "-a", "-s", "-screen 0 2800x2800x24 -nolisten tcp",
                   sys.executable, str(Path(__file__).resolve()), "--internal-case", str(path)]
        environment = os.environ.copy()
        environment[CASE_MARKER] = config["case_nonce"]
        with (path.parent / "harness.log").open("x", encoding="utf-8") as log:
            wrapper = subprocess.Popen(command, stdout=log, stderr=subprocess.STDOUT,
                                       env=environment, start_new_session=True)
            try:
                wrapper_identity = wait_for_wrapper_identity(wrapper, config["case_nonce"])
                write_json(path.parent / "outer-process.json", wrapper_identity)
                returncode = wrapper.wait(timeout=70)
            except Exception as error:
                write_json(path.parent / "outer-failure.json", {"case_id": config["case_id"],
                           "error": str(error), "traceback": traceback.format_exc(),
                           "harness_sha256": arguments.expected_harness_sha256})
                raise
            finally:
                cleanup = close_case_group(wrapper, config)
        require(returncode == 0 and (path.parent / "result.json").is_file(),
                "Desktop case failed; inspect " + str(path.parent / "failure.json"))
        record = read_json(path.parent / "result.json")
        record["outer_cleanup"] = cleanup
        require(record.get("passed") is True, "A desktop case did not pass")
        print(json.dumps({"case_id": record["case_id"], "passed": True,
                          "profile": record["profile"], "scenario": record["scenario"]}), flush=True)
        return record

    try:
        with concurrent.futures.ThreadPoolExecutor(max_workers=arguments.jobs) as executor:
            futures = {executor.submit(run_case, path): path for path in configs}
            for future in concurrent.futures.as_completed(futures):
                try:
                    report["cases"].append(future.result())
                except Exception as error:
                    report["cases"].append({"case_id": futures[future].parent.name,
                                             "passed": False, "error": str(error)})
        report["cases"].sort(key=lambda case: case["case_id"])
        require(len(report["cases"]) == 8 and all(case.get("passed") for case in report["cases"]),
                "One or more desktop cases failed; all available case receipts are retained")
        for profile in PROFILES:
            pairs = [(Path("baseline-" + profile + "-normal/startup-0.png"),
                      Path("candidate-" + profile + "-normal/startup-0.png"), "initial startup before workers"),
                     (Path("baseline-" + profile + "-failure/startup-0.png"),
                      Path("candidate-" + profile + "-failure/startup-0.png"), "initial failure-case startup before workers"),
                     (Path("baseline-" + profile + "-normal/final.png"),
                      Path("candidate-" + profile + "-normal/final.png"), "final shelf Slint snapshot"),
                     (Path("baseline-" + profile + "-normal/final-x11.png"),
                      Path("candidate-" + profile + "-normal/final-x11.png"), "stable actual X11 shelf"),
                     (Path("baseline-" + profile + "-failure/final.png"),
                      Path("candidate-" + profile + "-failure/final.png"), "retained error view")]
            for left, right, reason in pairs:
                report["comparisons"].append(compare_images(output, left, right, profile + ": " + reason))
        for variant in VARIANTS:
            source_inventory(inputs[variant])
            require(digest(inputs[variant]["binary"]) == inputs[variant]["binary_sha256"],
                    "An executable changed during verification")
        require(digest(__file__) == arguments.expected_harness_sha256, "The harness changed during verification")
        report["passed"] = len(report["cases"]) == 8 and len(report["comparisons"]) == 10
        report["visual_review"] = {"status": "pending", "original_pngs_retained": True}
        write_json(output / "results.json", report)
    except Exception as error:
        report["error"] = str(error)
        report["traceback"] = traceback.format_exc()
        write_json(output / "failure.json", report)
        raise


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--inputs", type=Path)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--expected-harness-sha256")
    parser.add_argument("--delay-ms", type=int, default=1800)
    parser.add_argument("--jobs", type=int, choices=(1, 2), default=2)
    parser.add_argument("--internal-case", type=Path, help=argparse.SUPPRESS)
    selected = parser.parse_args()
    if selected.internal_case:
        sys.exit(internal_case(selected.internal_case))
    require(selected.inputs is not None and selected.output is not None
            and selected.expected_harness_sha256 is not None, "Select inputs, a fresh output, and a harness hash")
    main(selected)
