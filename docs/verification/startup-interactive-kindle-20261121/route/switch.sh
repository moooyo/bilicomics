#!/bin/sh
set -eu
umask 077
export LC_ALL=C

fail() { printf 'route: %s\n' "$*" >&2; exit 1; }
present() { [ -e "$1" ] || [ -L "$1" ]; }
directory() { [ ! -L "$1" ] && [ -d "$1" ] && [ "$(readlink -f "$1")" = "$1" ]; }
plain() { [ ! -L "$1" ] && [ -f "$1" ] && [ "$(stat -c '%u:%h' "$1")" = 0:1 ]; }
identity() { plain "$1" && stat -c '%d:%i:%f:%s:%h:%Y' "$1"; }
dir_identity() { directory "$1" && stat -c '%d:%i' "$1"; }
equal() { plain "$1" && plain "$2" && cmp -s "$1" "$2"; }

case "$#" in 2|3) :;; *) fail 'usage: switch.sh RUN inspect|activate|restore|cleanup';; esac
run=$1
operation=$2
case "$run" in bili-shelf-*) digits=${run#bili-shelf-};; *) fail 'invalid run';; esac
case "$digits" in ''|*[!0-9a-f]*) fail 'invalid run';; esac
[ "${#digits}" -eq 32 ] || fail 'invalid run'
case "$operation" in inspect|activate|restore|cleanup) :;; *) fail 'invalid operation';; esac

media=/mnt/us
stage=$media/$run
home=$media/bilicomics
doc=$media/documents/bilicomics.sh
live=$home/launch.sh
retained=$stage/retained-launch.sh
ready=$stage/entry-ready.sh
shim=$stage/entry.sh
shipping=$stage/shipping-launcher.sh
journal=$stage/prepared
pending=$stage/prepared.pending
armed=$stage/armed
picked=$stage/picked
tool=$stage/route-fs

directory "$media" && directory "$stage" && directory "$home" && [ "$0" = "$stage/switch.sh" ] &&
    plain "$stage/switch.sh" && plain "$stage/run-id" && plain "$tool" &&
    printf '%s\n' "$run" | cmp -s - "$stage/run-id" || fail 'unrecognized staging directory'
if [ "$#" -eq 2 ]; then exec "$tool" lock "$run" "$operation"; fi
[ "$3" = locked ] && "$tool" held "$run" || fail 'missing operation lock'

native_idle() {
    [ "$(lipc-get-prop com.lab126.appmgrd activeApp)" = com.lab126.KPPMainApp ] &&
        [ "$(lipc-get-prop -s com.lab126.powerd state)" = active ] || return 1
    case "$(status statusbar)" in *start/running*) :;; *) return 1;; esac
    case "$(readlink -f /tmp)" in /tmp|/var/tmp) :;; *) return 1;; esac
    for application in bilicomics aidoku weread; do
        ! present "/tmp/$application-launcher.lock" || return 1
    done
    managers=$(pidof awesome) && [ -n "$managers" ] || return 1
    for manager in $managers; do
        case "$manager" in ''|*[!0-9]*) return 1;; esac
        case "$(sed -n 's/^State:[[:space:]]*\([A-Z]\).*/\1/p' "/proc/$manager/status")" in R|S) :;; *) return 1;; esac
    done
}

document_identity() { plain "$doc" && stat -c '%d:%i:%f:%s:%h' "$doc"; }

document_verified() {
    directory "$media/documents" && original_document=$(document_identity) &&
        equal "$doc" "$stage/document-launcher.sh" || return 1
    seen=0
    for path in "$media"/documents/[Bb][Ii][Ll][Ii][Cc][Oo][Mm][Ii][Cc][Ss].*; do
        present "$path" || continue
        case "${path##*/}" in
            [Bb][Ii][Ll][Ii][Cc][Oo][Mm][Ii][Cc][Ss].[Ss][Hh])
                [ "$path" = "$doc" ] || return 1
                seen=$((seen + 1));;
            [Bb][Ii][Ll][Ii][Cc][Oo][Mm][Ii][Cc][Ss].[Ss][Dd][Rr]|[Bb][Ii][Ll][Ii][Cc][Oo][Mm][Ii][Cc][Ss].[Ss][Hh].[Ss][Dd][Rr])
                return 1;;
        esac
    done
    [ "$seen" -eq 1 ] || return 1
    rows=$(sqlite3 -readonly -cmd '.timeout 1500' /var/local/cc.db "SELECT count(*)||':'||coalesce(sum(p_location COLLATE BINARY='$doc' AND p_mimeType='text/x-shellscript' AND p_titles_0_nominal='BiliComics for Kindle'),0) FROM Entries WHERE p_location COLLATE NOCASE='$doc';") || return 1
    [ "$rows" = 1:1 ] && [ "$(document_identity)" = "$original_document" ] || return 1
    printf '%s\n' "$original_document"
}

