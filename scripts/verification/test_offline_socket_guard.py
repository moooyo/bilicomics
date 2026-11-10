"""Remote-only tests for the standalone offline socket boundary.

Run only through ssh test-env with BILICOMICS_REMOTE_VERIFICATION=1. Provide
absolute, prebuilt native executables in BILICOMICS_OFFLINE_SOCKET_GUARD and
BILICOMICS_OFFLINE_SOCKET_TEST_HELPER. This suite never builds the application,
contacts the Kindle, takes a device lease, or connects/sends on a network socket.
Only AF_UNIX fixture sockets exchange synthetic payloads.
"""

from __future__ import annotations

import errno
import fcntl
import json
import os
from pathlib import Path
import platform
import select
import shutil
import socket
import stat
import subprocess
import sys
import tempfile
import unittest


SOURCE_PATH = Path(__file__).resolve()
SUCCESS_FIELDS = {
    "offline_socket_guard", "schema_version", "mode", "process_id", "uid", "gid",
    "effective_uid", "effective_gid", "no_new_privs", "seccomp_mode", "installed",
    "checks_passed", "audit_arch", "inherited_non_unix_closed",
    "inherited_unix_preserved", "inherited_non_socket_preserved",
    "denied_socket_mask", "denied_socketpair_mask", "unix_socket_allowed",
    "unix_socketpair_allowed", "socketcall_checked", "socketcall_denied",
    "x32_checked", "x32_denied",
}


def require_remote():
    if sys.platform != "linux" or os.environ.get("BILICOMICS_REMOTE_VERIFICATION") != "1":
        raise RuntimeError("Run socket guard fixtures only through ssh test-env")


def setUpModule():
    require_remote()


def process_state():
    status = {}
    for line in Path("/proc/self/status").read_text().splitlines():
        key, separator, value = line.partition(":")
        if separator:
            status[key] = value.strip()
    namespaces = {}
    for name in ("net", "ipc", "mnt", "user", "uts", "pid", "cgroup"):
        path = Path("/proc/self/ns") / name
        if path.exists():
            namespaces[name] = path.stat().st_ino
    return {
        "pid": os.getpid(), "uid": os.getuid(), "gid": os.getgid(),
        "effective_uid": os.geteuid(), "effective_gid": os.getegid(),
        "no_new_privs": int(status["NoNewPrivs"]),
        "seccomp_mode": int(status["Seccomp"]), "namespaces": namespaces,
    }


def socket_checks():
    attempts = [
        (socket.AF_UNSPEC, socket.SOCK_STREAM),
        (socket.AF_INET, socket.SOCK_STREAM),
        (socket.AF_INET6, socket.SOCK_STREAM),
        (socket.AF_PACKET, socket.SOCK_DGRAM),
        (socket.AF_NETLINK, socket.SOCK_RAW),
        (socket.AF_VSOCK, socket.SOCK_STREAM),
        (255, socket.SOCK_STREAM),
    ]
    result = []
    for family, kind in attempts:
        row = {"family": int(family)}
        for operation in ("socket", "socketpair"):
            try:
                value = (socket.socket(family, kind, 0) if operation == "socket"
                         else socket.socketpair(family, kind, 0))
            except OSError as error:
                row[operation] = error.errno
            else:
                for descriptor in ((value,) if operation == "socket" else value):
                    descriptor.close()
                row[operation] = 0
        result.append(row)
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM):
        pass
    left, right = socket.socketpair(socket.AF_UNIX, socket.SOCK_STREAM)
    with left, right:
        left.settimeout(2)
        right.settimeout(2)
        left.sendall(b"unix-pair-fixture")
        pair_ok = right.recv(64) == b"unix-pair-fixture"
    return {"attempts": result, "unix_pair_ok": pair_ok, "state": process_state()}


def descriptor_state(fd):
    try:
        mode = os.fstat(fd).st_mode
    except OSError as error:
        return {"errno": error.errno}
    if stat.S_ISSOCK(mode):
        value = socket.socket(fileno=fd)
        try:
            domain = value.getsockopt(socket.SOL_SOCKET, socket.SO_DOMAIN)
        finally:
            value.detach()
        return {"kind": "socket", "domain": domain}
    if stat.S_ISFIFO(mode):
        return {"kind": "fifo"}
    if stat.S_ISREG(mode):
        return {"kind": "file"}
    return {"kind": "other"}


