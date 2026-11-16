#!/usr/bin/env python3
"""Private Perf05 measurement orchestration for test-env.

The archive is extracted before measurement. This is not a CBZ import feature.
Only hashes, synthetic identities, numeric diagnostics, and timing samples may
be exported from the private run directory. Archive contents, the SQLite copy,
application logs, and actual comic pixels must never become repository assets.

The guest runner uses actual XTest input. The ignored App-library runner uses
scripted App intents with a software window and Desktop DeviceControl. Neither
runner measures a physical Kindle, SEND, an EPDC waveform, or native latency.
"""

import argparse
import ctypes
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import signal
import sqlite3
import stat
import struct
import subprocess
import sys
import time
import zipfile


KIND = "perf05_private_guest_png_v1"
ACCOUNT = "guest"
COMIC = "999990005"
EPISODE = "99999000501"
REVISION = "perf05-png-v1"
COVER_URL = "https://perf05.invalid/local-cover.png"
TEST_NAME = "app::perf05_benchmark::perf05_controlled_host"
DIMENSIONS = (2480, 3508)
WINDOW = (1860, 2480)
NATURAL_CADENCE_MS = 1500
MAX_MEMBER = 64 * 1024 * 1024
MAX_TOTAL = 512 * 1024 * 1024
MAX_COVER = 8 * 1024 * 1024


def require(condition, message):
    if not condition:
        raise AssertionError(message)


def remote_only():
    require(sys.platform.startswith("linux") and os.environ.get("BILICOMICS_REMOTE_VERIFICATION") == "1",
            "Run only in the explicitly designated Linux verification environment")


def digest(path):
    result = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            result.update(chunk)
    return result.hexdigest()


def valid_sha(value):
    require(re.fullmatch(r"[0-9a-f]{64}", value or "") is not None, "A complete lowercase SHA-256 is required")
    return value


def regular(path):
    require(path.is_file() and not path.is_symlink() and path.stat().st_nlink == 1,
            "An input must be an unlinked regular file")


def write_json(path, value):
    path.write_text(json.dumps(value, ensure_ascii=True, indent=2) + "\n", encoding="utf-8")
    path.chmod(0o600)


def records(path, final=False):
    if not path.exists():
        return []
    content = path.read_bytes()
    if final:
        require(not content or content.endswith(b"\n"), "A diagnostic ends with a partial record")
    return [json.loads(line) for line in content.splitlines(keepends=True) if line.endswith(b"\n") and line.strip()]


def wait_for(description, predicate, process=None, timeout=90):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if process is not None:
            require(process.poll() is None, "The application exited while waiting for " + description)
        result = predicate()
        if result:
            return result
        time.sleep(0.01)
    raise AssertionError("Timed out waiting for " + description)


def private_directory(path):
    path = path.resolve()
    require(not path.exists(), "Use a fresh private output directory")
    path.mkdir(parents=True, mode=0o700)
    path.chmod(0o700)
    return path


def fixture_manifest(root):
    root = root.resolve()
    regular(root / "manifest.json")
    manifest = json.loads((root / "manifest.json").read_text())
    require(manifest.get("kind") == KIND and manifest.get("page_count") == 8,
            "The private input is not the fixed eight-page Perf05 fixture")
    scope = manifest.get("synthetic_scope", {})
    require(scope.get("account") == ACCOUNT and scope.get("comic_id") == COMIC
            and scope.get("episode_id") == EPISODE and scope.get("remote_identity_claimed") is False,
            "The private fixture does not bind the fixed synthetic numeric identifiers")
    require(manifest.get("dimensions") == list(DIMENSIONS), "The fixture source geometry changed")
    expected = {"manifest.json"}
    for row in manifest["files"]:
        relative = PurePosixPath(row["path"])
        require(not relative.is_absolute() and ".." not in relative.parts and "\\" not in row["path"],
                "A fixture inventory path escapes its root")
        path = root.joinpath(*relative.parts)
        regular(path)
        require(path.stat().st_size == row["bytes"] and digest(path) == row["sha256"],
                "A private fixture file differs from its manifest")
        expected.add(relative.as_posix())
    actual = {str(path.relative_to(root).as_posix()) for path in root.rglob("*") if path.is_file()}
    require(actual == expected and not any(path.is_symlink() for path in root.rglob("*")),
            "The private fixture inventory contains an unbound file or symlink")
    require(not (root / "data/account/session.json").exists(), "A guest fixture contains a session")
    return manifest


