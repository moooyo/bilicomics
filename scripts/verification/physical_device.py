#!/usr/bin/env python3
"""Coordinate one supervised, isolated physical Kindle run from the Linux verification host.

This program never acquires the shared device lease, injects touch or power
events, authenticates to an account, or promotes a runtime pair to a support
claim. The coordinator must obtain and return the exclusive lease separately.
"""

from __future__ import annotations

import argparse
import hashlib
import io
import json
import math
import os
from pathlib import Path, PurePosixPath
import re
import secrets
import shlex
import stat
import struct
import subprocess
import sys
import tarfile
import time
import zipfile


LOCK = "/tmp/bilicomics-launcher.lock"
GEOMETRIES = {"pw5": (1236, 1648), "pw6": (1264, 1680), "scribe": (1860, 2480), "scribe2025": (1980, 2640)}
IDENTITY_FIELDS = ("comic_id", "episode_id", "revision", "reader_generation", "route", "launch_preference",
                   "mode", "core_mode", "fit", "direction", "rotation", "landscape_layout", "cover_alone",
                   "dither", "footer", "demo_strip")
ANCHOR_FIELDS = ("page_id", "page_index", "crop_index", "crop_count", "source_x", "source_y", "zoom_ratio")
SAFE_LOGS = ("launcher.log", "application.log", "watchdog.log", "recovery.log", "lifecycle.jsonl", "performance.jsonl",
             "ui-socket-guard.json",
             "native-baseline.tsv", "awesome-after.tsv", "statusbar-before.txt", "statusbar-after.txt",
             "launcher-lock-after.txt", "owned-processes.tsv", "observer-group-members.tsv",
             "observer-group-proofs.tsv", "launcher-timing.tsv")
EPDC_MODES = ("Y8", "Y8INV")
INVERT_BYTES = bytes(255 - value for value in range(256))
CONTENT_MODES = ("demo", "distinct_fixture")
GUARD_FIELDS = frozenset(("offline_socket_guard", "schema_version", "mode", "process_id", "uid", "gid",
    "effective_uid", "effective_gid", "no_new_privs", "seccomp_mode", "installed", "checks_passed", "audit_arch",
    "inherited_non_unix_closed", "inherited_unix_preserved", "inherited_non_socket_preserved", "denied_socket_mask",
    "denied_socketpair_mask", "unix_socket_allowed", "unix_socketpair_allowed", "socketcall_checked",
    "socketcall_denied", "x32_checked", "x32_denied"))


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def file_hash(path):
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def digest_argument(value):
    if re.fullmatch(r"[0-9a-f]{64}", value) is None:
        raise argparse.ArgumentTypeError("A full lowercase SHA-256 is required.")
    return value


def socket_guard_proof(raw, mode, pid=None):
    """Accept only the pinned ARM guard's complete, numeric success schema."""
    require(len(raw) <= 8192, "The socket guard proof exceeds its size bound.")
    value = json.loads(raw)
    require(isinstance(value, dict) and set(value) == GUARD_FIELDS
            and all(type(item) is int and item >= 0 for item in value.values()), "Invalid socket guard proof schema.")
    fixed = {"offline_socket_guard": 1, "schema_version": 1, "mode": mode, "installed": 1,
             "checks_passed": 1, "seccomp_mode": 2, "audit_arch": 1073741864,
             "denied_socket_mask": 63, "denied_socketpair_mask": 63, "unix_socket_allowed": 1,
             "unix_socketpair_allowed": 1, "socketcall_checked": 1, "socketcall_denied": 1,
             "x32_checked": 0, "x32_denied": 0, "uid": 0, "effective_uid": 0}
    require(all(value.get(key) == item for key, item in fixed.items()) and value["process_id"] > 1
            and value["no_new_privs"] in (0, 1) and (pid is None or value["process_id"] == pid),
            "The ARM socket guard did not establish this process's required isolation.")
    return value


def fixture_inputs(arguments):
    """Validate opt-in inputs before any new device directory is created."""
    mode = getattr(arguments, "content_mode", "demo")
    require(mode in CONTENT_MODES, "Unknown physical content mode.")
    fields = ("fixture_directory", "expected_fixture_sha256", "socket_guard", "expected_guard_sha256")
    supplied = [getattr(arguments, field, None) for field in fields]
    if mode == "demo":
        require(not any(value is not None for value in supplied), "Fixture or guard inputs require explicit distinct_fixture mode.")
        return None
    require(all(value is not None for value in supplied), "Distinct fixtures require a pinned fixture and ARM socket guard.")
    directory = arguments.fixture_directory
    require(directory.is_dir() and not directory.is_symlink(), "The fixture input must be an independent directory.")
    from physical.distinct_reader_fixture import validate_fixture
    report = validate_fixture(directory)
    require(report.get("status") == "passed" and report.get("manifest_sha256") == arguments.expected_fixture_sha256,
            "The fixture manifest differs from the selected exact SHA-256.")
    manifest = report["manifest"]
    require(manifest.get("kind") == "distinct_offline_guest_fixture" and manifest.get("account_key") == "guest",
            "Only the controlled guest fixture is accepted.")
    guard = arguments.socket_guard
    require(guard.is_file() and not guard.is_symlink(), "The socket guard must be a regular ARM input.")
    guard_data = guard.read_bytes()
    arm_elf(guard_data)
    require(sha256(guard_data) == arguments.expected_guard_sha256, "The socket guard digest differs.")
    files = {}
    for item in manifest["files"]:
        relative = PurePosixPath(item["path"])
        require(not relative.is_absolute() and ".." not in relative.parts and relative.parts[0] == "data",
                "A fixture input escaped the isolated data directory.")
        source = directory / str(relative)
        data = source.read_bytes()
        require(len(data) == item["bytes"] and sha256(data) == item["sha256"], "A fixture input changed after validation.")
        require(str(relative) not in files, "A fixture input path is duplicated.")
        files[str(relative)] = (data, 0o600)
    return {"manifest": manifest, "manifest_sha256": report["manifest_sha256"], "files": files,
            "guard": guard_data, "guard_sha256": arguments.expected_guard_sha256}


def fixture_reader_snapshot(values, manifest, kind):
    """Use real non-demo reader snapshots, never a fabricated demo UI trace."""
    require(kind in ("ready", "reader", "suspended", "resumed"), "Unknown fixture checkpoint kind.")
    require(any(value.get("event") == "ready" for value in values), "The fixture application is not ready.")
    ready_index = next((index for index in range(len(values) - 1, -1, -1)
                        if values[index].get("event") == "reader-ready"), None)
    require(ready_index is not None, "The fixture reader has not published a ready state.")
    ready = values[ready_index]
    require(not any(value.get("event") in ("reader-closed", "quit") for value in values[ready_index + 1:]),
            "A closed reader cannot reuse an old fixture ready record.")
    power = next((value for value in reversed(values) if value.get("event") in ("suspend", "resume")), None)
    suspended = power is not None and power.get("event") == "suspend"
    require(suspended == (kind == "suspended") and (not suspended or power.get("suspended") is True),
            "The fixture power state differs from the requested checkpoint.")
    if kind == "resumed":
        require(power and power.get("event") == "resume" and power.get("suspended") is False,
                "The fixture has not resumed after a native sleep transition.")
    state_record = next((value for value in reversed(values) if isinstance(value.get("reader_state"), dict)), None)
    require(state_record is not None and ready.get("reader_loading") is False and ready.get("route") == "reader",
            "The fixture reader has no current complete page.")
    reader = state_record["reader_state"]
    expected = {"comic_id": manifest["comic_id"], "episode_id": manifest["episode_id"], "revision": manifest["revision"],
                "route": "reader", "mode": "page", "core_mode": "page", "fit": "page", "direction": "ltr",
                "landscape_layout": "single", "rotation": 0, "zoom_ratio": 1.0, "source_x": 0.0, "source_y": 0.0,
                "crop_index": 0, "crop_count": 1, "demo_strip": False, "effective_spread": False,
                "transition": False, "menu": False, "sheet": "", "rendering": False}
    require(all(reader.get(key) == value for key, value in expected.items()),
            "The fixture is outside the supported quiet portrait Page/FitPage state.")
    require(all(type(reader.get(key)) is bool for key in ("demo_strip", "effective_spread", "transition", "menu", "rendering"))
            and all(type(reader.get(key)) is int and reader[key] > 0 for key in ("reader_generation", "render_version")),
            "The fixture state lacks typed lifecycle and presentation identities.")
    page_index = reader.get("page_index")
    require(type(page_index) is int and 0 <= page_index < len(manifest["pages"]), "Unknown fixture page index.")
    page = manifest["pages"][page_index]
    require(reader.get("page_id") == page["page_id"] and reader.get("page_number") == page_index + 1,
            "The current page does not match the pinned fixture descriptor.")
    frame = [reader.get("frame_width"), reader.get("frame_height")]
    viewport = [reader.get("viewport_width"), reader.get("viewport_height")]
    require(all(type(value) is int and value > 0 for value in frame + viewport)
            and frame == viewport
            and viewport[0] < viewport[1] and reader.get("second_frame_width") == reader.get("second_frame_height") == 0
            and ready.get("frame_size") == frame,
            "The fixture frame or viewport geometry is unavailable or unsupported.")
    resources = ready.get("reader_resources") or {}
    require(resources.get("reader_active") is True and resources.get("reader_generation") == reader["reader_generation"]
            and resources.get("page_size") == {"width": frame[0], "height": frame[1]},
            "The fixture resource snapshot does not match the presented page.")
    require(all(ready.get("reader_state", {}).get(field) == reader.get(field)
                for field in IDENTITY_FIELDS + ANCHOR_FIELDS + ("render_version", "menu", "sheet", "rendering")),
            "The latest fixture ready record is stale relative to its lifecycle state.")
    return {"reader_state": reader, "ready": ready, "state_record": state_record, "latest_power": power, "page": page}


