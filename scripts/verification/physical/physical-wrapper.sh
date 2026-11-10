#!/bin/sh
# Install only in a fresh supervised physical-device run directory.
set -eu
umask 077

# Internal observation calls are separate launcher children, never UI runs.
relay_mode=0
portrait_probe_mode=0
portrait_publish_mode=0
native_capture_mode=0
portrait_profile=''
canonical_sequence() {
    case "$1" in *[!0-9]*|''|0*) return 1 ;; esac
    [ "${#1}" -le 20 ] || return 1
    [ "${#1}" -eq 20 ] || return 0
    sequence_value=$1
    sequence_limit=18446744073709551615
    while [ -n "$sequence_value" ]; do
        sequence_digit=${sequence_value%"${sequence_value#?}"}
        limit_digit=${sequence_limit%"${sequence_limit#?}"}
        [ "$sequence_digit" -ge "$limit_digit" ] || return 0
        [ "$sequence_digit" -le "$limit_digit" ] || return 1
        sequence_value=${sequence_value#?}
        sequence_limit=${sequence_limit#?}
    done
}
for argument in "$@"; do
    if [ "$argument" = --relay-power-events ]; then
        [ "$#" -eq 1 ] || exit 2
        relay_mode=1
    fi
    if [ "$argument" = --probe-native-portrait-framebuffer ]; then
        [ "$#" -eq 4 ] && [ "$1" = --probe-native-portrait-framebuffer ] \
            && [ "$2" = --kindle ] && [ "$3" = --device ] || exit 2
        case "$4" in pw5|pw6|scribe|scribe2025) ;; *) exit 2 ;; esac
        portrait_probe_mode=1
        portrait_profile=$4
    fi
    if [ "$argument" = --capture-native-display ]; then
        [ "$#" -eq 4 ] && [ "$1" = --capture-native-display ] \
            && [ "$2" = --kindle ] && [ "$3" = --device ] || exit 2
        case "$4" in pw5|pw6|scribe|scribe2025) ;; *) exit 2 ;; esac
        native_capture_mode=1
        portrait_profile=$4
    fi
    if [ "$argument" = --publish-native-portrait-orientation ]; then
        [ "$#" -eq 5 ] && [ "$1" = --publish-native-portrait-orientation ] \
            && [ "$3" = --kindle ] && [ "$4" = --device ] || exit 2
        case "$5" in pw5|pw6|scribe|scribe2025) ;; *) exit 2 ;; esac
        case "$2" in
            U:[0-3]|D:[0-3]|FIXED:[0-3]) ;;
            CALIBRATED:*) canonical_sequence "${2#CALIBRATED:}" || exit 2 ;;
            *) exit 2 ;;
        esac
        portrait_publish_mode=1
        portrait_profile=$5
    fi
done

run_root=${BILICOMICS_HOME:?}
case "$run_root" in
    /mnt/us/bilicomics-physical-*) run_id=${run_root#/mnt/us/bilicomics-physical-} ;;
    *) exit 2 ;;
