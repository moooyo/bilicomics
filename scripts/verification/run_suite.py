#!/usr/bin/env python3
"""Run reproducible pre-device checks in an isolated designated Linux workspace.

Desktop fixtures, native signing, firmware emulation, and physical acceptance are
separate evidence classes. This runner never connects to a Kindle or an account.
"""

import argparse
import datetime
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import shutil
import signal
import stat
import subprocess
import sys
import time
import traceback
import uuid


PHASES = ("core", "defaults", "pointer", "reader", "guest", "startup", "lifecycle", "launcher", "package")
SOURCE_TREES = ("assets", "src", "ui", "vendor", "tests")
ROOT_INPUTS = ("Cargo.toml", "Cargo.lock", "build.rs", ".cargo/config.toml")
PIPELINE_TREES = ("scripts", "packaging")
LIFECYCLE_KEYS = ("BILICOMICS_LIFECYCLE_DIR", "BILICOMICS_LAUNCH_TOKEN", "BILICOMICS_LAUNCH_GENERATION")


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def write_json(path, value):
    path.write_text(json.dumps(value, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")


def input_files(app, trees):
    files = [app / name for name in ROOT_INPUTS if (app / name).is_file()]
    for tree in trees:
        directory = app / tree
        if not directory.exists():
            continue
        for path in directory.rglob("*"):
            relative = path.relative_to(app)
            if any(part in ("target", ".git", "__pycache__") for part in relative.parts):
                continue
            if path.is_file() and not path.is_symlink() and path.suffix not in (".pyc", ".log", ".tmp"):
                files.append(path)
    return sorted(set(files), key=lambda path: path.relative_to(app).as_posix())


def manifest_text(app, trees):
    return "".join(f"{sha256(path)}  {path.relative_to(app).as_posix()}\n" for path in input_files(app, trees))


def file_identity(path):
    return {"path": str(path), "bytes": path.stat().st_size, "sha256": sha256(path)}


class StageTimeout(RuntimeError):
    """A timed-out stage must be cleaned up before this run stops."""


def interrupt_suite(signum, frame):
    raise KeyboardInterrupt(f"Remote verification interrupted by signal {signum}")


def stop_owned_group(process):
    """Terminate only the process group created for this exact stage."""
    report = {"process_id": process.pid, "process_group": process.pid, "term_sent": False, "kill_sent": False}

    def exists():
        try:
            os.killpg(process.pid, 0)
            return True
        except ProcessLookupError:
            return False

    def send(signum):
        try:
            os.killpg(process.pid, signum)
            return True
        except ProcessLookupError:
            return False

    if exists():
        report["term_sent"] = send(signal.SIGTERM)
        deadline = time.monotonic() + 35
        while exists() and time.monotonic() < deadline:
            process.poll()
            time.sleep(0.05)
    if exists():
        report["kill_sent"] = send(signal.SIGKILL)
    try:
        process.wait(timeout=5)
        report["leader_reaped"] = True
    except subprocess.TimeoutExpired:
        report["leader_reaped"] = False
    deadline = time.monotonic() + 2
    while exists() and time.monotonic() < deadline:
        time.sleep(0.05)
    report["owned_process_group_gone"] = not exists()
    report["leader_exit_code"] = process.returncode
    report["scope"] = "Only this stage's new session; detached application cleanup is handled by each harness"
    return report


class Suite:
    def __init__(self, arguments):
        self.args = arguments
        self.workspace = arguments.workspace.resolve(strict=True)
        self.app = (arguments.app_root or self.workspace / "app").resolve(strict=True)
        self.target = (arguments.target_dir or self.workspace / "target").absolute()
        require(self.app == (self.workspace / "app").resolve(strict=True),
                "Application sources must use the canonical workspace/app directory")
        identifier = datetime.datetime.now(datetime.timezone.utc).strftime("%Y%m%dT%H%M%SZ") + "-" + uuid.uuid4().hex[:8]
        self.output = (arguments.output or self.workspace / "evidence" / identifier).absolute()
        require(self.output.resolve().is_relative_to(self.workspace) and self.output.resolve() != self.workspace,
                "Evidence must be a new directory inside the isolated workspace")
        self.output.mkdir(mode=0o700, parents=True, exist_ok=False)
        self.logs = self.output / "logs"
        self.logs.mkdir()
        self.harness = self.app / "scripts/verification"
        self.environment = os.environ.copy()
        self.environment["CARGO_TARGET_DIR"] = str(self.target)
        self.environment["CARGO_BUILD_JOBS"] = self.environment.get("CARGO_BUILD_JOBS") or "12"
        self.environment["RUST_TEST_THREADS"] = self.environment.get("RUST_TEST_THREADS") or "1"
        for key in LIFECYCLE_KEYS:
            self.environment.pop(key, None)
        self.source_text = manifest_text(self.app, SOURCE_TREES)
        self.pipeline_text = manifest_text(self.app, SOURCE_TREES + PIPELINE_TREES)
        (self.output / "source-inputs.sha256").write_text(self.source_text, encoding="utf-8")
        (self.output / "pipeline-inputs.sha256").write_text(self.pipeline_text, encoding="utf-8")
        self.source_sha = sha256(self.output / "source-inputs.sha256")
        self.binary = (arguments.binary or self.target / "debug/bilicomics").resolve()
        if "core" in arguments.phases and not arguments.check_only:
            require(self.binary == (self.target / "debug/bilicomics").resolve(),
                    "The core phase must test the desktop executable produced by this run")
            require(arguments.binary_source_manifest is None,
                    "A new core build cannot use a different binary's historical source manifest")
        elif any(name in arguments.phases for name in ("defaults", "pointer", "reader", "guest", "startup", "lifecycle")):
            require(arguments.binary_source_manifest is not None,
                    "An existing desktop binary requires its separate --binary-source-manifest")
        self.binary_sha = arguments.expected_binary_sha256
        self.report = {
            "schema_version": 1,
            "started_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
            "workspace": str(self.workspace), "app_root": str(self.app), "output": str(self.output),
            "runner": file_identity(Path(__file__).resolve()),
            "host": {"hostname": platform.node(), "system": platform.system(), "machine": platform.machine()},
            "resources": {"cargo_build_jobs": self.environment["CARGO_BUILD_JOBS"],
                          "rust_test_threads": self.environment["RUST_TEST_THREADS"],
                          "stage_commands_serial": True, "default_capture_workers": 1},
            "source_inputs": file_identity(self.output / "source-inputs.sha256"),
            "pipeline_inputs": file_identity(self.output / "pipeline-inputs.sha256"),
            "binary_input_provenance": None,
            "selected_phases": arguments.phases,
            "phases": {name: {"status": "not_run", "reason": "Phase was not selected"} for name in PHASES},
            "automated_status": "running", "visual_acceptance": "not_claimed",
            "physical_device_connected": False, "firmware_emulator_run": False,
            "live_account_used": False, "remote_transactions_performed": False,
            "limits": [
                "Captured pixels and automatic state assertions do not establish human visual acceptance.",
                "Controlled OS fixtures do not establish native firmware or physical panel behavior.",
                "No physical Kindle connection, real credentials, or account content is used by this runner.",
            ],
        }
        if arguments.binary_source_manifest:
            provenance = arguments.binary_source_manifest.resolve(strict=True)
            shutil.copyfile(provenance, self.output / "binary-build-source.sha256")
            self.report["binary_input_provenance"] = {
                "kind": "declared_existing_binary_build_manifest",
                "original_manifest": str(provenance),
                **file_identity(self.output / "binary-build-source.sha256"),
                "pipeline_manifest_is_not_binary_build_manifest": True,
            }
        self.save()

    def save(self):
        write_json(self.output / "summary.json", self.report)

    def evidence_paths(self):
        files, directories, special = [], [], []
        for path in sorted(self.output.rglob("*")):
            mode = path.lstat().st_mode
            relative = path.relative_to(self.output).as_posix()
            if stat.S_ISREG(mode):
                files.append(path)
            elif stat.S_ISDIR(mode):
                directories.append(relative)
            else:
                special.append({"path": relative, "mode": oct(stat.S_IFMT(mode)),
                                "kind": "fifo" if stat.S_ISFIFO(mode) else "non_regular_file"})
        return files, directories, special

    def command(self, name, arguments, timeout=600):
        print(json.dumps({"command": name, "status": "running"}), flush=True)
        started = time.monotonic()
        log = self.logs / f"{name}.log"
        with log.open("w", encoding="utf-8") as output:
            process = subprocess.Popen([str(value) for value in arguments], cwd=self.app,
                                       env=self.environment, stdout=output, stderr=subprocess.STDOUT,
                                       start_new_session=True)
            try:
                exit_code = process.wait(timeout=timeout)
            except BaseException as error:
                cleanup = stop_owned_group(process)
                cleanup.update({"command": name, "error": str(error), "timeout_seconds": timeout,
                                "interrupted": isinstance(error, KeyboardInterrupt)})
                write_json(self.logs / f"{name}.termination.json", cleanup)
                if isinstance(error, subprocess.TimeoutExpired):
                    raise StageTimeout(f"{name} timed out; this suite stopped after owned-group cleanup; inspect {log}") from error
                raise
        record = {"name": name, "arguments": [str(value) for value in arguments],
                  "exit_code": exit_code, "seconds": round(time.monotonic() - started, 3),
                  "log": str(log), "log_sha256": sha256(log)}
        if exit_code != 0:
            cleanup = stop_owned_group(process)
            cleanup.update({"command": name, "exit_code": exit_code})
            write_json(self.logs / f"{name}.termination.json", cleanup)
        require(exit_code == 0, f"{name} exited with {exit_code}; inspect {log}")
        return record

    def identify_binary(self):
        require(self.binary.is_file() and os.access(self.binary, os.X_OK), "The selected desktop binary is unavailable")
        current = sha256(self.binary)
        if self.binary_sha:
            require(current == self.binary_sha.lower(), "The desktop binary digest differs from its declared identity")
        else:
            require("core" in self.args.phases and not self.args.check_only,
                    "Existing binaries require --expected-binary-sha256")
        self.binary_sha = current
        self.report["desktop_binary"] = file_identity(self.binary)

    def xvfb(self, script, *arguments):
        return ["xvfb-run", "-a", "-s", "-screen 0 2200x3000x24", sys.executable,
                self.harness / script, *arguments]

    def core(self):
        checks = [self.command("cargo-check", ["cargo", "check", "--locked", "--offline"], timeout=1800)]
        if self.args.check_only:
            return {"checks": checks, "scope": "cargo check only; no tests, build, or runtime phase selected"}
        checks.append(self.command("cargo-lib-tests", ["cargo", "test", "--locked", "--offline", "--lib"], timeout=1800))
        test_log = (self.logs / "cargo-lib-tests.log").read_text()
        test_totals = re.findall(r"test result: ok\.\s+(\d+) passed; (\d+) failed; (\d+) ignored;", test_log)
        require(test_totals and sum(int(row[0]) for row in test_totals) > 0,
                "The application library test log has no passing executed tests")
        test_summary = {name: sum(int(row[index]) for row in test_totals)
                        for index, name in enumerate(("passed", "failed", "ignored"))}
        checks.append(self.command("cargo-desktop-build", ["cargo", "build", "--locked", "--offline", "--bin", "bilicomics"], timeout=1800))
        self.identify_binary()
        sealed = self.workspace / "sealed" / self.binary_sha / "bilicomics"
        sealed.parent.mkdir(parents=True, mode=0o700, exist_ok=False)
        shutil.copyfile(self.binary, sealed)
        sealed.chmod(0o500)
        self.binary = sealed
        self.report["desktop_binary"] = file_identity(self.binary)
        self.report["binary_input_provenance"] = {
            "kind": "built_in_this_run", "build_source_manifest": file_identity(self.output / "source-inputs.sha256")}
        return {"checks": checks, "test_summary": test_summary, "scope": "Application library tests and desktop build"}

    def defaults(self):
        command = [sys.executable, self.harness / "render_states.py", "--plan", self.harness / "design-v2-cases.json",
                   "--batch", "design-v2", "--binary", self.binary, "--expected-sha256", self.binary_sha,
                   "--output-root", self.output / "defaults"]
        check = self.command("defaults", command)
        result = json.loads((self.output / "defaults/design-v2-results.json").read_text())
        require(result["capture_failures"] == 0 and result["rendered"] == 23, "The complete 23-view v2 capture matrix did not pass")
        return {"checks": [check], "rendered": result["rendered"], "visual_review": "pending"}

    def pointer(self):
        check = self.command("pointer", self.xvfb("pointer_v2.py", "--workspace", self.workspace,
            "--binary", self.binary, "--expected-binary-sha256", self.binary_sha, "--output", self.output / "pointer"))
        result = json.loads((self.output / "pointer/pointer-v2-result.json").read_text())
        require(result["passed"] is True and len(result["cases"]) == 8, "The complete eight-case pointer matrix did not pass")
        return {"checks": [check], "cases": len(result["cases"]), "visual_review": "pending"}

    def reader(self):
        manifest = self.output / "source-inputs.sha256"
        if self.args.binary_source_manifest:
            manifest = self.output / "binary-build-source.sha256"
        check = self.command("reader", self.xvfb("reader_v2_runtime.py", "--workspace", self.workspace,
            "--app-root", self.app, "--binary", self.binary, "--expected-binary-sha256", self.binary_sha,
            "--source-manifest", manifest, "--source-sha256", sha256(manifest), "--output", self.output / "reader"))
        result = json.loads((self.output / "reader/reader-v2-runtime-result.json").read_text())
        require(result["runtime_passed"] is True and len(result["cases"]) == 9, "The complete nine-case Reader matrix did not pass")
        return {"checks": [check], "cases": len(result["cases"]), "visual_review": result["visual_review"]["status"]}

    def guest(self):
        check = self.command("guest", self.xvfb("reader_guest_fixtures.py", "--workspace", self.workspace,
            "--binary", self.binary, "--expected-binary-sha256", self.binary_sha, "--assets-root", self.app / "assets/demo",
            "--output", self.output / "guest", "--include-loading"))
        result = json.loads((self.output / "guest/results.json").read_text())
        require(result["all_selected_automated_invariants_passed"] is True and len(result["cases"]) == 5,
                "Guest cases failed or contain an observability gap; see the fixture report")
        return {"checks": [check], "cases": len(result["cases"]), "visual_review": "pending",
                "manual_visual_review_pending": result["manual_visual_review_pending"]}

    def startup(self):
        check = self.command("startup", [sys.executable, self.harness / "startup_frames.py", "--binary", self.binary,
            "--expected-binary-sha256", self.binary_sha, "--output", self.output / "startup"])
        result = json.loads((self.output / "startup/results.json").read_text())
        require(result["passed"] == result["total"] == 2, "Startup frame checks failed")
        return {"checks": [check], "cases": result["total"], "visual_review": "pending"}

    def lifecycle(self):
        check = self.command("lifecycle", self.xvfb("launcher_real_reader.py", "--workspace", self.workspace,
            "--binary", self.binary, "--expected-binary-sha256", self.binary_sha, "--output", self.output / "lifecycle"))
        result = json.loads((self.output / "lifecycle/launcher-real-reader-result.json").read_text())
        require(result["passed"] is True, "Real-process lifecycle assertions failed")
        return {"checks": [check], "scope": "Production launcher with controlled Linux services and real Rust process"}

    def launcher(self):
        syntax = self.command("launcher-shell-syntax", ["sh", "-n", self.app / "packaging/kindle/launch.sh"])
        fixtures = self.command("launcher-fixtures", [sys.executable, self.harness / "launcher_fixtures.py",
                                "--tests", self.app / "packaging/tests/launcher_test.py"], timeout=600)
        fixture_log = (self.logs / "launcher-fixtures.log").read_text()
        count = re.search(r"Ran (\d+) tests? in", fixture_log)
        require(count and int(count.group(1)) > 0, "The launcher fixture log has no executed tests")
        return {"checks": [syntax, fixtures], "tests_run": int(count.group(1)),
                "scope": "Controlled launcher fixtures; no real system services"}

    def package(self):
        checks = []
        if self.args.build_kindle:
            checks.append(self.command("arm-build-package", ["sh", self.app / "scripts/build-kindle.sh", self.workspace], timeout=2400))
        version = __import__("tomllib").loads((self.app / "Cargo.toml").read_text())["package"]["version"]
        archive = self.args.archive or self.app / f"artifacts/verification/bilicomics-kindle-{version}-dev.zip"
        binary = self.args.arm_binary or self.target / "armv7-unknown-linux-musleabihf/release/bilicomics"
        require(binary.is_file() and archive.is_file(), "The ARM binary or development archive is missing")
        command = [sys.executable, self.harness / "verify_package.py", "--archive", archive, "--binary", binary,
                   "--app-root", self.app, "--output", self.output / "package.json",
                   "--expected-binary-sha256", self.args.expected_arm_sha256 or sha256(binary)]
        if self.args.expected_archive_sha256:
            command.extend(["--expected-archive-sha256", self.args.expected_archive_sha256])
        checks.append(self.command("package", command))
        result = json.loads((self.output / "package.json").read_text())
        require(result["status"] == "passed", "ARM/archive verification failed")
        return {"checks": checks, "archive": file_identity(archive), "arm_binary": file_identity(binary),
                "build_executed_in_this_run": self.args.build_kindle, "physical_device_verified": False}

    def run(self):
        stopped = False
        for name in PHASES:
            if name not in self.args.phases:
                continue
            if stopped:
                self.report["phases"][name] = {"status": "not_run", "reason": "A previous stage timed out or was interrupted"}
                continue
            started = time.monotonic()
            try:
                if name in ("defaults", "pointer", "reader", "guest", "startup", "lifecycle"):
                    if "core" in self.args.phases:
                        require(self.report["phases"]["core"]["status"] == "passed",
                                "The desktop runtime phase requires a successful core build in this run")
                    self.identify_binary()
                detail = getattr(self, name)()
                self.report["phases"][name] = {"status": "passed", "seconds": round(time.monotonic() - started, 3), **detail}
            except (StageTimeout, KeyboardInterrupt) as error:
                self.report["phases"][name] = {"status": "failed", "seconds": round(time.monotonic() - started, 3),
                                               "error": str(error), "traceback": traceback.format_exc(),
                                               "suite_stopped": True}
                stopped = True
            except Exception as error:
                self.report["phases"][name] = {"status": "failed", "seconds": round(time.monotonic() - started, 3),
                                               "error": str(error), "traceback": traceback.format_exc()}
            self.save()
            print(json.dumps({"phase": name, "status": self.report["phases"][name]["status"], "output": str(self.output)}), flush=True)
        unchanged = self.source_text == manifest_text(self.app, SOURCE_TREES) and self.pipeline_text == manifest_text(self.app, SOURCE_TREES + PIPELINE_TREES)
        self.report["inputs_unchanged_during_run"] = unchanged
        if self.binary_sha:
            binary_unchanged = self.binary.is_file() and sha256(self.binary) == self.binary_sha
            self.report["desktop_binary_unchanged_during_run"] = binary_unchanged
            unchanged = unchanged and binary_unchanged
        failed = [name for name, result in self.report["phases"].items() if result["status"] == "failed"]
        _, _, special = self.evidence_paths()
        self.report["evidence_portability"] = {
            "regular_files_and_directories_only": not special,
            "special_files_rejected": special,
            "manifest": str(self.output / "evidence-manifest.json"),
            "policy": "Each harness must remove its own FIFOs and other special files before Windows evidence retrieval",
        }
        self.report["automated_status"] = "passed" if unchanged and not failed and not special else "failed"
        self.report["finished_at"] = datetime.datetime.now(datetime.timezone.utc).isoformat()
        self.save()
        files, directories, special = self.evidence_paths()
        write_json(self.output / "evidence-manifest.json", {
            "schema_version": 1,
            "regular_files_and_directories_only": not special,
            "special_files_rejected": special,
            "directories": directories,
            "files": [{"path": path.relative_to(self.output).as_posix(), "bytes": path.stat().st_size,
                       "sha256": sha256(path)} for path in files if path != self.output / "evidence-manifest.json"],
            "manifest_excludes_itself": True,
        })
        print(json.dumps({"automated_status": self.report["automated_status"], "failed_phases": failed,
                          "summary": str(self.output / "summary.json")}), flush=True)
        return 0 if self.report["automated_status"] == "passed" else 1


def main():
    require(sys.platform.startswith("linux") and os.environ.get("BILICOMICS_REMOTE_VERIFICATION") == "1",
            "Run through ssh test-env with BILICOMICS_REMOTE_VERIFICATION=1; local verification is not authorized")
    os.umask(0o077)
    signal.signal(signal.SIGTERM, interrupt_suite)
    signal.signal(signal.SIGHUP, interrupt_suite)
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--workspace", type=Path, required=True)
    parser.add_argument("--app-root", type=Path)
    parser.add_argument("--target-dir", type=Path)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--phases", default=",".join(PHASES[:-1]))
    parser.add_argument("--check-only", action="store_true")
    parser.add_argument("--binary", type=Path)
    parser.add_argument("--expected-binary-sha256")
    parser.add_argument("--binary-source-manifest", type=Path)
    parser.add_argument("--build-kindle", action="store_true")
    parser.add_argument("--archive", type=Path)
    parser.add_argument("--arm-binary", type=Path)
    parser.add_argument("--expected-arm-sha256")
    parser.add_argument("--expected-archive-sha256")
    arguments = parser.parse_args()
    selected = list(dict.fromkeys(arguments.phases.split(",")))
    require(selected and all(name in PHASES for name in selected), f"Select phases from {','.join(PHASES)}")
    if arguments.check_only:
        require(not arguments.build_kindle, "--check-only cannot be combined with --build-kindle")
        selected = ["core"]
    if arguments.build_kindle and "package" not in selected:
        selected.append("package")
    arguments.phases = selected
    suite = Suite(arguments)
    return suite.run()


if __name__ == "__main__":
    sys.exit(main())
