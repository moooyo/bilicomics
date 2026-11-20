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
owned_lock_generation=''
observer_group_proven=0
observer_anchor_pid=''
observer_anchor_ticks=''
observer_anchor_exe=''
observer_anchor_parent=''
observer_anchor_generation=''
observer_anchor_nonce=''
cleanup_started=0
recovery_complete=1
media_worker_recovery_blocked=0
media_worker_wait_deadline=''
reason=unexpected-exit
ready_seen=0
statusbar_was_running=0
bounded_sequence=0
command_output=''
elf_receipt_dir=/tmp/bilicomics-physical-$run_id-elf
elf_receipt_path=$elf_receipt_dir/verified-elf.receipt
elf_receipt_pending=$elf_receipt_dir/verified-elf.receipt.pending
elf_receipt_created=0
elf_receipt_directory=''
elf_receipt_file=''
elf_receipt_pending_file=''
elf_receipt_record=''
elf_receipt_verified=0
: >"$logs/owned-processes.tsv"
: >"$logs/native-baseline.tsv"
: >"$logs/observer-group-members.tsv"
: >"$logs/observer-group-proofs.tsv"

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
proc_group_session() { proc_fields "$1" | awk '{ print $3 ":" $4 }'; }
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
elf_file_identity() {
    [ -f "$1" ] && [ ! -L "$1" ] || return 1
    [ "${2:-0}" != 1 ] || [ -x "$1" ] || return 1
    [ "$(readlink -f "$1")" = "$1" ] || return 1
    elf_stat=$(LC_ALL=C TZ=UTC0 stat -c '%d|%i|%u|%f|%h|%s|%y|%z' "$1" 2>/dev/null) || return 1
    IFS='|' read -r elf_dev elf_inode elf_uid elf_mode elf_links elf_size elf_mtime elf_ctime elf_extra <<EOF
$elf_stat
EOF
    [ -z "$elf_extra" ] && [ "$elf_uid" = "$elf_owner" ] && [ "$elf_links" = 1 ] || return 1
    for elf_decimal in "$elf_dev" "$elf_inode" "$elf_uid" "$elf_size"; do
        case "$elf_decimal" in *[!0-9]*|'') return 1 ;; 0|[1-9]*) ;; *) return 1 ;; esac
        [ "${#elf_decimal}" -le 20 ] || return 1
    done
    [ "$elf_dev" -gt 0 ] && [ "$elf_inode" -gt 0 ] && [ "$elf_size" -gt 0 ] || return 1
    case "$elf_mode" in 81[0-9a-f][0-9a-f]) ;; *) return 1 ;; esac
    # These are the finest timestamp fields supplied by stat. Zero filesystem
    # nanoseconds are valid; missing/literal fields are never a cache fallback.
    for elf_time in "$elf_mtime" "$elf_ctime"; do
        case "$elf_time" in ????-??-??\ ??:??:??.?????????*) ;; *) return 1 ;; esac
        case "$elf_time" in *[!0-9\ .:+-]*) return 1 ;; esac
        elf_fraction=${elf_time#*.}
        elf_fraction=${elf_fraction%% *}
        [ "${#elf_fraction}" -eq 9 ] || return 1
        case "$elf_fraction" in *[!0-9]*) return 1 ;; esac
    done
    [ "$elf_stat" = "$elf_dev|$elf_inode|$elf_uid|$elf_mode|$elf_links|$elf_size|$elf_mtime|$elf_ctime" ] || return 1
    printf '%s\n' "$elf_stat"
}
elf_canonical_temporary_base() {
    elf_resolved_temp=$(readlink -f /tmp) || return 1
    # Only the known Kindle /tmp -> /var/tmp base alias is admitted. Receipt
    # directory and file leaves still require their canonical fixed paths.
    case "$elf_resolved_temp" in /tmp|/var/tmp) ;; *) return 1 ;; esac
    [ -d "$elf_resolved_temp" ] && [ ! -L "$elf_resolved_temp" ] || return 1
    printf '%s\n' "$elf_resolved_temp"
}
elf_receipt_directory_identity() {
    [ "$(elf_canonical_temporary_base)" = "$elf_temporary_base" ] || return 1
    [ -d "$elf_receipt_dir" ] && [ ! -L "$elf_receipt_dir" ] || return 1
    [ "$(readlink -f "$elf_receipt_dir")" = "$elf_receipt_dir" ] || return 1
    elf_dir_stat=$(LC_ALL=C stat -c '%d|%i|%u|%f' "$elf_receipt_dir" 2>/dev/null) || return 1
    IFS='|' read -r elf_dir_dev elf_dir_inode elf_dir_uid elf_dir_mode elf_dir_extra <<EOF
$elf_dir_stat
EOF
    [ -z "$elf_dir_extra" ] && [ "$elf_dir_uid" = "$elf_owner" ] || return 1
    case "$elf_dir_mode" in 41c0|45c0) ;; *) return 1 ;; esac
    case "$elf_dir_dev:$elf_dir_inode" in *[!0-9:]*|:*|*:) return 1 ;; esac
    [ "$elf_dir_dev" -gt 0 ] && [ "$elf_dir_inode" -gt 0 ] || return 1
    [ "$elf_dir_stat" = "$elf_dir_dev|$elf_dir_inode|$elf_dir_uid|$elf_dir_mode" ] || return 1
    printf '%s\n' "$elf_dir_stat"
}
elf_private_receipt_identity() {
    elf_private_identity=$(elf_file_identity "$1") || return 1
    IFS='|' read -r elf_private_dev elf_private_inode elf_private_uid elf_private_mode elf_private_links elf_private_size elf_private_mtime elf_private_ctime <<EOF
$elf_private_identity
EOF
    [ "$elf_private_mode" = 8180 ] && [ "$elf_private_size" -le 512 ] || return 1
    printf '%s\n' "$elf_private_identity"
}
elf_receipt_matches() {
    [ -f "$1" ] && [ ! -L "$1" ] || return 1
    elf_receipt_hex=$(dd if="$1" bs=1 count=513 2>/dev/null | od -An -v -tx1 | tr -d ' \n') || return 1
    elf_expected_hex=$(printf '%s\n' "$elf_receipt_record" | od -An -v -tx1 | tr -d ' \n') || return 1
    [ "$elf_receipt_hex" = "$elf_expected_hex" ] && [ "${#elf_receipt_hex}" -le 1024 ]
}
prepare_verified_elf_receipt() {
    reason=elf-verification-unavailable
    elf_owner=$(id -u) || return 1
    for elf_tool in stat readlink sha256sum dd od tr; do command -v "$elf_tool" >/dev/null 2>&1 || return 1; done
    reason=unsupported-elf-temporary-base
    elf_temporary_base=$(elf_canonical_temporary_base) || return 1
    elf_receipt_dir=$elf_temporary_base/bilicomics-physical-$run_id-elf
    elf_receipt_path=$elf_receipt_dir/verified-elf.receipt
    elf_receipt_pending=$elf_receipt_dir/verified-elf.receipt.pending
    [ -f "$logs/binary-name" ] && [ ! -L "$logs/binary-name" ] || return 1
    elf_binary_name=$(read_one "$logs/binary-name")
    case "$elf_binary_name" in bilicomics-*) elf_digest=${elf_binary_name#bilicomics-} ;; *) return 1 ;; esac
    case "$elf_digest" in *[!0-9a-f]*|'') return 1 ;; esac
    [ "${#elf_digest}" -eq 64 ] || return 1
    elf_binary_path=$run_root/app/$elf_binary_name
    reason=existing-elf-receipt-directory
    mkdir -m 700 "$elf_receipt_dir" 2>/dev/null || return 1
    elf_receipt_created=1
    reason=unsupported-elf-receipt-directory
    elf_receipt_directory=$(elf_receipt_directory_identity) || return 1
    reason=unsupported-elf-file-identity
    elf_before=$(elf_file_identity "$elf_binary_path" 1) || return 1
    reason=elf-complete-verification-failed
    # The one complete SHA is outside the production native-reader deadline.
    # Helpers reuse this receipt only with unchanged metadata and their original
    # run/observer/native-reader proof. This is not a hostile same-UID boundary.
    elf_checksum_command=$(command -v sha256sum 2>/dev/null || :)
    case "$elf_checksum_command" in
        /*) bounded_command 10 "$elf_checksum_command" "$elf_binary_path" || return 1 ;;
        sha256sum)
            # BusyBox ash reports an installed applet without an absolute path.
            # Use only the watchdog's kernel-reported, known BusyBox executable;
            # the existing bounded child image/birth/token proof still applies.
            elf_busybox_exe=$(proc_exe "$$" || :)
            case "$elf_busybox_exe" in /bin/busybox|/usr/bin/busybox) ;; *) return 1 ;; esac
            [ -f "$elf_busybox_exe" ] && [ ! -L "$elf_busybox_exe" ] && [ -x "$elf_busybox_exe" ] \
                && [ "$(readlink -f "$elf_busybox_exe")" = "$elf_busybox_exe" ] || return 1
            bounded_command 10 "$elf_busybox_exe" sha256sum "$elf_binary_path" || return 1
            ;;
        *) return 1 ;;
    esac
    [ "$(cat "$command_output" 2>/dev/null || :)" = "$elf_digest  $elf_binary_path" ] || return 1
    reason=elf-changed-during-verification
    [ "$(elf_file_identity "$elf_binary_path" 1)" = "$elf_before" ] \
        && [ "$(read_one "$logs/binary-name")" = "$elf_binary_name" ] \
        && [ "$(read_one "$logs/run-token")" = "$token" ] || return 1
    elf_issuer_birth=$(proc_ticks "$$")
    case "$elf_issuer_birth" in *[!0-9]*|'') return 1 ;; esac
    elf_receipt_record=v1\|$run_id\|$token\|$elf_binary_name\|$$\|$elf_issuer_birth\|$elf_before
    [ "${#elf_receipt_record}" -lt 512 ] || return 1
    reason=elf-receipt-publication-failed
    [ "$(elf_receipt_directory_identity)" = "$elf_receipt_directory" ] || return 1
    [ ! -e "$elf_receipt_path" ] && [ ! -L "$elf_receipt_path" ] \
        && [ ! -e "$elf_receipt_pending" ] && [ ! -L "$elf_receipt_pending" ] || return 1
    (set -C; printf '%s\n' "$elf_receipt_record" >"$elf_receipt_pending") || return 1
    elf_receipt_pending_file=$(elf_private_receipt_identity "$elf_receipt_pending") || return 1
    elf_receipt_matches "$elf_receipt_pending" || return 1
    [ "$(elf_receipt_directory_identity)" = "$elf_receipt_directory" ] \
        && [ "$(elf_private_receipt_identity "$elf_receipt_pending")" = "$elf_receipt_pending_file" ] \
        && [ ! -e "$elf_receipt_path" ] && [ ! -L "$elf_receipt_path" ] || return 1
    mv "$elf_receipt_pending" "$elf_receipt_path" || return 1
    # Rename may update ctime; pin the resulting receipt instead of reusing its
    # pre-rename timestamps. Its inode/owner/mode/size must retain the same core.
    elf_receipt_file=$(elf_private_receipt_identity "$elf_receipt_path") || return 1
    elf_pending_core=${elf_receipt_pending_file%|*}
    elf_pending_core=${elf_pending_core%|*}
    elf_final_core=${elf_receipt_file%|*}
    elf_final_core=${elf_final_core%|*}
    [ "$elf_pending_core" = "$elf_final_core" ] && elf_receipt_matches "$elf_receipt_path" \
        && [ "$(elf_receipt_directory_identity)" = "$elf_receipt_directory" ] \
        && [ "$(elf_private_receipt_identity "$elf_receipt_path")" = "$elf_receipt_file" ] || return 1
    elf_receipt_verified=1
    printf '%s\n' verified >"$logs/verified-elf-status"
}
remove_verified_elf_receipt() {
    [ "$elf_receipt_created" -eq 1 ] || return 0
    if [ -z "$elf_receipt_directory" ] || [ "$(elf_receipt_directory_identity)" != "$elf_receipt_directory" ]; then
        printf '%s\n' 'ELF receipt directory identity changed; preserving it for review.' >>"$logs/recovery.log"
        recovery_complete=0
        return 0
    fi
    for elf_cache_entry in "$elf_receipt_dir/"* "$elf_receipt_dir/".[!.]* "$elf_receipt_dir/"..?*; do
        [ -e "$elf_cache_entry" ] || [ -L "$elf_cache_entry" ] || continue
        case "$elf_cache_entry" in "$elf_receipt_path"|"$elf_receipt_pending") ;; *)
            printf '%s\n' 'ELF receipt directory has unexpected entries; preserving it for review.' >>"$logs/recovery.log"
            recovery_complete=0
            return 0
            ;;
        esac
    done
    if [ ! -e "$elf_receipt_path" ] && [ ! -L "$elf_receipt_path" ] \
        && [ ! -e "$elf_receipt_pending" ] && [ ! -L "$elf_receipt_pending" ]; then
        rmdir "$elf_receipt_dir" 2>/dev/null || recovery_complete=0
        return 0
    fi
    [ "$recovery_complete" -eq 1 ] || {
        printf '%s\n' 'ELF receipt retained until owned reader recovery is complete.' >>"$logs/recovery.log"
        return 0
    }
    for elf_receipt_leaf in "$elf_receipt_path" "$elf_receipt_pending"; do
        [ -e "$elf_receipt_leaf" ] || [ -L "$elf_receipt_leaf" ] || continue
        case "$elf_receipt_leaf" in
            "$elf_receipt_path") elf_owned_file=$elf_receipt_file ;;
            "$elf_receipt_pending") elf_owned_file=$elf_receipt_pending_file ;;
        esac
        if [ -z "$elf_owned_file" ] || [ "$(elf_private_receipt_identity "$elf_receipt_leaf")" != "$elf_owned_file" ] \
            || ! elf_receipt_matches "$elf_receipt_leaf" \
            || [ "$(elf_receipt_directory_identity)" != "$elf_receipt_directory" ]; then
            printf '%s\n' 'ELF receipt identity changed; preserving it for review.' >>"$logs/recovery.log"
            recovery_complete=0
            return 0
        fi
        rm "$elf_receipt_leaf" || { recovery_complete=0; return 0; }
    done
    [ "$(elf_receipt_directory_identity)" = "$elf_receipt_directory" ] \
        && rmdir "$elf_receipt_dir" 2>/dev/null || recovery_complete=0
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
read_candidate_lock() {
    [ -d "$lock" ] && [ ! -L "$lock" ] || return 1
    for candidate_lock_leaf in pid nonce generation; do
        [ -f "$lock/$candidate_lock_leaf" ] && [ ! -L "$lock/$candidate_lock_leaf" ] || return 1
    done
    [ "$(read_one "$lock/pid")" = "$launcher_pid" ] || return 1
    candidate_lock_nonce=$(read_one "$lock/nonce")
    case "$candidate_lock_nonce" in *[!0-9a-f]*|'') return 1 ;; esac
    [ "${#candidate_lock_nonce}" -eq 32 ] || return 1
    candidate_lock_record=$(read_one "$lock/generation")
    candidate_lock_generation=${candidate_lock_record%%:*}
    case "$candidate_lock_generation" in *[!0-9]*|'') return 1 ;; esac
    [ "$candidate_lock_generation" -gt 0 ] || return 1
    [ "$candidate_lock_record" = "$candidate_lock_generation:$candidate_lock_nonce" ] || return 1
    candidate_lock_inode=$(ls -id "$lock" 2>/dev/null | awk '{ print $1 }')
    case "$candidate_lock_inode" in *[!0-9]*|'') return 1 ;; esac
    if [ -n "$owned_lock_nonce" ]; then
        [ "$candidate_lock_nonce:$candidate_lock_generation:$candidate_lock_inode" = "$owned_lock_nonce:$owned_lock_generation:$owned_lock_inode" ] || return 1
    fi
}
remember_candidate_lock() {
    if [ -z "$owned_lock_nonce" ]; then
        owned_lock_nonce=$candidate_lock_nonce
        owned_lock_generation=$candidate_lock_generation
        owned_lock_inode=$candidate_lock_inode
        printf '%s|%s|%s|%s\n' "$launcher_pid" "$launcher_ticks" "$owned_lock_inode" "$owned_lock_nonce" >"$logs/owned-launcher-lock" || recovery_complete=0
    fi
}
observe_owned_lock() {
    [ "$launcher_script_verified" -eq 1 ] && launcher_child_matches || return 0
    read_candidate_lock || return 0
    remember_candidate_lock
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
read_group_identity_snapshot() {
    group_identity_file=$1
    group_identity_expected_exe=$2
    group_identity_expected_parent=$3
    [ -f "$logs/$group_identity_file" ] && [ ! -L "$logs/$group_identity_file" ] || return 1
    group_identity_line=$(read_one "$logs/$group_identity_file")
    IFS='|' read -r group_identity_pid group_identity_ticks group_identity_exe group_identity_parent group_identity_token <<EOF
$group_identity_line
EOF
    [ "$group_identity_line" = "$group_identity_pid|$group_identity_ticks|$group_identity_exe|$group_identity_parent|$token" ] || return 1
    [ "$group_identity_token" = "$token" ] && [ "$group_identity_exe" = "$group_identity_expected_exe" ] && [ "$group_identity_parent" = "$group_identity_expected_parent" ] || return 1
    case "$group_identity_pid:$group_identity_ticks:$group_identity_parent" in *[!0-9:]*|:*|*:) return 1 ;; esac
    [ "$group_identity_pid" -gt 1 ] && [ "$group_identity_parent" -gt 0 ] || return 1
}
candidate_observer_member() {
    for candidate_member_file in power-relay.identity power-source-runner.identity; do
        case "$candidate_member_file" in
            power-relay.identity) candidate_member_exe=$run_root/app/$(read_one "$logs/binary-name") ;;
            *) candidate_member_exe=$launcher_exe ;;
        esac
        read_group_identity_snapshot "$candidate_member_file" "$candidate_member_exe" "$candidate_observer_pid" || continue
        same_process "$group_identity_pid" "$group_identity_ticks" "$group_identity_exe" || continue
        [ "$(proc_parent "$group_identity_pid")" = "$candidate_observer_pid" ] || continue
        [ "$(proc_group_session "$group_identity_pid")" = "$candidate_observer_pid:$candidate_observer_pid" ] || continue
        same_process "$group_identity_pid" "$group_identity_ticks" "$group_identity_exe" && return 0
    done
    # A surviving source can outlive its runner. The private runner snapshot
    # preserves the original two-edge lineage; the source still pins the group.
    read_group_identity_snapshot power-source-runner.identity "$launcher_exe" "$candidate_observer_pid" || return 1
    candidate_source_parent=$group_identity_pid
    candidate_source_parent_ticks=$group_identity_ticks
    read_group_identity_snapshot power-event.identity "$(read_one "$logs/power-event-executable")" "$candidate_source_parent" || return 1
    same_process "$group_identity_pid" "$group_identity_ticks" "$group_identity_exe" || return 1
    candidate_source_current_parent=$(proc_parent "$group_identity_pid")
    if same_process "$candidate_source_parent" "$candidate_source_parent_ticks" "$launcher_exe"; then
        [ "$candidate_source_current_parent" = "$candidate_source_parent" ] || return 1
    else
        [ "$candidate_source_current_parent" = "$candidate_source_parent" ] || [ "$candidate_source_current_parent" = 1 ] || [ "$candidate_source_current_parent" = "$$" ] || return 1
    fi
    [ "$(proc_group_session "$group_identity_pid")" = "$candidate_observer_pid:$candidate_observer_pid" ] || return 1
    same_process "$group_identity_pid" "$group_identity_ticks" "$group_identity_exe"
}
observe_observer_group() {
    [ "$launcher_script_verified" -eq 1 ] || return 0
    read_candidate_lock || return 0
    [ -f "$lock/observer-owned" ] && [ ! -L "$lock/observer-owned" ] || return 0
    candidate_observer_line=$(read_one "$lock/observer-owned")
    IFS=':' read -r candidate_observer_generation candidate_observer_nonce candidate_observer_pid candidate_observer_ticks <<EOF
$candidate_observer_line
EOF
    [ "$candidate_observer_line" = "$candidate_lock_generation:$candidate_lock_nonce:$candidate_observer_pid:$candidate_observer_ticks" ] || return 0
    [ "$candidate_observer_generation:$candidate_observer_nonce" = "$candidate_lock_generation:$candidate_lock_nonce" ] || return 0
    case "$candidate_observer_pid:$candidate_observer_ticks" in *[!0-9:]*|:*|*:) return 0 ;; esac
    [ "$candidate_observer_pid" -gt 1 ] && [ "$candidate_observer_pid" != "$launcher_pid" ] && [ "$candidate_observer_pid" != "$$" ] || return 0
    candidate_observer_exe=$launcher_exe
    candidate_observer_lock_inode=$candidate_lock_inode
    same_process "$candidate_observer_pid" "$candidate_observer_ticks" "$candidate_observer_exe" || return 0
    [ "$(proc_group_session "$candidate_observer_pid")" = "$candidate_observer_pid:$candidate_observer_pid" ] || return 0
    candidate_observer_proof=''
    if launcher_child_matches; then
        [ "$(proc_parent "$candidate_observer_pid")" = "$launcher_pid" ] || return 0
        candidate_observer_proof=live-master
    else
        # A stale PID file alone never authorizes a group signal. Trusted
        # wrapper records must retain both the anchor and a live member chain.
        [ -f "$lock/observer-go" ] && [ ! -L "$lock/observer-go" ] || return 0
        [ "$(read_one "$lock/observer-go")" = "$candidate_observer_generation:$candidate_observer_nonce:$candidate_observer_pid" ] || return 0
        candidate_anchor_recorded=0
        for candidate_anchor_file in observer-anchor.identity power-observer.identity; do
            read_group_identity_snapshot "$candidate_anchor_file" "$candidate_observer_exe" "$launcher_pid" || continue
            [ "$group_identity_pid:$group_identity_ticks" = "$candidate_observer_pid:$candidate_observer_ticks" ] || continue
            candidate_anchor_recorded=1
            break
        done
        [ "$candidate_anchor_recorded" -eq 1 ] && candidate_observer_member || return 0
        candidate_observer_proof=private-lineage-and-live-member
    fi
    same_process "$candidate_observer_pid" "$candidate_observer_ticks" "$candidate_observer_exe" || return 0
    [ "$(proc_group_session "$candidate_observer_pid")" = "$candidate_observer_pid:$candidate_observer_pid" ] || return 0
    read_candidate_lock || return 0
    [ "$candidate_lock_generation:$candidate_lock_nonce:$candidate_lock_inode" = "$candidate_observer_generation:$candidate_observer_nonce:$candidate_observer_lock_inode" ] || return 0
    [ "$(read_one "$lock/observer-owned")" = "$candidate_observer_line" ] || return 0
    if [ "$observer_group_proven" -eq 1 ]; then
        if [ "$observer_anchor_pid:$observer_anchor_ticks:$observer_anchor_generation:$observer_anchor_nonce" != "$candidate_observer_pid:$candidate_observer_ticks:$candidate_observer_generation:$candidate_observer_nonce" ]; then
            recovery_complete=0
            printf '%s\n' 'A different observer group requires operator review.' >>"$logs/recovery.log"
        fi
        return 0
    fi
    remember_candidate_lock
    observer_anchor_pid=$candidate_observer_pid
    observer_anchor_ticks=$candidate_observer_ticks
    observer_anchor_exe=$candidate_observer_exe
    observer_anchor_parent=$launcher_pid
    observer_anchor_generation=$candidate_observer_generation
    observer_anchor_nonce=$candidate_observer_nonce
    observer_group_proven=1
    printf '%s|%s|%s|%s|%s|%s|%s|%s|%s\n' "$observer_anchor_pid" "$observer_anchor_ticks" "$observer_anchor_exe" "$launcher_pid" "$observer_anchor_pid" "$observer_anchor_pid" "$observer_anchor_generation" "$owned_lock_inode" "$candidate_observer_proof" >>"$logs/observer-group-proofs.tsv" || recovery_complete=0
}
observer_anchor_matches() {
    [ "$observer_group_proven" -eq 1 ] || return 1
    same_process "$observer_anchor_pid" "$observer_anchor_ticks" "$observer_anchor_exe" || return 1
    [ "$(proc_group_session "$observer_anchor_pid")" = "$observer_anchor_pid:$observer_anchor_pid" ] || return 1
    if launcher_child_matches; then
        [ "$(proc_parent "$observer_anchor_pid")" = "$observer_anchor_parent" ] || return 1
    fi
    same_process "$observer_anchor_pid" "$observer_anchor_ticks" "$observer_anchor_exe"
}
observer_group_target_matches() {
    [ "$observer_group_proven" -eq 1 ] || return 1
    if observer_anchor_matches; then return 0; fi
    while IFS='|' read -r member_pid member_ticks member_exe member_parent member_role member_group member_session; do
        [ "$member_group:$member_session" = "$observer_anchor_pid:$observer_anchor_pid" ] || continue
        same_process "$member_pid" "$member_ticks" "$member_exe" || continue
        [ "$(proc_group_session "$member_pid")" = "$observer_anchor_pid:$observer_anchor_pid" ] || continue
        same_process "$member_pid" "$member_ticks" "$member_exe" && return 0
    done <"$logs/observer-group-members.tsv"
    return 1
}
record_observer_group_members() {
    observer_group_target_matches || return 0
    for observer_member_dir in /proc/[0-9]*; do
        observer_member_pid=${observer_member_dir#/proc/}
        [ "$(proc_group_session "$observer_member_pid")" = "$observer_anchor_pid:$observer_anchor_pid" ] || continue
        observer_member_ticks=$(proc_ticks "$observer_member_pid")
        observer_member_exe=$(proc_exe "$observer_member_pid" || :)
        observer_member_parent=$(proc_parent "$observer_member_pid")
        case "$observer_member_pid:$observer_member_ticks:$observer_member_parent" in *[!0-9:]*|:*|*:) continue ;; esac
        case "$observer_member_exe" in /*) ;; *) continue ;; esac
        case "$observer_member_exe" in *'|'*|*'(deleted)'*) continue ;; esac
        same_process "$observer_member_pid" "$observer_member_ticks" "$observer_member_exe" || continue
        [ "$(proc_group_session "$observer_member_pid")" = "$observer_anchor_pid:$observer_anchor_pid" ] || continue
        if ! observer_group_target_matches; then
            # A new candidate cannot establish its own group provenance after
            # the previously captured anchor and member pins have disappeared.
            recovery_complete=0
            printf '%s\n' 'The observer group pin changed during member enumeration.' >>"$logs/recovery.log"
            continue
        fi
        same_process "$observer_member_pid" "$observer_member_ticks" "$observer_member_exe" || continue
        [ "$(proc_group_session "$observer_member_pid")" = "$observer_anchor_pid:$observer_anchor_pid" ] || continue
        observer_member_line=$observer_member_pid'|'$observer_member_ticks'|'$observer_member_exe'|'$observer_member_parent'|observer-group'
        if ! grep -F -x "$observer_member_line" "$logs/owned-processes.tsv" >/dev/null 2>&1; then
            printf '%s\n' "$observer_member_line" >>"$logs/owned-processes.tsv" || recovery_complete=0
        fi
        if observer_sleep_member "$observer_member_pid" "$observer_member_ticks" "$observer_member_exe" "$observer_member_parent"; then
            observer_sleep_line=$observer_member_pid'|'$observer_member_ticks'|'$observer_member_exe'|'$observer_member_parent'|observer-sleep'
            if ! grep -F -x "$observer_sleep_line" "$logs/owned-processes.tsv" >/dev/null 2>&1; then
                printf '%s\n' "$observer_sleep_line" >>"$logs/owned-processes.tsv" || recovery_complete=0
            fi
        fi
        observer_member_line=$observer_member_line'|'$observer_anchor_pid'|'$observer_anchor_pid
        if ! grep -F -x "$observer_member_line" "$logs/observer-group-members.tsv" >/dev/null 2>&1; then
            printf '%s\n' "$observer_member_line" >>"$logs/observer-group-members.tsv" || recovery_complete=0
        fi
    done
}
observer_sleep_member() {
    sleep_member_pid=$1
    sleep_member_ticks=$2
    sleep_member_exe=$3
    sleep_member_parent=$4
    sleep_parent_ticks=$(proc_ticks "$sleep_member_parent")
    sleep_parent_exe=$(proc_exe "$sleep_member_parent" || :)
    recorded_observer_anchor "$sleep_member_parent" "$sleep_parent_ticks" "$sleep_parent_exe" \
        && same_process "$sleep_member_parent" "$sleep_parent_ticks" "$sleep_parent_exe" || return 1
    [ "$(proc_parent "$sleep_member_pid")" = "$sleep_member_parent" ] \
        && [ "$(proc_group_session "$sleep_member_pid")" = "$observer_anchor_pid:$observer_anchor_pid" ] || return 1
    sleep_command=$(command -v sleep 2>/dev/null || :)
    case "$sleep_command" in
        /*) [ "$(readlink -f "$sleep_command" 2>/dev/null || :)" = "$sleep_member_exe" ] || return 1 ;;
        sleep) case "$sleep_member_exe" in /bin/busybox|/usr/bin/busybox) ;; *) return 1 ;; esac ;;
        *) return 1 ;;
    esac
    sleep_arguments=$(tr '\000' '\n' <"/proc/$sleep_member_pid/cmdline" 2>/dev/null) || return 1
    [ "$sleep_arguments" = "$(printf 'sleep\n1')" ] \
        && [ "$(wc -c <"/proc/$sleep_member_pid/cmdline" 2>/dev/null)" -eq 8 ] \
        && same_process "$sleep_member_pid" "$sleep_member_ticks" "$sleep_member_exe"
}
observer_group_has_media_worker() {
    for protected_member_dir in /proc/[0-9]*; do
        protected_member_pid=${protected_member_dir#/proc/}
        [ "$(proc_group_session "$protected_member_pid")" = "$observer_anchor_pid:$observer_anchor_pid" ] || continue
        protected_member_ticks=$(proc_ticks "$protected_member_pid")
        protected_member_state=$(proc_state "$protected_member_pid")
        case "$protected_member_state" in Z|X|'') continue ;; esac
        protected_member_exe=$(proc_exe "$protected_member_pid" || :)
        protected_member_parent=$(proc_parent "$protected_member_pid")
        if [ "$protected_member_exe" != "$run_root/app/kindle-jpeg-worker" ] \
            && ! recorded_ui_child "$protected_member_pid" "$protected_member_ticks" \
            && same_process "$protected_member_pid" "$protected_member_ticks" "$protected_member_exe" \
            && recorded_observer_member "$protected_member_pid" "$protected_member_ticks" "$protected_member_exe"; then
            continue
        fi
        # A group signal cannot exclude a DMA owner or an unbound member
        # that could still exec into one. Membership alone grants no signal.
        media_worker_recovery_blocked=1
        recovery_complete=0
        printf 'Observer group signal withheld for an exact media worker or unbound member: %s|%s|%s|%s\n' \
            "$protected_member_pid" "$protected_member_ticks" "$protected_member_exe" \
            "$protected_member_parent" >>"$logs/recovery.log"
        return 0
    done
    return 1
}
stop_owned_observer_group() {
    # Preserve the anchor until this point. It pins the private PGID/SID even
    # when the production master has already died or its lock was removed.
    observer_group_target_matches || return 0
    record_observer_group_members
    observer_group_target_matches || { recovery_complete=0; return 0; }
    observer_group_has_media_worker && return 0
    if kill -KILL "-$observer_anchor_pid" 2>/dev/null; then
        printf 'A proven observer group received KILL: %s\n' "$observer_anchor_pid" >>"$logs/recovery.log"
    elif observer_group_target_matches; then
        recovery_complete=0
        printf 'A proven observer group could not be signalled: %s\n' "$observer_anchor_pid" >>"$logs/recovery.log"
    fi
}
discover_children() {
    observe_owned_lock
    observe_observer_group
    # These trusted wrappers prove the original lineage before exec, allowing
    # recovery after reparenting or after normal launcher-lock removal.
    record_persistent_identity runner.identity runner "$launcher_exe"
    record_persistent_identity native.identity application "$run_root/app/$(read_one "$logs/binary-name")"
    record_persistent_identity observer-anchor.identity observer-anchor "$launcher_exe"
    record_persistent_identity power-observer.identity observer "$launcher_exe"
    record_persistent_identity power-source-runner.identity source-runner "$launcher_exe"
    record_persistent_identity power-relay.identity relay "$run_root/app/$(read_one "$logs/binary-name")"
    record_persistent_identity power-event.identity power-event "$(read_one "$logs/power-event-executable")"
    fallback_scan=0
    while IFS='|' read -r parent_pid parent_ticks parent_exe parent_parent parent_role; do
        same_process "$parent_pid" "$parent_ticks" "$parent_exe" || continue
        parent_tasks_seen=0
        for children_file in /proc/$parent_pid/task/[0-9]*/children; do
            if [ -r "$children_file" ]; then
                parent_tasks_seen=1
                child_numbers=''
                IFS= read -r child_numbers <"$children_file" || :
                for discover_pid in $child_numbers; do record_process "$discover_pid" "$parent_pid" descendant; done
            else
                fallback_scan=1
            fi
        done
        [ "$parent_tasks_seen" -ne 0 ] || fallback_scan=1
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
    if [ "$3" = "$run_root/app/kindle-jpeg-worker" ]; then
        printf 'Exact media worker reserved for natural exit, not %s: %s|%s|%s\n' \
            "$4" "$1" "$2" "$3" >>"$logs/recovery.log"
        return 0
    fi
    recorded_signal_role "$1" "$2" "$3" || return 0
    kill "-$4" "$1" 2>/dev/null || :
}
recorded_observer_anchor() {
    anchor_guard_pid=$1
    anchor_guard_ticks=$2
    anchor_guard_exe=$3
    while IFS='|' read -r anchor_known_pid anchor_known_ticks anchor_known_exe anchor_known_parent anchor_known_role; do
        case "$anchor_known_role" in observer|observer-anchor) ;; *) continue ;; esac
        [ "$anchor_guard_pid:$anchor_guard_ticks:$anchor_guard_exe" = "$anchor_known_pid:$anchor_known_ticks:$anchor_known_exe" ] && return 0
    done <"$logs/owned-processes.tsv"
    return 1
}
signal_descendants() {
    requested_signal=$1
    while IFS='|' read -r child_pid child_ticks child_exe child_parent child_role; do
        [ "$child_pid" != "$launcher_pid" ] || continue
        if recorded_observer_anchor "$child_pid" "$child_ticks" "$child_exe"; then
            if [ "$observer_group_proven" -ne 1 ] && same_process "$child_pid" "$child_ticks" "$child_exe"; then
                recovery_complete=0
                printf 'An observer anchor lacks complete group proof; preserving it for review: %s\n' "$child_pid" >>"$logs/recovery.log"
            fi
            continue
        fi
        case "$child_role" in runner|application|relay|power-event|source-runner) ;; *) continue ;; esac
        if [ "$observer_group_proven" -eq 1 ] && same_process "$child_pid" "$child_ticks" "$child_exe" \
            && [ "$(proc_group_session "$child_pid")" = "$observer_anchor_pid:$observer_anchor_pid" ]; then
            # A group signal must retain its live anchor or member pin. Killing
            # that pin first could strand an unobserved FIFO writer or relay.
            continue
        fi
        signal_record "$child_pid" "$child_ticks" "$child_exe" "$requested_signal"
    done <"$logs/owned-processes.tsv"
}
recorded_signal_role() {
    signal_role_pid=$1
    signal_role_ticks=$2
    signal_role_exe=$3
    while IFS='|' read -r signal_known_pid signal_known_ticks signal_known_exe signal_known_parent signal_known_role; do
        [ "$signal_role_pid:$signal_role_ticks:$signal_role_exe" = "$signal_known_pid:$signal_known_ticks:$signal_known_exe" ] || continue
        case "$signal_known_role" in runner|application|relay|power-event|source-runner) return 0 ;; esac
    done <"$logs/owned-processes.tsv"
    return 1
}
recorded_observer_member() {
    observer_role_pid=$1
    observer_role_ticks=$2
    observer_role_exe=$3
    while IFS='|' read -r observer_known_pid observer_known_ticks observer_known_exe observer_known_parent observer_known_role; do
        [ "$observer_role_pid:$observer_role_ticks:$observer_role_exe" = "$observer_known_pid:$observer_known_ticks:$observer_known_exe" ] || continue
        case "$observer_known_role" in observer|observer-anchor|relay|power-event|source-runner|observer-sleep) return 0 ;; esac
    done <"$logs/owned-processes.tsv"
    return 1
}
recorded_application_parent() {
    application_parent_pid=$1
    while IFS='|' read -r application_known_pid application_known_ticks application_known_exe application_known_parent application_known_role; do
        [ "$application_known_role" = application ] && [ "$application_known_pid" = "$application_parent_pid" ] || continue
        [ "$application_known_exe" = "$run_root/app/$(read_one "$logs/binary-name")" ] && return 0
    done <"$logs/owned-processes.tsv"
    return 1
}
recorded_ui_child() {
    ui_child_pid=$1
    ui_child_ticks=$2
    while IFS='|' read -r ui_known_pid ui_known_ticks ui_known_exe ui_known_parent ui_known_role; do
        [ "$ui_known_role" = descendant ] && [ "$ui_child_pid:$ui_child_ticks" = "$ui_known_pid:$ui_known_ticks" ] || continue
        recorded_application_parent "$ui_known_parent" && return 0
    done <"$logs/owned-processes.tsv"
    return 1
}
recorded_media_worker() {
    media_record_pid=$1
    media_record_ticks=$2
    while IFS='|' read -r media_known_pid media_known_ticks media_known_exe media_known_parent media_known_role; do
        [ "$media_record_pid:$media_record_ticks" = "$media_known_pid:$media_known_ticks" ] || continue
        [ "$media_known_exe" = "$run_root/app/kindle-jpeg-worker" ] || continue
        case "$media_known_parent" in *[!0-9]*|''|0) continue ;; esac
        case "$media_known_role" in descendant|observer-group) return 0 ;; esac
    done <"$logs/owned-processes.tsv"
    return 1
}
process_birth_exited() {
    exited_pid=$1
    exited_ticks=$2
    case "$exited_pid:$exited_ticks" in *[!0-9:]*|:*|*:) return 1 ;; esac
    [ "$exited_pid" -gt 1 ] || return 1
    [ -d "/proc/$exited_pid" ] || return 0
    exited_current_ticks=$(proc_ticks "$exited_pid")
    [ -n "$exited_current_ticks" ] || return 1
    [ "$exited_current_ticks" = "$exited_ticks" ] || return 0
    case "$(proc_state "$exited_pid")" in Z|X) return 0 ;; esac
    return 1
}
media_producers_gone() {
    producers_gone=1
    if [ -n "$launcher_pid" ] && ! process_birth_exited "$launcher_pid" "$launcher_ticks"; then
        producers_gone=0
    fi
    while IFS='|' read -r producer_pid producer_ticks producer_exe producer_parent producer_role; do
        case "$producer_role" in application|runner) ;; *) continue ;; esac
        if ! process_birth_exited "$producer_pid" "$producer_ticks"; then
            producers_gone=0
            printf 'Waiting for recorded media producer exit: %s|%s|%s|%s\n' \
                "$producer_pid" "$producer_ticks" "$producer_exe" "$producer_role" >>"$logs/recovery.log"
        fi
    done <"$logs/owned-processes.tsv"
    if [ -e "$logs/native.identity" ] || [ -L "$logs/native.identity" ]; then
        [ -f "$logs/native.identity" ] && [ ! -L "$logs/native.identity" ] || return 1
        producer_identity=$(read_one "$logs/native.identity")
        IFS='|' read -r native_producer_pid native_producer_ticks native_producer_exe native_producer_parent native_producer_token <<EOF
