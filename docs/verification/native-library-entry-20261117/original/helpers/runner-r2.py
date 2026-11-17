#!/usr/bin/env python3
"""Verify frozen Library entry fixtures and refresh packaging on test-env only."""

import argparse
import datetime
import hashlib
import importlib.util
import json
import os
from pathlib import Path, PurePosixPath
import platform
import re
import shlex
import shutil
import signal
import stat
import subprocess
import sys
import tarfile
import time
import traceback
import zipfile


ROOT = Path("/root/bilicomics-native-library-entry-20261117-01a1d11a-r1")
SOURCE = ROOT / "candidate-r1/app"
ARCHIVE = ROOT / "candidate-r1/source.tar"
OUTPUT = ROOT / "checks-r2"
BASE = Path("/root/bilicomics-startup-sequence-20261117-01a1d11a-r1/checks-r3")
BASE_SELECTED = BASE / "selected-arm"
COMMIT = "39fac822a982868078201e605e50bdacf1dcf3ed"
ARCHIVE_SHA256 = "5c513d92e48cbcdcbf408dbc02a80a6d9678455e4e06c95a62dd63523736b4b5"
SOURCE_SHA256 = "8537123d6b1dc0c75a048b09d1204664988a6c1668ea35d721bfbac641ff49c6"
BINARY_SHA256 = "04fc6f93b4327911ca1a3bb5200175c9f2e13fb21ac5e56280012ff44b6e2e59"
WORKER_SHA256 = "12c8988c6a93cade9923d54bef784b774e13cb5e3b042f2e72ea5ecb6b897beb"
BASE_ZIP_SHA256 = "1fe8ac38613d21904d187b41e973ff80302b0c78fe69b320e3d7143a31fd53ee"
PACKAGE_NAME = "bilicomics-kindle-0.1.25-dev.zip"
NEW_CASES = (
    "test_library_entry_waits_for_independent_worker_and_normal_exit",
    "test_library_entry_preserves_worker_failure_status",
    "test_library_drive_failure_before_worker_record_finishes_owned_cleanup",
    "test_library_hup_requests_worker_cleanup_and_waits_for_its_status",
    "test_library_int_requests_worker_cleanup_and_waits_for_its_status",
    "test_library_term_requests_worker_cleanup_and_waits_for_its_status",
    "test_library_early_cancellation_waits_until_worker_cleanup_traps_exist",
    "test_library_cancellation_before_pid_capture_remains_pending",
    "test_library_cancellation_does_not_signal_a_changed_worker_birth",
    "test_retiring_only_library_entry_keeps_worker_sleep_and_recovery_alive",
    "test_second_library_cancellation_never_targets_the_first_lock_owner",
    "test_library_entry_propagates_setsid_exec_failure",
    "test_library_entry_refuses_nohup_when_setsid_is_missing",
    "test_default_admission_returns_while_its_independent_worker_remains_active",
    "test_default_admission_preserves_detached_setsid_failure_acknowledgement",
    "test_default_admission_preserves_nohup_fallback",
    "test_detached_argument_keeps_existing_supervisor_identity_and_status",
)


def now():
    return datetime.datetime.now(datetime.timezone.utc).isoformat()


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def sha256(path):
    result = hashlib.sha256()
    with Path(path).open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            result.update(block)
    return result.hexdigest()


def identity(path):
    path = Path(path)
    require(path.is_file() and not path.is_symlink(), "A selected input is not a regular file: " + str(path))
    return {"path": str(path), "bytes": path.stat().st_size, "sha256": sha256(path)}


def write_json(path, value):
    temporary = path.with_name(path.name + ".tmp")
    temporary.write_text(json.dumps(value, indent=2) + "\n", encoding="utf-8")
    temporary.replace(path)


def process_identity(pid):
    fields = Path(f"/proc/{pid}/stat").read_text().rsplit(") ", 1)[1].split()
    return {"pid": pid, "state": fields[0], "parent": int(fields[1]),
            "group": int(fields[2]), "session": int(fields[3]), "birth": fields[19]}


