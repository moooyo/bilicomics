#!/usr/bin/env python3
"""Verify failed checkpoints recover through the real suspended App control path.

Run only on the designated Linux worker with a sealed desktop binary. The
private/ directory contains disposable demo state and lifecycle capabilities;
export only evidence/. No launcher, native service, live account, or Kindle is
used. Suspend and resume use the production request-file and Unix signal path.
"""

import argparse
from contextlib import contextmanager
import hashlib
import json
import math
import os
from pathlib import Path
import platform
import re
import secrets
import shutil
import signal
import sqlite3
import struct
import sys
import time
import traceback

sys.dont_write_bytecode = True
from resource_fixture import file_identity, require, sha256
from resource_regression import (Guard, OwnedProcess, cleanup_signals,
                                 export_manifest, owned_signal, proc_record,
                                 remaining, same_birth, source_manifest,
                                 start_xvfb, utc_now, wait_for_real_executable,
                                 write_json)


MIB = 1024 * 1024
ACCOUNT = "design-demo"
FAIL_TRIGGER = "sleep_probe_reject_anchor"
AUDIT_TABLE = "sleep_probe_anchor_writes"
AUDIT_INSERT = "sleep_probe_count_anchor_insert"
AUDIT_UPDATE = "sleep_probe_count_anchor_update"
TRACE_EVENTS = {"reader-ready", "suspend", "resume", "quit"}
ANCHOR_FIELDS = ("episode_id", "revision", "page_id", "page_index", "source_x",
                 "source_y", "zoom_ratio", "rotation", "mode", "finished")
STATE_FIELDS = ("comic_id", "episode_id", "reader_generation", "revision",
                "page_id", "page_index", "page_number", "source_x", "source_y",
                "zoom_ratio", "rotation", "mode", "core_mode", "fit",
                "direction", "crop_index", "crop_count", "frame_width",
                "frame_height", "second_frame_width", "second_frame_height",
                "viewport_width", "viewport_height", "transition", "rendering",
                "demo_strip", "menu", "sheet")


class ObservationGap(AssertionError):
    """The selected binary did not publish a required production fact."""


@contextmanager
def database_connection(path):
    require(path.is_file() and not path.is_symlink(), "The private demo database is unavailable")
    # A FAIL trigger provides an immediate, controlled failure. Never simulate
    # storage failure by occupying a writer lock for SQLite's busy timeout.
    connection = sqlite3.connect(path, timeout=0.1)
    try:
        connection.execute("PRAGMA busy_timeout=100")
        yield connection
        connection.commit()
    except BaseException:
        connection.rollback()
        raise
    finally:
        connection.close()


def sql_literal(value):
    return "'" + str(value).replace("'", "''") + "'"


def atomic_marker(directory, name, value):
    require(directory.is_dir() and not directory.is_symlink(), "The private lifecycle directory changed")
    target = directory / name
    require(not target.is_symlink(), "Refusing to replace a lifecycle symlink")
    temporary = directory / ("." + name + ".probe-new")
    with temporary.open("x", encoding="utf-8") as stream:
        stream.write(value + "\n")
        stream.flush()
        os.fsync(stream.fileno())
    temporary.replace(target)


def marker(path):
    if not path.exists():
        return None
    require(path.is_file() and not path.is_symlink(), "An unsafe lifecycle marker appeared")
    require(path.stat().st_size <= 1024, "A lifecycle marker exceeded its production limit")
    return path.read_text().strip()


def normalized_anchor(value):
    require(isinstance(value, dict) and all(key in value for key in (*ANCHOR_FIELDS, "updated_at")),
            "The persisted full ReadingAnchor is not observable")
    # Checkpoint timestamps legitimately change. Preserve every other field,
    # including future fields, so a partial-position comparison cannot pass.
    return {key: item for key, item in value.items() if key != "updated_at"}