production_snapshot() (
    directory "$home/app" && directory "$home/data" && card_identity=$(document_verified) || exit 1
    [ "$(sed -n 's/^Kindle[[:space:]]*\([0-9][0-9.]*\).*/\1/p' /etc/prettyversion.txt | head -n 1)" = 5.19.5 ] &&
        [ "$(cat /sys/class/graphics/fb0/bits_per_pixel)" = 8 ] &&
        [ "$(cat /sys/class/graphics/fb0/virtual_size)" = 1872,4960 ] &&
        [ -c /dev/fb0 ] || exit 1
    for root in "$home/data" "$stage/data"; do
        directory "$root" && plain "$root/device-profile" && plain "$root/device-allowlist" &&
            printf 'scribe\n' | cmp -s - "$root/device-profile" &&
            printf 'scribe 5.19.5\n' | cmp -s - "$root/device-allowlist" || exit 1
    done
    [ "$(stat -c %s "$home/app/bilicomics")" = 74340832 ] &&
        [ "$(stat -c %s "$home/app/kindle-jpeg-worker")" = 426672 ] || exit 1
    IFS= read -r boot < /proc/sys/kernel/random/boot_id || exit 1
    case "$boot" in ''|*[!0-9a-f-]*) exit 1;; esac
    [ "${#boot}" -eq 36 ] || exit 1
    printf '%s\n' "$run" "$boot"
    for path in "$stage" "$home" "$home/app" "$home/data" "$media/documents"; do dir_identity "$path" || exit 1; done
    printf '%s\n' "$card_identity"
    for path in "$home/app/bilicomics" "$home/app/kindle-jpeg-worker" "$home/data/device-profile" \
        "$home/data/device-allowlist" "$shim" "$shipping" "$stage/launch.sh" "$stage/app/bilicomics" \
        "$stage/app/kindle-jpeg-worker" "$stage/data/device-profile" "$stage/data/device-allowlist" \
        "$stage/run-id" "$stage/cleanup-assets" "$stage/switch.sh" "$tool"; do
        identity "$path" || exit 1
    done
)

record() {
    printf '%s\n%s\n%s\n' "$protected" "$stock_identity" "$entry_identity"
}

inspect_state() {
    stock_identity= entry_identity= state=
    if equal "$live" "$shipping" && ! present "$retained" && equal "$ready" "$shim"; then
        stock_identity=$(identity "$live") && entry_identity=$(identity "$ready") || return 1
        state=stock
    elif ! present "$live" && equal "$retained" "$shipping" && equal "$ready" "$shim"; then
        stock_identity=$(identity "$retained") && entry_identity=$(identity "$ready") || return 1
        state=parked
    elif equal "$live" "$shim" && equal "$retained" "$shipping" && ! present "$ready"; then
        stock_identity=$(identity "$retained") && entry_identity=$(identity "$live") || return 1
        state=unarmed
        if present "$armed"; then state=armed; fi
        if present "$picked"; then state=used; fi
    else
        return 1
    fi
    if present "$armed"; then
        directory "$armed" && { [ "$state" = armed ] || [ "$state" = used ]; } || return 1
    fi
    if present "$picked"; then directory "$picked" || return 1; fi
    if present "$pending"; then
        plain "$pending" && ! present "$journal" && [ "$state" = stock ] && ! present "$picked" || return 1
    fi
    if present "$journal"; then
        plain "$journal" && record | cmp -s - "$journal" || return 1
    else
        [ "$state" = stock ] && ! present "$picked" || return 1
    fi
}

