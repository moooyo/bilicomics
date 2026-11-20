#!/bin/sh
set -eu
export LC_ALL=C
cd /proc
[ "$(id -u)" = 0 ] || exit 2

kind() {
    if [ -L "$1" ]; then printf 'link'
    elif [ -d "$1" ]; then printf 'directory'
    elif [ -f "$1" ]; then printf 'file'
    elif [ -e "$1" ]; then printf 'other'
    else printf 'absent'; fi
}

native() {
    sample=$1
    front=$(lipc-get-prop com.lab126.appmgrd activeApp 2>/dev/null || :)
    case "$front" in com.lab126.KPPMainApp) front=native;; '') front=unknown;; *) front=other;; esac
    power=$(lipc-get-prop -s com.lab126.powerd state 2>/dev/null || :)
    case "$power" in active) :;; '') power=unknown;; *) power=not-active;; esac
    case "$(status statusbar 2>/dev/null || :)" in *start/running*) bar=running;; *) bar=not-confirmed;; esac
    pids=$(pidof awesome 2>/dev/null || :)
    window=running
    [ -n "$pids" ] || window=not-confirmed
    for pid in $pids; do
        case "$pid" in *[!0-9]*) window=not-confirmed; continue;; esac
        case "$(sed -n 's/^State:[[:space:]]*\([A-Z]\).*/\1/p' "/proc/$pid/status" 2>/dev/null || :)" in
            S|R) :;; *) window=not-confirmed;;
        esac
    done
    printf '%s_front=%s\n%s_power=%s\n%s_statusbar=%s\n%s_window_manager=%s\n' \
        "$sample" "$front" "$sample" "$power" "$sample" "$bar" "$sample" "$window"
    for project in bilicomics aidoku weread; do
        printf '%s_lock_%s=%s\n' "$sample" "$project" "$(kind "/tmp/$project-launcher.lock")"
    done
}

native before
firmware=$(sed -n 's/^Kindle[[:space:]]*\([0-9][0-9.]*\).*/\1/p' /etc/prettyversion.txt | head -n 1)
case "$firmware" in ''|*[!0-9.]*) firmware=unknown;; esac
printf 'firmware=%s\n' "$firmware"
for attribute in virtual_size bits_per_pixel stride rotate; do
    value=$(cat "/sys/class/graphics/fb0/$attribute" 2>/dev/null || :)
    case "$value" in ''|*[!0-9,]*) value=unknown;; esac
    printf 'fb_%s=%s\n' "$attribute" "$value"
done
for path in /mnt/us/bilicomics /mnt/us/bilicomics/app/bilicomics /mnt/us/bilicomics/app/kindle-jpeg-worker \
    /mnt/us/bilicomics/launch.sh /mnt/us/bilicomics/data \
    /mnt/us/documents/bilicomics.sh /mnt/us/documents/BiliComics.sh \
    /mnt/us/documents/bilicomics.sdr /mnt/us/documents/BiliComics.sdr \
    /mnt/us/documents/bilicomics.sh.sdr /mnt/us/documents/BiliComics.sh.sdr \
    /var/local/kmc/bin/sh_integration_launcher; do
    printf 'path:%s=%s\n' "$path" "$(kind "$path")"
done
for utility in sqlite3 xdotool xwininfo xwd tesseract lipc-get-prop lipc-set-prop; do
    if command -v "$utility" >/dev/null 2>&1; then available=yes; else available=no; fi
    printf 'tool_%s=%s\n' "$utility" "$available"
done
counts=unknown
if [ -f /var/local/cc.db ] && command -v sqlite3 >/dev/null 2>&1; then
    if result=$(sqlite3 -readonly -batch -noheader /var/local/cc.db \
        "PRAGMA query_only=ON; SELECT COUNT(*) FROM Entries WHERE p_location COLLATE BINARY IN ('/mnt/us/documents/bilicomics.sh','/mnt/us/documents/BiliComics.sh');" 2>/dev/null); then
        case "$result" in ''|*[!0-9]*) :;; *) counts=$result;; esac
    fi
fi
printf 'catalog_selected_rows=%s\n' "$counts"
native after