def anchor_from_reader(state):
    require(all(key in state for key in STATE_FIELDS), "The complete live reader snapshot is unavailable")
    require(state["transition"] is False and state["demo_strip"] is False
            and state["core_mode"] == "page" and state["mode"] == "page",
            "The fault must target a paged ReadingAnchor, not a strip preference")
    return {"episode_id": state["episode_id"], "revision": state["revision"],
            "page_id": state["page_id"], "page_index": state["page_index"],
            "source_x": state["source_x"], "source_y": state["source_y"],
            "zoom_ratio": state["zoom_ratio"], "rotation": state["rotation"],
            "mode": state["core_mode"], "finished": state["transition"]}


def stored_anchor(database, comic):
    with database_connection(database) as connection:
        row = connection.execute(
            "SELECT payload FROM reading_anchors WHERE account_key=? AND comic_id=?",
            (ACCOUNT, comic)).fetchone()
    require(row is not None, "The selected demo comic has no durable reading anchor")
    value = json.loads(row[0])
    normalized_anchor(value)
    return value


def audit_writes(database):
    with database_connection(database) as connection:
        rows = connection.execute(
            "SELECT sequence,operation,payload FROM sleep_probe_anchor_writes ORDER BY sequence").fetchall()
    return [{"sequence": row[0], "operation": row[1], "anchor": json.loads(row[2])}
            for row in rows]


def install_fault(database, comic, expected):
    with database_connection(database) as connection:
        connection.execute("BEGIN IMMEDIATE")
        row = connection.execute(
            "SELECT payload FROM image_indices WHERE account_key=? AND episode_id=? AND revision=?",
            (ACCOUNT, expected["episode_id"], expected["revision"])).fetchone()
        require(row is not None, "The demo page descriptor is unavailable")
        index = json.loads(row[0])
        old_index = expected["page_index"] - 1
        require(old_index >= 0 and len(index["images"]) > expected["page_index"],
                "A distinct earlier page is required for the durable stale-anchor fault")
        old_page = index["images"][old_index]
        require(old_page["index"] == old_index and old_page["id"] != expected["page_id"],
                "The older demo page must have a separate descriptor identity")
        stale = {**expected, "page_index": old_index, "page_id": old_page["id"],
                 "updated_at": max(0, int(time.time()) - 3600)}
        changed = connection.execute(
            "UPDATE reading_anchors SET payload=?,updated_at=? WHERE account_key=? AND comic_id=?",
            (json.dumps(stale), stale["updated_at"], ACCOUNT, comic)).rowcount
        require(changed == 1, "The controlled stale-anchor update did not target exactly one fixture row")
        connection.execute(
            "CREATE TABLE sleep_probe_anchor_writes ("
            "sequence INTEGER PRIMARY KEY,operation TEXT NOT NULL,payload TEXT NOT NULL)")
        condition = ("NEW.account_key=" + sql_literal(ACCOUNT)
                     + " AND NEW.comic_id=" + sql_literal(comic))
        for name, operation in ((AUDIT_INSERT, "INSERT"), (AUDIT_UPDATE, "UPDATE")):
            connection.execute(
                f"CREATE TRIGGER {name} AFTER {operation} ON reading_anchors WHEN {condition} BEGIN "
                f"INSERT INTO {AUDIT_TABLE}(operation,payload) VALUES ({sql_literal(operation)},NEW.payload); END")
        connection.execute(
            f"CREATE TRIGGER {FAIL_TRIGGER} BEFORE INSERT ON reading_anchors WHEN {condition} "
            "BEGIN SELECT RAISE(FAIL,'controlled_sleep_checkpoint_failure'); END")
    require(normalized_anchor(stored_anchor(database, comic)) == normalized_anchor(stale),
            "The stale anchor was not durably installed")
    require(audit_writes(database) == [], "A successful anchor write bypassed the controlled failure")
    return stale


def remove_fault(database):
    with database_connection(database) as connection:
        row = connection.execute(
            "SELECT type FROM sqlite_master WHERE name=?", (FAIL_TRIGGER,)).fetchone()
        require(row == ("trigger",), "The controlled failure trigger disappeared before removal")
        connection.execute(f"DROP TRIGGER {FAIL_TRIGGER}")