esac
case "$run_id" in *[!a-z0-9-]*|'') exit 2 ;; esac
[ "${#run_id}" -le 48 ] || exit 2
logs=$run_root/data/logs
lock=/tmp/bilicomics-launcher.lock
physical_wrapper_cr=$(printf '\r')
read_one() {
    [ -r "$1" ] || return 2
    physical_wrapper_first_line=''
    if IFS= read -r physical_wrapper_first_line <"$1"; then
        printf '%s\n' "${physical_wrapper_first_line%"$physical_wrapper_cr"}"
    elif [ -n "$physical_wrapper_first_line" ]; then
        # Preserve an unterminated first line instead of losing it on EOF.
        printf '%s' "${physical_wrapper_first_line%"$physical_wrapper_cr"}"
    fi
}
read_record() { [ -f "$1" ] && [ ! -L "$1" ] && read_one "$1"; }
is_pid() {
    case "$1" in *[!0-9]*|''|0*|1) return 1 ;; esac
    [ "${#1}" -le 10 ]
}
has_token() {
    [ -r "/proc/$1/environ" ] || return 1
    physical_wrapper_token_lines=$(tr '\000' '\n' <"/proc/$1/environ") || return 1
    # Padding restores complete line boundaries after command substitution has
    # removed trailing newlines. Match the same NUL-to-newline record as before.
    case "
$physical_wrapper_token_lines
" in
        *"
BILICOMICS_PHYSICAL_TOKEN=$token
"*) return 0 ;;
        *) return 1 ;;
    esac
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
read_exact_record() {
    [ -f "$1" ] && [ ! -L "$1" ] || return 1
    exact_record_budget=${2:-256}
    case "$exact_record_budget" in 256|512) ;; *) return 1 ;; esac
    exact_record_octets=$(od -An -v -tx1 -N "$((exact_record_budget + 1))" "$1" 2>/dev/null) || return 1
    exact_record_hex=''
    for exact_record_octet in $exact_record_octets; do
        exact_record_hex=$exact_record_hex$exact_record_octet
    done
    [ -n "$exact_record_hex" ] && [ "${#exact_record_hex}" -le $((exact_record_budget * 2)) ] || return 1
    exact_record_value=$(read_one "$1") || return 1
    exact_record_octets=$(printf '%s\n' "$exact_record_value" | od -An -v -tx1) || return 1
    exact_record_expected=''
    for exact_record_octet in $exact_record_octets; do
        exact_record_expected=$exact_record_expected$exact_record_octet
    done
    [ "$exact_record_hex" = "$exact_record_expected" ] || return 1
    printf '%s\n' "$exact_record_value"
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
    # Keep the finest timestamp representation supplied by stat, including its
    # fractional positions. Literal/unsupported fields are not a cache fallback.
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
    # Kindle exposes the known /tmp base through /var/tmp. No other resolved
    # base, or symlink at a receipt directory/file leaf, is authorized here.
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
load_verified_elf_receipt() {
    elf_temporary_base=$(elf_canonical_temporary_base) || return 1
    elf_receipt_dir=$elf_temporary_base/bilicomics-physical-$run_id-elf
    elf_receipt_path=$elf_receipt_dir/verified-elf.receipt
    elf_receipt_directory=$(elf_receipt_directory_identity) || return 1
    elf_receipt_file=$(elf_file_identity "$elf_receipt_path") || return 1
    IFS='|' read -r cache_dev cache_inode cache_uid cache_mode cache_links cache_size cache_mtime cache_ctime <<EOF
$elf_receipt_file
EOF
    [ "$cache_mode" = 8180 ] && [ "$cache_size" -le 512 ] || return 1
    elf_receipt_record=$(read_exact_record "$elf_receipt_path" 512) || return 1
    IFS='|' read -r cache_version cache_run cache_token cache_binary cache_issuer cache_birth cache_dev cache_inode cache_uid cache_mode cache_links cache_size cache_mtime cache_ctime cache_extra <<EOF
$elf_receipt_record
EOF
    [ "$cache_version" = v1 ] && [ "$cache_run" = "$run_id" ] && [ "$cache_token" = "$token" ] \
        && [ "$cache_binary" = "$binary" ] && [ -z "$cache_extra" ] || return 1
    is_pid "$cache_issuer" || return 1
    case "$cache_birth" in *[!0-9]*|'') return 1 ;; esac
    [ "${#cache_birth}" -le 20 ] || return 1
    elf_cached_identity="$cache_dev|$cache_inode|$cache_uid|$cache_mode|$cache_links|$cache_size|$cache_mtime|$cache_ctime"
    [ "$elf_receipt_record" = "v1|$run_id|$token|$binary|$cache_issuer|$cache_birth|$elf_cached_identity" ] || return 1
    [ "$(elf_file_identity "$real_binary" 1)" = "$elf_cached_identity" ] || return 1
    elf_receipt_issuer=$cache_issuer
    elf_receipt_birth=$cache_birth
    [ "$(elf_file_identity "$elf_receipt_path")" = "$elf_receipt_file" ] \
        && [ "$(elf_receipt_directory_identity)" = "$elf_receipt_directory" ] || return 1
}
bind_verified_elf_issuer() {
    [ "$launcher_parent" = "$elf_receipt_issuer" ] || return 1
    elf_issuer_snapshot=$(snapshot_process "$elf_receipt_issuer") || return 1
    IFS='|' read -r elf_issuer_pid elf_issuer_birth elf_issuer_parent elf_issuer_group elf_issuer_session elf_issuer_exe <<EOF
$elf_issuer_snapshot
EOF
    [ "$elf_issuer_birth" = "$elf_receipt_birth" ] \
        && [ "$(read_record "$logs/watchdog.identity")" = "$elf_receipt_issuer|$elf_receipt_birth|$elf_issuer_exe" ] || return 1
}
check_verified_elf_receipt() {
    [ "$(read_record "$logs/run-token")" = "$token" ] && [ "$(read_record "$logs/binary-name")" = "$binary" ] \
        && [ "$(elf_receipt_directory_identity)" = "$elf_receipt_directory" ] \
        && [ "$(elf_file_identity "$elf_receipt_path")" = "$elf_receipt_file" ] \
        && [ "$(read_exact_record "$elf_receipt_path" 512)" = "$elf_receipt_record" ] \
        && [ "$(elf_file_identity "$real_binary" 1)" = "$elf_cached_identity" ] \
        && [ "$(read_record "$logs/watchdog.identity")" = "$elf_receipt_issuer|$elf_receipt_birth|$elf_issuer_exe" ] \
        && assert_process "$elf_receipt_issuer" "$elf_issuer_snapshot"
}
lock_inode() { stat -c '%i' "$lock" 2>/dev/null; }
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
physical_wrapper_trace() {
    [ "${BILICOMICS_LAUNCH_TRACE:-0}" = 1 ] || return 0
    case "${1:-}" in
        probe_entry|capture_entry|publish_entry|elf_receipt_loaded|observer_loaded|issuer_bound|ancestry_scanned|ancestry_rechecked|final_checks_entry) ;;
        *) return 0 ;;
    esac
    [ -d "$run_root" ] && [ ! -L "$run_root" ] && [ ! -L "$run_root/data" ] \
        && [ -d "$logs" ] && [ ! -L "$logs" ] || return 0
    physical_trace_path=$logs/physical-wrapper-timing.tsv
    [ ! -L "$physical_trace_path" ] || return 0
    if [ -e "$physical_trace_path" ] && [ ! -f "$physical_trace_path" ]; then return 0; fi
    # Keep the optional diagnostic write and its file-size limit in a child.
    # No tracing failure may change the helper's checks, limits, or exit status.
    (
        ulimit -f 16 || exit 0
        IFS=' ' read -r physical_trace_uptime physical_trace_unused </proc/uptime || exit 0
        case "$physical_trace_uptime" in *[!0-9.]*|'') exit 0 ;; esac
        [ "${#physical_trace_uptime}" -le 32 ] || exit 0
        printf '%s\t%s\tpid=%s\n' "$physical_trace_uptime" "$1" "$$" >>"$physical_trace_path"
    ) 2>/dev/null || :
}

