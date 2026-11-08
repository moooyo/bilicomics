#!/bin/sh
# Preserve native power events while recording the supervised observer lineage.
set -eu
umask 077
run_root=${BILICOMICS_HOME:?}
case "$run_root" in /mnt/us/bilicomics-physical-*) run_id=${run_root#/mnt/us/bilicomics-physical-} ;; *) exit 2 ;; esac
case "$run_id" in *[!a-z0-9-]*|'') exit 2 ;; esac
[ "${#run_id}" -le 48 ] || exit 2
logs=$run_root/data/logs
lock=/tmp/bilicomics-launcher.lock
read_one() { sed -n '1{s/\r$//;p;}' "$1"; }
read_record() { [ -f "$1" ] && [ ! -L "$1" ] && read_one "$1"; }
is_pid() {
    case "$1" in *[!0-9]*|''|0*|1) return 1 ;; esac
    [ "${#1}" -le 10 ]
}
has_token() {
    [ -r "/proc/$1/environ" ] || return 1
    tr '\000' '\n' <"/proc/$1/environ" | grep -F -x "BILICOMICS_PHYSICAL_TOKEN=$token" >/dev/null
}
snapshot_process() {
    is_pid "$1" || return 1
    snapshot_pid=$1
    IFS= read -r snapshot_stat <"/proc/$snapshot_pid/stat" || return 1
    snapshot_tail=${snapshot_stat##*) }
    [ "$snapshot_tail" != "$snapshot_stat" ] || return 1
    set -- $snapshot_tail
    [ "$#" -ge 20 ] || return 1
    case "$1" in Z|X|x) return 1 ;; esac
    snapshot_parent=$2
    snapshot_pgid=$3
    snapshot_sid=$4
    shift 19
    snapshot_birth=$1
    case "$snapshot_parent:$snapshot_pgid:$snapshot_sid:$snapshot_birth" in
        *[!0-9:]*|:*|*:) return 1 ;;
    esac
    snapshot_exe=$(readlink "/proc/$snapshot_pid/exe") || return 1
    case "$snapshot_exe" in /*) ;; *) return 1 ;; esac
    case "$snapshot_exe" in *'|'*|*'(deleted)'*) return 1 ;; esac
    has_token "$snapshot_pid" || return 1
    printf '%s|%s|%s|%s|%s|%s\n' "$snapshot_pid" "$snapshot_birth" "$snapshot_parent" \
        "$snapshot_pgid" "$snapshot_sid" "$snapshot_exe"
}
assert_process() { [ "$(snapshot_process "$1")" = "$2" ]; }
lock_inode() { ls -id "$lock" 2>/dev/null | awk '{ print $1 }'; }
load_observer() {
    [ -d "$lock" ] && [ ! -L "$lock" ] || return 1
    observer_lock_inode=$(lock_inode)
    case "$observer_lock_inode" in *[!0-9]*|'') return 1 ;; esac
    launcher_pid=$(read_record "$lock/pid") || return 1
    is_pid "$launcher_pid" || return 1
    launch_nonce=$(read_record "$lock/nonce") || return 1
    case "$launch_nonce" in *[!0-9a-f]*|'') return 1 ;; esac
    [ "${#launch_nonce}" -eq 32 ] || return 1
    generation_record=$(read_record "$lock/generation") || return 1
    launch_generation=${generation_record%:"$launch_nonce"}
    case "$launch_generation" in *[!0-9]*|''|0*) return 1 ;; esac
    [ "${#launch_generation}" -le 9 ] && [ "$generation_record" = "$launch_generation:$launch_nonce" ] || return 1
    observer_record=$(read_record "$lock/observer-owned") || return 1
    IFS=: read -r owned_generation owned_nonce observer_pid observer_birth owned_extra <<EOF
$observer_record
EOF
    is_pid "$observer_pid" || return 1
    case "$observer_birth" in *[!0-9]*|'') return 1 ;; esac
    [ -z "$owned_extra" ] && [ "$observer_record" = "$launch_generation:$launch_nonce:$observer_pid:$observer_birth" ] || return 1
    observer_gate=$launch_generation:$launch_nonce:$observer_pid
    [ "$(read_record "$lock/observer-go")" = "$observer_gate" ] || return 1
    launcher_snapshot=$(snapshot_process "$launcher_pid") || return 1
    IFS='|' read -r launcher_id launcher_birth launcher_parent launcher_pgid launcher_sid launcher_exe <<EOF
$launcher_snapshot
EOF
    tr '\000' '\n' <"/proc/$launcher_pid/cmdline" | grep -F -x "$run_root/launch.sh" >/dev/null || return 1
    observer_snapshot=$(snapshot_process "$observer_pid") || return 1
    IFS='|' read -r observer_id observed_birth observer_parent observer_pgid observer_sid observer_exe <<EOF
$observer_snapshot
EOF
    [ "$observed_birth" = "$observer_birth" ] && [ "$observer_parent" = "$launcher_pid" ] \
        && [ "$observer_pgid:$observer_sid" = "$observer_pid:$observer_pid" ] \
        && [ "$observer_exe" = "$launcher_exe" ] || return 1
    check_observer
}
check_observer() {
    [ -d "$lock" ] && [ ! -L "$lock" ] && [ "$(lock_inode)" = "$observer_lock_inode" ] || return 1
    [ "$(read_record "$lock/pid")" = "$launcher_pid" ] \
        && [ "$(read_record "$lock/nonce")" = "$launch_nonce" ] \
        && [ "$(read_record "$lock/generation")" = "$generation_record" ] \
        && [ "$(read_record "$lock/observer-owned")" = "$observer_record" ] \
        && [ "$(read_record "$lock/observer-go")" = "$observer_gate" ] || return 1
    assert_process "$launcher_pid" "$launcher_snapshot" && assert_process "$observer_pid" "$observer_snapshot"
}
publish_line() {
    publish_name=$1
    publish_value=$2
    [ ! -e "$logs/$publish_name" ] && [ ! -L "$logs/$publish_name" ] || return 1
    [ ! -e "$logs/$publish_name.pending" ] && [ ! -L "$logs/$publish_name.pending" ] || return 1
    (set -C; printf '%s\n' "$publish_value" >"$logs/$publish_name.pending") || return 1
    [ ! -e "$logs/$publish_name" ] && [ ! -L "$logs/$publish_name" ] || return 1
    mv "$logs/$publish_name.pending" "$logs/$publish_name"
}

token=$(read_record "$logs/run-token")
case "$token" in *[!0-9a-f]*|'') exit 2 ;; esac
[ "${#token}" -eq 64 ] && [ "${BILICOMICS_PHYSICAL_TOKEN:-}" = "$token" ] || exit 2
original_command=$(read_record "$logs/power-event-command")
original_exe=$(read_record "$logs/power-event-executable")
case "$original_command:$original_exe" in /*:/*) ;; *) exit 2 ;; esac
[ "$original_command" != "$run_root/tools/lipc-wait-event" ] || exit 2
[ "$original_exe" != "$run_root/tools/lipc-wait-event" ] || exit 2
[ -x "$original_command" ] && [ "$(readlink -f "$original_command")" = "$original_exe" ] || exit 2
load_observer || exit 2
event_snapshot=$(snapshot_process "$$") || exit 2
IFS='|' read -r event_pid event_birth runner_pid event_pgid event_sid event_exe <<EOF
$event_snapshot
EOF
is_pid "$runner_pid" || exit 2
runner_snapshot=$(snapshot_process "$runner_pid") || exit 2
IFS='|' read -r runner_id runner_birth runner_parent runner_pgid runner_sid runner_exe <<EOF
$runner_snapshot
EOF
[ "$runner_parent" = "$observer_pid" ] && [ "$runner_pgid:$runner_sid" = "$observer_pid:$observer_pid" ] \
    && [ "$event_pgid:$event_sid" = "$observer_pid:$observer_pid" ] \
    && [ "$runner_exe" = "$observer_exe" ] && [ "$event_exe" = "$observer_exe" ] || exit 2

# The source runner is the event's real parent. Its shell $$ is not its PID.
check_event_chain() {
    check_observer && assert_process "$runner_pid" "$runner_snapshot" && assert_process "$$" "$event_snapshot"
}
check_event_chain || exit 2
publish_line power-source-runner.identity "$runner_pid|$runner_birth|$runner_exe|$observer_pid|$token" || exit 2
check_event_chain || exit 2
# This legacy filename now identifies the setsid anchor, never the source runner.
publish_line power-observer.identity "$observer_pid|$observer_birth|$observer_exe|$launcher_pid|$token" || exit 2
check_event_chain || exit 2
publish_line power-event.identity "$event_pid|$event_birth|$original_exe|$runner_pid|$token" || exit 2
check_event_chain || exit 2
[ -x "$original_command" ] && [ "$(readlink -f "$original_command")" = "$original_exe" ] || exit 2
exec "$original_command" "$@"
