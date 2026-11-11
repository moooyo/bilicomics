#!/bin/sh
# Name: BiliComics for Kindle
# The compatibility allowlist is intentionally empty until device acceptance.

set -eu
umask 077

BILICOMICS_HOME=${BILICOMICS_HOME:-/mnt/us/bilicomics}
APP_DIR=$BILICOMICS_HOME/app
DATA_DIR=$BILICOMICS_HOME/data
PROFILE_FILE=$DATA_DIR/device-profile
ALLOWLIST_FILE=$DATA_DIR/device-allowlist
LOCK_DIR=/tmp/bilicomics-launcher.lock

fail() {
    printf '%s\n' "bilicomics: $*" >&2
    exit 1
}

case "$BILICOMICS_HOME" in
    /*) ;;
    *) fail 'BILICOMICS_HOME must be an absolute path.' ;;
esac

[ -x "$APP_DIR/bilicomics" ] || fail 'The application executable is missing.'
[ -r "$PROFILE_FILE" ] || fail 'Configure data/device-profile before launching.'
[ -r "$ALLOWLIST_FILE" ] || fail 'Configure data/device-allowlist before launching.'

PROFILE=$(sed -n '1{s/\r$//;p;}' "$PROFILE_FILE")
case "$PROFILE" in
    pw5|pw6|scribe|scribe2025) ;;
    *) fail 'Unknown configured device profile.' ;;
esac

[ -r /etc/prettyversion.txt ] || fail 'Kindle firmware information is unavailable.'
FIRMWARE=$(sed -n 's/^Kindle[[:space:]]*\([0-9][0-9.]*\).*$/\1/p' /etc/prettyversion.txt | head -n 1)
[ -n "$FIRMWARE" ] || fail 'The firmware version could not be determined.'

# Match two literal fields; do not evaluate configuration as shell code.
if ! awk -v profile="$PROFILE" -v firmware="$FIRMWARE" '
    { sub(/\r$/, "") }
    /^[[:space:]]*#/ { next }
    NF == 2 && $1 == profile && $2 == firmware { found = 1 }
    END { exit !found }
' "$ALLOWLIST_FILE"; then
    fail "The configured profile/firmware combination has not been accepted: $PROFILE $FIRMWARE."
fi

BILICOMICS_APPROVED_PROFILE=$PROFILE
BILICOMICS_APPROVED_FIRMWARE=$FIRMWARE
export BILICOMICS_APPROVED_PROFILE BILICOMICS_APPROVED_FIRMWARE

[ -c /dev/fb0 ] || fail 'The framebuffer is unavailable.'
[ -r /dev/fb0 ] && [ -w /dev/fb0 ] || fail 'Framebuffer read/write access is required.'

mkdir -p "$DATA_DIR/logs" "$DATA_DIR/fonts"

# Return control to a document launcher before taking over the display.
if [ "${1:-}" != '--detached' ]; then
    SCRIPT_PATH=$(CDPATH= cd "$(dirname "$0")" && pwd)/$(basename "$0")
    export BILICOMICS_HOME
    if command -v setsid >/dev/null 2>&1; then
        setsid sh "$SCRIPT_PATH" --detached >/dev/null 2>&1 </dev/null &
    else
        nohup sh "$SCRIPT_PATH" --detached >/dev/null 2>&1 </dev/null &
    fi
    exit 0
fi

# Do not remove another launcher's lock or attempt to repair its service state.
mkdir "$LOCK_DIR" 2>/dev/null || fail 'Another launcher owns the display lock.'
printf '%s\n' "$$" >"$LOCK_DIR/pid"

APP_RUNNER_PID=''
APP_GENERATION=1
APP_PHASE=none
APP_STATUS=0
LAUNCH_TOKEN=''
EVENT_SOURCE_PID=''
EVENT_SOURCE_BIRTH=''
EVENT_FD_OPEN=0
CONTROL_FD_OPEN=0
LIFECYCLE_WAIT_PID=''
LIFECYCLE_SEQUENCE=0
LIFECYCLE_ACTION=''
WAKE_PROBE_PID=''
WAKE_PROBE_BIRTH=''
WAKE_PROBE_SEQUENCE=0
WAKE_PROBE_KIND=''
SLEEP_GENERATION=0
SLEEP_REQUESTED=0
WAKE_PENDING=0
WAKE_CONFIRMED=0
WAKE_DISMISS_GENERATION=0
WAKE_FALLBACK_ALLOWED=0
WAKE_FAST_PROBES_REMAINING=0
WAKE_PROBES_REMAINING=0
PAUSED_WM_PIDS=''
PAUSED_WM_IDENTITIES=''
STATUSBAR_STOPPED=0
STATUSBAR_QUIESCED=0
DISPLAY_OWNED=0
COVER_SLEEP_SUPPORTED=0
SLEEP_COVER_ACTIVE=0
SLEEP_COVER_SEQUENCE=0
NATIVE_READ_PID=''
NATIVE_READ_BIRTH=''
NATIVE_READ_SEQUENCE=0
NATIVE_READ_REAPED=1
NATIVE_MODE_SEQUENCE=0
NATIVE_MODE_PENDING=0
NATIVE_EPDC_MODE=''
NATIVE_PORTRAIT_ORIENTATION=''
NATIVE_PORTRAIT_ROTATION=''
NATIVE_CAPTURE_STATUS=unknown
NATIVE_RESUME_PRESENTATION=''
NATIVE_CAPABILITY_VALID=0
NATIVE_CAPABILITY_APP=''
NATIVE_CAPABILITY_APP_BIRTH=''
NATIVE_CAPABILITY_RUNNER_BIRTH=''
NATIVE_CAPABILITY_DIRECTORY=''
NATIVE_CAPABILITY_FILE=''
NATIVE_NOTICE_PID=''
NATIVE_NOTICE_BIRTH=''
LAUNCHER_LOG_PID=''
LAUNCHER_LOG_BIRTH=''
APPLICATION_LOG_PID=''
APPLICATION_LOG_BIRTH=''
LOG_OUTPUT_OPEN=0
LOG_CLOSING=0
LOG_BYTE_LIMIT=1048576

launch_trace() {
    [ "${BILICOMICS_LAUNCH_TRACE:-0}" = 1 ] || return 0
    case "$1" in *[!a-z_]*|'') return 0 ;; esac
    trace_path=$DATA_DIR/logs/launcher-timing.tsv
    [ ! -L "$trace_path" ] || return 0
    if [ -e "$trace_path" ] && [ ! -f "$trace_path" ]; then return 0; fi
    IFS=' ' read -r trace_uptime trace_unused </proc/uptime || return 0
    case "$trace_uptime" in *[!0-9.]*|'') return 0 ;; esac
    [ "${#trace_uptime}" -le 32 ] || return 0
    IFS= read -r trace_process </proc/self/stat || return 0
    trace_pid=${trace_process%% *}
    case "$trace_pid" in *[!0-9]*|'') return 0 ;; esac
    printf '%s\t%s\tpid=%s\tphase=%s\tsleep=%s\tcapture=%s\tprobe=%s\tlifecycle=%s\tmode=%s\tread_sequence=%s\n' \
        "$trace_uptime" "$1" "$trace_pid" "$APP_PHASE" "$SLEEP_GENERATION" "$NATIVE_MODE_SEQUENCE" "$WAKE_PROBE_SEQUENCE" "$LIFECYCLE_SEQUENCE" "${NATIVE_EPDC_MODE:-unknown}" "$NATIVE_READ_SEQUENCE" 2>/dev/null >>"$trace_path" || :
}

read_process() {
    case "$1" in *[!0-9]*|'') return 1 ;; esac
    IFS= read -r process_stat <"/proc/$1/stat" || return 1
    [ "${process_stat%% *}" = "$1" ] || return 1
    process_tail=${process_stat##*) }
    [ "$process_tail" != "$process_stat" ] || return 1
    set -- $process_tail
    [ "$#" -ge 20 ] || return 1
    PROCESS_STATE=$1
    PROCESS_PARENT=$2
    PROCESS_GROUP=$3
    PROCESS_SESSION=$4
    shift 19
    case "$1" in *[!0-9]*|'') return 1 ;; esac
    PROCESS_BIRTH=$1
}

process_birth() {
    read_process "$1" || return 1
    printf '%s\n' "$PROCESS_BIRTH"
}

process_parent() {
    read_process "$1" || return 1
    printf '%s\n' "$PROCESS_PARENT"
}

log_leaf_identity() {
    [ ! -L "$1" ] && [ -f "$1" ] || return 1
    LOG_IDENTITY=$(LC_ALL=C stat -c '%d:%i:%h:%s' "$1" 2>/dev/null) || return 1
    log_fields=$LOG_IDENTITY
    log_fields=${log_fields#*:}
    log_fields=${log_fields#*:}
    [ "${log_fields%%:*}" = 1 ] || return 1
    LOG_SIZE=${log_fields#*:}
    case "$LOG_SIZE" in *[!0-9]*|'') return 1 ;; esac
}

prepare_log() {
    log_path=$DATA_DIR/logs/$1.log
    log_backup=$log_path.1
    [ ! -L "$log_path" ] && [ ! -L "$log_backup" ] || return 1
    if [ -e "$log_backup" ]; then
        log_leaf_identity "$log_backup" || return 1
        if [ "$LOG_SIZE" -gt "$LOG_BYTE_LIMIT" ]; then
            log_previous=$LOG_IDENTITY
            tail -c "$LOG_BYTE_LIMIT" "$log_backup" >"$LOCK_DIR/log-$1-archive.pending" || return 1
            log_leaf_identity "$log_backup" && [ "$LOG_IDENTITY" = "$log_previous" ] || return 1
            mv -f "$LOCK_DIR/log-$1-archive.pending" "$log_backup" || return 1
        fi
    fi
    if [ ! -e "$log_path" ]; then
        ( set -C; : >"$log_path" ) || return 1
    fi
    log_leaf_identity "$log_path" || return 1
    if [ "$LOG_SIZE" -ge "$LOG_BYTE_LIMIT" ]; then
        log_previous=$LOG_IDENTITY
        tail -c "$LOG_BYTE_LIMIT" "$log_path" >"$LOCK_DIR/log-$1-archive.pending" || return 1
        log_leaf_identity "$log_path" && [ "$LOG_IDENTITY" = "$log_previous" ] || return 1
        mv -f "$LOCK_DIR/log-$1-archive.pending" "$log_backup" || return 1
        : >"$LOCK_DIR/log-$1-empty.pending" || return 1
        mv -f "$LOCK_DIR/log-$1-empty.pending" "$log_path" || return 1
    fi
}

log_sink_is_owned() {
    [ -n "$1" ] && [ -n "$2" ] || return 1
    read_process "$1" 2>/dev/null || return 1
    [ "$PROCESS_PARENT" = "$$" ] && [ "$PROCESS_BIRTH" = "$2" ] &&
        [ "$PROCESS_GROUP:$PROCESS_SESSION" = "$1:$1" ]
}

start_log_sink() {
    log_name=$1
    log_path=$DATA_DIR/logs/$log_name.log
    log_remaining=0
    log_expected=''
    if log_leaf_identity "$log_path" && [ "$LOG_SIZE" -le "$LOG_BYTE_LIMIT" ]; then
        log_expected=${LOG_IDENTITY%:*}
        log_remaining=$((LOG_BYTE_LIMIT - LOG_SIZE))
    fi
    rm -f "$LOCK_DIR/log-$log_name-ready" "$LOCK_DIR/log-$log_name-ready.pending"
    setsid sh -c '
        fifo=$1
        path=$2
        expected=$3
        remaining=$4
        ready=$5
        header=$6
        quota=$7
        name=$8
        directory=$9
        trap "" HUP INT TERM
        exec 9>"$directory/power-events" || exit 1
        log_sink_exit() {
            sink_status=$?
            trap - 0
            stop_record=$(cat "$directory/log-stopping" 2>/dev/null || :)
            [ "$stop_record" != "$header" ] || return 0
            IFS= read -r sink_stat </proc/self/stat || return 0
            sink_pid=${sink_stat%% *}
            sink_tail=${sink_stat##*) }
            set -- $sink_tail
            [ "$#" -ge 20 ] || return 0
            shift 19
            sink_birth=$1
            printf "%s:%s:%s:%s\n" "$header" "$sink_pid" "$sink_birth" "$sink_status" >"$directory/log-$name-exit.pending" &&
                mv -f "$directory/log-$name-exit.pending" "$directory/log-$name-exit" || return 0
            printf "_bilicomics_log_sink_exit %s %s %s %s\n" "$header" "$name" "$sink_pid" "$sink_birth" >&9 || :
        }
        trap log_sink_exit 0
        # The wrapper retains the FIFO read descriptor while head or cat exits.
        # Write failures stop disk logging, but continue draining application I/O.
        exec 7<"$fifo" || exit 1
        sink_wrapper_pid=$$
        if (
            trap - 0
            [ -n "$expected" ] && [ ! -L "$path" ] && [ -f "$path" ] || exit 1
            # A POSIX special-builtin redirection failure may exit this child.
            # The outer wrapper retains the FIFO and always falls back to drain.
            exec 8>>"$path" || exit 1
            opened=$(LC_ALL=C stat -Lc "%d:%i:%h" /proc/self/fd/8 2>/dev/null || :)
            [ "$opened" = "$expected" ] || exit 1
            printf "%s:%s:ok\n" "$header" "$sink_wrapper_pid" >"$ready.pending" && mv -f "$ready.pending" "$ready" || exit 1
            if [ "$remaining" -gt 0 ]; then head -c "$remaining" <&7 >&8 2>/dev/null || exit 1; fi
            # This fixed-size marker does not consume any of the log byte quota.
            if [ "$(LC_ALL=C stat -Lc %s /proc/self/fd/8 2>/dev/null || :)" = 1048576 ]; then
                printf "%s\n" "$header" >"$quota" 2>/dev/null || :
            fi
        ) 9>&-; then :; else
            printf "%s:%s:discard\n" "$header" "$sink_wrapper_pid" >"$ready.pending" && mv -f "$ready.pending" "$ready" || exit 1
        fi
        if cat <&7 >/dev/null 2>/dev/null 9>&-; then exit 0; else exit 1; fi
    ' sh "$LOCK_DIR/log-$log_name-pipe" "$log_path" "$log_expected" "$log_remaining" \
        "$LOCK_DIR/log-$log_name-ready" "$APP_GENERATION:$LAUNCH_TOKEN" "$LOCK_DIR/log-$log_name-quota" "$log_name" "$LOCK_DIR" \
        >/dev/null 2>&1 3>&- 4>&- 5>&- 6>&- &
    log_pid=$!
    log_birth=''
    log_checks=100
    while [ "$log_checks" -gt 0 ]; do
        if read_process "$log_pid" 2>/dev/null && [ "$PROCESS_PARENT" = "$$" ]; then
            if [ -z "$log_birth" ]; then log_birth=$PROCESS_BIRTH; fi
            [ "$PROCESS_BIRTH" = "$log_birth" ] || break
            case "$PROCESS_STATE" in Z|X) break ;; esac
            if [ "$PROCESS_GROUP:$PROCESS_SESSION" = "$log_pid:$log_pid" ]; then
                log_ready=''
                if [ -r "$LOCK_DIR/log-$log_name-ready" ]; then
                    log_ready=$(cat "$LOCK_DIR/log-$log_name-ready" 2>/dev/null || :)
                fi
                case "$log_ready" in
                    "$APP_GENERATION:$LAUNCH_TOKEN:$log_pid:ok"|"$APP_GENERATION:$LAUNCH_TOKEN:$log_pid:discard") break ;;
                esac
            fi
        fi
        log_checks=$((log_checks - 1))
        usleep 10000
    done
    case "$log_name" in
        launcher) LAUNCHER_LOG_PID=$log_pid; LAUNCHER_LOG_BIRTH=$log_birth ;;
        application) APPLICATION_LOG_PID=$log_pid; APPLICATION_LOG_BIRTH=$log_birth ;;
    esac
    log_sink_is_owned "$log_pid" "$log_birth" || return 1
    printf '%s:%s:%s:%s\n' "$APP_GENERATION" "$LAUNCH_TOKEN" "$log_pid" "$log_birth" >"$LOCK_DIR/log-$log_name-owned.pending" || return 1
    mv -f "$LOCK_DIR/log-$log_name-owned.pending" "$LOCK_DIR/log-$log_name-owned" || return 1
    case "${log_ready:-}" in
        "$APP_GENERATION:$LAUNCH_TOKEN:$log_pid:ok"|"$APP_GENERATION:$LAUNCH_TOKEN:$log_pid:discard") return 0 ;;
    esac
    return 1
}

ensure_log_sinks() {
    [ "$LOG_OUTPUT_OPEN" -eq 1 ] || return 0
    [ "$LOG_CLOSING" -eq 0 ] || return 0
    if ! log_sink_is_owned "$LAUNCHER_LOG_PID" "$LAUNCHER_LOG_BIRTH" || [ "$PROCESS_STATE" = Z ] || [ "$PROCESS_STATE" = X ]; then
        [ -z "$LAUNCHER_LOG_PID" ] || wait "$LAUNCHER_LOG_PID" 2>/dev/null || :
        start_log_sink launcher || return 1
    fi
    if ! log_sink_is_owned "$APPLICATION_LOG_PID" "$APPLICATION_LOG_BIRTH" || [ "$PROCESS_STATE" = Z ] || [ "$PROCESS_STATE" = X ]; then
        [ -z "$APPLICATION_LOG_PID" ] || wait "$APPLICATION_LOG_PID" 2>/dev/null || :
        start_log_sink application || return 1
    fi
}

handle_log_sink_exit() {
    [ "$LOG_CLOSING" -eq 0 ] || return 0
    set -- $power_event
    [ "$#" -eq 5 ] && [ "$2" = "$APP_GENERATION:$LAUNCH_TOKEN" ] || return 0
    case "$3" in
        launcher) notice_log_pid=$LAUNCHER_LOG_PID; notice_log_birth=$LAUNCHER_LOG_BIRTH ;;
        application) notice_log_pid=$APPLICATION_LOG_PID; notice_log_birth=$APPLICATION_LOG_BIRTH ;;
        *) return 0 ;;
    esac
    [ "$4:$5" = "$notice_log_pid:$notice_log_birth" ] || return 0
    log_exit_record=$(cat "$LOCK_DIR/log-$3-exit" 2>/dev/null || :)
    case "$log_exit_record" in "$APP_GENERATION:$LAUNCH_TOKEN:$notice_log_pid:$notice_log_birth:"*) ;; *) return 0 ;; esac
    case "${log_exit_record##*:}" in *[!0-9]*|'') return 0 ;; esac
    # The exact wrapper has finished its consumer. Bound its final exit and
    # promptly replace the drain even when no native power event follows.
    stop_log_sink "$notice_log_pid" "$notice_log_birth"
    start_log_sink "$3" || fail 'The bounded production log drain could not be restarted.'
}

stop_log_sink() {
    log_stop_checks=20
    while log_sink_is_owned "$1" "$2" && [ "$log_stop_checks" -gt 0 ]; do
        case "$PROCESS_STATE" in Z|X) break ;; esac
        log_stop_checks=$((log_stop_checks - 1))
        usleep 10000
    done
    if log_sink_is_owned "$1" "$2"; then
        kill -KILL "-$1" 2>/dev/null || :
    elif [ -n "$1" ] && read_process "$1" 2>/dev/null && [ "$PROCESS_PARENT" = "$$" ] && [ "$PROCESS_BIRTH" = "$2" ]; then
        kill -KILL "$1" 2>/dev/null || :
    fi
    [ -z "$1" ] || wait "$1" 2>/dev/null || :
}

start_logging() {
    [ ! -L "$DATA_DIR/logs" ] && [ -d "$DATA_DIR/logs" ] || return 1
    prepare_log launcher && prepare_log application || return 1
    mkfifo "$LOCK_DIR/log-launcher-pipe" "$LOCK_DIR/log-application-pipe" || return 1
    # Keep a read end until cleanup. An unexpectedly terminated sink cannot
    # deliver SIGPIPE to the App; the event/cleanup path restarts its drain.
    exec 5<>"$LOCK_DIR/log-launcher-pipe"
    exec 6<>"$LOCK_DIR/log-application-pipe"
    start_log_sink launcher && start_log_sink application || return 1
    exec 1>&5 2>&5
    LOG_OUTPUT_OPEN=1
}

stop_logging() {
    exec 1>/dev/null 2>&1
    exec 5>&- 6>&-
    LOG_OUTPUT_OPEN=0
    stop_log_sink "$APPLICATION_LOG_PID" "$APPLICATION_LOG_BIRTH"
    stop_log_sink "$LAUNCHER_LOG_PID" "$LAUNCHER_LOG_BIRTH"
    rm -f "$LOCK_DIR/log-launcher-pipe" "$LOCK_DIR/log-application-pipe" \
        "$LOCK_DIR/log-stopping" \
        "$LOCK_DIR/log-launcher-ready" "$LOCK_DIR/log-launcher-ready.pending" "$LOCK_DIR/log-launcher-quota" \
        "$LOCK_DIR/log-application-ready" "$LOCK_DIR/log-application-ready.pending" "$LOCK_DIR/log-application-quota" \
        "$LOCK_DIR/log-launcher-owned" "$LOCK_DIR/log-launcher-owned.pending" \
        "$LOCK_DIR/log-application-owned" "$LOCK_DIR/log-application-owned.pending" \
        "$LOCK_DIR/log-launcher-exit" "$LOCK_DIR/log-launcher-exit.pending" \
        "$LOCK_DIR/log-application-exit" "$LOCK_DIR/log-application-exit.pending" \
        "$LOCK_DIR/log-launcher-archive.pending" "$LOCK_DIR/log-application-archive.pending" \
        "$LOCK_DIR/log-launcher-empty.pending" "$LOCK_DIR/log-application-empty.pending"
}

native_read_is_owned() {
    [ -n "$NATIVE_READ_PID" ] && [ -n "$NATIVE_READ_BIRTH" ] || return 1
    read_process "$NATIVE_READ_PID" 2>/dev/null || return 1
    [ "$PROCESS_PARENT" = "$$" ] && [ "$PROCESS_BIRTH" = "$NATIVE_READ_BIRTH" ]
}

native_read_group_is_owned() {
    native_read_is_owned || return 1
    [ "$PROCESS_GROUP:$PROCESS_SESSION" = "$NATIVE_READ_PID:$NATIVE_READ_PID" ]
}

stop_native_read() {
    NATIVE_READ_REAPED=0
    if native_read_group_is_owned; then
        # The verified leader remains alive after the getter exits. Kill its
        # entire private group so a timed-out descendant cannot reply later.
        kill -KILL "-$NATIVE_READ_PID" 2>/dev/null || :
    elif native_read_is_owned; then
        # Before setsid completes the gated wrapper has not started an IPC.
        kill -KILL "$NATIVE_READ_PID" 2>/dev/null || :
    fi
    native_read_reap=10
    while native_read_is_owned && [ "$native_read_reap" -gt 0 ]; do
        case "$PROCESS_STATE" in
            Z|X) break ;;
        esac
        usleep 100000
        native_read_reap=$((native_read_reap - 1))
    done
    if [ -n "$NATIVE_READ_PID" ]; then
        if ! read_process "$NATIVE_READ_PID" 2>/dev/null || [ "$PROCESS_STATE" = Z ] || [ "$PROCESS_STATE" = X ]; then
            wait "$NATIVE_READ_PID" 2>/dev/null || :
            NATIVE_READ_REAPED=1
        fi
    else
        NATIVE_READ_REAPED=1
    fi
    if [ "$NATIVE_READ_REAPED" -eq 1 ]; then
        NATIVE_READ_PID=''
        NATIVE_READ_BIRTH=''
    fi
    rm -f "$LOCK_DIR/native-read-done" "$LOCK_DIR/native-read-done.pending" "$LOCK_DIR/native-read-command" "$LOCK_DIR/native-read-command.pending" "$LOCK_DIR/native-read-go" "$LOCK_DIR/native-read-go.pending"
}

native_read_command() {
    rm -f "$LOCK_DIR/native-read-output" "$LOCK_DIR/native-read-done" "$LOCK_DIR/native-read-done.pending" "$LOCK_DIR/native-read-command" "$LOCK_DIR/native-read-command.pending" "$LOCK_DIR/native-read-go" "$LOCK_DIR/native-read-go.pending" || return 1
    NATIVE_READ_SEQUENCE=$((NATIVE_READ_SEQUENCE + 1))
    setsid sh -c '
        completed=$1
        permission=$2
        sequence=$3
        token=$4
        command_record=$5
        shift 5
        wrapper_pid=$$
        # Keep a finite lifetime even if the supervisor itself is killed.
        (
            IFS= read -r watchdog_stat </proc/self/stat || exit 1
            watchdog_pid=${watchdog_stat%% *}
            watchdog_tail=${watchdog_stat##*) }
            set -- $watchdog_tail
            [ "$#" -ge 20 ] && [ "$3:$4" = "$wrapper_pid:$wrapper_pid" ] || exit 1
            shift 19
            watchdog_birth=$1
            sleep 2
            IFS= read -r watchdog_stat </proc/self/stat || exit 1
            [ "${watchdog_stat%% *}" = "$watchdog_pid" ] || exit 1
            watchdog_tail=${watchdog_stat##*) }
            set -- $watchdog_tail
            [ "$#" -ge 20 ] && [ "$3:$4" = "$wrapper_pid:$wrapper_pid" ] || exit 1
            shift 19
            [ "$1" = "$watchdog_birth" ] || exit 1
            kill -KILL "-$wrapper_pid" 2>/dev/null || exit 1
        ) &
        while [ "$(cat "$permission" 2>/dev/null || :)" != "$wrapper_pid:$sequence:$token" ]; do
            usleep 10000
        done
        # Capture one overflow byte beyond the record budget. The exact parser
        # rejects that entire response, including a plausible valid prefix.
        # Bound stdout without limiting unrelated files the getter may use.
        collector_status=0
        (
            if "$@"; then child_status=0; else child_status=$?; fi
            printf "%s:%s:%s\n" "$sequence" "$token" "$child_status" >"$command_record.pending"
            mv -f "$command_record.pending" "$command_record"
        ) | dd bs=1 count=129 2>/dev/null || collector_status=1
        command_status=1
        if [ "$collector_status" -eq 0 ] && [ "$(cat "$command_record" 2>/dev/null || :)" = "$sequence:$token:0" ]; then
            command_status=0
        fi
        printf "%s:%s:%s\n" "$sequence" "$token" "$command_status" >"$completed.pending"
        mv -f "$completed.pending" "$completed"
        while :; do sleep 1; done
    ' sh "$LOCK_DIR/native-read-done" "$LOCK_DIR/native-read-go" "$NATIVE_READ_SEQUENCE" "$LAUNCH_TOKEN" "$LOCK_DIR/native-read-command" "$@" >"$LOCK_DIR/native-read-output" 2>/dev/null 3>&- 4>&- 5>&- 6>&- &
    NATIVE_READ_PID=$!
    NATIVE_READ_BIRTH=''
    native_read_checks=20
    native_read_status=1
    native_read_released=0
    while [ "$native_read_checks" -gt 0 ]; do
        read_process "$NATIVE_READ_PID" 2>/dev/null || break
        [ "$PROCESS_PARENT" = "$$" ] || break
        if [ -z "$NATIVE_READ_BIRTH" ]; then NATIVE_READ_BIRTH=$PROCESS_BIRTH; fi
        [ "$PROCESS_BIRTH" = "$NATIVE_READ_BIRTH" ] || break
        case "$PROCESS_STATE" in
            Z|X) break ;;
        esac
        if [ "$PROCESS_GROUP:$PROCESS_SESSION" = "$NATIVE_READ_PID:$NATIVE_READ_PID" ]; then
            if [ "$native_read_released" -eq 0 ]; then
                if ! { printf '%s:%s:%s\n' "$NATIVE_READ_PID" "$NATIVE_READ_SEQUENCE" "$LAUNCH_TOKEN" >"$LOCK_DIR/native-read-go.pending" &&
                    mv -f "$LOCK_DIR/native-read-go.pending" "$LOCK_DIR/native-read-go"; }; then
                    break
                fi
                native_read_released=1
                launch_trace native_read_go
            fi
            if [ -r "$LOCK_DIR/native-read-done" ]; then
                native_read_done=$(cat "$LOCK_DIR/native-read-done" 2>/dev/null || :)
                if [ "$native_read_done" = "$NATIVE_READ_SEQUENCE:$LAUNCH_TOKEN:0" ]; then
                    native_read_status=0
                    launch_trace native_read_ready
                fi
                break
            fi
        fi
        native_read_checks=$((native_read_checks - 1))
        usleep 100000
    done
    stop_native_read
    if [ "$NATIVE_READ_REAPED" -eq 1 ]; then
        launch_trace native_read_reaped
    else
        native_read_status=1
        launch_trace native_read_cleanup_pending
    fi
    return "$native_read_status"
}

native_read_hex() {
    native_hex_words=$(od -An -v -tx1 "$LOCK_DIR/native-read-output") || return 1
    for native_hex_word in $native_hex_words; do printf '%s' "$native_hex_word"; done
}

read_capability_process() {
    case "$1" in *[!0-9]*|'') return 1 ;; esac
    IFS= read -r capability_process_stat <"/proc/$1/stat" || return 1
    [ "${capability_process_stat%% *}" = "$1" ] || return 1
    capability_process_tail=${capability_process_stat##*) }
    [ "$capability_process_tail" != "$capability_process_stat" ] || return 1
    set -- $capability_process_tail
    [ "$#" -ge 20 ] || return 1
    CAPABILITY_PROCESS_STATE=$1
    CAPABILITY_PROCESS_PARENT=$2
    shift 19
    CAPABILITY_PROCESS_BIRTH=$1
    case "$CAPABILITY_PROCESS_STATE:$CAPABILITY_PROCESS_BIRTH" in Z:*|X:*|*:*[!0-9]*|*:) return 1 ;; esac
}

capability_context_matches() {
    # These private binding markers are written only by this owning launcher.
    # The App is the final no-follow/strict-record authority before GETVAR or I/O.
    [ -f "$LOCK_DIR/nonce" ] && [ ! -L "$LOCK_DIR/nonce" ] || return 1
    [ -f "$LOCK_DIR/generation" ] && [ ! -L "$LOCK_DIR/generation" ] || return 1
    [ -f "$LOCK_DIR/application-pid" ] && [ ! -L "$LOCK_DIR/application-pid" ] || return 1
    IFS= read -r capability_nonce <"$LOCK_DIR/nonce" || return 1
    IFS= read -r capability_generation <"$LOCK_DIR/generation" || return 1
    IFS= read -r capability_application <"$LOCK_DIR/application-pid" || return 1
    [ "$capability_nonce" = "$LAUNCH_TOKEN" ] && [ "$capability_generation" = "$APP_GENERATION:$LAUNCH_TOKEN" ] || return 1
    [ "$capability_application" = "$NATIVE_CAPABILITY_APP" ] || return 1
    [ "$capability_application" != "$APP_RUNNER_PID" ] && [ "$capability_application" != "$$" ] || return 1
    read_capability_process "$capability_application" || return 1
    [ "$CAPABILITY_PROCESS_PARENT" = "$APP_RUNNER_PID" ] && [ "$CAPABILITY_PROCESS_BIRTH" = "$NATIVE_CAPABILITY_APP_BIRTH" ] || return 1
    read_capability_process "$APP_RUNNER_PID" || return 1
    [ "$CAPABILITY_PROCESS_PARENT" = "$$" ] && [ "$CAPABILITY_PROCESS_BIRTH" = "$NATIVE_CAPABILITY_RUNNER_BIRTH" ]
}

framebuffer_capability_is_current() {
    [ "$NATIVE_CAPABILITY_VALID" -eq 1 ] || return 1
    capability_context_matches || return 1
    [ ! -L "$LOCK_DIR" ] && [ ! -L "$LOCK_DIR/native-output-capability" ] && [ -f "$LOCK_DIR/native-output-capability" ] || return 1
    capability_directory=$(LC_ALL=C stat -c '%d:%i:%u:%f' "$LOCK_DIR" 2>/dev/null) || return 1
    capability_file=$(LC_ALL=C stat -c '%d:%i:%u:%f:%h:%s:%Y:%Z' "$LOCK_DIR/native-output-capability" 2>/dev/null) || return 1
    [ -n "$capability_directory" ] && [ "$capability_directory" = "$NATIVE_CAPABILITY_DIRECTORY" ] &&
        [ -n "$capability_file" ] && [ "$capability_file" = "$NATIVE_CAPABILITY_FILE" ] || return 1
    capability_context_matches
}

cache_framebuffer_capability() {
    # This is a conservative feature candidate, never a cached output mode.
    # Read it off the wake critical path after the App has suspended. A private,
    # bounded helper contains a capability path-replacement/FIFO read race. The App
    # independently repeats no-follow capability validation before fresh GETVAR.
    NATIVE_CAPABILITY_VALID=0
    command -v stat >/dev/null 2>&1 || return 0
    [ ! -L "$LOCK_DIR" ] && [ ! -L "$LOCK_DIR/native-output-capability" ] && [ -f "$LOCK_DIR/native-output-capability" ] || return 0
    IFS= read -r NATIVE_CAPABILITY_APP <"$LOCK_DIR/application-pid" || return 0
    read_capability_process "$NATIVE_CAPABILITY_APP" || return 0
    [ "$CAPABILITY_PROCESS_PARENT" = "$APP_RUNNER_PID" ] || return 0
    NATIVE_CAPABILITY_APP_BIRTH=$CAPABILITY_PROCESS_BIRTH
    read_capability_process "$APP_RUNNER_PID" || return 0
    [ "$CAPABILITY_PROCESS_PARENT" = "$$" ] || return 0
    NATIVE_CAPABILITY_RUNNER_BIRTH=$CAPABILITY_PROCESS_BIRTH
    capability_context_matches || return 0
    NATIVE_CAPABILITY_DIRECTORY=$(LC_ALL=C stat -c '%d:%i:%u:%f' "$LOCK_DIR" 2>/dev/null) || return 0
    NATIVE_CAPABILITY_FILE=$(LC_ALL=C stat -c '%d:%i:%u:%f:%h:%s:%Y:%Z' "$LOCK_DIR/native-output-capability" 2>/dev/null) || return 0
    capability_directory_tail=${NATIVE_CAPABILITY_DIRECTORY#*:*:}
    capability_owner=${capability_directory_tail%%:*}
    capability_effective_owner=$(id -u 2>/dev/null) || return 0
    [ -n "$capability_effective_owner" ] && [ "$capability_owner" = "$capability_effective_owner" ] || return 0
    # SGID may be inherited from /tmp without granting group access.
    case "${capability_directory_tail#*:}" in 41c0|45c0) ;; *) return 0 ;; esac
    capability_file_tail=${NATIVE_CAPABILITY_FILE#*:*:}
    [ "${capability_file_tail%%:*}" = "$capability_owner" ] || return 0
    capability_file_tail=${capability_file_tail#*:}
    case "${capability_file_tail%%:*}" in 8180|8100) ;; *) return 0 ;; esac
    capability_file_tail=${capability_file_tail#*:}
    [ "${capability_file_tail%%:*}" = 1 ] || return 0
    capability_file_tail=${capability_file_tail#*:}
    capability_size=${capability_file_tail%%:*}
    case "$capability_size" in *[!0-9]*|'') return 0 ;; esac
    [ "$capability_size" -gt 0 ] && [ "$capability_size" -le 128 ] || return 0
    capability_expected=$APP_GENERATION:$LAUNCH_TOKEN:$NATIVE_CAPABILITY_APP:framebuffer8
    [ "$capability_size" -eq "$((${#capability_expected} + 1))" ] || return 0
    if native_read_command sh -c '
        [ -f "$1" ] && [ ! -L "$1" ] || exit 1
        IFS= read -r line <"$1" || exit 1
        [ "$line" = "$2" ] || exit 1
        # The exact byte count requires one LF and rejects extra or binary data.
        [ "$3" -eq "$((${#line} + 1))" ] || exit 1
        printf "framebuffer8\n"
    ' sh "$LOCK_DIR/native-output-capability" "$capability_expected" "$capability_size"; then
        case "$(native_read_hex)" in
            6672616d65627566666572380a)
                NATIVE_CAPABILITY_VALID=1
                if framebuffer_capability_is_current; then launch_trace native_capability_framebuffer; else NATIVE_CAPABILITY_VALID=0; fi
                ;;
        esac
    fi
    return 0
}

capture_native_epdc_mode() {
    NATIVE_EPDC_MODE=''
    NATIVE_PORTRAIT_ORIENTATION=''
    NATIVE_PORTRAIT_ROTATION=''
    NATIVE_CAPTURE_STATUS=unknown
    # Display mode and portrait calibration share one proven group and one
    # two-second deadline. One application helper validates its protected read
    # context before native I/O and performs the complete sampling transaction.
    # The exact compact-record parser remains the launcher acceptance boundary.
    if ! BILICOMICS_LIFECYCLE_DIR="$LOCK_DIR" \
        BILICOMICS_LAUNCH_GENERATION="$APP_GENERATION" \
        BILICOMICS_LAUNCH_TOKEN="$LAUNCH_TOKEN" \
        native_read_command "$APP_DIR/bilicomics" --capture-native-display --kindle --device "$PROFILE"; then
        return 1
    fi
    native_capture_hex=$(native_read_hex) || return 1
    case "$native_capture_hex" in
        72656164793a59383a*) NATIVE_EPDC_MODE=Y8; native_portrait_hex=${native_capture_hex#72656164793a59383a} ;;
        72656164793a5938494e563a*) NATIVE_EPDC_MODE=Y8INV; native_portrait_hex=${native_capture_hex#72656164793a5938494e563a} ;;
        6e6f742d6163746976650a) NATIVE_CAPTURE_STATUS=not-active; return 0 ;;
        *) return 1 ;;
    esac
    case "$native_portrait_hex" in
        553a3[0-3]0a) NATIVE_PORTRAIT_ORIENTATION=U; native_rotation_hex=${native_portrait_hex#553a} ;;
        443a3[0-3]0a) NATIVE_PORTRAIT_ORIENTATION=D; native_rotation_hex=${native_portrait_hex#443a} ;;
        46495845443a3[0-3]0a) NATIVE_PORTRAIT_ORIENTATION=FIXED; native_rotation_hex=${native_portrait_hex#46495845443a} ;;
        *) NATIVE_EPDC_MODE=''; return 1 ;;
    esac
    case "$native_rotation_hex" in
        300a) NATIVE_PORTRAIT_ROTATION=0 ;;
        310a) NATIVE_PORTRAIT_ROTATION=1 ;;
        320a) NATIVE_PORTRAIT_ROTATION=2 ;;
        330a) NATIVE_PORTRAIT_ROTATION=3 ;;
        *) NATIVE_EPDC_MODE=''; NATIVE_PORTRAIT_ORIENTATION=''; return 1 ;;
    esac
    NATIVE_CAPTURE_STATUS=known
}

publish_native_epdc_mode() {
    published_native_mode=$NATIVE_EPDC_MODE
    if [ "$1" = resume-native-epdc-mode ] && [ "$NATIVE_RESUME_PRESENTATION" = framebuffer ]; then
        published_native_mode=FRAMEBUFFER
    fi
    case "$published_native_mode" in Y8|Y8INV) ;; FRAMEBUFFER) [ "$1" = resume-native-epdc-mode ] || return 1 ;; *) return 1 ;; esac
    if [ "$1" = resume-native-epdc-mode ]; then
        printf '%s:%s:%s:%s\n' "$APP_GENERATION" "$LAUNCH_TOKEN" "$LIFECYCLE_SEQUENCE" "$published_native_mode" >"$LOCK_DIR/$1.pending"
    else
        printf '%s:%s:%s\n' "$APP_GENERATION" "$LAUNCH_TOKEN" "$NATIVE_EPDC_MODE" >"$LOCK_DIR/$1.pending"
    fi
    mv -f "$LOCK_DIR/$1.pending" "$LOCK_DIR/$1"
}

publish_native_portrait_orientation() {
    case "$1" in
        native-portrait-orientation)
            case "$NATIVE_PORTRAIT_ORIENTATION" in U|D|FIXED) ;; *) return 1 ;; esac
            case "$NATIVE_PORTRAIT_ROTATION" in 0|1|2|3) ;; *) return 1 ;; esac
            native_portrait_payload=$NATIVE_PORTRAIT_ORIENTATION:$NATIVE_PORTRAIT_ROTATION
            ;;
        resume-native-portrait-orientation)
            # This is permission for a fresh read-only kernel observation bound
            # to this exact transition, never a cached startup direction.
            native_portrait_payload=CALIBRATED:$LIFECYCLE_SEQUENCE
            ;;
        *) return 1 ;;
    esac
    # The application publishes only these typed leaves through its pinned
    # private directory descriptor. Exclusive no-follow creation and a
    # no-replace rename must succeed; shell redirection never opens a marker.
    # The same native-reader lease bounds the writer and proves its ancestry.
    if ! native_read_command sh -c '
        BILICOMICS_LIFECYCLE_DIR=$1
        BILICOMICS_LAUNCH_GENERATION=$2
        BILICOMICS_LAUNCH_TOKEN=$3
        export BILICOMICS_LIFECYCLE_DIR BILICOMICS_LAUNCH_GENERATION BILICOMICS_LAUNCH_TOKEN
        exec "$4" --publish-native-portrait-orientation "$5" --kindle --device "$6"
    ' sh "$LOCK_DIR" "$APP_GENERATION" "$LAUNCH_TOKEN" "$APP_DIR/bilicomics" "$native_portrait_payload" "$PROFILE"; then
        return 1
    fi
    [ "$(native_read_hex)" = 5055424c49534845440a ]
}

stop_native_notice() {
    [ -n "$NATIVE_NOTICE_PID" ] || return 0
    if [ -z "$NATIVE_NOTICE_BIRTH" ] && [ "$(process_parent "$NATIVE_NOTICE_PID" 2>/dev/null || :)" = "$$" ]; then
        NATIVE_NOTICE_BIRTH=$(process_birth "$NATIVE_NOTICE_PID" 2>/dev/null || :)
    fi
    if [ "$(process_parent "$NATIVE_NOTICE_PID" 2>/dev/null || :)" = "$$" ] &&
        [ -n "$NATIVE_NOTICE_BIRTH" ] &&
        [ "$(process_birth "$NATIVE_NOTICE_PID" 2>/dev/null || :)" = "$NATIVE_NOTICE_BIRTH" ]; then
        kill -KILL "$NATIVE_NOTICE_PID" 2>/dev/null || :
    fi
    # This child has no descendants. Never wait for an unproven live PID;
    # its write-only control end also closes when native supervision ends.
    native_notice_reap=10
    while [ "$native_notice_reap" -gt 0 ]; do
        case "$(sed -n 's/^State:[[:space:]]*//p' "/proc/$NATIVE_NOTICE_PID/status" 2>/dev/null || :)" in
            Z*|X*|'') wait "$NATIVE_NOTICE_PID" 2>/dev/null || :; break ;;
        esac
        usleep 100000
        native_notice_reap=$((native_notice_reap - 1))
    done
    NATIVE_NOTICE_PID=''
    NATIVE_NOTICE_BIRTH=''
}