token=$(read_record "$logs/run-token")
case "$token" in *[!0-9a-f]*|'') exit 2 ;; esac
[ "${#token}" -eq 64 ] || exit 2
[ "${BILICOMICS_PHYSICAL_TOKEN:-}" = "$token" ] || exit 2
binary=$(read_record "$logs/binary-name")
case "$binary" in bilicomics-*) digest=${binary#bilicomics-} ;; *) exit 2 ;; esac
case "$digest" in *[!0-9a-f]*|'') exit 2 ;; esac
[ "${#digest}" -eq 64 ] || exit 2
real_binary=$run_root/app/$binary
[ -f "$real_binary" ] && [ ! -L "$real_binary" ] && [ -x "$real_binary" ] || exit 2

if [ "$portrait_probe_mode" -eq 1 ] || [ "$native_capture_mode" -eq 1 ] || [ "$portrait_publish_mode" -eq 1 ]; then
    if [ "$native_capture_mode" -eq 1 ]; then
        physical_wrapper_trace capture_entry
    elif [ "$portrait_probe_mode" -eq 1 ]; then
        physical_wrapper_trace probe_entry
    else
        physical_wrapper_trace publish_entry
    fi
    [ "${BILICOMICS_APPROVED_PROFILE:-}" = "$portrait_profile" ] || exit 2
    [ "$(read_record "$run_root/data/device-profile")" = "$portrait_profile" ] || exit 2
    elf_owner=$(id -u) || exit 2
    load_verified_elf_receipt || exit 2
    physical_wrapper_trace elf_receipt_loaded
    load_observer || exit 2
    physical_wrapper_trace observer_loaded
    bind_verified_elf_issuer || exit 2
    physical_wrapper_trace issuer_bound
    if [ "$native_capture_mode" -eq 1 ] || [ "$portrait_publish_mode" -eq 1 ]; then
        [ "${BILICOMICS_LIFECYCLE_DIR:-}" = "$lock" ] \
            && [ "${BILICOMICS_LAUNCH_GENERATION:-}" = "$launch_generation" ] \
            && [ "${BILICOMICS_LAUNCH_TOKEN:-}" = "$launch_nonce" ] || exit 2
    fi
    probe_permission=$(read_exact_record "$lock/native-read-go") || exit 2
    IFS=: read -r probe_leader probe_sequence probe_nonce probe_extra <<EOF
