#!/usr/bin/env python3
"""Prepare or execute an isolated startup-sequence A/B batch.

Preparation is host-only. Execute only on test-env, after reviewing preparation
and acquiring no other device resource. One fresh aggregate lease covers all
cases; every case has a fresh nonce and independent data and recovery records.
Authentication references stay in a private host configuration.
"""

from __future__ import annotations

import argparse
from contextlib import contextmanager
from datetime import datetime, timezone
import hashlib
import io
import json
import os
from pathlib import Path, PurePosixPath
import re
import secrets
import shlex
import shutil
import signal
import stat
import subprocess
import sys
import tarfile
import time
import zipfile


LEASE_DIRECTORY = "/tmp/codex-kindle-physical-test.lock"
PRIMARY_VARIANTS = ("baseline", "candidate")
VARIANTS = PRIMARY_VARIANTS
STARTUP_ARGUMENTS = {
    "baseline": ["--demo", "--offline", "--serial-startup-preparation"],
    "candidate": ["--demo", "--offline"],
}
SHARED_IDENTITY_FIELDS = ("source_commit", "source_fingerprint", "binary_sha256", "worker_sha256",
                          "launcher_sha256", "package_sha256", "compiler_fingerprint", "build_configuration")
PRIMARY_ROUNDS = 6
CASE_LIMIT_SECONDS = 45
RESTORE_LIMIT_SECONDS = 25
q = shlex.quote
CONTROL_SIGNALS = (signal.SIGINT, signal.SIGTERM, signal.SIGHUP)
ACTIVE_CANCELLATION = None


class BatchCancelled(RuntimeError):
    pass


class Cancellation:
    def __init__(self):
        self.cancelled = False
        self.recovering = False
        self.received = []
        self.previous = {}
        self.defer_depth = 0
        self.pending = False

    def handle(self, number, frame):
        self.received.append(number)
        if self.cancelled or self.recovering:
            return
        if self.defer_depth:
            self.pending = True
            return
        self.cancelled = True
        raise BatchCancelled("The startup driver received a cancellation signal")

    def __enter__(self):
        global ACTIVE_CANCELLATION
        require(ACTIVE_CANCELLATION is None, "Cancellation handlers are already installed")
        ACTIVE_CANCELLATION = self
        for number in CONTROL_SIGNALS:
            self.previous[number] = signal.signal(number, self.handle)
        return self

    def __exit__(self, kind, value, trace):
        global ACTIVE_CANCELLATION
        for number, previous in self.previous.items():
            signal.signal(number, previous)
        ACTIVE_CANCELLATION = None


@contextmanager
def defer_control_signals():
    # Delay only the parent's Python exception. OS masks remain unchanged so
    # newly exec'd transport children retain normal TERM/HUP behavior.
    control = ACTIVE_CANCELLATION
    if control is not None:
        control.defer_depth += 1
    try:
        yield
    finally:
        if control is not None:
            control.defer_depth -= 1
            if not control.defer_depth and control.pending and not control.cancelled and not control.recovering:
                control.pending = False
                control.cancelled = True
                raise BatchCancelled("The startup driver received a deferred cancellation signal")


def require(value, message):
    if not value:
        raise RuntimeError(message)


def sha(content):
    return hashlib.sha256(content).hexdigest()


def regular(path):
    path = Path(path)
    metadata = path.lstat()
    require(stat.S_ISREG(metadata.st_mode) and not path.is_symlink() and metadata.st_nlink == 1,
            "A host input is not an unlinked regular file: " + path.name)
    return path.read_bytes()


def new_file(path, content, mode=0o600):
    content = content.encode() if isinstance(content, str) else content
    fd = os.open(path, os.O_CREAT | os.O_EXCL | os.O_WRONLY | os.O_NOFOLLOW, mode)
    with os.fdopen(fd, "wb") as stream:
        stream.write(content)
    Path(path).chmod(mode)


def json_new(path, value):
    new_file(path, json.dumps(value, indent=2, sort_keys=True) + "\n")


def sha_value(value):
    require(isinstance(value, str) and re.fullmatch(r"[0-9a-f]{64}", value), "An exact SHA-256 is required")
    return value


def checked_replace(text, before, after):
    require(text.count(before) == 1, "A launcher instrumentation anchor is missing or ambiguous: " + before[:90])
    return text.replace(before, after, 1)


PROC_FUNCTIONS = r'''
proc_read() {
    case "$1" in ''|*[!0-9]*) return 1;; esac
    IFS= read -r proc_line <"/proc/$1/stat" || return 1
    [ "${proc_line%% *}" = "$1" ] || return 1
    proc_tail=${proc_line##*) }
    [ "$proc_tail" != "$proc_line" ] || return 1
    set -- $proc_tail
    [ "$#" -ge 20 ] || return 1
    proc_state=$1; proc_parent=$2; proc_group=$3; proc_session=$4
    case "$7" in ''|*[!0-9]*) return 1;; esac
    proc_flags=$7
    shift 19
    case "$1" in ''|*[!0-9]*) return 1;; esac
    proc_birth=$1
}
same_birth() { proc_read "$1" 2>/dev/null && [ "$proc_birth" = "$2" ]; }
has_case_token() {
    case_process=$1
    [ -d "/proc/$case_process" ] || return 1
    if ! proc_read "$case_process" 2>/dev/null; then
        [ ! -d "/proc/$case_process" ] && return 1
        return 2
    fi
    # A verified exited task has no live descriptor or worker ownership.
    case "$proc_state" in Z|X) return 1;; esac
    # Linux PF_KTHREAD (stat field 9) proves this is a kernel task rather
    # than an unobservable userspace application or codec worker.
    [ $((proc_flags & 2097152)) -eq 0 ] || return 1
    [ -r "/proc/$case_process/environ" ] || return 2
    if case_environment=$(tr '\000' '\n' <"/proc/$case_process/environ" 2>/dev/null); then
        if printf '%s\n' "$case_environment" | grep -Fx "BILICOMICS_STARTUP_CASE_TOKEN=$token" >/dev/null; then
            return 0
        else
            token_grep_status=$?
            [ "$token_grep_status" -eq 1 ] && return 1
            return 2
        fi
    else
        [ ! -d "/proc/$case_process" ] && return 1
        if proc_read "$case_process" 2>/dev/null; then
            case "$proc_state" in Z|X) return 1;; esac
        fi
        return 2
    fi
}
now_seconds() {
    IFS=' ' read -r now_uptime now_unused </proc/uptime
    now_value=${now_uptime%%.*}
}
'''


