#!/usr/bin/env python3
"""Manage a private host-side BiliComics device lease without network access.

Run this Linux-only coordinator utility on test-env. The lease token remains in
the private owner record and the separately supplied private token file. Public
results contain its SHA-256 only. A successful release does not delete the token
file, which remains a private audit input and cannot be reused by acquire.

Parent-directory advisory locks serialize callers of this utility. Descriptor
relative operations and repeated identity checks reject observed namespace
replacement. Cooperating same-user coordinators must never rename or replace an
existing lease. An administrator or a same-user process ignoring that protocol
can change files between POSIX pathname operations; this utility is not a
security boundary against such an actor. Failed operations retain partial state
instead of attempting recursive cleanup or recreating ownership records.
"""

from __future__ import annotations

import argparse
from contextlib import ExitStack, contextmanager
from dataclasses import dataclass
from datetime import datetime, timezone
import errno
import hashlib
import json
import os
from pathlib import Path
import re
import secrets
import stat
import sys

try:
    import fcntl
except ImportError:
    fcntl = None


PROJECT = "BiliComics"
OWNER_NAME = "owner.json"
TOKEN_PATTERN = re.compile(r"[0-9a-f]{48}")
OWNER_PATTERN = re.compile(r"[A-Za-z0-9][A-Za-z0-9._:-]{0,127}")
RUN_PATTERN = re.compile(r"[a-z0-9][a-z0-9-]{0,47}")
OWNER_FIELDS = frozenset(("schema_version", "project", "state", "owner_thread",
                          "run_id", "run_token", "acquired_at_utc"))


class LeaseError(Exception):
    """An expected failure whose message and details are safe to publish."""

    def __init__(self, code, message, **details):
        super().__init__(message)
        self.code = code
        self.message = message
        self.details = details


@dataclass(frozen=True)
class LeaseRequest:
    lease_dir: Path
    token_file: Path
    owner_thread: str
    run_id: str


def _require(condition, code, message):
    if not condition:
        raise LeaseError(code, message)


def _supported_host():
    _require(sys.platform.startswith("linux") and fcntl is not None
             and all(hasattr(os, name) for name in ("O_NOFOLLOW", "O_DIRECTORY", "O_CLOEXEC")),
             "unsupported_host", "Run the host lease utility on Linux test-env.")


def _absolute_path(value):
    raw = os.fspath(value)
    _require(isinstance(raw, str) and "\0" not in raw,
             "invalid_path", "An explicit absolute host path is required.")
    path = Path(raw)
    _require(path.is_absolute() and path.name not in ("", ".", "..")
             and ".." not in path.parts and len(path.parts) <= 128,
             "invalid_path", "Use an absolute host path without parent traversal.")
    return path


def _inside(path, directory):
    return path.parts[:len(directory.parts)] == directory.parts


def _request(value):
    _supported_host()
    lease_dir = _absolute_path(value.lease_dir)
    token_file = _absolute_path(value.token_file)
    _require(isinstance(value.owner_thread, str) and OWNER_PATTERN.fullmatch(value.owner_thread) is not None,
             "invalid_owner", "A bounded coordinator thread identifier is required.")
    _require(isinstance(value.run_id, str) and RUN_PATTERN.fullmatch(value.run_id) is not None,
             "invalid_run_id", "Use a lowercase run identifier of at most 48 characters.")
    _require(not _inside(token_file, lease_dir),
             "unsafe_token_location", "Keep the private token file outside the shared lease directory.")
    return LeaseRequest(lease_dir, token_file, value.owner_thread, value.run_id)


def _directory_identity(value):
    return value.st_dev, value.st_ino, value.st_uid, stat.S_IMODE(value.st_mode)


def _file_identity(value):
    return (value.st_dev, value.st_ino, value.st_uid, stat.S_IMODE(value.st_mode),
            value.st_nlink, value.st_size, value.st_mtime_ns, value.st_ctime_ns)


