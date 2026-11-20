#!/bin/sh
set -eu
umask 077
export LC_ALL=C
run=${1:?}
op=${2:?}
case "$run" in bili-prod-????????) :;; *) exit 2;; esac
case "${run#bili-prod-}" in *[!0-9a-f]*) exit 2;; esac
stage=/mnt/us/bilicomics-$run
priv=/var/tmp/bilicomics-$run
state=$priv/state
package=$stage/package
home=/mnt/us/bilicomics
doc=/mnt/us/documents/bilicomics.sh
lock=/tmp/bilicomics-launcher.lock
lock_var_parent=/var/tmp
present() { [ -e "$1" ] || [ -L "$1" ]; }
plain() { [ -f "$1" ] && [ ! -L "$1" ]; }
dir() { [ -d "$1" ] && [ ! -L "$1" ] && [ "$(readlink -f "$1")" = "$1" ]; }
for root in "$stage" "$priv"; do
    dir "$root" && plain "$root/owner" && printf '%s\n' "$run" | cmp -s - "$root/owner" || exit 2
done
dir "$stage/tools" && dir "$package" && dir "$package/bilicomics" && dir "$package/documents" || exit 2
native() {
    [ "$(lipc-get-prop com.lab126.appmgrd activeApp)" = com.lab126.KPPMainApp ] &&
        [ "$(lipc-get-prop -s com.lab126.powerd state)" = active ] || return 1
    case "$(status statusbar)" in *start/running*) :;; *) return 1;; esac
    for project in bilicomics aidoku weread; do ! present "/tmp/$project-launcher.lock" || return 1; done
    managers=$(pidof awesome) && [ -n "$managers" ] || return 1
    for manager in $managers; do
        case "$(sed -n 's/^State:[[:space:]]*\([A-Z]\).*/\1/p' "/proc/$manager/status")" in R|S) :;; *) return 1;; esac
    done
}
sql() { sqlite3 -readonly -cmd '.timeout 1500' "/var/local/$1.db" "$2"; }
rows() {
    sql cc "SELECT count(*)||':'||coalesce(sum(p_location COLLATE BINARY='$doc' AND
        p_mimeType='text/x-shellscript' AND p_titles_0_nominal='BiliComics for Kindle'),0)
        FROM Entries WHERE p_location COLLATE NOCASE='$doc';"
}
await_rows() {
    remaining=40
    while [ "$remaining" -gt 0 ]; do
        [ "$(rows)" = "$1" ] && return 0
        remaining=$((remaining - 1)); sleep 1
    done
    return 1
}
scan() { lipc-set-prop -i com.lab126.scanner doFullScan 1 >/dev/null; }
document_lookup() {
    if present "$1"; then kind_id f "$1"; else printf 'absent\n'; fi
}
document_matches() { cmp -s "$1" "$2"; }
only_probe_name() {
    seen=0
    for candidate in "$1"/* "$1"/.[!.]* "$1"/..?*; do
        present "$candidate" || continue
        [ "${2:-one}" = one ] && [ "${candidate##*/}" = foldprobe ] || return 1
        seen=$((seen + 1))
    done
    case "${2:-one}" in empty) [ "$seen" -eq 0 ];; one) [ "$seen" -eq 1 ];; *) return 1;; esac
}
await_probe_gone() {
    attempts=71
    while [ "$attempts" -gt 0 ]; do
        low_view=$(document_lookup "$sample") && high_view=$(document_lookup "$variant") || return 1
        if ! present "$probe_dir" && [ "$low_view:$high_view" = absent:absent ]; then return 0; fi
        if present "$probe_dir"; then
            [ "$(kind_id d "$probe_dir")" = "$probe_identity" ] && only_probe_name "$probe_dir" empty || return 1
        fi
        attempts=$((attempts - 1))
        if [ "$attempts" -gt 0 ]; then usleep 100000 || return 1; fi
    done
    return 1
}
casefold_proof() (
    media_device=$1
    probe_dir=$stage/.document-casefold
    sample=$probe_dir/foldprobe
    variant=$probe_dir/FoldProbe
    probe_identity=; sample_identity=; unlinked=no; probe_mode=
    [ "$(stat -c %d "$stage")" = "$media_device" ] && mkdir "$probe_dir" || exit 1
    probe_identity=$(kind_id d "$probe_dir") || exit 1
    dispose_probe() {
        outcome=$?
        trap - 0 HUP INT TERM
        [ "$(kind_id d "$probe_dir")" = "$probe_identity" ] || exit 1
        if [ "$unlinked" = no ] && present "$sample"; then
            [ -n "$sample_identity" ] && only_probe_name "$probe_dir" &&
                [ "$(kind_id f "$sample")" = "$sample_identity" ] && rm "$sample" || exit 1
        fi
        [ "$(kind_id d "$probe_dir")" = "$probe_identity" ] && only_probe_name "$probe_dir" empty && rmdir "$probe_dir" || exit 1
        await_probe_gone || exit 1
        if [ "$outcome" -eq 0 ]; then printf '%s\n' "$probe_mode"; fi
        exit "$outcome"
    }
    trap dispose_probe 0
    trap 'exit 1' HUP INT TERM
    [ "${probe_identity%%:*}" = "$media_device" ] && [ "$(document_lookup "$sample")" = absent ] &&
        [ "$(document_lookup "$variant")" = absent ] || exit 1
    (set -C; printf '%s:first\n' "$run" >"$sample") || exit 1
    sample_identity=$(kind_id f "$sample") || exit 1
    [ "${sample_identity%%:*}" = "$media_device" ] || exit 1
    for phase in first second; do
        only_probe_name "$probe_dir" && [ "$(kind_id f "$sample")" = "$sample_identity" ] || exit 1
        if [ "$phase" = second ]; then printf '%s:second\n' "$run" >"$sample" || exit 1; fi
        alternate=$(document_lookup "$variant") && printf '%s:%s\n' "$run" "$phase" | document_matches - "$sample" || exit 1
        if [ "$alternate" = absent ]; then observed=exact
        else
            [ "${alternate%%:*}" = "$media_device" ] && printf '%s:%s\n' "$run" "$phase" | document_matches - "$variant" || exit 1
            observed=folded
        fi
        if [ "$phase" = second ]; then [ "$observed" = "$probe_mode" ] || exit 1; fi
        probe_mode=$observed
    done
    only_probe_name "$probe_dir" && [ "$(kind_id f "$sample")" = "$sample_identity" ] && rm "$sample" || exit 1
    unlinked=yes
)
proof_context() {
    stage_identity=$(kind_id d "$stage") && directory_identity=$(kind_id d "${doc%/*}") &&
        state_identity=$(kind_id d "$state") && boot_identity=$(cat /proc/sys/kernel/random/boot_id) || return 1
    [ -n "$boot_identity" ] && [ "${stage_identity%%:*}" = "${directory_identity%%:*}" ] || return 1
    printf '%s %s %s %s %s\n' "$run" "$boot_identity" "$stage_identity" "$directory_identity" "$state_identity"
}
prepare_volume() {
    ! present "$state/document-volume" && initial=$(proof_context) && volume=$(stat -c %d "${doc%/*}") &&
        policy=$(casefold_proof "$volume") && [ "$initial" = "$(proof_context)" ] || return 1
    case "$policy" in exact|folded) :;; *) return 1;; esac
    (set -C; printf '%s %s\n' "$policy" "$initial" >"$state/document-volume")
}
volume_policy() {
    plain "$state/document-volume" && dir "$state" && [ "$(stat -c '%u:%a:%h' "$state/document-volume")" = 0:600:1 ] &&
        context=$(proof_context) || return 1
    for policy in folded exact; do
        if printf '%s %s\n' "$policy" "$context" | cmp -s - "$state/document-volume"; then printf '%s\n' "$policy"; return 0; fi
    done
    return 1
}
extra_absent() {
    directory=${doc%/*}
    dir "$directory" && [ -r "$directory" ] && [ -x "$directory" ] || return 1
    spelled=absent
    for leaf in "$directory"/[Bb][Ii][Ll][Ii][Cc][Oo][Mm][Ii][Cc][Ss].*; do
        present "$leaf" || continue
        name=${leaf##*/}
        case "$name" in
            [Bb][Ii][Ll][Ii][Cc][Oo][Mm][Ii][Cc][Ss].[Ss][Hh])
                [ "$name" = bilicomics.sh ] && [ "$spelled" = absent ] || return 1
                spelled=lower;;
            [Bb][Ii][Ll][Ii][Cc][Oo][Mm][Ii][Cc][Ss].[Ss][Dd][Rr]|[Bb][Ii][Ll][Ii][Cc][Oo][Mm][Ii][Cc][Ss].[Ss][Hh].[Ss][Dd][Rr]) return 1;;
        esac
    done
    lower=$(document_lookup "$doc") && upper=$(document_lookup "$directory/BiliComics.sh") || return 1
    alias_mode=absent
    if [ "$spelled" = lower ]; then
        [ "$lower" != absent ] && public_policy=$(volume_policy) && plain "$package/documents/bilicomics.sh" &&
            document_matches "$package/documents/bilicomics.sh" "$doc" || return 1
        if [ "$upper" != absent ]; then
            [ "$public_policy" = folded ] && [ "${upper%%:*}" = "${lower%%:*}" ] && [ "${lower%%:*}" = "$(stat -c %d "$directory")" ] &&
                document_matches "$package/documents/bilicomics.sh" "$directory/BiliComics.sh" || return 1
            if [ "$upper" = "$lower" ]; then alias_mode=same-inode
            else
                [ "$(rows)" = 1:1 ] || return 1
                alias_mode=casefold-proven
            fi
            [ "$(document_lookup "$doc")" = "$lower" ] && [ "$(document_lookup "$directory/BiliComics.sh")" = "$upper" ] || return 1
        else [ "$public_policy" = exact ] || return 1; fi
    else [ "$lower" = absent ] && [ "$upper" = absent ] || return 1; fi
    for extra in "$doc.sdr" /mnt/us/documents/bilicomics.sdr \
        /mnt/us/documents/BiliComics.sh.sdr /mnt/us/documents/BiliComics.sdr; do ! present "$extra" || return 1; done
}
protect() {
    handler=$(sql appreg "SELECT value FROM properties WHERE handlerId='tech.hackerdude.shell_integration.launcher' AND name='command';") || return 1
    case "$handler" in /var/local/kmc/bin/sh_integration_launcher|/var/local/kindle-catalog-entry/v1-*/aidoku-library-dispatch) :;; *) return 1;; esac
    case "$handler" in *[!a-zA-Z0-9./_-]*|*/../*|*//*|*/./*) return 1;; esac
    plain "$handler" && [ -x "$handler" ] || return 1
    printf 'handler=%s\n' "$handler" || return 1
    for peer in "$handler" /var/local/kmc/bin/sh_integration_launcher /mnt/us/documents/Aidoku.sh /mnt/us/documents/aidoku.sh \
        /mnt/us/documents/weread.sh /mnt/us/documents/WeRead.sh /mnt/us/aidoku/launch.sh /mnt/us/aidoku/app/aidoku \
        /mnt/us/weread/launch.sh /mnt/us/weread/app/weread; do
        if present "$peer"; then identity=$(stat -c '%d:%i:%f:%s:%y' "$peer") || return 1; else identity=absent; fi
        printf '%s=%s\n' "$peer" "$identity" || return 1
    done
    sql cc "SELECT quote(p_location)||','||quote(p_titles_0_nominal)||','||quote(p_mimeType) AS snapshot FROM Entries
        WHERE p_location COLLATE BINARY IN ('/mnt/us/documents/Aidoku.sh','/mnt/us/documents/aidoku.sh',
        '/mnt/us/documents/weread.sh','/mnt/us/documents/WeRead.sh') ORDER BY snapshot COLLATE BINARY;" || return 1
    for prop in flIntensity currentAmberLevel; do
        value=$(lipc-get-prop com.lab126.powerd "$prop" 2>/dev/null || :)
        case "$value" in ''|*[!0-9]*) value=unavailable;; esac
        printf '%s=%s\n' "$prop" "$value" || return 1
    done
    cat /proc/sys/kernel/random/boot_id
}
unchanged() {
    dir "$state" && plain "$state/before" && protect >"$state/after" && cmp -s "$state/before" "$state/after"
}
kind_id() {
    case "$1" in d) dir "$2";; f) plain "$2" && [ "$(stat -c %h "$2")" = 1 ];; *) return 1;; esac || return 1
    stat -c '%d:%i' "$2"
}
remember() {
    identity=$(kind_id "$1" "$2") || return 1
    printf '%s %s %s\n' "$1" "$identity" "$2" >>"$state/journal"
}
make_dir() {
    dir "${1%/*}" && mkdir "$1" && remember d "$1"
}
copy_file() {
    dir "${2%/*}" && plain "$1" && ! present "$2" &&
        (set -C; cat "$1" >"$2") && remember f "$2" && chmod "$3" "$2" && cmp -s "$1" "$2"
}
valid_relative() {
    case "$1" in ''|*[!A-Za-z0-9_./+-]*|/*|*/|*//*|.|..|*/.|*/..|./*|../*|*/./*|*/../*) return 1;; esac
}
package_list() {
    (cd "$package" && find . ! -type d -print) >"$state/found" || return 1
    [ "$(wc -l <"$state/found")" -eq 49 ] || return 1
    while IFS= read -r item; do
        relative=${item#./}
        valid_relative "$relative" && dir "${package}/${relative%/*}" 2>/dev/null || {
            case "$relative" in README.md|THIRD_PARTY_NOTICES.md) :;; *) return 1;; esac
        }
        plain "$package/$relative" || return 1
        case "$relative" in
            bilicomics/launch.sh|bilicomics/app/bilicomics|bilicomics/app/kindle-jpeg-worker|bilicomics/data/device-profile|bilicomics/data/device-allowlist|documents/bilicomics.sh|README.md|THIRD_PARTY_NOTICES.md|notices/*) :;;
            *) return 1;;
        esac
    done <"$state/found"
    for item in bilicomics/launch.sh bilicomics/app/bilicomics bilicomics/app/kindle-jpeg-worker \
        bilicomics/data/device-profile bilicomics/data/device-allowlist documents/bilicomics.sh README.md THIRD_PARTY_NOTICES.md notices/Cargo.lock; do
        plain "$package/$item" || return 1
    done
    (cd "$package" && find notices -type d -print | sort) >"$state/notice-dirs"
}
new_config() {
    printf 'scribe\n' >"$state/profile"
    printf 'scribe 5.19.5\n' >"$state/allowlist"
}
source_for() {
    case "$1" in
        "$doc") printf '%s\n' "$package/documents/bilicomics.sh";;
        "$home/data/device-profile") printf '%s\n' "$state/profile";;
        "$home/data/device-allowlist") printf '%s\n' "$state/allowlist";;
        "$home/launch.sh"|"$home/app/bilicomics"|"$home/app/kindle-jpeg-worker") printf '%s\n' "$package/bilicomics/${1#"$home"/}";;
        "$home/README.md"|"$home/THIRD_PARTY_NOTICES.md"|"$home/notices/"*) printf '%s\n' "$package/${1#"$home"/}";;
        *) return 1;;
    esac
}
journal_matches() {
    dir "$state" && plain "$state/journal" || return 1
    while read -r kind recorded path extra; do
        [ -z "$extra" ] || return 1
        case "$path" in "$home"|"$doc") :;; "$home"/*) valid_relative "${path#"$home"/}" || return 1;; *) return 1;; esac
        dir "${path%/*}" && [ "$(kind_id "$kind" "$path")" = "$recorded" ] || return 1
        if [ "$kind" = f ]; then
            source=$(source_for "$path") && plain "$source" || return 1
            if [ "$path" != "$home/app/bilicomics" ] || [ "${1:-full}" != fast ]; then cmp -s "$source" "$path" || return 1; fi
        fi
    done <"$state/journal"
}
installed() {
    plain "$state/installed" && journal_matches "${1:-full}" && extra_absent && [ "$(rows)" = 1:1 ] && unchanged
}
install() {
    native && dir "${doc%/*}" && dir "${home%/*}" && ! present "$home" && ! present "$doc" &&
        extra_absent && [ "$(rows)" = 0:0 ] || return 1
    [ "$(sed -n 's/^Kindle[[:space:]]*\([0-9][0-9.]*\).*/\1/p' /etc/prettyversion.txt | head -n 1)" = 5.19.5 ] || return 1
    command -v setsid >/dev/null && command -v usleep >/dev/null || return 1
    mkdir "$state" && protect >"$state/before.pending" && mv "$state/before.pending" "$state/before" || return 1
    prepare_volume && package_list && new_config && : >"$state/journal" || return 1
    make_dir "$home" && make_dir "$home/app" && make_dir "$home/data" || return 1
    for path in launch.sh app/bilicomics app/kindle-jpeg-worker; do
        copy_file "$package/bilicomics/$path" "$home/$path" 755 || return 1
    done
    copy_file "$state/profile" "$home/data/device-profile" 600 && copy_file "$state/allowlist" "$home/data/device-allowlist" 600 || return 1
    while IFS= read -r path; do valid_relative "$path" && make_dir "$home/$path" || return 1; done <"$state/notice-dirs"
    while IFS= read -r item; do
        path=${item#./}
        case "$path" in README.md|THIRD_PARTY_NOTICES.md|notices/*) copy_file "$package/$path" "$home/$path" 644 || return 1;; esac
    done <"$state/found"
    extra_absent && copy_file "$package/documents/bilicomics.sh" "$doc" 755 &&
        scan && await_rows 1:1 && : >"$state/installed" && installed fast
}
process() {
    case "$1" in ''|0*|*[!0-9]*) return 1;; esac
    IFS= read -r stat_line <"/proc/$1/stat" || return 1
    [ "${stat_line%% *}" = "$1" ] && tail_fields=${stat_line##*) } && [ "$tail_fields" != "$stat_line" ] || return 1
    set -- $tail_fields
    [ "$#" -ge 20 ] || return 1
    case "$1" in Z|X) return 1;; esac
    parent=$2; group=$3; session=$4; shift 19; birth=$1
    case "$birth" in ''|*[!0-9]*) return 1;; esac
}
shell_argv() {
    [ "$(readlink -f "/proc/$1/exe")" = "$(readlink -f /bin/sh)" ] || return 1
    for shell_name in sh /bin/sh; do
        printf '%s\000' "$shell_name" "$home/launch.sh" "$2" | cmp -s - "/proc/$1/cmdline" && return 0
    done
    case "$(readlink -f /bin/sh)" in */busybox)
        for shell_name in busybox "$(readlink -f /bin/sh)"; do
            printf '%s\000' "$shell_name" sh "$home/launch.sh" "$2" | cmp -s - "/proc/$1/cmdline" && return 0
        done;;
    esac
    return 1
}
production_lock_identity() (
    [ "$lock" = /tmp/bilicomics-launcher.lock ] || exit 1
    lock_parent=${lock%/*}
    if [ -L "$lock_parent" ]; then
        [ "$(stat -c %u "$lock_parent")" = 0 ] || exit 1
        target=$(readlink "$lock_parent") || exit 1
        [ "$target" = "$lock_var_parent" ] || [ "$target" = "${lock_var_parent#/}" ] || exit 1
        dir "$lock_var_parent" && [ "$(readlink -f "$lock_parent")" = "$lock_var_parent" ] || exit 1
        canonical_parent=$lock_var_parent
    else dir "$lock_parent" || exit 1; canonical_parent=$lock_parent; fi
    [ "$(stat -c %u "$canonical_parent")" = 0 ] || exit 1
    canonical=$canonical_parent/bilicomics-launcher.lock
    [ -d "$lock" ] && [ ! -L "$lock" ] && dir "$canonical" && [ "$(readlink -f "$lock")" = "$canonical" ] || exit 1
    identity=$(stat -c '%u:%a:%d:%i' "$lock") && [ "$identity" = "$(stat -c '%u:%a:%d:%i' "$canonical")" ] || exit 1
    case "$identity" in 0:700:*|0:2700:*) printf '%s\n' "$identity";; *) exit 1;; esac
)
worker_identity() {
    plain "$state/armed" && lock_identity=$(production_lock_identity) &&
        plain "$lock/pid" && plain "$lock/nonce" && plain "$lock/generation" || return 1
    IFS= read -r worker <"$lock/pid" && IFS= read -r nonce <"$lock/nonce" || return 1
    case "$nonce" in ''|*[!0-9a-f]*) return 1;; esac
    [ "${#nonce}" -eq 32 ] && printf '1:%s\n' "$nonce" | cmp -s - "$lock/generation" || return 1
    process "$worker" && [ "$group:$session" = "$worker:$worker" ] && shell_argv "$worker" --detached || return 1
    worker_birth=$birth; entry=$parent
    IFS= read -r boundary <"$state/armed" && [ "$worker_birth" -ge "$boundary" ] || return 1
    process "$entry" && [ "$birth" -ge "$boundary" ] && shell_argv "$entry" --library || return 1
    [ "$lock_identity" = "$(production_lock_identity)" ] || return 1
    worker_record="$worker:$worker_birth:$entry:$birth:$nonce:$lock_identity"
    if present "$state/worker"; then
        plain "$state/worker" && printf '%s\n' "$worker_record" | cmp -s - "$state/worker" || return 1
    fi
}
ready_identity() {
    worker_identity && plain "$lock/application-pid" && plain "$lock/lifecycle-ready" || return 1
    printf '1:%s\n' "$nonce" | cmp -s - "$lock/lifecycle-ready" || return 1
    IFS= read -r app <"$lock/application-pid" && process "$app" || return 1
    app_birth=$birth; runner=$parent
    [ "$app_birth" -ge "$worker_birth" ] && process "$runner" && [ "$parent" = "$worker" ] || return 1
    [ "$(stat -Lc '%d:%i' "/proc/$app/exe")" = "$(kind_id f "$home/app/bilicomics")" ] || return 1
    printf '%s\000' "$home/app/bilicomics" --kindle --device scribe --data-dir "$home/data" | cmp -s - "/proc/$app/cmdline" || return 1
    for pair in "BILICOMICS_LIFECYCLE_DIR=$lock" "BILICOMICS_LAUNCH_GENERATION=1" "BILICOMICS_LAUNCH_TOKEN=$nonce"; do
        tr '\000' '\n' <"/proc/$app/environ" | grep -Fxq "$pair" || return 1
    done
    framebuffer=no; evdev=no
    for descriptor in "/proc/$app/fd/"*; do
        target=$(readlink "$descriptor" 2>/dev/null) || continue
        case "$target" in
            /dev/fb0) framebuffer=yes;;
            /dev/input/event*) index=${target#/dev/input/event}; case "$index" in ''|*[!0-9]*) :;; *) evdev=yes;; esac;;
        esac
    done
    [ "$framebuffer:$evdev" = yes:yes ] && process "$app" && [ "$birth:$parent" = "$app_birth:$runner" ] &&
        worker_identity || return 1
    ready_record="$worker_record:$runner:$app:$app_birth"
}
arm() {
    native && installed && [ "$(find "$home/data" | wc -l)" -eq 3 ] && ! present "$state/armed" && process "$$" &&
        (set -C; printf '%s\n' "$birth" >"$state/armed") || return 1
    setsid sh "$stage/tools/control.sh" "$run" guard </dev/null >/dev/null 2>&1 &
    remaining=50
    while [ "$remaining" -gt 0 ]; do
        plain "$state/guard.pid" && return 0
        remaining=$((remaining - 1)); usleep 20000
    done
    return 1
}
observe() {
    plain "$state/armed" && ! present "$state/ready" || return 1
    remaining=110
    while [ "$remaining" -gt 0 ]; do
        if ready_identity 2>/dev/null; then
            first=$ready_record
            sleep 1
            if ready_identity 2>/dev/null && [ "$ready_record" = "$first" ]; then
                runtime_snapshot || :
                ready_identity 2>/dev/null && [ "$ready_record" = "$first" ] || return 1
                (set -C; printf '%s\n' "$worker_record" >"$state/worker") &&
                    (set -C; printf '%s\n' "$ready_record" >"$state/ready") && return 0
                return 1
            fi
        fi
        remaining=$((remaining - 1)); usleep 200000
    done
    return 1
}
managed_stop() {
    if ! plain "$state/armed"; then native; return; fi
    if plain "$state/stopped"; then native; return; fi
    remaining=60
    while [ "$remaining" -gt 0 ]; do
        if worker_identity 2>/dev/null; then
            previous=$worker_record
            if ! present "$state/worker"; then (set -C; printf '%s\n' "$worker_record" >"$state/worker") || return 1; fi
            if (set -C; printf '%s\n' "$previous" >"$state/signalled") 2>/dev/null; then
                if ! plain "$state/runtime"; then runtime_snapshot || :; fi
                worker_identity && [ "$worker_record" = "$previous" ] && kill -TERM "$worker" || return 1
            elif ! plain "$state/signalled" || ! printf '%s\n' "$previous" | cmp -s - "$state/signalled"; then
                return 1
            fi
            break
        fi
        if plain "$state/signalled" && native; then break; fi
        remaining=$((remaining - 1)); usleep 200000
    done
    remaining=55
    while [ "$remaining" -gt 0 ]; do
        if native; then
            if plain "$state/worker"; then
                IFS=: read -r old_worker old_birth old_entry old_entry_birth ignored <"$state/worker" || return 1
                if { process "$old_worker" 2>/dev/null && [ "$birth" = "$old_birth" ]; } ||
                   { process "$old_entry" 2>/dev/null && [ "$birth" = "$old_entry_birth" ]; }; then
                    remaining=$((remaining - 1)); sleep 1; continue
                fi
            fi
            : >"$state/stopped"; return 0
        fi
        remaining=$((remaining - 1)); sleep 1
    done
    return 1
}
guard() {
    plain "$state/armed" && ! present "$state/guard.pid" && process "$$" || return 1
    (set -C; printf '%s:%s\n' "$$" "$birth" >"$state/guard.pid") || return 1
    remaining=75
    while [ "$remaining" -gt 0 ]; do
        if plain "$state/stopped"; then : >"$state/guard.done"; return 0; fi
        remaining=$((remaining - 1)); sleep 1
    done
    if managed_stop; then : >"$state/guard.done"; else return 1; fi
}
guard_closed() {
    ! plain "$state/armed" && return 0
    if ! present "$state/guard.pid" && plain "$state/stopped"; then native; return; fi
    remaining=30
    while [ "$remaining" -gt 0 ]; do
        if plain "$state/guard.pid"; then
            IFS=: read -r guard_pid guard_birth <"$state/guard.pid" || return 1
            if ! process "$guard_pid" 2>/dev/null || [ "$birth" != "$guard_birth" ]; then
                plain "$state/stopped" && native && return 0
                return 1
            fi
        fi
        remaining=$((remaining - 1)); usleep 100000
    done
    return 1
}
known_runtime() {
    plain "$state/armed" || return 1
    case "$1" in
        "$home/data/logs"|"$home/data/fonts"|"$home/data/covers"|"$home/data/protocol-assets"|"$home/data/account"|"$home/data/account/private") kind=d;;
        "$home/data/state.sqlite3"|"$home/data/state.sqlite3-shm"|"$home/data/state.sqlite3-wal"|"$home/data/state.sqlite3-journal"|\
        "$home/data/logs/application.log"|"$home/data/logs/launcher.log"|"$home/data/protocol-assets/efae82c96a7eef44bee5.wasm") kind=f;;
        *) return 1;;
    esac
    kind_id "$kind" "$1"
}
runtime_unchanged() {
    plain "$state/runtime" || return 1
    expected=$(awk -v path="$1" '$3==path {print $1 " " $2; found++} END {exit found!=1}' "$state/runtime") &&
        identity=$(known_runtime "$1") && [ "$expected" = "$2 $identity" ]
}
runtime_snapshot() {
    dir "$home/data" && find "$home/data" -print >"$state/transient" || return 1
    : >"$state/runtime.pending" || return 1
    while IFS= read -r path; do
        case "$path" in "$home/data"|"$home/data/device-profile"|"$home/data/device-allowlist") continue;; esac
        valid_relative "${path#"$home"/}" && identity=$(known_runtime "$path") || return 1
        if dir "$path"; then kind=d; else kind=f; fi
        printf '%s %s %s\n' "$kind" "$identity" "$path" >>"$state/runtime.pending" || return 1
    done <"$state/transient"
    mv "$state/runtime.pending" "$state/runtime"
}
collect_rollback() {
    if present "$home"; then
        dir "$home" && find "$home" -depth -print >"$state/remove" || return 1
    else : >"$state/remove"; fi
    if present "$doc"; then printf '%s\n' "$doc" >>"$state/remove" || return 1; fi
    : >"$state/checked" || return 1
    while IFS= read -r path; do
        case "$path" in "$home"|"$doc") :;; "$home"/*) valid_relative "${path#"$home"/}" || return 1;; *) return 1;; esac
        dir "${path%/*}" || return 1
        record=$(awk -v path="$path" '$3==path {print $1 " " $2; found++} END {exit found!=1}' "$state/journal") && {
            kind=${record%% *}; identity=${record#* }
            [ "$(kind_id "$kind" "$path")" = "$identity" ] || return 1
            if [ "$kind" = f ]; then source=$(source_for "$path") && plain "$source" && cmp -s "$source" "$path" || return 1; fi
        } || {
            if dir "$path"; then kind=d; else kind=f; fi
            runtime_unchanged "$path" "$kind" && identity=$(kind_id "$kind" "$path") || return 1
        }
        printf '%s %s %s\n' "$kind" "$identity" "$path" >>"$state/checked" || return 1
    done <"$state/remove"
}
rollback() {
    ! present "$state/committed" && managed_stop && guard_closed && native && extra_absent || return 1
    if ! present "$home" && ! present "$doc"; then
        if plain "$state/journal" && grep -Fq " $doc" "$state/journal"; then scan && await_rows 0:0 || return 1; fi
        [ "$(rows)" = 0:0 ] || return 1
        if plain "$state/before"; then unchanged; fi
        return
    fi
    unchanged && plain "$state/journal" && collect_rollback || return 1
    while read -r kind identity path; do
        dir "${path%/*}" && [ "$(kind_id "$kind" "$path")" = "$identity" ] || return 1
        case "$kind" in f) rm "$path" || return 1;; d) rmdir "$path" || return 1;; *) return 1;; esac
    done <"$state/checked"
    if grep -Fq " $doc" "$state/journal"; then scan && await_rows 0:0 || return 1; fi
    ! present "$home" && ! present "$doc" && [ "$(rows)" = 0:0 ] && extra_absent && unchanged
}
verify() {
    native && installed && plain "$state/ready" && plain "$state/signalled" && plain "$state/stopped" && guard_closed
}
retire_guard() {
    for leaf in guard.done guard.pid runtime runtime.pending transient; do
        if present "$state/$leaf"; then plain "$state/$leaf" && rm "$state/$leaf" || return 1; fi
    done
}
case "$op" in
    native) native;;
    document-view)
        native && extra_absent && [ "$spelled" = lower ] && [ "$(rows)" = 1:1 ] && plain "$state/journal" &&
            awk -v file="$doc" -v identity="$lower" '$3==file {found++; if ($1!="f" || $2!=identity || NF!=3) bad=1} END {exit found!=1 || bad}' "$state/journal" || exit 1
        printf 'doc_lower_id=%s:1\ndoc_upper_id=%s\ndoc_alias_mode=%s\n' "$lower" "$(if [ "$upper" = absent ]; then printf absent; else printf '%s:1' "$upper"; fi)" "$alias_mode";;
    install) install;;
    arm) arm;;
    guard) guard;;
    observe) observe;;
    stop) managed_stop;;
    verify) verify;;
    commit)
        if plain "$state/committed"; then native && installed; else verify && : >"$state/committed"; fi
        retire_guard;;
    rollback) rollback && retire_guard;;
    *) exit 2;;
esac