def fixture_publication(values, snapshot, pid):
    """Keep deferred evidence distinct when this frozen ARM omitted render_request."""
    records = [value for value in values if value.get("process_id") == pid]
    published = next((value for value in reversed(records)
                      if value.get("kind") == "reader" and value.get("stage") == "reader_model_published"), None)
    require(published is not None and type(published.get("operation_id")) is int,
            "The fixture has no completed reader model publication.")
    operation = published["operation_id"]
    clock = published.get("monotonic_ns")
    require(type(clock) is int and clock > 0, "The fixture publication has no monotonic identity.")
    same = [value for value in records if value.get("kind") == "reader" and value.get("operation_id") == operation]
    request = next((value for value in reversed(same) if value.get("stage") == "render_request"), None)
    navigation = next((value for value in reversed(same) if value.get("stage") == "navigation_request"), None)
    reader = snapshot["reader_state"]
    if request is not None:
        data = request.get("data") or {}
        require(data.get("source_kind") == "receipt" and data.get("generation") == reader.get("reader_generation")
                and data.get("version") == reader.get("render_version"), "The fixture render request belongs to another page generation.")
        origin, binding = request, "render_request"
    else:
        require(navigation is not None and any(value.get("stage") == "render_deferred" for value in same),
                "A publication without render_request requires its own deferred navigation chain.")
        origin, binding = navigation, "deferred_without_render_request"
    # Resume can reannounce an unchanged retained model without producing a
    # new reader-ready hash. The snapshot already binds the current resume or
    # reader state to that ready image; use its current observation time here.
    require(type(origin.get("monotonic_ns")) is int and origin["monotonic_ns"] <= clock
            and type(snapshot["state_record"].get("monotonic_ns")) is int
            and clock <= snapshot["state_record"]["monotonic_ns"], "The fixture publication and current state chronology differ.")
    require(not any(value.get("kind") == "reader" and value.get("stage") in
                    ("navigation_request", "render_request", "render_deferred", "reader_transition")
                    and type(value.get("monotonic_ns")) is int and value["monotonic_ns"] > clock for value in records),
            "A newer reader request has not been published; the old page is not current evidence.")
    native_frames = [value for value in records if value.get("kind") == "native" and value.get("stage") == "frame"
                     and (value.get("data") or {}).get("reader_operation_id") == operation]
    submissions = []
    empty_frames = 0
    for frame in native_frames:
        frame_data = frame.get("data") or {}
        attempts = frame_data.get("refresh_ioctl_attempts")
        require(type(attempts) is int and attempts >= 0, "A fixture native frame has malformed submission accounting.")
        if attempts:
            submissions.append(frame)
        else:
            dirty = frame_data.get("dirty_region") or {}
            require(frame_data.get("refresh_accepted") is None and frame_data.get("refresh_ioctl_returned") is False
                    and frame_data.get("refresh_errno") is None and frame_data.get("refresh_region") is None
                    and (dirty.get("width") == 0 or dirty.get("height") == 0),
                    "A fixture frame changed content or reported an error without a refresh submission.")
            empty_frames += 1
    native = submissions[-1] if submissions else None
    require(native is not None, "The fixture publication has no native framebuffer submission.")
    data = native["data"]
    require(data.get("rendered") is True and data.get("refresh_accepted") is True and data.get("rotation_degrees") == 0
            and data.get("refresh_ioctl_returned") is True and type(data.get("refresh_ioctl_attempts")) is int
            and data["refresh_ioctl_attempts"] > 0 and data.get("refresh_errno") is None
            and data.get("framebuffer_bits_per_pixel") == 8 and type(native.get("monotonic_ns")) is int
            and native["monotonic_ns"] >= clock,
            "The latest fixture native frame submission is incomplete, rotated, or older than its model publication.")
    for stage in ("framebuffer_write", "refresh_submit"):
        timing = (data.get("stages") or {}).get(stage) or {}
        require(type(timing.get("started_monotonic_ns")) is int and type(timing.get("finished_monotonic_ns")) is int
                and clock <= timing["started_monotonic_ns"] <= timing["finished_monotonic_ns"] <= native["monotonic_ns"],
                "The fixture native frame has an incomplete stage.")
    return {"operation_id": operation, "binding": binding, "render_request_present": request is not None,
            "request_generation_version_verified": request is not None, "latency_origin": origin["stage"],
            "published_monotonic_ns": clock, "native_frame": data, "marker_required": True,
            "state_evidence_event": snapshot["state_record"].get("event"),
            "state_evidence_monotonic_ns": snapshot["state_record"]["monotonic_ns"],
            "empty_native_frames_excluded": empty_frames,
            "native_binding_scope": "Current model publication; later resource-only snapshots do not require a redraw",
            "physical_panel_update_confirmed": False}


def fixture_ready_hint(values, manifest, kind, before=None, expect="retained"):
    """An inexpensive wait hint; the full identity and pixel proof remain mandatory."""
    try:
        ready = next(value for value in reversed(values) if value.get("event") == "ready")
        pid = ready["process_id"]
        require(type(pid) is int and pid > 1, "Invalid fixture readiness PID.")
        snapshot = fixture_reader_snapshot([value for value in values if value.get("process_id") == pid], manifest, kind)
        if before is not None:
            old, current = before["reader_state"], snapshot["reader_state"]
            require(before["native_process"]["pid"] == pid and all(old.get(key) == current.get(key) for key in IDENTITY_FIELDS),
                    "Fixture identity is not retained.")
            if expect == "next-page":
                require(current["page_index"] == old["page_index"] + 1 and current["page_id"] != old["page_id"],
                        "The next fixture page is not ready.")
            else:
                require(all(old.get(key) == current.get(key) for key in ANCHOR_FIELDS), "The fixture anchor is not retained.")
        return True
    except (RuntimeError, StopIteration, KeyError, TypeError, ValueError):
        return False


def fixture_marker(geometry, projection, checkpoint, manifest):
    """Check one evidenced orientation and polarity; never search rotations or mirrors."""
    reader = checkpoint["reader_state"]
    orientation = checkpoint["native_process"].get("portrait_calibration") or {}
    native = checkpoint["fixture_publication"]["native_frame"]
    require(orientation.get("direction") == "U" and geometry.get("rotate") == orientation.get("raw_rotation")
            and native.get("rotation_degrees") == 0, "The fixture marker has no current upright orientation binding.")
    width, height = geometry["width"], geometry["height"]
    require(projection.get("status") == "generated" and projection.get("image_mode") == "L"
            and native.get("framebuffer_size") == [width, height]
            and native.get("framebuffer_stride") == geometry.get("line_length")
            and native.get("output_inverted") == (checkpoint["native_epdc_mode"] == "Y8INV")
            and [reader["viewport_width"], reader["viewport_height"]] == [width, height],
            "The fixture marker geometry or explicit EPDC polarity is unbound.")
    page = manifest["pages"][reader["page_index"]]
    marker = page["machine_marker"]
    require(marker.get("encoding") == "binary-lsb-first-v1" and marker.get("value") == reader["page_number"],
            "Unsupported fixture marker encoding.")
    require([reader["frame_width"], reader["frame_height"]] == [width, height],
            "The fixture raster must be the full viewport canvas.")
    # ReaderCore returns a full canvas. Its aspect-preserving image is fitted
    # inside that canvas with positive f64 rounding and integer centering.
    ratio = min(width / page["width"], height / page["height"])
    frame_width = min(width, max(1, math.floor(page["width"] * ratio + 0.5)))
    frame_height = min(height, max(1, math.floor(page["height"] * ratio + 0.5)))
    left, top = (width - frame_width) // 2, (height - frame_height) // 2
    pixels, samples = projection["pixels"], []
    for sample in marker["samples"]:
        x = round(left + sample["x"] * frame_width / page["width"])
        y = round(top + sample["y"] * frame_height / page["height"])
        require(2 <= x < width - 2 and 2 <= y < height - 2, "A fixture marker lies outside the visible frame.")
        observed = sorted(pixels[(y + dy) * width + x + dx] for dy in (-1, 0, 1) for dx in (-1, 0, 1))[4]
        require(sample["luma"] in (0, 255) and abs(observed - sample["luma"]) <= 34,
                "The framebuffer marker differs from the pinned source page or polarity.")
        samples.append({"x": x, "y": y, "expected_luma": sample["luma"], "observed_luma": observed})
    decoded = sum((1 << bit) for bit, index in enumerate(marker["bit_sample_indices"])
                  if samples[index]["observed_luma"] >= 128)
    require(decoded == page["page_number"] == reader["page_number"], "The framebuffer marker identifies a different fixture page.")
    return {"passed": True, "decoded_page_number": decoded, "samples": samples,
            "canvas_size": [width, height], "fit_image_size": [frame_width, frame_height],
            "fit_image_offset": [left, top],
            "mapping": "upright, unmirrored, centered Page/FitPage at the verified framebuffer geometry",
            "pixel_polarity_source": "current explicit native EPDC mode after declared framebuffer visual conversion",
            "fixture_manifest_sha256": checkpoint["fixture_manifest_sha256"], "physical_panel_update_confirmed": False}


def shell_line(*arguments):
    return shlex.join(str(argument) for argument in arguments)


def json_write(path, value):
    temporary = path.with_name(path.name + ".pending")
    temporary.write_text(json.dumps(value, indent=2, ensure_ascii=True) + "\n", encoding="utf-8")
    temporary.replace(path)


def normalized_script(path):
    require(path.is_file() and not path.is_symlink(), "A script input must be a regular file.")
    data = path.read_bytes().replace(b"\r\n", b"\n")
    require(b"\r" not in data and b"\0" not in data and data.startswith(b"#!/bin/sh\n"),
            "A device script must be a LF-terminated POSIX shell script.")
    return data


def arm_elf(data):
    require(len(data) >= 20 and data[:7] == b"\x7fELF\x01\x01\x01" and struct.unpack_from("<H", data, 18)[0] == 40,
            "The selected executable must be a little-endian 32-bit ARM ELF.")


def trace_epdc_mode(values):
    """Use the latest explicit process snapshot without guessing from older state."""
    for index in range(len(values) - 1, -1, -1):
        value = values[index]
        if "native_epdc_mode" not in value:
            continue
        mode = value["native_epdc_mode"]
        known = isinstance(mode, str) and mode in EPDC_MODES
        return {"mode": mode if known else None, "source": "native-application-trace",
                "trace_event": value.get("event"), "trace_record_index": index,
                "gap": None if known else "The latest explicit native EPDC snapshot is unknown."}
    return {"mode": None, "source": None, "trace_event": None, "trace_record_index": None,
            "gap": "The native application trace has no explicit EPDC mode snapshot."}


