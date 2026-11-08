#!/bin/sh
# Bound one isolated physical-device run without managing another application.
set -eu
umask 077

run_root=${1:?Supply the isolated physical-device root}
token=${2:?Supply the run ownership token}
init_timeout=${3:-90}
session_timeout=${4:-180}
case "$run_root" in
    /mnt/us/bilicomics-physical-*) run_id=${run_root#/mnt/us/bilicomics-physical-} ;;
    *) exit 2 ;;
esac
case "$run_id" in *[!a-z0-9-]*|'') exit 2 ;; esac
[ "${#run_id}" -le 48 ] || exit 2
case "$token" in *[!0-9a-f]*|'') exit 2 ;; esac
[ "${#token}" -eq 64 ] || exit 2
case "$init_timeout:$session_timeout" in *[!0-9:]*|:*) exit 2 ;; esac
[ "$init_timeout" -ge 10 ] && [ "$init_timeout" -le 120 ] || exit 2
[ "$session_timeout" -gt "$init_timeout" ] && [ "$session_timeout" -le 600 ] || exit 2
logs=$run_root/data/logs
lock=/tmp/bilicomics-launcher.lock
read_one() { sed -n '1{s/\r$//;p;}' "$1" 2>/dev/null || :; }
[ "$(read_one "$logs/run-token")" = "$token" ] || exit 2
[ "${BILICOMICS_PHYSICAL_TOKEN:-}" = "$token" ] || exit 2
[ -x "$run_root/app/bilicomics" ] && [ -r "$run_root/launch.sh" ] || exit 2
[ ! -e "$logs/watchdog.ready" ] && [ ! -e "$logs/watchdog.done" ] || exit 2
[ ! -e "$lock" ] && [ ! -L "$lock" ] || { printf '%s\n' 'Refusing an existing production launcher lock.' >&2; exit 3; }

launcher_pid=''
launcher_ticks=''
launcher_exe=''
launcher_proven=0
launcher_script_verified=0
owned_lock_inode=''
owned_lock_nonce=''
cleanup_started=0
recovery_complete=1
reason=unexpected-exit
ready_seen=0
statusbar_was_running=0
bounded_sequence=0
command_output=''
: >"$logs/owned-processes.tsv"
: >"$logs/native-baseline.tsv"