def child_main(arguments):
    require_remote()
    mode, *arguments = arguments
    if mode == "state":
        result = process_state()
    elif mode == "sockets":
        result = socket_checks()
    elif mode == "mark":
        Path(arguments[0]).write_text("executed\n")
        return 0
    elif mode == "hold-state":
        proof_stat = Path(arguments[0]).stat()
        proof_fds = 0
        for name in os.listdir("/proc/self/fd"):
            try:
                value = os.fstat(int(name))
            except OSError as error:
                if error.errno == errno.EBADF:
                    continue
                raise
            proof_fds += (value.st_dev, value.st_ino) == (proof_stat.st_dev, proof_stat.st_ino)
        print(json.dumps({"state": process_state(), "proof_fds": proof_fds}), flush=True)
        sys.stdin.read(1)
        return 0
    elif mode == "fds":
        specification = json.loads(arguments[0])
        # Inspect before opening any output file could reuse a closed FD.
        result = {
            "closed": {str(fd): descriptor_state(fd) for fd in specification["closed"]},
            "preserved": {str(fd): descriptor_state(fd)
                          for fd in specification.get("preserved", [])},
        }
        if "file" in specification:
            result["file_payload"] = os.read(specification["file"], 64).decode("ascii")
        if "fifo" in specification:
            result["fifo_payload"] = os.read(specification["fifo"], 64).decode("ascii")
        if "unix" in specification:
            value = socket.socket(fileno=specification["unix"])
            value.settimeout(2)
            result["unix_payload"] = value.recv(64).decode("ascii")
            value.sendall(b"unix-fixture-reply")
            value.detach()
        if "output" in specification:
            Path(specification["output"]).write_text(json.dumps(result) + "\n")
            return 0
    elif mode == "unix-connect":
        kind, address = arguments
        if kind == "abstract":
            address = "\0" + address
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as value:
            value.settimeout(3)
            value.connect(address)
            value.sendall(b"unix-fixture-request")
            result = {"reply": value.recv(64).decode("ascii")}
    elif mode == "fork-exec":
        read_fd, write_fd = os.pipe()
        child = os.fork()
        if child == 0:
            os.close(read_fd)
            os.dup2(write_fd, 1)
            os.close(write_fd)
            os.execv(sys.executable,
                     [sys.executable, str(SOURCE_PATH), "--child", "sockets"])
        os.close(write_fd)
        with os.fdopen(read_fd, "r") as stream:
            descendant = json.load(stream)
        _, status = os.waitpid(child, 0)
        result = {"state": process_state(), "forked_pid": child,
                  "exit_code": os.waitstatus_to_exitcode(status), "descendant": descendant}
    else:
        raise RuntimeError("Unknown socket guard child mode")
    print(json.dumps(result))
    return 0


class OfflineSocketGuardTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.guard = cls.executable("BILICOMICS_OFFLINE_SOCKET_GUARD")
        cls.launcher = cls.executable("BILICOMICS_OFFLINE_SOCKET_TEST_HELPER")

    @staticmethod
    def executable(variable):
        value = os.environ.get(variable, "")
        path = Path(value)
        if not value or not path.is_absolute() or not path.is_file() or not os.access(path, os.X_OK):
            raise RuntimeError(f"Set {variable} to a prebuilt absolute native executable")
        return str(path)

    def command(self, *arguments, deny_install=False, launcher_mode=None):
        mode = launcher_mode or ("--deny-install" if deny_install else "--allow-install")
        return [self.launcher, mode,
                self.guard, *map(str, arguments)]

    def child_command(self, mode, *arguments, deny_install=False, proof_file=None,
                      launcher_mode=None):
        prefix = [] if proof_file is None else ["--proof-file", proof_file]
        return self.command(*prefix, "--", sys.executable, SOURCE_PATH, "--child", mode,
                            *arguments, deny_install=deny_install, launcher_mode=launcher_mode)

    def run_command(self, command, **options):
        return subprocess.run(command, capture_output=True, text=True, timeout=12,
                              check=False, **options)

    def proof(self, text, mode, expected_nnp=1):
        lines = text.splitlines()
        self.assertEqual(len(lines), 1, text)
        value = json.loads(lines[0])
        self.assertEqual(set(value), SUCCESS_FIELDS)
        self.assertTrue(all(type(item) is int for item in value.values()))
        expected = {
            "offline_socket_guard": 1, "schema_version": 1, "mode": mode,
            "installed": 1, "checks_passed": 1, "seccomp_mode": 2,
            "no_new_privs": expected_nnp, "denied_socket_mask": 63,
            "denied_socketpair_mask": 63, "unix_socket_allowed": 1,
            "unix_socketpair_allowed": 1, "uid": os.getuid(), "gid": os.getgid(),
            "effective_uid": os.geteuid(), "effective_gid": os.getegid(),
        }
        if platform.machine() == "x86_64":
            expected.update(audit_arch=0xC000003E, socketcall_checked=0,
                            socketcall_denied=0, x32_checked=1, x32_denied=1)
        else:
            expected.update(audit_arch=0x40000028, socketcall_checked=1,
                            socketcall_denied=1, x32_checked=0, x32_denied=0)
        for name, item in expected.items():
            self.assertEqual(value[name], item, name)
        self.assertGreater(value["process_id"], 0)
        for name in ("inherited_non_unix_closed", "inherited_unix_preserved",
                     "inherited_non_socket_preserved"):
            self.assertGreaterEqual(value[name], 0, name)
        return value

    def assert_socket_results(self, value):
        self.assertEqual(len(value["attempts"]), 7)
        for row in value["attempts"]:
            self.assertEqual(row["socket"], errno.EPERM, row)
            self.assertEqual(row["socketpair"], errno.EPERM, row)
        self.assertTrue(value["unix_pair_ok"])
        self.assertEqual(value["state"]["seccomp_mode"], 2)
        self.assertEqual(value["state"]["no_new_privs"], 1)

    def test_probe_reports_actual_family_checks(self):
        result = self.run_command(self.command("--probe"))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stderr, "")
        self.proof(result.stdout, 1)

    def test_exec_preserves_pid_credentials_and_namespaces(self):
        baseline = process_state()
        process = subprocess.Popen(self.child_command("state"), stdout=subprocess.PIPE,
                                   stderr=subprocess.PIPE, text=True)
        stdout, stderr = process.communicate(timeout=12)
        self.assertEqual(process.returncode, 0, stderr)
        proof = self.proof(stderr, 2)
        child = json.loads(stdout)
        self.assertEqual(proof["process_id"], process.pid)
        self.assertEqual(child["pid"], process.pid)
        self.assertEqual(child["no_new_privs"], 1)
        self.assertEqual(child["seccomp_mode"], 2)
        for name in ("uid", "gid", "effective_uid", "effective_gid", "namespaces"):
            self.assertEqual(child[name], baseline[name], name)

    def test_proof_file_is_readable_while_head_buffers_a_live_target_stderr(self):
        head = shutil.which("head")
        self.assertIsNotNone(head, "The remote regression fixture requires head")
        with tempfile.TemporaryDirectory(prefix="offline-socket-buffer-") as temporary:
            proof_path = Path(temporary) / "guard-proof.json"
            application_log = Path(temporary) / "application.log"
            process = None
            with application_log.open("wb") as log:
                sink = subprocess.Popen([head, "-c", "1048576"], stdin=subprocess.PIPE,
                                        stdout=log, stderr=subprocess.DEVNULL)
                try:
                    process = subprocess.Popen(
                        self.child_command("hold-state", proof_path, proof_file=proof_path),
                        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=sink.stdin,
                        text=True, umask=0o077)
                    sink.stdin.close()
                    sink.stdin = None
                    self.assertTrue(select.select([process.stdout], [], [], 5)[0],
                                    "The target did not reach its live fixture handshake")
                    child = json.loads(process.stdout.readline())
                    self.assertIsNone(process.poll())
                    self.assertIsNone(sink.poll())
                    proof_text = proof_path.read_text()
                    proof = self.proof(proof_text, 2)
                    self.assertEqual(proof["process_id"], process.pid)
                    self.assertEqual(child["state"]["pid"], process.pid)
                    self.assertEqual(child["proof_fds"], 0)
                    metadata = proof_path.stat()
                    self.assertTrue(stat.S_ISREG(metadata.st_mode))
                    self.assertEqual(metadata.st_nlink, 1)
                    self.assertEqual(stat.S_IMODE(metadata.st_mode), 0o600)
                    self.assertEqual(application_log.read_bytes(), b"")
                    stdout, _ = process.communicate(input="q", timeout=5)
                    self.assertEqual(process.returncode, 0)
                    self.assertEqual(stdout, "")
                    self.assertEqual(sink.wait(timeout=5), 0)
                    self.assertEqual(application_log.read_text(), proof_text)
                finally:
                    if process is not None and process.poll() is None:
                        process.kill()
                        process.communicate(timeout=5)
                    if sink.poll() is None:
                        sink.kill()
                    sink.wait(timeout=5)

    def test_existing_or_symlink_proof_leaves_are_preserved_without_exec(self):
        with tempfile.TemporaryDirectory(prefix="offline-socket-existing-") as temporary:
            root = Path(temporary)
            marker = root / "executed"
            existing = root / "existing.json"
            existing.write_bytes(b"existing-proof-fixture\n")
            symlink = root / "linked.json"
            symlink.symlink_to(existing)
            dangling = root / "dangling.json"
            missing = root / "missing.json"
            dangling.symlink_to(missing)
            for path in (existing, symlink, dangling):
                with self.subTest(path=path.name):
                    before = path.lstat()
                    result = self.run_command(self.child_command("mark", marker, proof_file=path))
                    self.assertEqual(result.returncode, 125)
                    self.assertEqual(result.stdout, "")
                    failure = json.loads(result.stderr)
                    self.assertEqual(failure["error_stage"], 7)
                    self.assertIn(failure["error_errno"], (errno.EEXIST, errno.ELOOP))
                    after = path.lstat()
                    self.assertEqual((after.st_ino, after.st_mode, after.st_size),
                                     (before.st_ino, before.st_mode, before.st_size))
                    self.assertFalse(marker.exists())
                    self.assertEqual(existing.read_bytes(), b"existing-proof-fixture\n")
                    self.assertFalse(missing.exists())
            self.assertTrue(symlink.is_symlink())
            self.assertTrue(dangling.is_symlink())

    def test_invalid_proof_arguments_and_uncreatable_paths_prevent_exec(self):
        with tempfile.TemporaryDirectory(prefix="offline-socket-path-") as temporary:
            root = Path(temporary)
            marker = root / "executed"
            proof_path = root / "proof.json"
            target = [sys.executable, str(SOURCE_PATH), "--child", "mark", str(marker)]
            invalid = [
                ["--proof-file"], ["--proof-file", str(proof_path)],
                ["--proof-file", "", "--", *target],
                ["--proof-file", "relative.json", "--", *target],
                ["--proof-file", str(proof_path), "--probe"],
                ["--probe", "--proof-file", str(proof_path)],
                ["--proof-file", str(proof_path), "--", "relative-target"],
            ]
            for arguments in invalid:
                with self.subTest(arguments=arguments[:3]):
                    result = self.run_command(self.command(*arguments))
                    self.assertEqual(result.returncode, 2)
                    self.assertEqual(json.loads(result.stderr)["error_stage"], 1)
                    self.assertFalse(marker.exists())
                    self.assertFalse(proof_path.exists())
            parent_file = root / "parent-file"
            parent_file.write_bytes(b"parent-fixture")
            for path in (root / "missing-parent" / "proof.json", parent_file / "proof.json"):
                with self.subTest(path=str(path)):
                    result = self.run_command(self.child_command("mark", marker, proof_file=path))
                    self.assertEqual(result.returncode, 125)
                    self.assertEqual(json.loads(result.stderr)["error_stage"], 7)
                    self.assertFalse(marker.exists())
            self.assertEqual(parent_file.read_bytes(), b"parent-fixture")

    def test_proof_write_or_fsync_failure_prevents_exec_and_preserves_evidence(self):
        cases = (("--limit-proof-file", errno.EFBIG), ("--deny-proof-sync", errno.EIO))
        for launcher_mode, expected_errno in cases:
            with self.subTest(launcher_mode=launcher_mode), \
                    tempfile.TemporaryDirectory(prefix="offline-socket-proof-failure-") as temporary:
                root = Path(temporary)
                marker = root / "executed"
                proof_path = root / "proof.json"
                result = self.run_command(self.child_command(
                    "mark", marker, proof_file=proof_path, launcher_mode=launcher_mode))
                self.assertEqual(result.returncode, 125)
                self.assertEqual(result.stdout, "")
                self.assertEqual(json.loads(result.stderr), {
                    "offline_socket_guard": 1, "schema_version": 1,
                    "error_stage": 7, "error_errno": expected_errno,
                })
                self.assertFalse(marker.exists())
                self.assertTrue(proof_path.is_file())
                if launcher_mode == "--limit-proof-file":
                    self.assertEqual(proof_path.read_bytes(), b"")
                else:
                    self.proof(proof_path.read_text(), 2)

    def test_fork_and_exec_inherit_the_restriction(self):
        result = self.run_command(self.child_command("fork-exec"))
        self.assertEqual(result.returncode, 0, result.stderr)
        proof = self.proof(result.stderr, 2)
        value = json.loads(result.stdout)
        self.assertEqual(value["exit_code"], 0)
        self.assertEqual(value["state"]["pid"], proof["process_id"])
        self.assertNotEqual(value["forked_pid"], proof["process_id"])
        self.assertEqual(value["descendant"]["state"]["pid"], value["forked_pid"])
        self.assertEqual(value["descendant"]["state"]["namespaces"],
                         value["state"]["namespaces"])
        self.assert_socket_results(value["descendant"])

    def test_foreign_descriptors_and_stdin_close_but_local_descriptors_survive(self):
        left, right = socket.socketpair()
        read_fd, write_fd = os.pipe()
        high_fd = None
        try:
            with left, right, socket.socket(socket.AF_INET, socket.SOCK_STREAM) as foreign, \
                    tempfile.TemporaryFile() as document:
                high_fd = fcntl.fcntl(foreign.fileno(), fcntl.F_DUPFD, 200)
                document.write(b"file-fixture")
                document.seek(0)
                os.write(write_fd, b"fifo-fixture")
                left.settimeout(2)
                left.sendall(b"unix-fixture-request")
                preserved = [document.fileno(), read_fd, right.fileno()]
                specification = {
                    "closed": [0, high_fd], "preserved": preserved,
                    "file": document.fileno(), "fifo": read_fd, "unix": right.fileno(),
                }
                result = self.run_command(self.child_command("fds", json.dumps(specification)),
                                          stdin=foreign.fileno(),
                                          pass_fds=(*preserved, high_fd))
                self.assertEqual(result.returncode, 0, result.stderr)
                proof = self.proof(result.stderr, 2)
                self.assertGreaterEqual(proof["inherited_non_unix_closed"], 2)
                self.assertGreaterEqual(proof["inherited_unix_preserved"], 1)
                self.assertGreaterEqual(proof["inherited_non_socket_preserved"], 4)
                value = json.loads(result.stdout)
                self.assertEqual(value["closed"],
                                 {str(fd): {"errno": errno.EBADF} for fd in (0, high_fd)})
                self.assertEqual(value["preserved"][str(document.fileno())], {"kind": "file"})
                self.assertEqual(value["preserved"][str(read_fd)], {"kind": "fifo"})
                self.assertEqual(value["preserved"][str(right.fileno())],
                                 {"kind": "socket", "domain": socket.AF_UNIX})
                self.assertEqual(value["file_payload"], "file-fixture")
                self.assertEqual(value["fifo_payload"], "fifo-fixture")
                self.assertEqual(value["unix_payload"], "unix-fixture-request")
                self.assertEqual(left.recv(64), b"unix-fixture-reply")
        finally:
            os.close(read_fd)
            os.close(write_fd)
            if high_fd is not None:
                os.close(high_fd)

    def test_foreign_stdout_is_closed_before_exec(self):
        with tempfile.TemporaryDirectory(prefix="offline-socket-stdout-") as temporary, \
                socket.socket(socket.AF_INET6, socket.SOCK_DGRAM) as foreign:
            output = Path(temporary) / "result.json"
            specification = {"closed": [1], "output": str(output)}
            result = subprocess.run(self.child_command("fds", json.dumps(specification)),
                                    stdout=foreign.fileno(), stderr=subprocess.PIPE,
                                    text=True, timeout=12, check=False)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertGreaterEqual(self.proof(result.stderr, 2)["inherited_non_unix_closed"], 1)
            self.assertEqual(json.loads(output.read_text())["closed"],
                             {"1": {"errno": errno.EBADF}})

    def test_foreign_stderr_is_closed_and_missing_proof_prevents_exec(self):
        with tempfile.TemporaryDirectory(prefix="offline-socket-stderr-") as temporary, \
                socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as foreign:
            marker = Path(temporary) / "executed"
            result = subprocess.run(self.child_command("mark", marker),
                                    stdout=subprocess.PIPE, stderr=foreign.fileno(),
                                    text=True, timeout=12, check=False)
            self.assertEqual(result.returncode, 125)
            self.assertEqual(result.stdout, "")
            self.assertFalse(marker.exists())

    def test_pathname_and_abstract_unix_ipc_cross_the_guard_boundary(self):
        for kind in ("pathname", "abstract"):
            with self.subTest(kind=kind), \
                    tempfile.TemporaryDirectory(prefix="offline-socket-ipc-") as temporary, \
                    socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as listener:
                address = (str(Path(temporary) / "ipc.sock") if kind == "pathname"
                           else Path(temporary).name)
                listener.bind(address if kind == "pathname" else "\0" + address)
                listener.listen(1)
                listener.settimeout(5)
                process = subprocess.Popen(self.child_command("unix-connect", kind, address),
                                           stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                           text=True)
                try:
                    connection, _ = listener.accept()
                    with connection:
                        connection.settimeout(3)
                        self.assertEqual(connection.recv(64), b"unix-fixture-request")
                        connection.sendall(b"unix-fixture-reply")
                    stdout, stderr = process.communicate(timeout=12)
                    self.assertEqual(process.returncode, 0, stderr)
                    self.proof(stderr, 2)
                    self.assertEqual(json.loads(stdout), {"reply": "unix-fixture-reply"})
                finally:
                    if process.poll() is None:
                        process.kill()
                        process.communicate(timeout=5)

    def test_failed_install_never_executes_even_with_preexisting_seccomp(self):
        with tempfile.TemporaryDirectory(prefix="offline-socket-install-") as temporary:
            marker = Path(temporary) / "executed"
            result = self.run_command(self.child_command("mark", marker, deny_install=True))
            self.assertEqual(result.returncode, 125)
            self.assertEqual(result.stdout, "")
            self.assertEqual(json.loads(result.stderr), {
                "offline_socket_guard": 1, "schema_version": 1,
                "error_stage": 3, "error_errno": errno.EPERM,
            })
            self.assertFalse(marker.exists())

    def test_invalid_arguments_never_execute_a_target(self):
        with tempfile.TemporaryDirectory(prefix="offline-socket-arguments-") as temporary:
            marker = Path(temporary) / "executed"
            target = [sys.executable, str(SOURCE_PATH), "--child", "mark", str(marker)]
            invalid = [[], ["--"], ["--", "relative"], ["--", ""], ["--unknown"],
                       ["--probe", *target], target]
            for arguments in invalid:
                with self.subTest(arguments=arguments[:2]):
                    result = self.run_command(self.command(*arguments))
                    self.assertEqual(result.returncode, 2, result.stderr)
                    self.assertEqual(result.stdout, "")
                    self.assertEqual(json.loads(result.stderr)["error_stage"], 1)
                    self.assertFalse(marker.exists())

    def test_missing_exec_target_is_reported_after_filter_proof(self):
        with tempfile.TemporaryDirectory(prefix="offline-socket-missing-") as temporary:
            target = Path(temporary) / "missing"
            result = self.run_command(self.command("--", target))
            self.assertEqual(result.returncode, 127)
            self.assertEqual(result.stdout, "")
            lines = result.stderr.splitlines()
            self.assertEqual(len(lines), 2)
            self.proof(lines[0], 2)
            self.assertEqual(json.loads(lines[1]), {
                "offline_socket_guard": 1, "schema_version": 1,
                "error_stage": 6, "error_errno": errno.ENOENT,
            })

    def test_direct_install_preserves_inherited_no_new_privs_or_fails_closed(self):
        inherited = process_state()["no_new_privs"]
        result = self.run_command([self.guard, "--probe"])
        if result.returncode == 0:
            self.assertEqual(result.stderr, "")
            self.proof(result.stdout, 1, expected_nnp=inherited)
        else:
            self.assertEqual(result.returncode, 125)
            self.assertEqual(result.stdout, "")
            failure = json.loads(result.stderr)
            self.assertEqual(failure["error_stage"], 3)
            self.assertIn(failure["error_errno"], (errno.EACCES, errno.EPERM))

    @unittest.skipUnless(platform.machine() == "x86_64", "Native x86_64 ABI fixture")
    def test_x32_and_compat_arch_are_rejected_without_breaking_native_getuid(self):
        result = self.run_command(self.command("--", self.launcher, "--check-abi"))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.proof(result.stderr, 2)
        self.assertEqual(json.loads(result.stdout), {
            "x32_denied": 1, "compat_arch_killed": 1, "native_getuid_allowed": 1,
        })


if __name__ == "__main__":
    require_remote()
    if len(sys.argv) > 2 and sys.argv[1] == "--child":
        raise SystemExit(child_main(sys.argv[2:]))
    unittest.main()
