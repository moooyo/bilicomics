#!/bin/sh
set -efu
umask 077
export LC_ALL=C

fail() { printf '%s\n' "$1" >&2; exit 1; }
present() { [ -e "$1" ] || [ -L "$1" ]; }
directory() { [ -d "$1" ] && [ ! -L "$1" ] && [ "$(readlink -f "$1")" = "$1" ] && [ "$(stat -c %u "$1")" = 0 ]; }
plain() { [ -f "$1" ] && [ ! -L "$1" ] && [ "$(stat -c '%u:%h' "$1")" = 0:1 ]; }
number() { case "$1" in ''|0*|*[!0-9]*) return 1;; esac; [ "${#1}" -le 20 ]; }

[ "$#" -eq 2 ] || fail 'Expected a run and a guard operation.'
run=$1
op=$2
case "$run" in bili-shelf-*) digits=${run#bili-shelf-};; *) exit 2;; esac
case "$digits" in ''|*[!0-9a-f]*) exit 2;; esac
[ "${#digits}" -eq 32 ] || exit 2
case "$op" in native|wait|stop) :;; *) exit 2;; esac
stage=/mnt/us/$run
tmp=/tmp
alias=/var/tmp
proc=/proc
lock=$tmp/bilicomics-launcher.lock

device() {
    [ "$(id -u)" = 0 ] && [ -c /dev/fb0 ] &&
        [ "$(sed -n 's/^Kindle[[:space:]]*\([0-9][0-9.]*\).*/\1/p' /etc/prettyversion.txt | head -n 1)" = 5.19.5 ] &&
        [ "$(cat /sys/class/graphics/fb0/bits_per_pixel)" = 8 ] &&
        [ "$(cat /sys/class/graphics/fb0/virtual_size)" = 1872,4960 ] || return 1
    case "$(readlink -f "$tmp")" in "$tmp") directory "$tmp";; "$alias") [ -L "$tmp" ] && [ "$(stat -c %u "$tmp")" = 0 ] && directory "$alias";; *) return 1;; esac
}

native_idle() {
    for application in bilicomics aidoku weread; do ! present "$tmp/$application-launcher.lock" || return 1; done
    [ "$(lipc-get-prop com.lab126.appmgrd activeApp)" = com.lab126.KPPMainApp ] &&
        [ "$(lipc-get-prop -s com.lab126.powerd state)" = active ] || return 1
    case "$(status statusbar)" in *start/running*) :;; *) return 1;; esac
    managers=$(pidof awesome) && [ -n "$managers" ] || return 1
    for manager in $managers; do
        number "$manager" || return 1
        case "$(sed -n 's/^State:[[:space:]]*\([A-Z]\).*/\1/p' "$proc/$manager/status")" in R|S) :;; *) return 1;; esac
    done
    for application in bilicomics aidoku weread; do ! present "$tmp/$application-launcher.lock" || return 1; done
}

process() {
    number "$1" && IFS= read -r line <"$proc/$1/stat" && [ "${line%% *}" = "$1" ] || return 1
    tail=${line##*) }; [ "$tail" != "$line" ] || return 1
    set -- $tail
    [ "$#" -ge 20 ] || return 1
    state=$1; parent=$2; group=$3; session=$4; shift 19; birth=$1
    number "$birth"
}

stage_bound() {
    directory "$stage" && plain "$stage/run-id" && printf '%s\n' "$run" | cmp -s - "$stage/run-id"
}

picked() {
    directory "$stage/picked" && plain "$stage/picked/entry" && [ "$(stat -c %s "$stage/picked/entry")" -le 96 ] &&
        IFS=' ' read -r label entry entry_birth extra <"$stage/picked/entry" &&
        [ "$label" = "$run" ] && [ -z "$extra" ] && number "$entry" && number "$entry_birth" &&
        printf '%s %s %s\n' "$run" "$entry" "$entry_birth" | cmp -s - "$stage/picked/entry"
}

launcher_argv() {
    [ "$(readlink -f "$proc/$1/exe")" = "$(readlink -f /bin/sh)" ] || return 1
    for name in sh /bin/sh; do
        printf '%s\000' "$name" "$stage/launch.sh" "$2" | cmp -s - "$proc/$1/cmdline" && return 0
    done
    case "$(readlink -f /bin/sh)" in */busybox)
        for name in busybox "$(readlink -f /bin/sh)"; do
            printf '%s\000' "$name" sh "$stage/launch.sh" "$2" | cmp -s - "$proc/$1/cmdline" && return 0
        done;;
    esac
    return 1
}

lock_id() (
    actual=$(readlink -f "$tmp") || exit 1
    case "$actual" in "$tmp"|"$alias") :;; *) exit 1;; esac
    canonical=$actual/bilicomics-launcher.lock
    [ ! -L "$lock" ] && directory "$canonical" && [ "$(readlink -f "$lock")" = "$canonical" ] || exit 1
    value=$(stat -c '%u:%a:%d:%i' "$canonical") && [ "$value" = "$(stat -c '%u:%a:%d:%i' "$lock")" ] || exit 1
    case "$value" in 0:700:*|0:2700:*) printf '%s\n' "$value";; *) exit 1;; esac
)