def seed(arguments):
    archive_path = arguments.cbz.resolve()
    regular(archive_path)
    require(digest(archive_path) == valid_sha(arguments.expected_cbz_sha256), "The private CBZ hash differs")
    root = private_directory(arguments.output)
    data = root / "data"
    pages = data / "accounts" / hashlib.sha256(ACCOUNT.encode()).hexdigest() / "pages"
    pages.mkdir(parents=True, mode=0o700)
    # Reuse the repository's existing guest receipt schema. Its v2 -> current
    # migration is real App work and is identical for every cloned run.
    sys.path.insert(0, str(arguments.verification_dir.resolve()))
    from reader_guest_fixtures import SCHEMA
    values = []
    ignored_metadata = []
    metadata_limits = {"ComicInfo.xml": 1024 * 1024, "README.txt": 64 * 1024}
    with zipfile.ZipFile(archive_path) as archive:
        entries = []
        seen = set()
        members = archive.infolist()
        require(len(members) <= 64 and sum(item.file_size for item in members) <= MAX_TOTAL,
                "The entire Perf05 archive exceeds its member or uncompressed byte budget")
        for item in members:
            relative = PurePosixPath(item.filename)
            require(not relative.is_absolute() and ".." not in relative.parts and "\\" not in item.filename,
                    "A CBZ member has an unsafe path")
            require(item.filename not in seen and not item.flag_bits & 1, "A CBZ member is repeated or encrypted")
            seen.add(item.filename)
            require(not stat.S_ISLNK(item.external_attr >> 16), "A CBZ member is a symlink")
            if item.is_dir():
                require(item.file_size == 0, "A standard CBZ directory entry must not contain data")
                continue
            if item.filename in metadata_limits:
                require(0 <= item.file_size <= metadata_limits[item.filename],
                        "An allowlisted root metadata member exceeds its byte budget")
                # The archive hash binds these exact root metadata objects.
                # Never read, extract, parse, execute, or count them as pages.
                ignored_metadata.append({"name": item.filename, "bytes": item.file_size})
                continue
            require(relative.suffix.lower() == ".png" and 0 < item.file_size <= MAX_MEMBER,
                    "Perf05 must contain only bounded PNG page members")
            entries.append(item)
        require(len(entries) == 8 and sum(item.file_size for item in entries) <= MAX_TOTAL,
                "Perf05 must contain exactly eight bounded PNG pages")
        entries.sort(key=lambda item: [(0, int(part)) if part.isdigit() else (1, part.lower())
                                      for part in re.split(r"(\d+)", item.filename)])
        for index, item in enumerate(entries):
            content = archive.read(item)
            require(len(content) == item.file_size and content[:8] == b"\x89PNG\r\n\x1a\n"
                    and content[12:16] == b"IHDR" and struct.unpack(">II", content[16:24]) == DIMENSIONS,
                    "A Perf05 source is not an original 2480 by 3508 PNG")
            encoded_sha = hashlib.sha256(content).hexdigest()
            path = pages / (encoded_sha + ".png")
            if not path.exists():
                path.write_bytes(content)
                path.chmod(0o600)
            values.append({"index": index, "page_id": "perf05-page-" + str(index + 1).zfill(2),
                           "file_name": path.name, "sha256": encoded_sha, "bytes": len(content)})
    cover = next((value for value in values if value["bytes"] <= MAX_COVER), None)
    require(cover is not None, "No original Perf05 PNG fits the existing eight-MiB sleep-cover input budget")
    identity = f"sleep-cover-v1:{len(ACCOUNT)}:{ACCOUNT}:{len(COMIC)}:{COMIC}:{COVER_URL}"
    cover_name = hashlib.sha256(identity.encode()).hexdigest() + ".png"
    covers = data / "covers"
    covers.mkdir(mode=0o700)
    shutil.copyfile(pages / cover["file_name"], covers / cover_name)
    (covers / cover_name).chmod(0o600)
    stamp = 1700000000
    comic = {"id": COMIC, "title": "Private Perf05 fixture", "author": "Private measurement",
             "cover_url": COVER_URL, "description": "Controlled offline performance input",
             "latest_episode_id": EPISODE, "latest_episode_title": "Private chapter",
             "latest_order": 1.0, "chapter_count": 1, "favorite": True, "finished": False,
             "updated_at": stamp, "last_episode_id": EPISODE, "last_read_at": stamp,
             "extra": {"fixture_only": True}}
    episode = {"id": EPISODE, "comic_id": COMIC, "title": "Private chapter", "order": 1.0,
               "access": "free", "read": False, "extra": {"fixture_only": True, "page_count": 8}}
    index = {"episode_id": EPISODE, "revision": REVISION,
             "images": [{"id": value["page_id"], "index": value["index"], "width": DIMENSIONS[0],
                         "height": DIMENSIONS[1]} for value in values], "extra": {"fixture_only": True}}
    prefs = {"onboarded": True, "reader_guidance_seen": True, "launch": "resume", "preload": 1,
             "mode": "page", "fit": "page", "direction": "ltr", "refresh_mode": "fast",
             "dither": True, "dither_method": "bayer8", "every": "6", "jpeg_acceleration": False,
             "landscape": "spread", "cover_alone": False, "sleep_cover": True, "footer": False}
    anchor = {"episode_id": EPISODE, "revision": REVISION, "page_id": values[0]["page_id"], "page_index": 0,
              "source_x": 0.0, "source_y": 0.0, "zoom_ratio": 1.0, "rotation": 0, "mode": "page",
              "finished": False, "updated_at": stamp}
    connection = sqlite3.connect(data / "state.sqlite3")
    try:
        connection.executescript(SCHEMA)
        connection.execute("INSERT INTO comics VALUES(?,?,?,?,?)", (ACCOUNT, COMIC, json.dumps(comic), stamp, stamp))
        connection.execute("INSERT INTO comic_details VALUES(?,?,?)",
                           (ACCOUNT, COMIC, json.dumps({"comic": comic, "episodes": [episode], "extra": {}})))
        connection.execute("INSERT INTO catalog_members VALUES(?,?,?,?)", (ACCOUNT, "shelf", COMIC, 0))
        connection.execute("INSERT INTO image_indices VALUES(?,?,?,?,?)", (ACCOUNT, EPISODE, REVISION, json.dumps(index), stamp))
        connection.execute("INSERT INTO reading_anchors VALUES(?,?,?,?,?,?)",
                           (ACCOUNT, COMIC, EPISODE, REVISION, json.dumps(anchor), stamp))
        connection.execute("INSERT INTO preferences VALUES(?,?,?)", (ACCOUNT, "preferences", json.dumps(prefs)))
        for value in values:
            receipt = {"episode_id": EPISODE, "revision": REVISION, "page_id": value["page_id"],
                       "page_index": value["index"], "file_name": value["file_name"], "sha256": value["sha256"],
                       "bytes": value["bytes"], "width": DIMENSIONS[0], "height": DIMENSIONS[1],
                       "retained": True, "verified_at": 0, "decode_validation_version": 0}
            connection.execute("INSERT INTO page_receipts VALUES(?,?,?,?,?,?)",
                               (ACCOUNT, EPISODE, REVISION, value["page_id"], value["index"], json.dumps(receipt)))
            connection.execute("INSERT INTO page_identity_history VALUES(?,?,?,?,?,?)",
                               (ACCOUNT, EPISODE, REVISION, value["page_id"], value["sha256"], value["bytes"]))
        connection.commit()
        connection.execute("PRAGMA wal_checkpoint(TRUNCATE)")
    finally:
        connection.close()
    inventory = [{"path": path.relative_to(root).as_posix(), "sha256": digest(path), "bytes": path.stat().st_size}
                 for path in sorted(data.rglob("*")) if path.is_file()]
    write_json(root / "manifest.json", {"kind": KIND, "private_material": True, "do_not_commit": True,
               "synthetic_scope": {"account": ACCOUNT, "comic_id": COMIC, "episode_id": EPISODE,
                                   "remote_identity_claimed": False,
                                   "purpose": "Fixed numeric syntax for isolated offline guest measurements"},
               "archive_sha256": arguments.expected_cbz_sha256, "page_count": 8, "dimensions": list(DIMENSIONS),
               "ignored_archive_metadata": ignored_metadata,
               "pages": values, "preferences": prefs, "schema_seed_version": 2, "files": inventory,
               "cover": {"page_index": cover["index"], "encoded_sha256": cover["sha256"], "bytes": cover["bytes"],
                         "managed_file_name": cover_name, "fixture_role": "Original PNG as local sleep-cover candidate"},
               "preparation_scope": "CBZ extraction and seed construction occurred outside application measurement",
               "proof_scope": "Receipt validation markers remain zero for real application verification"})
    fixture_manifest(root)
    print(json.dumps({"status": "seeded_private", "fixture_manifest_sha256": digest(root / "manifest.json"),
                      "archive_sha256": arguments.expected_cbz_sha256, "page_count": 8}))