def checkpoint_ready_hint(values, kind, before=None, expect="retained"):
    """Gate costly polling with untrusted trace hints, never checkpoint evidence."""
    if (not isinstance(values, list) or kind not in ("ready", "reader", "suspended", "resumed")
            or not all(isinstance(value, dict) for value in values)):
        return False
    ready = next((value for value in reversed(values) if value.get("event") == "ready"), None)
    pid = ready.get("process_id") if ready else None
    if type(pid) is not int or pid <= 1:
        return False
    values = [value for value in values if value.get("process_id") == pid]
    ui = next((value for value in reversed(values) if value.get("event") == "ui-state"), None)
    if not ui or ui.get("demo") is not True or ui.get("view_model_fixture_only") is not False:
        return False
    state = ui.get("state")
    if not isinstance(state, dict):
        return False
    power = next((value for value in reversed(values) if value.get("event") in ("suspend", "resume")), None)
    if kind == "suspended":
        if not power or power.get("event") != "suspend" or power.get("suspended") is not True:
            return False
    elif power and power.get("event") == "suspend":
        return False
    if kind == "resumed" and (not power or power.get("event") != "resume" or power.get("suspended") is not False):
        return False
    reader = next((value["reader_state"] for value in reversed(values) if isinstance(value.get("reader_state"), dict)), None)
    if kind in ("reader", "resumed"):
        reader_ready = next((value for value in reversed(values) if value.get("event") == "reader-ready"), None)
        frame = reader_ready.get("frame_size") if reader_ready else None
        ui_reader = state.get("reader")
        if (not reader_ready or reader_ready.get("reader_loading") is not False
                or not isinstance(frame, list) or len(frame) != 2 or not all(type(size) is int and size > 0 for size in frame)
                or not reader or reader.get("route") != "reader" or state.get("route") != "reader"
                or not isinstance(ui_reader, dict) or ui_reader.get("loading") is not False):
            return False
        ready_reader = reader_ready.get("reader_state")
        if not isinstance(ready_reader, dict) or any(ready_reader.get(field) != reader.get(field) for field in IDENTITY_FIELDS + ANCHOR_FIELDS):
            return False
    if before is not None:
        if not isinstance(before, dict) or not isinstance(before.get("native_process"), dict):
            return False
        old = before.get("reader_state")
        if before["native_process"].get("pid") != pid or not isinstance(old, dict) or not reader:
            return False
        if any(old.get(field) != reader.get(field) for field in IDENTITY_FIELDS):
            return False
        if expect == "next-page":
            if (type(old.get("page_index")) is not int or reader.get("page_index") != old["page_index"] + 1
                    or reader.get("page_id") == old.get("page_id")):
                return False
        elif any(old.get(field) != reader.get(field) for field in ANCHOR_FIELDS):
            return False
    return True