SUPERVISOR = r'''#!/bin/sh
# An independent per-case watchdog owns only its direct production launcher.
set -eu
umask 077
stage=$1
runtime=$2
case_name=$3
token=$4
run_id=$5
case "$stage" in /mnt/us/bilicomics-startup-sequence-*) ;; *) exit 2;; esac
case "$runtime" in /tmp/bilicomics-startup-sequence-*) ;; *) exit 2;; esac
case "$case_name" in baseline-warmup|candidate-warmup|baseline-0[1-6]|candidate-0[1-6]) ;; *) exit 2;; esac
case "$token" in ''|*[!0-9a-f]*) exit 2;; esac
[ "${#token}" -eq 64 ] || exit 2
[ ! -L "$stage" ] && [ "$(cat "$stage/owner")" = "$run_id" ] || exit 2
[ ! -L "$runtime" ] && [ "$(cat "$runtime/owner")" = "$run_id" ] || exit 2
out=$stage/out/$case_name
[ "$(cat "$out/owner")" = "$token" ] || exit 2
variant=${case_name%%-*}
application=$stage/$variant/app/bilicomics
launcher=$stage/$variant/launch.sh
lock=/tmp/bilicomics-launcher.lock
export BILICOMICS_STARTUP_CASE_TOKEN=$token
export BILICOMICS_STARTUP_BATCH=$run_id
''' + PROC_FUNCTIONS + r'''
proc_read $$
printf '%s %s\n' "$$" "$proc_birth" >"$out/supervisor.pid"
launcher_pid=''
launcher_birth=''
lock_identity=''
lock_nonce=''
lock_generation=''
launcher_status=255
launcher_reaped=0
content_seen=0
cleanup_started=0
fallback_attempted=0
lock_hard_failure=0
lock_tail_pending=0
lock_tail_drained=0
lock_tail_deadline_cs=''
reason=unexpected-exit

mark() {
    IFS=' ' read -r mark_clock mark_unused </proc/uptime
    printf '%s %s\n' "$mark_clock" "$1" >>"$out/shell.tsv"
}
owned_launcher() {
    [ -n "$launcher_pid" ] && [ -n "$launcher_birth" ] || return 1
    same_birth "$launcher_pid" "$launcher_birth" && [ "$proc_parent" = "$$" ] || return 1
    has_case_token "$launcher_pid" || return 1
    tr '\000' '\n' <"/proc/$launcher_pid/cmdline" | grep -Fx "$launcher" >/dev/null
}
lock_clock() {
    IFS=' ' read -r lock_uptime lock_unused </proc/uptime || return 1
    lock_whole=${lock_uptime%%.*}; lock_fraction=${lock_uptime#*.}
    case "$lock_whole:$lock_fraction" in ''|*[!0-9:]*|:*|*:) return 1;; esac
    [ "${#lock_fraction}" -eq 2 ] || return 1
    lock_fraction=${lock_fraction#0}; lock_fraction=${lock_fraction:-0}
    lock_now_cs=$((lock_whole * 100 + lock_fraction))
}
lock_note() {
    IFS=' ' read -r lock_stamp lock_unused </proc/uptime || return 1
    # Never emit raw nonce, generation or capability contents.
    printf '%s\t%s\t%s\n' "$lock_stamp" "$1" "$2" >>"$out/lock-observation.tsv"
}
lock_foreign() {
    lock_hard_failure=1
    lock_note foreign "$1" || :
    return 1
}
lock_indeterminate() {
    # Creation can legitimately precede publication of the first anchors.
    [ -n "$lock_identity" ] || return 0
    lock_note indeterminate "$1" || :
    return 2
}
lock_leaf_type() {
    [ ! -L "$lock/$1" ] || { lock_foreign "$1-symlink"; return 1; }
    if [ ! -f "$lock/$1" ]; then
        if [ -e "$lock/$1" ] || [ -L "$lock/$1" ]; then
            lock_foreign "$1-type"; return 1
        fi
        lock_indeterminate "$1-absent"; return $?
    fi
}
lock_anchor_matches() {
    if lock_leaf_type "$1"; then :; else return $?; fi
    if cmp -s "$lock/$1" "$out/lock-expected-$1"; then
        return 0
    else
        lock_cmp_status=$?
        if [ "$lock_cmp_status" -eq 1 ]; then lock_foreign "$1-bytes"; return 1; fi
        lock_indeterminate "$1-read-failed"; return $?
    fi
}
observe_lock() {
    # 0: a coherent owned observation (or initial publication is pending).
    # 1: affirmative foreign identity/type/bytes; this failure is sticky.
    # 2: a bound namespace/anchor is absent or unreadable, never proof of exit.
    [ ! -L "$lock" ] || { lock_foreign namespace-symlink; return 1; }
    if [ ! -d "$lock" ]; then
        if [ -e "$lock" ] || [ -L "$lock" ]; then lock_foreign namespace-type; return 1; fi
        lock_indeterminate namespace-absent; return $?
    fi
    if ! lock_current_identity=$(stat -c '%d:%i' "$lock" 2>/dev/null); then
        lock_indeterminate namespace-stat-failed; return $?
    fi
    if [ -n "$lock_identity" ]; then
        [ "$lock_current_identity" = "$lock_identity" ] || { lock_foreign namespace-inode; return 1; }
        for lock_anchor in pid nonce generation; do
            if lock_anchor_matches "$lock_anchor"; then :; else return $?; fi
        done
    else
        for lock_anchor in pid nonce generation; do
            # Missing creation anchors retain the original startup wait.
            [ -e "$lock/$lock_anchor" ] || [ -L "$lock/$lock_anchor" ] || return 0
            if lock_leaf_type "$lock_anchor"; then :; else return $?; fi
        done
        if ! lock_initial_pid=$(cat "$lock/pid" 2>/dev/null); then return 0; fi
        [ -n "$lock_initial_pid" ] || return 0
        [ "$lock_initial_pid" = "$launcher_pid" ] || { lock_foreign pid-bytes; return 1; }
        if ! nonce=$(cat "$lock/nonce" 2>/dev/null); then return 0; fi
        if ! generation=$(cat "$lock/generation" 2>/dev/null); then return 0; fi
        [ -n "$nonce" ] && [ -n "$generation" ] || return 0
        printf '%s\n' "$launcher_pid" >"$out/lock-expected-pid"
        printf '%s\n' "$nonce" >"$out/lock-expected-nonce"
        printf '%s\n' "$generation" >"$out/lock-expected-generation"
        for lock_anchor in pid nonce generation; do
            if cmp -s "$lock/$lock_anchor" "$out/lock-expected-$lock_anchor"; then :; else
                lock_cmp_status=$?
                [ "$lock_cmp_status" -ne 1 ] || { lock_foreign "$lock_anchor-bytes"; return 1; }
                return 0
            fi
        done
    fi
    [ ! -L "$lock" ] || { lock_foreign namespace-symlink; return 1; }
    if ! lock_after_identity=$(stat -c '%d:%i' "$lock" 2>/dev/null); then
        lock_indeterminate namespace-stat-failed; return $?
    fi
    [ "$lock_after_identity" = "$lock_current_identity" ] || { lock_foreign namespace-inode; return 1; }
    if [ -z "$lock_identity" ]; then
        lock_identity=$lock_current_identity; lock_nonce=$nonce; lock_generation=$generation
        printf '%s %s %s\n' "$lock_identity" "$lock_nonce" "$lock_generation" >"$out/launcher-lock.identity"
        lock_note owned initial-bound || return 2
    fi
    return 0
}
verify_lock_tail_exit() {
    [ "$lock_tail_pending" -eq 1 ] || return 0
    lock_clock || return 1
    [ "$lock_now_cs" -le "$lock_tail_deadline_cs" ] || return 1
    [ ! -e "$lock" ] && [ ! -L "$lock" ] || return 1
    lock_tail_drained=1
    lock_note retired verified-child-exit-and-namespace-absent || return 1
}
snapshot_owned() {
    for process in /proc/[0-9]*; do
        pid=${process##*/}
        [ "$pid" != "$$" ] || continue
        if has_case_token "$pid" 2>/dev/null && proc_read "$pid" 2>/dev/null; then
            exe=$(readlink "$process/exe" 2>/dev/null || :)
            printf '%s %s %s %s %s %s\n' "$pid" "$proc_birth" "$proc_parent" "$proc_group" "$proc_session" "$exe" >>"$out/owned-processes.tsv"
        fi
    done
}
observed_application_is_live() {
    same_birth "$application_pid" "$application_birth" || return 1
    case "$proc_state" in Z|X) return 1;; esac
    [ "$(readlink "/proc/$application_pid/exe" 2>/dev/null || :)" = "$application" ] || return 1
    has_case_token "$application_pid" || return 1
    same_birth "$application_pid" "$application_birth" || return 1
    case "$proc_state" in Z|X) return 1;; esac
}
native_restored() {
    [ ! -e "$lock" ] && [ ! -L "$lock" ] || return 1
    fallback_statusbar_is_running || { printf 'statusbar-unproven\n' >>"$out/native-proof-failures.tsv"; return 1; }
    while IFS=' ' read -r pid birth exe; do
        [ -n "$pid" ] || continue
        same_birth "$pid" "$birth" || return 1
        case "$proc_state" in S|R) ;; *) return 1;; esac
        [ "$(readlink "/proc/$pid/exe" 2>/dev/null || :)" = "$exe" ] || return 1
    done <"$out/native-before.tsv"
}
owned_gone() {
    [ -f "$out/owned-processes.tsv" ] && [ ! -L "$out/owned-processes.tsv" ] && [ -r "$out/owned-processes.tsv" ] || return 1
    while IFS=' ' read -r pid birth unused; do
        [ -n "$pid" ] || continue
        if [ -d "/proc/$pid" ]; then
            if proc_read "$pid" 2>/dev/null; then
                [ "$proc_birth" != "$birth" ] || { printf '%s recorded-birth-present\n' "$pid" >>"$out/process-closure-failures.tsv"; return 1; }
            else
                [ ! -d "/proc/$pid" ] || { printf '%s recorded-stat-unreadable\n' "$pid" >>"$out/process-closure-failures.tsv"; return 1; }
            fi
        fi
    done <"$out/owned-processes.tsv"
    for process in /proc/[0-9]*; do
        pid=${process##*/}
        [ "$pid" != "$$" ] || continue
        if has_case_token "$pid" 2>/dev/null; then
            printf '%s current-case-token\n' "$pid" >>"$out/process-closure-failures.tsv"
            return 1
        else
            token_status=$?
            [ "$token_status" -eq 1 ] || [ ! -d "$process" ] || { printf '%s owner-unreadable\n' "$pid" >>"$out/process-closure-failures.tsv"; return 1; }
        fi
    done
}
cleanup() {
    [ "$cleanup_started" -eq 0 ] || return
    cleanup_started=1
    trap - 0
    trap '' HUP INT TERM
    mark cleanup_begin
    # Do not signal a worker, DMA helper, unknown descendant, or reused PID.
    if owned_launcher; then
        snapshot_owned
        kill -TERM "$launcher_pid" 2>/dev/null || :
    fi
    now_seconds; deadline=$((now_value + 25))
    while owned_launcher; do
        now_seconds
        [ "$now_value" -lt "$deadline" ] || break
        usleep 100000
    done
    launcher_exited=0
    if [ -n "$launcher_pid" ] && [ -n "$launcher_birth" ] && [ "$launcher_reaped" -eq 0 ]; then
        if [ -d "/proc/$launcher_pid" ]; then
            if proc_read "$launcher_pid" 2>/dev/null; then
                if [ "$proc_birth" != "$launcher_birth" ]; then launcher_exited=1; fi
                case "$proc_state" in Z|X) launcher_exited=1;; esac
            elif [ ! -d "/proc/$launcher_pid" ]; then
                launcher_exited=1
            fi
        else
            launcher_exited=1
        fi
        if [ "$launcher_exited" -eq 1 ]; then
            if wait "$launcher_pid"; then launcher_status=0; else launcher_status=$?; fi
            launcher_reaped=1
        fi
    fi
    restored=false
    processes=false
    while :; do
        restored=false
        processes=false
        if native_restored; then restored=true; fi
        if owned_gone; then processes=true; fi
        if [ "$restored:$processes:$launcher_reaped:$fallback_attempted" = false:true:1:0 ] && { [ -e "$lock" ] || [ -L "$lock" ]; }; then
            fallback_attempted=1
            fallback_restore_native || :
            restored=false
            processes=false
            if native_restored; then restored=true; fi
            if owned_gone; then processes=true; fi
        fi
        [ "$restored:$processes" != true:true ] || break
        now_seconds
        [ "$now_value" -lt "$deadline" ] || break
        usleep 100000
    done
    restored=false
    processes=false
    if native_restored; then restored=true; fi
    if owned_gone; then processes=true; fi
    passed=false
    [ "$restored:$processes:$launcher_status:$reason:$content_seen:$fallback_attempted:$lock_hard_failure" != true:true:0:normal-exit:1:0:0 ] || passed=true
    if [ "$lock_tail_pending" -eq 1 ] && [ "$lock_tail_drained" -ne 1 ]; then passed=false; fi
    absent=false
    [ -e "$lock" ] || [ -L "$lock" ] || absent=true
    mark case_finished
    printf '{"passed":%s,"native_restored":%s,"owned_processes_gone":%s,"launcher_exit_status":%s,"production_lock_absent":%s,"content_observed":%s,"fallback_attempted":%s,"lock_hard_failure":%s,"lock_tail_pending":%s,"lock_tail_drained":%s,"reason":"%s"}\n' \
        "$passed" "$restored" "$processes" "$launcher_status" "$absent" "$content_seen" "$fallback_attempted" "$lock_hard_failure" "$lock_tail_pending" "$lock_tail_drained" "$reason" >"$out/recovery.json.pending"
    mv "$out/recovery.json.pending" "$out/recovery.json"
}
trap cleanup 0
trap 'reason=supervisor-signal; exit 143' HUP INT TERM
: >"$out/owned-processes.tsv"
mark document_entry
export BILICOMICS_HOME=$stage/$variant
export BILICOMICS_RESEARCH_DATA=$stage/cases/$case_name/data
export BILICOMICS_RESEARCH_OUT=$out
export BILICOMICS_LAUNCH_TRACE=1
export SLINT_KINDLE_INPUT_DIAGNOSTICS=1
DISPLAY=:0.0 sh "$launcher" --detached >>"$out/launcher.log" 2>&1 &
launcher_pid=$!
proc_read "$launcher_pid"
[ "$proc_parent" = "$$" ] || exit 3
launcher_birth=$proc_birth
printf '%s %s\n' "$launcher_pid" "$launcher_birth" >"$out/launcher.pid"
now_seconds; deadline=$((now_value + 45))
while :; do
    if ! proc_read "$launcher_pid" 2>/dev/null; then
        [ ! -d "/proc/$launcher_pid" ] && break
        reason=launcher-identity-unreadable
        exit 7
    fi
    [ "$proc_birth" = "$launcher_birth" ] || break
    case "$proc_state" in Z|X) break;; esac
    if observe_lock; then :; else
        lock_observation_status=$?
        if [ "$lock_observation_status" -eq 1 ]; then reason=lock-ownership-changed; exit 4; fi
        [ "$lock_observation_status" -eq 2 ] || { reason=lock-observation-failed; exit 4; }
        if [ "$lock_tail_pending" -eq 0 ]; then
            lock_clock || { reason=lock-clock-unreadable; exit 4; }
            lock_tail_pending=1
            lock_tail_deadline_cs=$((lock_now_cs + 200))
            lock_note retirement-pending bounded-two-second-drain || :
        fi
    fi
    if [ "$lock_tail_pending" -eq 1 ]; then
        # Never resume ordinary sampling after a bound anchor disappeared.
        # Only the already recorded direct child's verified exit can drain it.
        lock_clock || { reason=lock-clock-unreadable; exit 4; }
        [ "$lock_now_cs" -lt "$lock_tail_deadline_cs" ] || { reason=lock-retirement-unproven; exit 4; }
        usleep 20000
        continue
    fi
    if [ "$content_seen" -eq 0 ] && [ -f "$out/performance.jsonl" ] &&
        awk '/"kind":"startup"/ && /"stage":"ui_ready"/ { ready=1 } ready && /"kind":"native"/ && /"stage":"frame"/ && /"refresh_accepted":true/ && /"rendered":true/ { found=1 } END { exit !found }' "$out/performance.jsonl"; then
        content_seen=1
        mark content_observed
        application_pid=$(cat "$lock/application-pid" 2>/dev/null || :)
        case "$application_pid" in ''|*[!0-9]*) reason=application-identity-missing; exit 5;; esac
        [ "$(readlink "/proc/$application_pid/exe")" = "$application" ] || { reason=application-image-changed; exit 5; }
        has_case_token "$application_pid" || { reason=application-token-changed; exit 5; }
        proc_read "$application_pid" || { reason=application-birth-unreadable; exit 5; }
        application_birth=$proc_birth
        case "$case_name" in
            baseline-06|candidate-06)
                observed_application_is_live || { reason=application-ended-before-early-capture; exit 5; }
                "$stage/tools/startup-clock" "$runtime/$case_name/early-request.json" -- \
                    "$stage/tools/physical-framebuffer" --capture >"$out/content-frame.raw"
                "$stage/tools/startup-clock" "$runtime/$case_name/early-complete.json" -- /bin/true
                observed_application_is_live || { reason=application-ended-during-early-capture; exit 5; }
                cp "$runtime/$case_name/early-request.json" "$out/early-request.json"
                cp "$runtime/$case_name/early-complete.json" "$out/early-complete.json"
                ;;
        esac
        first_business_row=$(awk '/"kind":"startup"/ && /"stage":"ui_ready"/ { ready=1 } ready && /"kind":"native"/ && /"stage":"frame"/ && /"refresh_accepted":true/ && /"rendered":true/ { print; exit }' "$out/performance.jsonl")
        first_send_ns=$(printf '%s\n' "$first_business_row" | sed -n 's/.*"refresh_submit":{"finished_monotonic_ns":\([0-9][0-9]*\),.*/\1/p')
        case "$first_send_ns" in ''|*[!0-9]*) reason=business-clock-missing; exit 6;; esac
        settled_ns=$((first_send_ns + 1500000000))
        printf '%s\n' "$settled_ns" >"$out/post-business-target-ns.txt"
        case "$case_name" in
            baseline-06|candidate-06)
                observed_application_is_live || { reason=application-ended-before-scheduled-capture; exit 5; }
                printf '%s\n' "$settled_ns" >"$out/settled-deadline-ns.txt"
                "$stage/tools/startup-clock" --wait-until "$settled_ns" "$runtime/$case_name/settled-request.json" -- \
                    "$stage/tools/physical-framebuffer" --capture >"$out/settled-frame.raw"
                "$stage/tools/startup-clock" "$runtime/$case_name/settled-complete.json" -- /bin/true
                observed_application_is_live || { reason=application-ended-during-scheduled-capture; exit 5; }
                cp "$runtime/$case_name/settled-request.json" "$out/settled-request.json"
                cp "$runtime/$case_name/settled-complete.json" "$out/settled-complete.json"
                cp "$runtime/$case_name/settled-request.json" "$out/post-business-gate.json"
                printf '{"schema_version":1,"application_pid":%s,"start_ticks":%s,"early_before":true,"early_after":true,"settled_before":true,"settled_after":true}\n' \
                    "$application_pid" "$application_birth" >"$out/capture-liveness.json"
                ;;
            *)
                "$stage/tools/startup-clock" --wait-until "$settled_ns" "$runtime/$case_name/post-business-gate.json" -- /bin/true
                cp "$runtime/$case_name/post-business-gate.json" "$out/post-business-gate.json"
                ;;
        esac
        # All comparisons cross the same gate before heavy procfs sampling. Owner
        # discovery cannot delay the early or scheduled framebuffer capture.
        mark proc_observation_begin
        [ "$(readlink "/proc/$application_pid/exe")" = "$application" ] || { reason=application-ended-before-observation; exit 5; }
        has_case_token "$application_pid" || { reason=application-token-changed; exit 5; }
        sed -n '/^VmRSS:/p; /^VmHWM:/p; /^RssAnon:/p; /^RssFile:/p; /^RssShmem:/p' "/proc/$application_pid/status" >"$out/memory-at-content.txt"
        : >"$out/input-fds-at-content.txt"
        for descriptor in /proc/$application_pid/fd/*; do
            target=$(readlink "$descriptor" 2>/dev/null || :)
            case "$target" in /dev/input/event*) printf '%s %s\n' "${descriptor##*/}" "$target" >>"$out/input-fds-at-content.txt";; esac
        done
        snapshot_owned
    fi
    now_seconds
    [ "$now_value" -lt "$deadline" ] || { reason=case-timeout; exit 124; }
    usleep 100000
done
verify_lock_tail_exit || { reason=lock-retirement-unproven; exit 4; }
if wait "$launcher_pid"; then launcher_status=0; else launcher_status=$?; fi
launcher_reaped=1
reason=normal-exit
cleanup
exit 0
'''