queue_native_notice() {
    stop_native_notice
    (
        trap - 0 HUP INT TERM
        # Send barriers through the same relay as native power records. The
        # parent immediately returns to the shared event reader under load.
        exec 3>&-
        exec 4>"$LOCK_DIR/power-control"
        printf '%s\n' "$1" >&4
    ) &
    NATIVE_NOTICE_PID=$!
    NATIVE_NOTICE_BIRTH=$(process_birth "$NATIVE_NOTICE_PID" 2>/dev/null || :)
}

signal_recorded_child() {
    child_record=$1
    child_signal=$2
    recorded_pid=$(sed -n '1p' "$child_record" 2>/dev/null || :)
    case "$recorded_pid" in
        *[!0-9]*|'') return 1 ;;
    esac
    recorded_parent=$(sed -n 's/^PPid:[[:space:]]*//p' "/proc/$recorded_pid/status" 2>/dev/null || :)
    [ "$recorded_parent" = "$3" ] || return 1
    kill "-$child_signal" "$recorded_pid" 2>/dev/null
}

configure_cover_sleep() {
    # Observe the existing framework while it is running. This does not alter
    # advertisements, native locks, powerd policy, or the system sleep image.
    COVER_SLEEP_SUPPORTED=0
    native_read_command lipc-get-prop -s com.lab126.blanket load || return 0
    cover_configuration_size=$(LC_ALL=C stat -c %s "$LOCK_DIR/native-read-output") || return 0
    [ "$cover_configuration_size" -gt 0 ] && [ "$cover_configuration_size" -le 128 ] || return 0
    # Validate raw bytes before shell substitution, which cannot retain NUL.
    cover_configuration_bytes=$(od -An -v -tu1 "$LOCK_DIR/native-read-output") || return 0
    for cover_byte in $cover_configuration_bytes; do
        case "$cover_byte" in 9|10|13) continue ;; esac
        [ "$cover_byte" -ge 32 ] && [ "$cover_byte" -le 126 ] || return 0
    done
    cover_configuration=$(cat "$LOCK_DIR/native-read-output") || return 0
    case "$cover_configuration" in ''|*ad_screensaver*) return 0 ;; esac
    case "$cover_configuration" in *[![:space:]]*) ;; *) return 0 ;; esac
    case "$cover_configuration" in *[![:print:][:space:]]*) return 0 ;; esac
    COVER_SLEEP_SUPPORTED=1
    return 0
}