require_entry_gone() (
    if ! present "$picked/entry"; then return 0; fi
    plain "$picked/entry" || exit 1
    IFS=' ' read -r entry_run pid birth extra < "$picked/entry" || exit 1
    [ "$entry_run" = "$run" ] && [ -z "$extra" ] &&
        printf '%s %s %s\n' "$run" "$pid" "$birth" | cmp -s - "$picked/entry" || exit 1
    case "$pid" in ''|0*|*[!0-9]*) exit 1;; esac
    case "$birth" in ''|*[!0-9]*) exit 1;; esac
    [ -d "/proc/$pid" ] || return 0
    IFS= read -r process_stat < "/proc/$pid/stat" || exit 1
    [ "${process_stat%% *}" = "$pid" ] || exit 1
    set -- ${process_stat##*) }
    [ "$#" -ge 20 ] || exit 1
    shift 19
    [ "$1" != "$birth" ]
)

move() {
    "$tool" move "$run" "$1" "$2" && "$tool" persist "$run" || fail 'transition interrupted; inspect and restore only'
}

probe_volume() {
    probe=$stage/rename-probe
    mkdir "$probe" && mkdir "$probe/left" "$probe/right" || return 1
    (set -C; printf '%s\n' "$run" > "$probe/left/dummy") || return 1
    original=$(identity "$probe/left/dummy") && "$tool" probe "$run" "$original" &&
        [ "$(identity "$probe/left/dummy")" = "$original" ] &&
        printf '%s\n' "$run" | cmp -s - "$probe/left/dummy" || return 1
    rm "$probe/left/dummy" && rmdir "$probe/left" "$probe/right" "$probe"
}

launcher_verified() {
    binding=' --kindle --device "$PROFILE" --data-dir "$DATA_DIR" >&6'
    order_option=''
    [ "$(grep -F -c -e "$binding" "$shipping")" = 1 ] || return 1
    awk -v run="$run" -v binding="$binding" -v order_option="$order_option" '
        {
            place = index($0, binding)
            if (place != 0) {
                printf "%s --demo --offline --shelf-render-trace \"$DATA_DIR/%s.jsonl\" --exit-after-ms 2500%s >&6%s\n",
                    substr($0, 1, place + length(binding) - 5), run, order_option, substr($0, place + length(binding))
            } else print
        }
    ' "$shipping" | cmp -s - "$stage/launch.sh"
}