NATIVE_RECOVERY = regular(Path(__file__).with_name("native_recovery_fragment.sh")).decode().replace("\r\n", "\n")
SUPERVISOR = checked_replace(SUPERVISOR, "cleanup() {\n", NATIVE_RECOVERY + "\ncleanup() {\n")


def instrument_launcher(content, variant):
    require(variant in PRIMARY_VARIANTS, "Use a primary startup variant")
    launcher = content.decode("utf-8").replace("\r\n", "\n")
    launcher = checked_replace(launcher, "DATA_DIR=$BILICOMICS_HOME/data", "DATA_DIR=${BILICOMICS_RESEARCH_DATA:?}")
    command = '"$APP_DIR/bilicomics" --kindle --device "$PROFILE" --data-dir "$DATA_DIR"'
    replacement = command + " " + " ".join(STARTUP_ARGUMENTS[variant]) + (' --screen shelf'
        ' --performance-trace "$BILICOMICS_RESEARCH_OUT/performance.jsonl"'
        ' --lifecycle-trace "$BILICOMICS_RESEARCH_OUT/lifecycle.jsonl"'
        ' --capture "$BILICOMICS_RESEARCH_OUT/final.png" --exit-after-ms 6500')
    launcher = checked_replace(launcher, command, replacement)
    # Keep both production validation passes and its return-before-takeover
    # behavior. Only the detached target gains an independent watchdog parent.
    target = ('sh "$BILICOMICS_RESEARCH_STAGE/supervise.sh" "$BILICOMICS_RESEARCH_STAGE"'
              ' "$BILICOMICS_RESEARCH_RUNTIME" "$BILICOMICS_RESEARCH_CASE"'
              ' "$BILICOMICS_STARTUP_CASE_TOKEN" "$BILICOMICS_STARTUP_BATCH"'
              ' >>"$BILICOMICS_RESEARCH_OUT/supervisor.log" 2>&1 </dev/null &')
    launcher = checked_replace(launcher, 'setsid sh "$SCRIPT_PATH" --detached >/dev/null 2>&1 </dev/null &', 'setsid ' + target)
    launcher = checked_replace(launcher, 'nohup sh "$SCRIPT_PATH" --detached >/dev/null 2>&1 </dev/null &', 'nohup ' + target)
    # Existing numeric production stage markers remain enabled for all variants.
    # The additional marker is immediately before the exact UI exec request.
    launcher = checked_replace(launcher, "    runner_child=$!\n", "    runner_child=$!\n    launch_trace research_application_started\n")
    return launcher.encode()