cover_sleep_can_own_display() {
    [ "$COVER_SLEEP_SUPPORTED" -eq 1 ] && [ "$DISPLAY_OWNED" -eq 1 ] &&
        [ "$STATUSBAR_QUIESCED" -eq 1 ] && [ -n "$PAUSED_WM_PIDS" ] || return 1
    # Recheck native locking on every transition; an earlier permission never
    # authorizes painting over a password that was configured later.
    [ ! -e /var/local/system/userpasswdenabled ] &&
        [ ! -L /var/local/system/userpasswdenabled ] || return 1
    for cover_wm_pid in $PAUSED_WM_PIDS; do
        cover_wm_birth=''
        for cover_wm_identity in $PAUSED_WM_IDENTITIES; do
            if [ "${cover_wm_identity%%:*}" = "$cover_wm_pid" ]; then
                cover_wm_birth=${cover_wm_identity#*:}
                break
            fi
        done
        [ -n "$cover_wm_birth" ] && read_process "$cover_wm_pid" 2>/dev/null || return 1
        [ "$PROCESS_BIRTH" = "$cover_wm_birth" ] || return 1
        case "$PROCESS_STATE" in T|t) ;; *) return 1 ;; esac
    done
}

sleep_cover_records_match() {
    # Keep mutable-leaf reads inside the existing owned, bounded helper. Exact
    # byte counts reject trailing records; a FIFO replacement cannot hang Main.
    cover_expected=$APP_GENERATION:$LAUNCH_TOKEN:$SLEEP_COVER_SEQUENCE:suspend
    if native_read_command sh -c '
        for leaf in sleep-cover-request sleep-cover-ready; do
            path=$1/$leaf
            [ -f "$path" ] && [ ! -L "$path" ] || exit 1
            size=$(LC_ALL=C stat -c %s "$path") || exit 1
            [ "$size" = "$((${#2} + 1))" ] || exit 1
            IFS= read -r record <"$path" || exit 1
            [ "$record" = "$2" ] || exit 1
        done
        printf "cover-ready\n"
    ' sh "$LOCK_DIR" "$cover_expected"; then
        [ "$(native_read_hex)" = 636f7665722d72656164790a ]
    else
        return 1
    fi
}