def _safe_directory(value, *, private=False):
    _require(stat.S_ISDIR(value.st_mode), "unsafe_directory", "An expected host directory is not a directory.")
    mode = stat.S_IMODE(value.st_mode)
    if private:
        _require(value.st_uid == os.geteuid() and mode == 0o700,
                 "unsafe_directory_permissions", "Private host directories must be owned by this user with mode 0700.")
    else:
        _require(value.st_uid in (0, os.geteuid()) and not mode & (stat.S_ISUID | stat.S_ISGID)
                 and (not mode & 0o022 or bool(mode & stat.S_ISVTX)),
                 "unsafe_directory_permissions", "A host path has unsafe directory ownership or permissions.")


def _safe_file(value):
    _require(stat.S_ISREG(value.st_mode) and value.st_nlink == 1,
             "unsafe_file_type", "Ownership and token inputs must be regular files without hard links.")
    _require(value.st_uid == os.geteuid() and stat.S_IMODE(value.st_mode) == 0o600,
             "unsafe_file_permissions", "Private host files must be owned by this user with mode 0600.")


@dataclass
class _Directory:
    fd: int
    identity: tuple
    parent: _Directory | None = None
    name: str | None = None
    private: bool = False

    def check(self):
        if self.parent is not None:
            self.parent.check()
        current = os.fstat(self.fd)
        _safe_directory(current, private=self.private)
        _require(_directory_identity(current) == self.identity,
                 "directory_changed", "A host directory identity or its permissions changed.")
        if self.parent is not None:
            try:
                linked = os.stat(self.name, dir_fd=self.parent.fd, follow_symlinks=False)
            except FileNotFoundError:
                raise LeaseError("directory_replaced", "An expected host directory was removed or replaced.") from None
            _require(stat.S_ISDIR(linked.st_mode) and _directory_identity(linked) == self.identity,
                     "directory_replaced", "An expected host directory was removed or replaced.")


@dataclass(frozen=True)
class _Entry:
    directory: _Directory
    name: str


def _open_directory(stack, *, parent=None, name=None, private=False):
    flags = os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC
    try:
        fd = os.open("/" if parent is None else name, flags,
                     **({} if parent is None else {"dir_fd": parent.fd}))
    except OSError as error:
        if error.errno in (errno.ELOOP, errno.ENOTDIR):
            raise LeaseError("unsafe_directory_link", "Directory paths must not contain links or non-directory entries.") from None
        if error.errno == errno.ENOENT:
            raise LeaseError("directory_missing", "A required host directory is absent.") from None
        raise
    stack.callback(os.close, fd)
    value = os.fstat(fd)
    _safe_directory(value, private=private)
    result = _Directory(fd, _directory_identity(value), parent, name, private)
    result.check()
    return result


def _open_parent(stack, path, *, private=False):
    current = _open_directory(stack)
    for component in path.parts[1:-1]:
        current = _open_directory(stack, parent=current, name=component)
    _safe_directory(os.fstat(current.fd), private=private)
    if private:
        current.private = True
    return _Entry(current, path.name)


