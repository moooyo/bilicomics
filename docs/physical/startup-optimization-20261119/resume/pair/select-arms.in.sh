#!/bin/sh
# Research-only: called by the owner-bound controller outside the timed interval.
set -eu
export LC_ALL=C
run='@RUN@'
home=/mnt/us/bilicomics
stage=/mnt/us/bilicomics-$run
runtime=/var/tmp/bilicomics-$run
prodlock=/tmp/bilicomics-launcher.lock
tool=$stage/tools/pair-fs
selector=$stage/tools/select-arms.sh
fail() { printf '%s\n' "pair selector: $1" >&2; exit 1; }
present() { [ -e "$1" ] || [ -L "$1" ]; }
dir() { [ -d "$1" ] && [ ! -L "$1" ] && [ "$(readlink -f "$1")" = "$1" ]; }
file() {
    [ -f "$1" ] && [ ! -L "$1" ] && [ "$(readlink -f "$1")" = "$1" ] || return 1
    metadata=$(stat -c '%u:%h:%s' "$1") || return 1
    case "$metadata" in 0:1:*) count=${metadata#0:1:};; *) return 1;; esac
    case "$count" in ''|*[!0-9]*) return 1;; esac
    [ "$count" -gt 0 ] && [ "$count" -le "$2" ]
}
marker() { file "$1" 128 && printf '%s\n' "$2" | cmp -s - "$1"; }
context() {
    dir "$stage" && dir "$runtime" && dir "$stage/tools" &&
        [ "$(stat -c '%u:%a' "$runtime")" = '0:700' ] &&
        marker "$stage/owner" "$run" && marker "$runtime/owner" "$run";
}
no_user() {
    ! present "$prodlock" && [ "$(lipc-get-prop com.lab126.appmgrd activeApp)" = com.lab126.KPPMainApp ];
}
case "$run" in lib-p-????????) ;; *) fail 'run binding';; esac
case "${run#lib-p-}" in *[!0-9a-f]*) fail 'run characters';; esac
context || fail 'owned staging or private runtime'
case "${1:-}:${2:-}:${3:-}:$#" in
    A:select::2|B:select::2|A:restore::2|A:verify::2|B:verify::2|A:probe::2)
        wanted=$1; mode=$2
        if [ "$mode" != verify ]; then
            file "$tool" 4194304 && [ -x "$tool" ] && file "$selector" 65536 || fail 'guarded selector input'
            exec "$tool" lock "$runtime/owner" "$selector" "$wanted" "$mode"
        fi;;
    locked:A:select:3|locked:B:select:3|locked:A:restore:3|locked:A:probe:3)
        wanted=$2; mode=$3
        "$tool" held "$runtime/owner" || fail 'no exact exclusive selector lock';;
    *) fail 'require A/B select, A restore, A/B verify or A probe';;
esac
no_user || fail 'native foreground or launcher lock'

hash() {
    sumline=$(sha256sum "$1") || return 1
    printf '%s\n' "${sumline%% *}"
}
bundle() {
    # Exact rosters also reject FIFOs, nested directories, links and hidden additions.
    br=$1; arm=$2
    dir "$br" && dir "$br/app" && marker "$br/.native-library-owner" "$run" &&
        marker "$br/.bench-arm" "$arm" || return 1
    for path in "$br"/* "$br"/.[!.]* "$br"/..?*; do
        present "$path" || continue
        case "${path##*/}" in launch.sh|app|.native-library-owner|.bench-arm) ;; *) return 1;; esac
    done
    for path in "$br/app"/* "$br/app"/.[!.]* "$br/app"/..?*; do
        present "$path" || continue
        case "${path##*/}" in bilicomics|kindle-jpeg-worker) ;; *) return 1;; esac
    done
    file "$br/launch.sh" 2097152 && file "$br/app/bilicomics" 134217728 &&
        file "$br/app/kindle-jpeg-worker" 134217728 && [ -x "$br/launch.sh" ] &&
        [ -x "$br/app/bilicomics" ] && [ -x "$br/app/kindle-jpeg-worker" ] || return 1
    l=$(hash "$br/launch.sh") && a=$(hash "$br/app/bilicomics") && w=$(hash "$br/app/kindle-jpeg-worker") || return 1
    case "$arm" in A) expected='@A_L@ @A_APP@ @A_W@';; B) expected='@B_L@ @B_APP@ @B_W@';; *) return 1;; esac
    [ "$l $a $w" = "$expected" ] || return 1
    printf '%s %s %s %s\n' "$arm" "$l" "$a" "$w"
}
for path in "$stage"/park-*; do
    present "$path" || continue
    case "${path##*/}" in park-A|park-B) ;; *) fail 'unknown parked directory';; esac
done
pa=$stage/park-A; pb=$stage/park-B
if present "$home"; then
    dir "$home" || fail 'canonical root'
    if marker "$home/.bench-arm" A; then
        active=A; aroot=$home; broot=$pb; ! present "$pa" || fail 'duplicate A'
    elif marker "$home/.bench-arm" B; then
        active=B; aroot=$pa; broot=$home; ! present "$pb" || fail 'duplicate B'
    else fail 'unknown canonical arm'; fi
else
    [ "$mode" = restore ] || fail 'canonical root absent outside recovery'
    active=neither; aroot=$pa; broot=$pb
fi
result_a=$(bundle "$aroot" A) && result_b=$(bundle "$broot" B) || fail 'room, arm roster or program hashes'
no_user || fail 'native window changed during integrity checks'
case "$mode" in
    probe)
        [ "$active" = A ] || fail 'initial probe must preserve A'
        "$tool" probe "$runtime/owner" "$stage" || fail 'safe rename not supported on stage volume';;
    verify) [ "$active" = "$wanted" ] || fail 'different arm is active';;
    select|restore)
        if [ "$wanted" != "$active" ]; then
            if [ "$active" != neither ]; then
                "$tool" move "$runtime/owner" "$home" "$stage/park-$active" || fail 'cannot park active arm'
            fi
            "$tool" move "$runtime/owner" "$stage/park-$wanted" "$home" || fail 'cannot restore desired arm'
        fi;;
esac
! present "$stage/park-$wanted" && marker "$home/.bench-arm" "$wanted" && marker "$home/.native-library-owner" "$run" || fail 'postcondition'
final=$(bundle "$home" "$wanted") || fail 'canonical bytes after selection'
case "$wanted" in A) [ "$final" = "$result_a" ];; B) [ "$final" = "$result_b" ];; esac || fail 'selected bytes differ'
printf '%s\n' "$final"