cover_wake_power_is_active() {
    # The WM stays stopped in this branch. Only powerd is queried; the App will
    # obtain fresh kernel polarity and rotation before its first resumed frame.
    native_read_command lipc-get-prop -s com.lab126.powerd state || return 1
    case "$(native_read_hex)" in 616374697665|6163746976650a) ;; *) return 1 ;; esac
    native_read_command lipc-get-prop -s com.lab126.powerd state || return 1
    case "$(native_read_hex)" in 616374697665|6163746976650a) ;; *) return 1 ;; esac
}

fallback_cover_sleep() {
    SLEEP_COVER_ACTIVE=0
    NATIVE_MODE_PENDING=0
    NATIVE_RESUME_PRESENTATION=''
    WAKE_CONFIRMED=0
    WAKE_DISMISS_GENERATION=0
    WAKE_FALLBACK_ALLOWED=0
    release_display
    launch_trace cover_sleep_fallback
}

release_display() {
    [ "$DISPLAY_OWNED" -eq 1 ] || return 0
    for wm_pid in $PAUSED_WM_PIDS; do
        kill -CONT "$wm_pid" 2>/dev/null || :
    done
    PAUSED_WM_PIDS=''
    PAUSED_WM_IDENTITIES=''
    if [ "$STATUSBAR_STOPPED" -eq 1 ]; then
        start statusbar >/dev/null 2>&1 || :
    fi
    STATUSBAR_STOPPED=0
    STATUSBAR_QUIESCED=0
    if command -v xrefresh >/dev/null 2>&1; then
        xrefresh >/dev/null 2>&1 || :
    fi
    DISPLAY_OWNED=0
    launch_trace native_released
}

