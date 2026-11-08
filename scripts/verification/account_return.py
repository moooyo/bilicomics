#!/usr/bin/env python3
"""Verify Account toolbar return controls with real XTest input on test-env.

The timed intent only selects the System fixture. Every return or exit under
examination is delivered through the actual Rust/Slint toolbar hit target.
"""

import argparse
import datetime
import json
import os
from pathlib import Path
import re
import select
import signal
import subprocess
import sys
import tempfile
import time
import traceback

from harness_signals import install_interruption_handlers
from pointer_v2 import (CONTEXT_KEYS, INPUT_DELIVERY_PROTOCOL, PROFILES, Pointer,
                        expect, normalized, require, trace_records, write_json)
from run_suite import file_identity, sha256, stop_owned_group


EXIT_DEADLINE_MS = 30000
TAP_EXIT_LIMIT_SECONDS = 3.0
PLANS = (
    ("pw5-home", "pw5", "account", False, False),
    ("pw5-system-back-home", "pw5", "system", False, True),
    ("scribe-home", "scribe", "account", False, False),
    ("scribe-system", "scribe", "system", False, False),
    ("scribe-system-landscape", "scribe", "system", True, False),
)
SOURCE_INPUTS = (
    "Cargo.toml", "ui/account.slint", "ui/widgets.slint", "assets/i18n/account.json", "build.rs",
    "src/main.rs", "src/app/account.rs", "scripts/verification/account_return.py",
    "scripts/verification/pointer_v2.py", "scripts/verification/run_suite.py",
    "scripts/verification/harness_signals.py",
)


def close_owned_group(process):
    # Repeated terminal signals must not interrupt owned-session cleanup.
    handlers = {value: signal.getsignal(value) for value in
                (signal.SIGTERM, signal.SIGHUP, signal.SIGINT)}
    try:
        for value in handlers:
            signal.signal(value, signal.SIG_IGN)
        return stop_owned_group(process)
    finally:
        for value, handler in handlers.items():
            signal.signal(value, handler)