now_seconds() {
    read -r uptime_value unused </proc/uptime
    printf '%s\n' "${uptime_value%%.*}"
}
proc_fields() {
    case "$1" in *[!0-9]*|'') return 1 ;; esac
    [ -r "/proc/$1/stat" ] || return 1
    # The comm field may contain spaces and closing parentheses.
    IFS= read -r proc_stat_line <"/proc/$1/stat" || return 1
    printf '%s\n' "${proc_stat_line##*) }"
}
proc_ticks() { proc_fields "$1" | awk '{ print $20 }'; }
proc_parent() { proc_fields "$1" | awk '{ print $2 }'; }
proc_state() { proc_fields "$1" | awk '{ print $1 }'; }
proc_exe() { readlink "/proc/$1/exe" 2>/dev/null; }
has_token() {
    [ -r "/proc/$1/environ" ] || return 1
    tr '\000' '\n' <"/proc/$1/environ" | grep -F -x "BILICOMICS_PHYSICAL_TOKEN=$token" >/dev/null 2>&1
}
same_process() {
    [ "$(proc_ticks "$1")" = "$2" ] && [ "$(proc_exe "$1" || :)" = "$3" ] && has_token "$1"
}
same_native_process() {
    [ "$(proc_ticks "$1")" = "$2" ] && [ "$(proc_exe "$1" || :)" = "$3" ]
}
bounded_command() {
    command_limit=$1
    shift
    bounded_sequence=$((bounded_sequence + 1))
    command_output=$logs/service-command-$bounded_sequence.txt
    command_path=$(command -v "$1" 2>/dev/null || :)
    [ -n "$command_path" ] || return 127
    command_expected_exe=$(readlink -f "$command_path" 2>/dev/null || :)
    [ -n "$command_expected_exe" ] || return 127
    (trap - HUP INT TERM; exec "$@") >"$command_output" 2>&1 &
    command_pid=$!
    command_ticks=$(proc_ticks "$command_pid")
    command_initial_exe=$(proc_exe "$command_pid" || :)
    case "$command_ticks" in *[!0-9]*|'')
        if [ ! -d "/proc/$command_pid" ] || [ "$(proc_parent "$command_pid")" != "$$" ]; then
            if wait "$command_pid"; then return 0; else return $?; fi
        fi
        recovery_complete=0
        printf 'A bounded command identity could not be recorded: %s\n' "$1" >>"$logs/recovery.log"
        return 125
        ;;
    esac
    command_started=$(now_seconds)
    while [ "$(proc_ticks "$command_pid")" = "$command_ticks" ]; do
        command_state=$(proc_state "$command_pid")
        [ "$command_state" != Z ] && [ "$command_state" != X ] || break
        [ $(( $(now_seconds) - command_started )) -lt "$command_limit" ] || break
        sleep 1
    done
    command_state=$(proc_state "$command_pid")
    if [ "$(proc_ticks "$command_pid")" != "$command_ticks" ] || [ "$command_state" = Z ] || [ "$command_state" = X ]; then
        if wait "$command_pid"; then return 0; else return $?; fi
    fi
    # This is the direct child created above. Permit only its recorded shell
    # image or the resolved command image, with the same birth time and token.
    for command_signal in TERM KILL; do
        if same_process "$command_pid" "$command_ticks" "$command_expected_exe" || { [ -n "$command_initial_exe" ] && same_process "$command_pid" "$command_ticks" "$command_initial_exe"; }; then
            kill "-$command_signal" "$command_pid" 2>/dev/null || :
        fi
        [ "$command_signal" != TERM ] || sleep 1
    done
    command_state=$(proc_state "$command_pid")
    if [ "$(proc_ticks "$command_pid")" != "$command_ticks" ] || [ "$command_state" = Z ] || [ "$command_state" = X ]; then wait "$command_pid" 2>/dev/null || :; fi
    recovery_complete=0
    printf 'A bounded native command timed out: %s\n' "$1" >>"$logs/recovery.log"
    return 124
}
launcher_matches() {
    launcher_child_matches || return 1
    tr '\000' '\n' <"/proc/$launcher_pid/cmdline" | grep -F -x "$run_root/launch.sh" >/dev/null 2>&1
}
launcher_child_matches() {
    [ "$launcher_proven" -eq 1 ] && [ -n "$launcher_pid" ] && same_process "$launcher_pid" "$launcher_ticks" "$launcher_exe"
}
known_parent() {
    candidate_parent=$1
    while IFS='|' read -r known_pid known_ticks known_exe known_parent_pid known_role; do
        [ "$candidate_parent" = "$known_pid" ] || continue
        same_process "$known_pid" "$known_ticks" "$known_exe" && return 0
    done <"$logs/owned-processes.tsv"
    return 1
}
record_process() {
    record_pid=$1
    record_parent=$2
    record_role=$3
    has_token "$record_pid" || return 0
    record_ticks=$(proc_ticks "$record_pid")
    record_exe=$(proc_exe "$record_pid" || :)
    case "$record_ticks" in *[!0-9]*|'') return 0 ;; esac
    case "$record_exe" in /*) ;; *) return 0 ;; esac
    case "$record_exe" in *'|'*|*'(deleted)'*) return 0 ;; esac
    [ "$(proc_parent "$record_pid")" = "$record_parent" ] || return 0
    record_line=$record_pid'|'$record_ticks'|'$record_exe'|'$record_parent'|'$record_role
    if ! grep -F -x "$record_line" "$logs/owned-processes.tsv" >/dev/null 2>&1; then
        printf '%s\n' "$record_line" >>"$logs/owned-processes.tsv" || { recovery_complete=0; return 1; }
    fi
}
observe_owned_lock() {
    [ "$launcher_script_verified" -eq 1 ] || return 0
    [ "$(proc_ticks "$launcher_pid")" = "$launcher_ticks" ] || return 0
    [ -d "$lock" ] && [ ! -L "$lock" ] || return 0
    [ "$(read_one "$lock/pid")" = "$launcher_pid" ] || return 0
    candidate_nonce=$(read_one "$lock/nonce")
    case "$candidate_nonce" in *[!0-9a-f]*|'') return 0 ;; esac
    [ "${#candidate_nonce}" -eq 32 ] || return 0
    [ "$(read_one "$lock/generation")" = "1:$candidate_nonce" ] || return 0
    candidate_inode=$(ls -id "$lock" 2>/dev/null | awk '{ print $1 }')
    case "$candidate_inode" in *[!0-9]*|'') return 0 ;; esac
    if [ -z "$owned_lock_nonce" ]; then
        owned_lock_nonce=$candidate_nonce
        owned_lock_inode=$candidate_inode
        printf '%s|%s|%s|%s\n' "$launcher_pid" "$launcher_ticks" "$owned_lock_inode" "$owned_lock_nonce" >"$logs/owned-launcher-lock" || recovery_complete=0
    fi
}
record_persistent_identity() {
    persistent_file=$1
    persistent_role=$2
    persistent_expected_exe=$3
    persistent_line=$(read_one "$logs/$persistent_file")
    IFS='|' read -r persistent_pid persistent_ticks persistent_exe persistent_parent persistent_token <<EOF
$persistent_line
EOF
    [ "$persistent_token" = "$token" ] && [ "$persistent_exe" = "$persistent_expected_exe" ] || return 0
    case "$persistent_pid:$persistent_ticks:$persistent_parent" in *[!0-9:]*|:*|*:) return 0 ;; esac
    same_process "$persistent_pid" "$persistent_ticks" "$persistent_exe" || return 0
    persistent_record=$persistent_pid'|'$persistent_ticks'|'$persistent_exe'|'$persistent_parent'|'$persistent_role
    if ! grep -F -x "$persistent_record" "$logs/owned-processes.tsv" >/dev/null 2>&1; then
        printf '%s\n' "$persistent_record" >>"$logs/owned-processes.tsv" || recovery_complete=0
    fi
}
discover_children() {
    observe_owned_lock
    # These trusted wrappers prove the original lineage before exec, allowing
    # recovery after reparenting or after normal launcher-lock removal.
    record_persistent_identity runner.identity runner "$launcher_exe"
    record_persistent_identity native.identity application "$run_root/app/$(read_one "$logs/binary-name")"
    record_persistent_identity power-observer.identity observer "$launcher_exe"
    record_persistent_identity power-event.identity power-event "$(read_one "$logs/power-event-executable")"
    fallback_scan=0
    while IFS='|' read -r parent_pid parent_ticks parent_exe parent_parent parent_role; do
        same_process "$parent_pid" "$parent_ticks" "$parent_exe" || continue
        children_file=/proc/$parent_pid/task/$parent_pid/children
        if [ -r "$children_file" ]; then
            child_numbers=''
            IFS= read -r child_numbers <"$children_file" || :
            for discover_pid in $child_numbers; do record_process "$discover_pid" "$parent_pid" descendant; done
        else
            fallback_scan=1
        fi
    done <"$logs/owned-processes.tsv"
    if [ "$fallback_scan" -eq 1 ]; then
        for process_dir in /proc/[0-9]*; do
            discover_pid=${process_dir#/proc/}
            [ "$discover_pid" != "$$" ] && [ "$discover_pid" != "$launcher_pid" ] || continue
            discover_parent=$(proc_parent "$discover_pid")
            case "$discover_parent" in *[!0-9]*|'') continue ;; esac
            known_parent "$discover_parent" || continue
            record_process "$discover_pid" "$discover_parent" descendant
        done
    fi
}
signal_record() {
    same_process "$1" "$2" "$3" || return 0
    kill "-$4" "$1" 2>/dev/null || :
}
signal_descendants() {
    requested_signal=$1
    while IFS='|' read -r child_pid child_ticks child_exe child_parent child_role; do
        [ "$child_pid" != "$launcher_pid" ] || continue
        signal_record "$child_pid" "$child_ticks" "$child_exe" "$requested_signal"
    done <"$logs/owned-processes.tsv"
}
restore_native() {
    restoration_allowed=0
    if [ "$launcher_script_verified" -eq 1 ]; then
        if [ ! -e "$lock" ] && [ ! -L "$lock" ]; then
            restoration_allowed=1
        elif [ -n "$owned_lock_nonce" ] && [ -d "$lock" ] && [ ! -L "$lock" ] && [ "$(read_one "$lock/pid")" = "$launcher_pid" ] && [ "$(read_one "$lock/nonce")" = "$owned_lock_nonce" ] && [ "$(ls -id "$lock" 2>/dev/null | awk '{print $1}')" = "$owned_lock_inode" ]; then
            restoration_allowed=1
        else
            recovery_complete=0
            printf '%s\n' 'A current launcher lock is not proven to belong to this run; native restoration is withheld.' >>"$logs/recovery.log"
        fi
    fi
    while IFS='|' read -r baseline_kind baseline_pid baseline_ticks baseline_exe baseline_state; do
        [ "$baseline_kind" = awesome ] || continue
        if ! same_native_process "$baseline_pid" "$baseline_ticks" "$baseline_exe"; then
            printf 'Native awesome identity changed: %s\n' "$baseline_pid" >>"$logs/recovery.log"
            recovery_complete=0
            continue
        fi
        current_state=$(proc_state "$baseline_pid")
        case "$baseline_state:$current_state" in
            T:T|T:t|t:T|t:t) ;;
            T:*|t:*) recovery_complete=0; printf 'A previously stopped awesome process changed state: %s\n' "$baseline_pid" >>"$logs/recovery.log" ;;
            *:T|*:t) if [ "$restoration_allowed" -eq 1 ]; then kill -CONT "$baseline_pid" 2>/dev/null || recovery_complete=0; else recovery_complete=0; fi ;;
        esac
    done <"$logs/native-baseline.tsv"
    # Exclusive handoff and the recorded direct child bind this baseline to
    # the run even if a service stop and crash occurred between samples.
    if [ "$statusbar_was_running" -eq 1 ] && [ "$restoration_allowed" -eq 1 ]; then
        bounded_command 3 status statusbar || :
        case "$(cat "$command_output" 2>/dev/null || :)" in
            *stop/waiting*) bounded_command 5 start statusbar || recovery_complete=0 ;;
            *start/running*) ;;
            *) recovery_complete=0 ;;
        esac
    elif [ "$statusbar_was_running" -eq 0 ]; then
        bounded_command 3 status statusbar || :
        case "$(cat "$command_output" 2>/dev/null || :)" in *stop/waiting*) ;; *) recovery_complete=0 ;; esac
    fi
    if [ "$restoration_allowed" -eq 1 ] && command -v xrefresh >/dev/null 2>&1; then bounded_command 3 xrefresh || :; fi
    bounded_command 3 status statusbar || :
    cat "$command_output" >"$logs/statusbar-after.txt" 2>/dev/null || recovery_complete=0
    : >"$logs/awesome-after.tsv" || recovery_complete=0
    while IFS='|' read -r baseline_kind baseline_pid baseline_ticks baseline_exe baseline_state; do
        [ "$baseline_kind" = awesome ] || continue
        printf '%s|%s|%s|%s\n' "$baseline_pid" "$(proc_ticks "$baseline_pid")" "$(proc_exe "$baseline_pid" || :)" "$(proc_state "$baseline_pid")" >>"$logs/awesome-after.tsv" || recovery_complete=0
    done <"$logs/native-baseline.tsv"
}
remove_owned_lock() {
    [ -e "$lock" ] || [ -L "$lock" ] || return 0
    if [ ! -d "$lock" ] || [ -L "$lock" ] || [ -z "$owned_lock_nonce" ] || [ "$(read_one "$lock/pid")" != "$launcher_pid" ] || [ "$(read_one "$lock/nonce")" != "$owned_lock_nonce" ] || [ "$(ls -id "$lock" 2>/dev/null | awk '{ print $1 }')" != "$owned_lock_inode" ]; then
        printf '%s\n' 'Launcher lock identity is not proven; leaving it untouched.' >>"$logs/recovery.log"
        recovery_complete=0
        return 0
    fi
    for lock_item in "$lock"/* "$lock"/.[!.]* "$lock"/..?*; do
        [ -e "$lock_item" ] || [ -L "$lock_item" ] || continue
        case "${lock_item##*/}" in
            pid|power-events|nonce|generation|application-pid|observer-pid|application-exit|application-exit.pending|lifecycle-ready|lifecycle-ready.pending|lifecycle-request|lifecycle-request.pending|lifecycle-ack|lifecycle-ack.pending) ;;
            *) printf '%s\n' 'An unexpected lock entry requires operator review.' >>"$logs/recovery.log"; recovery_complete=0; return 0 ;;
        esac
        if [ -L "$lock_item" ] || { [ "${lock_item##*/}" = power-events ] && [ ! -p "$lock_item" ]; } || { [ "${lock_item##*/}" != power-events ] && [ ! -f "$lock_item" ]; }; then
            printf '%s\n' 'An unexpected lock entry type requires operator review.' >>"$logs/recovery.log"
            recovery_complete=0
            return 0
        fi
    done
    for lock_name in pid power-events nonce generation application-pid observer-pid application-exit application-exit.pending lifecycle-ready lifecycle-ready.pending lifecycle-request lifecycle-request.pending lifecycle-ack lifecycle-ack.pending; do
        rm -f "$lock/$lock_name"
    done
    rmdir "$lock" 2>/dev/null || recovery_complete=0
}
cleanup() {
    [ "$cleanup_started" -eq 0 ] || return 0
    cleanup_started=1
    # Evidence storage failure must never skip remaining native restoration.
    set +e
    trap '' HUP INT TERM
    trap - 0
    discover_children || recovery_complete=0
    if launcher_child_matches; then kill -TERM "$launcher_pid" 2>/dev/null || :; fi
    remaining=12
    while [ "$remaining" -gt 0 ] && launcher_child_matches; do
        sleep 1
        discover_children
        remaining=$((remaining - 1))
    done
    # Every signal rechecks PID, birth time, token and executable, using only
    # descendants recorded while their ancestry was still proven.
    signal_descendants TERM
    sleep 2
    signal_descendants KILL
    if launcher_child_matches; then kill -KILL "$launcher_pid" 2>/dev/null || :; fi
    convergence_remaining=3
    while [ "$convergence_remaining" -gt 0 ]; do
        sleep 1
        convergence_remaining=$((convergence_remaining - 1))
    done
    launcher_status=unknown
    if [ -n "$launcher_pid" ]; then
        launcher_final_state=$(proc_state "$launcher_pid")
        if [ ! -d "/proc/$launcher_pid" ] || [ "$(proc_ticks "$launcher_pid")" != "$launcher_ticks" ] || [ "$launcher_final_state" = Z ] || [ "$launcher_final_state" = X ]; then
            if wait "$launcher_pid"; then launcher_status=0; else launcher_status=$?; fi
        else
            # A task in uninterruptible kernel sleep must not block recovery.
            recovery_complete=0
            printf '%s\n' 'The launcher did not reach a reapable state.' >>"$logs/recovery.log"
        fi
    fi
    while IFS='|' read -r child_pid child_ticks child_exe child_parent child_role; do
        if same_process "$child_pid" "$child_ticks" "$child_exe"; then
            recovery_complete=0
            printf 'An owned process remains: %s\n' "$child_pid" >>"$logs/recovery.log"
        fi
    done <"$logs/owned-processes.tsv"
    # Unknown orphans are reported without signalling an unproven identity.
    # The wrappers above make app/runner/power-observer orphans recoverable.
    for orphan_dir in /proc/[0-9]*; do
        orphan_pid=${orphan_dir#/proc/}
        [ "$orphan_pid" != "$$" ] && [ "$(proc_parent "$orphan_pid")" = 1 ] && has_token "$orphan_pid" || continue
        orphan_state=$(proc_state "$orphan_pid")
        case "$orphan_state" in Z|X|'') continue ;; esac
        recovery_complete=0
        printf 'An unproven owned-token orphan requires operator review: %s|%s|%s\n' "$orphan_pid" "$(proc_ticks "$orphan_pid")" "$(proc_exe "$orphan_pid" || :)" >>"$logs/recovery.log"
    done
    restore_native
    if [ "$recovery_complete" -eq 1 ]; then remove_owned_lock; fi
    if [ -e "$lock" ] || [ -L "$lock" ]; then printf '%s\n' present >"$logs/launcher-lock-after.txt"; else printf '%s\n' absent >"$logs/launcher-lock-after.txt"; fi
    printf '%s|%s|%s|%s\n' "$token" "$reason" "$recovery_complete" "$launcher_status" >"$logs/watchdog.done.pending"
    mv "$logs/watchdog.done.pending" "$logs/watchdog.done"
    [ "$recovery_complete" -eq 1 ]
}