take_display() {
    [ "$DISPLAY_OWNED" -eq 0 ] || return 0
    DISPLAY_OWNED=1
    launch_trace native_take_begin
    if [ "${1:-classic}" = framebuffer ]; then pause_window_manager; fi
    # Each takeover records its own baseline. A sleep restores that baseline
    # only after the application has released native rendering and input.
    if command -v status >/dev/null 2>&1 && command -v stop >/dev/null 2>&1 && command -v start >/dev/null 2>&1; then
        case "$(status statusbar 2>/dev/null || :)" in
            *start/running*)
                if stop statusbar >/dev/null 2>&1; then
                    STATUSBAR_STOPPED=1
                    case "$(status statusbar 2>/dev/null || :)" in
                        *stop/waiting*) STATUSBAR_QUIESCED=1 ;;
                    esac
                fi
                ;;
            *stop/waiting*) STATUSBAR_QUIESCED=1 ;;
        esac
    fi
    if [ "${1:-classic}" != framebuffer ]; then pause_window_manager; fi
    launch_trace native_take_done
}

pause_window_manager() {
    if command -v pidof >/dev/null 2>&1; then
        for wm_pid in $(pidof awesome 2>/dev/null || :); do
            case "$wm_pid" in
                *[!0-9]*|'') continue ;;
            esac
            # A previously stopped process must remain stopped after exit.
            case "$(sed -n 's/^State:[[:space:]]*//p' "/proc/$wm_pid/status" 2>/dev/null || :)" in
                T*|t*|'') continue ;;
            esac
            wm_birth=$(process_birth "$wm_pid" 2>/dev/null || :)
            if kill -STOP "$wm_pid" 2>/dev/null; then
                PAUSED_WM_PIDS="$PAUSED_WM_PIDS $wm_pid"
                if [ -n "$wm_birth" ] && [ "$(process_birth "$wm_pid" 2>/dev/null || :)" = "$wm_birth" ]; then
                    PAUSED_WM_IDENTITIES="$PAUSED_WM_IDENTITIES $wm_pid:$wm_birth"
                fi
                launch_trace awesome_paused
            fi
        done
    fi
}

run_application() {
    trap - 0 HUP INT TERM
    exec 4>&-
    runner_child=''
    runner_stopping=0
    runner_interrupted=0
    stop_runner() {
        runner_stopping=1
        runner_interrupted=1
        if [ -n "$runner_child" ]; then
            kill -TERM "$runner_child" 2>/dev/null || :
        fi
    }
    trap stop_runner HUP INT TERM
    BILICOMICS_LIFECYCLE_DIR=$LOCK_DIR BILICOMICS_LAUNCH_GENERATION=$APP_GENERATION BILICOMICS_LAUNCH_TOKEN=$LAUNCH_TOKEN "$APP_DIR/bilicomics" --kindle --device "$PROFILE" --data-dir "$DATA_DIR" >&6 2>&1 3>&- 4>&- 5>&- 6>&- &
    runner_child=$!
    printf '%s\n' "$runner_child" >"$LOCK_DIR/application-pid.pending"
    mv -f "$LOCK_DIR/application-pid.pending" "$LOCK_DIR/application-pid"
    if [ "$runner_stopping" -eq 1 ]; then
        kill -TERM "$runner_child" 2>/dev/null || :
    fi
    while :; do
        runner_interrupted=0
        if wait "$runner_child"; then
            runner_status=0
        else
            runner_status=$?
        fi
        [ "$runner_interrupted" -eq 1 ] || break
    done
    printf '%s:%s:%s\n' "$APP_GENERATION" "$LAUNCH_TOKEN" "$runner_status" >"$LOCK_DIR/application-exit.pending"
    mv -f "$LOCK_DIR/application-exit.pending" "$LOCK_DIR/application-exit"
    printf '_bilicomics_app_exit %s %s %s\n' "$APP_GENERATION" "$LAUNCH_TOKEN" "$runner_status" >&3
    exit "$runner_status"
}

stop_application() {
    APP_STATUS=0
    if [ -n "$APP_RUNNER_PID" ]; then
        kill -TERM "$APP_RUNNER_PID" 2>/dev/null || :
        remaining=5
        while kill -0 "$APP_RUNNER_PID" 2>/dev/null && [ "$remaining" -gt 0 ]; do
            sleep 1
            remaining=$((remaining - 1))
        done
        if kill -0 "$APP_RUNNER_PID" 2>/dev/null; then
            signal_recorded_child "$LOCK_DIR/application-pid" KILL "$APP_RUNNER_PID" || :
            sleep 1
            kill -KILL "$APP_RUNNER_PID" 2>/dev/null || :
        fi
        if wait "$APP_RUNNER_PID"; then
            APP_STATUS=0
        else
            APP_STATUS=$?
        fi
        APP_RUNNER_PID=''
    fi
    rm -f "$LOCK_DIR/application-pid" "$LOCK_DIR/application-pid.pending"
}

finish_recorded_application_exit() {
    recorded_exit=$(sed -n '1p' "$LOCK_DIR/application-exit" 2>/dev/null || :)
    case "$recorded_exit" in
        "$APP_GENERATION:$LAUNCH_TOKEN:"*)
            recorded_status=${recorded_exit#"$APP_GENERATION:$LAUNCH_TOKEN:"}
            case "$recorded_status" in
                *[!0-9]*|'') fail 'The recorded application exit status is invalid.' ;;
            esac
            wait "$APP_RUNNER_PID" 2>/dev/null || :
            APP_RUNNER_PID=''
            exit "$recorded_status"
            ;;
    esac
    return 1
}

observe_power() {
    # Keep the source, bounded relay, and their wrappers in one proven group.
    # The leader remains alive after EOF or a relay error until supervisor cleanup.
    exec setsid sh -c '
        directory=$1
        generation=$2
        token=$3
        application=$4
        wrapper_pid=$$
        trap - 0 HUP INT TERM
        gate_checks=200
        while [ "$(cat "$directory/observer-go" 2>/dev/null || :)" != "$generation:$token:$wrapper_pid" ]; do
            [ "$gate_checks" -gt 0 ] || exit 97
            gate_checks=$((gate_checks - 1))
            usleep 10000
        done
        # Retain no FIFO read end after ownership has been proved.
        exec 3>"$directory/power-events"
        (
            trap - 0 HUP INT TERM
            lipc-wait-event -m -s 0 com.lab126.powerd goingToScreenSaver,outOfScreenSaver,exitingScreenSaver,wakeupFromSuspend,usbConfigured &
            source_pid=$!
            printf "%s\n" "$source_pid" >"$directory/observer-pid.pending"
            mv -f "$directory/observer-pid.pending" "$directory/observer-pid"
            if wait "$source_pid"; then source_status=0; else source_status=$?; fi
            # Publish the actual source result before the producer closes its
            # private FIFO writer. A clean relay EOF can then reconcile it.
            printf "%s:%s:%s\n" "$generation" "$token" "$source_status" >"$directory/observer-status.pending"
            mv -f "$directory/observer-status.pending" "$directory/observer-status"
        ) >"$directory/power-source" 2>/dev/null 3>&- &
        source_runner=$!
        printf "%s\n" "$source_runner" >"$directory/observer-runner-pid.pending"
        mv -f "$directory/observer-runner-pid.pending" "$directory/observer-runner-pid"
        # Each stream is opened independently. Setting relay O_NONBLOCK must
        # never change the master or wrapper open-file-description flags.
        BILICOMICS_POWER_RELAY_GENERATION=$generation BILICOMICS_POWER_RELAY_TOKEN=$token "$application" --relay-power-events <"$directory/power-source" >"$directory/power-events" 2>/dev/null 3<"$directory/power-control" 4>&- &
        relay_pid=$!
        printf "%s\n" "$relay_pid" >"$directory/observer-relay-pid.pending"
        mv -f "$directory/observer-relay-pid.pending" "$directory/observer-relay-pid"
        if wait "$relay_pid"; then relay_status=0; else relay_status=$?; fi
        observer_status=$relay_status
        if [ "$relay_status" -eq 0 ]; then
            source_record=$(cat "$directory/observer-status" 2>/dev/null || :)
            case "$source_record" in
                "$generation:$token:"*)
                    source_status=${source_record#"$generation:$token:"}
                    case "$source_status" in *[!0-9]*|"") observer_status=97 ;; *) observer_status=$source_status ;; esac
                    ;;
                *) observer_status=97 ;;
            esac
            wait "$source_runner" 2>/dev/null || :
        fi
        printf "_bilicomics_observer_exit %s\n" "$observer_status" >&3
        while :; do sleep 1; done
    ' sh "$LOCK_DIR" "$APP_GENERATION" "$LAUNCH_TOKEN" "$APP_DIR/bilicomics" 4>&- 5>&- 6>&-
}