class Case:
    def __init__(self, arguments, runtime, plan):
        self.arguments = arguments
        self.name, self.profile, self.section, self.landscape, self.local_back = plan
        self.directory = arguments.output / self.name
        self.directory.mkdir(mode=0o700)
        self.runtime = runtime / self.name
        self.runtime.mkdir(mode=0o700)
        self.trace = self.runtime / "ui.jsonl"
        self.process = self.xvfb = self.pointer = None
        self.log = self.xvfb_log = None
        self.previous_display = os.environ.get("DISPLAY")
        self.result = {"name": self.name, "profile": self.profile,
                       "landscape": self.landscape, "passed": False,
                       "initial_section": self.section, "clicks": [],
                       "observations": [], "screenshots": []}

    def wait(self, description, predicate, timeout=25):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if self.process is not None:
                require(self.process.poll() is None,
                        f"The original application exited while waiting for {description}")
            try:
                value = predicate()
                if value:
                    return value
            except (FileNotFoundError, json.JSONDecodeError):
                pass
            time.sleep(0.04)
        raise AssertionError(f"Timed out waiting for {description}")

    def records(self):
        records = trace_records(self.trace)
        require(all(record.get("process_id") == self.process.pid for record in records),
                "A trace record belongs to a replacement application process")
        require(sum(record.get("event") == "ready" for record in records) <= 1,
                "The application initialized more than once")
        return records

    def observe(self, description, section, after=0):
        expected = {"route": "account", "account.section": section}

        def match():
            for record in reversed(self.records()[after:]):
                if (record.get("event") == "ui-state" and record.get("demo") is True
                        and record.get("view_model_fixture_only") is False
                        and expect(normalized(record), expected)):
                    return record
            return None

        record = self.wait(description, match)
        self.result["observations"].append({"description": description, "record": record})
        return record

    def identity(self):
        require(self.process.poll() is None, "The original application is no longer alive")
        executable = Path(f"/proc/{self.process.pid}/exe")
        identity = {"process_id": self.process.pid,
                    "executable": str(executable.resolve(strict=True)),
                    "executable_sha256": sha256(executable),
                    "start_ticks": Path(f"/proc/{self.process.pid}/stat").read_text()
                    .rpartition(") ")[2].split()[19]}
        require(identity["executable"] == str(self.arguments.binary)
                and identity["executable_sha256"] == self.arguments.expected_binary_sha256,
                "The running process is not the selected binary")
        require(os.getpgid(self.process.pid) == self.process.pid
                and os.getsid(self.process.pid) == self.process.pid,
                "The application is not in this case's owned new session")
        return identity

    def capture(self, name):
        time.sleep(0.18)
        self.result["screenshots"].append(
            self.pointer.capture(self.window, self.directory / (name + ".png")))

    def start(self):
        self.xvfb_log = (self.directory / "xvfb.log").open("w", encoding="utf-8")
        self.xvfb = subprocess.Popen(
            ["Xvfb", "-displayfd", "1", "-screen", "0", "2200x3000x24", "-nolisten", "tcp"],
            stdout=subprocess.PIPE, stderr=self.xvfb_log, start_new_session=True)
        require(select.select([self.xvfb.stdout], [], [], 10)[0],
                "The owned Xvfb display did not become ready")
        display_number = os.read(self.xvfb.stdout.fileno(), 64).decode("ascii").strip()
        require(re.fullmatch(r"\d+", display_number) and self.xvfb.poll() is None,
                "The owned Xvfb server did not report a live display")
        os.environ["DISPLAY"] = ":" + display_number
        self.pointer = Pointer()
        self.result["display"] = {"name": os.environ["DISPLAY"], "owned_process_id": self.xvfb.pid}
        environment = os.environ.copy()
        for key in CONTEXT_KEYS:
            environment.pop(key, None)
        environment.pop("WAYLAND_DISPLAY", None)
        environment["SLINT_SCALE_FACTOR"] = "1"
        environment["SLINT_BACKEND"] = "winit-software"
        command = [str(self.arguments.binary), "--demo", "--offline", "--device", self.profile,
                   "--screen", "account", "--data-dir", str(self.runtime / "data"),
                   "--lifecycle-trace", str(self.trace), "--exit-after-ms", str(EXIT_DEADLINE_MS)]
        if self.landscape:
            command.append("--landscape")
        initial_actions = []
        if self.section == "system":
            initial_actions = [{"at_ms": 0, "section": "account", "action": "section", "value": "system"}]
            actions = self.runtime / "initial-actions.json"
            write_json(actions, initial_actions)
            command.extend(["--actions", str(actions)])
        self.result.update({"command": command, "initial_actions": initial_actions,
                            "intent_scope": "System selection only; no scripted back or exit"})
        self.log = (self.directory / "application.log").open("w", encoding="utf-8")
        self.process = subprocess.Popen(command, env=environment, cwd=self.arguments.workspace / "app",
                                        stdout=self.log, stderr=subprocess.STDOUT, start_new_session=True)
        ready = self.wait("the real App ready record", lambda: next(
            (record for record in self.records() if record.get("event") == "ready"), None))
        self.ready_observed_at = time.monotonic()
        require(ready.get("route") == "account", "The real application did not start on Account")
        self.observe("the ready live Account section", self.section)
        self.window = self.wait("the mapped window after App readiness", self.pointer.window)
        size = PROFILES[self.profile]["size"]
        if self.landscape:
            size = tuple(reversed(size))
        require((self.window["width"], self.window["height"]) == size,
                "The real window does not match the selected logical design profile")
        self.initial_identity = self.identity()
        self.result.update({"ready_record": ready, "window": self.window,
                            "running_binary": self.initial_identity,
                            "readiness_gate": "Original PID ready and matching live ui-state, then mapped X11 window"})

    def tap(self, name, x):
        geometry = PROFILES[self.profile]
        point = (x, round(geometry["status"] + geometry["header"] / 2))
        require(0 <= point[0] < self.window["width"] and 0 <= point[1] < self.window["height"],
                "The toolbar coordinate is outside the actual application window")
        require(self.identity() == self.initial_identity, "The application changed before the toolbar tap")
        require(time.monotonic() - self.ready_observed_at < 10,
                "The toolbar tap would be too close to the 30-second fallback exit")
        prior = len(self.records())
        started = time.monotonic()
        self.pointer.click(self.window, point)
        self.result["clicks"].append({"name": name, "client": list(point),
                                      "window_id": self.window["id"], "monotonic_seconds": started})
        return prior, started

    def run(self):
        self.start()
        if self.local_back:
            self.capture("before-local-back")
            prior, _ = self.tap("System toolbar back to Account home", 60)
            self.observe("the pointer-driven Account home after local back", "account", prior)
            require(self.identity() == self.initial_identity,
                    "The Paperwhite local back exited or restarted the application")
            self.result["local_back_preserved_process"] = True
            self.capture("home-after-local-back")
        else:
            self.capture("before-return")
        close_x = self.window["width"] - (32 if self.profile == "scribe" else 8) - 24
        _, started = self.tap("Account toolbar close to Kindle", close_x)
        try:
            exit_code = self.process.wait(timeout=max(0.01, TAP_EXIT_LIMIT_SECONDS - (time.monotonic() - started)))
        except subprocess.TimeoutExpired as error:
            raise AssertionError("The toolbar tap did not exit within three seconds") from error
        elapsed = time.monotonic() - started
        self.result["exit"] = {"code": exit_code, "after_tap_ms": round(elapsed * 1000, 3),
                               "maximum_after_tap_ms": TAP_EXIT_LIMIT_SECONDS * 1000,
                               "fallback_exit_after_ready_ms": EXIT_DEADLINE_MS}
        require(exit_code == 0 and elapsed <= TAP_EXIT_LIMIT_SECONDS,
                "The toolbar return did not complete a prompt successful shutdown")
        try:
            os.killpg(self.process.pid, 0)
        except ProcessLookupError:
            self.result["owned_application_group_gone_before_cleanup"] = True
        else:
            raise AssertionError("The toolbar return left a descendant in the owned application session")
        # Keep the isolated display alive briefly to detect a replacement window.
        deadline = time.monotonic() + 0.4
        while time.monotonic() < deadline:
            require(self.pointer.window() is None, "An application window remained or restarted after return")
            time.sleep(0.04)
        records = self.records()
        require(sum(record.get("event") == "ready" for record in records) == 1,
                "The process readiness history does not identify one application run")
        self.result.update({"passed": True, "original_process_exited": True,
                            "replacement_window_observed": False, "ready_count": 1,
                            "process_ids_in_trace": sorted({record["process_id"] for record in records})})

    def close(self):
        cleanup = {}
        try:
            if self.process is not None:
                cleanup["application"] = close_owned_group(self.process)
        finally:
            try:
                if self.pointer is not None:
                    self.pointer.close()
            finally:
                if self.xvfb is not None:
                    cleanup["xvfb"] = close_owned_group(self.xvfb)
                    self.xvfb.stdout.close()
                for stream in (self.log, self.xvfb_log):
                    if stream is not None:
                        stream.close()
                if self.previous_display is None:
                    os.environ.pop("DISPLAY", None)
                else:
                    os.environ["DISPLAY"] = self.previous_display
                self.result["cleanup"] = cleanup
                if not all(item["owned_process_group_gone"] for item in cleanup.values()):
                    self.result.update({"passed": False, "cleanup_error": "An owned session survived cleanup"})


