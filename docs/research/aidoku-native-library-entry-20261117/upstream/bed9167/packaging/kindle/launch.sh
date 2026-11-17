#!/bin/sh
# Name: Aidoku for Kindle
# DontUseFBInk
# Keep the app in RAM while stock powerd owns the screen and input.
set -eu
umask 077
AIDOKU_HOME=${AIDOKU_HOME:-/mnt/us/aidoku}
APP_DIR=$AIDOKU_HOME/app
DATA_DIR=$AIDOKU_HOME/data
LOCK_DIR=${AIDOKU_LOCK_DIR:-/tmp/aidoku-launcher.lock}
FRAMEBUFFER=${AIDOKU_FRAMEBUFFER:-/dev/fb0}
ROOT_PID=$$

fail() { printf '%s\n' "aidoku: $*" >&2; exit 1; }
valid_pid() { case "$1" in ''|*[!0-9]*) return 1 ;; *) [ "$1" -gt 1 ] ;; esac; }
process_stat_fields() {
    process_stat_record=''
    { IFS= read -r process_stat_record <"/proc/$1/stat"; } 2>/dev/null || return 1
    case "$process_stat_record" in "$1 ("*) ;; *) return 1 ;; esac
    # comm can contain spaces and parentheses. Numeric fields follow its final ).
    process_fields=${process_stat_record##*) }
    [ "$process_fields" != "$process_stat_record" ]
}
parent_pid() {
    process_stat_fields "$1" || return 0
    set -- $process_fields
    [ "$#" -ge 2 ] || return 0
    case "$2" in ''|*[!0-9]*) return 0 ;; esac
    printf '%s\n' "$2"
}
process_state() {
    process_stat_fields "$1" || return 0
    set -- $process_fields
    [ "$#" -ge 1 ] || return 0
    printf '%s\n' "$1"
}
process_birth() {
    process_stat_fields "$1" || return 1
    set -- $process_fields
    [ "$#" -ge 20 ] || return 1
    shift 19
    case "$1" in ''|*[!0-9]*) return 1 ;; esac
    printf '%s\n' "$1"
}
owned_pid() {
    valid_pid "$1" && [ "$(parent_pid "$1")" = "$2" ] || return 1
    case "$(process_state "$1")" in ''|Z*|X*) return 1 ;; esac
    kill -0 "$1" 2>/dev/null
}
signal_child() { owned_pid "$1" "$2" || return 1; kill "-$3" "$1" 2>/dev/null; }
write_record() { printf '%s\n' "$2" >"$LOCK_DIR/$1.pending"; mv "$LOCK_DIR/$1.pending" "$LOCK_DIR/$1"; }
matches_record() { [ "$(cat "$LOCK_DIR/$1" 2>/dev/null || :)" = "$2" ]; }
ordinary_exit() {
    ordinary_record=$(cat "$LOCK_DIR/ordinary-exit" 2>/dev/null || :)
    ordinary_application=$1
    set -- $ordinary_record
    [ "$#" -eq 3 ] && [ "$1" = "$ordinary_application" ] && [ "$2" = "$NONCE" ] || return 1
    case "$3" in ''|*[!0-9]*) return 1 ;; esac
    # A UI exit written before a queued sleep event still wins after its epoch
    # advances. Each new ELF generation clears the old ordinary-exit record.
    [ "$3" -le "$SLEEP_EPOCH" ]
}
set_phase() { APP_PHASE=$1; write_record phase "$APP_PHASE"; }