observer_is_owned() {
    [ -n "$EVENT_SOURCE_PID" ] && [ -n "$EVENT_SOURCE_BIRTH" ] || return 1
    read_process "$EVENT_SOURCE_PID" 2>/dev/null || return 1
    [ "$PROCESS_PARENT" = "$$" ] && [ "$PROCESS_BIRTH" = "$EVENT_SOURCE_BIRTH" ]
}

observer_group_is_owned() {
    observer_is_owned || return 1
    [ "$PROCESS_GROUP:$PROCESS_SESSION" = "$EVENT_SOURCE_PID:$EVENT_SOURCE_PID" ]
}

start_observer() {
    observe_power &
    EVENT_SOURCE_PID=$!
    EVENT_SOURCE_BIRTH=''
    observer_checks=20
    while [ "$observer_checks" -gt 0 ]; do
        if [ -z "$EVENT_SOURCE_BIRTH" ] && read_process "$EVENT_SOURCE_PID" 2>/dev/null && [ "$PROCESS_PARENT" = "$$" ]; then
            EVENT_SOURCE_BIRTH=$PROCESS_BIRTH
        fi
        if observer_group_is_owned; then
            printf '%s:%s:%s:%s\n' "$APP_GENERATION" "$LAUNCH_TOKEN" "$EVENT_SOURCE_PID" "$EVENT_SOURCE_BIRTH" >"$LOCK_DIR/observer-owned.pending"
            mv -f "$LOCK_DIR/observer-owned.pending" "$LOCK_DIR/observer-owned"
            printf '%s:%s:%s\n' "$APP_GENERATION" "$LAUNCH_TOKEN" "$EVENT_SOURCE_PID" >"$LOCK_DIR/observer-go.pending"
            mv -f "$LOCK_DIR/observer-go.pending" "$LOCK_DIR/observer-go"
            return 0
        fi
        observer_checks=$((observer_checks - 1))
        usleep 100000
    done
    fail 'Power event observer ownership could not be established.'
}

stop_observer() {
    if [ -n "$EVENT_SOURCE_PID" ]; then
        if observer_group_is_owned; then
            # KILL also terminates a source that ignores TERM and a relay blocked
            # on a full event FIFO, while the verified leader pins this group.
            kill -KILL "-$EVENT_SOURCE_PID" 2>/dev/null || :
        elif observer_is_owned; then
            kill -KILL "$EVENT_SOURCE_PID" 2>/dev/null || :
        fi
        observer_reap=10
        while [ "$observer_reap" -gt 0 ]; do
            case "$(sed -n 's/^State:[[:space:]]*//p' "/proc/$EVENT_SOURCE_PID/status" 2>/dev/null || :)" in
                Z*|X*|'') wait "$EVENT_SOURCE_PID" 2>/dev/null || :; break ;;
            esac
            usleep 100000
            observer_reap=$((observer_reap - 1))
        done
        EVENT_SOURCE_PID=''
        EVENT_SOURCE_BIRTH=''
    fi
    rm -f "$LOCK_DIR/observer-pid" "$LOCK_DIR/observer-pid.pending" "$LOCK_DIR/observer-runner-pid" "$LOCK_DIR/observer-runner-pid.pending" "$LOCK_DIR/observer-relay-pid" "$LOCK_DIR/observer-relay-pid.pending" "$LOCK_DIR/observer-status" "$LOCK_DIR/observer-status.pending" "$LOCK_DIR/observer-go" "$LOCK_DIR/observer-go.pending" "$LOCK_DIR/observer-owned" "$LOCK_DIR/observer-owned.pending" "$LOCK_DIR/power-source"
}

wait_for_lifecycle() {
    trap - 0 HUP INT TERM
    exec 4>&-
    waiter_child=''
    waiter_stopping=0
    stop_waiter_child() {
        waiter_stopping=1
        if [ -n "$waiter_child" ]; then
            kill -TERM "$waiter_child" 2>/dev/null || :
            wait "$waiter_child" 2>/dev/null || :
        fi
    }
    trap stop_waiter_child HUP INT TERM
    waiter_sequence=$1
    waiter_action=$2
    waiter_expected=$APP_GENERATION:$LAUNCH_TOKEN:$waiter_sequence:$waiter_action
    waiter_signaled=0
    waiter_remaining=8
    while [ "$waiter_stopping" -eq 0 ]; do
        if [ "$waiter_signaled" -eq 0 ] && [ "$(sed -n '1p' "$LOCK_DIR/lifecycle-ready" 2>/dev/null || :)" = "$APP_GENERATION:$LAUNCH_TOKEN" ]; then
            case "$waiter_action" in
                suspend) waiter_signal=USR1 ;;
                resume) waiter_signal=USR2 ;;
            esac
            if signal_recorded_child "$LOCK_DIR/application-pid" "$waiter_signal" "$APP_RUNNER_PID"; then
                waiter_signaled=1
                launch_trace "signal_$waiter_action"
            fi
        fi
        waiter_ack=$(sed -n '1p' "$LOCK_DIR/lifecycle-ack" 2>/dev/null || :)
        case "$waiter_ack" in
            "$waiter_expected"|"$waiter_expected:ok"|"$waiter_expected:checkpoint-error")
                waiter_status=ok
                [ "$waiter_ack" != "$waiter_expected:checkpoint-error" ] || waiter_status=checkpoint-error
                printf '_bilicomics_lifecycle_ack %s %s %s %s %s\n' "$APP_GENERATION" "$LAUNCH_TOKEN" "$waiter_sequence" "$waiter_action" "$waiter_status" >&3
                return 0
                ;;
        esac
        [ "$waiter_remaining" -gt 0 ] || break
        sleep 1 &
        waiter_child=$!
        if [ "$waiter_stopping" -eq 1 ]; then
            kill -TERM "$waiter_child" 2>/dev/null || :
        fi
        wait "$waiter_child" 2>/dev/null || :
        waiter_child=''
        waiter_remaining=$((waiter_remaining - 1))
    done
    if [ "$waiter_stopping" -eq 0 ]; then
        printf '_bilicomics_lifecycle_timeout %s %s %s %s\n' "$APP_GENERATION" "$LAUNCH_TOKEN" "$waiter_sequence" "$waiter_action" >&3
    fi
}

stop_lifecycle_waiter() {
    if [ -n "$LIFECYCLE_WAIT_PID" ]; then
        kill -TERM "$LIFECYCLE_WAIT_PID" 2>/dev/null || :
        wait "$LIFECYCLE_WAIT_PID" 2>/dev/null || :
        LIFECYCLE_WAIT_PID=''
    fi
}

request_lifecycle() {
    stop_lifecycle_waiter
    LIFECYCLE_SEQUENCE=$((LIFECYCLE_SEQUENCE + 1))
    LIFECYCLE_ACTION=$1
    launch_trace "lifecycle_request_$LIFECYCLE_ACTION"
    if [ "$LIFECYCLE_ACTION" = resume ]; then
        publish_native_epdc_mode resume-native-epdc-mode || fail 'The resume display snapshot is unavailable.'
        publish_native_portrait_orientation resume-native-portrait-orientation || fail 'The resume portrait calibration request is unavailable.'
    else
        rm -f "$LOCK_DIR/resume-native-epdc-mode" "$LOCK_DIR/resume-native-epdc-mode.pending" "$LOCK_DIR/resume-native-portrait-orientation" "$LOCK_DIR/resume-native-portrait-orientation.pending"
        SLEEP_COVER_ACTIVE=0
        SLEEP_COVER_SEQUENCE=$LIFECYCLE_SEQUENCE
        rm -f "$LOCK_DIR/sleep-cover-request" "$LOCK_DIR/sleep-cover-request.pending" "$LOCK_DIR/sleep-cover-ready" "$LOCK_DIR/sleep-cover-ready.pending"
        if cover_sleep_can_own_display; then
            printf '%s:%s:%s:suspend\n' "$APP_GENERATION" "$LAUNCH_TOKEN" "$LIFECYCLE_SEQUENCE" >"$LOCK_DIR/sleep-cover-request.pending"
            mv -f "$LOCK_DIR/sleep-cover-request.pending" "$LOCK_DIR/sleep-cover-request"
        fi
    fi
    printf '%s:%s:%s:%s\n' "$APP_GENERATION" "$LAUNCH_TOKEN" "$LIFECYCLE_SEQUENCE" "$LIFECYCLE_ACTION" >"$LOCK_DIR/lifecycle-request.pending"
    mv -f "$LOCK_DIR/lifecycle-request.pending" "$LOCK_DIR/lifecycle-request"
    wait_for_lifecycle "$LIFECYCLE_SEQUENCE" "$LIFECYCLE_ACTION" &
    LIFECYCLE_WAIT_PID=$!
}

probe_wake_later() {
    trap - 0 HUP INT TERM
    exec 4>&-
    IFS= read -r probe_self_stat </proc/self/stat || return 1
    probe_wrapper_pid=${probe_self_stat%% *}
    probe_child=''
    probe_child_birth=''
    probe_stopping=0
    stop_probe_child() {
        probe_stopping=1
        if [ -n "$probe_child" ]; then
            if [ -z "$probe_child_birth" ] && [ "$(process_parent "$probe_child" 2>/dev/null || :)" = "$probe_wrapper_pid" ]; then
                probe_child_birth=$(process_birth "$probe_child" 2>/dev/null || :)
            fi
            if [ -n "$probe_child_birth" ] && [ "$(process_parent "$probe_child" 2>/dev/null || :)" = "$probe_wrapper_pid" ] &&
                [ "$(process_birth "$probe_child" 2>/dev/null || :)" = "$probe_child_birth" ]; then
                kill -TERM "$probe_child" 2>/dev/null || :
                probe_child_reap=5
                while [ "$probe_child_reap" -gt 0 ]; do
                    case "$(sed -n 's/^State:[[:space:]]*//p' "/proc/$probe_child/status" 2>/dev/null || :)" in
                        Z*|X*|'') wait "$probe_child" 2>/dev/null || :; break ;;
                    esac
                    usleep 10000
                    probe_child_reap=$((probe_child_reap - 1))
                done
                if [ "$(process_parent "$probe_child" 2>/dev/null || :)" = "$probe_wrapper_pid" ] &&
                    [ "$(process_birth "$probe_child" 2>/dev/null || :)" = "$probe_child_birth" ]; then
                    kill -KILL "$probe_child" 2>/dev/null || :
                    probe_child_reap=5
                    while [ "$probe_child_reap" -gt 0 ]; do
                        case "$(sed -n 's/^State:[[:space:]]*//p' "/proc/$probe_child/status" 2>/dev/null || :)" in
                            Z*|X*|'') wait "$probe_child" 2>/dev/null || :; break ;;
                        esac
                        usleep 10000
                        probe_child_reap=$((probe_child_reap - 1))
                    done
                fi
            fi
        fi
    }
    trap stop_probe_child HUP INT TERM
    case "$2" in
        fast) usleep 100000 & ;;
        normal) sleep 2 & ;;
        backoff) sleep 10 & ;;
        *) return 1 ;;
    esac
    probe_child=$!
    probe_child_birth=$(process_birth "$probe_child" 2>/dev/null || :)
    if [ "$probe_stopping" -eq 1 ]; then
        stop_probe_child
    fi
    if wait "$probe_child" 2>/dev/null; then probe_elapsed=1; else probe_elapsed=0; fi
    if [ "$probe_stopping" -eq 0 ] && [ "$probe_elapsed" -eq 1 ]; then
        printf '_bilicomics_wake_check %s %s %s\n' "$LAUNCH_TOKEN" "$1" "$3" >&3
    elif [ "$probe_stopping" -eq 0 ]; then
        launch_trace wake_timer_failed
    fi
}

