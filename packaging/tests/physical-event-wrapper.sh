#!/bin/sh
# Preserve native power events while recording the supervised observer lineage.
set -eu
umask 077
run_root=${BILICOMICS_HOME:?}
case "$run_root" in /mnt/us/bilicomics-physical-*) run_id=${run_root#/mnt/us/bilicomics-physical-} ;; *) exit 2 ;; esac
case "$run_id" in *[!a-z0-9-]*|'') exit 2 ;; esac
[ "${#run_id}" -le 48 ] || exit 2
logs=$run_root/data/logs
read_one() { sed -n '1{s/\r$//;p;}' "$1"; }
token=$(read_one "$logs/run-token")
case "$token" in *[!0-9a-f]*|'') exit 2 ;; esac
[ "${#token}" -eq 64 ] && [ "${BILICOMICS_PHYSICAL_TOKEN:-}" = "$token" ] || exit 2
original_command=$(read_one "$logs/power-event-command")
original_exe=$(read_one "$logs/power-event-executable")
case "$original_command:$original_exe" in /*:/*) ;; *) exit 2 ;; esac
[ "$original_command" != "$run_root/tools/lipc-wait-event" ] || exit 2
[ -x "$original_command" ] && [ "$(readlink -f "$original_command")" = "$original_exe" ] || exit 2
stat_tail=$(sed 's/^.*) //' "/proc/$$/stat")
start_ticks=$(printf '%s\n' "$stat_tail" | awk '{ print $20 }')
parent_pid=$(printf '%s\n' "$stat_tail" | awk '{ print $2 }')
parent_tail=$(sed 's/^.*) //' "/proc/$parent_pid/stat")
parent_ticks=$(printf '%s\n' "$parent_tail" | awk '{ print $20 }')
parent_parent=$(printf '%s\n' "$parent_tail" | awk '{ print $2 }')
parent_exe=$(readlink "/proc/$parent_pid/exe")
launcher_pid=$(read_one /tmp/bilicomics-launcher.lock/pid)
[ "$parent_parent" = "$launcher_pid" ] || exit 2
tr '\000' '\n' <"/proc/$parent_pid/environ" | grep -F -x "BILICOMICS_PHYSICAL_TOKEN=$token" >/dev/null
printf '%s|%s|%s|%s|%s\n' "$parent_pid" "$parent_ticks" "$parent_exe" "$launcher_pid" "$token" >"$logs/power-observer.identity.pending"
mv "$logs/power-observer.identity.pending" "$logs/power-observer.identity"
printf '%s|%s|%s|%s|%s\n' "$$" "$start_ticks" "$original_exe" "$parent_pid" "$token" >"$logs/power-event.identity.pending"
mv "$logs/power-event.identity.pending" "$logs/power-event.identity"
exec "$original_command" "$@"