$probe_permission
EOF
    is_pid "$probe_leader" || exit 2
    case "$probe_sequence" in *[!0-9]*|''|0*) exit 2 ;; esac
    [ "${#probe_sequence}" -le 10 ] && [ -z "$probe_extra" ] \
        && [ "$probe_permission" = "$probe_leader:$probe_sequence:$launch_nonce" ] \
        && [ "$probe_nonce" = "$launch_nonce" ] || exit 2
    probe_leader_snapshot=$(snapshot_process "$probe_leader") || exit 2
    IFS='|' read -r probe_id probe_birth probe_parent probe_pgid probe_sid probe_exe <<EOF
$probe_leader_snapshot
EOF
    [ "$probe_parent" = "$launcher_pid" ] \
        && [ "$probe_pgid:$probe_sid" = "$probe_leader:$probe_leader" ] \
        && [ "$probe_exe" = "$launcher_exe" ] || exit 2
    # Every pipeline shell between this helper and the permitted native reader
    # must remain in that reader's private session and retain its live identity.
    probe_member=$$
    probe_chain=''
    probe_remaining=12
    probe_reached_leader=0
    while [ "$probe_remaining" -gt 0 ]; do
        probe_snapshot=$(snapshot_process "$probe_member") || exit 2
        IFS='|' read -r probe_id probe_birth probe_parent probe_pgid probe_sid probe_exe <<EOF
$probe_snapshot
EOF
        [ "$probe_pgid:$probe_sid" = "$probe_leader:$probe_leader" ] \
            && [ "$probe_exe" = "$launcher_exe" ] || exit 2
        probe_chain="$probe_chain
$probe_snapshot"
        if [ "$probe_member" = "$probe_leader" ]; then
            [ "$probe_snapshot" = "$probe_leader_snapshot" ] || exit 2
            probe_reached_leader=1
            break
        fi
        probe_member=$probe_parent
        probe_remaining=$((probe_remaining - 1))
    done
    [ "$probe_reached_leader" -eq 1 ] || exit 2
    physical_wrapper_trace ancestry_scanned
    while IFS='|' read -r probe_id probe_birth probe_parent probe_pgid probe_sid probe_exe; do
        [ -n "$probe_id" ] || continue
        assert_process "$probe_id" "$probe_id|$probe_birth|$probe_parent|$probe_pgid|$probe_sid|$probe_exe" || exit 2
    done <<EOF
$probe_chain
EOF
    physical_wrapper_trace ancestry_rechecked
    physical_wrapper_trace final_checks_entry
    [ "$(read_exact_record "$lock/native-read-go")" = "$probe_permission" ] \
        && check_observer && assert_process "$probe_leader" "$probe_leader_snapshot" \
        && check_verified_elf_receipt || exit 2
    # Do not read UI controls, append demonstration arguments, or publish UI IDs.
    exec "$real_binary" "$@"
fi

if [ "$relay_mode" -eq 1 ]; then
    # This branch must not read UI controls or publish the native UI identity.
    [ "$(readlink -f "$real_binary")" = "$real_binary" ] || exit 2
    [ "$(sha256sum "$real_binary" | awk '{ print $1 }')" = "$digest" ] || exit 2
    load_observer || exit 2
    relay_snapshot=$(snapshot_process "$$") || exit 2
    IFS='|' read -r relay_pid relay_birth relay_parent relay_pgid relay_sid relay_exe <<EOF
$relay_snapshot
EOF
    [ "$relay_parent" = "$observer_pid" ] && [ "$relay_pgid:$relay_sid" = "$observer_pid:$observer_pid" ] \
        && [ "$relay_exe" = "$observer_exe" ] || exit 2
    check_observer && assert_process "$$" "$relay_snapshot" || exit 2
    publish_line observer-anchor.identity "$observer_pid|$observer_birth|$observer_exe|$launcher_pid|$token" || exit 2
    check_observer && assert_process "$$" "$relay_snapshot" || exit 2
    publish_line power-relay.identity "$relay_pid|$relay_birth|$real_binary|$observer_pid|$token" || exit 2
    check_observer && assert_process "$$" "$relay_snapshot" || exit 2
    exec "$real_binary" "$@"
fi

# UI execution is outside the production native-reader deadline and retains
# its complete content check; only native-display helpers use the receipt.
[ "$(sha256sum "$real_binary" | awk '{ print $1 }')" = "$digest" ] || exit 2
screen=$(read_one "$logs/screen")
case "$screen" in shelf|reader) ;; *) exit 2 ;; esac
duration=$(read_one "$logs/exit-after-ms")
case "$duration" in *[!0-9]*|'') exit 2 ;; esac
[ "$duration" -ge 1000 ] && [ "$duration" -le 600000 ] || exit 2
content_mode=demo
if [ -e "$logs/content-mode" ] || [ -L "$logs/content-mode" ]; then
    content_mode=$(read_exact_record "$logs/content-mode") || exit 2