case "$AIDOKU_HOME:$LOCK_DIR" in /*:/*) ;; *) fail 'Application and lock directories must be absolute.' ;; esac
[ -x "$APP_DIR/aidoku" ] || fail 'The ARM application executable is missing.'
[ -c "$FRAMEBUFFER" ] || fail 'A native Kindle framebuffer is required.'
mkdir -p "$DATA_DIR/logs" "$DATA_DIR/imports"
PROFILE=pw5
if [ -r "$DATA_DIR/device-profile" ]; then PROFILE=$(sed -n '1{s/\r$//;p;}' "$DATA_DIR/device-profile"); fi
case "$PROFILE" in pw5|pw6|scribe|scribe2|scribe3|scribe3-nofrontlight) ;; *) fail 'Unknown device profile.' ;; esac
SCRIPT_PATH=$(CDPATH= cd "$(dirname "$0")" && pwd)/$(basename "$0")
if [ "${1:-}" != '--foreground' ]; then
    export AIDOKU_HOME
    if command -v setsid >/dev/null 2>&1; then
        setsid sh "$SCRIPT_PATH" --foreground >>"$DATA_DIR/logs/launcher.log" 2>&1 </dev/null &
    else
        nohup sh "$SCRIPT_PATH" --foreground >>"$DATA_DIR/logs/launcher.log" 2>&1 </dev/null &
    fi
    LIBRARY_WORKER_PID=$!
    if [ "${1:-}" = '--library' ]; then
        # Keep the native Scriptlet active until its owned worker restores the
        # display. The worker remains in its independent session if appmgrd
        # retires this small waiting entry during a native ownership transition.
        library_cancel() {
            signal_child "$LIBRARY_WORKER_PID" "$ROOT_PID" TERM || :
            wait "$LIBRARY_WORKER_PID" 2>/dev/null || :
            exit "$1"
        }
        trap 'library_cancel 129' HUP
        trap 'library_cancel 130' INT
        trap 'library_cancel 143' TERM
        LIBRARY_STATUS=0
        wait "$LIBRARY_WORKER_PID" || LIBRARY_STATUS=$?
        exit "$LIBRARY_STATUS"
    fi
    exit 0
fi

# A different launcher's lock is never stolen or repaired.
mkdir "$LOCK_DIR" 2>/dev/null || fail 'Another Aidoku launcher owns the display lock.'
printf '%s\n' "$ROOT_PID" >"$LOCK_DIR/pid"
printf 'aidoku: Launcher worker started (pid=%s).\n' "$ROOT_PID" >&2
APP_PID=''; RUNNER_PID=''; OBSERVER_PID=''; ACK_PID=''; WAKE_PID=''; RESUME_PID=''; REDRAW_PID=''
APP_GENERATION=0; APP_PHASE=starting; SLEEP_EPOCH=0; SLEEP_GENERATION=0
NATIVE_READ_PID=''; NATIVE_READ_BIRTH=''; NATIVE_READ_SEQUENCE=0
NATIVE_RESUME_ATTEMPTS=0
SLEEP_APP_PID=0; SLEEP_PENDING=0; WAKE_PENDING=0
DISPLAY_OWNED=0; PAUSED_WM_RECORDS=''; STATUSBAR_STOPPED=0; STOCK_REPAINT_PENDING=0; EXIT_REDRAW=1; EVENT_FD_OPEN=0
STATUSBAR_QUIESCED=0; COVER_SLEEP_SUPPORTED=0; COVER_SLEEP_CHECKED=0; SLEEP_COVER_ACTIVE=0
COVER_SEQUENCE=0; SLEEP_COVER_SEQUENCE=0; SLEEP_APP_BIRTH=''; SLEEP_RUNNER_BIRTH=''

native_read_is_owned() {
    [ -n "$NATIVE_READ_PID" ] && [ -n "$NATIVE_READ_BIRTH" ] || return 1
    process_stat_fields "$NATIVE_READ_PID" || return 1
    set -- $process_fields
    [ "$#" -ge 20 ] && [ "$2" = "$ROOT_PID" ] || return 1
    NATIVE_READ_GROUP=$3; NATIVE_READ_SESSION=$4
    shift 19
    [ "$1" = "$NATIVE_READ_BIRTH" ]
}

native_read_group_is_owned() {
    native_read_is_owned || return 1
    [ "$NATIVE_READ_GROUP:$NATIVE_READ_SESSION" = "$NATIVE_READ_PID:$NATIVE_READ_PID" ]
}

stop_native_read() {
    if native_read_group_is_owned; then
        # The verified leader remains alive after the getter exits. Kill its
        # entire private group so a timed-out descendant cannot reply later.
        kill -KILL "-$NATIVE_READ_PID" 2>/dev/null || :
    elif native_read_is_owned; then
        # Before setsid completes the gated wrapper has not started an IPC.
        kill -KILL "$NATIVE_READ_PID" 2>/dev/null || :
    fi
    native_read_reap=50
    while native_read_is_owned && [ "$native_read_reap" -gt 0 ]; do
        case "$(process_state "$NATIVE_READ_PID")" in
            Z*|X*|'') break ;;
        esac
        usleep 20000
        native_read_reap=$((native_read_reap - 1))
    done
    if [ -n "$NATIVE_READ_PID" ]; then
        case "$(process_state "$NATIVE_READ_PID")" in
            Z*|X*|'') wait "$NATIVE_READ_PID" 2>/dev/null || : ;;
        esac
    fi
    NATIVE_READ_PID=''
    NATIVE_READ_BIRTH=''
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
    ' sh "$LOCK_DIR/native-read-done" "$LOCK_DIR/native-read-go" "$NATIVE_READ_SEQUENCE" "$NONCE" "$LOCK_DIR/native-read-command" "$@" >"$LOCK_DIR/native-read-output" 2>/dev/null 3>&- 4>&- &
    NATIVE_READ_PID=$!
    NATIVE_READ_BIRTH=$(process_birth "$NATIVE_READ_PID" 2>/dev/null || :)
    # Keep the same two-second sleep budget, with earlier completion detection.
    native_read_checks=100
    native_read_status=1
    native_read_released=0
    while [ "$native_read_checks" -gt 0 ]; do
        if [ -z "$NATIVE_READ_BIRTH" ] && [ "$(parent_pid "$NATIVE_READ_PID" 2>/dev/null || :)" = "$ROOT_PID" ]; then
            NATIVE_READ_BIRTH=$(process_birth "$NATIVE_READ_PID" 2>/dev/null || :)
        fi
        case "$(process_state "$NATIVE_READ_PID")" in
            Z*|X*|'') break ;;
        esac
        if native_read_group_is_owned; then
            if [ "$native_read_released" -eq 0 ]; then
                if ! { printf '%s:%s:%s\n' "$NATIVE_READ_PID" "$NATIVE_READ_SEQUENCE" "$NONCE" >"$LOCK_DIR/native-read-go.pending" &&
                    mv -f "$LOCK_DIR/native-read-go.pending" "$LOCK_DIR/native-read-go"; }; then
                    break
                fi
                native_read_released=1
            fi
            if [ -r "$LOCK_DIR/native-read-done" ]; then
                [ "$(cat "$LOCK_DIR/native-read-done" 2>/dev/null || :)" != "$NATIVE_READ_SEQUENCE:$NONCE:0" ] || native_read_status=0
                break
            fi
        fi
        native_read_checks=$((native_read_checks - 1))
        usleep 20000
    done
    stop_native_read
    return "$native_read_status"
}

native_read_hex() {
    od -An -v -tx1 "$LOCK_DIR/native-read-output" | tr -d ' \n'
}

configure_cover_sleep() {
    # Compatibility path for a publisher without a combined capability result.
    # Adopt the BiliComics read-only blanket observation. An unknown module,
    # advertisement module, malformed response or failed query keeps stock sleep.
    # Probe once before display takeover; capability belongs to this managed
    # session. Sleep and wake still recheck current ownership independently.
    COVER_SLEEP_SUPPORTED=0
    native_read_command lipc-get-prop -s com.lab126.blanket load || return 0
    cover_configuration_size=$(LC_ALL=C stat -c %s "$LOCK_DIR/native-read-output") || return 0
    [ "$cover_configuration_size" -gt 0 ] && [ "$cover_configuration_size" -le 128 ] || return 0
    cover_configuration_bytes=$(od -An -v -tu1 "$LOCK_DIR/native-read-output") || return 0
    for cover_byte in $cover_configuration_bytes; do
        case "$cover_byte" in 9|10|13) continue ;; esac
        [ "$cover_byte" -ge 32 ] && [ "$cover_byte" -le 126 ] || return 0
    done
    cover_configuration=$(cat "$LOCK_DIR/native-read-output") || return 0
    case "$cover_configuration" in *[!a-z_[:space:]]*) return 0 ;; esac
    cover_seen_screensaver=0
    for cover_module in $cover_configuration; do
        case "$cover_module" in
            screensaver) cover_seen_screensaver=1 ;;
            # Standard resident modules may coexist with the stock screensaver.
            langpicker|blankwindow|usb) ;;
            *) return 0 ;;
        esac
    done
    [ "$cover_seen_screensaver" -eq 1 ] || return 0
    COVER_SLEEP_SUPPORTED=1
}

clear_sleep_cover_records() {
    # These are the only cover protocol leaves; never remove a wildcard path.
    rm -f "$LOCK_DIR/sleep-cover-request" "$LOCK_DIR/sleep-cover-request.pending" "$LOCK_DIR/sleep-cover-ready" "$LOCK_DIR/sleep-cover-ready.pending"
    SLEEP_COVER_ACTIVE=0; SLEEP_COVER_SEQUENCE=0
}

sleep_cover_context_is_current() {
    [ "$SLEEP_GENERATION" = "$APP_GENERATION" ] && [ "$SLEEP_APP_PID" = "$APP_PID" ] &&
        [ "$SLEEP_COVER_SEQUENCE" -gt 0 ] && [ -n "$SLEEP_APP_BIRTH" ] && [ -n "$SLEEP_RUNNER_BIRTH" ] || return 1
    owned_pid "$APP_PID" "$RUNNER_PID" && owned_pid "$RUNNER_PID" "$ROOT_PID" || return 1
    [ "$(process_birth "$APP_PID")" = "$SLEEP_APP_BIRTH" ] &&
        [ "$(process_birth "$RUNNER_PID")" = "$SLEEP_RUNNER_BIRTH" ]
}

cover_sleep_can_own_display() {
    [ "$COVER_SLEEP_SUPPORTED" -eq 1 ] && [ "$DISPLAY_OWNED" -eq 1 ] &&
        [ "$STATUSBAR_STOPPED" -eq 1 ] && [ "$STATUSBAR_QUIESCED" -eq 1 ] &&
        [ -n "$PAUSED_WM_RECORDS" ] || return 1
    # A passcode configured after launch immediately revokes custom ownership.
    [ ! -e /var/local/system/userpasswdenabled ] && [ ! -L /var/local/system/userpasswdenabled ] || return 1
    case "$(status statusbar 2>/dev/null || :)" in *stop/waiting*) ;; *) return 1 ;; esac
    for cover_wm_record in $PAUSED_WM_RECORDS; do
        cover_wm_pid=${cover_wm_record%%:*}; cover_wm_birth=${cover_wm_record#*:}
        [ -n "$cover_wm_birth" ] && [ "$(process_birth "$cover_wm_pid")" = "$cover_wm_birth" ] || return 1
        case "$(process_state "$cover_wm_pid")" in T*|t*) ;; *) return 1 ;; esac
    done
}

sleep_cover_records_match() {
    sleep_cover_context_is_current || return 1
    cover_expected="$SLEEP_GENERATION $SLEEP_APP_PID $NONCE $SLEEP_EPOCH $SLEEP_COVER_SEQUENCE suspend"
    cover_leaves=sleep-cover-request
    [ "${1:-ready}" = request ] || cover_leaves="$cover_leaves sleep-cover-ready"
    # Mutable leaves are read in the existing bounded private reader group.
    # Exact sizes reject extra lines/NULs; before/after identities reject edits.
    if native_read_command sh -c '
        directory=$1; expected=$2; leaves=$3; owner=$4; shift 4
        [ -d "$directory" ] && [ ! -L "$directory" ] || exit 1
        directory_record=$(LC_ALL=C stat -c "%d:%i:%u:%f" "$directory") || exit 1
        directory_tail=${directory_record#*:*:}
        [ "${directory_tail%%:*}" = "$owner" ] || exit 1
        case "${directory_tail#*:}" in 41c0|45c0) ;; *) exit 1 ;; esac
        check_record() {
            path=$directory/$1; value=$2
            [ -f "$path" ] && [ ! -L "$path" ] || return 1
            before=$(LC_ALL=C stat -c "%d:%i:%u:%f:%h:%s:%Y:%Z" "$path") || return 1
            tail=${before#*:*:}
            [ "${tail%%:*}" = "$owner" ] || return 1
            tail=${tail#*:}
            case "${tail%%:*}" in 8180|8100) ;; *) return 1 ;; esac
            tail=${tail#*:}
            [ "${tail%%:*}" = 1 ] || return 1
            tail=${tail#*:}
            [ "${tail%%:*}" = "$((${#value} + 1))" ] || return 1
            IFS= read -r record <"$path" || return 1
            [ "$record" = "$value" ] || return 1
            after=$(LC_ALL=C stat -c "%d:%i:%u:%f:%h:%s:%Y:%Z" "$path") || return 1
            [ "$before" = "$after" ]
        }
        while [ "$#" -gt 0 ]; do check_record "$1" "$2" || exit 1; shift 2; done
        for leaf in $leaves; do check_record "$leaf" "$expected" || exit 1; done
        [ "$(LC_ALL=C stat -c "%d:%i:%u:%f" "$directory")" = "$directory_record" ] || exit 1
        printf "cover-ready\n"
    ' sh "$LOCK_DIR" "$cover_expected" "$cover_leaves" "$(id -u)" \
        nonce "$NONCE" generation "$SLEEP_GENERATION:$NONCE" application-pid "$SLEEP_APP_PID" \
        epoch "$SLEEP_EPOCH" sleep-request "$SLEEP_APP_PID $NONCE $SLEEP_EPOCH"; then
        [ "$(native_read_hex)" = 636f7665722d72656164790a ] && sleep_cover_context_is_current
    else
        return 1
    fi
}

retain_sleep_cover() {
    [ "$COVER_SLEEP_SUPPORTED" -eq 1 ] || return 1
    cover_sleep_can_own_display && sleep_cover_records_match || return 1
    # The Rust ready record certifies its independent full-update completion,
    # saved checkpoint and paused native display/input/activity barrier.
    case "$(process_state "$APP_PID")" in T*|t*) ;; *) return 1 ;; esac
    SLEEP_COVER_ACTIVE=1
}

take_display() {
    [ "$DISPLAY_OWNED" -eq 0 ] || return 0
    DISPLAY_OWNED=1; PAUSED_WM_RECORDS=''; STATUSBAR_STOPPED=0; STATUSBAR_QUIESCED=0; STOCK_REPAINT_PENDING=1
    # Freeze the WM before stopping statusbar can damage the stock X11 scene.
    if command -v pidof >/dev/null 2>&1; then
        for wm_pid in $(pidof awesome 2>/dev/null || :); do
            valid_pid "$wm_pid" || continue
            case "$(process_state "$wm_pid")" in ''|T*|t*|Z*) continue ;; esac
            wm_birth=$(process_birth "$wm_pid")
            [ -n "$wm_birth" ] || continue
            if kill -STOP "$wm_pid" 2>/dev/null; then PAUSED_WM_RECORDS="$PAUSED_WM_RECORDS $wm_pid:$wm_birth"; fi
        done
    fi
    if command -v status >/dev/null 2>&1 && command -v stop >/dev/null 2>&1 && command -v start >/dev/null 2>&1; then
        case "$(status statusbar 2>/dev/null || :)" in
            *start/running*)
                if stop statusbar >/dev/null 2>&1; then
                    STATUSBAR_STOPPED=1
                    case "$(status statusbar 2>/dev/null || :)" in *stop/waiting*) STATUSBAR_QUIESCED=1 ;; esac
                fi ;;
        esac
    fi
}

stop_redraw() {
    [ -n "$REDRAW_PID" ] || return 0
    signal_child "$REDRAW_PID" "$ROOT_PID" KILL || :
    wait "$REDRAW_PID" 2>/dev/null || :
    REDRAW_PID=''
}
redraw_stock() {
    xrefresh -display :0 -none >/dev/null 2>&1 3>&- & REDRAW_PID=$!
    redraw_remaining=2
    while owned_pid "$REDRAW_PID" "$ROOT_PID" && [ "$redraw_remaining" -gt 0 ]; do sleep 1; redraw_remaining=$((redraw_remaining - 1)); done
    stop_redraw
}
release_display() {
    if [ "$DISPLAY_OWNED" -eq 1 ]; then
        for wm_record in $PAUSED_WM_RECORDS; do
            wm_pid=${wm_record%%:*}; wm_birth=${wm_record#*:}
            if [ "$(process_birth "$wm_pid")" = "$wm_birth" ]; then kill -CONT "$wm_pid" 2>/dev/null || :; fi
        done
        PAUSED_WM_RECORDS=''
        if [ "$STATUSBAR_STOPPED" -eq 1 ]; then start statusbar >/dev/null 2>&1 || :; fi
        STATUSBAR_STOPPED=0; STATUSBAR_QUIESCED=0; DISPLAY_OWNED=0
    fi
    # Direct framebuffer writes bypass X damage. Preserve one final repaint
    # across quiet power handoffs, including an exit while the App is parked.
    # USB takeover keeps this request unconsumed and never repaints its view.
    if [ "$1" -eq 1 ] && [ "$STOCK_REPAINT_PENDING" -eq 1 ]; then
        STOCK_REPAINT_PENDING=0
        if command -v xrefresh >/dev/null 2>&1; then redraw_stock; fi
    fi
}
handoff_power_display() {
    if retain_sleep_cover; then return 0; fi
    clear_sleep_cover_records
    # Stock powerd paints its own screensaver transition. Forcing the X11 scene
    # here can expose the native bookshelf before sleep or retained restoration.
    release_display 0
}

# Background functions inherit $$ from the supervisor. This builtin reads the
# actual helper PID before any subprocess is created.
helper_setup() {
    trap - 0 HUP INT TERM
    read -r HELPER_SELF HELPER_STAT_REST </proc/self/stat
    HELPER_PREFIX=$1; HELPER_CHILD=''; HELPER_COMMAND=''; HELPER_STOPPING=0; HELPER_INTERRUPTED=0
    trap helper_cancel HUP INT TERM
}
helper_cancel() {
    HELPER_STOPPING=1; HELPER_INTERRUPTED=1
    if [ -n "$HELPER_CHILD" ]; then signal_child "$HELPER_CHILD" "$HELPER_SELF" TERM || :; fi
    if [ -n "$HELPER_COMMAND" ]; then
        helper_signal=TERM; [ "$HELPER_PREFIX" != observer ] || helper_signal=INT
        signal_child "$HELPER_COMMAND" "$HELPER_SELF" "$helper_signal" || :
    fi
}
helper_wait() {
    while :; do
        HELPER_INTERRUPTED=0
        if wait "$1"; then HELPER_STATUS=0; else HELPER_STATUS=$?; fi
        [ "$HELPER_INTERRUPTED" -eq 1 ] || break
    done
    return "$HELPER_STATUS"
}
helper_sleep() {
    sleep 1 & HELPER_CHILD=$!
    write_record "$HELPER_PREFIX-child" "$HELPER_CHILD"
    if [ "$HELPER_STOPPING" -eq 1 ]; then signal_child "$HELPER_CHILD" "$HELPER_SELF" TERM || :; fi
    helper_wait "$HELPER_CHILD" || :
    HELPER_CHILD=''; rm -f "$LOCK_DIR/$HELPER_PREFIX-child"
    [ "$HELPER_STOPPING" -eq 0 ]
}
stop_helper() {
    helper_stop_pid=$1; helper_stop_prefix=$2
    [ -n "$helper_stop_pid" ] || return 0
    signal_child "$helper_stop_pid" "$ROOT_PID" TERM || :
    helper_stop_remaining=2
    while owned_pid "$helper_stop_pid" "$ROOT_PID" && [ "$helper_stop_remaining" -gt 0 ]; do
        sleep 1; helper_stop_remaining=$((helper_stop_remaining - 1))
    done
    if owned_pid "$helper_stop_pid" "$ROOT_PID"; then
        for helper_stop_suffix in child command; do
            helper_stop_child=$(cat "$LOCK_DIR/$helper_stop_prefix-$helper_stop_suffix" 2>/dev/null || :)
            if [ -n "$helper_stop_child" ]; then signal_child "$helper_stop_child" "$helper_stop_pid" KILL || :; fi
        done
        # Allow child reaping before terminating an unresponsive helper.
        sleep 1; signal_child "$helper_stop_pid" "$ROOT_PID" KILL || :
    fi
    wait "$helper_stop_pid" 2>/dev/null || :
    rm -f "$LOCK_DIR/$helper_stop_prefix-child" "$LOCK_DIR/$helper_stop_prefix-command"
}
stop_ack() { stop_helper "$ACK_PID" ack; ACK_PID=''; }
stop_wake() { stop_helper "$WAKE_PID" wake; WAKE_PID=''; }
stop_resume() { stop_helper "$RESUME_PID" resume; RESUME_PID=''; }

run_observer() {
    helper_setup observer
    lipc-wait-event -m -s 0 com.lab126.powerd goingToScreenSaver,outOfScreenSaver,exitingScreenSaver,usbConfigured >&3 2>/dev/null 3>&- &
    HELPER_COMMAND=$!; write_record observer-command "$HELPER_COMMAND"
    if [ "$HELPER_STOPPING" -eq 1 ]; then signal_child "$HELPER_COMMAND" "$HELPER_SELF" INT || :; fi
    helper_wait "$HELPER_COMMAND" || :
    HELPER_COMMAND=''; rm -f "$LOCK_DIR/observer-command"
    if [ "$HELPER_STOPPING" -eq 0 ]; then printf 'observer-exited %s %s\n' "$NONCE" "$HELPER_STATUS" >&3; fi
}
start_observer() {
    run_observer & OBSERVER_PID=$!
    # Admit native observation as soon as the current observer owns its source.
    observer_checks=100
    while [ "$observer_checks" -gt 0 ]; do
        observer_command=$(cat "$LOCK_DIR/observer-command" 2>/dev/null || :)
        if owned_pid "$OBSERVER_PID" "$ROOT_PID" && owned_pid "$observer_command" "$OBSERVER_PID"; then return 0; fi
        owned_pid "$OBSERVER_PID" "$ROOT_PID" || fail 'Power event observation ended before startup.'
        observer_checks=$((observer_checks - 1))
        usleep 20000
    done
    fail 'Power event observer ownership was not established before startup.'
}
run_application() {
    runner_generation=$1; runner_resume=$2
    helper_setup runner
    if [ "$runner_resume" -eq 1 ]; then
        "$APP_DIR/aidoku" --kindle --device "$PROFILE" --data-dir "$DATA_DIR" --resume 3>&- &
    else
        "$APP_DIR/aidoku" --kindle --device "$PROFILE" --data-dir "$DATA_DIR" 3>&- &
    fi
    HELPER_COMMAND=$!; runner_application=$HELPER_COMMAND
    write_record application-pid "$runner_application"; write_record runner-command "$runner_application"
    printf 'app-started %s %s %s\n' "$runner_generation" "$runner_application" "$NONCE" >&3
    if [ "$HELPER_STOPPING" -eq 1 ]; then signal_child "$HELPER_COMMAND" "$HELPER_SELF" TERM || :; fi
    helper_wait "$HELPER_COMMAND" || :
    HELPER_COMMAND=''; rm -f "$LOCK_DIR/runner-command"
    printf 'app-exited %s %s %s %s\n' "$runner_generation" "$runner_application" "$NONCE" "$HELPER_STATUS" >&3
}
start_application() {
    APP_GENERATION=$((APP_GENERATION + 1)); APP_PID=''
    clear_sleep_cover_records
    rm -f "$LOCK_DIR/application-pid" "$LOCK_DIR/sleep-request" "$LOCK_DIR/sleep-ready" "$LOCK_DIR/resume-request" "$LOCK_DIR/resume-ready" "$LOCK_DIR/fallback-sleep" "$LOCK_DIR/ordinary-exit" "$LOCK_DIR/native-portrait-calibration" "$LOCK_DIR/native-portrait-calibration.pending" "$LOCK_DIR/resume-native-portrait-calibration" "$LOCK_DIR/resume-native-portrait-calibration.pending"
    rm -f "$LOCK_DIR"/resume-native-portrait-calibration-*.pending
    write_record generation "$APP_GENERATION:$NONCE"
    AIDOKU_LAUNCH_GENERATION=$APP_GENERATION; export AIDOKU_LAUNCH_GENERATION
    set_phase starting
    # Every ELF generation has its own native WM/GETVAR anchor. The bounded
    # reader publishes its typed private record before awesome is stopped.
    if [ "$DISPLAY_OWNED" -eq 1 ]; then release_display 0; fi
    startup_capability_argument=''
    if [ "$COVER_SLEEP_CHECKED" -eq 0 ]; then startup_capability_argument=--capture-sleep-capability; fi
    native_read_command "$APP_DIR/aidoku" --kindle --device "$PROFILE" --capture-native-portrait $startup_capability_argument || fail 'The native startup portrait calibration is unavailable.'
    case "$(native_read_hex)" in
        5055424c495348454420310a) COVER_SLEEP_SUPPORTED=1 ;;
        5055424c495348454420300a) COVER_SLEEP_SUPPORTED=0 ;;
        5055424c49534845440a)
            if [ "$COVER_SLEEP_CHECKED" -eq 0 ]; then configure_cover_sleep; fi ;;
        *) fail 'The native startup portrait publication is invalid.' ;;
    esac
    COVER_SLEEP_CHECKED=1
    take_display
    run_application "$APP_GENERATION" "$1" & RUNNER_PID=$!
}
stop_application() {
    if [ -z "$APP_PID" ]; then APP_PID=$(cat "$LOCK_DIR/application-pid" 2>/dev/null || :); fi
    if [ -n "$APP_PID" ] && [ -n "$RUNNER_PID" ] && owned_pid "$APP_PID" "$RUNNER_PID"; then
        # A stopped ELF cannot process TERM; never continue only the runner.
        signal_child "$APP_PID" "$RUNNER_PID" CONT || :
        signal_child "$APP_PID" "$RUNNER_PID" TERM || :
        app_stop_remaining=5
        while owned_pid "$APP_PID" "$RUNNER_PID" && [ "$app_stop_remaining" -gt 0 ]; do sleep 1; app_stop_remaining=$((app_stop_remaining - 1)); done
        signal_child "$APP_PID" "$RUNNER_PID" KILL || :
    fi
    stop_helper "$RUNNER_PID" runner
    RUNNER_PID=''; APP_PID=''; rm -f "$LOCK_DIR/application-pid"
}

run_ack_watch() {
    ack_epoch=$1; ack_generation=$2; ack_application=$3; ack_budget=$4
    helper_setup ack; ack_remaining=$ack_budget
    while :; do
        [ "$HELPER_STOPPING" -eq 0 ] || return 0
        if matches_record sleep-ready "$ack_application $NONCE $ack_epoch"; then
            printf 'sleep-ready %s %s %s %s\n' "$ack_epoch" "$ack_generation" "$ack_application" "$NONCE" >&3; return 0
        fi
        [ "$ack_remaining" -gt 0 ] || break
        helper_sleep || return 0; ack_remaining=$((ack_remaining - 1))
    done
    printf 'ack-timeout %s %s %s %s\n' "$ack_epoch" "$ack_generation" "$ack_application" "$NONCE" >&3
}
begin_sleep() {
    SLEEP_GENERATION=$APP_GENERATION; SLEEP_APP_PID=$APP_PID
    SLEEP_APP_BIRTH=$(process_birth "$APP_PID" 2>/dev/null || :)
    SLEEP_RUNNER_BIRTH=$(process_birth "$RUNNER_PID" 2>/dev/null || :)
    NATIVE_RESUME_ATTEMPTS=0
    clear_sleep_cover_records
    COVER_SEQUENCE=$((COVER_SEQUENCE + 1)); SLEEP_COVER_SEQUENCE=$COVER_SEQUENCE
    write_record sleep-request "$SLEEP_APP_PID $NONCE $SLEEP_EPOCH"
    rm -f "$LOCK_DIR/sleep-ready" "$LOCK_DIR/resume-request" "$LOCK_DIR/resume-ready" "$LOCK_DIR/fallback-sleep" "$LOCK_DIR/resume-native-portrait-calibration" "$LOCK_DIR/resume-native-portrait-calibration.pending"
    rm -f "$LOCK_DIR"/resume-native-portrait-calibration-*.pending
    sleep_ack_budget=5
    if cover_sleep_can_own_display && sleep_cover_context_is_current; then
        write_record sleep-cover-request "$SLEEP_GENERATION $SLEEP_APP_PID $NONCE $SLEEP_EPOCH $SLEEP_COVER_SEQUENCE suspend"
        if sleep_cover_records_match request; then
            # The independent presentation deadline precedes checkpoint/pause.
            # Keep the existing five-second budget for every stock transition.
            sleep_ack_budget=12
        else
            clear_sleep_cover_records
        fi
    fi
    set_phase pausing; signal_child "$APP_PID" "$RUNNER_PID" USR1 || :
    run_ack_watch "$SLEEP_EPOCH" "$SLEEP_GENERATION" "$SLEEP_APP_PID" "$sleep_ack_budget" & ACK_PID=$!
}
monotonic_centiseconds() {
    read -r clock_uptime clock_idle </proc/uptime || return 1
    case "$clock_uptime" in *.*) ;; *) return 1 ;; esac
    clock_seconds=${clock_uptime%.*}; clock_fraction=${clock_uptime#*.}
    case "$clock_seconds" in ''|*[!0-9]*|0[0-9]*) return 1 ;; esac
    case "$clock_fraction" in [0-9][0-9]) ;; *) return 1 ;; esac
    # Prefix the fraction to keep 08 and 09 out of shell octal arithmetic.
    printf '%s\n' "$((clock_seconds * 100 + 1$clock_fraction - 100))"
}
run_wake_probe() {
    wake_epoch=$1; wake_generation=$2; wake_application=$3
    helper_setup wake; wake_remaining=10; wake_park_reported=0
    if wake_now=$(monotonic_centiseconds); then wake_deadline=$((wake_now + 1000)); else wake_deadline=0; fi
    while [ "$wake_remaining" -gt 0 ] && [ "$wake_deadline" -gt 0 ]; do
        [ "$HELPER_STOPPING" -eq 0 ] || return 0
        if wake_now=$(monotonic_centiseconds); then :; else break; fi
        wake_budget=$((wake_deadline - wake_now))
        [ "$wake_budget" -gt 0 ] || break
        wake_timeout_ms=$((wake_budget * 10))
        if [ "$wake_timeout_ms" -gt 700 ]; then wake_timeout_ms=700; fi
        [ "$wake_timeout_ms" -ge 1 ] && [ "$wake_timeout_ms" -le 700 ] || break
        if [ "$wake_park_reported" -eq 0 ] && matches_record sleep-ready "$wake_application $NONCE $wake_epoch"; then
            printf 'wake-parked %s %s %s %s\n' "$wake_epoch" "$wake_generation" "$wake_application" "$NONCE" >&3; wake_park_reported=1
        fi
        rm -f "$LOCK_DIR/wake-power"
        # The same executable supplies a bounded native query before loading
        # fonts, data or UI. A quick reply must not wait for a timer.
        "$APP_DIR/aidoku" --native-power-state --native-power-state-timeout-ms "$wake_timeout_ms" >"$LOCK_DIR/wake-power" 2>/dev/null 3>&- &
        HELPER_COMMAND=$!; write_record wake-command "$HELPER_COMMAND"
        if [ "$HELPER_STOPPING" -eq 1 ]; then signal_child "$HELPER_COMMAND" "$HELPER_SELF" TERM || :; fi
        if helper_wait "$HELPER_COMMAND"; then wake_query_status=0; else wake_query_status=$?; fi
        HELPER_COMMAND=''; rm -f "$LOCK_DIR/wake-command"
        [ "$HELPER_STOPPING" -eq 0 ] || return 0
        if wake_now=$(monotonic_centiseconds); then :; else break; fi
        [ "$wake_now" -lt "$wake_deadline" ] || break
        if [ "$wake_query_status" -eq 0 ] && [ "$(cat "$LOCK_DIR/wake-power" 2>/dev/null || :)" = active ]; then
            printf 'wake-active %s %s %s %s\n' "$wake_epoch" "$wake_generation" "$wake_application" "$NONCE" >&3; return 0
        fi
        wake_remaining=$((wake_remaining - 1))
        [ "$wake_remaining" -gt 0 ] || break
        # Preserve the ten-second overall budget with integer-only backoffs.
        # An inactive result with less than one second left ends the probe.
        [ "$((wake_deadline - wake_now))" -ge 100 ] || break
        helper_sleep || return 0
    done
    printf 'wake-timeout %s %s %s %s\n' "$wake_epoch" "$wake_generation" "$wake_application" "$NONCE" >&3
}
start_wake_probe() {
    [ -z "$WAKE_PID" ] || return 0
    # A retained cover keeps DISPLAY_OWNED=1. It still needs bounded active-state
    # retries; applying a stock-only ownership guard here strands that wake.
    run_wake_probe "$SLEEP_EPOCH" "$SLEEP_GENERATION" "$SLEEP_APP_PID" & WAKE_PID=$!
}
run_resume_watch() {
    resume_epoch=$1; resume_generation=$2; resume_application=$3
    helper_setup resume; resume_remaining=5
    while [ "$resume_remaining" -gt 0 ]; do
        [ "$HELPER_STOPPING" -eq 0 ] || return 0
        if matches_record resume-ready "$resume_application $NONCE $resume_epoch"; then
            printf 'resume-ready %s %s %s %s\n' "$resume_epoch" "$resume_generation" "$resume_application" "$NONCE" >&3; return 0
        fi
        helper_sleep || return 0
        printf 'resume-check %s %s %s %s\n' "$resume_epoch" "$resume_generation" "$resume_application" "$NONCE" >&3
        resume_remaining=$((resume_remaining - 1))
    done
    printf 'resume-timeout %s %s %s %s\n' "$resume_epoch" "$resume_generation" "$resume_application" "$NONCE" >&3
}
valid_sleep_message() {
    [ "$1" = "$SLEEP_EPOCH" ] && [ "$2" = "$SLEEP_GENERATION" ] && [ "$3" = "$SLEEP_APP_PID" ] && [ "$4" = "$NONCE" ]
}
send_resume() {
    # Signals have no payload. Bind both CONT and USR2 to this exact sleep so a
    # delayed signal from an earlier wake cannot cancel a later sleep request.
    write_record resume-request "$APP_PID $NONCE $SLEEP_EPOCH"
    signal_child "$APP_PID" "$RUNNER_PID" CONT || :
    signal_child "$APP_PID" "$RUNNER_PID" USR2 || :
}
cover_wake_power_is_active() {
    # A user-wake intent is required by the caller before either observation.
    native_read_command lipc-get-prop -s com.lab126.powerd state || return 1
    case "$(native_read_hex)" in 616374697665|6163746976650a) ;; *) return 1 ;; esac
    native_read_command lipc-get-prop -s com.lab126.powerd state || return 1
    case "$(native_read_hex)" in 616374697665|6163746976650a) ;; *) return 1 ;; esac
}
begin_user_wake() {
    [ "$SLEEP_PENDING" -eq 1 ] || return 0
    NATIVE_RESUME_ATTEMPTS=0
    WAKE_PENDING=1; stop_ack
    if [ "$APP_PHASE" = pausing ]; then set_phase wake-live; fi
    if matches_record sleep-ready "$SLEEP_APP_PID $NONCE $SLEEP_EPOCH"; then handoff_power_display; fi
    start_wake_probe
}
resume_application() {
    [ "$SLEEP_PENDING" -eq 1 ] && [ "$WAKE_PENDING" -eq 1 ] || return 0
    stop_ack; stop_wake
    if [ "$SLEEP_COVER_ACTIVE" -eq 1 ] && [ "$DISPLAY_OWNED" -eq 1 ]; then
        if ! cover_sleep_can_own_display || ! sleep_cover_records_match; then
            clear_sleep_cover_records
            release_display 0
        elif ! cover_wake_power_is_active; then
            WAKE_PENDING=1; start_wake_probe
            return 0
        fi
    fi
    WAKE_PENDING=0
    if [ "$APP_PHASE" = fallback ]; then SLEEP_PENDING=0; start_application 1; return 0; fi
    if [ -n "$APP_PID" ] && owned_pid "$APP_PID" "$RUNNER_PID"; then
        # The current wake passed the native active-state gate. Freeze stock
        # drawing before the bounded portrait authorization transaction.
        take_display
        # This permission carries no direction or cached output mode. The
        # suspended App resolves fresh GETVAR against this generation's anchor
        # and obtains holding from its separate trusted sensor snapshot.
        if ! native_read_command "$APP_DIR/aidoku" --kindle --device "$PROFILE" --authorize-native-portrait-resume "$SLEEP_EPOCH"; then
            # A failed fresh portrait permission revokes retained ownership.
            clear_sleep_cover_records
            release_display 0
            printf '%s\n' 'aidoku: Waiting for a current native portrait resume permission.' >&2
            WAKE_PENDING=1
            NATIVE_RESUME_ATTEMPTS=$((NATIVE_RESUME_ATTEMPTS + 1))
            if [ "$NATIVE_RESUME_ATTEMPTS" -lt 3 ]; then start_wake_probe; fi
            return 0
        fi
        [ "$(native_read_hex)" = 5055424c49534845440a ] || fail 'The native portrait resume publication is invalid.'
        send_resume
        SLEEP_PENDING=0; set_phase resuming; stop_resume
        run_resume_watch "$SLEEP_EPOCH" "$APP_GENERATION" "$APP_PID" & RESUME_PID=$!
    else
        stop_application; SLEEP_PENDING=0; start_application 1
    fi
}

cleanup() {
    cleanup_status=$1; set +e; trap - 0; trap '' HUP INT TERM; APP_PHASE=closing
    stop_native_read; stop_redraw; stop_ack; stop_wake; stop_resume; stop_application; stop_helper "$OBSERVER_PID" observer; OBSERVER_PID=''
    clear_sleep_cover_records
    release_display "$EXIT_REDRAW"
    if [ "$EVENT_FD_OPEN" -eq 1 ]; then exec 3>&-; EVENT_FD_OPEN=0; fi
    for record in pid nonce generation phase epoch application-pid power-events sleep-request sleep-ready resume-request resume-ready fallback-sleep ordinary-exit lifecycle-state.json native-portrait-calibration resume-native-portrait-calibration native-read-output native-read-done native-read-command native-read-go runner-child runner-command observer-child observer-command ack-child ack-command wake-child wake-command wake-power resume-child resume-command; do rm -f "$LOCK_DIR/$record" "$LOCK_DIR/$record.pending"; done
    rm -f "$LOCK_DIR"/resume-native-portrait-calibration-*.pending
    rmdir "$LOCK_DIR" 2>/dev/null || :
    printf 'aidoku: Launcher worker exited with status %s.\n' "$cleanup_status" >&2
    exit "$cleanup_status"
}
trap 'cleanup "$?"' 0
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM
command -v lipc-wait-event >/dev/null 2>&1 || fail 'Power-event observation is required.'
command -v setsid >/dev/null 2>&1 && command -v usleep >/dev/null 2>&1 || fail 'A bounded private native-reader group is required.'
NONCE=$(tr -d '\n-' </proc/sys/kernel/random/uuid)
case "$NONCE" in ''|*[!0-9a-fA-F]*) fail 'The launcher nonce is invalid.' ;; esac
[ "${#NONCE}" -eq 32 ] || fail 'The launcher nonce has an invalid length.'
write_record nonce "$NONCE"; write_record epoch 0
AIDOKU_LAUNCHER_DIR=$LOCK_DIR; AIDOKU_LAUNCH_NONCE=$NONCE
export AIDOKU_HOME AIDOKU_LAUNCHER_DIR AIDOKU_LAUNCH_NONCE
mkfifo "$LOCK_DIR/power-events"
# Keep both ends open so transitions cannot drop queued wake or exit events.
exec 3<>"$LOCK_DIR/power-events"; EVENT_FD_OPEN=1
start_observer
start_application 0

while IFS= read -r event <&3; do
    case "$event" in
        goingToScreenSaver*)
            stop_resume; stop_wake; WAKE_PENDING=0
            if [ "$SLEEP_PENDING" -eq 0 ]; then
                SLEEP_EPOCH=$((SLEEP_EPOCH + 1)); write_record epoch "$SLEEP_EPOCH"
                SLEEP_PENDING=1; SLEEP_GENERATION=$APP_GENERATION; SLEEP_APP_PID=${APP_PID:-0}
                if [ "$APP_PHASE" = active ] || [ "$APP_PHASE" = resuming ]; then begin_sleep; fi
            fi ;;
        outOfScreenSaver*) begin_user_wake ;;
        exitingScreenSaver*)
            # Dismissal alone is not a user wake. RTC/background wake messages
            # and their companion dismissal never acquire native display/input.
            if [ "$WAKE_PENDING" -eq 1 ]; then start_wake_probe; fi ;;
        wakeupFromSuspend*) ;;
        usbConfigured*) EXIT_REDRAW=0; exit 143 ;;
        'app-started '*)
            set -- $event
            [ "$#" -eq 4 ] && [ "$2" = "$APP_GENERATION" ] && [ "$4" = "$NONCE" ] && valid_pid "$3" || continue
            [ "$APP_PHASE" = starting ] && matches_record application-pid "$3" || continue
            APP_PID=$3; set_phase active
            if [ "$SLEEP_PENDING" -eq 1 ]; then
                begin_sleep
                if [ "$WAKE_PENDING" -eq 1 ]; then stop_ack; set_phase wake-live; stop_wake; start_wake_probe; fi
            fi ;;
        'app-exited '*)
            set -- $event
            [ "$#" -eq 5 ] && [ "$2" = "$APP_GENERATION" ] && [ "$3" = "$APP_PID" ] && [ "$4" = "$NONCE" ] || continue
            case "$5" in ''|*[!0-9]*) continue ;; esac
            app_exit_status=$5; app_exit_phase=$APP_PHASE; wait "$RUNNER_PID" 2>/dev/null || :; RUNNER_PID=''; APP_PID=''
            if ordinary_exit "$3"; then exit "$app_exit_status"; fi
            if [ "$SLEEP_PENDING" -eq 0 ] && [ "$app_exit_phase" != resuming ]; then exit "$app_exit_status"; fi
            if [ "$app_exit_phase" = resuming ]; then stop_resume; SLEEP_PENDING=1; WAKE_PENDING=1; fi
            stop_ack; clear_sleep_cover_records; release_display 1; set_phase fallback
            if [ "$WAKE_PENDING" -eq 1 ]; then start_wake_probe; fi ;;
        'sleep-ready '*)
            set -- $event
            [ "$#" -eq 5 ] && valid_sleep_message "$2" "$3" "$4" "$5" || continue
            [ "$SLEEP_PENDING" -eq 1 ] && matches_record sleep-ready "$SLEEP_APP_PID $NONCE $SLEEP_EPOCH" || continue
            if ordinary_exit "$SLEEP_APP_PID"; then exit 0; fi
            if [ "$WAKE_PENDING" -eq 1 ]; then handoff_power_display; continue; fi
            [ "$APP_PHASE" = pausing ] || continue
            if signal_child "$APP_PID" "$RUNNER_PID" STOP; then set_phase parked; handoff_power_display; fi
            stop_ack ;;
        'ack-timeout '*)
            set -- $event
            [ "$#" -eq 5 ] && valid_sleep_message "$2" "$3" "$4" "$5" || continue
            [ "$APP_PHASE" = pausing ] && [ "$SLEEP_PENDING" -eq 1 ] && [ "$WAKE_PENDING" -eq 0 ] || continue
            if ordinary_exit "$SLEEP_APP_PID"; then exit 0; fi
            stop_ack; write_record fallback-sleep "$SLEEP_APP_PID $NONCE $SLEEP_EPOCH"
            stop_application; clear_sleep_cover_records; release_display 1; set_phase fallback ;;
        'wake-parked '*)
            set -- $event
            [ "$#" -eq 5 ] && valid_sleep_message "$2" "$3" "$4" "$5" || continue
            [ "$SLEEP_PENDING" -eq 1 ] && [ "$WAKE_PENDING" -eq 1 ] && matches_record sleep-ready "$SLEEP_APP_PID $NONCE $SLEEP_EPOCH" || continue
            handoff_power_display ;;
        'wake-active '*)
            set -- $event
            [ "$#" -eq 5 ] && valid_sleep_message "$2" "$3" "$4" "$5" || continue
            [ "$SLEEP_PENDING" -eq 1 ] && [ "$WAKE_PENDING" -eq 1 ] || continue
            if ordinary_exit "$SLEEP_APP_PID"; then exit 0; fi
            resume_application ;;
        'wake-timeout '*)
            set -- $event
            [ "$#" -eq 5 ] && valid_sleep_message "$2" "$3" "$4" "$5" || continue
            stop_wake
            if [ "$SLEEP_COVER_ACTIVE" -eq 1 ]; then clear_sleep_cover_records; release_display 0; fi
            printf '%s\n' 'aidoku: Waiting for native screen-saver dismissal or unlock.' >&2 ;;
        'resume-check '*)
            set -- $event
            [ "$#" -eq 5 ] && valid_sleep_message "$2" "$3" "$4" "$5" || continue
            [ "$APP_PHASE" = resuming ] && [ "$SLEEP_PENDING" -eq 0 ] && [ "$4" = "$APP_PID" ] || continue
            if matches_record resume-ready "$APP_PID $NONCE $SLEEP_EPOCH"; then stop_resume; clear_sleep_cover_records; set_phase active; continue; fi
            send_resume ;;
        'resume-ready '*)
            set -- $event
            [ "$#" -eq 5 ] && valid_sleep_message "$2" "$3" "$4" "$5" || continue
            if [ "$APP_PHASE" = resuming ] && [ "$4" = "$APP_PID" ] && matches_record resume-ready "$APP_PID $NONCE $SLEEP_EPOCH"; then stop_resume; clear_sleep_cover_records; set_phase active; fi ;;
        'resume-timeout '*)
            set -- $event
            [ "$#" -eq 5 ] && valid_sleep_message "$2" "$3" "$4" "$5" || continue
            [ "$APP_PHASE" = resuming ] && [ "$4" = "$APP_PID" ] || continue
            stop_resume
            if matches_record resume-ready "$APP_PID $NONCE $SLEEP_EPOCH"; then clear_sleep_cover_records; set_phase active; continue; fi
            if ordinary_exit "$APP_PID"; then exit 0; fi
            write_record fallback-sleep "$APP_PID $NONCE $SLEEP_EPOCH"
            stop_application; clear_sleep_cover_records; release_display 1; set_phase fallback
            SLEEP_PENDING=1; WAKE_PENDING=1; start_wake_probe ;;
        'observer-exited '*)
            set -- $event
            [ "$#" -eq 3 ] && [ "$2" = "$NONCE" ] || continue
            fail 'Power-event observation ended unexpectedly.' ;;
    esac
done
fail 'The power-event channel closed unexpectedly.'