class Trace:
    def __init__(self, path, evidence, process):
        self.path, self.evidence, self.process = path, evidence, process
        self.offset, self.buffer, self.records = 0, b"", []

    def read(self, final=False):
        if not self.path.exists():
            return self.records
        require(self.path.is_file() and not self.path.is_symlink(), "The production trace path became unsafe")
        size = self.path.stat().st_size
        require(self.offset <= size <= 8 * MIB, "The production trace changed identity or exceeded its bounded size")
        with self.path.open("rb") as stream:
            stream.seek(self.offset)
            chunk = stream.read()
        self.offset += len(chunk)
        self.buffer += chunk
        while b"\n" in self.buffer:
            line, self.buffer = self.buffer.split(b"\n", 1)
            if not line.strip():
                continue
            record = json.loads(line)
            require(isinstance(record, dict), "A production trace record is not an object")
            if record.get("event") not in TRACE_EVENTS:
                continue
            require(record.get("process_id") == self.process.process.pid,
                    "A different process wrote a lifecycle or Reader observation")
            self.records.append(record)
            with self.evidence.open("a", encoding="utf-8") as stream:
                stream.write(json.dumps({"observed_monotonic_ns": time.monotonic_ns(),
                                         "record": record}) + "\n")
        if final:
            require(not self.buffer, "The final production trace contains an incomplete record")
        return self.records

    def loaded(self, comic, episode, page):
        for record in reversed(self.read()):
            if (record.get("event") != "reader-ready" or record.get("route") != "reader"
                    or record.get("reader_loading") is not False):
                continue
            state = record.get("reader_state")
            frame = record.get("frame_size")
            if not (isinstance(state, dict) and isinstance(frame, list) and len(frame) == 2):
                raise ObservationGap("A ready Reader event omitted its live state or frame dimensions")
            if (state.get("comic_id"), state.get("episode_id"), state.get("page_number")) != (comic, episode, page):
                continue
            if not all(key in state for key in STATE_FIELDS):
                raise ObservationGap("A ready Reader event omitted complete anchor or frame facts")
            if (state["rendering"] is False and state["transition"] is False
                    and all(isinstance(value, int) and value > 0 for value in frame)
                    and frame == [state["frame_width"], state["frame_height"]]):
                return record
        return None

    def event(self, event, after):
        return next((record for record in self.read()[after:] if record["event"] == event), None)


def wait_for(description, predicate, process, guard, deadline, timeout):
    stop = min(deadline, time.monotonic() + timeout)
    last_error = None
    while time.monotonic() < stop:
        guard.check()
        remaining(deadline)
        require(process.process.poll() is None, "The real App exited while waiting for " + description)
        require(same_birth(process.identity), "The original App PID, birth, or private session changed")
        try:
            value = predicate()
            if value:
                return value
        except sqlite3.OperationalError as error:
            if "locked" not in str(error).lower() and "busy" not in str(error).lower():
                raise
            last_error = str(error)
        time.sleep(min(0.04, max(0, stop - time.monotonic())))
    raise ObservationGap("Timed out waiting for " + description + "; last transient error: " + str(last_error))


def owned_session_members(processes):
    sessions = {owned.identity["session"] for owned in processes if owned is not None}
    members = []
    for path in Path("/proc").iterdir():
        if not path.name.isdigit():
            continue
        try:
            current = proc_record(int(path.name))
        except (FileNotFoundError, ProcessLookupError, PermissionError):
            continue
        if current["session"] in sessions:
            members.append({key: current[key] for key in
                            ("pid", "birth_start_ticks", "process_group", "session", "state")})
    return members


def check_sleeping(trace, control, header, process):
    require(process.process.poll() is None and same_birth(process.identity),
            "The original App did not survive the failed suspend checkpoint")
    power = [record["event"] for record in trace.read() if record["event"] in ("suspend", "resume", "quit")]
    require(power == ["suspend"], "An unexpected resume or quit occurred during suspended maintenance")
    require(marker(control / "lifecycle-request") == header + ":1:suspend"
            and marker(control / "lifecycle-ack") == header + ":1:suspend:checkpoint-error",
            "The production signal context changed during suspended maintenance")