def source_identity(arguments):
    regular(arguments.binary)
    binary_sha = digest(arguments.binary)
    require(binary_sha == valid_sha(arguments.expected_binary_sha256), "The sealed executable SHA-256 differs")
    with arguments.binary.open("rb") as source:
        require(source.read(4) == b"\x7fELF", "Use the sealed Linux executable")
    manifest_sha = digest(arguments.source_manifest)
    require(manifest_sha == valid_sha(arguments.source_fingerprint), "The source manifest fingerprint differs")
    source_inputs = []
    source_root = arguments.source_root.resolve()
    for line in arguments.source_manifest.read_text().splitlines():
        match = re.fullmatch(r"([0-9a-f]{64}) [ *](.+)", line)
        require(match is not None, "The source inventory is not a sha256sum manifest")
        relative = PurePosixPath(match.group(2).removeprefix("./"))
        require(not relative.is_absolute() and ".." not in relative.parts and "\\" not in match.group(2),
                "The source inventory escapes its root")
        path = source_root.joinpath(*relative.parts)
        regular(path)
        require(path.resolve().is_relative_to(source_root) and digest(path) == match.group(1),
                "A frozen source input differs from its inventory")
        source_inputs.append(relative.as_posix())
    require(source_inputs and len(source_inputs) == len(set(source_inputs)),
            "The source inventory is empty or contains duplicate inputs")
    require(re.fullmatch(r"[0-9a-f]{40}", arguments.source_commit) is not None, "A complete base commit is required")
    return {"binary_sha256": binary_sha, "source_fingerprint": manifest_sha, "source_commit": arguments.source_commit,
            "harness_sha256": digest(Path(__file__)), "source_input_count": len(source_inputs),
            "runner_kind": arguments.command, "build_profile": arguments.build_profile}