worker_proof() {
    for application in aidoku weread; do ! present "$tmp/$application-launcher.lock" || return 1; done
    stage_bound && picked && process "$entry" && [ "$birth" = "$entry_birth" ] && launcher_argv "$entry" --library || return 1
    case "$state" in Z|X) return 1;; esac
    first_lock=$(lock_id) && plain "$lock/pid" && plain "$lock/nonce" && plain "$lock/generation" &&
        IFS= read -r worker <"$lock/pid" && number "$worker" && printf '%s\n' "$worker" | cmp -s - "$lock/pid" &&
        IFS= read -r nonce <"$lock/nonce" && [ "${#nonce}" -eq 32 ] || return 1
    case "$nonce" in *[!0-9a-f]*) return 1;; esac
    printf '%s\n' "$nonce" | cmp -s - "$lock/nonce" && printf '1:%s\n' "$nonce" | cmp -s - "$lock/generation" &&
        process "$worker" && [ "$parent:$group:$session" = "$entry:$worker:$worker" ] && [ "$birth" -ge "$entry_birth" ] &&
        launcher_argv "$worker" --detached && [ "$(lock_id)" = "$first_lock" ] || return 1
    case "$state" in Z|X) return 1;; esac
    proof=$entry:$entry_birth:$worker:$birth:$nonce:$first_lock
}

app_proof() {
    worker_proof && baseline=$proof && plain "$lock/application-pid" && IFS= read -r app <"$lock/application-pid" &&
        number "$app" && printf '%s\n' "$app" | cmp -s - "$lock/application-pid" &&
        directory "$stage/app" && plain "$stage/app/bilicomics" || return 1
    supervisor_birth=$birth
    process "$app" || return 1
    case "$state" in Z|X) return 1;; esac
    app_birth=$birth; runner=$parent
    [ "$(readlink "$proc/$app/exe")" = "$stage/app/bilicomics" ] &&
        [ "$(stat -Lc '%d:%i' "$proc/$app/exe")" = "$(stat -c '%d:%i' "$stage/app/bilicomics")" ] &&
        printf '%s\000' "$stage/app/bilicomics" --kindle --device scribe --data-dir "$stage/data" --demo --offline \
            --shelf-render-trace "$stage/data/$run.jsonl" --exit-after-ms 2500 | cmp -s - "$proc/$app/cmdline" || return 1
    for pair in "BILICOMICS_HOME=$stage" "BILICOMICS_LIFECYCLE_DIR=$lock" BILICOMICS_LAUNCH_GENERATION=1 "BILICOMICS_LAUNCH_TOKEN=$nonce"; do
        tr '\000' '\n' <"$proc/$app/environ" | grep -Fxq "$pair" || return 1
    done
    process "$runner" && [ "$parent" = "$worker" ] && [ "$birth" -ge "$supervisor_birth" ] && [ "$birth" -le "$app_birth" ] || return 1
    case "$state" in Z|X) return 1;; esac
    runner_birth=$birth
    worker_proof && [ "$proof" = "$baseline" ] && process "$app" && [ "$birth:$parent" = "$app_birth:$runner" ] &&
        process "$runner" && [ "$birth:$parent" = "$runner_birth:$worker" ] &&
        printf '%s\n' "$app" | cmp -s - "$lock/application-pid"
}

entry_gone() {
    if ! present "$proc/$entry"; then return 0; fi
    process "$entry" && [ "$birth" != "$entry_birth" ]
}

no_run_processes() (
    set +f
    for item in "$proc"/[0-9]*; do
        [ -d "$item" ] || continue
        if exe=$(readlink "$item/exe" 2>/dev/null); then
            case "$exe" in "$stage/"*) return 1;; esac
            case "$exe" in */busybox|*/sh|*/dash|*/ash)
                [ -r "$item/cmdline" ] || { [ ! -d "$item" ] && continue; return 1; }
                if tr '\000' '\n' 2>/dev/null <"$item/cmdline" | grep -Fxq "$stage/launch.sh"; then return 1; fi;;
            esac
        elif [ -d "$item" ]; then
            size=$(wc -c 2>/dev/null <"$item/cmdline") || { [ ! -d "$item" ] && continue; return 1; }
            [ "$size" -eq 0 ] || return 1
        fi
    done
)

device || fail 'Unexpected device or lock namespace.'
IFS= read -r boot <"$proc/sys/kernel/random/boot_id" &&
    printf '%s\n' "$boot" | grep -Eq '^[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}$' || fail 'No device boot identity.'
if [ "$op" = native ]; then
    native_idle || fail 'Native control is not established.'
    printf '%s\n' "$boot"; exit 0
fi
stage_bound || fail 'No run-owned staging identity.'
remaining=24; signalled=no; expected=; witnessed=
while [ "$remaining" -gt 0 ]; do
    if picked; then
        stamp=$entry:$entry_birth
        if [ -z "$expected" ]; then expected=$stamp; else [ "$expected" = "$stamp" ] || fail 'The picked entry changed.'; fi
        if [ "$op" = wait ] && [ -f "$lock/application-pid" ]; then
            case "$witnessed" in
                '') if app_proof 2>/dev/null; then witnessed=$app:$app_birth; fi;;
                ambiguous) :;;
                *) if IFS= read -r current <"$lock/application-pid" && process "$current" && [ "$current:$birth" != "$witnessed" ]; then witnessed=ambiguous; fi;;
            esac
        fi
        if entry_gone && native_idle && no_run_processes && picked && [ "$entry:$entry_birth" = "$expected" ] && entry_gone && native_idle; then
            case "$witnessed" in ''|ambiguous) :;; *) printf 'app_pid=%s\n' "${witnessed%%:*}";; esac
            exit 0
        fi
        if [ "$op:$signalled" = stop:no ] && worker_proof; then
            last=$proof
            if worker_proof && [ "$proof" = "$last" ] && [ "$entry:$entry_birth" = "$expected" ]; then
                signalled=yes; kill -TERM "$worker" 2>/dev/null || :
            fi
        fi
    elif [ -n "$expected" ]; then fail 'Lost the picked entry.'; fi
    remaining=$((remaining - 1)); [ "$remaining" -eq 0 ] || sleep 1
done
fail 'Retirement of the run and native restoration have not been established.'