def transition(action, sequence, status, control, header, trace, process, guard, deadline):
    prior = len(trace.read())
    request = f"{header}:{sequence}:{action}"
    started = time.monotonic_ns()
    atomic_marker(control, "lifecycle-request", request)
    delivered = owned_signal(process.identity, signal.SIGUSR1 if action == "suspend" else signal.SIGUSR2)
    require(delivered["sent"], "The real lifecycle signal could not reach its owned App process")
    wait_for("the real " + action + " acknowledgement",
             lambda: marker(control / "lifecycle-ack") == request + ":" + status,
             process, guard, deadline, 8)
    acknowledged = time.monotonic_ns()
    observed = wait_for("the real " + action + " trace", lambda: trace.event(action, prior),
                        process, guard, deadline, 2)
    require(observed.get("suspended") is (action == "suspend")
            and observed.get("checkpoint_saved") is (status == "ok")
            and observed.get("booting") is False and observed.get("route") == "reader",
            "The production lifecycle trace disagrees with its acknowledgement")
    return {"action": action, "sequence": sequence, "generation": 1,
            "request_channel": "Atomic private request file and production SignalBridge Unix signal",
            "signal": delivered["signal"], "acknowledgement_status": status,
            "acknowledgement_elapsed_ms": (acknowledged - started) / 1_000_000,
            "observed_monotonic_ns": acknowledged, "trace": observed}