trap 'cleanup' 0
trap 'reason=watchdog-signal; exit 143' HUP INT TERM
bounded_command 3 status statusbar || :
statusbar_baseline=$(cat "$command_output" 2>/dev/null || :)
printf '%s\n' "$statusbar_baseline" >"$logs/statusbar-before.txt"
case "$statusbar_baseline" in *start/running*) statusbar_was_running=1 ;; *stop/waiting*) ;; *) reason=unknown-statusbar-baseline; exit 3 ;; esac
command -v pidof >/dev/null 2>&1 || { reason=missing-pidof; exit 3; }
awesome_count=0
for baseline_pid in $(pidof awesome 2>/dev/null || :); do
    baseline_ticks=$(proc_ticks "$baseline_pid")
    baseline_exe=$(proc_exe "$baseline_pid" || :)
    baseline_state=$(proc_state "$baseline_pid")
    case "$baseline_ticks" in *[!0-9]*|'') continue ;; esac
    [ -n "$baseline_exe" ] && [ -n "$baseline_state" ] || continue
    printf 'awesome|%s|%s|%s|%s\n' "$baseline_pid" "$baseline_ticks" "$baseline_exe" "$baseline_state" >>"$logs/native-baseline.tsv"
    awesome_count=$((awesome_count + 1))
