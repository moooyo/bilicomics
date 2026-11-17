# Guarded native recovery for the isolated aggregate startup supervisor.
# This fragment is inlined after the supervisor's process helper functions.
# It never signals application processes or workers. The caller must implement
# owned_gone as a fail-closed check of recorded births and unknown owners.
# Required evidence: native-before.tsv (PID, birth, executable), launcher.pid
# (PID, birth), statusbar-before.txt, and the existing case/batch owner files.

fallback_note() {
    printf '%s\n' "$*" >>"$out/native-fallback.log"
}

fallback_owner_drained() {
    [ "${launcher_reaped:-0}" -eq 1 ] || return 1
    case "$launcher_pid:$launcher_birth" in ''|*[!0-9:]*|:*|*:) return 1;; esac
    [ "$launcher_pid" -gt 1 ] || return 1
    [ -f "$out/launcher.pid" ] && [ ! -L "$out/launcher.pid" ] || return 1
    IFS=' ' read -r fallback_recorded_pid fallback_recorded_birth fallback_recorded_extra <"$out/launcher.pid" || return 1
    [ "$fallback_recorded_pid:$fallback_recorded_birth:$fallback_recorded_extra" = "$launcher_pid:$launcher_birth:" ] || return 1
    if [ -d "/proc/$launcher_pid" ]; then
        proc_read "$launcher_pid" || return 1
        [ "$proc_birth" != "$launcher_birth" ] || return 1
    fi
    owned_gone || return 1
    return 0
}

fallback_lock_matches() {
    [ -n "$lock_identity" ] && [ -n "$lock_nonce" ] && [ -n "$lock_generation" ] || return 1
    case "$lock_nonce" in ''|*[!0-9a-f]*) return 1;; esac
    [ "${#lock_nonce}" -eq 32 ] || return 1
    fallback_generation_number=${lock_generation%%:*}
    case "$fallback_generation_number" in ''|*[!0-9]*) return 1;; esac
    [ "$fallback_generation_number" -gt 0 ] || return 1
    [ "$lock_generation" = "$fallback_generation_number:$lock_nonce" ] || return 1
    [ -d "$lock" ] && [ ! -L "$lock" ] || return 1
    [ "$(stat -c '%d:%i' "$lock" 2>/dev/null || :)" = "$lock_identity" ] || return 1
    for fallback_anchor in pid nonce generation; do
        [ -f "$lock/$fallback_anchor" ] && [ ! -L "$lock/$fallback_anchor" ] || return 1
        [ "$(stat -c '%h' "$lock/$fallback_anchor" 2>/dev/null || :)" = 1 ] || return 1
    done
    [ "$(cat "$lock/pid" 2>/dev/null || :)" = "$launcher_pid" ] || return 1
    [ "$(cat "$lock/nonce" 2>/dev/null || :)" = "$lock_nonce" ] || return 1
    [ "$(cat "$lock/generation" 2>/dev/null || :)" = "$lock_generation" ] || return 1
    return 0
}

fallback_context_matches() {
    [ -d "$stage" ] && [ ! -L "$stage" ] || return 1
    [ -d "$runtime" ] && [ ! -L "$runtime" ] || return 1
    [ -d "$out" ] && [ ! -L "$out" ] || return 1
    for fallback_context_file in "$stage/owner" "$runtime/owner" "$out/owner"; do
        [ -f "$fallback_context_file" ] && [ ! -L "$fallback_context_file" ] || return 1
    done
    [ "$(cat "$stage/owner" 2>/dev/null || :)" = "$run_id" ] || return 1
    [ "$(cat "$runtime/owner" 2>/dev/null || :)" = "$run_id" ] || return 1
    [ "$(cat "$out/owner" 2>/dev/null || :)" = "$token" ] || return 1
    return 0
}

