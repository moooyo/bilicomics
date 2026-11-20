#!/bin/sh
set -eu
umask 077
export LC_ALL=C
run=$1
op=$2
case "$run" in lib-p-????????) :;; *) exit 2;; esac
case "${run#lib-p-}" in *[!0-9a-f]*) exit 2;; esac
stage=/mnt/us/bilicomics-$run
priv=/var/tmp/bilicomics-$run
home=/mnt/us/bilicomics
doc=/mnt/us/documents/bilicomics.sh
weread_doc=/mnt/us/documents/weread.sh
weread_legacy_doc=/mnt/us/documents/WeRead.sh
lock=/tmp/bilicomics-launcher.lock
present() { [ -e "$1" ] || [ -L "$1" ]; }
plain() { [ -f "$1" ] && [ ! -L "$1" ]; }
dir() { [ -d "$1" ] && [ ! -L "$1" ] && [ "$(readlink -f "$1")" = "$1" ]; }
for root in "$stage" "$priv"; do
    dir "$root" && plain "$root/owner" && [ "$(cat "$root/owner")" = "$run" ] || exit 2
done
native() {
    [ "$(lipc-get-prop com.lab126.appmgrd activeApp)" = com.lab126.KPPMainApp ] &&
        [ "$(lipc-get-prop -s com.lab126.powerd state)" = active ] || return 1
    case "$(status statusbar)" in *start/running*) :;; *) return 1;; esac
    for project in bilicomics aidoku weread; do ! present "/tmp/$project-launcher.lock" || return 1; done
    pids=$(pidof awesome) && [ -n "$pids" ] || return 1
    for process in $pids; do
        case "$(sed -n 's/^State:[[:space:]]*\([A-Z]\).*/\1/p' "/proc/$process/status")" in R|S) :;; *) return 1;; esac
    done
}
protect() {
    registry=$(sqlite3 -readonly -cmd '.timeout 1500' /var/local/appreg.db \
        "SELECT value FROM properties WHERE handlerId='tech.hackerdude.shell_integration.launcher' AND name='command';") || return 1
    case "$registry" in /var/local/kmc/bin/sh_integration_launcher|/var/local/kindle-catalog-entry/v1-*/aidoku-library-dispatch) :;; *) return 1;; esac
    printf 'handler=%s\n' "$registry" || return 1
    for path in "$registry" /var/local/kmc/bin/sh_integration_launcher /mnt/us/documents/Aidoku.sh "$weread_doc" "$weread_legacy_doc" \
        /mnt/us/aidoku/launch.sh /mnt/us/aidoku/app/aidoku /mnt/us/weread/launch.sh /mnt/us/weread/app/weread; do
        if present "$path"; then identity=$(stat -c '%d:%i:%f:%s:%y' "$path") || return 1; else identity=absent; fi
        printf '%s=%s\n' "$path" "$identity" || return 1
    done
    sqlite3 -readonly -cmd '.timeout 1500' /var/local/cc.db \
        "SELECT 'weread-index='||quote(p_location)||','||quote(p_titles_0_nominal)||','||quote(p_mimeType) AS snapshot
         FROM Entries WHERE p_location COLLATE BINARY IN ('$weread_doc','$weread_legacy_doc') ORDER BY snapshot COLLATE BINARY;" || return 1
    for prop in flIntensity currentAmberLevel; do
        value=$(lipc-get-prop com.lab126.powerd "$prop" 2>/dev/null || :)
        case "$value" in ''|*[!0-9]*) value=unavailable;; esac
        printf '%s=%s\n' "$prop" "$value" || return 1
    done
    cat /proc/sys/kernel/random/boot_id
}
protection_unchanged() {
    protect >"$priv/protection.after" || return 1
    cmp -s "$priv/protection.before" "$priv/protection.after"
}
rows() {
    sqlite3 -readonly -cmd '.timeout 1500' /var/local/cc.db \
        "SELECT COUNT(*)||':'||COALESCE(SUM(p_location COLLATE BINARY='/mnt/us/documents/bilicomics.sh' AND p_titles_0_nominal='BiliComics for Kindle' AND p_mimeType='text/x-shellscript'),0) FROM Entries WHERE p_location COLLATE BINARY IN ('/mnt/us/documents/bilicomics.sh','/mnt/us/documents/BiliComics.sh');"
}
await_rows() {
    expected=$1; tries=40
    while [ "$tries" -gt 0 ]; do
        seen=$(rows) && [ "$seen" = "$expected" ] && return 0
        tries=$((tries - 1)); sleep 1
    done
    return 1
}
scan() { lipc-set-prop -i com.lab126.scanner doFullScan 1 >/dev/null; }
own_home() {
    dir "$home" && plain "$home/.native-library-owner" &&
        cmp -s "$home/.native-library-owner" "$priv/owner" && plain "$home/launch.sh" &&
        cmp -s "$home/launch.sh" "$stage/tools/entry.sh" || return 1
    for path in "$home"/* "$home"/.[!.]* "$home"/..?*; do
        present "$path" || continue
        case "${path##*/}" in launch.sh|.native-library-owner) :;; *) return 1;; esac
    done
}
selected_arm() {
    bundle=$stage/park-$1
    dir "$bundle" && dir "$bundle/app" && plain "$bundle/.bench-arm" && plain "$bundle/.native-library-owner" &&
        [ "$(cat "$bundle/.bench-arm")" = "$1" ] && cmp -s "$bundle/.native-library-owner" "$priv/owner" || return 1
    for leaf in launch.sh app/bilicomics app/kindle-jpeg-worker; do
        plain "$bundle/$leaf" && [ -x "$bundle/$leaf" ] && plain "$stage/reference-$1/$leaf" &&
            cmp -s "$bundle/$leaf" "$stage/reference-$1/$leaf" || return 1
    done
}
probe() {
    DISPLAY=:0 OMP_THREAD_LIMIT=1 "$stage/probe/lib/ld-linux-armhf.so.3" --inhibit-cache \
        --library-path "$stage/probe/lib" "$stage/probe/$1" "$2" "$stage/probe/models"
}
check_extra() {
    for path in "$doc.sdr" /mnt/us/documents/bilicomics.sdr /mnt/us/documents/BiliComics.sh \
        /mnt/us/documents/BiliComics.sh.sdr /mnt/us/documents/BiliComics.sdr; do ! present "$path" || return 1; done
}
read_case() {
    name=$1
    case "$name" in [ab]0[0-6]) :;; *) return 1;; esac
    out=$priv/$name
    case "$name" in a*) arm=A;; b*) arm=B;; esac
}
case_pid_alive() {
    plain "$1" || return 1
    read -r proc birth <"$1" || return 1
    case "$proc:$birth" in *[!0-9:]*|:*|*:) return 1;; esac
    [ -r "/proc/$proc/stat" ] || return 1
    line=$(cat "/proc/$proc/stat" 2>/dev/null) || return 1
    fields=${line##*) }; set -- $fields
    case "$1" in Z|X) return 1;; esac
    [ "$#" -ge 20 ] && shift 19 && [ "$1" = "$birth" ]
}
owned_demo() {
    plain "$lock/application-pid" && IFS= read -r app_pid <"$lock/application-pid" || return 1
    case "$app_pid" in ''|*[!0-9]*) return 1;; esac
    [ "$(readlink "/proc/$app_pid/exe")" = "$stage/park-$arm/app/bilicomics" ] &&
        tr '\000' '\n' <"/proc/$app_pid/cmdline" | grep -Fxq -- '--demo' &&
        tr '\000' '\n' <"/proc/$app_pid/cmdline" | grep -Fxq -- '--offline' &&
        tr '\000' '\n' <"/proc/$app_pid/cmdline" | grep -Fxq "$stage/cases/$name/data" &&
        tr '\000' '\n' <"/proc/$app_pid/environ" | grep -Fxq "BILICOMICS_BENCH_RUN=$run"
}
last_frame() {
    tries=175
    while [ "$tries" -gt 0 ]; do
        if plain "$out/lifecycle.jsonl" && plain "$out/performance.jsonl" && owned_demo &&
            grep -F '"event":"ready"' "$out/lifecycle.jsonl" | grep -Fq '"route":"shelf"' &&
            awk '/"kind":"startup","stage":"ui_ready"/ {ready=1}
                 ready && /"kind":"native","stage":"frame"/ && /"complete_frame_replacement":true/ &&
                     /"refresh_accepted":true/ && /"driver_completion_observed":true/ {seen=1}
                 END {exit !seen}' "$out/performance.jsonl"; then break; fi
        tries=$((tries - 1)); usleep 80000
    done
    [ "$tries" -gt 0 ] && owned_demo || return 1
    before=$app_pid
    "$stage/tools/frame-reader" --capture >"$out/frame.pending" || return 1
    owned_demo && [ "$before" = "$app_pid" ] || return 1
    IFS= read -r header <"$out/frame.pending"
    case "$header" in *'"visible_bytes":4612800,'*'"read_only":true}'*) :;; *) return 1;; esac
    [ "$(wc -c <"$out/frame.pending")" -eq $((4612801 + ${#header})) ] || return 1
    printf '%s\n' "$header" >"$out/frame-info.json"
    tail -c 4612800 "$out/frame.pending" | sha256sum | cut -d' ' -f1 >"$out/frame-pixels.txt"
    rm "$out/frame.pending"
}
case "$op" in
    bootstrap)
        native; ! present "$home" && ! present "$doc" && check_extra && [ "$(rows)" = 0:0 ] || exit 1
        ! present "$priv/protection.before" || exit 1
        protect >"$priv/protection.after"; mv "$priv/protection.after" "$priv/protection.before"
        for arm in A B; do
            ref=$stage/reference-$arm; mkdir "$ref" "$ref/app"
            for name in launch.sh app/bilicomics app/kindle-jpeg-worker; do
                plain "$stage/park-$arm/$name" || exit 1
                cp "$stage/park-$arm/$name" "$ref/$name"
            done
        done
        mkdir "$stage/cases"
        selected_arm A; selected_arm B
        mkdir "$home"
        (set -C; cat "$priv/owner" >"$home/.native-library-owner")
        : >"$priv/install-started"
        (set -C; cat "$stage/tools/entry.sh" >"$home/launch.sh"); chmod 755 "$home/launch.sh"
        own_home
        ! present "$doc" && ! present /mnt/us/documents/BiliComics.sh || exit 1
        (set -C; cat "$stage/document" >"$doc")
        : >"$priv/created-document"
        chmod 755 "$doc"
        cmp -s "$doc" "$stage/document"; scan; await_rows 1:1; protection_unchanged
        ;;
    navigation|card)
        native; probe native-target "$op"
        ;;
    nav-home|nav-library)
        native; probe card-input "$op"
        ;;
    pointer)
        native
        DISPLAY=:0 "$stage/probe/lib/ld-linux-armhf.so.3" --inhibit-cache --library-path "$stage/probe/lib" \
            "$stage/probe/card-input" restore-pointer "$3" "$4"
        ;;
    arm)
        read_case "$3"; native; await_rows 1:1; plain "$doc" && cmp -s "$doc" "$stage/document" || exit 1
        protection_unchanged; own_home; selected_arm "$arm"
        mkdir "$out" "$stage/cases/$name"
        cp -R "$stage/seed" "$stage/cases/$name/data"
        printf '%s\n' "$name" >"$priv/active.next"; mv "$priv/active.next" "$priv/active"
        ;;
    guard)
        read_case "$3"; [ "$(cat "$priv/active")" = "$name" ] || exit 1
        tries=75
        while [ "$tries" -gt 0 ]; do
            if plain "$out/entry.status" && ! case_pid_alive "$out/worker.pid"; then break; fi
            if plain "$out/abort" && ! case_pid_alive "$out/entry.pid" && ! case_pid_alive "$out/worker.pid"; then break; fi
            if [ "$tries" -le 15 ] && case_pid_alive "$out/worker.pid" &&
                plain "$lock/pid" && [ "$(cat "$lock/pid")" = "$proc" ] && plain "$lock/nonce" && plain "$lock/generation" &&
                tr '\000' '\n' <"/proc/$proc/environ" | grep -Fxq "BILICOMICS_BENCH_RUN=$run" &&
                tr '\000' '\n' <"/proc/$proc/environ" | grep -Fxq "BILICOMICS_BENCH_CASE=$name" &&
                case_pid_alive "$out/worker.pid"; then
                kill -TERM "$proc" && : >"$out/guard-signalled" || :
                break
            fi
            tries=$((tries - 1)); sleep 1
        done
        : >"$out/guard-done"
        ;;
    click)
        read_case "$3"; native; [ "$(cat "$priv/active")" = "$name" ] && [ "$(rows)" = 1:1 ] || exit 1
        ! present "$out/guard-started" || exit 1
        mkdir "$out/armed"
        : >"$out/guard-started"
        setsid sh "$stage/tools/control.sh" "$run" guard "$name" </dev/null >/dev/null 2>&1 &
        probe card-input open-card
        ;;
    wait)
        read_case "$3"; tries=85
        case "$name" in [ab]06) last_frame; tries=85;; esac
        while [ "$tries" -gt 0 ]; do
            if plain "$out/entry.status" && plain "$out/guard-done" && ! case_pid_alive "$out/entry.pid" &&
                ! case_pid_alive "$out/worker.pid" && native; then break; fi
            tries=$((tries - 1)); sleep 1
        done
        [ "$tries" -gt 0 ] && [ "$(cat "$out/entry.status")" = 0 ] && ! present "$out/guard-signalled" || exit 1
        plain "$out/final.json" && plain "$out/final.png" && plain "$out/performance.jsonl" && plain "$out/lifecycle.jsonl" || exit 1
        protection_unchanged
        ;;
    collect|collect-stages)
        read_case "$3"; cd "$out"
        set -- entry.pid worker.pid entry.status performance.jsonl lifecycle.jsonl final.json final.png guard-done
        if [ "$op" = collect-stages ]; then
            trace_dir=$stage/cases/$name/data/logs
            trace_file=$trace_dir/launcher-timing.tsv
            dir "$trace_dir" && plain "$trace_file" && plain worker.pid || exit 1
            size=$(stat -c %s "$trace_file")
            [ "$size" -gt 0 ] && [ "$size" -le 65536 ] || exit 1
            read -r worker birth extra <worker.pid
            case "$worker" in ''|0*|*[!0-9]*) exit 1;; esac
            case "$birth" in ''|*[!0-9]*) exit 1;; esac
            [ -z "$extra" ] && [ "${#worker}" -le 10 ] && ! present launcher-stages.tsv || exit 1
            set -C; exec 8>launcher-stages.tsv; set +C
            trap 'rm -f launcher-stages.tsv' 0
            awk -F '\t' -v worker="$worker" '
                $2 == "startup_logging_ready" { started = 1 }
                !started || $2 == "awesome_paused" { next }
                {
                    if ($2 !~ /^(startup_logging_ready|startup_sample_begin|startup_sample_ready|native_read_go|native_read_ready|native_read_reaped|native_take_begin|native_take_done)$/ ||
                        NF != 10 || $1 !~ /^[0-9]+\.[0-9][0-9]$/ || length($1) > 15 ||
                        $3 != "pid=" worker || $4 != "phase=none" || $5 != "sleep=0" ||
                        $6 != "capture=0" || $7 != "probe=0" || $8 != "lifecycle=0" ||
                        $9 !~ /^mode=(unknown|Y8|Y8INV)$/ || $10 !~ /^read_sequence=[0-3]$/ || ++count > 14) exit 1
                    print $1 "\t" $2 "\t" worker "\t" substr($10, 15)
                    if ($2 == "native_take_done") { done = 1; exit }
                }
                END { if (!done || count != 14) exit 1 }
            ' "$trace_file" >&8
            exec 8>&-
            set -- "$@" launcher-stages.tsv
        fi
        case "$name" in [ab]06) set -- "$@" frame-info.json frame-pixels.txt;; esac
        tar -cf - "$@"
        ;;
    quiesce)
        if plain "$priv/active"; then
            read_case "$(cat "$priv/active")"; : >"$out/abort"
            tries=90
            while [ "$tries" -gt 0 ]; do
                if { ! present "$out/guard-started" || plain "$out/guard-done"; } && ! case_pid_alive "$out/entry.pid" &&
                    ! case_pid_alive "$out/worker.pid" && native; then exit 0; fi
                tries=$((tries - 1)); sleep 1
            done
            exit 1
        fi
        native
        ;;
    uninstall)
        native
        if plain "$priv/created-document"; then
            if present "$doc"; then plain "$doc" && cmp -s "$doc" "$stage/document" || exit 1; rm "$doc"; fi
            scan; await_rows 0:0
        else ! present "$doc" || exit 1; fi
        check_extra
        if plain "$priv/install-started" && present "$home"; then
            own_home
            rm "$home/launch.sh" "$home/.native-library-owner"; rmdir "$home"
        fi
        ! present "$home" && ! present "$doc" && [ "$(rows)" = 0:0 ] || exit 1
        if plain "$priv/protection.before"; then protection_unchanged; fi
        ;;
    *) exit 2;;
esac