@contextmanager
def _access(request):
    with ExitStack() as stack:
        lease_entry = _open_parent(stack, request.lease_dir)
        token_entry = _open_parent(stack, request.token_file, private=True)
        try:
            fcntl.flock(lease_entry.directory.fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            raise LeaseError("operation_busy", "Another host lease operation is in progress; retry after it finishes.") from None
        stack.callback(fcntl.flock, lease_entry.directory.fd, fcntl.LOCK_UN)
        lease_entry.directory.check()
        token_entry.directory.check()
        yield stack, lease_entry, token_entry


def _write_all(fd, data):
    remaining = memoryview(data)
    while remaining:
        count = os.write(fd, remaining)
        _require(count > 0, "short_write", "A private host file could not be completely written.")
        remaining = remaining[count:]


def _create_file(entry, data, *, exists_code, created=None):
    entry.directory.check()
    try:
        fd = os.open(entry.name, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW | os.O_CLOEXEC,
                     0o600, dir_fd=entry.directory.fd)
    except FileExistsError:
        raise LeaseError(exists_code, "An existing host file will not be overwritten or removed.") from None
    try:
        if created is not None:
            created()
        _safe_file(os.fstat(fd))
        _write_all(fd, data)
        os.fsync(fd)
        _safe_file(os.fstat(fd))
    finally:
        os.close(fd)
    entry.directory.check()


@dataclass(frozen=True)
class _FileSnapshot:
    data: bytes
    identity: tuple


def _read_file(entry, limit):
    entry.directory.check()
    try:
        fd = os.open(entry.name, os.O_RDONLY | os.O_NONBLOCK | os.O_NOFOLLOW | os.O_CLOEXEC,
                     dir_fd=entry.directory.fd)
    except OSError as error:
        if error.errno == errno.ELOOP:
            raise LeaseError("unsafe_file_link", "Ownership and token inputs must not be symbolic links.") from None
        if error.errno == errno.ENOENT:
            raise LeaseError("input_missing", "An expected private ownership or token input is absent.") from None
        raise
    try:
        before = os.fstat(fd)
        _safe_file(before)
        _require(before.st_size <= limit, "input_too_large", "A private ownership input exceeds its size bound.")
        chunks = []
        total = 0
        while total <= limit:
            chunk = os.read(fd, min(4096, limit + 1 - total))
            if not chunk:
                break
            chunks.append(chunk)
            total += len(chunk)
        _require(total <= limit, "input_too_large", "A private ownership input exceeds its size bound.")
        after = os.fstat(fd)
        _safe_file(after)
        linked = os.stat(entry.name, dir_fd=entry.directory.fd, follow_symlinks=False)
        _require(stat.S_ISREG(linked.st_mode) and _file_identity(before) == _file_identity(after)
                 == _file_identity(linked), "input_changed", "A private ownership input changed while being read.")
        return _FileSnapshot(b"".join(chunks), _file_identity(after))
    finally:
        os.close(fd)


def _unique_pairs(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError("Duplicate ownership field")
        result[key] = value
    return result


def _owner_record(data, request):
    try:
        record = json.loads(data.decode("utf-8"), object_pairs_hook=_unique_pairs)
    except (ValueError, UnicodeError):
        raise LeaseError("invalid_owner_record", "The private ownership record is not valid unambiguous JSON.") from None
    _require(isinstance(record, dict) and set(record) == OWNER_FIELDS
             and type(record["schema_version"]) is int and record["schema_version"] == 1,
             "invalid_owner_record", "The private ownership record has an unsupported schema.")
    _require(record["project"] == PROJECT and record["state"] == "held"
             and record["owner_thread"] == request.owner_thread and record["run_id"] == request.run_id,
             "ownership_mismatch", "The shared lease does not belong to this exact project, coordinator, state and run.")
    _require(isinstance(record["run_token"], str) and TOKEN_PATTERN.fullmatch(record["run_token"]) is not None,
             "invalid_owner_record", "The private ownership record has an invalid token format.")
    try:
        acquired = datetime.fromisoformat(record["acquired_at_utc"])
        _require(acquired.tzinfo is not None, "invalid_owner_record", "The ownership timestamp must include its timezone.")
    except (ValueError, TypeError):
        raise LeaseError("invalid_owner_record", "The private ownership record has an invalid timestamp.") from None
    return record


def _private_token(data):
    try:
        token = data.decode("ascii")
    except UnicodeError:
        raise LeaseError("invalid_token", "The private expected token has an invalid format.") from None
    if token.endswith("\n"):
        token = token[:-1]
    _require(TOKEN_PATTERN.fullmatch(token) is not None,
             "invalid_token", "The private expected token must contain 48 lowercase hexadecimal characters.")
    return token


@dataclass
class _OwnedLease:
    request: LeaseRequest
    directory: _Directory
    owner_entry: _Entry
    token_entry: _Entry
    owner: _FileSnapshot
    token: _FileSnapshot
    record: dict
    token_sha256: str

    def recheck(self):
        self.directory.check()
        owner = _read_file(self.owner_entry, 4096)
        token = _read_file(self.token_entry, 64)
        _require(owner.identity == self.owner.identity and token.identity == self.token.identity
                 and secrets.compare_digest(owner.data, self.owner.data)
                 and secrets.compare_digest(token.data, self.token.data),
                 "ownership_changed", "The ownership record or private expected token was replaced or changed.")
        self.directory.check()

    def public(self):
        return {"project": PROJECT, "owner_thread": self.request.owner_thread,
                "run_id": self.request.run_id, "lease_token_sha256": self.token_sha256}


def _owned(stack, request, lease_entry, token_entry, *, directory=None):
    if directory is None:
        directory = _open_directory(stack, parent=lease_entry.directory, name=lease_entry.name, private=True)
    owner_entry = _Entry(directory, OWNER_NAME)
    owner = _read_file(owner_entry, 4096)
    token = _read_file(token_entry, 64)
    record = _owner_record(owner.data, request)
    expected = _private_token(token.data)
    _require(secrets.compare_digest(record["run_token"], expected),
             "token_mismatch", "The private expected token does not match the shared lease.")
    result = _OwnedLease(request, directory, owner_entry, token_entry, owner, token, record,
                         hashlib.sha256(expected.encode("ascii")).hexdigest())
    result.recheck()
    return result


def acquire(request):
    """Atomically reserve a new lease; retain partial state on any failure."""
    request = _request(request)
    progress = {"lease_directory_created": False, "private_token_created": False,
                "owner_record_created": False}
    try:
        with _access(request) as (stack, lease_entry, token_entry):
            try:
                os.mkdir(lease_entry.name, 0o700, dir_fd=lease_entry.directory.fd)
            except FileExistsError:
                raise LeaseError("lease_exists", "The shared lease already exists; obtain a fresh handoff without replacing it.") from None
            progress["lease_directory_created"] = True
            directory = _open_directory(stack, parent=lease_entry.directory, name=lease_entry.name, private=True)
            _require(not os.listdir(directory.fd), "unexpected_entries", "A new lease directory contains unexpected entries.")
            token = secrets.token_hex(24)
            _create_file(token_entry, (token + "\n").encode("ascii"), exists_code="token_file_exists",
                         created=lambda: progress.update(private_token_created=True))
            record = {"schema_version": 1, "project": PROJECT, "state": "held",
                      "owner_thread": request.owner_thread, "run_id": request.run_id,
                      "run_token": token, "acquired_at_utc": datetime.now(timezone.utc).isoformat()}
            _create_file(_Entry(directory, OWNER_NAME),
                         (json.dumps(record, sort_keys=True) + "\n").encode("utf-8"), exists_code="owner_file_exists",
                         created=lambda: progress.update(owner_record_created=True))
            os.fsync(directory.fd)
            os.fsync(token_entry.directory.fd)
            os.fsync(lease_entry.directory.fd)
            owned = _owned(stack, request, lease_entry, token_entry, directory=directory)
            return {"ok": True, "operation": "acquire", "state": "held", **owned.public(),
                    "acquired_at_utc": record["acquired_at_utc"]}
    except LeaseError as error:
        error.details.update(progress)
        raise
    except OSError:
        raise LeaseError("filesystem_error", "The host lease filesystem operation failed; partial state is retained.", **progress) from None


def status(request):
    """Verify this exact held lease; absence or different ownership fails closed."""
    request = _request(request)
    try:
        with _access(request) as (stack, lease_entry, token_entry):
            owned = _owned(stack, request, lease_entry, token_entry)
            return {"ok": True, "operation": "status", "state": "held", "ownership_verified": True,
                    **owned.public()}
    except OSError:
        raise LeaseError("filesystem_error", "The host lease could not be safely inspected.") from None


def release(request, *, native_restored_and_connections_closed=False):
    """Remove only verified ownership after the coordinator's recovery guard."""
    _require(native_restored_and_connections_closed is True,
             "recovery_guard_required", "Restore native control and close coordinator device connections before release.")
    request = _request(request)
    progress = {"owner_record_removed": False, "lease_directory_removed": False}
    try:
        with _access(request) as (stack, lease_entry, token_entry):
            owned = _owned(stack, request, lease_entry, token_entry)
            _require(os.listdir(owned.directory.fd) == [OWNER_NAME],
                     "unexpected_entries", "The lease contains unexpected entries; no cleanup was attempted.")
            owned.recheck()
            os.unlink(OWNER_NAME, dir_fd=owned.directory.fd)
            progress["owner_record_removed"] = True
            owned.directory.check()
            _require(not os.listdir(owned.directory.fd),
                     "unexpected_entries", "The lease directory changed after removing this run's owner record.")
            token = _read_file(token_entry, 64)
            _require(token.identity == owned.token.identity and secrets.compare_digest(token.data, owned.token.data),
                     "ownership_changed", "The private expected token changed during release.")
            os.fsync(owned.directory.fd)
            owned.directory.check()
            os.rmdir(lease_entry.name, dir_fd=lease_entry.directory.fd)
            progress["lease_directory_removed"] = True
            _require(os.fstat(owned.directory.fd).st_nlink == 0,
                     "directory_replaced", "The removed pathname did not identify this run's open lease directory.")
            os.fsync(lease_entry.directory.fd)
            try:
                os.stat(lease_entry.name, dir_fd=lease_entry.directory.fd, follow_symlinks=False)
                absent = False
            except FileNotFoundError:
                absent = True
            return {"ok": True, "operation": "release", "state": "released", **owned.public(),
                    "released_at_utc": datetime.now(timezone.utc).isoformat(),
                    "owner_and_token_checked": True, "native_restored_and_connections_closed": True,
                    "only_owned_owner_json_removed_then_rmdir": True,
                    "lease_directory_absent_after_release": absent, "private_token_retained": True}
    except LeaseError as error:
        error.details.update(progress)
        raise
    except OSError:
        raise LeaseError("filesystem_error", "The host lease release was incomplete; retained state requires coordinator review.", **progress) from None


@contextmanager
def _output_file(path, request):
    if path is None:
        yield None
        return
    path = _absolute_path(path)
    _require(path != request.token_file and not _inside(path, request.lease_dir),
             "unsafe_output_location", "Public output must be separate from private token and lease inputs.")
    with ExitStack() as stack:
        entry = _open_parent(stack, path)
        try:
            fd = os.open(entry.name, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW | os.O_CLOEXEC,
                         0o600, dir_fd=entry.directory.fd)
        except FileExistsError:
            raise LeaseError("output_exists", "An existing public output file will not be overwritten.") from None
        stack.callback(os.close, fd)
        _safe_file(os.fstat(fd))
        yield fd
        entry.directory.check()
        _safe_file(os.fstat(fd))


def _failure(operation, error):
    return {"ok": False, "operation": operation, "code": error.code,
            "message": error.message, "details": error.details}


def arguments(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--lease-dir", type=Path, required=True)
    parser.add_argument("--token-file", type=Path, required=True)
    parser.add_argument("--owner-thread", required=True)
    parser.add_argument("--run-id", required=True)
    parser.add_argument("--output", type=Path, help="New sanitized JSON file; existing files are never replaced.")
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("acquire")
    commands.add_parser("status")
    release_parser = commands.add_parser("release")
    release_parser.add_argument("--native-restored-and-connections-closed", action="store_true",
                                help="Coordinator recovery guard; not an interactive approval request.")
    return parser.parse_args(argv)


def main(argv=None):
    args = arguments(argv)
    report = None
    try:
        request = _request(LeaseRequest(args.lease_dir, args.token_file, args.owner_thread, args.run_id))
        with _output_file(args.output, request) as output_fd:
            try:
                if args.command == "release":
                    report = release(request, native_restored_and_connections_closed=args.native_restored_and_connections_closed)
                else:
                    report = {"acquire": acquire, "status": status}[args.command](request)
            except LeaseError as error:
                report = _failure(args.command, error)
            if output_fd is not None:
                _write_all(output_fd, (json.dumps(report, indent=2, sort_keys=True) + "\n").encode("utf-8"))
                os.fsync(output_fd)
    except LeaseError as error:
        if report is None:
            report = _failure(args.command, error)
        else:
            report = {**report, "public_output_error": error.code}
    except Exception:
        if report is None:
            report = _failure(args.command, LeaseError("filesystem_or_internal_error", "The host lease operation failed without publishing private details."))
        else:
            report = {**report, "public_output_error": "filesystem_or_internal_error"}
    print(json.dumps(report, indent=2, sort_keys=True))
    return 0 if report.get("ok") and "public_output_error" not in report else 1


if __name__ == "__main__":
    raise SystemExit(main())