def package_variant(record):
    require(not any(field in record for field in ("native_query_sha256", "deployed_native_query_sha256",
                "diagnostic_capture_instrumented")), "Legacy helper or diagnostic identities are prohibited")
    package = regular(record["package"])
    require(sha(package) == sha_value(record["package_sha256"]), "A source-bound package identity differs")
    require(re.fullmatch(r"[0-9a-f]{40}", record["source_commit"]), "An exact source commit is required")
    sha_value(record["source_fingerprint"])
    sha_value(record["compiler_fingerprint"])
    require(isinstance(record.get("build_configuration"), dict) and record["build_configuration"],
            "Freeze the compiler, target and release build configuration")
    members = {}
    with zipfile.ZipFile(io.BytesIO(package)) as archive:
        names = archive.namelist()
        require(len(names) == len(set(names)), "Duplicate package names")
        for name in ("bilicomics/app/bilicomics", "bilicomics/app/kindle-jpeg-worker", "bilicomics/launch.sh"):
            info = archive.getinfo(name)
            require(not info.is_dir() and stat.S_IFMT(info.external_attr >> 16) in (0, stat.S_IFREG), "Unsafe package entry")
            members[name.removeprefix("bilicomics/")] = archive.read(name)
        require("bilicomics/app/bilicomics-native-query" not in names,
                "The startup-sequence package must contain no native-query helper")
    for name in ("app/bilicomics", "app/kindle-jpeg-worker"):
        require(members[name][:7] == b"\x7fELF\x01\x01\x01" and members[name][18:20] == b"(\0", "A packaged executable is not ARM32")
    observed = {"binary_sha256": sha(members["app/bilicomics"]), "worker_sha256": sha(members["app/kindle-jpeg-worker"]),
                "launcher_sha256": sha(members["launch.sh"])}
    for name, value in observed.items():
        require(value == sha_value(record[name]), "A declared package member identity differs: " + name)
    return members, {**record, **observed, "binary_bytes": len(members["app/bilicomics"]),
                     "package_entry_count": len(names)}