clean_stage() {
    plain "$stage/cleanup-assets" || fail 'missing cleanup roster'
    while IFS= read -r asset; do
        case "$asset" in
            data/demo/fixtures/covers/*.png|data/demo/fixtures/pages/*.png|data/demo/fixtures/*.png|data/demo/fixtures/*.json) :;;
            *) fail 'invalid cleanup roster';;
        esac
        case "$asset" in *[!a-z0-9./-]*|*..*|*//*|*/./*) fail 'invalid cleanup roster';; esac
    done < "$stage/cleanup-assets"
    clean_tree "$stage" verify
    clean_tree "$stage" prune
    for relative in app/bilicomics app/kindle-jpeg-worker data/device-profile data/device-allowlist \
        prepared prepared.pending entry.sh entry-ready.sh shipping-launcher.sh document-launcher.sh launch.sh \
        cleanup-assets switch.sh route-fs; do
        path=$stage/$relative
        present "$path" || continue
        plain "$path" && rm "$path" || fail "cannot remove: $relative"
    done
    rmdir "$stage/app" "$stage/data" || fail 'staging application or data path is no longer empty'
    plain "$stage/run-id" && printf '%s\n' "$run" | cmp -s - "$stage/run-id" &&
        rm "$stage/run-id" || fail 'cannot remove run marker'
    rmdir "$stage" || fail 'staging directory is no longer empty'
}

clean_tree() (
    for path in "$1"/* "$1"/.[!.]* "$1"/..?*; do
        present "$path" || continue
        relative=${path#"$stage"/}
        case "$relative" in
            app|picked|data|data/fonts|data/logs|data/demo|data/demo/fixtures|data/demo/fixtures/covers|data/demo/fixtures/pages|\
            rename-probe|rename-probe/left|rename-probe/right)
                directory "$path" || fail "unsafe staged directory: $relative"
                clean_tree "$path" "$2"
                if [ "$2" = prune ] && [ "$relative" != app ] && [ "$relative" != data ]; then
                    rmdir "$path" || fail "staged directory changed: $relative"
                fi
                continue;;
            switch.sh|route-fs|run-id|entry.sh|entry-ready.sh|shipping-launcher.sh|document-launcher.sh|launch.sh|cleanup-assets|\
            prepared|prepared.pending|app/bilicomics|app/kindle-jpeg-worker|data/device-profile|data/device-allowlist)
                plain "$path" || fail "unsafe staged control file: $relative"
                continue;;
            picked/entry|\
            "data/$run.jsonl"|data/logs/launcher.log|data/logs/launcher.log.1|data/logs/application.log|data/logs/application.log.1|\
            data/demo/state.sqlite3|data/demo/state.sqlite3-shm|data/demo/state.sqlite3-wal|data/demo/state.sqlite3-journal) :;;
            rename-probe/left/dummy|rename-probe/right/dummy)
                plain "$path" && printf '%s\n' "$run" | cmp -s - "$path" || fail 'unrecognized volume probe';;
            data/demo/fixtures/.demo-*.pending|data/demo/fixtures/covers/.demo-*.pending|data/demo/fixtures/pages/.demo-*.pending)
                plain "$path" && plain "$picked/entry" &&
                    printf '%s\n' "${path##*/}" | grep -Eq '^\.demo-[1-9][0-9]{0,9}-[0-9]{1,39}-[0-9]{1,20}\.pending$' &&
                    [ "$(stat -c %s "$path")" -le 33554432 ] &&
                    [ "$(stat -c %Y "$path")" -ge "$(stat -c %Y "$picked/entry")" ] || fail 'unknown demo staging file';;
            data/demo/fixtures/*)
                grep -Fxq "$relative" "$stage/cleanup-assets" || fail 'unrecognized bundled demo leaf';;
            *) fail 'unknown staged contents; no bulk removal';;
        esac
        plain "$path" || fail "unsafe staged file: $relative"
        if [ "$2" = prune ]; then rm "$path" || fail "cannot remove: $relative"; fi
    done
)

directory "$stage/app" && plain "$shim" && plain "$shipping" && plain "$stage/launch.sh" &&
    ! cmp -s "$shipping" "$shim" && ! cmp -s "$shipping" "$stage/launch.sh" || fail 'invalid diagnostic launcher inputs'
protected=$(production_snapshot) || fail 'installed document, application or supported device profile differs'
inspect_state || fail 'route paths or retained launcher do not match this run'
case "$operation" in
    inspect) :;;
    activate)
        [ "$state" = stock ] && ! present "$journal" && ! present "$pending" && ! present "$picked" &&
            directory "$stage/data/demo" && plain "$stage/data/demo/state.sqlite3" &&
            ! present "$stage/data/$run.jsonl" && [ -x "$stage/app/bilicomics" ] && [ -x "$stage/app/kindle-jpeg-worker" ] &&
            native_idle || fail 'route is not fresh or native control is not idle'
        for path in rename-probe data/fonts data/logs data/demo/fixtures data/demo/state.sqlite3-shm \
            data/demo/state.sqlite3-wal data/demo/state.sqlite3-journal; do
            ! present "$stage/$path" || fail 'diagnostic runtime path is not fresh'
        done
        clean_tree "$stage" verify
        launcher_verified || fail 'staged launcher does not match the single approved argv edit'
        probe_volume || fail 'route volume not qualified; production launcher was not moved'
        (set -C; record > "$pending") || fail 'cannot prepare original identity record'
        move journal "$(identity "$pending")"
        inspect_state && [ "$state" = stock ] && native_idle || fail 'admission changed before parking'
        move park "$stock_identity"
        inspect_state && [ "$state" = parked ] || fail 'parked original differs'
        move publish "$entry_identity"
        inspect_state && [ "$state" = unarmed ] && native_idle || fail 'shim publication changed'
        mkdir "$armed" && "$tool" persist "$run" || fail 'cannot arm the diagnostic route'
        inspect_state && [ "$state" = armed ] || fail 'route is no longer armed for its first click';;
    restore)
        if present "$armed"; then directory "$armed" && rmdir "$armed" || fail 'unsafe arm marker'; fi
        require_entry_gone && native_idle || fail 'diagnostic entry is active or native control is not idle'
        inspect_state || fail 'route changed before restore'
        case "$state" in
            used|unarmed) move unpublish "$entry_identity"; inspect_state && [ "$state" = parked ] || fail 'cannot verify parked state';;
        esac
        if [ "$state" = parked ]; then move return "$stock_identity"; fi
        inspect_state && [ "$state" = stock ] && native_idle && [ "$(production_snapshot)" = "$protected" ] || fail 'production restoration not established';;
    cleanup)
        [ "$state" = stock ] && require_entry_gone && native_idle || fail 'restore production launcher before cleanup'
        trap '' HUP
        clean_stage
        state=removed;;
esac
printf 'run=%s\nroute=%s\n' "$run" "$state"