fallback_native_baselines_match() {
    fallback_required_state=$1
    [ -f "$out/native-before.tsv" ] && [ ! -L "$out/native-before.tsv" ] || return 1
    fallback_baseline_count=0
    while IFS=' ' read -r fallback_baseline_pid fallback_baseline_birth fallback_baseline_exe fallback_baseline_extra; do
        [ -n "$fallback_baseline_pid" ] || return 1
        case "$fallback_baseline_pid:$fallback_baseline_birth" in *[!0-9:]*|:*|*:) return 1;; esac
        [ "$fallback_baseline_pid" -gt 1 ] && [ -z "$fallback_baseline_extra" ] || return 1
        case "$fallback_baseline_exe" in /*) ;; *) return 1;; esac
        fallback_baseline_count=$((fallback_baseline_count + 1))
        [ "$fallback_baseline_count" -le 16 ] || return 1
        same_birth "$fallback_baseline_pid" "$fallback_baseline_birth" || return 1
        [ "$(readlink "/proc/$fallback_baseline_pid/exe" 2>/dev/null || :)" = "$fallback_baseline_exe" ] || return 1
        same_birth "$fallback_baseline_pid" "$fallback_baseline_birth" || return 1
        case "$fallback_required_state:$proc_state" in
            running:S|running:R|recoverable:S|recoverable:R|recoverable:T|recoverable:t) ;;
            *) return 1;;
        esac
    done <"$out/native-before.tsv"
    [ "$fallback_baseline_count" -gt 0 ] || return 1
    return 0
}

fallback_statusbar_baseline_running() {
    [ -f "$out/statusbar-before.txt" ] && [ ! -L "$out/statusbar-before.txt" ] || return 1
    fallback_baseline_status=$(cat "$out/statusbar-before.txt" 2>/dev/null || :) || return 1
    case "$fallback_baseline_status" in 'statusbar start/running'*) return 0;; *) return 1;; esac
}

fallback_service_child_matches() {
    same_birth "$fallback_command_pid" "$fallback_command_birth" || return 1
    [ "$proc_parent" = "$$" ] || return 1
    has_case_token "$fallback_command_pid" || return 1
    fallback_command_exe=$(readlink "/proc/$fallback_command_pid/exe" 2>/dev/null || :)
    [ "$fallback_command_exe" = "$fallback_command_expected_exe" ] ||
        { [ -n "$fallback_command_initial_exe" ] && [ "$fallback_command_exe" = "$fallback_command_initial_exe" ]; } || return 1
    same_birth "$fallback_command_pid" "$fallback_command_birth" || return 1
    [ "$proc_parent" = "$$" ] || return 1
    return 0
}

fallback_service_child_state() {
    # 0: verified active; 1: the recorded child is verified exited;
    # 2: the PID still exists but its identity is unknown.
    [ -d "/proc/$fallback_command_pid" ] || return 1
    if ! proc_read "$fallback_command_pid" 2>/dev/null; then
        [ ! -d "/proc/$fallback_command_pid" ] && return 1
        return 2
    fi
    [ "$proc_birth" = "$fallback_command_birth" ] || return 1
    case "$proc_state" in Z|X) return 1;; esac
    return 0
}

fallback_service_call() {
    # Only these two service-control operations are admitted. A timed-out
    # direct child may receive TERM with its full identity; no worker receives
    # a signal and no process group or KILL escalation is used here.
    [ "$#" -eq 3 ] || return 1
    fallback_command_limit=$1
    fallback_command_name=$2
    fallback_command_argument=$3
    case "$fallback_command_limit:$fallback_command_name:$fallback_command_argument" in
        3:status:statusbar|5:start:statusbar) ;;
        *) return 1;;
    esac
    # Native recovery shares the caller's existing cleanup deadline. Do not
    # begin a command that could consume the remaining deadline plus grace.
    now_seconds
    if [ -n "${deadline:-}" ] && [ $((now_value + fallback_command_limit + 1)) -ge "$deadline" ]; then
        fallback_note 'Native helper withheld: the shared cleanup deadline has insufficient time.' || :
        return 1
    fi
    fallback_command_path=$(command -v "$fallback_command_name" 2>/dev/null || :)
    case "$fallback_command_path" in /*) ;; *) return 1;; esac
    fallback_command_expected_exe=$(readlink -f "$fallback_command_path" 2>/dev/null || :)
    [ -n "$fallback_command_expected_exe" ] || return 1
    fallback_service_sequence=$(( ${fallback_service_sequence:-0} + 1 ))
    fallback_service_output=$out/native-fallback-service-$fallback_service_sequence.txt
    [ ! -e "$fallback_service_output" ] && [ ! -L "$fallback_service_output" ] || return 1
    ( set -C; : >"$fallback_service_output" ) || return 1
    ( trap - 0 HUP INT TERM; exec "$fallback_command_path" "$fallback_command_argument" ) >"$fallback_service_output" 2>&1 &
    fallback_command_pid=$!
    fallback_command_birth=''
    fallback_command_initial_exe=''
    if proc_read "$fallback_command_pid" 2>/dev/null; then
        fallback_command_birth=$proc_birth
        if [ "$proc_parent" != "$$" ]; then
            fallback_note "Native helper parent identity is unproven: $fallback_command_pid." || :
            return 1
        fi
        fallback_command_initial_exe=$(readlink "/proc/$fallback_command_pid/exe" 2>/dev/null || :)
        printf '%s %s %s %s %s %s\n' "$fallback_command_pid" "$fallback_command_birth" "$$" "$proc_group" "$proc_session" "$fallback_command_expected_exe" >>"$out/owned-processes.tsv" || return 1
    else
        if [ ! -d "/proc/$fallback_command_pid" ]; then
            if wait "$fallback_command_pid"; then return 0; else return $?; fi
        fi
        fallback_note "Native helper identity is incomplete; leaving PID $fallback_command_pid for review." || :
        return 1
    fi
    now_seconds
    fallback_command_deadline=$((now_value + fallback_command_limit))
    while :; do
        if fallback_service_child_state; then
            :
        else
            fallback_state=$?
            [ "$fallback_state" -eq 1 ] || return 1
            if wait "$fallback_command_pid"; then return 0; else return $?; fi
        fi
        now_seconds
        [ "$now_value" -lt "$fallback_command_deadline" ] || break
        usleep 100000 || return 1
    done
    if fallback_service_child_matches; then
        fallback_note "Native helper timed out; TERM to bound direct child $fallback_command_pid/$fallback_command_birth." || :
        kill -TERM "$fallback_command_pid" 2>/dev/null || :
    else
        fallback_note "Native helper identity changed; no signal sent to PID $fallback_command_pid." || :
    fi
    now_seconds
    fallback_command_deadline=$((now_value + 1))
    while :; do
        if fallback_service_child_state; then
            :
        else
            fallback_state=$?
            [ "$fallback_state" -eq 1 ] || return 1
            wait "$fallback_command_pid" 2>/dev/null || :
            return 1
        fi
        now_seconds
        [ "$now_value" -lt "$fallback_command_deadline" ] || break
        usleep 100000 || return 1
    done
    # An active or unknown helper remains recorded and blocks native release.
    return 1
}

fallback_statusbar_is_running() {
    fallback_service_call 3 status statusbar || return 1
    [ -f "$fallback_service_output" ] && [ ! -L "$fallback_service_output" ] || return 1
    fallback_current_status=$(cat "$fallback_service_output" 2>/dev/null || :)
    case "$fallback_current_status" in 'statusbar start/running'*) return 0;; *) return 1;; esac
}

fallback_allowed_lock_leaf() {
    fallback_leaf_type=regular
    case "$1" in
        power-events|power-source|power-control|log-launcher-pipe|log-application-pipe) fallback_leaf_type=fifo;;
        pid|nonce|generation|application-pid|application-pid.pending|observer-pid|observer-pid.pending|observer-go|observer-go.pending|observer-owned|observer-owned.pending|observer-runner-pid|observer-runner-pid.pending|observer-relay-pid|observer-relay-pid.pending|observer-status|observer-status.pending|application-exit|application-exit.pending|lifecycle-ready|lifecycle-ready.pending|lifecycle-request|lifecycle-request.pending|lifecycle-ack|lifecycle-ack.pending|sleep-cover-request|sleep-cover-request.pending|sleep-cover-ready|sleep-cover-ready.pending|native-output-capability|native-output-capability.pending|native-epdc-mode|native-epdc-mode.pending|resume-native-epdc-mode|resume-native-epdc-mode.pending|native-portrait-orientation|native-portrait-orientation.pending|resume-native-portrait-orientation|resume-native-portrait-orientation.pending|native-read-output|native-read-done|native-read-done.pending|native-read-command|native-read-command.pending|native-read-go|native-read-go.pending) ;;
        log-launcher-ready|log-launcher-ready.pending|log-launcher-owned|log-launcher-owned.pending|log-launcher-quota|log-launcher-archive.pending|log-launcher-empty.pending|log-application-ready|log-application-ready.pending|log-application-owned|log-application-owned.pending|log-application-quota|log-application-archive.pending|log-application-empty.pending|log-stopping|log-launcher-exit|log-launcher-exit.pending|log-application-exit|log-application-exit.pending) ;;
        *) return 1;;
    esac
    return 0
}

fallback_lock_leaves_safe() {
    fallback_lock_matches || return 1
    for fallback_leaf in "$lock"/* "$lock"/.[!.]* "$lock"/..?*; do
        [ -e "$fallback_leaf" ] || [ -L "$fallback_leaf" ] || continue
        [ ! -L "$fallback_leaf" ] || return 1
        fallback_allowed_lock_leaf "${fallback_leaf##*/}" || return 1
        case "$fallback_leaf_type" in
            fifo) [ -p "$fallback_leaf" ] || return 1;;
            regular) [ -f "$fallback_leaf" ] || return 1;;
            *) return 1;;
        esac
        [ "$(stat -c '%h' "$fallback_leaf" 2>/dev/null || :)" = 1 ] || return 1
    done
    return 0
}