def stop_owned_stage(process, expected):
    """Allow fixture finalizers to run; never discover another task's processes."""
    report = {"process_id": process.pid, "identity": expected, "signals": [],
              "scope": "Only the exact newly created stage session; fixtures own detached worker cleanup"}
    for signum, seconds in ((signal.SIGINT, 55), (signal.SIGTERM, 35), (signal.SIGKILL, 5)):
        if process.poll() is not None:
            break
        try:
            current = process_identity(process.pid)
        except FileNotFoundError:
            break
        require(current["birth"] == expected["birth"]
                and current["group"] == current["session"] == process.pid,
                "A timed-out stage no longer has the owned PID/birth/session identity")
        os.killpg(process.pid, signum)
        report["signals"].append(signal.Signals(signum).name)
        try:
            process.wait(timeout=seconds)
        except subprocess.TimeoutExpired:
            pass
    process.wait(timeout=5)
    report["leader_reaped"] = True
    report["returncode"] = process.returncode
    return report


class Runner:
    def __init__(self):
        require(sys.platform == "linux" and ROOT.resolve(strict=True) == ROOT
                and SOURCE.resolve(strict=True) == SOURCE,
                "Run only in the selected isolated test-env workspace")
        os.umask(0o077)
        sys.dont_write_bytecode = True
        OUTPUT.mkdir(mode=0o700, exist_ok=False)
        self.logs = OUTPUT / "logs"
        self.logs.mkdir(mode=0o700)
        self.temporary = OUTPUT / "tmp"
        self.temporary.mkdir(mode=0o700)
        self.selected = OUTPUT / "selected-arm"
        self.selected.mkdir(mode=0o700)
        self.environment = os.environ.copy()
        cleared = []
        for name in list(self.environment):
            if name in {"PYTHONPATH", "PYTHONHOME", "DISPLAY", "WAYLAND_DISPLAY"} or name.startswith((
                    "BILICOMICS_", "SLINT_KINDLE_", "TEST_")):
                cleared.append(name)
                del self.environment[name]
        self.environment.update(PYTHONDONTWRITEBYTECODE="1", PYTHONNOUSERSITE="1",
            PYTHONPATH=str(SOURCE / "scripts/verification"), TMPDIR=str(self.temporary),
            CARGO_NET_OFFLINE="true", BILICOMICS_REMOTE_VERIFICATION="1", LC_ALL="C", TZ="UTC")
        self.archived_files = {}
        self.frozen_files = {}
        self.manifest_function = None
        self.manifest_trees = None
        self.manifest_before = None
        self.run_suite_manifest_sha256 = None
        self.report = {"schema_version": 1, "status": "running", "passed": False,
            "started_at": now(), "candidate_commit": COMMIT, "root": str(ROOT),
            "source": str(SOURCE), "output": str(OUTPUT), "runner": identity(Path(__file__)),
            "shell_runner": identity(Path(__file__).with_name("run-r2.sh")),
            "host": {"hostname": platform.node(), "system": platform.system(), "machine": platform.machine()},
            "environment": {name: self.environment[name] for name in (
                "PYTHONDONTWRITEBYTECODE", "PYTHONNOUSERSITE", "PYTHONPATH", "TMPDIR",
                "CARGO_NET_OFFLINE", "BILICOMICS_REMOTE_VERIFICATION", "LC_ALL", "TZ")},
            "cleared_inherited_environment_names": sorted(cleared), "steps": [],
            "build_performed": False, "rust_compilation_performed": False,
            "arm_binary_executed": False, "physical_device_accessed": False,
            "device_lease_acquired": False, "firmware_emulator_used": False,
            "live_account_used": False, "remote_account_writes": False, "device_verified": False,
            "limitations": ["Linux fixture and static archive checks do not establish native Library behavior or physical acceptance.",
                "The application and JPEG helper are unchanged selected ARM build inputs and are never executed by this runner."]}
        self.save()

    def save(self):
        write_json(OUTPUT / "summary.json", self.report)

    def run(self, label, command, timeout, expected_tests=None, cwd=SOURCE):
        command = [str(part) for part in command]
        log = self.logs / (label + ".log")
        entry = {"label": label, "command": command, "shell_display": shlex.join(command),
                 "cwd": str(cwd), "timeout_seconds": timeout, "started_at": now(), "status": "running"}
        self.report["steps"].append(entry)
        self.save()
        print(json.dumps({"running": label, "command": command}), flush=True)
        process = None
        process_before = None
        started = time.monotonic()
        try:
            with log.open("x", encoding="utf-8") as output:
                process = subprocess.Popen(command, cwd=cwd, env=self.environment, stdout=output,
                                           stderr=subprocess.STDOUT, start_new_session=True)
                process_before = process_identity(process.pid)
                entry["process_identity"] = process_before
                entry["returncode"] = process.wait(timeout=timeout)
            require(entry["returncode"] == 0, label + " failed; inspect its complete raw log")
            if expected_tests is not None:
                value = log.read_text(encoding="utf-8")
                counts = re.findall(r"^Ran (\d+) tests? in [0-9.]+s$", value, re.MULTILINE)
                require(counts == [str(expected_tests)] and re.search(r"^OK$", value, re.MULTILINE),
                        label + " did not report the complete expected passing unittest suite")
                entry["executed_tests"] = expected_tests
                entry["unittest_ok"] = True
            entry["status"] = "passed"
        except BaseException as error:
            entry["status"] = "failed"
            entry["error"] = type(error).__name__ + ": " + str(error)
            if process is not None and process.poll() is None and process_before is not None:
                entry["cleanup"] = stop_owned_stage(process, process_before)
            raise
        finally:
            entry["seconds"] = round(time.monotonic() - started, 3)
            entry["finished_at"] = now()
            if log.is_file():
                entry["log"] = identity(log)
            self.save()
        print(json.dumps({"passed": label, "seconds": entry["seconds"],
                          "executed_tests": entry.get("executed_tests")}), flush=True)
        return log

    def freeze(self, source, destination, expected_sha=None):
        require(not destination.exists(), "A frozen output must be new")
        with Path(source).open("rb") as before, destination.open("xb") as after:
            shutil.copyfileobj(before, after, length=1024 * 1024)
        destination.chmod(0o400)
        result = identity(destination)
        require(result["sha256"] == sha256(source), "A frozen copy differs from its selected source")
        if expected_sha:
            require(result["sha256"] == expected_sha, "A selected frozen input differs from its digest")
        self.frozen_files[str(destination)] = result["sha256"]
        return result

    def prepare_sources(self):
        require(sha256(ARCHIVE) == ARCHIVE_SHA256, "The source archive digest differs from its pin")
        self.report["source_archive"] = identity(ARCHIVE)
        rows = []
        with tarfile.open(ARCHIVE, "r:*") as stored:
            require(stored.pax_headers.get("comment") == COMMIT, "The Git archive PAX commit differs")
            members = stored.getmembers()
            require(all(item.isdir() or item.isfile() for item in members), "The source archive has a special entry")
            files = [item for item in members if item.isfile()]
            require(files, "The source archive has no files")
            prefix = "app/" if all(item.name.startswith("app/") for item in files) else ""
            for item in files:
                name = item.name[len(prefix):]
                relative = PurePosixPath(name)
                require(not relative.is_absolute() and ".." not in relative.parts
                        and relative.as_posix() == name and name not in self.archived_files,
                        "The source archive contains an unsafe or duplicate file path")
                extracted = SOURCE / Path(*relative.parts)
                require(extracted.is_file() and not extracted.is_symlink()
                        and extracted.resolve().is_relative_to(SOURCE), "An extracted source is unsafe: " + name)
                with stored.extractfile(item) as stream:
                    digest = hashlib.file_digest(stream, "sha256").hexdigest()
                require(extracted.stat().st_size == item.size and sha256(extracted) == digest,
                        "Extracted bytes differ from the frozen Git archive: " + name)
                self.archived_files[name] = digest
                rows.append(digest + "  " + name + "\n")
        paths = list(SOURCE.rglob("*"))
        require(not any(path.is_symlink() for path in paths), "The extracted source contains a symlink")
        require({path.relative_to(SOURCE).as_posix() for path in paths if path.is_file()} == set(self.archived_files),
                "The extracted source file set differs from the frozen archive")
        archive_manifest = OUTPUT / "archive-source-inputs.sha256"
        archive_manifest.write_text("".join(sorted(rows, key=lambda row: row.split("  ", 1)[1])), encoding="utf-8")
        self.report["archive_source_inputs"] = dict(identity(archive_manifest), files=len(rows),
            pax_commit=COMMIT, all_extracted_bytes_identical=True, extracted_file_set_exact=True)
        sys.path.insert(0, str(SOURCE / "scripts/verification"))
        from run_suite import SOURCE_TREES, manifest_text
        native_spec = importlib.util.spec_from_file_location("frozen_native_protocol", SOURCE / "scripts/verification/native_protocol.py")
        native = importlib.util.module_from_spec(native_spec)
        native_spec.loader.exec_module(native)
        self.manifest_function = native.build_input_manifest
        self.manifest_before = self.manifest_function(SOURCE)
        current = OUTPUT / "source-inputs.sha256"
        current.write_text(self.manifest_before, encoding="utf-8")
        old = BASE / "source-inputs.sha256"
        require(sha256(old) == SOURCE_SHA256 and current.read_bytes() == old.read_bytes(),
                "The complete binary build source manifest differs from the selected old build")
        alternate_text = manifest_text(SOURCE, SOURCE_TREES)
        alternate = OUTPUT / "source-inputs-run-suite.sha256"
        alternate.write_text(alternate_text, encoding="utf-8")
        self.run_suite_manifest_sha256 = sha256(alternate)
        def pairs(value):
            return {name: digest for digest, name in (line.split("  ", 1) for line in value.splitlines())}
        require(pairs(alternate_text) == pairs(self.manifest_before),
                "The two repository build manifest functions disagree on a path or digest")
        for row in self.manifest_before.splitlines():
            digest, name = row.split("  ", 1)
            require(self.archived_files.get(name) == digest, "A binary input is not bound to the Git archive")
        self.freeze(old, OUTPUT / "binary-build-source.sha256", SOURCE_SHA256)
        self.report["binary_input_provenance"] = {"source_inputs": identity(current),
            "old_build_source_inputs": identity(old), "files": len(self.manifest_before.splitlines()),
            "byte_identical": True, "canonical_serialization": "native_protocol.build_input_manifest using sorted Path values",
            "alternate_manifest": identity(alternate), "both_repository_manifest_path_digest_pairs_identical": True,
            "different_serialization_order_is_not_a_source_change": True}
        self.report["preserved_first_attempt"] = {"summary": identity(ROOT / "checks-r1/summary.json"),
            "diagnosis": identity(ROOT / "manifest-order-diagnosis-r1.json"),
            "reason": "Correct verifier serialization order without changing any source input or historical evidence"}
        self.report["product_and_package_sources"] = {name: digest for name, digest in sorted(self.archived_files.items())
            if name.startswith(("packaging/", "scripts/verification/", "docs/research/aidoku-native-library-entry-20261117/"))
            or name == "THIRD_PARTY_NOTICES.md"}
        self.save()

    def toolchain(self):
        self.run("toolchain-python", [sys.executable, "-VV"], 30)
        self.run("toolchain-readelf", ["readelf", "--version"], 30)
        self.run("toolchain-setsid", ["setsid", "--version"], 30)
        tools = {}
        for name in ("python3", "sh", "setsid", "readelf"):
            path = shutil.which(name, path=self.environment.get("PATH"))
            require(path is not None, "A required verification tool is unavailable: " + name)
            tools[name] = {"selected_path": path, "executable": identity(Path(path).resolve(strict=True))}
        self.report["verification_tools"] = tools
        self.report["base_build_toolchain_record"] = self.freeze(BASE / "compiler-profile.json", OUTPUT / "base-compiler-profile.json")
        self.report["base_build_summary"] = self.freeze(BASE / "summary.json", OUTPUT / "base-build-summary.json")
        self.save()

    def fixtures(self):
        launcher = SOURCE / "packaging/tests/launcher_test.py"
        self.run("library-entry-lifecycle-17", [sys.executable, "-B", launcher, "-v"]
                 + ["LauncherLifecycleTests." + name for name in NEW_CASES], 1200, 17)
        self.run("launcher-full-146", [sys.executable, "-B", launcher, "-v"], 1200, 146)
        self.run("library-entry-package-25", [sys.executable, "-B", "-m", "unittest", "-v",
                 "test_library_entry_package"], 1200, 25)
        for module, count in (("test_startup_package", 18), ("test_spatial_dither_package", 5), ("test_native_media_package", 3)):
            self.run(module.replace("_", "-"), [sys.executable, "-B", "-m", "unittest", "-v", module], 1200, count)
        for name in ("launch.sh", "document-launcher.sh"):
            self.run("shell-syntax-" + name[:-3], ["sh", "-n", SOURCE / "packaging/kindle" / name], 30)

    def package(self):
        base_zip = BASE_SELECTED / PACKAGE_NAME
        require(sha256(base_zip) == BASE_ZIP_SHA256, "The selected base archive differs from its pin")
        binary = self.selected / "bilicomics"
        worker = self.selected / "kindle-jpeg-worker"
        self.report["selected_arm_binary"] = self.freeze(BASE_SELECTED / "bilicomics", binary, BINARY_SHA256)
        self.report["selected_jpeg_worker"] = self.freeze(BASE_SELECTED / "kindle-jpeg-worker", worker, WORKER_SHA256)
        self.report["base_archive"] = identity(base_zip)
        self.report["base_archive_sidecar"] = identity(base_zip.with_suffix(".manifest.json"))
        package = self.selected / PACKAGE_NAME
        provenance = OUTPUT / "rebundle-provenance.json"
        self.run("rebundle-library-entry", [sys.executable, "-B", SOURCE / "scripts/verification/rebundle_launcher.py",
            "--source-root", SOURCE, "--base-archive", base_zip,
            "--expected-base-archive-sha256", BASE_ZIP_SHA256, "--expected-binary-sha256", BINARY_SHA256,
            "--expected-worker-sha256", WORKER_SHA256, "--binary-source-manifest", BASE / "source-inputs.sha256",
            "--expected-binary-source-manifest-sha256", SOURCE_SHA256,
            "--output", package, "--provenance-output", provenance], 1200)
        require(package.is_file(), "The rebundler did not create an archive")
        result = json.loads(provenance.read_text(encoding="utf-8"))
        require(result.get("passed") is True and result.get("complete_source_pairs_identical") is True
                and result.get("source_manifest_sha256") == self.run_suite_manifest_sha256
                and result.get("binary_source_manifest_sha256") == SOURCE_SHA256
                and result.get("base_entry_count") == 45 and result.get("output_entry_count") == 49
                and result.get("build_performed") is False and result.get("arm_binary_executed") is False
                and result.get("physical_device_accessed") is False and result.get("device_verified") is False,
                "The rebundle provenance did not establish the selected static-only schema migration")
        check_output = OUTPUT / "package-verification.json"
        self.run("strict-package-verification", [sys.executable, "-B", SOURCE / "scripts/verification/verify_package.py",
            "--archive", package, "--binary", binary, "--worker", worker, "--app-root", SOURCE,
            "--output", check_output, "--expected-binary-sha256", BINARY_SHA256,
            "--expected-worker-sha256", WORKER_SHA256, "--expected-archive-sha256", sha256(package)], 1200)
        checked = json.loads(check_output.read_text(encoding="utf-8"))
        require(checked.get("passed") is True and checked.get("device_verified") is False
                and checked.get("arm_binary_executed") is False and len(checked.get("checks", [])) == 20,
                "The strict static checker did not pass all 20 expected checks")
        from rebundle_launcher import PACKAGING_SOURCES
        from verify_package import ALLOWED_FILES, EXECUTABLE_FILES, FIXED_TIMESTAMP, LEGACY_ALLOWED_FILES
        rows = []
        with zipfile.ZipFile(base_zip) as previous, zipfile.ZipFile(package) as current:
            require(set(previous.namelist()) == LEGACY_ALLOWED_FILES and set(current.namelist()) == ALLOWED_FILES,
                    "The old or new ZIP file set differs from its exact schema")
            for name in current.namelist():
                actual = current.getinfo(name)
                content = current.read(name)
                row = {"path": name, "bytes": len(content), "sha256": hashlib.sha256(content).hexdigest(),
                       "mode": oct(stat.S_IMODE(actual.external_attr >> 16)), "timestamp": list(actual.date_time)}
                require(actual.date_time == FIXED_TIMESTAMP, "A package timestamp is not fixed")
                if name in EXECUTABLE_FILES:
                    require(stat.S_IMODE(actual.external_attr >> 16) == 0o755, "An executable mode differs")
                if name in previous.namelist():
                    old = previous.getinfo(name)
                    row["old_metadata_preserved"] = actual.external_attr == old.external_attr and actual.date_time == old.date_time
                    row["old_bytes_identical"] = content == previous.read(name)
                    require(row["old_metadata_preserved"], "Existing ZIP metadata changed: " + name)
                    if name not in PACKAGING_SOURCES:
                        require(row["old_bytes_identical"], "An unselected payload changed: " + name)
                if name in PACKAGING_SOURCES:
                    expected = (SOURCE / PACKAGING_SOURCES[name]).read_bytes()
                    if name in ("bilicomics/launch.sh", "documents/bilicomics.sh"):
                        expected = expected.replace(b"\r\n", b"\n")
                    require(content == expected, "Selected packaging bytes differ from frozen source: " + name)
                    row["packaging_source"] = PACKAGING_SOURCES[name]
                rows.append(row)
            require(current.read("bilicomics/app/bilicomics") == binary.read_bytes()
                    and current.read("bilicomics/app/kindle-jpeg-worker") == worker.read_bytes(),
                    "A packaged ELF differs from its selected bytes")
        audit = {"passed": True, "base_entries": 45, "output_entries": 49,
            "selected_packaging_sources": len(PACKAGING_SOURCES), "entries": rows,
            "added_entries": result["added_entries"], "updated_entries": result["updated_entries"],
            "all_old_modes_and_timestamps_preserved": True,
            "unselected_old_payloads_byte_identical": True, "embedded_arm_bytes_identical": True,
            "strict_checker_checks": len(checked["checks"]), "device_verified": False}
        write_json(OUTPUT / "package-entry-audit.json", audit)
        self.report["package"] = {"archive": identity(package),
            "sidecar": identity(package.with_suffix(".manifest.json")), "provenance": identity(provenance),
            "verification": identity(check_output), "entry_audit": identity(OUTPUT / "package-entry-audit.json"),
            "entry_count": 49, "strict_checker_checks": 20, "device_verified": False}
        for path in (package, package.with_suffix(".manifest.json")):
            self.frozen_files[str(path)] = sha256(path)
            path.chmod(0o400)
        self.save()

    def final_integrity(self):
        current_files = {path.relative_to(SOURCE).as_posix() for path in SOURCE.rglob("*") if path.is_file()}
        result = {"source_archive_unchanged": sha256(ARCHIVE) == ARCHIVE_SHA256,
            "all_archived_sources_unchanged": bool(self.archived_files) and all(
                (SOURCE / name).is_file() and not (SOURCE / name).is_symlink() and sha256(SOURCE / name) == digest
                for name, digest in self.archived_files.items()),
            "extracted_file_set_exact": current_files == set(self.archived_files),
            "compiled_inputs_unchanged": self.manifest_function is not None and self.manifest_before ==
                self.manifest_function(SOURCE),
            "frozen_artifacts_unchanged": all(sha256(path) == digest for path, digest in self.frozen_files.items()),
            "runner_unchanged": sha256(Path(__file__)) == self.report["runner"]["sha256"],
            "shell_runner_unchanged": sha256(Path(__file__).with_name("run-r2.sh")) == self.report["shell_runner"]["sha256"]}
        self.report["final_integrity"] = result
        self.report["fixture_temporary_entries_remaining"] = sorted(path.relative_to(self.temporary).as_posix()
            for path in self.temporary.rglob("*"))
        self.save()
        require(all(result.values()), "Frozen input or output integrity changed during verification")
        require(not self.report["fixture_temporary_entries_remaining"], "The fixture temporary directory is not empty")

    def execute(self):
        try:
            self.prepare_sources()
            print(json.dumps({"passed": "source-archive-and-binary-input-identity",
                "source_manifest_sha256": SOURCE_SHA256, "files": self.report["binary_input_provenance"]["files"]}), flush=True)
            self.toolchain()
            self.fixtures()
            self.package()
            self.final_integrity()
            self.report.update(status="passed", passed=True)
        except BaseException as error:
            self.report.update(status="failed", passed=False, error=type(error).__name__ + ": " + str(error))
            (OUTPUT / "failure.txt").write_text(traceback.format_exc(), encoding="utf-8")
            try:
                self.final_integrity()
            except BaseException as integrity_error:
                self.report["final_integrity_error"] = type(integrity_error).__name__ + ": " + str(integrity_error)
        finally:
            self.report["finished_at"] = now()
            self.report["executed_unittests_including_targeted_repeat"] = sum(
                entry.get("executed_tests", 0) for entry in self.report["steps"])
            self.save()
        print(json.dumps({"passed": self.report["passed"], "summary": str(OUTPUT / "summary.json"),
                          "error": self.report.get("error")}), flush=True)
        return 0 if self.report["passed"] else 1


def interrupted(signum, frame):
    raise KeyboardInterrupt("Runner interrupted by signal " + str(signum))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.parse_args()
    signal.signal(signal.SIGTERM, interrupted)
    signal.signal(signal.SIGINT, interrupted)
    return Runner().execute()


if __name__ == "__main__":
    raise SystemExit(main())