def main():
    require(sys.platform.startswith("linux") and os.environ.get("BILICOMICS_REMOTE_VERIFICATION") == "1",
            "Run only through ssh test-env with BILICOMICS_REMOTE_VERIFICATION=1")
    os.umask(0o077)
    install_interruption_handlers()
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--workspace", type=Path, required=True)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--expected-binary-sha256", required=True)
    parser.add_argument("--output", type=Path, required=True)
    arguments = parser.parse_args()
    require(all(path.is_absolute() for path in (arguments.workspace, arguments.binary, arguments.output)),
            "Workspace, binary, and output paths must all be absolute")
    require(re.fullmatch(r"[0-9a-fA-F]{64}", arguments.expected_binary_sha256), "A complete binary SHA-256 is required")
    arguments.expected_binary_sha256 = arguments.expected_binary_sha256.lower()
    arguments.workspace = arguments.workspace.resolve(strict=True)
    arguments.binary = arguments.binary.resolve(strict=True)
    arguments.output = arguments.output.resolve()
    app = arguments.workspace / "app"
    require(Path(__file__).resolve() == (app / "scripts/verification/account_return.py").resolve(strict=True),
            "Run the staged source harness from the canonical isolated workspace/app")
    require(arguments.binary.is_file() and os.access(arguments.binary, os.X_OK)
            and arguments.binary.is_relative_to(arguments.workspace), "The selected workspace binary is unavailable")
    require(sha256(arguments.binary) == arguments.expected_binary_sha256, "The selected binary SHA-256 does not match")
    require(arguments.output.is_relative_to(arguments.workspace) and arguments.output != arguments.workspace
            and not arguments.output.is_relative_to(app), "Output must be a new evidence directory outside application sources")
    sources = {name: file_identity(app / name) for name in SOURCE_INPUTS}
    arguments.output.mkdir(mode=0o700, parents=True, exist_ok=False)
    report = {"suite": "account_toolbar_return", "passed": False,
              "started_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
              "source_inputs": sources, "selected_binary": file_identity(arguments.binary),
              "input_delivery_protocol": INPUT_DELIVERY_PROTOCOL,
              "live_account_used": False, "remote_transactions_performed": False,
              "physical_device_connected": False, "firmware_emulator_run": False,
              "visual_acceptance": "pending", "cases": [],
              "scope": "Real offline demo desktop controls; no Kindle hardware support claim",
              "binary_source_correspondence": "The parent build manifest links the binary to source; these hashes identify staged inputs only",
              "evidence_policy": "Only safe result JSON, demo logs, and actual window PNGs; runtime data is not exported"}
    result_path = arguments.output / "account-return-result.json"
    write_json(result_path, report)
    try:
        with tempfile.TemporaryDirectory(prefix="account-return-runtime-", dir=arguments.workspace) as temporary:
            for plan in PLANS:
                case = Case(arguments, Path(temporary), plan)
                try:
                    case.run()
                except Exception as error:
                    case.result.update({"passed": False, "error": str(error), "traceback": traceback.format_exc()})
                finally:
                    case.close()
                    report["cases"].append(case.result)
                    write_json(result_path, report)
                print(json.dumps({"case": case.name, "passed": case.result["passed"],
                                  "error": case.result.get("error")}), flush=True)
        report["source_inputs_unchanged"] = sources == {name: file_identity(app / name) for name in SOURCE_INPUTS}
        report["selected_binary_unchanged"] = sha256(arguments.binary) == arguments.expected_binary_sha256
        report["passed"] = (len(report["cases"]) == len(PLANS) and all(case["passed"] for case in report["cases"])
                            and report["source_inputs_unchanged"] and report["selected_binary_unchanged"])
    except BaseException as error:
        report.update({"passed": False, "error": str(error), "traceback": traceback.format_exc()})
    finally:
        report["finished_at"] = datetime.datetime.now(datetime.timezone.utc).isoformat()
        write_json(result_path, report)
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    sys.exit(main())