def clone_data(fixture, output, mode, cover):
    manifest = fixture_manifest(fixture)
    data = output / "data"
    shutil.copytree(fixture / "data", data)
    with sqlite3.connect(data / "state.sqlite3") as connection:
        preferences = json.loads(connection.execute("SELECT payload FROM preferences WHERE account_key=? AND key='preferences'",
                                                   (ACCOUNT,)).fetchone()[0])
        preferences.update(refresh_mode=mode, sleep_cover=cover == "on")
        connection.execute("UPDATE preferences SET payload=? WHERE account_key=? AND key='preferences'",
                           (json.dumps(preferences), ACCOUNT))
    return data, manifest


def configuration(arguments, manifest, scope):
    return {"scope": scope, "archive_sha256": manifest["archive_sha256"], "pages": manifest["pages"],
            "source_dimensions": list(DIMENSIONS), "profile": "scribe", "physical_window": list(WINDOW),
            "scale_factor": 2, "orientation": "portrait", "mode": "page", "fit": "page", "direction": "ltr",
            "refresh_mode": arguments.refresh_mode, "saved_dither": True, "saved_dither_method": "bayer8",
            "preload": 1, "jpeg_acceleration": False, "sleep_cover": arguments.cover,
            "natural_cadence_ms": NATURAL_CADENCE_MS,
            "natural_cadence_scope": "Fixed host idle after each completed checkpoint; no prediction wait",
            "cache_scope": "Fresh process and cloned seed; OS page cache is not controlled",
            "input_identity_reads": "Source files are hashed before timing; this is not a cold-disk measurement",
            "spread_scope": "Not selected; production spread prediction is intentionally ineligible",
            "cover_geometry_limit": [1536, 2048], "physical_device_used": False, "send_measured": False}


def phase_markers(measurements):
    values = []
    for row in measurements.get("events", []):
        if row.get("event") == "reader_start":
            values.append({"name": "first_read", "monotonic_ns": row["stamp"]["monotonic_ns"]})
        elif row.get("event") == "scripted_intent":
            values.append({"name": row["phase"], "monotonic_ns": row["before"]["monotonic_ns"]})
    for row in measurements.get("stimuli", []):
        values.append({"name": row["name"], "monotonic_ns": row["before_monotonic_ns"]})
    return sorted(values, key=lambda row: row["monotonic_ns"])