def prepare(config_path, root):
    require(sys.platform == "linux", "Prepare on test-env")
    config = json.loads(regular(config_path))
    require(root.is_absolute() and root.parent == Path("/root") and not os.path.lexists(root), "Use a fresh /root batch directory")
    run_id = config.get("run_id") or "startup-sequence-20261117-" + secrets.token_hex(4)
    require(re.fullmatch(r"startup-sequence-[0-9]{8}-[0-9a-f]{8}", run_id), "Use a fresh startup-sequence batch identity")
    require(re.fullmatch(r"[0-9a-f-]{36}", config["owner_thread"]), "An exact owner thread identity is required")
    require(config.get("lease_dir", LEASE_DIRECTORY) == LEASE_DIRECTORY, "Use the shared Kindle lease")
    require(config["profile"] == "scribe" and re.fullmatch(r"[0-9.]{3,24}", config["firmware"]), "This plan is scoped to the supervised Scribe")
    require(config["fixture_scope"] == "isolated-demo-offline", "Use an isolated demonstration fixture")
    root.mkdir(mode=0o700)
    for relative in ("private", "evidence", "inputs", "tools"):
        (root / relative).mkdir(mode=0o700)
    stage, runtime = "/mnt/us/bilicomics-" + run_id, "/tmp/bilicomics-" + run_id
    require(set(config["variants"]) == set(PRIMARY_VARIANTS), "Supply exactly the two primary source-bound launch variants")
    cases = [name + "-warmup" for name in PRIMARY_VARIANTS]
    for number in range(1, PRIMARY_ROUNDS + 1):
        cases.extend(name + f"-{number:02d}" for name in (PRIMARY_VARIANTS if number % 2 else PRIMARY_VARIANTS[::-1]))
    case_metadata = {name: {"software_variant": name.split("-", 1)[0],
        "measurement_role": "warmup" if name.endswith("-warmup") else "primary"} for name in cases}
    records, members = {}, {}
    primary_packages = {name: package_variant(config["variants"][name]) for name in PRIMARY_VARIANTS}
    for field in SHARED_IDENTITY_FIELDS:
        require(primary_packages["baseline"][1][field] == primary_packages["candidate"][1][field],
                "The shared source/package/compiler identity differs: " + field)
    for name in VARIANTS:
        inputs, original = primary_packages[name]
        require(original.get("startup_arguments") == STARTUP_ARGUMENTS[name],
                "Freeze exactly the approved startup arguments: " + name)
        record = dict(original)
        records[name] = record
        for relative, content in inputs.items():
            members[name + "/" + relative] = (instrument_launcher(content, name) if relative == "launch.sh" else content)
        record["instrumented_launcher_sha256"] = sha(members[name + "/launch.sh"])
    serial_flag = b" --serial-startup-preparation"
    baseline_launcher, candidate_launcher = members["baseline/launch.sh"], members["candidate/launch.sh"]
    require(baseline_launcher.count(serial_flag) == 1 and serial_flag not in candidate_launcher
            and baseline_launcher.replace(serial_flag, b"", 1) == candidate_launcher,
            "The instrumented launchers may differ only by the serial preparation flag")
    for name in ("startup-clock", "physical-framebuffer"):
        helper = regular(config[name.replace("-", "_")])
        require(sha(helper) == sha_value(config[name.replace("-", "_") + "_sha256"]), "A helper identity differs")
        require(helper[:7] == b"\x7fELF\x01\x01\x01" and helper[18:20] == b"(\0", "A helper is not ARM32")
        members["tools/" + name] = helper
    fixture = Path(config["fixture_data"]).resolve(strict=True)
    require(not Path(config["fixture_data"]).is_symlink(), "The fixture root cannot be a link")
    require(fixture.is_dir() and not fixture.is_symlink(), "The sealed host fixture is missing")
    fixture_manifest = []
    for path in sorted(fixture.rglob("*")):
        require(not path.is_symlink(), "A fixture link is prohibited")
        if path.is_dir():
            continue
        relative = path.relative_to(fixture).as_posix()
        require(not relative.endswith(("-wal", "-shm")) and relative.split("/")[0] not in ("logs", "sessions"), "Use a closed demonstration snapshot without runtime logs")
        content = regular(path)
        members["fixture/data/" + relative] = content
        fixture_manifest.append({"path": relative, "bytes": len(content), "sha256": sha(content)})
    manifest_bytes = (json.dumps(fixture_manifest, sort_keys=True, separators=(",", ":")) + "\n").encode()
    require(sha(manifest_bytes) == sha_value(config["fixture_fingerprint"]), "The sealed fixture manifest differs")
    members["fixture/data/device-profile"] = (config["profile"] + "\n").encode()
    members["fixture/data/device-allowlist"] = (config["profile"] + " " + config["firmware"] + "\n").encode()
    members["supervise.sh"] = SUPERVISOR.encode()
    entry = f'''#!/bin/sh
# Name: BiliComics startup comparison
# DontUseFBInk
set -eu
stage={q(stage)}
runtime={q(runtime)}
run_id={q(run_id)}
[ ! -L "$stage" ] && [ "$(cat "$stage/owner")" = "$run_id" ] || exit 2
IFS=' ' read -r case_name token <"$runtime/dispatch"
case "$case_name" in {'|'.join(cases)}) ;; *) exit 2;; esac
variant=${{case_name%%-*}}
BILICOMICS_HOME=$stage/$variant
BILICOMICS_RESEARCH_STAGE=$stage
BILICOMICS_RESEARCH_RUNTIME=$runtime
BILICOMICS_RESEARCH_CASE=$case_name
BILICOMICS_RESEARCH_DATA=$stage/cases/$case_name/data
BILICOMICS_RESEARCH_OUT=$stage/out/$case_name
BILICOMICS_STARTUP_CASE_TOKEN=$token
BILICOMICS_STARTUP_BATCH=$run_id
export BILICOMICS_HOME BILICOMICS_RESEARCH_STAGE BILICOMICS_RESEARCH_RUNTIME BILICOMICS_RESEARCH_CASE
export BILICOMICS_RESEARCH_DATA BILICOMICS_RESEARCH_OUT BILICOMICS_STARTUP_CASE_TOKEN BILICOMICS_STARTUP_BATCH
exec sh "$BILICOMICS_HOME/launch.sh"
'''
    members["entry.sh"] = entry.encode()
    for case_name in cases:
        token = secrets.token_hex(32)
        members["out/" + case_name + "/owner"] = (token + "\n").encode()
    payload = io.BytesIO()
    with tarfile.open(fileobj=payload, mode="w:gz") as archive:
        for name, content in members.items():
            info = tarfile.TarInfo(name)
            info.size = len(content)
            info.mode = 0o755 if name.endswith(".sh") or name in ("tools/startup-clock", "tools/physical-framebuffer") or "/app/" in name else 0o600
            info.mtime = 0
            archive.addfile(info, io.BytesIO(content))
    new_file(root / "inputs/payload.tar.gz", payload.getvalue())
    json_new(root / "inputs/fixture-manifest.json", fixture_manifest)
    new_file(root / "tools/startup_ab.py", regular(__file__), 0o700)
    new_file(root / "tools/native_recovery_fragment.sh", NATIVE_RECOVERY, 0o600)
    new_file(root / "tools/guarded-device-ssh.py", regular(config["guarded_transport"]), 0o700)
    new_file(root / "tools/device_lease.py", regular(config["lease_utility"]), 0o600)
    transport = json.loads(regular(config["transport_template"]))
    transport.update(owner_thread=config["owner_thread"], lease_run_id=run_id,
                     lease_dir=LEASE_DIRECTORY, lease_token_file=str(root / "private/lease-token"),
                     lease_tool_directory=str(root / "tools"))
    json_new(root / "private/transport.json", transport)
    config = {**config, "root": str(root), "run_id": run_id, "stage": stage, "runtime": runtime,
              "cases": cases, "case_metadata": case_metadata, "primary_rounds": PRIMARY_ROUNDS,
              "variants": records, "payload_sha256": sha(payload.getvalue()),
              "instrumented_sources": {name: sha(value) for name, value in members.items() if name.endswith(".sh")},
              "supervisor_sha256": sha(SUPERVISOR.encode()), "case_timeout_seconds": CASE_LIMIT_SECONDS,
              "native_recovery_fragment_sha256": sha(NATIVE_RECOVERY.encode()),
              "host_tools": {path.name: sha(regular(path)) for path in sorted((root / "tools").iterdir())},
              "restoration_timeout_seconds": RESTORE_LIMIT_SECONDS}
    json_new(root / "private/config.json", config)
    json_new(root / "evidence/preparation.json", {key: value for key, value in config.items()
        if key not in ("transport_template", "guarded_transport", "lease_utility", "preserve_paths")})
    require(not (root / "private/lease-token").exists(), "Preparation must not create a lease")
    print(json.dumps({"prepared": True, "root": str(root), "run_id": run_id,
                      "payload_sha256": config["payload_sha256"], "physical_device_accessed": False}, indent=2))