def run_case(args, report, private, evidence, guard, deadline):
    process = xvfb = trace = None
    control, data = private / "lifecycle", private / "data"
    token = secrets.token_hex(16)
    header = "1:" + token
    fixture_path = args.app_root / "assets/demo/catalog.json"
    fixture = json.loads(fixture_path.read_text())
    require(fixture.get("demo_only") is True, "Only the isolated fictional design catalog may be used")
    resume = fixture["continue_reading"]
    comic, episode, page = str(resume["comic_id"]), str(resume["episode_id"]), int(resume["page"])
    require(page > 1 and str(fixture["comics"][0]["id"]) == comic,
            "The explicit reader screen must open the selected default fixture comic")
    report["fixture"] = {"account_kind": "isolated design-demo", "comic_id": comic,
                         "episode_id": episode, "page_number": page,
                         "catalog": file_identity(fixture_path),
                         "artwork_origin": "Bundled fictional design demo pages, materialized by the real App",
                         "session_seeded": False, "credentials_seeded": False,
                         "acquired_comics_used": False}
    database = data / "demo/state.sqlite3"
    raw_log, raw_trace = private / "application.log", private / "lifecycle.jsonl"
    cleanup = []
    report["cleanup"] = cleanup
    try:
        control.mkdir(mode=0o700)
        data.mkdir(mode=0o700)
        atomic_marker(control, "nonce", token)
        atomic_marker(control, "generation", header)
        xvfb, display = start_xvfb(evidence, guard, deadline)
        environment = os.environ.copy()
        for key in ("BILICOMICS_LIFECYCLE_DIR", "BILICOMICS_LAUNCH_TOKEN", "BILICOMICS_LAUNCH_GENERATION",
                    "WAYLAND_DISPLAY", "WAYLAND_SOCKET", "XAUTHORITY"):
            environment.pop(key, None)
        environment.update({"DISPLAY": display, "SLINT_BACKEND": "winit-software", "WINIT_UNIX_BACKEND": "x11",
                            "XDG_SESSION_TYPE": "x11", "BILICOMICS_LIFECYCLE_DIR": str(control),
                            "BILICOMICS_LAUNCH_TOKEN": token, "BILICOMICS_LAUNCH_GENERATION": "1"})
        command = ["unshare", "-n", "--", str(args.binary), "--demo", "--offline", "--device", args.profile,
                   "--screen", "reader", "--data-dir", str(data), "--lifecycle-trace", str(raw_trace),
                   "--exit-after-ms", str(int(args.deadline_seconds * 1000) + 10_000)]
        process = OwnedProcess("application", command, raw_log, environment=environment)
        guard.add(process)
        report.update({"command": command, "process_identity": process.identity,
                       "xvfb_identity": xvfb.identity, "display": display,
                       "actual_executable": wait_for_real_executable(process, args.binary, guard, deadline)})
        atomic_marker(control, "application-pid", str(process.process.pid))
        namespace, parent_namespace = os.readlink(f"/proc/{process.process.pid}/ns/net"), os.readlink("/proc/self/ns/net")
        require(namespace != parent_namespace, "The actual App did not enter its isolated network namespace")
        report["network_isolation"] = {"application_namespace": namespace, "harness_namespace": parent_namespace,
                                       "offline_requested": True, "network_namespace_separate": True}
        trace = Trace(raw_trace, evidence / "events.jsonl", process)
        wait_for("production SignalBridge readiness", lambda: marker(control / "lifecycle-ready") == header,
                 process, guard, deadline, 10)
        ready = wait_for("a fully rendered actual demo Reader", lambda: trace.loaded(comic, episode, page),
                         process, guard, deadline, args.ready_timeout_seconds)
        baseline = ready["reader_state"]
        expected = anchor_from_reader(baseline)
        original_anchor = stored_anchor(database, comic)
        require(normalized_anchor(original_anchor) == expected,
                "The initial full durable anchor disagrees with the live Reader")
        require([record["event"] for record in trace.read() if record["event"] in ("suspend", "resume", "quit")] == [],
                "A power transition occurred before the controlled fault was installed")
        report["reader_ready"] = ready
        report["initial_full_anchor"] = original_anchor
        stale = install_fault(database, comic, expected)
        report["injected_stale_anchor"] = stale
        report["fault"] = {"kind": "BEFORE INSERT reading_anchors RAISE(FAIL) trigger",
                           "targets_only_selected_demo_comic": True, "writer_lock_held_for_failure": False,
                           "harness_sqlite_busy_timeout_ms": 100, "successful_writes_before_suspend": 0}
        suspended = transition("suspend", 1, "checkpoint-error", control, header, trace, process, guard, deadline)
        require(suspended["trace"].get("reader_state") == baseline,
                "Failed checkpoint changed the complete retained Reader anchor or frame")
        require(normalized_anchor(stored_anchor(database, comic)) == normalized_anchor(stale)
                and audit_writes(database) == [], "The failed checkpoint unexpectedly changed durable Reader state")
        check_sleeping(trace, control, header, process)
        report["suspend"] = suspended
        remove_fault(database)
        removed_at = time.monotonic_ns()
        report["fault"]["removed_while_suspended"] = True

        def recovered():
            check_sleeping(trace, control, header, process)
            anchor = stored_anchor(database, comic)
            writes = audit_writes(database)
            if normalized_anchor(anchor) == expected:
                require(len(writes) == 1 and normalized_anchor(writes[0]["anchor"]) == expected,
                        "Recovery did not consist of exactly one successful complete anchor checkpoint")
                return {"anchor": anchor, "audit": writes, "observed_monotonic_ns": time.monotonic_ns()}
            require(normalized_anchor(anchor) == normalized_anchor(stale),
                    "Suspended maintenance wrote an unexpected partial or foreign anchor")
            return None

        recovered_state = wait_for("the live RAM anchor to be restored before any resume signal", recovered,
                                   process, guard, deadline, args.retry_timeout_seconds)
        recovered_state["elapsed_after_fault_removal_ms"] = (recovered_state["observed_monotonic_ns"] - removed_at) / 1_000_000
        recovered_state["resume_signal_sent"] = False
        recovered_state["ordinary_app_timer_suspended"] = True
        report["suspended_recovery"] = recovered_state
        samples, stable_until = [], time.monotonic() + args.stable_seconds
        while time.monotonic() < stable_until:
            guard.check()
            remaining(deadline)
            check_sleeping(trace, control, header, process)
            anchor, writes = stored_anchor(database, comic), audit_writes(database)
            require(normalized_anchor(anchor) == expected and len(writes) == 1,
                    "A successful suspended checkpoint was repeated or changed during the stable idle window")
            samples.append({"monotonic_ns": time.monotonic_ns(), "successful_anchor_writes": len(writes),
                            "anchor_sha256": hashlib.sha256(json.dumps(anchor, sort_keys=True).encode()).hexdigest()})
            time.sleep(min(0.1, max(0, stable_until - time.monotonic())))
        report["stable_suspended_window"] = {"seconds": args.stable_seconds, "samples": samples,
                                             "successful_anchor_writes_before": 1,
                                             "successful_anchor_writes_after": 1,
                                             "scope": "Observed anchor writes only; no claim that all SQLite I/O or OS wakes are zero"}
        resumed = transition("resume", 2, "ok", control, header, trace, process, guard, deadline)
        require(resumed["trace"].get("reader_state") == baseline,
                "Wake replaced the Reader, full source anchor, prepared frame, or presentation state")
        awake_anchor, awake_writes = stored_anchor(database, comic), audit_writes(database)
        require(normalized_anchor(awake_anchor) == expected and len(awake_writes) == 2,
                "Wake did not perform one complete live-anchor checkpoint after suspended recovery")
        report["resume"] = resumed
        report["awake_full_anchor"] = awake_anchor
        report["awake_audit_writes"] = awake_writes
        before_quit = len(trace.read())
        require(owned_signal(process.identity, signal.SIGTERM)["sent"], "The real PowerQuit signal was not delivered")
        process.process.wait(timeout=remaining(deadline, 10))
        require(process.process.returncode == 0, "The real App did not exit cleanly after PowerQuit")
        trace.read(final=True)
        quit_record = trace.event("quit", before_quit)
        require(quit_record is not None and quit_record.get("suspended") is False,
                "The real PowerQuit transition was not observed")
        require(marker(control / "lifecycle-ready") is None,
                "The production SignalBridge left readiness advertised after exit")
        require([record["event"] for record in trace.records if record["event"] in ("suspend", "resume", "quit")]
                == ["suspend", "resume", "quit"], "Unexpected extra real power transitions occurred")
        report["quit"] = {"signal": "SIGTERM", "production_event": "PowerQuit", "trace": quit_record,
                          "exit_code": process.process.returncode, "readiness_retired": True}
        report["final_full_anchor"] = stored_anchor(database, comic)
        require(normalized_anchor(report["final_full_anchor"]) == expected,
                "Shutdown changed the recovered complete Reader position")
        report["same_process_and_reader_generation"] = True
        report["retained_full_frame_dimensions"] = [baseline["frame_width"], baseline["frame_height"]]
        report["case_passed"] = True
    finally:
        cleanup_errors = []
        with cleanup_signals():
            for owned in (process, xvfb):
                if owned is None:
                    continue
                try:
                    stopped = owned.stop()
                    cleanup.append(stopped)
                    if not stopped["original_process_still_present"] and owned in guard.targets:
                        guard.remove(owned)
                except Exception as error:
                    cleanup_errors.append(owned.role + " cleanup: " + str(error))
            try:
                if trace is not None:
                    trace.read(final=process is not None and process.process.poll() is not None)
            except Exception as error:
                cleanup_errors.append("final trace: " + str(error))
            try:
                if raw_log.exists():
                    # Read a capability-length overlap before truncation, so a
                    # token crossing the tail boundary is redacted as a whole.
                    with raw_log.open("rb") as stream:
                        size = raw_log.stat().st_size
                        stream.seek(max(0, size - 64 * 1024 - len(token)))
                        log_tail = stream.read().decode("utf-8", errors="replace").replace(token, "[redacted-fixture-token]")
                    (evidence / "application.log").write_text(log_tail[-64 * 1024:], encoding="utf-8")
                    report["application_log_tail_truncated"] = size > 64 * 1024
            except Exception as error:
                cleanup_errors.append("bounded diagnostic export: " + str(error))
            try:
                require(control.resolve().is_relative_to(private.resolve()) and not control.is_symlink(),
                        "Refusing unsafe lifecycle capability cleanup")
                if control.exists():
                    shutil.rmtree(control)
                report["lifecycle_capabilities_removed"] = not control.exists()
            except Exception as error:
                cleanup_errors.append("lifecycle capability cleanup: " + str(error))
            try:
                report["remaining_owned_session_members"] = owned_session_members((process, xvfb))
            except Exception as error:
                cleanup_errors.append("owned session inspection: " + str(error))
        report["owned_processes_cleaned"] = (not cleanup_errors
            and all(not value["original_process_still_present"] for value in cleanup)
            and report.get("remaining_owned_session_members") == [])
        if cleanup_errors:
            report["case_passed"] = False
            report["cleanup_errors"] = cleanup_errors