fi
case "$content_mode" in
    demo) ;;
    distinct_fixture)
        # The production launcher supplies exactly one isolated data directory.
        [ "$#" -eq 5 ] && [ "$1" = --kindle ] && [ "$2" = --device ] \
            && [ "$4" = --data-dir ] && [ "$5" = "$run_root/data" ] && [ "$screen" = reader ] || exit 2
        case "$3" in pw5|pw6|scribe|scribe2025) ;; *) exit 2 ;; esac
        [ ! -e "$run_root/data/account" ] && [ ! -L "$run_root/data/account" ] || exit 2
        guard_name=$(read_exact_record "$logs/socket-guard-name") || exit 2
        guard_digest=$(read_exact_record "$logs/socket-guard-sha256") || exit 2
        fixture_digest=$(read_exact_record "$logs/fixture-sha256") || exit 2
        case "$guard_digest:$fixture_digest" in *[!0-9a-f:]*|:*) exit 2 ;; esac
        [ "${#guard_digest}" -eq 64 ] && [ "${#fixture_digest}" -eq 64 ] \
            && [ "$guard_name" = "offline-socket-guard-$guard_digest" ] || exit 2
        [ "$(read_exact_record "$logs/socket-guard-approved")" = "$token|$guard_digest|$fixture_digest" ] || exit 2
        socket_guard=$run_root/app/$guard_name
        [ -f "$socket_guard" ] && [ ! -L "$socket_guard" ] && [ -x "$socket_guard" ] \
            && [ "$(readlink -f "$socket_guard")" = "$socket_guard" ] \
            && [ "$(sha256sum "$socket_guard" | awk '{ print $1 }')" = "$guard_digest" ] || exit 2
        ;;
    *) exit 2 ;;
esac
performance_trace=0
if [ -e "$logs/performance-trace-enabled" ] || [ -L "$logs/performance-trace-enabled" ]; then
    performance_trace=$(read_record "$logs/performance-trace-enabled") || exit 2
fi
case "$performance_trace" in
    ''|0) ;;
    1) set -- "$@" --performance-trace "$logs/performance.jsonl" ;;
    *) exit 2 ;;
esac
[ "$content_mode" != distinct_fixture ] || [ "$performance_trace" = 1 ] || exit 2

# exec retains the PID expected by the production launcher's signal bridge.
stat_tail=$(sed 's/^.*) //' "/proc/$$/stat")
start_ticks=$(printf '%s\n' "$stat_tail" | awk '{ print $20 }')
parent_pid=$(printf '%s\n' "$stat_tail" | awk '{ print $2 }')
case "$start_ticks:$parent_pid" in *[!0-9:]*|:*) exit 2 ;; esac
launcher_pid=$(read_one "$lock/pid")
parent_tail=$(sed 's/^.*) //' "/proc/$parent_pid/stat")
parent_ticks=$(printf '%s\n' "$parent_tail" | awk '{ print $20 }')
parent_parent=$(printf '%s\n' "$parent_tail" | awk '{ print $2 }')
parent_exe=$(readlink "/proc/$parent_pid/exe")
[ "$parent_parent" = "$launcher_pid" ] || exit 2
[ "${BILICOMICS_LAUNCH_GENERATION:-}" = 1 ] || exit 2
[ "${BILICOMICS_LAUNCH_TOKEN:-}" = "$(read_one "$lock/nonce")" ] || exit 2
tr '\000' '\n' <"/proc/$parent_pid/environ" | grep -F -x "BILICOMICS_PHYSICAL_TOKEN=$token" >/dev/null
publish_line runner.identity "$parent_pid|$parent_ticks|$parent_exe|$launcher_pid|$token" || exit 2
publish_line native.pid "$$" || exit 2
publish_line native.identity "$$|$start_ticks|$real_binary|$parent_pid|$token" || exit 2
if [ "$content_mode" = distinct_fixture ]; then
    # The guard installs and proves its filter before the same PID execs the ARM.
    exec "$socket_guard" --proof-file "$logs/ui-socket-guard.json" -- "$real_binary" "$@" --offline --screen "$screen" \
        --lifecycle-trace "$logs/lifecycle.jsonl" --exit-after-ms "$duration"
fi
exec "$real_binary" "$@" --demo --offline --screen "$screen" \
    --lifecycle-trace "$logs/lifecycle.jsonl" --exit-after-ms "$duration"