def trace_analysis(path, strict=True, markers=None):
    values = records(path, final=strict)
    require(values, "The opt-in production performance trace is empty")
    if strict:
        terminal = [value for value in values if value.get("stage") == "trace_stopped"]
        require(len(terminal) == 1 and terminal[0].get("data", {}).get("reason") == "finished",
                "The production timing trace is incomplete or exhausted its limit")
    stages = {}
    operations = {}
    phase_by_operation = {}
    for value in values:
        if value.get("kind") not in ("reader", "reader-prepare"):
            continue
        at = value.get("monotonic_ns")
        if isinstance(at, int):
            marker = next((row for row in reversed(markers or []) if row["monotonic_ns"] <= at), None)
            if marker is not None:
                phase_by_operation.setdefault(value["operation_id"], marker["name"])
    for value in values:
        key = str(value.get("operation_id"))
        operations.setdefault(key, []).append(value)
        stage = value.get("stage")
        data = value.get("data") or {}
        if stage in ("ui_image_import", "reader_preparation", "next_frame_preparation", "worker_end", "speculative_worker_end"):
            elapsed = data.get("elapsed", data)
            if isinstance(elapsed, dict) and isinstance(elapsed.get("wall_us"), int):
                stages.setdefault(stage, []).append({"operation_id": value["operation_id"], "kind": value["kind"],
                                                     "phase": phase_by_operation.get(value["operation_id"]),
                                                     "thread_id": value.get("thread_id"), "monotonic_ns": value.get("monotonic_ns"),
                                                     "wall_us": elapsed["wall_us"], "cpu_us": elapsed.get("cpu_us"),
                                                     "copied_bytes": data.get("copied_bytes"), "profile": data.get("profile"),
                                                     "outcome": data.get("outcome"), "cancelled": data.get("cancelled")})
    latencies = []
    reader_intervals, cover_intervals = [], []
    for key, operation in operations.items():
        def find(stage):
            return next((value for value in operation if value.get("stage") == stage), None)
        request, published = find("navigation_request") or find("render_request"), find("reader_model_published")
        if request and published and all(isinstance(value.get("monotonic_ns"), int) for value in (request, published)):
            latencies.append({"operation_id": int(key), "action": (request.get("data") or {}).get("action"),
                              "phase": phase_by_operation.get(int(key)),
                              "request_to_model_us": (published["monotonic_ns"] - request["monotonic_ns"]) // 1000})
        begin, end = find("worker_begin") or find("speculative_worker_begin"), find("worker_end") or find("speculative_worker_end")
        if begin and end and isinstance(begin.get("monotonic_ns"), int) and isinstance(end.get("monotonic_ns"), int):
            interval = (begin["monotonic_ns"], end["monotonic_ns"])
            if str(begin.get("kind", "")).startswith("cover") or str(begin.get("kind", "")).startswith("sleep-cover"):
                cover_intervals.append(interval)
            elif begin.get("kind") in ("reader", "reader-prepare"):
                reader_intervals.append(interval)
    overlap = sum(max(0, min(left[1], right[1]) - max(left[0], right[0]))
                  for left in reader_intervals for right in cover_intervals) // 1000
    counts = {stage: sum(value.get("stage") == stage for value in values) for stage in (
        "navigation_request", "prepared_frame_lookup", "prepared_frame_hit", "next_frame_retained",
        "next_frame_admission_skipped", "obsolete_render_cancel_requested", "request_superseded", "reader_cancelled_result",
        "reader_result_superseded", "reader_render_retired", "job_cancelled", "cancelled_before_delivery",
        "speculative_worker_cancelled")}
    reader_cancellations = {stage: sum(value.get("stage") == stage and value.get("kind") == "reader" for value in values)
                            for stage in ("reader_cancelled_result", "job_cancelled", "cancelled_before_delivery")}
    phase_hits = []
    for phase in dict.fromkeys(row["name"] for row in markers or []):
        selected = [value for value in values if phase_by_operation.get(value.get("operation_id")) == phase]

        def ids(stage, kind="reader"):
            return {value["operation_id"] for value in selected if value.get("kind") == kind and value.get("stage") == stage}

        navigation, lookups, hits = ids("navigation_request"), ids("prepared_frame_lookup"), ids("prepared_frame_hit")
        predictions = [value for value in selected if value.get("kind") == "reader-prepare"]
        cancelled_predictions = {value["operation_id"] for value in predictions
                                 if value.get("stage") == "speculative_worker_cancelled"
                                 or (value.get("stage") == "next_frame_preparation"
                                     and (value.get("data") or {}).get("cancelled") is True)}
        phase_hits.append({"phase": phase, "navigation_requests": len(navigation), "lookup_attempts": len(lookups),
                           "prepared_frame_hits": len(hits), "hit_operation_ids": sorted(hits),
                           "lookup_hit_ratio": len(lookups & hits) / len(lookups) if lookups else None,
                           "navigation_hit_ratio": len(navigation & hits) / len(navigation) if navigation else None,
                           "prediction_retained_events": len(ids("next_frame_retained", "reader-prepare")),
                           "prediction_cancelled_attempts": len(cancelled_predictions)})
    return {"trace_sha256": digest(path), "complete_records": len(values), "stage_samples": stages, "counts": counts,
            "request_to_model": latencies, "prefetch_hit_proof": "prepared_frame_hit, never lookup candidate alone",
            "phase_markers": markers or [], "phase_hits": phase_hits,
            "phase_attribution": "First record of the owning reader or prediction operation after a host stimulus marker",
            "hit_ratio_scope": "Only actual prepared_frame_hit operations count; prediction retention and cancellation are separate",
            "reader_cancellation_counts": reader_cancellations, "cancel_observed": any(reader_cancellations.values()),
            "cover_worker_overlap": {"status": "observed" if cover_intervals else "unsupported_diagnostic_gap",
                                     "pairwise_worker_interval_intersection_us": overlap if cover_intervals else None,
                                     "cpu_overlap_claimed": False,
                                     "scope": "Actual traced worker intervals; absent instrumentation is not zero"}}


def stop(process, log):
    if process.poll() is None:
        process.send_signal(signal.SIGTERM)
        try:
            process.wait(timeout=20)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=5)
    log.close()


def run_library(arguments):
    identity = source_identity(arguments)
    identity["library_runner_sha256"] = digest(arguments.source_root / "src/app/perf05_benchmark.rs")
    require(digest(arguments.fixture / "manifest.json") == valid_sha(arguments.expected_fixture_manifest_sha256),
            "The pinned private fixture manifest differs")
    root = private_directory(arguments.output)
    data, manifest = clone_data(arguments.fixture, root, arguments.refresh_mode, arguments.cover)
    environment = os.environ.copy()
    environment.update(PERF05_FIXTURE=str(arguments.fixture.resolve()), PERF05_DATA=str(data), PERF05_OUTPUT=str(root),
                       PERF05_COVER=arguments.cover, PERF05_REFRESH_MODE=arguments.refresh_mode,
                       PERF05_BINARY_SHA256=identity["binary_sha256"], PERF05_SOURCE_FINGERPRINT=identity["source_fingerprint"],
                       PERF05_SOURCE_COMMIT=identity["source_commit"], PERF05_HARNESS_SHA256=identity["harness_sha256"])
    command = ["unshare", "-n", "--", str(arguments.binary.resolve()), "--exact", TEST_NAME,
               "--ignored", "--nocapture", "--test-threads=1"]
    with (root / "application.log").open("w") as log:
        process = subprocess.Popen(command, env=environment, stdout=log, stderr=subprocess.STDOUT)
        try:
            code = process.wait(timeout=arguments.timeout)
        except BaseException:
            stop(process, log)
            raise
    require(code == 0, "The controlled-host ignored runner failed; inspect the private log")
    raw = json.loads((root / "library-result.json").read_text())
    require(raw.get("status") == "passed" and raw.get("process_id") == process.pid,
            "The controlled-host report did not come from this successful process")
    require(raw.get("scope") == "controlled_host_app_library" and raw.get("physical_device_used") is False,
            "The controlled fixture reported another verification scope")
    report = {"status": "passed", "identity": identity,
              "configuration": configuration(arguments, manifest, "controlled_host_app_library"),
              "fixture_manifest_sha256": digest(arguments.fixture / "manifest.json"),
              "measurements": raw, "performance": trace_analysis(root / "performance.jsonl", markers=phase_markers(raw)),
              "private_material": True, "export_policy": "Export only this numeric/hash report after review"}
    write_json(root / "report.json", report)
    print(json.dumps({"status": report["status"], "scope": report["configuration"]["scope"],
                      "report": str(root / "report.json")}))


def reader_state(path, process, page=None):
    value = next((value for value in reversed(records(path)) if value.get("event") == "reader-ready"
                  and value.get("process_id") == process.pid), None)
    if value is None:
        return None
    state = value.get("reader_state") or {}
    if state.get("comic_id") != COMIC or value.get("reader_loading") or state.get("rendering"):
        return None
    if page is not None and state.get("page_number") != page:
        return None
    if state.get("menu") or state.get("sheet") or state.get("transition"):
        return None
    return value


def run_guest(arguments):
    require(arguments.cover == "off", "Standalone guest desktop cannot exercise the Kindle-only sleep-cover route")
    require(os.environ.get("DISPLAY", "").startswith(":"), "Run under an isolated Xvfb display")
    identity = source_identity(arguments)
    require(digest(arguments.fixture / "manifest.json") == valid_sha(arguments.expected_fixture_manifest_sha256),
            "The pinned private fixture manifest differs")
    root = private_directory(arguments.output)
    data, manifest = clone_data(arguments.fixture, root, arguments.refresh_mode, arguments.cover)
    sys.path.insert(0, str(arguments.verification_dir.resolve()))
    from reader_guest_fixtures import X11
    x11 = X11()
    x11.lib.XSync.argtypes = [ctypes.c_void_p, ctypes.c_int]
    trace = root / "lifecycle.jsonl"
    performance = root / "performance.jsonl"
    environment = os.environ.copy()
    environment.update(SLINT_BACKEND="winit-software", SLINT_SCALE_FACTOR="2", WINIT_X11_SCALE_FACTOR="2")
    command = ["unshare", "-n", "--", str(arguments.binary.resolve()), "--offline", "--device", "scribe", "--screen", "reader",
               "--data-dir", str(data), "--lifecycle-trace", str(trace), "--performance-trace", str(performance),
               "--exit-after-ms", str(arguments.timeout * 1000)]
    log = (root / "application.log").open("w")
    launch_marker = time.monotonic_ns()
    process = subprocess.Popen(command, env=environment, stdout=log, stderr=subprocess.STDOUT)
    checkpoints, stimuli = [], [{"name": "first_read", "before_monotonic_ns": launch_marker,
                                 "input": "Actual standalone process launch"}]
    try:
        parent_ns = os.readlink("/proc/self/ns/net")
        wait_for("the isolated network namespace", lambda: os.readlink(f"/proc/{process.pid}/ns/net") != parent_ns,
                 process=process, timeout=5)
        window = wait_for("the actual guest window", lambda: x11.window(process), process=process)
        require((window["width"], window["height"]) == WINDOW, "The guest physical window differs from 1860 by 2480")

        def checkpoint(name, page):
            ready = wait_for(name, lambda: reader_state(trace, process, page), process=process)
            state = ready["reader_state"]
            require(state["refresh_mode"] == arguments.refresh_mode and state["fit"] == "page"
                    and state["dither_method"] == "bayer8" and state["dither"] is True
                    and state["hardware_y4_requested"] is False and state["frame_width"] == WINDOW[0]
                    and state["frame_height"] == WINDOW[1], "The actual guest Reader configuration differs")
            # Pixel settling is correctness evidence outside the reported
            # production request/prepare/import timing spans.
            started, previous, count, capture, stable = time.monotonic(), None, 0, None, False
            path = root / "private-current-frame.png"
            while time.monotonic() - started < 20:
                capture = x11.capture(window, path)
                count = count + 1 if capture["rgb_sha256"] == previous else 1
                previous = capture["rgb_sha256"]
                if time.monotonic() - started >= 1.5 and count >= 3:
                    stable = True
                    break
                time.sleep(0.25)
            require(stable, "The actual guest pixels did not settle")
            checkpoints.append({"name": name, "page_number": page, "rgb_sha256": capture["rgb_sha256"],
                                "physical_size": capture["size"], "anchor": {key: state[key] for key in
                                ("page_index", "source_x", "source_y", "zoom_ratio", "rotation")}})

        def click(name, x, y):
            begin = time.monotonic_ns()
            x11.click(window, window["width"] * x, window["height"] * y)
            stimuli.append({"name": name, "before_monotonic_ns": begin, "after_monotonic_ns": time.monotonic_ns(),
                            "input": "Actual XTest motion, button down and up"})
            return begin

        checkpoint("first_read", 1)
        for page in (2, 3, 4):
            time.sleep(NATURAL_CADENCE_MS / 1000)
            phase = "natural_next_" + str(page)
            click(phase, 0.88, 0.5)
            checkpoint(phase, page)
        click("open_return_menu", 0.5, 0.5)
        wait_for("the real menu before returning to page one", lambda: next((value for value in reversed(records(trace))
                 if value.get("event") == "reader-ready" and value.get("process_id") == process.pid), {})
                 .get("reader_state", {}).get("menu") is True, process=process)
        # The existing Scribe beginning row is centered 172 logical pixels
        # above the bottom. This remains a real widget click, not App dispatch.
        return_started = click("return_to_start", 0.1, 1 - (172 * 2 / WINDOW[1]))
        checkpoint("return_to_start", 1)
        returned_request = next((value for value in records(performance) if value.get("kind") == "reader"
                                and value.get("stage") == "navigation_request"
                                and (value.get("data") or {}).get("action") == "beginning"
                                and (value.get("monotonic_ns") or 0) >= return_started), None)
        require(returned_request is not None, "The actual beginning widget did not produce its reader request")
        wait_for("a genuinely retained page-one prediction after returning", lambda: any(
                 value.get("stage") == "next_frame_retained"
                 and (value.get("monotonic_ns") or 0) >= returned_request["monotonic_ns"]
                 for value in records(performance)), process=process)
        predicted_started = click("predicted_next", 0.88, 0.5)
        checkpoint("predicted_next", 2)
        predicted_trace = records(performance)
        predicted_request = next((value for value in predicted_trace if value.get("kind") == "reader"
                                  and value.get("stage") == "navigation_request"
                                  and (value.get("monotonic_ns") or 0) >= predicted_started), None)
        require(predicted_request is not None and any(value.get("stage") == "prepared_frame_hit"
                and value.get("operation_id") == predicted_request["operation_id"] for value in predicted_trace),
                "The forward page did not actually reuse a retained predicted frame")
        click("repeat_back", 0.12, 0.5)
        checkpoint("repeat_back", 1)
        click("repeat_next", 0.88, 0.5)
        checkpoint("repeat_next", 2)
        for index in range(4):
            click("rapid_next_" + str(index + 1), 0.88, 0.5)
        checkpoint("rapid_final", 6)
        time.sleep(3)
        checkpoint("idle_retained", 6)
        by_name = {row["name"]: row for row in checkpoints}
        for left, right in (("first_read", "return_to_start"), ("first_read", "repeat_back"),
                            ("natural_next_2", "predicted_next"), ("predicted_next", "repeat_next"),
                            ("rapid_final", "idle_retained")):
            require(by_name[left]["rgb_sha256"] == by_name[right]["rgb_sha256"],
                    "Repeated or retained actual reader pixels changed")
        process.send_signal(signal.SIGTERM)
        require(process.wait(timeout=20) == 0, "The standalone guest did not shut down cleanly")
    finally:
        stop(process, log)
        x11.close()
    measurements = {"checkpoints": checkpoints, "stimuli": stimuli,
                    "cancel": {"status": "not_exercised_in_this_group", "required_group": "controlled_host_app_library"},
                    "sleep_cover": {"status": "unsupported_guest_desktop", "elapsed_us": None}}
    report = {"status": "passed", "identity": identity,
              "configuration": configuration(arguments, manifest, "actual_xtest_guest_desktop"),
              "fixture_manifest_sha256": digest(arguments.fixture / "manifest.json"),
              "measurements": measurements,
              "performance": trace_analysis(performance, markers=phase_markers(measurements)), "private_material": True,
              "export_policy": "The private-current-frame PNG and raw inputs must remain private"}
    write_json(root / "report.json", report)
    print(json.dumps({"status": "passed", "scope": report["configuration"]["scope"], "report": str(root / "report.json")}))


def compare(arguments):
    baseline = json.loads(arguments.baseline.read_text())
    final = json.loads(arguments.final.read_text())
    require(baseline["status"] == final["status"] == "passed", "Both measured runs must pass their contracts")
    require(baseline["configuration"] == final["configuration"], "Before/after input, geometry, scope, or settings differ")
    for key in ("harness_sha256", "library_runner_sha256", "build_profile"):
        require(baseline["identity"].get(key) == final["identity"].get(key),
                "Before/after measurement harness or build profile differs")
    before = baseline["measurements"]["checkpoints"]
    after = final["measurements"]["checkpoints"]
    require([row["name"] for row in before] == [row["name"] for row in after], "Before/after checkpoints differ")
    pixels = []
    for left, right in zip(before, after):
        require(left["page_number"] == right["page_number"] and left["physical_size"] == right["physical_size"],
                "Before/after checkpoint source position or physical geometry differs")
        require(left["anchor"] == right["anchor"], "Before/after source anchors differ")
        equal = left["rgb_sha256"] == right["rgb_sha256"]
        pixels.append({"name": left["name"], "page_number": left["page_number"], "rgb_equal": equal,
                       "baseline_rgb_sha256": left["rgb_sha256"], "final_rgb_sha256": right["rgb_sha256"]})
    left_cover, right_cover = baseline["measurements"].get("sleep_cover", {}), final["measurements"].get("sleep_cover", {})
    require(left_cover.get("status") == right_cover.get("status"), "Before/after cover capability scopes differ")
    if left_cover.get("status") == "observed":
        require(left_cover["cover_enabled"] == right_cover["cover_enabled"]
                and left_cover["cover_physical_size"] == right_cover["cover_physical_size"],
                "Before/after cover preference or high-resolution geometry differs")
        pixels.append({"name": "sleep_cover", "rgb_equal": left_cover["rgb_sha256"] == right_cover["rgb_sha256"],
                       "baseline_rgb_sha256": left_cover["rgb_sha256"], "final_rgb_sha256": right_cover["rgb_sha256"]})
    report = {"status": "passed" if all(row["rgb_equal"] for row in pixels) else "pixel_mismatch",
              "scope": baseline["configuration"]["scope"], "configuration": baseline["configuration"],
              "baseline_identity": baseline["identity"], "final_identity": final["identity"], "pixels": pixels,
              "baseline_performance": baseline["performance"], "final_performance": final["performance"],
              "interpretation": "Compare like-for-like host samples. No physical panel or SEND performance claim."}
    require(not arguments.output.exists(), "Do not overwrite prior comparison evidence")
    write_json(arguments.output, report)
    require(report["status"] == "passed", "The final reader pixels differ from the baseline")
    print(json.dumps({"status": report["status"], "report": str(arguments.output)}))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    seed_parser = commands.add_parser("seed")
    seed_parser.add_argument("--cbz", type=Path, required=True)
    seed_parser.add_argument("--expected-cbz-sha256", required=True)
    seed_parser.add_argument("--verification-dir", type=Path, default=Path(__file__).parent)
    seed_parser.add_argument("--output", type=Path, required=True)
    seed_parser.set_defaults(function=seed)
    for name, function in (("run-library", run_library), ("run-guest", run_guest)):
        command = commands.add_parser(name)
        command.add_argument("--binary", type=Path, required=True)
        command.add_argument("--expected-binary-sha256", required=True)
        command.add_argument("--source-root", type=Path, required=True)
        command.add_argument("--source-manifest", type=Path, required=True)
        command.add_argument("--source-fingerprint", required=True)
        command.add_argument("--source-commit", required=True)
        command.add_argument("--build-profile", required=True)
        command.add_argument("--verification-dir", type=Path, default=Path(__file__).parent)
        command.add_argument("--fixture", type=Path, required=True)
        command.add_argument("--expected-fixture-manifest-sha256", required=True)
        command.add_argument("--refresh-mode", choices=("fast", "normal"), default="fast")
        command.add_argument("--cover", choices=("on", "off"), default="off")
        command.add_argument("--timeout", type=int, default=300)
        command.add_argument("--output", type=Path, required=True)
        command.set_defaults(function=function)
    comparison = commands.add_parser("compare")
    comparison.add_argument("--baseline", type=Path, required=True)
    comparison.add_argument("--final", type=Path, required=True)
    comparison.add_argument("--output", type=Path, required=True)
    comparison.set_defaults(function=compare)
    arguments = parser.parse_args()
    remote_only()
    arguments.function(arguments)


if __name__ == "__main__":
    main()