fallback_remove_owned_lock() {
    fallback_context_matches && fallback_owner_drained && fallback_lock_leaves_safe || return 1
    # Keep the three ownership anchors until the final checked unlink. Unknown
    # leaves and all linked/directory/device types block even partial cleanup.
    for fallback_remove_leaf in "$lock"/* "$lock"/.[!.]* "$lock"/..?*; do
        [ -e "$fallback_remove_leaf" ] || [ -L "$fallback_remove_leaf" ] || continue
        case "${fallback_remove_leaf##*/}" in pid|nonce|generation) continue;; esac
        fallback_lock_matches || return 1
        [ ! -L "$fallback_remove_leaf" ] || return 1
        fallback_allowed_lock_leaf "${fallback_remove_leaf##*/}" || return 1
        case "$fallback_leaf_type" in
            fifo) [ -p "$fallback_remove_leaf" ] || return 1;;
            regular) [ -f "$fallback_remove_leaf" ] || return 1;;
            *) return 1;;
        esac
        rm -f -- "$fallback_remove_leaf" || return 1
    done
    fallback_context_matches && fallback_owner_drained && fallback_lock_leaves_safe || return 1
    fallback_note "Removing the proven inactive launcher lock $lock_identity." || return 1
    rm -f -- "$lock/pid" "$lock/nonce" "$lock/generation" || return 1
    rmdir "$lock" || return 1
    [ ! -e "$lock" ] && [ ! -L "$lock" ] || return 1
    return 0
}

