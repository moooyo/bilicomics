#!/bin/sh
# Install only in a fresh supervised physical-device run directory.
set -eu
umask 077

run_root=${BILICOMICS_HOME:?}
case "$run_root" in
    /mnt/us/bilicomics-physical-*) run_id=${run_root#/mnt/us/bilicomics-physical-} ;;
    *) exit 2 ;;
esac
case "$run_id" in *[!a-z0-9-]*|'') exit 2 ;; esac
[ "${#run_id}" -le 48 ] || exit 2
logs=$run_root/data/logs
read_one() { sed -n '1{s/\r$//;p;}' "$1"; }
token=$(read_one "$logs/run-token")
case "$token" in *[!0-9a-f]*|'') exit 2 ;; esac
[ "${#token}" -eq 64 ] || exit 2
[ "${BILICOMICS_PHYSICAL_TOKEN:-}" = "$token" ] || exit 2
binary=$(read_one "$logs/binary-name")
case "$binary" in bilicomics-*) digest=${binary#bilicomics-} ;; *) exit 2 ;; esac
case "$digest" in *[!0-9a-f]*|'') exit 2 ;; esac
[ "${#digest}" -eq 64 ] || exit 2
screen=$(read_one "$logs/screen")
case "$screen" in shelf|reader) ;; *) exit 2 ;; esac
duration=$(read_one "$logs/exit-after-ms")
case "$duration" in *[!0-9]*|'') exit 2 ;; esac
[ "$duration" -ge 1000 ] && [ "$duration" -le 600000 ] || exit 2
[ -x "$run_root/app/$binary" ] || exit 2

# exec retains the PID expected by the production launcher's signal bridge.
stat_tail=$(sed 's/^.*) //' "/proc/$$/stat")
start_ticks=$(printf '%s\n' "$stat_tail" | awk '{ print $20 }')
parent_pid=$(printf '%s\n' "$stat_tail" | awk '{ print $2 }')
case "$start_ticks:$parent_pid" in *[!0-9:]*|:*) exit 2 ;; esac
launcher_pid=$(read_one /tmp/bilicomics-launcher.lock/pid)
parent_tail=$(sed 's/^.*) //' "/proc/$parent_pid/stat")
parent_ticks=$(printf '%s\n' "$parent_tail" | awk '{ print $20 }')
parent_parent=$(printf '%s\n' "$parent_tail" | awk '{ print $2 }')
parent_exe=$(readlink "/proc/$parent_pid/exe")
[ "$parent_parent" = "$launcher_pid" ] || exit 2
[ "${BILICOMICS_LAUNCH_GENERATION:-}" = 1 ] || exit 2
[ "${BILICOMICS_LAUNCH_TOKEN:-}" = "$(read_one /tmp/bilicomics-launcher.lock/nonce)" ] || exit 2
tr '\000' '\n' <"/proc/$parent_pid/environ" | grep -F -x "BILICOMICS_PHYSICAL_TOKEN=$token" >/dev/null
printf '%s|%s|%s|%s|%s\n' "$parent_pid" "$parent_ticks" "$parent_exe" "$launcher_pid" "$token" >"$logs/runner.identity.pending"
mv "$logs/runner.identity.pending" "$logs/runner.identity"
printf '%s\n' "$$" >"$logs/native.pid.pending"
mv "$logs/native.pid.pending" "$logs/native.pid"
printf '%s|%s|%s|%s|%s\n' "$$" "$start_ticks" "$run_root/app/$binary" "$parent_pid" "$token" >"$logs/native.identity.pending"
mv "$logs/native.identity.pending" "$logs/native.identity"
exec "$run_root/app/$binary" "$@" --demo --offline --screen "$screen" \
    --lifecycle-trace "$logs/lifecycle.jsonl" --exit-after-ms "$duration"