def native_functions():
    return PROC_FUNCTIONS + r'''
native_ready() {
    for lock in /tmp/bilicomics-launcher.lock /tmp/aidoku-launcher.lock /tmp/weread-launcher.lock; do
        [ ! -e "$lock" ] && [ ! -L "$lock" ] || return 1
    done
    case "$(status statusbar)" in *start/running*) ;; *) return 1;; esac
    native_pids=$(pidof awesome || :)
    [ -n "$native_pids" ] || return 1
    for native_pid in $native_pids; do
        proc_read "$native_pid" || return 1
        case "$proc_state" in S|R) ;; *) return 1;; esac
    done
}
save_native() {
    native_ready
    for native_pid in $native_pids; do
        proc_read "$native_pid"
        printf '%s %s %s\n' "$native_pid" "$proc_birth" "$(readlink "/proc/$native_pid/exe")"
    done
}
'''


class Batch:
    def __init__(self, root):
        require(sys.platform == "linux", "Run on test-env")
        self.root = root.resolve(strict=True)
        self.config = json.loads(regular(root / "private/config.json"))
        require(self.config["root"] == str(self.root) and not (root / "report.json").exists(), "A batch can execute only once")
        require(self.config["host_tools"] == {path.name: sha(regular(path)) for path in sorted((root / "tools").iterdir())
                if not (path.name == "__pycache__" and path.is_dir() and not path.is_symlink())},
                "A frozen host tool changed before device access")
        self.stage, self.runtime = self.config["stage"], self.config["runtime"]
        self.owner, self.run_id = self.config["owner_thread"], self.config["run_id"]
        self.transport = root / "private/transport.json"
        self.record = {"schema_version": 1, "run_id": self.run_id, "owner_thread": self.owner,
            "deployment_stage": self.stage,
            "started_at_utc": datetime.now(timezone.utc).isoformat(), "variants": self.config["variants"],
            "cases": self.config["cases"], "case_metadata": self.config["case_metadata"],
            "primary_rounds": PRIMARY_ROUNDS, "commands": [], "success": False, "lease_released": False,
            "native_restored": False, "same_boot": False, "install_unchanged": False,
            "lighting_restored": False, "transport_closed": False, "completed_cases": [],
            "preservation_scope": "Exact selected regular-file identities, including recorded absences; this is not a whole-tree census",
            "preservation_path_count": len(self.config["preserve_paths"]),
            "scope": "One fresh aggregate lease; warmup A/B then six balanced AB/BA pairs; one shared source/package/ELF/compiler identity and only the frozen serial preparation flag differs; offline demo snapshot; native Scriptlet route; CLOCK_MONOTONIC readiness and first Shelf SEND endpoints; no optical or complete-cover claim.",
            "post_business_observation": {"target": "first qualified business SEND plus 1500 ms", "all_cases_cross_gate": True,
                "procfs_sampling": "After the post-business gate and any scheduled framebuffer capture; not exact SEND sampling or a continuous peak"}}
        self.active_case = None
        self.lighting = None
        self.owned = False
        self.staged = False
        self.device_writes_started = False
        self.transports = []
        self.marker = "bilicomics-startup-batch:" + self.run_id

    def signal_transport(self, process, birth, requested_signal):
        # start_new_session binds this live direct child to its private group.
        # Recheck birth/group/session immediately before a timeout signal.
        if process.poll() is not None or birth is None:
            return False
        try:
            fields = Path(f"/proc/{process.pid}/stat").read_text().rsplit(") ", 1)[1].split()
            if int(fields[19]) != birth or int(fields[2]) != process.pid or int(fields[3]) != process.pid:
                return False
            os.killpg(process.pid, requested_signal)
            return True
        except (OSError, ValueError, IndexError):
            return False

    def close_transport(self, process, birth):
        # This runs for timeouts and every asynchronous exception. Repeated
        # cancellation cannot interrupt bounded transport reaping.
        with defer_control_signals():
            self.signal_transport(process, birth, signal.SIGTERM)
            try:
                return process.communicate(timeout=5)
            except subprocess.TimeoutExpired:
                self.signal_transport(process, birth, signal.SIGKILL)
                try:
                    return process.communicate(timeout=5)
                except subprocess.TimeoutExpired as caught:
                    output, error = caught.output or b"", caught.stderr or b""
                    for stream in (process.stdin, process.stdout, process.stderr):
                        if stream is not None:
                            stream.close()
                    process.poll()
                    return output, error

    def command(self, script, name, data=None, timeout=30, required=True):
        script = "# " + self.marker + "\nset -eu\n" + script
        command = [sys.executable, str(self.root / "tools/guarded-device-ssh.py"), "--config", str(self.transport), script]
        process, birth, output, error = None, None, b"", b""
        expired = False
        interrupted = None
        try:
            with defer_control_signals():
                process = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                           stderr=subprocess.PIPE, start_new_session=True)
                try:
                    fields = Path(f"/proc/{process.pid}/stat").read_text().rsplit(") ", 1)[1].split()
                    birth = int(fields[19])
                except (OSError, ValueError, IndexError):
                    birth = None
                self.transports.append({"pid": process.pid, "birth": birth})
            output, error = process.communicate(data, timeout=timeout)
        except subprocess.TimeoutExpired:
            expired = True
            output, error = self.close_transport(process, birth)
        except BaseException as caught:
            interrupted = caught
            if process is not None:
                output, error = self.close_transport(process, birth)
        new_file(self.root / "evidence" / (name + ".stdout"), output)
        new_file(self.root / "evidence" / (name + ".stderr"), error)
        returncode = process.returncode if process is not None else None
        self.record["commands"].append({"name": name, "returncode": returncode, "timeout": expired,
                                       "interrupted": interrupted is not None})
        if interrupted is not None:
            raise interrupted
        if required:
            require(returncode == 0 and not expired, "A bounded device command failed: " + name)
        return output if returncode == 0 and not expired else None

    def guard(self):
        return f'''test ! -L {q(self.stage)}
test "$(readlink -f {q(self.stage)})" = {q(self.stage)}
test "$(cat {q(self.stage + '/owner')})" = {q(self.run_id)}
test ! -L {q(self.runtime)}
test "$(cat {q(self.runtime + '/owner')})" = {q(self.run_id)}
'''

    def preserve_script(self):
        paths = self.config["preserve_paths"]
        require(isinstance(paths, list) and paths, "List the daily installation/data/entry paths to preserve")
        require({"/mnt/us/bilicomics/app/bilicomics", "/mnt/us/bilicomics/launch.sh", "/mnt/us/bilicomics/data/state.sqlite3"}.issubset(paths),
                "Preserve the expected daily application, launcher and state identities")
        lines = []
        for path in paths:
            require(isinstance(path, str) and path.startswith("/mnt/us/") and ".." not in PurePosixPath(path).parts,
                    "Invalid preserved path")
            quoted = q(path)
            lines.append(f'''test ! -L {quoted}
if [ -e {quoted} ]; then test -f {quoted}; sha256sum {quoted}; else printf '%s\\n' {q('MISSING ' + path)}; fi''')
        return "\n".join(lines) + "\n"

    def save_lighting(self):
        lines = self.command("lipc-get-prop -i com.lab126.powerd flIntensity\nlipc-get-prop -i com.lab126.powerd currentAmberLevel", "lighting-before").decode().splitlines()
        require(len(lines) == 2 and all(value.isdecimal() and 0 <= int(value) <= 24 for value in lines), "Invalid original lighting")
        self.lighting = lines

    def input_roster(self, name):
        return self.command(r'''
count=0
for node in /sys/class/input/event*; do
    [ -d "$node" ] || continue
    count=$((count + 1))
    [ "$count" -le 64 ] || exit 1
    printf '%s\n' "${node##*/}"
    for leaf in dev device/name device/capabilities/abs; do
        [ -f "$node/$leaf" ] && [ ! -L "$node/$leaf" ] || exit 1
        head -c 8193 "$node/$leaf"
    done
done
[ "$count" -gt 0 ]
''', name)

    def restore_lighting(self, name):
        require(self.lighting is not None, "Missing original lighting")
        first, second = self.lighting
        self.command(f'''lipc-set-prop -i com.lab126.powerd flIntensity {first}
lipc-set-prop -i com.lab126.powerd currentAmberLevel {second}
test "$(lipc-get-prop -i com.lab126.powerd flIntensity)" = {first}
test "$(lipc-get-prop -i com.lab126.powerd currentAmberLevel)" = {second}
''', name)

    def prepare_device(self):
        initial = self.command(native_functions() + r'''
native_ready
command -v cmp >/dev/null
test "$(lipc-get-prop -s com.lab126.powerd state)" = active
cat /etc/prettyversion.txt
''', "native-before")
        require(re.search(rb"Kindle\s+" + re.escape(self.config["firmware"].encode()) + rb"(?=[^0-9.]|$)", initial), "The supervised firmware differs")
        self.record["native_baseline_sha256"] = sha(initial)
        self.before_install = self.command(self.preserve_script(), "install-before")
        self.boot = self.command("cat /proc/sys/kernel/random/boot_id", "boot-before")
        self.native_ids = self.command(native_functions() + "save_native | sort\n", "native-identities-before")
        self.roster = self.input_roster("input-roster-before")
        self.save_lighting()
        self.device_writes_started = True
        self.command(f'''test ! -e {q(self.stage)} && test ! -L {q(self.stage)}
test ! -e {q(self.runtime)} && test ! -L {q(self.runtime)}
mkdir {q(self.stage)}
printf '%s\\n' {q(self.run_id)} >{q(self.stage + '/owner')}
mkdir -m 700 {q(self.runtime)}
printf '%s\\n' {q(self.run_id)} >{q(self.runtime + '/owner')}
lipc-set-prop -i com.lab126.powerd touchScreenSaverTimeout 1
''', "create-stage")
        self.staged = True
        payload = regular(self.root / "inputs/payload.tar.gz")
        require(sha(payload) == self.config["payload_sha256"], "The prepared payload changed")
        self.command(self.guard() + f"tar -xzf - -C {q(self.stage)}", "upload", payload, timeout=180)
        checks = []
        for variant in VARIANTS:
            record = self.config["variants"][variant]
            for name, digest in (("app/bilicomics", record["binary_sha256"]), ("app/kindle-jpeg-worker", record["worker_sha256"]),
                                 ("launch.sh", record["instrumented_launcher_sha256"])):
                checks.append(f'test "$(sha256sum {q(self.stage + "/" + variant + "/" + name)} | cut -d " " -f 1)" = {digest}')
            helper = self.stage + "/" + variant + "/app/bilicomics-native-query"
            checks.append(f'test ! -e {q(helper)} && test ! -L {q(helper)}')
        self.command(self.guard() + "\n".join(checks) + f'''\n{q(self.stage + '/tools/physical-framebuffer')} --info
mkdir {q(self.stage + '/cases')}
for case_name in {' '.join(map(q, self.config['cases']))}; do
    mkdir {q(self.stage + '/cases')}/"$case_name"
    cp -R {q(self.stage + '/fixture/data')} {q(self.stage + '/cases')}/"$case_name"/data
    mkdir -m 700 {q(self.runtime)}/"$case_name"
done
''', "verify-upload", timeout=35)
        info = (self.root / "evidence/verify-upload.stdout").read_bytes().splitlines()[0]
        geometry = json.loads(info)
        require(geometry.get("read_only") is True and geometry.get("width") == 1860 and geometry.get("height") == 2480
                and geometry.get("xoffset") == geometry.get("yoffset") == 0, "The framebuffer differs from the accepted Scribe geometry")
        native_frame = self.command(self.guard() + f'{q(self.stage + "/tools/physical-framebuffer")} --capture',
                                    "native-frame-before", timeout=20)
        self.record["native_framebuffer_before_sha256"] = sha(native_frame)

    def case(self, case_name):
        out = self.stage + "/out/" + case_name
        self.active_case = case_name
        software_variant = self.config["case_metadata"][case_name]["software_variant"]
        variant_record = self.config["variants"][software_variant]
        binary_path = self.stage + "/" + software_variant + "/app/bilicomics"
        launcher_path = self.stage + "/" + software_variant + "/launch.sh"
        identity = f'''test -f {q(binary_path)} && test ! -L {q(binary_path)}
binary_digest=$(sha256sum {q(binary_path)} | cut -d ' ' -f 1)
test "$binary_digest" = {variant_record['binary_sha256']}
test -f {q(launcher_path)} && test ! -L {q(launcher_path)}
launcher_digest=$(sha256sum {q(launcher_path)} | cut -d ' ' -f 1)
test "$launcher_digest" = {variant_record['instrumented_launcher_sha256']}
printf '{{"binary_sha256":"%s","launcher_sha256":"%s"}}\\n' "$binary_digest" "$launcher_digest" >{q(out + '/deployed-identity.json')}
'''
        script = self.guard() + native_functions() + f'''
{identity}
save_native >{q(out + '/native-before.tsv')}
test -s {q(out + '/native-before.tsv')}
status statusbar >{q(out + '/statusbar-before.txt')}
test "$(sort {q(out + '/native-before.tsv')})" = {q(self.native_ids.decode().rstrip())}
token=$(cat {q(out + '/owner')})
printf '%s %s\\n' {q(case_name)} "$token" >{q(self.runtime + '/dispatch.pending')}
mv {q(self.runtime + '/dispatch.pending')} {q(self.runtime + '/dispatch')}
lipc-set-prop -i com.lab126.powerd touchScreenSaverTimeout 1
{q(self.stage + '/tools/startup-clock')} {q(self.runtime + '/' + case_name + '/request.json')} -- lipc-set-prop com.lab126.appmgrd start {q('app://tech.hackerdude.shell_integration.launcher/' + self.stage + '/entry.sh')}
remaining=800
while [ "$remaining" -gt 0 ] && [ ! -f {q(out + '/recovery.json')} ]; do
    usleep 100000
    remaining=$((remaining - 1))
done
test "$remaining" -gt 0
cp {q(self.runtime + '/' + case_name + '/request.json')} {q(out + '/request.json')}
for log_name in launcher-timing.tsv launcher.log application.log; do
    source={q(self.stage + '/cases/' + case_name + '/data/logs')}/"$log_name"
    destination={q(out)}/production-"$log_name"
    if [ -e "$source" ] || [ -L "$source" ]; then
        test -f "$source" && test ! -L "$source"
        test ! -e "$destination" && test ! -L "$destination"
        cp "$source" "$destination"
    fi
done
cat {q(out + '/recovery.json')}
IFS=' ' read -r supervisor_pid supervisor_birth <{q(out + '/supervisor.pid')}
remaining=50
while same_birth "$supervisor_pid" "$supervisor_birth"; do
    [ "$remaining" -gt 0 ] || exit 1
    usleep 100000
    remaining=$((remaining - 1))
done
native_ready
test "$(save_native | sort)" = {q(self.native_ids.decode().rstrip())}
'''
        recovery = json.loads(self.command(script, "case-" + case_name, timeout=88))
        require(recovery.get("passed") is True and recovery.get("owned_processes_gone") is True
                and recovery.get("native_restored") is True and recovery.get("production_lock_absent") is True
                and recovery.get("launcher_exit_status") == 0, "A case did not restore native control: " + case_name)
        self.restore_lighting("lighting-" + case_name)
        self.record["completed_cases"].append(case_name)
        self.active_case = None
        print("COMPLETED " + case_name, flush=True)

    def abort_active(self):
        if self.active_case is None:
            return
        out = self.stage + "/out/" + self.active_case
        script = self.guard() + PROC_FUNCTIONS + f'''
if [ -f {q(out + '/supervisor.pid')} ]; then
    IFS=' ' read -r pid birth <{q(out + '/supervisor.pid')}
    token=$(cat {q(out + '/owner')})
    if same_birth "$pid" "$birth" && has_case_token "$pid"; then
        tr '\\000' '\\n' <"/proc/$pid/cmdline" | grep -Fx {q(self.stage + '/supervise.sh')} >/dev/null
        kill -TERM "$pid"
    fi
fi
remaining=350
while [ "$remaining" -gt 0 ] && [ ! -f {q(out + '/recovery.json')} ]; do usleep 100000; remaining=$((remaining - 1)); done
test "$remaining" -gt 0
cat {q(out + '/recovery.json')}
'''
        value = self.command(script, "abort-active", timeout=42, required=False)
        require(value is not None and json.loads(value).get("native_restored") is True
                and json.loads(value).get("owned_processes_gone") is True, "Active case recovery is incomplete; retain the lease")
        self.active_case = None

    def transport_closure(self):
        active, unreadable = [], 0
        marker = self.marker.encode()
        helper = os.fsencode(self.root / "tools/guarded-device-ssh.py")
        for process in Path("/proc").iterdir():
            if not process.name.isdecimal() or process.name == str(os.getpid()):
                continue
            try:
                if process.stat().st_uid != os.geteuid():
                    continue
                args = (process / "cmdline").read_bytes().split(b"\0")
                fields = (process / "stat").read_bytes().rsplit(b") ", 1)[1].split()
                require(len(fields) >= 20, "Incomplete host process identity")
                if helper in args or (args and args[0].rsplit(b"/", 1)[-1] == b"ssh" and any(marker in arg for arg in args)):
                    active.append({"pid": int(process.name), "birth": int(fields[19])})
            except (FileNotFoundError, ProcessLookupError):
                continue
            except (OSError, ValueError, IndexError, RuntimeError):
                unreadable += 1
        report = {"schema_version": 1, "run_id": self.run_id, "owner_thread": self.owner,
            "active_owned_transports": active, "unreadable_host_process_count": unreadable,
            "connections_closed": not active and not unreadable, "device_polled": False,
            "signals_sent": False, "lease_accessed": False, "ssh_multiplexing": False}
        json_new(self.root / "evidence/transport-closure.json", report)
        require(report["connections_closed"], "Owned transports did not close; retain the lease")
        self.record["transport_closed"] = True

    def finish_device(self):
        self.abort_active()
        if self.lighting:
            self.restore_lighting("restore-lighting")
            self.record["lighting_restored"] = True
        self.command(native_functions() + "native_ready\n" +
                     f'test "$(save_native | sort)" = {q(self.native_ids.decode().rstrip())}\n', "native-final")
        self.record["native_restored"] = True
        if self.staged:
            native_frame = self.command(self.guard() + f'{q(self.stage + "/tools/physical-framebuffer")} --capture',
                                        "native-frame-final", timeout=20)
            self.record["native_framebuffer_final_sha256"] = sha(native_frame)
            output = self.command(self.guard() + f'tar -czf - -C {q(self.stage)} out', "collect", timeout=45)
            new_file(self.root / "outputs.tar.gz", output)
            self.record["outputs_sha256"] = sha(output)
        after = self.command(self.preserve_script(), "install-after")
        self.record["install_unchanged"] = after == self.before_install
        boot = self.command("cat /proc/sys/kernel/random/boot_id", "boot-after")
        self.record["same_boot"] = boot == self.boot
        roster = self.input_roster("input-roster-after")
        self.record["input_roster_unchanged"] = roster == self.roster
        require(self.record["input_roster_unchanged"], "Input metadata changed across the batch")
        require(self.record["install_unchanged"] and self.record["same_boot"], "Installation/data or boot identity changed")
        if self.staged:
            self.command(self.guard() + native_functions() + f'''native_ready
rm -rf -- {q(self.stage)}
test ! -e {q(self.stage)}
test ! -L {q(self.runtime)}
test "$(cat {q(self.runtime + '/owner')})" = {q(self.run_id)}
rm -rf -- {q(self.runtime)}
test ! -e {q(self.runtime)}
''', "remove-stage")
            self.record["temporary_stage_removed"] = True

    def run(self, cancellation=None):
        sys.dont_write_bytecode = True
        sys.path.insert(0, str(self.root / "tools"))
        from device_lease import LeaseRequest, acquire, release
        request = LeaseRequest(Path(LEASE_DIRECTORY), self.root / "private/lease-token", self.owner, self.run_id)
        error = None
        fields = Path("/proc/self/stat").read_text().rsplit(") ", 1)[1].split()
        json_new(self.root / "evidence/driver.json", {"pid": os.getpid(), "birth": int(fields[19]),
                 "run_id": self.run_id, "owner_thread": self.owner})
        try:
            with defer_control_signals():
                proof = acquire(request)
                self.owned = True
            json_new(self.root / "evidence/lease-acquired.json", proof)
            self.prepare_device()
            for case_name in self.config["cases"]:
                self.case(case_name)
            self.record["success"] = True
        except BaseException as caught:
            self.record["success"] = False
            self.record["error"] = type(caught).__name__ + ": " + str(caught)
            error = caught
        finally:
            if cancellation is not None:
                cancellation.recovering = True
            if self.owned:
                try:
                    if self.device_writes_started:
                        self.finish_device()
                    else:
                        self.record["native_state_changed"] = False
                    self.transport_closure()
                    proof = release(request, native_restored_and_connections_closed=True)
                    json_new(self.root / "evidence/lease-released.json", proof)
                    self.record["lease_released"] = proof.get("state") == "released" and proof.get("owner_and_token_checked") is True
                    require(self.record["lease_released"], "Exact lease release did not succeed")
                except BaseException as caught:
                    self.record["success"] = False
                    self.record["recovery_error"] = type(caught).__name__ + ": " + str(caught)
                    error = error or caught
            self.record["finished_at_utc"] = datetime.now(timezone.utc).isoformat()
            if cancellation is not None:
                self.record["cancelled"] = cancellation.cancelled
                self.record["driver_signals"] = cancellation.received
            json_new(self.root / "report.json", self.record)
            print(json.dumps({key: value for key, value in self.record.items() if key != "commands"}, indent=2), flush=True)
        if (self.root / "outputs.tar.gz").exists():
            with tarfile.open(self.root / "outputs.tar.gz") as archive:
                archive.extractall(self.root / "outputs", filter="data")
        if error is not None:
            raise RuntimeError("The batch failed; inspect private evidence and retained lease status") from error


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="operation", required=True)
    prepare_args = sub.add_parser("prepare")
    prepare_args.add_argument("--config", type=Path, required=True)
    prepare_args.add_argument("--batch-root", type=Path, required=True)
    run_args = sub.add_parser("run")
    run_args.add_argument("--batch-root", type=Path, required=True)
    args = parser.parse_args()
    os.umask(0o077)
    if args.operation == "prepare":
        prepare(args.config, args.batch_root)
    else:
        with Cancellation() as cancellation:
            Batch(args.batch_root).run(cancellation)


if __name__ == "__main__":
    main()