fallback_restore_native() {
    fallback_context_matches || return 1
    fallback_note 'Native fallback requested; checking complete producer/worker exit.' || return 1
    if ! fallback_owner_drained; then
        fallback_note 'Native fallback withheld: an application, launcher or worker is live or unproven.' || :
        return 1
    fi
    fallback_statusbar_baseline_running || {
        fallback_note 'Native fallback withheld: the original running statusbar baseline is missing.' || :
        return 1
    }
    if [ ! -e "$lock" ] && [ ! -L "$lock" ]; then
        fallback_native_baselines_match running && fallback_statusbar_is_running && fallback_owner_drained || return 1
        fallback_note 'Native state was already restored; no recovery action was needed.' || return 1
        return 0
    fi
    if ! fallback_lock_leaves_safe || ! fallback_native_baselines_match recoverable; then
        fallback_note 'Native fallback withheld: lock contents or native process identities changed.' || :
        return 1
    fi
    while IFS=' ' read -r fallback_resume_pid fallback_resume_birth fallback_resume_exe fallback_resume_extra; do
        fallback_context_matches && fallback_owner_drained && fallback_lock_matches || return 1
        same_birth "$fallback_resume_pid" "$fallback_resume_birth" || return 1
        [ "$(readlink "/proc/$fallback_resume_pid/exe" 2>/dev/null || :)" = "$fallback_resume_exe" ] || return 1
        same_birth "$fallback_resume_pid" "$fallback_resume_birth" || return 1
        case "$proc_state" in
            T|t)
                fallback_note "CONT to the proven native baseline $fallback_resume_pid/$fallback_resume_birth." || return 1
                kill -CONT "$fallback_resume_pid" 2>/dev/null || return 1
                ;;
            S|R) ;;
            *) return 1;;
        esac
    done <"$out/native-before.tsv"
    fallback_context_matches && fallback_owner_drained && fallback_lock_matches || return 1
    fallback_service_call 3 status statusbar || return 1
    fallback_current_status=$(cat "$fallback_service_output" 2>/dev/null || :)
    case "$fallback_current_status" in
        'statusbar stop/waiting'*)
            fallback_context_matches && fallback_owner_drained && fallback_lock_matches || return 1
            fallback_note 'Starting only the previously running native statusbar.' || return 1
            fallback_service_call 5 start statusbar || return 1
            ;;
        'statusbar start/running'*) ;;
        *) return 1;;
    esac
    fallback_native_baselines_match running && fallback_statusbar_is_running || return 1
    fallback_context_matches && fallback_owner_drained && fallback_lock_matches || return 1
    fallback_remove_owned_lock || return 1
    fallback_native_baselines_match running && fallback_statusbar_is_running && fallback_owner_drained || return 1
    fallback_note 'Native fallback completed; baseline processes, service and owned lock were checked.' || return 1
    return 0
}