current_wake_probe_is_owned() {
    [ -n "$WAKE_PROBE_PID" ] && [ -n "$WAKE_PROBE_BIRTH" ] || return 1
    [ "$(process_parent "$WAKE_PROBE_PID" 2>/dev/null || :)" = "$$" ] &&
        [ "$(process_birth "$WAKE_PROBE_PID" 2>/dev/null || :)" = "$WAKE_PROBE_BIRTH" ]
}

stop_wake_probe() {
    if [ -n "$WAKE_PROBE_PID" ]; then
        if [ -z "$WAKE_PROBE_BIRTH" ] && [ "$(process_parent "$WAKE_PROBE_PID" 2>/dev/null || :)" = "$$" ]; then
            WAKE_PROBE_BIRTH=$(process_birth "$WAKE_PROBE_PID" 2>/dev/null || :)
        fi
        if current_wake_probe_is_owned; then kill -TERM "$WAKE_PROBE_PID" 2>/dev/null || :; fi
        probe_reap=10
        while [ "$probe_reap" -gt 0 ]; do
            case "$(sed -n 's/^State:[[:space:]]*//p' "/proc/$WAKE_PROBE_PID/status" 2>/dev/null || :)" in
                Z*|X*|'') wait "$WAKE_PROBE_PID" 2>/dev/null || :; break ;;
            esac
            usleep 100000
            probe_reap=$((probe_reap - 1))
        done
        if current_wake_probe_is_owned; then
            kill -KILL "$WAKE_PROBE_PID" 2>/dev/null || :
            probe_reap=10
            while [ "$probe_reap" -gt 0 ]; do
                case "$(sed -n 's/^State:[[:space:]]*//p' "/proc/$WAKE_PROBE_PID/status" 2>/dev/null || :)" in
                    Z*|X*|'') wait "$WAKE_PROBE_PID" 2>/dev/null || :; break ;;
                esac
                usleep 10000
                probe_reap=$((probe_reap - 1))
            done
        fi
        WAKE_PROBE_PID=''
        WAKE_PROBE_BIRTH=''
        WAKE_PROBE_KIND=''
    fi
}

schedule_wake_probe() {
    # A retained cover still needs bounded powerd retries until the user's wake
    # reaches active. The later barrier separately validates display ownership.
    [ "$APP_PHASE" = sleeping ] && { [ "$DISPLAY_OWNED" -eq 0 ] || [ "$SLEEP_COVER_ACTIVE" -eq 1 ]; } && [ "$WAKE_PENDING" -eq 1 ] || return 0
    [ "$NATIVE_MODE_PENDING" -eq 0 ] || return 0
    stop_wake_probe
    WAKE_PROBE_SEQUENCE=$((WAKE_PROBE_SEQUENCE + 1))
    WAKE_PROBE_KIND=$1
    launch_trace "wake_probe_$1"
    probe_wake_later "$SLEEP_GENERATION" "$1" "$WAKE_PROBE_SEQUENCE" &
    WAKE_PROBE_PID=$!
    WAKE_PROBE_BIRTH=$(process_birth "$WAKE_PROBE_PID" 2>/dev/null || :)
}

retry_native_dismissal() {
    probe_delay=normal
    if [ "$WAKE_FAST_PROBES_REMAINING" -gt 0 ]; then
        WAKE_FAST_PROBES_REMAINING=$((WAKE_FAST_PROBES_REMAINING - 1))
        probe_delay=fast
    elif [ "$WAKE_PROBES_REMAINING" -gt 0 ]; then
        WAKE_PROBES_REMAINING=$((WAKE_PROBES_REMAINING - 1))
        if [ "$WAKE_PROBES_REMAINING" -eq 0 ]; then
            printf '%s\n' 'bilicomics: Waiting for native unlock or screen-saver dismissal.' >&2
        fi
    else
        probe_delay=backoff
    fi
    schedule_wake_probe "$probe_delay"
}

resume_application() {
    [ "$APP_PHASE" = sleeping ] && [ "$WAKE_PENDING" -eq 1 ] || return 0
    [ "$NATIVE_MODE_PENDING" -eq 0 ] || return 0
    if [ "$SLEEP_COVER_ACTIVE" -eq 1 ] && [ "$DISPLAY_OWNED" -eq 1 ]; then
        if ! cover_sleep_can_own_display || ! framebuffer_capability_is_current || ! sleep_cover_records_match; then
            fallback_cover_sleep
        elif cover_wake_power_is_active; then
            stop_wake_probe
            NATIVE_MODE_SEQUENCE=$((NATIVE_MODE_SEQUENCE + 1))
            NATIVE_MODE_PENDING=1
            NATIVE_RESUME_PRESENTATION=framebuffer
            NATIVE_EPDC_MODE=''
            WAKE_CONFIRMED=1
            launch_trace cover_wake_barrier
            queue_native_notice "_bilicomics_native_mode_ready $APP_GENERATION $LAUNCH_TOKEN $SLEEP_GENERATION $NATIVE_MODE_SEQUENCE"
            return 0
        else
            launch_trace cover_wake_inactive
            retry_native_dismissal
            return 0
        fi
    fi
    [ "$DISPLAY_OWNED" -eq 0 ] || return 0
    # OutOfSS can precede actual stock dismissal. Only explicit ExitingSS uses
    # the fast path; the existing active-only fallback retains its minimum delay.
    if [ "$WAKE_DISMISS_GENERATION" -ne "$SLEEP_GENERATION" ] && [ "$WAKE_FALLBACK_ALLOWED" -eq 0 ]; then
        if [ -z "$WAKE_PROBE_PID" ]; then schedule_wake_probe normal; fi
        return 0
    fi
    NATIVE_MODE_SEQUENCE=$((NATIVE_MODE_SEQUENCE + 1))
    NATIVE_MODE_PENDING=1
    NATIVE_RESUME_PRESENTATION=''
    if [ "$WAKE_DISMISS_GENERATION" -eq "$SLEEP_GENERATION" ] && framebuffer_capability_is_current; then
        NATIVE_RESUME_PRESENTATION=framebuffer
        NATIVE_EPDC_MODE=''
        WAKE_CONFIRMED=1
        launch_trace wake_framebuffer_barrier
        queue_native_notice "_bilicomics_native_mode_ready $APP_GENERATION $LAUNCH_TOKEN $SLEEP_GENERATION $NATIVE_MODE_SEQUENCE"
        return 0
    fi
    stop_wake_probe
    NATIVE_RESUME_PRESENTATION=observed
    launch_trace wake_sample_begin
    capture_native_epdc_mode || NATIVE_EPDC_MODE=''
    if [ "$NATIVE_CAPTURE_STATUS" = not-active ]; then
        launch_trace wake_sample_inactive
        NATIVE_MODE_PENDING=0
        retry_native_dismissal
        return 0
    fi
    if [ "$NATIVE_CAPTURE_STATUS" = known ]; then
        WAKE_CONFIRMED=1
        launch_trace wake_sample_ready
    else
        launch_trace wake_sample_unknown
    fi
    # The relay drains its native source before publishing this barrier.
    # Sleep and USB records queued during the getter precede the result.
    queue_native_notice "_bilicomics_native_mode_ready $APP_GENERATION $LAUNCH_TOKEN $SLEEP_GENERATION $NATIVE_MODE_SEQUENCE"
}

cleanup() {
    trap '' HUP INT TERM
    trap - 0
    saved_status=$1
    ensure_log_sinks || :
    LOG_CLOSING=1
    printf '%s:%s\n' "$APP_GENERATION" "$LAUNCH_TOKEN" >"$LOCK_DIR/log-stopping" 2>/dev/null || :
    stop_native_notice
    stop_native_read
    stop_wake_probe
    stop_lifecycle_waiter
    stop_observer
    stop_application
    release_display
    if [ "$EVENT_FD_OPEN" -eq 1 ]; then
        exec 3>&-
    fi
    if [ "$CONTROL_FD_OPEN" -eq 1 ]; then
        exec 4>&-
    fi
    stop_logging
    rm -f "$LOCK_DIR/pid" "$LOCK_DIR/power-events" "$LOCK_DIR/power-control" "$LOCK_DIR/nonce" "$LOCK_DIR/generation" "$LOCK_DIR/application-exit" "$LOCK_DIR/application-exit.pending" "$LOCK_DIR/lifecycle-ready" "$LOCK_DIR/lifecycle-ready.pending" "$LOCK_DIR/lifecycle-request" "$LOCK_DIR/lifecycle-request.pending" "$LOCK_DIR/lifecycle-ack" "$LOCK_DIR/lifecycle-ack.pending" "$LOCK_DIR/sleep-cover-request" "$LOCK_DIR/sleep-cover-request.pending" "$LOCK_DIR/sleep-cover-ready" "$LOCK_DIR/sleep-cover-ready.pending" "$LOCK_DIR/native-epdc-mode" "$LOCK_DIR/native-epdc-mode.pending" "$LOCK_DIR/resume-native-epdc-mode" "$LOCK_DIR/resume-native-epdc-mode.pending" "$LOCK_DIR/native-portrait-orientation" "$LOCK_DIR/native-portrait-orientation.pending" "$LOCK_DIR/resume-native-portrait-orientation" "$LOCK_DIR/resume-native-portrait-orientation.pending" "$LOCK_DIR/native-output-capability" "$LOCK_DIR/native-output-capability.pending" "$LOCK_DIR/native-read-output"
    rmdir "$LOCK_DIR" 2>/dev/null || :
    exit "$saved_status"
}

trap 'cleanup "$?"' 0
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

