#!/usr/bin/env python3
"""Pure host-filesystem lease tests; run only on the designated Linux worker.

All tokens and ownership records in these tests are synthetic. No network,
device, account, launcher, native service, or operator input is involved.
"""

from __future__ import annotations

from concurrent.futures import ThreadPoolExecutor
from contextlib import redirect_stdout
from dataclasses import replace
from datetime import datetime, timezone
import errno
import io
import json
import os
from pathlib import Path
import secrets
import stat
import sys
import tempfile
from threading import Barrier
import unittest
from unittest import mock

import device_lease as lease


@unittest.skipUnless(sys.platform.startswith("linux"), "The coordinator lease protocol is Linux-only.")
class DeviceLeaseTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="bilicomics-lease-test-")
        self.addCleanup(self.temporary.cleanup)
        self.base = Path(self.temporary.name)
        self.base.chmod(0o700)
        self.private = self.base / "private"
        self.private.mkdir(mode=0o700)
        self.path = self.base / "shared.lock"
        self.token_file = self.private / "expected-token"
        self.request = lease.LeaseRequest(self.path, self.token_file, "test-bili-thread", "test-run-01")

    def write_private(self, path, data):
        fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        try:
            data = data.encode("utf-8") if isinstance(data, str) else data
            while data:
                count = os.write(fd, data)
                data = data[count:]
        finally:
            os.close(fd)

    def assert_error(self, function, code):
        with self.assertRaises(lease.LeaseError) as caught:
            function()
        self.assertEqual(caught.exception.code, code)
        return caught.exception

    def guarded_release(self, request=None):
        return lease.release(request or self.request, native_restored_and_connections_closed=True)

    def owner_bytes(self):
        return (self.path / lease.OWNER_NAME).read_bytes()

    def common_arguments(self, output=None):
        arguments = ["--lease-dir", str(self.path), "--token-file", str(self.token_file),
                     "--owner-thread", self.request.owner_thread, "--run-id", self.request.run_id]
        if output is not None:
            arguments += ["--output", str(output)]
        return arguments

    def test_acquire_status_and_guarded_release_keep_private_audit_token(self):
        acquired = lease.acquire(self.request)
        verified = lease.status(self.request)
        token = self.token_file.read_text().strip()
        self.assertTrue(lease.TOKEN_PATTERN.fullmatch(token) is not None)
        self.assertEqual(acquired["lease_token_sha256"], verified["lease_token_sha256"])
        self.assertEqual(stat.S_IMODE(self.path.stat().st_mode), 0o700)
        self.assertEqual(stat.S_IMODE((self.path / lease.OWNER_NAME).stat().st_mode), 0o600)
        self.assertEqual(stat.S_IMODE(self.token_file.stat().st_mode), 0o600)
        released = self.guarded_release()
        self.assertTrue(released["lease_directory_absent_after_release"])
        self.assertFalse(self.path.exists())
        self.assertTrue(secrets.compare_digest(self.token_file.read_text().strip(), token),
                        "Release changed the private audit token.")

    def test_sixteen_concurrent_contenders_have_exactly_one_owner(self):
        barrier = Barrier(16)

        def contend(index):
            request = replace(self.request, token_file=self.private / ("actor-" + str(index)),
                              owner_thread="actor-" + str(index), run_id="run-" + str(index))
            barrier.wait(timeout=10)
            try:
                lease.acquire(request)
                return "acquired", request
            except lease.LeaseError as error:
                return error.code, request

        with ThreadPoolExecutor(max_workers=16) as pool:
            results = [future.result(timeout=20) for future in [pool.submit(contend, index) for index in range(16)]]
        winners = [request for outcome, request in results if outcome == "acquired"]
        self.assertEqual(len(winners), 1)
        self.assertTrue(all(outcome in ("acquired", "lease_exists", "operation_busy") for outcome, _ in results))
        self.assertEqual(sum(request.token_file.exists() for _, request in results), 1)
        self.assertTrue(lease.status(winners[0])["ownership_verified"])

    def test_existing_peer_directory_and_its_bytes_are_untouched(self):
        self.path.mkdir(mode=0o700)
        sentinel = self.path / "peer-owner"
        self.write_private(sentinel, "peer sentinel")
        self.assert_error(lambda: lease.acquire(self.request), "lease_exists")
        self.assertTrue(sentinel.read_text() == "peer sentinel")
        self.assertFalse(self.token_file.exists())

    def test_existing_actor_token_is_not_overwritten_or_removed(self):
        self.write_private(self.token_file, "reserved actor token\n")
        failure = self.assert_error(lambda: lease.acquire(self.request), "token_file_exists")
        self.assertTrue(failure.details["lease_directory_created"])
        self.assertTrue(self.token_file.read_text() == "reserved actor token\n")
        self.assertTrue(self.path.is_dir())
        self.assertFalse((self.path / lease.OWNER_NAME).exists())
        self.assert_error(lambda: lease.acquire(self.request), "lease_exists")

    def test_wrong_owner_and_run_never_remove_current_owner(self):
        lease.acquire(self.request)
        original = self.owner_bytes()
        for request in (replace(self.request, owner_thread="another-thread"),
                        replace(self.request, run_id="another-run")):
            with self.subTest(field=request.owner_thread + request.run_id):
                self.assert_error(lambda: self.guarded_release(request), "ownership_mismatch")
                self.assertTrue(self.owner_bytes() == original, "A mismatched actor changed the held lease.")

    def test_wrong_project_and_state_never_remove_owner(self):
        lease.acquire(self.request)
        owner_path = self.path / lease.OWNER_NAME
        original = json.loads(owner_path.read_text())
        for field, value in (("project", "WeRead"), ("state", "released")):
            with self.subTest(field=field):
                owner_path.write_text(json.dumps({**original, field: value}), encoding="utf-8")
                prior = owner_path.read_bytes()
                self.assert_error(self.guarded_release, "ownership_mismatch")
                self.assertTrue(owner_path.read_bytes() == prior)

    def test_wrong_expected_token_never_removes_owner(self):
        lease.acquire(self.request)
        original = self.owner_bytes()
        self.token_file.write_text("f" * 48 + "\n", encoding="ascii")
        self.assert_error(self.guarded_release, "token_mismatch")
        self.assertTrue(self.owner_bytes() == original)

    def test_recovery_guard_is_required_before_release_io(self):
        self.assert_error(lambda: lease.release(self.request), "recovery_guard_required")
        self.assertFalse(self.path.exists())
        lease.acquire(self.request)
        original = self.owner_bytes()
        self.assert_error(lambda: lease.release(self.request), "recovery_guard_required")
        self.assertTrue(self.owner_bytes() == original)

    def test_lease_symlink_is_not_followed(self):
        lease.acquire(self.request)
        parked = self.base / "parked"
        self.path.rename(parked)
        self.path.symlink_to(parked, target_is_directory=True)
        original = (parked / lease.OWNER_NAME).read_bytes()
        self.assert_error(lambda: lease.status(self.request), "unsafe_directory_link")
        self.assert_error(self.guarded_release, "unsafe_directory_link")
        self.assert_error(lambda: lease.acquire(self.request), "lease_exists")
        self.assertTrue((parked / lease.OWNER_NAME).read_bytes() == original)

    def test_symlink_in_parent_path_is_not_followed(self):
        alias = self.base / "alias"
        alias.symlink_to(self.private, target_is_directory=True)
        request = replace(self.request, token_file=alias / "expected-token")
        self.assert_error(lambda: lease.acquire(request), "unsafe_directory_link")
        self.assertFalse(self.path.exists())

    def test_owner_and_token_symlinks_are_rejected(self):
        lease.acquire(self.request)
        for path in (self.path / lease.OWNER_NAME, self.token_file):
            with self.subTest(kind=path.name):
                parked = path.with_name(path.name + ".original")
                path.rename(parked)
                path.symlink_to(parked)
                self.assert_error(lambda: lease.status(self.request), "unsafe_file_link")
                self.assert_error(self.guarded_release, "unsafe_file_link")
                path.unlink()
                parked.rename(path)

    def test_owner_and_token_hardlinks_are_rejected(self):
        lease.acquire(self.request)
        for index, path in enumerate((self.path / lease.OWNER_NAME, self.token_file)):
            with self.subTest(index=index):
                alias = self.private / ("hardlink-" + str(index))
                os.link(path, alias)
                original = self.owner_bytes()
                self.assert_error(lambda: lease.status(self.request), "unsafe_file_type")
                self.assert_error(self.guarded_release, "unsafe_file_type")
                self.assertTrue(self.owner_bytes() == original)
                alias.unlink()

    def test_lease_owner_and_token_permissions_are_enforced(self):
        lease.acquire(self.request)
        cases = ((self.path, 0o755, 0o700, "unsafe_directory_permissions"),
                 (self.path / lease.OWNER_NAME, 0o644, 0o600, "unsafe_file_permissions"),
                 (self.token_file, 0o640, 0o600, "unsafe_file_permissions"))
        for path, unsafe, restored, code in cases:
            with self.subTest(kind=path.name):
                path.chmod(unsafe)
                self.assert_error(lambda: lease.status(self.request), code)
                self.assert_error(self.guarded_release, code)
                path.chmod(restored)

    def test_actor_token_parent_must_be_private(self):
        self.private.chmod(0o755)
        self.assert_error(lambda: lease.acquire(self.request), "unsafe_directory_permissions")
        self.assertFalse(self.path.exists())

    def test_extra_lease_entries_block_release_without_cleanup(self):
        lease.acquire(self.request)
        original = self.owner_bytes()
        extra = self.path / "unexpected"
        self.write_private(extra, "keep this entry")
        self.assert_error(self.guarded_release, "unexpected_entries")
        self.assertTrue(self.owner_bytes() == original)
        self.assertTrue(extra.exists())

    def test_owner_replacement_before_unlink_is_detected(self):
        lease.acquire(self.request)
        owner_path = self.path / lease.OWNER_NAME
        original = self.owner_bytes()
        replacement = self.private / "replacement-owner"
        self.write_private(replacement, original)
        original_recheck = lease._OwnedLease.recheck
        calls = 0

        def replace_before_recheck(owned):
            nonlocal calls
            calls += 1
            if calls == 2:
                os.replace(replacement, owner_path)
            return original_recheck(owned)

        with mock.patch.object(lease._OwnedLease, "recheck", replace_before_recheck):
            failure = self.assert_error(self.guarded_release, "ownership_changed")
        self.assertFalse(failure.details["owner_record_removed"])
        self.assertTrue(owner_path.exists())

    def test_directory_aba_at_unlink_never_removes_replacement_peer(self):
        lease.acquire(self.request)
        parked = self.base / "detached-original"
        original_unlink = os.unlink
        peer_bytes = None
        swapped = False

        def swap_then_unlink(name, *, dir_fd=None):
            nonlocal peer_bytes, swapped
            if name == lease.OWNER_NAME and not swapped:
                swapped = True
                self.path.rename(parked)
                self.path.mkdir(mode=0o700)
                record = {"schema_version": 1, "project": "WeRead", "state": "held",
                          "owner_thread": "peer-thread", "run_id": "peer-run", "run_token": "a" * 48,
                          "acquired_at_utc": datetime.now(timezone.utc).isoformat()}
                peer_bytes = json.dumps(record).encode("utf-8")
                self.write_private(self.path / lease.OWNER_NAME, peer_bytes)
            return original_unlink(name, dir_fd=dir_fd)

        with mock.patch.object(lease.os, "unlink", side_effect=swap_then_unlink):
            failure = self.assert_error(self.guarded_release, "directory_replaced")
        self.assertTrue(failure.details["owner_record_removed"])
        self.assertFalse(failure.details["lease_directory_removed"])
        self.assertTrue((self.path / lease.OWNER_NAME).read_bytes() == peer_bytes,
                        "Release changed the replacement peer's ownership.")
        self.assertTrue(parked.is_dir())

    def test_rmdir_failure_retains_directory_and_reports_partial_release(self):
        lease.acquire(self.request)
        with mock.patch.object(lease.os, "rmdir", side_effect=PermissionError(errno.EACCES, "synthetic failure")):
            failure = self.assert_error(self.guarded_release, "filesystem_error")
        self.assertTrue(failure.details["owner_record_removed"])
        self.assertFalse(failure.details["lease_directory_removed"])
        self.assertTrue(self.path.is_dir())
        self.assertTrue(self.token_file.exists())
        self.assert_error(lambda: lease.acquire(replace(self.request, token_file=self.private / "new-token")), "lease_exists")

    def test_owner_write_failure_keeps_private_token_and_partial_directory(self):
        original_create = lease._create_file

        def fail_owner(entry, data, **options):
            if entry.name == lease.OWNER_NAME:
                raise OSError(errno.ENOSPC, "synthetic storage failure")
            return original_create(entry, data, **options)

        with mock.patch.object(lease, "_create_file", side_effect=fail_owner):
            failure = self.assert_error(lambda: lease.acquire(self.request), "filesystem_error")
        self.assertTrue(failure.details["private_token_created"])
        self.assertTrue(self.path.is_dir())
        self.assertTrue(self.token_file.exists())
        self.assertFalse((self.path / lease.OWNER_NAME).exists())

    def test_partial_token_write_failure_is_preserved_and_reported(self):
        with mock.patch.object(lease, "_write_all", side_effect=OSError(errno.ENOSPC, "synthetic storage failure")):
            failure = self.assert_error(lambda: lease.acquire(self.request), "filesystem_error")
        self.assertTrue(failure.details["lease_directory_created"])
        self.assertTrue(failure.details["private_token_created"])
        self.assertFalse(failure.details["owner_record_created"])
        self.assertTrue(self.token_file.is_file())
        self.assertTrue(self.path.is_dir())
        self.assertFalse((self.path / lease.OWNER_NAME).exists())

    def test_duplicate_owner_fields_are_rejected(self):
        lease.acquire(self.request)
        owner_path = self.path / lease.OWNER_NAME
        original = owner_path.read_text().rstrip()
        owner_path.write_text(original[:-1] + ', "state": "held"}\n', encoding="utf-8")
        self.assert_error(lambda: lease.status(self.request), "invalid_owner_record")
        self.assertTrue(owner_path.exists())

    def test_cli_public_stdout_and_output_never_include_raw_token(self):
        output = self.base / "acquired.json"
        stdout = io.StringIO()
        with redirect_stdout(stdout):
            result = lease.main(self.common_arguments(output) + ["acquire"])
        self.assertEqual(result, 0)
        token = self.token_file.read_text().strip()
        public = stdout.getvalue() + output.read_text()
        self.assertTrue(token not in public, "A private token reached public output.")
        self.assertTrue("run_token" not in public)
        self.assertEqual(json.loads(stdout.getvalue())["state"], "held")
        self.assertEqual(stat.S_IMODE(output.stat().st_mode), 0o600)
        wrong = self.common_arguments() + ["release", "--native-restored-and-connections-closed"]
        wrong[wrong.index("--owner-thread") + 1] = "wrong-thread"
        stdout = io.StringIO()
        with redirect_stdout(stdout):
            result = lease.main(wrong)
        self.assertEqual(result, 1)
        self.assertTrue(token not in stdout.getvalue(), "A failure published a private token.")

    def test_existing_public_output_is_not_overwritten_and_operation_does_not_start(self):
        output = self.base / "existing-output.json"
        self.write_private(output, "keep existing output")
        stdout = io.StringIO()
        with redirect_stdout(stdout):
            result = lease.main(self.common_arguments(output) + ["acquire"])
        self.assertEqual(result, 1)
        self.assertEqual(json.loads(stdout.getvalue())["code"], "output_exists")
        self.assertTrue(output.read_text() == "keep existing output")
        self.assertFalse(self.path.exists())

    def test_private_or_lease_output_locations_are_rejected_before_operation(self):
        for output in (self.token_file, self.path / "report.json"):
            with self.subTest(kind=output.name):
                stdout = io.StringIO()
                with redirect_stdout(stdout):
                    result = lease.main(self.common_arguments(output) + ["acquire"])
                self.assertEqual(result, 1)
                self.assertEqual(json.loads(stdout.getvalue())["code"], "unsafe_output_location")
                self.assertFalse(self.path.exists())

    def test_unexpected_error_text_is_not_published(self):
        marker = secrets.token_hex(24)
        stdout = io.StringIO()
        with mock.patch.object(lease, "acquire", side_effect=RuntimeError(marker)), redirect_stdout(stdout):
            result = lease.main(self.common_arguments() + ["acquire"])
        self.assertEqual(result, 1)
        self.assertTrue(marker not in stdout.getvalue(), "An unexpected error exposed private details.")
        self.assertEqual(json.loads(stdout.getvalue())["code"], "filesystem_or_internal_error")

    def test_traversal_relative_and_in_lease_token_paths_are_rejected(self):
        cases = ((replace(self.request, lease_dir=Path("relative.lock")), "invalid_path"),
                 (replace(self.request, token_file=self.private / ".." / "token"), "invalid_path"),
                 (replace(self.request, token_file=self.path / "private-token"), "unsafe_token_location"))
        for request, code in cases:
            with self.subTest(code=code):
                self.assert_error(lambda: lease.acquire(request), code)
                self.assertFalse(self.path.exists())


if __name__ == "__main__":
    unittest.main()