def framebuffer_memory_pixels(geometry, pixels):
    """Decode the declared framebuffer visual, without applying the EPDC mode."""
    width, height, bits = geometry["width"], geometry["height"], geometry["bits_per_pixel"]
    require(width > 0 and height > 0 and bits in (8, 16, 24, 32), "Unsupported framebuffer pixel geometry.")
    raw = bytes(pixels)
    require(len(raw) == width * height * (bits // 8), "The framebuffer pixel buffer is incomplete.")
    if bits == 8:
        require(geometry["grayscale"] == 1 or geometry["visual"] in (0, 1),
                "An unconfirmed 8-bit palette cannot be converted as grayscale.")
        visual_inverted = geometry["visual"] == 0
        converted = raw.translate(INVERT_BYTES) if visual_inverted else raw
        return {"image_mode": "L", "pixels": converted,
                "conversion": "linear grayscale; MONO01 inverted when declared",
                "framebuffer_visual_inversion_applied": visual_inverted,
                "native_epdc_mode_applied": False}
    require(geometry["byte_order"] in ("little", "big"), "Unsupported framebuffer byte order.")
    require(all(geometry[color + "_msb_right"] == 0 and 0 < geometry[color + "_length"] <= 8
                and 0 <= geometry[color + "_offset"]
                and geometry[color + "_offset"] + geometry[color + "_length"] <= bits
                for color in ("red", "green", "blue")), "Unsupported packed-color channel layout.")
    converted = bytearray(width * height * 3)
    stride = bits // 8
    for index in range(width * height):
        value = int.from_bytes(raw[index * stride:(index + 1) * stride], geometry["byte_order"])
        for channel, color in enumerate(("red", "green", "blue")):
            maximum = (1 << geometry[color + "_length"]) - 1
            converted[index * 3 + channel] = ((value >> geometry[color + "_offset"]) & maximum) * 255 // maximum
    return {"image_mode": "RGB", "pixels": bytes(converted),
            "conversion": "declared packed-color framebuffer bitfields",
            "framebuffer_visual_inversion_applied": False,
            "native_epdc_mode_applied": False}


def framebuffer_panel_projection(geometry, memory, native_epdc_mode):
    """Project a verified 8-bit EPDC mode as a separate step after visual decoding."""
    record = {"status": "unavailable", "pixels": None, "image_mode": None,
              "native_epdc_mode": native_epdc_mode if native_epdc_mode in EPDC_MODES else None,
              "epdc_inversion_applied": False,
              "input_semantics": "framebuffer memory after the declared visual conversion",
              "conversion_order": ["declared framebuffer visual", "verified native EPDC mode"],
              "application_software_inversion_reapplied": False,
              "evidence": "derived EPDC-mode projection of framebuffer memory",
              "physical_panel_update_confirmed": False, "panel_photograph": False}
    if geometry["bits_per_pixel"] != 8 or memory["image_mode"] != "L":
        record["gap"] = "Panel projection is limited to declared 8-bit grayscale framebuffers."
        return record
    if native_epdc_mode not in EPDC_MODES:
        record["gap"] = "A verified native application EPDC mode snapshot is unavailable."
        return record
    inverted = native_epdc_mode == "Y8INV"
    pixels = bytes(memory["pixels"])
    record.update({"status": "generated", "pixels": pixels.translate(INVERT_BYTES) if inverted else pixels,
                   "image_mode": "L", "epdc_inversion_applied": inverted, "gap": None})
    return record


class Harness:
    def __init__(self, arguments):
        self.args = arguments
        require(sys.platform == "linux", "Run this harness on ssh test-env, not on the operator's Windows machine.")
        require(re.fullmatch(r"[a-z0-9][a-z0-9-]{0,47}", arguments.run_id) is not None, "Unsafe isolated run ID.")
        self.root = "/mnt/us/bilicomics-physical-" + arguments.run_id
        self.logs = self.root + "/data/logs"
        self.output = arguments.output.resolve()
        self.output.mkdir(parents=True, exist_ok=True)
        self.state_path = self.output / "run.json"
        self.state = json.loads(self.state_path.read_text()) if self.state_path.exists() else {}
        if self.state:
            require(self.state.get("run_root") == self.root, "The output directory belongs to another run.")
        self.check_lease()

    def check_lease(self):
        lease = self.args.lease_dir.resolve(strict=True)
        require(lease.is_dir() and not self.args.lease_dir.is_symlink(), "The shared lease must be a coordinator-owned directory.")
        owner_path = lease / "owner.json"
        require(owner_path.is_file() and not owner_path.is_symlink(), "The shared lease requires a regular owner.json file.")
        token_path = self.args.lease_token_file.resolve(strict=True)
        require(token_path.is_file() and not self.args.lease_token_file.is_symlink(), "The expected lease token must be a regular file.")
        lease_record = json.loads(owner_path.read_text())
        owner = lease_record.get("owner_thread")
        actual = lease_record.get("run_token", "")
        expected = token_path.read_text().strip()
        require(lease_record.get("project") == "BiliComics" and lease_record.get("state") == "held"
                and owner == self.args.lease_owner, "The exclusive shared device lease has another owner or is not held.")
        require(re.fullmatch(r"[0-9a-f]{48}", expected) is not None and isinstance(actual, str)
                and secrets.compare_digest(actual, expected),
                "The coordinator's lease token does not match the shared lease.")
        if self.state:
            require(self.state.get("lease_owner") == owner and self.state.get("lease_token_sha256") == sha256(expected.encode()),
                    "The shared lease changed after this run began.")
        self.lease_identity = {"lease_owner": owner, "lease_token_sha256": sha256(expected.encode()),
                               "lease_run_id": lease_record.get("run_id"),
                               "shared_lease_managed_by": "external coordinator"}

    def remote(self, script, *, data=None, check=True, timeout=20):
        self.check_lease()
        command = shell_line("sh", "-c", "set -eu\numask 077\n" + script)
        if self.args.ssh_helper:
            helper = self.args.ssh_helper.resolve(strict=True)
            require(helper.is_file() and os.access(helper, os.X_OK), "The configured SSH helper is not executable.")
            route = [str(helper), command]
        else:
            require(self.args.ssh_target is not None, "Select --ssh-helper or --ssh-target.")
            route = ["ssh", "-o", "BatchMode=yes", "-o", "StrictHostKeyChecking=yes", "-o", "ConnectTimeout=10"]
            for option in self.args.ssh_option:
                require(not re.match(r"(?i)\s*(StrictHostKeyChecking|BatchMode)\s*=", option),
                        "Host-key verification and noninteractive authentication cannot be overridden.")
                route.extend(["-o", option])
            route.extend([self.args.ssh_target, command])
        try:
            result = subprocess.run(route, input=data, capture_output=True, timeout=timeout)
        except subprocess.TimeoutExpired:
            raise TimeoutError("A device command exceeded its bounded transport deadline.") from None
        if check and result.returncode:
            # Do not publish command vectors, authentication material, or raw
            # device output in exception reports.
            raise RuntimeError(f"A device command failed with exit status {result.returncode}.")
        return result

    def read(self, remote_path, *, required=True):
        result = self.remote(shell_line("cat", remote_path), check=required)
        return result.stdout if result.returncode == 0 else b""

    def write(self, remote_path, data):
        self.remote("cat > " + shlex.quote(remote_path), data=data)

    def save(self):
        json_write(self.state_path, self.state)

    def run_token(self):
        token_path = self.output / ".run-token"
        require(token_path.is_file() and not token_path.is_symlink(), "The private per-run ownership token is unavailable.")
        token = token_path.read_text().strip()
        require(re.fullmatch(r"[0-9a-f]{64}", token) is not None, "The per-run token has an invalid format.")
        return token

    def preflight(self):
        require(not self.state, "Preflight requires a fresh host evidence directory.")
        firmware = self.args.expected_firmware
        require(re.fullmatch(r"[0-9]+(?:\.[0-9]+)+", firmware) is not None, "An exact firmware version is required.")
        require(self.args.operator_model.strip(), "The coordinator must supply the confirmed device model label.")
        script = r"""
[ -c /dev/fb0 ] && [ -r /dev/fb0 ] && [ -w /dev/fb0 ]
[ ! -e /tmp/bilicomics-launcher.lock ] && [ ! -L /tmp/bilicomics-launcher.lock ]
firmware=$(sed -n 's/^Kindle[[:space:]]*\([0-9][0-9.]*\).*$/\1/p' /etc/prettyversion.txt | head -n 1)
printf 'firmware=%s\n' "$firmware"
for name in virtual_size stride bits_per_pixel rotate; do
    if [ -r "/sys/class/graphics/fb0/$name" ]; then printf 'fb_%s=' "$name"; cat "/sys/class/graphics/fb0/$name"; fi
done
for service in powerd lab126_gui statusbar; do
    printf 'service_%s=' "$service"
    status "$service"
done
native_awesome_pids=$(pidof awesome)
printf 'awesome_pids=%s\n' "$native_awesome_pids"
native_epdc_mode=unknown
native_epdc_mode_probe=not-normal-native
native_mode_probe_allowed=1
case "$(status statusbar 2>/dev/null || :)" in *start/running*) ;; *) native_mode_probe_allowed=0 ;; esac
for native_pid in $native_awesome_pids; do
    native_stat=$(sed 's/^.*) //' "/proc/$native_pid/stat" 2>/dev/null || :)
    native_state=$(printf '%s\n' "$native_stat" | awk '{print $1}')
    case "$native_state" in ''|T|t|Z|X) native_mode_probe_allowed=0 ;; esac
done
if [ "$native_mode_probe_allowed" -eq 1 ]; then
    if command -v timeout >/dev/null 2>&1 && command -v lipc-get-prop >/dev/null 2>&1; then
        native_epdc_mode_probe=bounded-native-preflight-lipc
        native_mode_result=$(timeout 3 lipc-get-prop -s com.lab126.winmgr epdcMode 2>/dev/null || :)
        case "$native_mode_result" in Y8|Y8INV) native_epdc_mode=$native_mode_result ;; esac
    else
        native_epdc_mode_probe=bounded-query-unavailable
    fi
fi
printf 'native_epdc_mode=%s\n' "$native_epdc_mode"
printf 'native_epdc_mode_probe=%s\n' "$native_epdc_mode_probe"
"""
        raw = self.remote(script).stdout.decode("utf-8", errors="strict")
        fields = dict(line.split("=", 1) for line in raw.splitlines() if "=" in line)
        require(fields.get("firmware") == firmware, "The actual firmware differs from the explicitly selected runtime pair.")
        require(all("start/running" in fields.get("service_" + name, "") for name in ("powerd", "lab126_gui")),
                "The native power and GUI services must be running before a supervised run.")
        require(fields.get("awesome_pids", "").strip(), "The native window manager is unavailable.")
        self.state = {"schema_version": 1, "evidence_class": "physical-device", "run_id": self.args.run_id,
                      "run_root": self.root, "phase": "preflight", **self.lease_identity,
                      "operator_confirmed_model": self.args.operator_model.strip(), "profile": self.args.profile,
                      "expected_firmware": firmware, "preflight": fields, "physical_device_accessed": True,
                      "live_account_used": False, "remote_account_writes": False, "input_injected_by_harness": False,
                      "hardware_support_accepted": False, "physical_panel_review": "pending",
                      "harness_sha256": file_hash(Path(__file__)), "checkpoints": {}, "observations": []}
        mode = fields.get("native_epdc_mode")
        self.state["native_epdc_mode_preflight"] = {
            "mode": mode if mode in EPDC_MODES else None,
            "source": "native-preflight-lipc" if mode in EPDC_MODES else None,
            "probe": fields.get("native_epdc_mode_probe"),
            "scope": "normal native display before this isolated run takes ownership",
            "profile": self.args.profile, "lease_owner": self.lease_identity["lease_owner"],
            "lease_run_id": self.lease_identity["lease_run_id"],
            "lease_token_sha256": self.lease_identity["lease_token_sha256"],
            "used_as_current_application_mode_fallback": False,
        }
        self.save()
        return {"phase": "preflight", "firmware": firmware, "run_root": self.root,
                "native_epdc_mode_preflight": self.state["native_epdc_mode_preflight"],
                "framebuffer_ioctl_geometry": "pending deployment of the pinned read-only helper"}

    def deploy(self):
        require(self.state.get("phase") == "preflight", "Deployment requires this run's successful preflight.")
        require(self.args.approve_runtime_pair, "Deployment requires the coordinator's explicit supervised runtime-pair selection.")
        fixture = fixture_inputs(self.args)
        package = self.args.package.resolve(strict=True)
        require(file_hash(package) == self.args.expected_package_sha256, "The package differs from the selected exact SHA-256.")
        helper = self.args.framebuffer_helper.resolve(strict=True)
        helper_data = helper.read_bytes()
        arm_elf(helper_data)
        require(sha256(helper_data) == self.args.expected_helper_sha256, "The read-only framebuffer helper digest differs.")
        wrapper_data = normalized_script(self.args.wrapper.resolve(strict=True))
        watchdog_data = normalized_script(self.args.watchdog.resolve(strict=True))
        event_wrapper_data = normalized_script(self.args.event_wrapper.resolve(strict=True))
        staged = {}
        with zipfile.ZipFile(package) as archive:
            names = archive.namelist()
            require(len(names) == len(set(names)), "The selected package contains duplicate paths.")
            for entry in archive.infolist():
                path = PurePosixPath(entry.filename)
                require(not path.is_absolute() and ".." not in path.parts and not entry.is_dir(), "The package contains an unsafe path.")
                mode = entry.external_attr >> 16
                require(stat.S_IFMT(mode) in (0, stat.S_IFREG), "The package contains a nonregular entry.")
                if entry.filename.startswith("notices/"):
                    staged[entry.filename] = (archive.read(entry), 0o644)
                elif entry.filename.startswith("bilicomics/app/"):
                    staged[entry.filename[len("bilicomics/"):]] = (archive.read(entry), 0o755 if entry.filename.endswith("/bilicomics") else 0o644)
            binary_data = archive.read("bilicomics/app/bilicomics")
            arm_elf(binary_data)
            require(sha256(binary_data) == self.args.expected_binary_sha256, "The packaged ARM binary digest differs.")
            repository_pairs = [line for line in archive.read("bilicomics/data/device-allowlist").decode().splitlines()
                                if line.strip() and not line.lstrip().startswith("#")]
            require(not repository_pairs, "The package unexpectedly changes the repository acceptance allowlist.")
            launcher_data = archive.read("bilicomics/launch.sh").replace(b"\r\n", b"\n")
            require(b"\r" not in launcher_data and launcher_data.startswith(b"#!/bin/sh\n"), "The launcher is not a normalized shell script.")
        binary_name = "bilicomics-" + self.args.expected_binary_sha256
        staged.pop("app/bilicomics", None)
        staged.update({"app/" + binary_name: (binary_data, 0o755), "app/bilicomics": (wrapper_data, 0o755),
                       "app/physical-framebuffer": (helper_data, 0o755), "physical-watchdog.sh": (watchdog_data, 0o755),
                       "tools/lipc-wait-event": (event_wrapper_data, 0o755),
                       "launch.sh": (launcher_data, 0o755), "data/device-profile": ((self.state["profile"] + "\n").encode(), 0o600),
                       "data/device-allowlist": ((self.state["profile"] + " " + self.state["expected_firmware"] + "\n").encode(), 0o600)})
        token = secrets.token_hex(32)
        (self.output / ".run-token").write_text(token + "\n")
        (self.output / ".run-token").chmod(0o600)
        staged["data/logs/run-token"] = ((token + "\n").encode(), 0o600)
        staged["data/logs/binary-name"] = ((binary_name + "\n").encode(), 0o600)
        mode = "distinct_fixture" if fixture is not None else "demo"
        staged["data/logs/content-mode"] = ((mode + "\n").encode(), 0o600)
        if fixture is not None:
            require(not (set(staged) & set(fixture["files"])), "Fixture inputs collide with device control files.")
            staged.update(fixture["files"])
            guard_name = "offline-socket-guard-" + fixture["guard_sha256"]
            staged["app/" + guard_name] = (fixture["guard"], 0o755)
            staged["data/logs/socket-guard-name"] = ((guard_name + "\n").encode(), 0o600)
            staged["data/logs/socket-guard-sha256"] = ((fixture["guard_sha256"] + "\n").encode(), 0o600)
            staged["data/logs/fixture-sha256"] = ((fixture["manifest_sha256"] + "\n").encode(), 0o600)
        payload = io.BytesIO()
        with tarfile.open(fileobj=payload, mode="w") as archive:
            for name, (data, file_mode) in sorted(staged.items()):
                entry = tarfile.TarInfo(name)
                entry.size, entry.mode, entry.mtime = len(data), file_mode, 0
                archive.addfile(entry, io.BytesIO(data))
        self.remote("[ ! -e " + shlex.quote(self.root) + " ] && [ ! -L " + shlex.quote(self.root) + " ]\n" +
                    shell_line("mkdir", self.root))
        self.remote(shell_line("tar", "-xf", "-", "-C", self.root), data=payload.getvalue(), timeout=120)
        digests = {name: sha256(data) for name, (data, _) in staged.items()}
        response = self.remote(shell_line("sha256sum", *(self.root + "/" + name for name in sorted(digests)))).stdout.decode()
        actual = {line.split(None, 1)[1].strip(): line.split(None, 1)[0] for line in response.splitlines()}
        require(all(actual.get(self.root + "/" + name) == digest for name, digest in digests.items()), "A deployed file digest differs.")
        geometry = json.loads(self.remote(shell_line(self.root + "/app/physical-framebuffer", "--info")).stdout)
        self.check_geometry(geometry)
        if fixture is not None:
            guard_path = self.root + "/app/" + guard_name
            probe = socket_guard_proof(self.remote(shell_line(guard_path, "--probe")).stdout, 1)
            compatibility = self.remote("query=$(command -v lipc-get-prop)\ncase \"$query\" in /*) ;; *) exit 4 ;; esac\n" +
                                        shell_line(guard_path, "--") + ' "$query" -s com.lab126.winmgr epdcMode\n')
            compatibility_proof = socket_guard_proof(compatibility.stderr, 2)
            epdc = compatibility.stdout.decode().strip()
            require(epdc in EPDC_MODES and epdc == self.state["preflight"].get("native_epdc_mode"),
                    "The guarded native IPC query did not preserve the preflight EPDC observation.")
            require(all(probe[key] == compatibility_proof[key] for key in
                        ("uid", "gid", "effective_uid", "effective_gid", "no_new_privs", "audit_arch")),
                    "The guarded IPC compatibility check changed process credentials or isolation context.")
            self.state.update({"fixture_manifest": fixture["manifest"], "fixture_manifest_sha256": fixture["manifest_sha256"],
                               "fixture_evidence_class": "controlled-offline-receipt-fixture",
                               "socket_guard": {"name": guard_name, "sha256": fixture["guard_sha256"],
                                                "probe": probe, "native_ipc_proof": compatibility_proof,
                                                "native_ipc_epdc_mode": epdc, "passed": True}})
            self.write(self.logs + "/socket-guard-approved",
                       (token + "|" + fixture["guard_sha256"] + "|" + fixture["manifest_sha256"] + "\n").encode())
        self.state.update({"phase": "deployed", "binary_name": binary_name,
                           "binary_sha256": self.args.expected_binary_sha256, "package_sha256": file_hash(package),
                           "deployed_file_sha256": digests, "framebuffer": geometry, "content_mode": mode,
                           "runtime_pair_scope": "this supervised isolated run only; no repository support entry"})
        self.save()
        return {"phase": "deployed", "binary_sha256": self.state["binary_sha256"], "framebuffer": geometry}

    def check_geometry(self, geometry):
        require((geometry.get("width"), geometry.get("height")) == GEOMETRIES[self.state["profile"]],
                "The actual framebuffer dimensions differ from the selected design profile.")
        require(geometry.get("xoffset") == geometry.get("yoffset") == 0 and geometry.get("read_only") is True,
                "Native acceptance requires unpanned geometry from the read-only helper.")

    def start(self):
        require(self.state.get("phase") == "deployed", "Each isolated physical run can be started only once.")
        if self.state.get("content_mode", "demo") == "distinct_fixture":
            require(self.state.get("socket_guard", {}).get("passed") is True and self.args.screen == "reader"
                    and getattr(self.args, "performance_trace", False),
                    "A distinct fixture requires the approved socket guard, Reader route and performance evidence.")
        require(1000 <= self.args.exit_after_ms <= (600000 if self.args.assisted else 30000), "Use a bounded assisted session or a smoke duration of at most 30 seconds.")
        require(10 <= self.args.init_timeout <= 120 and self.args.init_timeout < self.args.session_timeout <= 600,
                "The independent initialization and absolute session deadlines are invalid.")
        token = self.run_token()
        for name, value in (("screen", self.args.screen), ("exit-after-ms", str(self.args.exit_after_ms))):
            self.write(self.logs + "/" + name, (value + "\n").encode())
        performance_trace = getattr(self.args, "performance_trace", False)
        if performance_trace:
            self.write(self.logs + "/performance-trace-enabled", b"1\n")
        # Persist intent before dispatch: an SSH transport failure can occur
        # after the independent watchdog has already started on the device.
        self.state.update({"phase": "starting", "screen": self.args.screen, "exit_after_ms": self.args.exit_after_ms,
                           "init_timeout_seconds": self.args.init_timeout, "absolute_timeout_seconds": self.args.session_timeout,
                           "assisted_physical_input": self.args.assisted, "started_unix": time.time(),
                           "performance_trace_enabled": performance_trace})
        self.save()
        self.remote("[ ! -e " + shlex.quote(LOCK) + " ] && [ ! -L " + shlex.quote(LOCK) + " ]\n" +
                    "if command -v setsid >/dev/null 2>&1; then detach=setsid; else detach=nohup; fi\n" +
                    "BILICOMICS_PHYSICAL_TOKEN=" + shlex.quote(token) + " \"$detach\" sh " +
                    shell_line(self.root + "/physical-watchdog.sh", self.root, token, self.args.init_timeout, self.args.session_timeout) +
                    " >>" + shlex.quote(self.logs + "/watchdog.log") + " 2>&1 </dev/null &\nprintf '%s\\n' \"$!\"\n")
        deadline = time.monotonic() + 12
        while time.monotonic() < deadline:
            done = self.done(required=False)
            require(done is None, "The independent watchdog finished before its startup handshake; collect the failure evidence.")
            if self.read(self.logs + "/watchdog.ready", required=False).decode().strip() == token:
                self.state["phase"] = "running"
                self.save()
                return {"phase": "running", "absolute_timeout_seconds": self.args.session_timeout,
                        "ready": "pending; use checkpoint before interacting"}
            time.sleep(0.5)
        raise TimeoutError("The independent watchdog did not publish its startup handshake.")

    def done(self, *, required=True):
        raw = self.read(self.logs + "/watchdog.done", required=required).decode().strip()
        if not raw:
            return None
        fields = raw.split("|")
        require(len(fields) == 4 and secrets.compare_digest(fields[0], self.run_token()), "The watchdog completion record belongs to another run.")
        return {"reason": fields[1], "recovery_complete": fields[2] == "1", "launcher_exit_status": fields[3]}

    def native_identity(self):
        fields = self.read(self.logs + "/native.identity").decode().strip().split("|")
        require(len(fields) == 5 and secrets.compare_digest(fields[4], self.run_token()), "The wrapper's native identity is not this run's identity.")
        pid, ticks, executable, parent, _ = fields
        require(pid.isdecimal() and int(pid) > 1 and ticks.isdecimal() and parent.isdecimal(), "Invalid native process identity.")
        require(executable == self.root + "/app/" + self.state["binary_name"], "The application executable is outside the isolated run.")
        fixture_mode = self.state.get("content_mode", "demo") == "distinct_fixture"
        flags = "--offline --kindle" if fixture_mode else "--demo --offline --kindle"
        script = shell_line("cat", f"/proc/{pid}/stat") + "\n" + shell_line("readlink", f"/proc/{pid}/exe") + "\n" + shell_line("sha256sum", f"/proc/{pid}/exe") + "\n" + \
                 "tr '\\000' '\\n' <" + shlex.quote(f"/proc/{pid}/environ") + " | " + shell_line("grep", "-F", "-x", "BILICOMICS_PHYSICAL_TOKEN=" + self.run_token()) + " >/dev/null\n" + \
                 "for flag in " + flags + "; do tr '\\000' '\\n' <" + shlex.quote(f"/proc/{pid}/cmdline") + " | grep -F -x -e \"$flag\" >/dev/null; done\n"
        rows = self.remote(script).stdout.decode().splitlines()
        require(len(rows) == 3, "The native process could not be fully identified.")
        actual_ticks = rows[0].rsplit(")", 1)[1].split()[19]
        require(actual_ticks == ticks and rows[1] == executable and rows[2].split()[0] == self.state["binary_sha256"],
                "The live PID, start time, executable path or digest changed.")
        identity = {"pid": int(pid), "start_ticks": int(ticks), "exe_path": executable, "exe_sha256": self.state["binary_sha256"],
                    "demo_offline_native_arguments_verified": not fixture_mode}
        if fixture_mode:
            require(self.state.get("socket_guard", {}).get("passed") is True, "The fixture socket guard was not approved.")
            # The production application's bounded log sink can buffer a short
            # stderr record until exit. The guard publishes this separate file
            # before exec, so live identity never depends on log flushing.
            proof = socket_guard_proof(self.read(self.logs + "/ui-socket-guard.json"), 2, int(pid))
            require(all(proof[key] == self.state["socket_guard"]["probe"][key] for key in
                        ("uid", "gid", "effective_uid", "effective_gid", "no_new_privs", "audit_arch")),
                    "The actual UI guard changed the approved process context.")
            extra = self.remote(shell_line("cat", f"/proc/{pid}/status") + "\nprintf '\\000'\n" +
                                shell_line("cat", f"/proc/{pid}/cmdline") + "\nprintf '\\000'\n" +
                                "generation=$(cat " + shlex.quote(LOCK + "/generation") + ")\n" +
                                "nonce=$(cat " + shlex.quote(LOCK + "/nonce") + ")\n" +
                                "portrait=$(cat " + shlex.quote(LOCK + "/native-portrait-orientation") + ")\n" +
                                '[ "$generation" = "1:$nonce" ]\nprefix="$generation:"\ncase "$portrait" in "$prefix"U:[0-3]) ;; *) exit 4 ;; esac\n' +
                                "tr '\\000' '\\n' <" + shlex.quote(f"/proc/{pid}/environ") +
                                ' | grep -F -x -e "BILICOMICS_LAUNCH_TOKEN=$nonce" >/dev/null\n' +
                                'printf \'%s\\n\' "${portrait#"$prefix"}"\n').stdout
            status_bytes, tail = extra.split(b"\0", 1)
            argument_bytes, portrait_bytes = tail.rsplit(b"\0\0", 1)
            arguments = [value.decode() for value in argument_bytes.split(b"\0")]
            expected = [executable, "--kindle", "--device", self.state["profile"], "--data-dir", self.root + "/data",
                        "--performance-trace", self.logs + "/performance.jsonl", "--offline", "--screen", "reader",
                        "--lifecycle-trace", self.logs + "/lifecycle.jsonl", "--exit-after-ms", str(self.state["exit_after_ms"])]
            require(arguments == expected, "The fixture UI arguments differ from the approved offline guest launch.")
            status = dict(line.split(":", 1) for line in status_bytes.decode().splitlines() if ":" in line)
            # Linux added the proc status field in 4.10. The guard still
            # proves its pre-exec value with prctl on older kernels; absence
            # here is unknown live metadata, not permission to omit Seccomp.
            reported_no_new_privs = status.get("NoNewPrivs")
            require(reported_no_new_privs is None or reported_no_new_privs.strip() == str(proof["no_new_privs"]),
                    "The reported live no-new-privileges value differs from the guard proof.")
            require(status.get("Seccomp", "").strip() == "2"
                    and [int(value) for value in status.get("Uid", "").split()[:2]] == [proof["uid"], proof["effective_uid"]]
                    and [int(value) for value in status.get("Gid", "").split()[:2]] == [proof["gid"], proof["effective_gid"]],
                    "The live fixture process no longer has the proved socket filter or credentials.")
            portrait = portrait_bytes.decode().strip()
            require(re.fullmatch(r"U:[0-3]", portrait) is not None, "The fixture lacks an owned upright portrait calibration.")
            identity.update({"content_mode": "distinct_fixture", "offline_guest_native_arguments_verified": True,
                             "socket_guard_proof": proof, "live_seccomp_mode": 2,
                             "live_no_new_privs": int(reported_no_new_privs.strip()) if reported_no_new_privs is not None else None,
                             "live_no_new_privs_scope": "proc status field when exposed; guard proof independently records the pre-exec prctl value",
                             "portrait_calibration": {"direction": "U", "raw_rotation": int(portrait[-1]),
                                                      "source": "owned launcher calibration; current geometry must still match"}})
        return identity

    def trace(self):
        raw = self.read(self.logs + "/lifecycle.jsonl", required=False)
        return [json.loads(line) for line in raw.splitlines(keepends=True) if line.endswith(b"\n") and line.strip()]

    def checkpoint_value(self, kind):
        identity = self.native_identity()
        values = [value for value in self.trace() if value.get("process_id") == identity["pid"]]
        if self.state.get("content_mode", "demo") == "distinct_fixture":
            snapshot = fixture_reader_snapshot(values, self.state["fixture_manifest"], kind)
            performance = [json.loads(line) for line in self.read(self.logs + "/performance.jsonl").splitlines(keepends=True)
                           if line.endswith(b"\n") and line.strip()]
            require(not any(value.get("kind") == "diagnostic" and value.get("stage") == "trace_stopped" for value in performance),
                    "The fixture performance trace stopped before it could prove the current page.")
            publication = (fixture_publication(performance, snapshot, identity["pid"]) if kind != "suspended"
                           else {"binding": "suspended-no-frame-capture", "marker_required": False})
            epdc = trace_epdc_mode(values)
            require(epdc["mode"] in EPDC_MODES, "The fixture needs an explicit current native EPDC mode.")
            epdc.update({"profile": self.state["profile"], **self.lease_identity, "native_process": identity,
                         "preflight": self.state.get("native_epdc_mode_preflight"), "preflight_used_as_current_mode_fallback": False})
            return {"native_process": identity, "reader_state": snapshot["reader_state"], "latest_power": snapshot["latest_power"],
                    "trace_records": len(values), "requested_checkpoint": kind, "native_epdc_mode": epdc["mode"],
                    "native_epdc_mode_evidence": epdc, "content_mode": "distinct_fixture",
                    "fixture_manifest_sha256": self.state["fixture_manifest_sha256"], "fixture_publication": publication,
                    "fixture_evidence_class": "controlled-offline-receipt-fixture"}
        require(any(value.get("event") == "ready" for value in values), "The application has not reached UI readiness.")
        ui = next((value for value in reversed(values) if value.get("event") == "ui-state"), None)
        require(ui and ui.get("demo") is True and ui.get("view_model_fixture_only") is False,
                "A native demonstration UI trace is required.")
        latest_power = next((value for value in reversed(values) if value.get("event") in ("suspend", "resume")), None)
        suspended = latest_power is not None and latest_power.get("event") == "suspend"
        reader = next((value for value in reversed(values) if isinstance(value.get("reader_state"), dict)), None)
        if kind == "suspended":
            require(suspended and latest_power.get("suspended") is True, "A native lifecycle transition has not suspended the application.")
        else:
            require(not suspended, "The app is suspended; native screensaver or unlock content must not be captured.")
        if kind in ("reader", "resumed"):
            valid_reader = next((value for value in reversed(values) if value.get("event") == "reader-ready"
                                 and value.get("reader_loading") is False and len(value.get("frame_size", [])) == 2
                                 and min(value["frame_size"]) > 0), None)
            require(valid_reader and reader and reader["reader_state"].get("route") == "reader"
                    and ui["state"].get("route") == "reader" and ui["state"].get("reader", {}).get("loading") is False,
                    "The current native reader page is not ready.")
        if kind == "resumed":
            require(latest_power and latest_power.get("event") == "resume" and latest_power.get("suspended") is False,
                    "The native lifecycle supervisor has not resumed this application after wake and unlock.")
        epdc = trace_epdc_mode(values)
        epdc.update({"profile": self.state["profile"], "lease_owner": self.lease_identity["lease_owner"],
                     "lease_run_id": self.lease_identity["lease_run_id"],
                     "lease_token_sha256": self.lease_identity["lease_token_sha256"],
                     "native_process": identity,
                     "preflight": self.state.get("native_epdc_mode_preflight"),
                     "preflight_used_as_current_mode_fallback": False})
        return {"native_process": identity, "ui": ui["state"], "reader_state": reader["reader_state"] if reader else None,
                "latest_power": latest_power, "trace_records": len(values), "requested_checkpoint": kind,
                "native_epdc_mode": epdc["mode"], "native_epdc_mode_evidence": epdc}

    def capture(self, name, checkpoint):
        require(checkpoint["requested_checkpoint"] != "suspended", "Native sleep or unlock images are not acceptance artifacts.")
        identity = checkpoint["native_process"]
        # Check powerd, launcher ownership and the final lifecycle request on
        # both sides of the read. An event race discards the in-memory payload.
        def display_gate():
            script = "IFS='|' read -r launcher_pid launcher_ticks launcher_exe <" + shlex.quote(self.logs + "/launcher.identity") + "\n" + \
                     "case \"$launcher_pid:$launcher_ticks\" in *[!0-9:]*|:*) exit 4 ;; esac\n" + \
                     "stat_tail=$(sed 's/^.*) //' \"/proc/$launcher_pid/stat\")\n" + \
                     "[ \"$(printf '%s\\n' \"$stat_tail\" | awk '{print $20}')\" = \"$launcher_ticks\" ]\n" + \
                     "[ \"$(readlink \"/proc/$launcher_pid/exe\")\" = \"$launcher_exe\" ]\n" + \
                     "tr '\\000' '\\n' <\"/proc/$launcher_pid/environ\" | " + shell_line("grep", "-F", "-x", "BILICOMICS_PHYSICAL_TOKEN=" + self.run_token()) + " >/dev/null\n" + \
                     "[ \"$(sed -n '1p' " + shlex.quote(LOCK + "/pid") + ")\" = \"$launcher_pid\" ]\n" + \
                     "[ \"$(sed -n '1p' " + shlex.quote(LOCK + "/nonce") + ")\" = \"$(cut -d'|' -f4 " + shlex.quote(self.logs + "/owned-launcher-lock") + ")\" ]\n" + \
                     "[ \"$(ls -id " + shlex.quote(LOCK) + " | awk '{print $1}')\" = \"$(cut -d'|' -f3 " + shlex.quote(self.logs + "/owned-launcher-lock") + ")\" ]\n" + \
                     "[ \"$(lipc-get-prop com.lab126.powerd state)\" = active ]\n" + \
                     "case \"$(status statusbar)\" in *stop/waiting*) ;; *) exit 4 ;; esac\n" + \
                     "request=$(sed -n '1p' " + shlex.quote(LOCK + "/lifecycle-request") + " 2>/dev/null || :)\n" + \
                     "case \"$request\" in *:suspend) exit 4 ;; esac\n" + \
                     "while IFS='|' read -r kind native_pid native_ticks native_exe native_state; do\n" + \
                     "  [ \"$kind\" = awesome ] || continue\n" + \
                     "  stat_tail=$(sed 's/^.*) //' \"/proc/$native_pid/stat\")\n" + \
                     "  [ \"$(printf '%s\\n' \"$stat_tail\" | awk '{print $20}')\" = \"$native_ticks\" ]\n" + \
                     "  [ \"$(readlink \"/proc/$native_pid/exe\")\" = \"$native_exe\" ]\n" + \
                     "  current_state=$(printf '%s\\n' \"$stat_tail\" | awk '{print $1}')\n" + \
                     "  case \"$current_state\" in T|t) ;; *) exit 4 ;; esac\n" + \
                     "done <" + shlex.quote(self.logs + "/native-baseline.tsv") + "\n"
            self.remote(script)
            current = self.checkpoint_value("reader" if checkpoint.get("content_mode") == "distinct_fixture"
                                            or checkpoint.get("ui", {}).get("route") == "reader" else "ready")
            require(current["native_process"] == identity, "The app identity changed at the framebuffer capture boundary.")
            return current
        before = display_gate()
        fixture_mode = checkpoint.get("content_mode") == "distinct_fixture"
        proof_fields = ("ui", "reader_state", "latest_power", "trace_records", "native_epdc_mode", "native_epdc_mode_evidence")
        if fixture_mode:
            proof_fields += ("fixture_publication", "fixture_manifest_sha256")
        require(all(before.get(field) == checkpoint.get(field) for field in proof_fields),
                "The reader state changed after the checkpoint; the image would not match its saved anchor.")
        before_geometry = (json.loads(self.remote(shell_line(self.root + "/app/physical-framebuffer", "--info")).stdout)
                           if fixture_mode else None)
        response = self.remote(shell_line(self.root + "/app/physical-framebuffer", "--capture"), timeout=30).stdout
        after = display_gate()
        require(before.get("latest_power") == after.get("latest_power") and before["trace_records"] == after["trace_records"],
                "The demo UI changed during capture; the framebuffer payload was discarded.")
        require(before.get("native_epdc_mode_evidence") == after.get("native_epdc_mode_evidence"),
                "The explicit native EPDC snapshot changed during capture; the framebuffer payload was discarded.")
        header, pixels = response.split(b"\n", 1)
        geometry = json.loads(header)
        self.check_geometry(geometry)
        if fixture_mode:
            after_geometry = json.loads(self.remote(shell_line(self.root + "/app/physical-framebuffer", "--info")).stdout)
            require(before_geometry == geometry == after_geometry and all(before.get(key) == after.get(key) for key in proof_fields),
                    "The fixture page, native frame or framebuffer geometry changed during capture.")
        require(len(pixels) == geometry["visible_bytes"], "The framebuffer read is incomplete.")
        raw_path = self.output / (name + ".framebuffer.raw")
        with raw_path.open("xb") as stream:
            stream.write(pixels)
        raw_sha256 = file_hash(raw_path)
        require(raw_sha256 == sha256(pixels), "The saved raw framebuffer evidence differs from the captured bytes.")
        memory = framebuffer_memory_pixels(geometry, pixels)
        projection = framebuffer_panel_projection(geometry, memory, checkpoint.get("native_epdc_mode"))
        marker = fixture_marker(geometry, projection, checkpoint, self.state["fixture_manifest"]) if fixture_mode else None
        from PIL import Image
        size = geometry["width"], geometry["height"]
        path = self.output / (name + ".png")
        with path.open("xb") as stream:
            Image.frombytes(memory["image_mode"], size, memory["pixels"]).save(stream, format="PNG")
        memory_sha256 = file_hash(path)
        panel_record = {key: value for key, value in projection.items() if key != "pixels"}
        panel_record.update({"path": None, "png_sha256": None,
                             "native_epdc_mode_evidence": checkpoint.get("native_epdc_mode_evidence"),
                             "framebuffer_visual_inversion_applied": memory["framebuffer_visual_inversion_applied"]})
        if projection["status"] == "generated":
            projected_path = self.output / (name + ".projected-panel.png")
            with projected_path.open("xb") as stream:
                Image.frombytes(projection["image_mode"], size, projection["pixels"]).save(stream, format="PNG")
            panel_record.update({"path": str(projected_path), "png_sha256": file_hash(projected_path)})
        record = {"path": str(path), "png_sha256": memory_sha256, "framebuffer_pixels_sha256": raw_sha256,
                  "geometry": geometry, "conversion": memory["conversion"], "evidence": "framebuffer memory pixels",
                  "physical_panel_update_confirmed": False, "native_home_or_account_content": False,
                  "raw_framebuffer": {"path": str(raw_path), "bytes": len(pixels), "sha256": raw_sha256,
                                      "semantics": "unmodified visible framebuffer memory bytes"},
                  "memory_png": {"path": str(path), "png_sha256": memory_sha256,
                                 "conversion": memory["conversion"],
                                 "framebuffer_visual_inversion_applied": memory["framebuffer_visual_inversion_applied"],
                                 "native_epdc_mode_applied": False},
                  "panel_projection": panel_record}
        if fixture_mode:
            record.update({"fixture_marker": marker, "content_mode": "distinct_fixture",
                           "fixture_evidence_class": "controlled-offline-receipt-fixture"})
        json_write(self.output / (name + ".framebuffer.json"), record)
        return record

    def checkpoint(self):
        require(self.state.get("phase") == "running", "A checkpoint requires this run's active independent watchdog.")
        require(re.fullmatch(r"[a-z0-9][a-z0-9-]{0,63}", self.args.name) is not None, "Unsafe checkpoint name.")
        require(self.args.name not in self.state["checkpoints"], "Each checkpoint name must be new.")
        fixture_mode = self.state.get("content_mode", "demo") == "distinct_fixture"
        if fixture_mode:
            require(self.args.capture == (self.args.require != "suspended"),
                    "An awake fixture checkpoint requires its marker capture; suspended checkpoints forbid captures.")
        deadline = time.monotonic() + self.args.wait_seconds
        while True:
            require(self.done(required=False) is None, "The run has ended; collect recovery evidence.")
            try:
                # Hints only defer the full native proof; they cannot authorize
                # a checkpoint or framebuffer read.
                hints = self.trace()
                before = self.state["checkpoints"].get(self.args.compare_to)
                hint = (fixture_ready_hint(hints, self.state["fixture_manifest"], self.args.require, before, self.args.expect)
                        if fixture_mode else checkpoint_ready_hint(hints, self.args.require, before, self.args.expect))
                require(hint,
                        "The untrusted lifecycle hint has not reached the requested checkpoint.")
                value = self.checkpoint_value(self.args.require)
                break
            except (RuntimeError, TimeoutError):
                if time.monotonic() >= deadline:
                    raise
                time.sleep(0.5)
        if self.args.compare_to:
            before = self.state["checkpoints"].get(self.args.compare_to)
            require(before is not None, "The comparison checkpoint does not exist.")
            require(before["native_process"] == value["native_process"], "Sleep or input changed the native application process.")
            old, new = before["reader_state"], value["reader_state"]
            require(isinstance(old, dict) and isinstance(new, dict), "A reader comparison requires two actual reader states.")
            require(all(old.get(field) == new.get(field) for field in IDENTITY_FIELDS), "Reader identity or settings changed unexpectedly.")
            if self.args.expect == "next-page":
                require(new.get("page_index") == old.get("page_index", -2) + 1 and new.get("page_id") != old.get("page_id"),
                        "The observed native state did not advance to the next source page.")
            else:
                require(all(old.get(field) == new.get(field) for field in ANCHOR_FIELDS), "The in-memory reading anchor changed across sleep.")
            value["comparison"] = {"before": self.args.compare_to, "expected": self.args.expect, "passed": True,
                                   "input_source": "native state transition; physical input requires explicit operator observation"}
        if self.args.capture:
            value["framebuffer"] = self.capture(self.args.name, value)
        self.state["checkpoints"][self.args.name] = value
        self.save()
        json_write(self.output / (self.args.name + ".checkpoint.json"), value)
        return {"checkpoint": self.args.name, "native_process": value["native_process"],
                "reader_state": value["reader_state"], "native_epdc_mode": value["native_epdc_mode"],
                "captured": bool(self.args.capture)}

    def stop(self):
        require(self.state.get("phase") in ("starting", "running", "stopped"), "There is no started run to stop.")
        done = self.done(required=False)
        if done is None:
            self.write(self.logs + "/stop.request", (self.run_token() + "\n").encode())
            deadline = time.monotonic() + 40
            while time.monotonic() < deadline:
                done = self.done(required=False)
                if done is not None:
                    break
                time.sleep(1)
        require(done is not None, "Recovery did not finish in the host wait; the independent watchdog retains its absolute deadline.")
        self.state.update({"phase": "stopped", "watchdog": done})
        self.save()
        self.collect()
        require(done["recovery_complete"], "The watchdog reports incomplete native recovery; coordinator intervention is required before lease return.")
        require(self.state["recovery_checks"]["passed"], "Native baseline restoration or launcher-lock release is incomplete.")
        return {"phase": "stopped", "watchdog": done, "native_baseline_restored": True,
                "physical_home_and_touch_confirmation": "pending operator observation",
                "shared_lease_released": False}

    def post_restore_process_snapshot(self):
        """One read-only PID/birth snapshot, without signalling or waiting for zombies."""
        require(self.state.get("phase") == "stopped" and self.state.get("watchdog", {}).get("recovery_complete") is True,
                "Process closure evidence requires completed watchdog recovery.")
        entries = []
        owned = self.output / "owned-processes.tsv"
        if owned.is_file():
            for line in owned.read_text().splitlines():
                fields = line.split("|")
                require(len(fields) == 5, "An owned process record is malformed.")
                entries.append((fields[0], fields[1], fields[2], fields[4]))
        for name in ("watchdog", "launcher"):
            raw = self.read(self.logs + "/" + name + ".identity", required=False).decode().strip()
            if raw:
                fields = raw.split("|")
                require(len(fields) == 3, "A supervisor identity record is malformed.")
                entries.append((*fields, name))
        native = self.read(self.logs + "/native.identity", required=False).decode().strip().split("|")
        if len(native) == 5:
            require(secrets.compare_digest(native[4], self.run_token()), "The recovery native identity belongs to another run.")
            entries.append((native[0], native[1], native[2], "application"))
        entries = sorted(set(entries))
        require(entries and len(entries) <= 256, "The recovery process identity set is empty or unbounded.")
        for pid, birth, executable, role in entries:
            require(pid.isdecimal() and int(pid) > 1 and birth.isdecimal() and executable.startswith("/")
                    and "\n" not in executable and "|" not in executable and re.fullmatch(r"[a-z0-9-]{1,64}", role),
                    "Unsafe recovery process identity.")
        script = r"""
snapshot_owned() {
    snapshot_pid=$1; snapshot_birth=$2; snapshot_exe=$3; snapshot_role=$4
    snapshot_state=gone; snapshot_actual_birth=none; snapshot_kernel=none
    if [ -e "/proc/$snapshot_pid/stat" ]; then
        if snapshot_tail=$(sed 's/^.*) //' "/proc/$snapshot_pid/stat" 2>/dev/null); then
            snapshot_actual_birth=$(printf '%s\n' "$snapshot_tail" | awk '{print $20}')
            snapshot_kernel=$(printf '%s\n' "$snapshot_tail" | awk '{print $1}')
            if [ "$snapshot_actual_birth" != "$snapshot_birth" ]; then snapshot_state=pid-reused
            else
                case "$snapshot_kernel" in
                    Z) snapshot_state=zombie ;;
                    X|x) snapshot_state=dead ;;
                    *) if [ "$(readlink "/proc/$snapshot_pid/exe" 2>/dev/null || :)" = "$snapshot_exe" ]; then
                           snapshot_state=active-match
                       else snapshot_state=identity-changed; fi ;;
                esac
            fi
        else snapshot_state=unreadable; fi
    fi
    printf '%s|%s|%s|%s|%s|%s\n' "$snapshot_pid" "$snapshot_birth" "$snapshot_role" "$snapshot_state" "$snapshot_actual_birth" "$snapshot_kernel"
}
"""
        script += "\n".join(shell_line("snapshot_owned", *entry) for entry in entries) + "\n"
        rows = self.remote(script).stdout.decode().splitlines()
        require(len(rows) == len(entries), "The recovery process snapshot is incomplete.")
        observed = []
        for expected, line in zip(entries, rows):
            fields = line.split("|")
            require(len(fields) == 6 and fields[:3] == [expected[0], expected[1], expected[3]]
                    and fields[3] in ("gone", "pid-reused", "zombie", "dead", "active-match", "identity-changed", "unreadable"),
                    "The recovery process snapshot lost its exact PID/birth binding.")
            observed.append({"pid": int(fields[0]), "expected_start_ticks": int(fields[1]), "role": fields[2],
                             "state": fields[3], "observed_start_ticks": fields[4], "kernel_state": fields[5]})
        report = {"observed_unix": time.time(), "processes": observed,
                  "passed": all(row["state"] in ("gone", "pid-reused", "zombie", "dead") for row in observed),
                  "zombies_reported_as_zombies": True, "signals_sent": False, "waited_for_zombies": False,
                  "scope": "Exact recorded PID/birth identities after native restoration; no process-name or whole-host scan"}
        self.state["post_restore_process_snapshot"] = report
        json_write(self.output / "post-restore-processes.json", report)
        return report

    def collect(self):
        require(self.state.get("phase") in ("starting", "running", "stopped"), "There is no run evidence to collect.")
        for name in SAFE_LOGS:
            data = self.read(self.logs + "/" + name, required=False)
            if data:
                (self.output / name).write_bytes(data)
        done = self.done(required=False)
        if done is not None:
            self.state.update({"phase": "stopped", "watchdog": done})
            before = (self.output / "statusbar-before.txt").read_text() if (self.output / "statusbar-before.txt").exists() else ""
            after = (self.output / "statusbar-after.txt").read_text() if (self.output / "statusbar-after.txt").exists() else ""
            service_matches = ("start/running" in before and "start/running" in after) or ("stop/waiting" in before and "stop/waiting" in after)
            baseline_path, after_path = self.output / "native-baseline.tsv", self.output / "awesome-after.tsv"
            baseline = [line.split("|") for line in baseline_path.read_text().splitlines()] if baseline_path.exists() else []
            native_after = {row[0]: row for line in after_path.read_text().splitlines() if len(row := line.split("|")) == 4} if after_path.exists() else {}
            wm_matches = bool(baseline) and all(row[0] == "awesome" and len(row) == 5 and row[1] in native_after
                         and native_after[row[1]][1:3] == row[2:4]
                         and (native_after[row[1]][3] in ("T", "t")) == (row[4] in ("T", "t")) for row in baseline)
            lock_absent = self.remote("[ ! -e " + shlex.quote(LOCK) + " ] && [ ! -L " + shlex.quote(LOCK) + " ]", check=False).returncode == 0
            self.state["recovery_checks"] = {"statusbar_baseline_matches": service_matches, "awesome_baseline_matches": wm_matches,
                                             "launcher_lock_absent": lock_absent,
                                             "passed": done["recovery_complete"] and service_matches and wm_matches and lock_absent,
                                             "native_home_and_touch_require_operator_observation": True}
            if self.state.get("content_mode", "demo") == "distinct_fixture" and self.state["recovery_checks"]["passed"]:
                snapshot = self.state.get("post_restore_process_snapshot")
                if not snapshot or not snapshot.get("passed"):
                    snapshot = self.post_restore_process_snapshot()
                self.state["recovery_checks"]["owned_process_snapshot_passed"] = snapshot["passed"]
                self.state["recovery_checks"]["passed"] = snapshot["passed"]
        self.save()
        return {"phase": self.state["phase"], "watchdog": done, "recovery_checks": self.state.get("recovery_checks")}

    def confirm(self):
        require(self.args.observer.strip() and self.args.note.strip(), "Record who directly observed the physical result and what was observed.")
        require(self.args.home or self.args.touch or self.args.panel or self.args.input_source, "Select an actual physical observation.")
        if self.args.home or self.args.touch:
            require(self.state.get("phase") == "stopped" and self.state.get("recovery_checks", {}).get("passed"),
                    "Confirm native Home or touch only after successful process and service recovery.")
        if self.args.panel:
            checkpoint = self.state["checkpoints"].get(self.args.checkpoint)
            require(checkpoint and checkpoint.get("framebuffer"), "Panel observation must identify an existing demo framebuffer checkpoint.")
        if self.args.input_source:
            require(self.state["checkpoints"].get(self.args.checkpoint) and self.state.get("assisted_physical_input"),
                    "Physical input observation requires an assisted run and an existing checkpoint.")
        observation = {"observer": self.args.observer.strip(), "note": self.args.note.strip(),
                       "home_restored": self.args.home, "native_touch_restored": self.args.touch,
                       "physical_panel_observed": self.args.panel, "checkpoint": self.args.checkpoint,
                       "physical_input_observed": self.args.input_source,
                       "source": "explicit operator observation; not inferred from framebuffer or process state"}
        self.state["observations"].append(observation)
        if self.args.panel:
            self.state["physical_panel_review"] = "operator observation recorded"
        self.save()
        return observation


def arguments():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--run-id", required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--lease-dir", type=Path, required=True)
    parser.add_argument("--lease-owner", required=True)
    parser.add_argument("--lease-token-file", type=Path, required=True)
    route = parser.add_mutually_exclusive_group(required=True)
    route.add_argument("--ssh-helper", type=Path, help="Executable helper accepting one remote shell command and forwarding stdin.")
    route.add_argument("--ssh-target", help="An already configured, noninteractive SSH target with a pinned host key.")
    parser.add_argument("--ssh-option", action="append", default=[])
    commands = parser.add_subparsers(dest="command", required=True)
    preflight = commands.add_parser("preflight")
    preflight.add_argument("--profile", choices=tuple(GEOMETRIES), required=True)
    preflight.add_argument("--expected-firmware", required=True)
    preflight.add_argument("--operator-model", required=True)
    deploy = commands.add_parser("deploy")
    deploy.add_argument("--package", type=Path, required=True)
    deploy.add_argument("--expected-package-sha256", type=digest_argument, required=True)
    deploy.add_argument("--expected-binary-sha256", type=digest_argument, required=True)
    deploy.add_argument("--framebuffer-helper", type=Path, required=True)
    deploy.add_argument("--expected-helper-sha256", type=digest_argument, required=True)
    deploy.add_argument("--wrapper", type=Path, required=True)
    deploy.add_argument("--watchdog", type=Path, required=True)
    deploy.add_argument("--event-wrapper", type=Path, required=True)
    deploy.add_argument("--content-mode", choices=CONTENT_MODES, default="demo")
    deploy.add_argument("--fixture-directory", type=Path)
    deploy.add_argument("--expected-fixture-sha256", type=digest_argument)
    deploy.add_argument("--socket-guard", type=Path)
    deploy.add_argument("--expected-guard-sha256", type=digest_argument)
    deploy.add_argument("--approve-runtime-pair", action="store_true")
    start = commands.add_parser("start")
    start.add_argument("--screen", choices=("shelf", "reader"), default="shelf")
    start.add_argument("--exit-after-ms", type=int, default=30000)
    start.add_argument("--init-timeout", type=int, default=90)
    start.add_argument("--session-timeout", type=int, default=180)
    start.add_argument("--assisted", action="store_true")
    start.add_argument("--performance-trace", action="store_true",
                       help="Opt into bounded per-stage timing for this isolated application run.")
    checkpoint = commands.add_parser("checkpoint")
    checkpoint.add_argument("--name", required=True)
    checkpoint.add_argument("--require", choices=("ready", "reader", "suspended", "resumed"), default="ready")
    checkpoint.add_argument("--wait-seconds", type=int, choices=range(0, 121), default=0)
    checkpoint.add_argument("--capture", action="store_true")
    checkpoint.add_argument("--compare-to")
    checkpoint.add_argument("--expect", choices=("retained", "next-page"), default="retained")
    commands.add_parser("stop")
    commands.add_parser("release", help="Stop this run; the external coordinator still owns shared-lease return.")
    commands.add_parser("collect")
    confirm = commands.add_parser("confirm")
    confirm.add_argument("--observer", required=True)
    confirm.add_argument("--note", required=True)
    confirm.add_argument("--home", action="store_true")
    confirm.add_argument("--touch", action="store_true")
    confirm.add_argument("--panel", action="store_true")
    confirm.add_argument("--checkpoint")
    confirm.add_argument("--input-source", choices=("finger-touch", "physical-power-button", "physical-sleep-and-wake"))
    return parser.parse_args()


def main():
    args = arguments()
    harness = None
    try:
        harness = Harness(args)
        result = getattr(harness, "stop" if args.command == "release" else args.command)()
        print(json.dumps(result, indent=2, ensure_ascii=True))
    except Exception as error:
        if harness is not None and harness.state:
            harness.state["last_failure"] = {"operation": args.command, "type": type(error).__name__, "message": str(error)}
            try:
                harness.save()
            except OSError:
                pass
            # Confirmation only records host evidence; rejecting it must not
            # stop an otherwise active device session.
            if args.command != "confirm" and harness.state.get("phase") in ("starting", "running"):
                try:
                    harness.stop()
                except Exception as recovery_error:
                    harness.state["failure_recovery"] = {"type": type(recovery_error).__name__, "message": str(recovery_error)}
                    try:
                        harness.save()
                    except OSError:
                        pass
        print(json.dumps({"status": "failed", "operation": args.command, "type": type(error).__name__, "message": str(error),
                          "recovery": "Use stop/collect with the same lease; the independent watchdog retains its absolute deadline."}, indent=2), file=sys.stderr)
        raise SystemExit(1)


if __name__ == "__main__":
    main()