command -v lipc-wait-event >/dev/null 2>&1 || fail 'Power event observation is required.'
command -v lipc-get-prop >/dev/null 2>&1 || fail 'Native display mode observation is required.'
command -v setsid >/dev/null 2>&1 || fail 'Private native display observation requires setsid.'
command -v usleep >/dev/null 2>&1 || fail 'Bounded native display observation requires usleep.'
command -v dd >/dev/null 2>&1 || fail 'Bounded native display output requires dd.'
command -v head >/dev/null 2>&1 && command -v tail >/dev/null 2>&1 && command -v stat >/dev/null 2>&1 || fail 'Bounded production logging requires head, tail, and stat.'
LAUNCH_TOKEN=$(od -An -N16 -tx1 /dev/urandom | tr -d ' \n')
[ "${#LAUNCH_TOKEN}" -eq 32 ] || fail 'The launcher context token could not be generated.'
printf '%s\n' "$LAUNCH_TOKEN" >"$LOCK_DIR/nonce"
printf '%s:%s\n' "$APP_GENERATION" "$LAUNCH_TOKEN" >"$LOCK_DIR/generation"
mkfifo "$LOCK_DIR/power-events"
mkfifo "$LOCK_DIR/power-source"
mkfifo "$LOCK_DIR/power-control"
# Keep one descriptor open so application exits and power events remain queued
# while independent acknowledgement or native unlock observers are pending.
exec 3<>"$LOCK_DIR/power-events"
EVENT_FD_OPEN=1
exec 4<>"$LOCK_DIR/power-control"
CONTROL_FD_OPEN=1
start_logging || fail 'Production log preparation or bounded output setup failed.'
start_observer

sleep 2
export BILICOMICS_HOME
launch_trace startup_sample_begin
if ! capture_native_epdc_mode || [ "$NATIVE_CAPTURE_STATUS" != known ]; then
    fail 'The native display mode could not be read before startup.'
fi
launch_trace startup_sample_ready
publish_native_epdc_mode native-epdc-mode || fail 'The startup display snapshot is unavailable.'
publish_native_portrait_orientation native-portrait-orientation || fail 'The startup portrait calibration is unavailable.'
configure_cover_sleep
queue_native_notice "_bilicomics_start_ready $APP_GENERATION $LAUNCH_TOKEN"

while IFS= read -r power_event <&3; do
    ensure_log_sinks || fail 'The bounded production log drain could not be restored.'
    case "$power_event" in
        '_bilicomics_log_sink_exit '*)
            handle_log_sink_exit
            ;;
        goingToScreenSaver*)
            SLEEP_GENERATION=$((SLEEP_GENERATION + 1))
            SLEEP_REQUESTED=1
            WAKE_PENDING=0
            WAKE_CONFIRMED=0
            WAKE_DISMISS_GENERATION=0
            WAKE_FALLBACK_ALLOWED=0
            WAKE_FAST_PROBES_REMAINING=0
            NATIVE_MODE_PENDING=0
            NATIVE_RESUME_PRESENTATION=''
            NATIVE_EPDC_MODE=''
            NATIVE_PORTRAIT_ORIENTATION=''
            NATIVE_PORTRAIT_ROTATION=''
            launch_trace sleep_event
            stop_wake_probe
            case "$APP_PHASE" in
                running)
                    APP_PHASE=suspending
                    request_lifecycle suspend
                    ;;
            esac
            ;;
        outOfScreenSaver*)
            launch_trace wake_out
            case "$APP_PHASE" in
                suspending|sleeping|resuming)
                    # A second sleep may arrive while resume is awaiting its
                    # acknowledgement. Retain the following user wake until
                    # that queued suspend has also been acknowledged.
                    if { [ "$APP_PHASE" != resuming ] || [ "$SLEEP_REQUESTED" -eq 1 ]; } && [ "$WAKE_PENDING" -eq 0 ]; then
                        WAKE_PENDING=1
                        WAKE_FALLBACK_ALLOWED=0
                        WAKE_FAST_PROBES_REMAINING=3
                        WAKE_PROBES_REMAINING=3
                        if [ "$SLEEP_COVER_ACTIVE" -eq 1 ] || [ "$WAKE_DISMISS_GENERATION" -eq "$SLEEP_GENERATION" ]; then
                            resume_application
                        else
                            schedule_wake_probe normal
                        fi
                    fi
                    ;;
            esac
            ;;
        exitingScreenSaver*)
            launch_trace wake_exit
            case "$APP_PHASE" in
                suspending|sleeping|resuming)
                    # Preserve explicit dismissal evidence even when firmware
                    # reports it before the user-wake intent for this cycle.
                    if [ "$SLEEP_GENERATION" -gt 0 ] && [ "$WAKE_DISMISS_GENERATION" -ne "$SLEEP_GENERATION" ]; then
                        WAKE_DISMISS_GENERATION=$SLEEP_GENERATION
                        WAKE_CONFIRMED=1
                        resume_application
                    fi
                    ;;
            esac
            ;;
        wakeupFromSuspend*)
            # RTC and background wakes never acquire display or input.
            ;;
        usbConfigured*)
            WAKE_PENDING=0
            WAKE_CONFIRMED=0
            WAKE_DISMISS_GENERATION=0
            WAKE_FALLBACK_ALLOWED=0
            launch_trace usb_takeover
            exit 143
            ;;
        '_bilicomics_start_ready '*)
            set -- $power_event
            if [ "$#" -eq 3 ] && [ "$2" = "$APP_GENERATION" ] && [ "$3" = "$LAUNCH_TOKEN" ] && [ "$APP_PHASE" = none ]; then
                stop_native_notice
                [ "$SLEEP_REQUESTED" -eq 0 ] || fail 'Power state changed before native display takeover.'
                take_display
                APP_PHASE=running
                run_application &
                APP_RUNNER_PID=$!
            fi
            ;;
        '_bilicomics_native_mode_ready '*)
            set -- $power_event
            if [ "$#" -eq 5 ] && [ "$2" = "$APP_GENERATION" ] && [ "$3" = "$LAUNCH_TOKEN" ] && [ "$4" = "$SLEEP_GENERATION" ] && [ "$5" = "$NATIVE_MODE_SEQUENCE" ] && [ "$NATIVE_MODE_PENDING" -eq 1 ] && [ "$APP_PHASE" = sleeping ] && { [ "$DISPLAY_OWNED" -eq 0 ] || [ "$SLEEP_COVER_ACTIVE" -eq 1 ]; } && [ "$WAKE_PENDING" -eq 1 ]; then
                stop_native_notice
                if [ "$NATIVE_RESUME_PRESENTATION" = framebuffer ]; then
                    if ! framebuffer_capability_is_current ||
                        { [ "$SLEEP_COVER_ACTIVE" -eq 1 ] && ! cover_sleep_can_own_display; } ||
                        { [ "$SLEEP_COVER_ACTIVE" -eq 0 ] && [ "$WAKE_DISMISS_GENERATION" -ne "$SLEEP_GENERATION" ]; }; then
                        NATIVE_MODE_PENDING=0
                        NATIVE_CAPABILITY_VALID=0
                        NATIVE_RESUME_PRESENTATION=''
                        launch_trace wake_capability_fallback
                        if [ "$SLEEP_COVER_ACTIVE" -eq 1 ]; then fallback_cover_sleep; fi
                        resume_application
                        continue
                    fi
                else
                    case "$NATIVE_EPDC_MODE" in Y8|Y8INV) ;; *) fail 'The native display mode could not be read before resume.' ;; esac
                fi
                [ "$WAKE_CONFIRMED" -eq 1 ] || fail 'Native dismissal was not confirmed before resume.'
                launch_trace wake_barrier_accepted
                NATIVE_MODE_PENDING=0
                WAKE_PENDING=0
                WAKE_CONFIRMED=0
                SLEEP_REQUESTED=0
                if [ "$NATIVE_RESUME_PRESENTATION" = framebuffer ]; then take_display framebuffer; else take_display; fi
                APP_PHASE=resuming
                request_lifecycle resume
                stop_wake_probe
            else
                launch_trace wake_barrier_dropped
            fi
            ;;
        '_bilicomics_app_exit '*)
            set -- $power_event
            if [ "$#" -eq 4 ] && [ "$2" = "$APP_GENERATION" ] && [ "$3" = "$LAUNCH_TOKEN" ]; then
                case "$4" in
                    *[!0-9]*|'') fail 'The application exit status is invalid.' ;;
                esac
                wait "$APP_RUNNER_PID" 2>/dev/null || :
                APP_RUNNER_PID=''
                exit "$4"
            fi
            ;;
        '_bilicomics_lifecycle_ack '*)
            set -- $power_event
            if [ "$#" -eq 6 ] && [ "$2" = "$APP_GENERATION" ] && [ "$3" = "$LAUNCH_TOKEN" ] && [ "$4" = "$LIFECYCLE_SEQUENCE" ] && [ "$5" = "$LIFECYCLE_ACTION" ]; then
                stop_lifecycle_waiter
                launch_trace "lifecycle_ack_$LIFECYCLE_ACTION"
                case "$6" in
                    checkpoint-error)
                        printf '%s\n' 'bilicomics: The local checkpoint could not be saved; the reading session remains in memory.' >&2
                        ;;
                    ok) ;;
                    *) fail 'The lifecycle acknowledgement status is invalid.' ;;
                esac
                if [ "$LIFECYCLE_ACTION" = suspend ] && [ "$APP_PHASE" = suspending ]; then
                    APP_PHASE=sleeping
                    cache_framebuffer_capability
                    if [ "$6" = ok ] && cover_sleep_can_own_display &&
                        framebuffer_capability_is_current && sleep_cover_records_match; then
                        SLEEP_COVER_ACTIVE=1
                        launch_trace cover_sleep_retained
                    else
                        SLEEP_COVER_ACTIVE=0
                        release_display
                    fi
                    resume_application
                elif [ "$LIFECYCLE_ACTION" = resume ] && [ "$APP_PHASE" = resuming ]; then
                    APP_PHASE=running
                    SLEEP_COVER_ACTIVE=0
                    rm -f "$LOCK_DIR/sleep-cover-request" "$LOCK_DIR/sleep-cover-request.pending" "$LOCK_DIR/sleep-cover-ready" "$LOCK_DIR/sleep-cover-ready.pending"
                    if [ "$SLEEP_REQUESTED" -eq 1 ]; then
                        APP_PHASE=suspending
                        request_lifecycle suspend
                    fi
                fi
            fi
            ;;
        '_bilicomics_lifecycle_timeout '*)
            set -- $power_event
            if [ "$#" -eq 5 ] && [ "$2" = "$APP_GENERATION" ] && [ "$3" = "$LAUNCH_TOKEN" ] && [ "$4" = "$LIFECYCLE_SEQUENCE" ] && [ "$5" = "$LIFECYCLE_ACTION" ]; then
                # The runner persists its result before notifying the FIFO.
                # Prefer that completed exit if it raced the deadline event.
                finish_recorded_application_exit || :
                fail "The application did not acknowledge $LIFECYCLE_ACTION before the lifecycle deadline."
            fi
            ;;
        '_bilicomics_wake_check '*)
            set -- $power_event
            if [ "$#" -eq 4 ] && [ "$2" = "$LAUNCH_TOKEN" ] && [ "$3" = "$SLEEP_GENERATION" ] && [ "$4" = "$WAKE_PROBE_SEQUENCE" ] && [ -n "$WAKE_PROBE_PID" ] && [ "$APP_PHASE" = sleeping ] && { [ "$DISPLAY_OWNED" -eq 0 ] || [ "$SLEEP_COVER_ACTIVE" -eq 1 ]; } && [ "$NATIVE_MODE_PENDING" -eq 0 ] && [ "$WAKE_PENDING" -eq 1 ]; then
                completed_probe_kind=$WAKE_PROBE_KIND
                stop_wake_probe
                if [ "$completed_probe_kind" = normal ]; then WAKE_FALLBACK_ALLOWED=1; fi
                resume_application
            else
                launch_trace wake_probe_stale
            fi
            ;;
        '_bilicomics_observer_exit '*)
            fail 'Power event observation ended unexpectedly.'
            ;;
    esac
done

fail 'The power event channel closed unexpectedly.'