done
[ "$awesome_count" -gt 0 ] || { reason=missing-native-window-manager; exit 3; }
[ -x "$run_root/tools/lipc-wait-event" ] || { reason=missing-event-identity-wrapper; exit 3; }
original_event_command=$(command -v lipc-wait-event 2>/dev/null || :)
case "$original_event_command" in /*) ;; *) reason=missing-native-power-observer; exit 3 ;; esac
original_event_exe=$(readlink -f "$original_event_command")
printf '%s\n' "$original_event_command" >"$logs/power-event-command"
printf '%s\n' "$original_event_exe" >"$logs/power-event-executable"
printf '%s|%s|%s\n' "$$" "$(proc_ticks "$$")" "$(proc_exe "$$")" >"$logs/watchdog.identity"
started=$(now_seconds)
BILICOMICS_HOME=$run_root BILICOMICS_PHYSICAL_TOKEN=$token DISPLAY=:0.0 PATH=$run_root/tools:$PATH sh "$run_root/launch.sh" --detached >>"$logs/launcher.log" 2>&1 &
launcher_pid=$!
launcher_ticks=$(proc_ticks "$launcher_pid")
launcher_exe=$(proc_exe "$launcher_pid" || :)
case "$launcher_ticks" in *[!0-9]*|'') reason=unrecorded-launcher; exit 3 ;; esac
# Establish the direct child before waiting for its shell exec handshake.
# This identity also owns cleanup if the expected script never starts.
if [ "$(proc_parent "$launcher_pid")" = "$$" ] && [ -n "$launcher_exe" ] && has_token "$launcher_pid"; then
    launcher_proven=1
else
    reason=unproven-direct-child
    exit 3
fi
record_process "$launcher_pid" "$$" launcher
handshake_remaining=3
while ! launcher_matches && [ "$handshake_remaining" -gt 0 ]; do
    sleep 1
    handshake_remaining=$((handshake_remaining - 1))
done
launcher_matches || { reason=unproven-launcher; exit 3; }
launcher_script_verified=1
printf '%s|%s|%s\n' "$launcher_pid" "$launcher_ticks" "$launcher_exe" >"$logs/launcher.identity"
printf '%s\n' "$token" >"$logs/watchdog.ready.pending"
mv "$logs/watchdog.ready.pending" "$logs/watchdog.ready"

while :; do
    discover_children
    [ "$recovery_complete" -eq 1 ] || { reason=watchdog-command-failure; break; }
    launcher_matches || { reason=launcher-exit; break; }
    [ "$(read_one "$logs/stop.request")" != "$token" ] || { reason=operator-stop; break; }
    elapsed=$(( $(now_seconds) - started ))
    if [ "$ready_seen" -eq 0 ]; then
        native_pid=$(read_one "$logs/native.pid")
        case "$native_pid" in *[!0-9]*|'') ;; *)
            if [ -r "$logs/lifecycle.jsonl" ] && awk -v pid="$native_pid" '$0 ~ /"event"[[:space:]]*:[[:space:]]*"ready"/ && $0 ~ ("\"process_id\"[[:space:]]*:[[:space:]]*" pid "([,}])") { found=1 } END { exit !found }' "$logs/lifecycle.jsonl"; then
                ready_seen=1
                printf '%s|%s|%s\n' "$token" "$native_pid" "$elapsed" >"$logs/watchdog.application-ready"
            fi
            ;;
        esac
    fi
    [ "$ready_seen" -ne 0 ] || [ "$elapsed" -lt "$init_timeout" ] || { reason=initialization-timeout; break; }
    [ "$elapsed" -lt "$session_timeout" ] || { reason=absolute-session-timeout; break; }
    sleep 1
done
cleanup