$producer_identity
EOF
        case "$native_producer_pid:$native_producer_ticks:$native_producer_parent" in *[!0-9:]*|:*|*:) return 1 ;; esac
        [ "$producer_identity" = "$native_producer_pid|$native_producer_ticks|$native_producer_exe|$native_producer_parent|$token" ] \
            && [ "$native_producer_exe" = "$run_root/app/$(read_one "$logs/binary-name")" ] || return 1
        process_birth_exited "$native_producer_pid" "$native_producer_ticks" || producers_gone=0
    fi
    [ "$producers_gone" -eq 1 ]
}
wait_for_media_workers() {
    # Share one five-second drain budget across cleanup checks. The host stop
    # deadline must still cover the existing launcher and native recovery.
    if [ -z "$media_worker_wait_deadline" ]; then
        media_worker_wait_deadline=$(( $(now_seconds) + 5 ))
    fi
    while :; do
        discover_children || { recovery_complete=0; media_worker_recovery_blocked=1; return 1; }
        media_worker_pending=0
        media_producers_gone || media_worker_pending=1
        # A forked UI child can still have the application's old executable
        # before pre_exec/exec. Pin PID/birth and ancestry until natural exit;
        # an executable-name check alone cannot close that signal race.
        while IFS='|' read -r media_ui_pid media_ui_ticks media_ui_exe media_ui_parent media_ui_role; do
            if [ "$media_ui_exe" != "$run_root/app/kindle-jpeg-worker" ]; then
                [ "$media_ui_role" = descendant ] && recorded_ui_child "$media_ui_pid" "$media_ui_ticks" || continue
            fi
            [ "$(proc_ticks "$media_ui_pid")" = "$media_ui_ticks" ] || continue
            case "$(proc_state "$media_ui_pid")" in Z|X|'') continue ;; esac
            media_ui_current_exe=$(proc_exe "$media_ui_pid" || :)
            if ! has_token "$media_ui_pid" || [ -z "$media_ui_current_exe" ] \
                || { [ "$media_ui_exe" = "$run_root/app/kindle-jpeg-worker" ] && [ "$media_ui_current_exe" != "$media_ui_exe" ]; }; then
                [ "$(proc_ticks "$media_ui_pid")" = "$media_ui_ticks" ] || continue
                case "$(proc_state "$media_ui_pid")" in Z|X|'') continue ;; esac
                recovery_complete=0
                media_worker_recovery_blocked=1
                printf 'Recorded media owner identity is unproven; recovery withheld: %s|%s|%s|%s\n' \
                    "$media_ui_pid" "$media_ui_ticks" "$media_ui_current_exe" "$media_ui_parent" >>"$logs/recovery.log"
                return 1
            fi
            if [ "$media_ui_current_exe" = "$run_root/app/kindle-jpeg-worker" ] \
                && ! recorded_media_worker "$media_ui_pid" "$media_ui_ticks" \
                && same_process "$media_ui_pid" "$media_ui_ticks" "$media_ui_current_exe"; then
                printf '%s|%s|%s|%s|descendant\n' "$media_ui_pid" "$media_ui_ticks" \
                    "$media_ui_current_exe" "$media_ui_parent" >>"$logs/owned-processes.tsv" || {
                    recovery_complete=0; media_worker_recovery_blocked=1; return 1;
                }
            fi
            media_worker_pending=1
            printf 'Waiting for potential UI media owner natural exit: %s|%s|%s|%s|token-matched\n' \
                "$media_ui_pid" "$media_ui_ticks" "$media_ui_current_exe" "$media_ui_parent" >>"$logs/recovery.log"
        done <"$logs/owned-processes.tsv"
        for media_candidate_dir in /proc/[0-9]*; do
            media_candidate_pid=${media_candidate_dir#/proc/}
            [ "$(proc_exe "$media_candidate_pid" || :)" = "$run_root/app/kindle-jpeg-worker" ] || continue
            media_candidate_ticks=$(proc_ticks "$media_candidate_pid")
            media_candidate_state=$(proc_state "$media_candidate_pid")
            case "$media_candidate_state" in Z|X|'') continue ;; esac
            media_candidate_token=unproven
            # An unrecorded exact-path candidate with a missing token also
            # blocks shutdown; discovery never grants it signal authority.
            if has_token "$media_candidate_pid"; then
                media_candidate_token=matched
            fi
            if ! same_process "$media_candidate_pid" "$media_candidate_ticks" "$run_root/app/kindle-jpeg-worker" \
                || ! recorded_media_worker "$media_candidate_pid" "$media_candidate_ticks"; then
                # The child may have exited between the read-only samples.
                [ "$(proc_ticks "$media_candidate_pid")" = "$media_candidate_ticks" ] || continue
                case "$(proc_state "$media_candidate_pid")" in Z|X|'') continue ;; esac
                recovery_complete=0
                media_worker_recovery_blocked=1
                printf 'Exact media worker ownership is unproven; recovery withheld: %s|%s|%s|%s|token-%s\n' \
                    "$media_candidate_pid" "$media_candidate_ticks" "$run_root/app/kindle-jpeg-worker" \
                    "$(proc_parent "$media_candidate_pid")" "$media_candidate_token" >>"$logs/recovery.log"
                return 1
            fi
            media_worker_pending=1
            printf 'Waiting for exact media worker natural exit: %s|%s|%s|%s|%s|token-matched\n' \
                "$media_candidate_pid" "$media_candidate_ticks" "$media_known_exe" "$media_known_parent" "$media_known_role" >>"$logs/recovery.log"
        done
        [ "$media_worker_pending" -ne 0 ] || return 0
        if [ "$(now_seconds)" -ge "$media_worker_wait_deadline" ]; then
            recovery_complete=0
            media_worker_recovery_blocked=1
            printf '%s\n' 'Media producer or owner natural-exit deadline expired; storage and watchdog control cleanup are withheld.' >>"$logs/recovery.log"
            return 1
        fi
        sleep 1
    done
}
restore_native() {
    restoration_allowed=0
    if [ "$launcher_script_verified" -eq 1 ]; then
        if [ ! -e "$lock" ] && [ ! -L "$lock" ]; then
            restoration_allowed=1
        elif [ -n "$owned_lock_nonce" ] && [ -d "$lock" ] && [ ! -L "$lock" ] && [ "$(read_one "$lock/pid")" = "$launcher_pid" ] && [ "$(read_one "$lock/nonce")" = "$owned_lock_nonce" ] && [ "$(read_one "$lock/generation")" = "$owned_lock_generation:$owned_lock_nonce" ] && [ "$(ls -id "$lock" 2>/dev/null | awk '{print $1}')" = "$owned_lock_inode" ]; then
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
    if [ ! -d "$lock" ] || [ -L "$lock" ] || [ -z "$owned_lock_nonce" ] || [ "$(read_one "$lock/pid")" != "$launcher_pid" ] || [ "$(read_one "$lock/nonce")" != "$owned_lock_nonce" ] || [ "$(read_one "$lock/generation")" != "$owned_lock_generation:$owned_lock_nonce" ] || [ "$(ls -id "$lock" 2>/dev/null | awk '{ print $1 }')" != "$owned_lock_inode" ]; then
        printf '%s\n' 'Launcher lock identity is not proven; leaving it untouched.' >>"$logs/recovery.log"
        recovery_complete=0
        return 0
    fi
    # A worker killed during startup can leave one empty timer receipt. Only
    # accept the exact observer previously bound to this generation and lock.
    settle_item=''
    settle_prefix=''
    if [ "${observer_group_proven:-0}" = 1 ] && [ -n "${observer_anchor_pid:-}" ] &&
        [ "${observer_anchor_generation:-}:${observer_anchor_nonce:-}" = "$owned_lock_generation:$owned_lock_nonce" ]; then
        settle_owner=$(stat -c %u "$lock" 2>/dev/null) || settle_owner=''
        case "$settle_owner" in ''|*[!0-9]*) ;; *) settle_prefix=$lock/observer-settled.$owned_lock_generation.$owned_lock_nonce.$observer_anchor_pid ;; esac
    fi
    for lock_item in "$lock"/* "$lock"/.[!.]* "$lock"/..?*; do
        [ -e "$lock_item" ] || [ -L "$lock_item" ] || continue
        case "${lock_item##*/}" in
            observer-settled.*)
                if [ -n "$settle_prefix" ] && [ -z "$settle_item" ] &&
                    { [ "$lock_item" = "$settle_prefix.ok" ] || [ "$lock_item" = "$settle_prefix.failed" ]; } &&
                    [ -d "$lock_item" ] && [ ! -L "$lock_item" ]; then
                    case "$(stat -c %u:%a "$lock_item" 2>/dev/null)" in
                        "$settle_owner:700"|"$settle_owner:2700") settle_item=$lock_item; continue ;;
                    esac
                fi
                printf '%s\n' 'An unproven startup timer receipt requires operator review.' >>"$logs/recovery.log"
                recovery_complete=0
                return 0 ;;
            pid|power-events|power-source|power-control|nonce|generation|application-pid|application-pid.pending|observer-pid|observer-pid.pending|observer-go|observer-go.pending|observer-owned|observer-owned.pending|observer-runner-pid|observer-runner-pid.pending|observer-relay-pid|observer-relay-pid.pending|observer-status|observer-status.pending|application-exit|application-exit.pending|lifecycle-ready|lifecycle-ready.pending|lifecycle-request|lifecycle-request.pending|lifecycle-ack|lifecycle-ack.pending|sleep-cover-request|sleep-cover-request.pending|sleep-cover-ready|sleep-cover-ready.pending|native-output-capability|native-output-capability.pending|native-epdc-mode|native-epdc-mode.pending|resume-native-epdc-mode|resume-native-epdc-mode.pending|native-portrait-orientation|native-portrait-orientation.pending|resume-native-portrait-orientation|resume-native-portrait-orientation.pending|native-read-output|native-read-done|native-read-done.pending|native-read-command|native-read-command.pending|native-read-go|native-read-go.pending) ;;
            log-launcher-pipe|log-launcher-ready|log-launcher-ready.pending|log-launcher-owned|log-launcher-owned.pending|log-launcher-quota|log-launcher-archive.pending|log-launcher-empty.pending|log-application-pipe|log-application-ready|log-application-ready.pending|log-application-owned|log-application-owned.pending|log-application-quota|log-application-archive.pending|log-application-empty.pending) ;;
            log-stopping|log-launcher-exit|log-launcher-exit.pending|log-application-exit|log-application-exit.pending) ;;
            *) printf '%s\n' 'An unexpected lock entry requires operator review.' >>"$logs/recovery.log"; recovery_complete=0; return 0 ;;
        esac
        if [ -L "$lock_item" ]; then
            printf '%s\n' 'An unexpected lock entry type requires operator review.' >>"$logs/recovery.log"
            recovery_complete=0
            return 0
        fi
        case "${lock_item##*/}" in
            power-events|power-source|power-control|log-launcher-pipe|log-application-pipe) lock_item_type_matches=0; [ ! -p "$lock_item" ] || lock_item_type_matches=1 ;;
            *) lock_item_type_matches=0; [ ! -f "$lock_item" ] || lock_item_type_matches=1 ;;
        esac
        if [ "$lock_item_type_matches" -ne 1 ]; then
            printf '%s\n' 'An unexpected lock entry type requires operator review.' >>"$logs/recovery.log"
            recovery_complete=0
            return 0
        fi
    done
    if [ -n "$settle_item" ] && ! rmdir "$settle_item" 2>/dev/null; then
        printf '%s\n' 'The startup timer receipt is not an owned empty directory.' >>"$logs/recovery.log"
        recovery_complete=0
        return 0
    fi
    for lock_name in pid power-events power-source power-control nonce generation application-pid application-pid.pending observer-pid observer-pid.pending observer-go observer-go.pending observer-owned observer-owned.pending observer-runner-pid observer-runner-pid.pending observer-relay-pid observer-relay-pid.pending observer-status observer-status.pending application-exit application-exit.pending lifecycle-ready lifecycle-ready.pending lifecycle-request lifecycle-request.pending lifecycle-ack lifecycle-ack.pending sleep-cover-request sleep-cover-request.pending sleep-cover-ready sleep-cover-ready.pending native-output-capability native-output-capability.pending native-epdc-mode native-epdc-mode.pending resume-native-epdc-mode resume-native-epdc-mode.pending native-portrait-orientation native-portrait-orientation.pending resume-native-portrait-orientation resume-native-portrait-orientation.pending native-read-output native-read-done native-read-done.pending native-read-command native-read-command.pending native-read-go native-read-go.pending; do
        rm -f "$lock/$lock_name"
    done
    for log_name in launcher application; do
        for log_leaf in pipe ready ready.pending owned owned.pending quota archive.pending empty.pending exit exit.pending; do
            rm -f "$lock/log-$log_name-$log_leaf"
        done
    done
    rm -f "$lock/log-stopping"
    rmdir "$lock" 2>/dev/null || recovery_complete=0
}
publish_cleanup_result() {
    if [ -e "$lock" ] || [ -L "$lock" ]; then printf '%s\n' present >"$logs/launcher-lock-after.txt"; else printf '%s\n' absent >"$logs/launcher-lock-after.txt"; fi
    printf '%s|%s|%s|%s\n' "$token" "$reason" "$recovery_complete" "$launcher_status" >"$logs/watchdog.done.pending"
    mv "$logs/watchdog.done.pending" "$logs/watchdog.done"
    [ "$recovery_complete" -eq 1 ]
}
cleanup() {
    [ "$cleanup_started" -eq 0 ] || return 0
    cleanup_started=1
    # Continue after evidence I/O errors; codec ownership still gates native
    # restoration and control-file removal.
    set +e
    trap '' HUP INT TERM
    trap - 0
    discover_children || recovery_complete=0
    record_observer_group_members || recovery_complete=0
    if launcher_child_matches; then kill -TERM "$launcher_pid" 2>/dev/null || :; fi
    remaining=12
    while [ "$remaining" -gt 0 ] && launcher_child_matches; do
        sleep 1
        discover_children
        remaining=$((remaining - 1))
    done
    discover_children || recovery_complete=0
    stop_owned_observer_group || recovery_complete=0
    # Every signal rechecks PID, birth time, token and executable, using only
    # descendants recorded while their ancestry was still proven.
    signal_descendants TERM
    sleep 2
    signal_descendants KILL
    # Capture any last wrapper publication before the master is hard-killed.
    # The observer anchor is signalled as a group, never as the first leaf.
    discover_children || recovery_complete=0
    stop_owned_observer_group || recovery_complete=0
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
    if [ "$media_worker_recovery_blocked" -eq 0 ] && wait_for_media_workers; then
        media_worker_drained=1
    else
        media_worker_drained=0
        reason=media-worker-recovery-blocked
        recovery_complete=0
        printf '%s\n' 'Watchdog final cleanup is withheld; launcher may already have restored native display. Stage, worker storage and ownership evidence remain retained.' >>"$logs/recovery.log"
    fi
    while IFS='|' read -r child_pid child_ticks child_exe child_parent child_role; do
        if same_process "$child_pid" "$child_ticks" "$child_exe"; then
            recovery_complete=0
            printf 'An owned process remains: %s\n' "$child_pid" >>"$logs/recovery.log"
        fi
    done <"$logs/owned-processes.tsv"
    # Unknown live identities carrying this private run tag are reported
    # without signalling them, including reparenting to a host subreaper.
    for orphan_dir in /proc/[0-9]*; do
        orphan_pid=${orphan_dir#/proc/}
        [ "$orphan_pid" != "$$" ] && has_token "$orphan_pid" || continue
        orphan_state=$(proc_state "$orphan_pid")
        case "$orphan_state" in Z|X|'') continue ;; esac
        recovery_complete=0
        printf 'An unproven owned-token process requires operator review: %s|%s|%s|%s\n' "$orphan_pid" "$(proc_ticks "$orphan_pid")" "$(proc_exe "$orphan_pid" || :)" "$(proc_parent "$orphan_pid")" >>"$logs/recovery.log"
    done
    if [ "$media_worker_drained" -eq 1 ]; then
        restore_native
        if [ "$recovery_complete" -eq 1 ]; then remove_owned_lock; fi
        remove_verified_elf_receipt
    fi
    publish_cleanup_result
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
if ! prepare_verified_elf_receipt; then
    printf '%s\n' "$reason" >"$logs/verified-elf-status"
    exit 3
fi
reason=unexpected-exit
started=$(now_seconds)
BILICOMICS_HOME=$run_root BILICOMICS_PHYSICAL_TOKEN=$token BILICOMICS_LAUNCH_TRACE=1 DISPLAY=:0.0 PATH=$run_root/tools:$PATH sh "$run_root/launch.sh" --detached >>"$logs/launcher.log" 2>&1 &
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