def arguments():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--workspace", type=Path, required=True)
    parser.add_argument("--app-root", type=Path, required=True)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--expected-binary-sha256", required=True)
    parser.add_argument("--binary-source-manifest", type=Path, required=True)
    parser.add_argument("--expected-source-manifest-sha256", required=True)
    parser.add_argument("--output", type=Path, required=True, help="A new isolated directory; export only evidence/")
    parser.add_argument("--profile", choices=("pw5", "pw6", "scribe", "scribe2025"), default="pw5")
    parser.add_argument("--ready-timeout-seconds", type=float, default=60)
    parser.add_argument("--retry-timeout-seconds", type=float, default=12)
    parser.add_argument("--stable-seconds", type=float, default=3.5)
    parser.add_argument("--deadline-seconds", type=float, default=120)
    parser.add_argument("--rss-limit-mib", type=int, default=768)
    return parser.parse_args()


def main():
    args = arguments()
    require(sys.platform == "linux", "Run only on the designated remote Linux worker")
    require(hasattr(os, "pidfd_open") and hasattr(signal, "pidfd_send_signal"),
            "The Linux worker must support birth-checked pidfd signaling")
    require(all(math.isfinite(value) for value in
                (args.ready_timeout_seconds, args.retry_timeout_seconds, args.stable_seconds, args.deadline_seconds)),
            "All observation windows and deadlines must be finite")
    require(args.ready_timeout_seconds > 0 and args.retry_timeout_seconds >= 3 and args.stable_seconds >= 3,
            "Use bounded readiness, retry, and at least three-second stable observation windows")
    require(args.deadline_seconds >= args.ready_timeout_seconds + args.retry_timeout_seconds + args.stable_seconds + 20,
            "The absolute deadline must leave room for every phase and cleanup")
    require(1 <= args.rss_limit_mib <= 768, "The observed combined workload RSS limit cannot exceed 768 MiB")
    for digest in (args.expected_binary_sha256, args.expected_source_manifest_sha256):
        require(re.fullmatch(r"[0-9a-fA-F]{64}", digest), "Selected inputs require explicit SHA-256 digests")
    args.workspace = args.workspace.resolve(strict=True)
    args.app_root = args.app_root.resolve(strict=True)
    args.binary = args.binary.resolve(strict=True)
    args.binary_source_manifest = args.binary_source_manifest.resolve(strict=True)
    args.output = args.output.resolve()
    require(args.output.is_relative_to(args.workspace) and args.output != args.workspace
            and not args.output.is_relative_to(args.app_root), "Output must be new workspace evidence outside application sources")
    require(args.binary.is_file() and not args.binary.is_symlink(), "A regular sealed desktop binary is required")
    require(sha256(args.binary) == args.expected_binary_sha256.lower(), "The selected binary digest differs")
    require(sha256(args.binary_source_manifest) == args.expected_source_manifest_sha256.lower(),
            "The selected build source manifest digest differs")
    expected_source = args.binary_source_manifest.read_text()
    require(source_manifest(args.app_root) == expected_source, "The current sources differ from the selected binary inputs")
    with args.binary.open("rb") as stream:
        header = stream.read(64)
    require(header[:7] == b"\x7fELF\x02\x01\x01" and len(header) >= 20
            and struct.unpack_from("<H", header, 18)[0] == 62,
            "Only the selected Linux x86-64 desktop ELF may execute; ARM binaries are excluded")
    os.umask(0o077)
    args.output.mkdir(parents=True, exist_ok=False)
    private, evidence = args.output / "private", args.output / "evidence"
    private.mkdir(mode=0o700)
    evidence.mkdir(mode=0o700)
    sources = evidence / "source"
    sources.mkdir(mode=0o700)
    inputs = []
    for name in ("sleep_maintenance.py", "resource_regression.py", "resource_fixture.py"):
        path = Path(__file__).with_name(name).resolve(strict=True)
        require(path.is_file() and not path.is_symlink(), "A required verifier helper is unsafe or missing")
        shutil.copyfile(path, sources / name)
        inputs.append(file_identity(path))
    shutil.copyfile(args.binary_source_manifest, evidence / "binary-build-source.sha256")
    report = {"schema_version": 1, "passed": False, "status": "running", "started_at": utc_now(),
              "case": "real_signal_suspended_checkpoint_failure_recovery", "binary": file_identity(args.binary),
              "source_manifest_sha256": args.expected_source_manifest_sha256.lower(), "runner_inputs": inputs,
              "host": {"hostname": platform.node(), "system": platform.system(), "machine": platform.machine()},
              "policy": {"absolute_deadline_seconds": args.deadline_seconds,
                         "rss_safety_ceiling_bytes": args.rss_limit_mib * MIB,
                         "rss_limit_scope": "Observed combined App and private Xvfb RSS, not a kernel cgroup limit",
                         "cleanup_budget_seconds_per_owned_process": 5},
              "physical_device_connected": False, "arm_binary_executed": False, "live_account_used": False,
              "remote_transactions_performed": False, "real_system_services_used": False,
              "captures_created": False, "safe_export_root": str(evidence),
              "observability_gap_is_failure": True,
              "limits": ["Desktop SignalBridge/App/control-queue coverage does not establish native framebuffer, evdev, or firmware acceptance.",
                         "Logical application suspend is tested; the Linux host and physical Kindle do not enter hardware suspend.",
                         "The stable window proves no repeated anchor writes in that window, not zero OS wakeups or SQLite activity.",
                         "Timestamp changes are allowed; every other durable anchor field and the complete live Reader snapshot must match.",
                         "Export only evidence/; private/ contains the disposable database, original trace, and fictional demo page materialization."]}
    write_json(evidence / "results.json", report)
    guard, interrupted = None, False
    deadline = time.monotonic() + args.deadline_seconds
    try:
        guard = Guard(private, evidence, deadline, args.rss_limit_mib * MIB)
        run_case(args, report, private, evidence, guard, deadline)
        require(report.get("case_passed") is True and report.get("owned_processes_cleaned") is True
                and report.get("lifecycle_capabilities_removed") is True, "The case or its cleanup did not complete")
        require(sha256(args.binary) == args.expected_binary_sha256.lower(), "The selected binary changed during verification")
        require(source_manifest(args.app_root) == expected_source, "Application source inputs changed during verification")
        require(all(sha256(Path(item["path"])) == item["sha256"] for item in inputs),
                "A verifier or helper changed during verification")
        remaining(deadline)
        guard.check()
        report.update({"passed": True, "status": "observed", "inputs_unchanged_during_run": True})
    except BaseException as error:
        report.update({"passed": False, "status": "failed", "error": str(error), "traceback": traceback.format_exc()})
        interrupted = isinstance(error, (KeyboardInterrupt, SystemExit))
    finally:
        if guard is not None:
            with cleanup_signals():
                report["watchdog_cleanup"] = guard.stop()
            stopped = report["watchdog_cleanup"]
            if (stopped["original_process_still_present"] or stopped.get("registered_workloads_remaining")
                    or stopped.get("safety_failure") or stopped.get("exit_code") != 0):
                report.update({"passed": False, "status": "failed",
                               "cleanup_error": "The independent guard reported a failure or surviving owned workload"})
        report["finished_at"] = utc_now()
        write_json(evidence / "results.json", report)
        export_manifest(evidence)
    print(json.dumps({"passed": report["passed"], "status": report["status"],
                      "safe_export_root": str(evidence)}), flush=True)
    return 130 if interrupted else 0 if report["passed"] else 1


if __name__ == "__main__":
    def interrupt(signum, frame):
        raise KeyboardInterrupt("Sleep maintenance verifier interrupted by signal " + str(signum))

    for event in (signal.SIGTERM, signal.SIGHUP, signal.SIGINT):
        signal.signal(event, interrupt)
    sys.exit(main())
