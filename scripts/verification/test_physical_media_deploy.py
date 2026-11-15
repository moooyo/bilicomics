"""Verify media-worker staging using synthetic ARM headers and mocked transport.

Run only on test-env. These cases neither connect to a Kindle nor acquire a
lease, execute an ARM input, or inspect a framebuffer.
"""

import io
import json
import os
from pathlib import Path
import struct
import subprocess
from subprocess import CompletedProcess
import sys
import tarfile
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch
import zipfile

import physical_device as physical


def setUpModule():
    if sys.platform != "linux" or os.environ.get("BILICOMICS_REMOTE_VERIFICATION") != "1":
        raise RuntimeError("Run media staging tests only through ssh test-env")


class PhysicalMediaDeploymentTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="bilicomics-media-deploy-")
        self.addCleanup(self.temporary.cleanup)
        self.directory = Path(self.temporary.name)
        header = bytearray(64)
        header[:7] = b"\x7fELF\x01\x01\x01"
        struct.pack_into("<H", header, 18, 40)
        self.binary = bytes(header) + b"synthetic-application"
        self.worker = bytes(header) + b"synthetic-jpeg-worker"
        self.helper = self.directory / "framebuffer-header"
        self.helper.write_bytes(bytes(header))
        self.script = self.directory / "wrapper.sh"
        self.script.write_bytes(b"#!/bin/sh\nexit 0\n")
        self.staged = {}

    def harness(self, *, worker=True, selected=True, worker_bytes=None, device_mode="755",
                worker_executable=True, worker_archive_mode=0o755):
        package = self.directory / "package.zip"
        payload = self.worker if worker_bytes is None else worker_bytes
        with zipfile.ZipFile(package, "w") as archive:
            archive.writestr("bilicomics/app/bilicomics", self.binary)
            archive.writestr("bilicomics/app/library.dat", b"non-executable support data")
            if worker:
                info = zipfile.ZipInfo("bilicomics/app/kindle-jpeg-worker")
                info.external_attr = (0o100000 | worker_archive_mode) << 16
                archive.writestr(info, payload)
            archive.writestr("bilicomics/data/device-allowlist", b"# No accepted pair\n")
            archive.writestr("bilicomics/launch.sh", self.script.read_bytes())
        harness = physical.Harness.__new__(physical.Harness)
        harness.root = "/mnt/us/bilicomics-physical-media-deploy-test"
        harness.logs = harness.root + "/data/logs"
        harness.output = self.directory
        harness.state = {"phase": "preflight", "profile": "scribe", "expected_firmware": "synthetic-firmware",
                         "preflight": {"native_epdc_mode": "Y8"}}
        harness.args = SimpleNamespace(
            content_mode="demo", fixture_directory=None, expected_fixture_sha256=None,
            socket_guard=None, expected_guard_sha256=None, approve_runtime_pair=True,
            package=package, expected_package_sha256=physical.file_hash(package),
            expected_binary_sha256=physical.sha256(self.binary),
            expected_worker_sha256=physical.sha256(payload) if selected else None,
            framebuffer_helper=self.helper, expected_helper_sha256=physical.file_hash(self.helper),
            wrapper=self.script, watchdog=self.script, event_wrapper=self.script,
        )

        def remote(command, *, data=None, **_):
            if command.startswith("tar -xf "):
                with tarfile.open(fileobj=io.BytesIO(data), mode="r:") as archive:
                    for member in archive.getmembers():
                        self.staged[member.name] = (archive.extractfile(member).read(), member.mode)
                return CompletedProcess([], 0, b"", b"")
            if command.startswith("sha256sum "):
                hashes = "".join(f"{physical.sha256(value[0])}  {harness.root}/{name}\n"
                                 for name, value in sorted(self.staged.items()))
                return CompletedProcess([], 0, hashes.encode(), b"")
            if command.startswith("set -eu\n[ -f ") and "kindle-jpeg-worker" in command:
                self.assertIn("[ -x ", command)
                self.assertIn("[ ! -L ", command)
                self.assertIn("readlink -f ", command)
                self.assertIn(physical.sha256(payload), command)
                self.assertNotIn(" = 755 ]", command)
                self.assertEqual(self.staged["app/kindle-jpeg-worker"][1], 0o755)
                if not worker_executable:
                    raise RuntimeError("The device JPEG worker is not executable.")
                modes = "".join(f"{role} {device_mode}\n" for role in ("worker", "application", "framebuffer_helper"))
                return CompletedProcess([], 0, modes.encode("ascii"), b"")
            if command.endswith("/physical-framebuffer --info"):
                geometry = {"width": 1860, "height": 2480, "xoffset": 0, "yoffset": 0, "read_only": True}
                return CompletedProcess([], 0, json.dumps(geometry).encode(), b"")
            self.assertTrue(command.startswith("[ ! -e "), "Unexpected mocked deployment operation")
            return CompletedProcess([], 0, b"", b"")

        harness.remote = Mock(side_effect=remote)
        harness.save = Mock()
        return harness

    def test_selected_worker_is_0755_and_support_data_stays_nonexecutable(self):
        harness = self.harness()
        self.assertEqual(harness.deploy()["phase"], "deployed")
        self.assertEqual(self.staged["app/kindle-jpeg-worker"], (self.worker, 0o755))
        self.assertEqual(self.staged["app/library.dat"][1], 0o644)
        self.assertEqual(harness.state["jpeg_worker"]["sha256"], physical.sha256(self.worker))
        self.assertTrue(harness.state["jpeg_worker"]["deployed_identity_verified"])
        self.assertEqual(harness.state["jpeg_worker"]["staged_mode"], "0755")
        self.assertEqual(harness.state["jpeg_worker"]["observed_device_mode"], "755")
        self.assertTrue(any("stat -c '%a'" in call.args[0] for call in harness.remote.call_args_list))

    def test_worker_without_selected_digest_fails_before_transport(self):
        harness = self.harness(selected=False)
        with self.assertRaisesRegex(RuntimeError, "explicitly selected SHA-256"):
            harness.deploy()
        harness.remote.assert_not_called()

    def test_device_vfat_777_mapping_keeps_exact_executable_and_hash_checks(self):
        harness = self.harness(device_mode="777")
        self.assertEqual(harness.deploy()["phase"], "deployed")
        self.assertEqual(self.staged["app/kindle-jpeg-worker"][1], 0o755)
        worker = harness.state["jpeg_worker"]
        self.assertEqual(worker["observed_device_mode"], "777")
        self.assertEqual(set(worker["same_filesystem_mode_observations"].values()), {"777"})
        self.assertTrue(worker["executable_verified"] and worker["deployed_identity_verified"])

    def test_device_nonexecutable_worker_cannot_be_accepted_from_its_mode_string(self):
        harness = self.harness(device_mode="777", worker_executable=False)
        with self.assertRaisesRegex(RuntimeError, "not executable"):
            harness.deploy()
        self.assertEqual(harness.state["phase"], "preflight")
        self.assertNotIn("jpeg_worker", harness.state)

    def test_worker_archive_must_still_declare_0755(self):
        harness = self.harness(worker_archive_mode=0o644)
        with self.assertRaisesRegex(RuntimeError, "retain mode 0755"):
            harness.deploy()
        harness.remote.assert_not_called()

    def test_real_shell_rejects_failed_worker_guards_before_publishing_mode_evidence(self):
        harness = self.harness()
        harness.deploy()
        proof = next(call.args[0] for call in harness.remote.call_args_list
                     if call.args[0].startswith("set -eu\n[ -f "))
        for fault in ("valid", "non-executable", "symlink", "wrong-bytes", "directory"):
            with self.subTest(fault=fault):
                root = self.directory / "shell" / fault
                for relative, (content, mode) in self.staged.items():
                    path = root / relative
                    path.parent.mkdir(parents=True, exist_ok=True)
                    path.write_bytes(content)
                    path.chmod(mode)
                worker = root / "app/kindle-jpeg-worker"
                if fault == "non-executable":
                    worker.chmod(0o644)
                elif fault == "symlink":
                    worker.unlink()
                    worker.symlink_to(root / "app/library.dat")
                elif fault == "wrong-bytes":
                    worker.write_bytes(b"changed after the upload digest check")
                elif fault == "directory":
                    worker.unlink()
                    worker.mkdir()
                command = proof.replace(harness.root, str(root))
                result = subprocess.run(["sh", "-c", command], capture_output=True, timeout=5)
                if fault == "valid":
                    self.assertEqual(result.returncode, 0, result.stderr.decode())
                    self.assertIn(b"worker 755\n", result.stdout)
                else:
                    self.assertEqual(result.returncode, 4)
                    self.assertEqual(result.stdout, b"", "A failed guard published misleading mode evidence")

    def test_changed_worker_fails_before_transport(self):
        harness = self.harness()
        harness.args.expected_worker_sha256 = "0" * 64
        with self.assertRaisesRegex(RuntimeError, "worker digest differs"):
            harness.deploy()
        harness.remote.assert_not_called()

    def test_non_arm_worker_fails_before_transport(self):
        harness = self.harness(worker_bytes=b"not an executable")
        with self.assertRaisesRegex(RuntimeError, "ARM ELF"):
            harness.deploy()
        harness.remote.assert_not_called()

    def test_selected_worker_cannot_be_missing_from_package(self):
        harness = self.harness(worker=False)
        with self.assertRaisesRegex(RuntimeError, "worker is missing"):
            harness.deploy()
        harness.remote.assert_not_called()

    def test_previous_package_without_worker_keeps_its_original_scope(self):
        harness = self.harness(worker=False, selected=False)
        self.assertEqual(harness.deploy()["phase"], "deployed")
        self.assertNotIn("jpeg_worker", harness.state)
        self.assertNotIn("app/kindle-jpeg-worker", self.staged)

    def test_action_selection_requires_one_declared_bounded_data_file(self):
        entry = {"path": "data/media-actions.json", "sha256": "a" * 64, "bytes": 40}
        manifest = {"kind": "native_media_offline_guest_fixture", "action_files": [entry["path"]],
                    "files": [entry]}
        self.assertEqual(physical.media_fixture_actions(manifest, entry["path"]), entry)
        for path in ("../media-actions.json", "data/nested/actions.json", "data/other.json", None):
            with self.subTest(path=path), self.assertRaises(RuntimeError):
                physical.media_fixture_actions(manifest, path)
        for invalid in (0, 131073, True):
            with self.subTest(bytes=invalid), self.assertRaises(RuntimeError):
                physical.media_fixture_actions({**manifest, "files": [{**entry, "bytes": invalid}]}, entry["path"])
        with self.assertRaisesRegex(RuntimeError, "one manifest identity"):
            physical.media_fixture_actions({**manifest, "files": [entry, entry]}, entry["path"])
        with self.assertRaises(RuntimeError):
            physical.media_fixture_actions({**manifest, "kind": "distinct_offline_guest_fixture"}, entry["path"])

    def test_action_argument_cannot_enter_the_demonstration_start_path(self):
        harness = self.harness()
        harness.state.update(phase="deployed", content_mode="demo")
        harness.args.fixture_actions = "data/media-actions.json"
        with self.assertRaisesRegex(RuntimeError, "explicit media_fixture mode"):
            harness.start()
        harness.remote.assert_not_called()

    def test_media_start_requires_a_declared_plan_before_any_transport(self):
        harness = self.harness()
        harness.state.update(phase="deployed", content_mode="media_fixture",
                             fixture_manifest={"kind": "native_media_offline_guest_fixture",
                                               "action_files": [], "files": []})
        harness.args.fixture_actions = None
        with self.assertRaisesRegex(RuntimeError, "validated"):
            harness.start()
        harness.remote.assert_not_called()

    def test_media_state_rejection_preserves_the_checkpoint_retry_contract(self):
        with patch("native_media_physical_fixture.reader_snapshot", side_effect=ValueError("not yet quiet")):
            with self.assertRaisesRegex(RuntimeError, "not yet quiet"):
                physical.media_fixture_reader_snapshot([], {}, "reader")


if __name__ == "__main__":
    unittest.main(verbosity=2)
