#!/bin/bash
# This program is invoked only by unshare -m --propagation private on test-env.
set -euo pipefail
umask 077
work=$(cd -P -- "$(dirname -- "$0")" && pwd -P)
old=${BILI_BASE_BUILD_ROOT:?Set the original retained build root}
test "$(readlink -f "$old")" = "$old"
case "$work/" in "$old/"*) exit 1;; esac
case "$old/" in "$work/"*) exit 1;; esac
case "$work" in *[,:]*) exit 1;; esac
test "$(readlink /proc/self/ns/mnt)" != "$(readlink /proc/1/ns/mnt)"
test "$(findmnt -no PROPAGATION /)" = private
test -d "$old/candidate-r1" && test -d "$old/target"
test -d "$work/combined/app" && test -d "$work/readonly-cache"
python3 -B "$work/build.py" base-tools
# Keep absolute paths from the original build so Cargo can resolve already-built
# dependencies and the unchanged JPEG worker can retain its original identity.
mount --bind "$old/target" "$work/readonly-cache"
mount -o remount,bind,ro "$work/readonly-cache"
mount -t overlay overlay -o "lowerdir=$work/readonly-cache,upperdir=$work/upper,workdir=$work/work" "$old/target"
mount --bind "$work/combined" "$old/candidate-r1"
test "$(findmnt -no FSTYPE --target "$old/target")" = overlay
test "$(stat -c '%d:%i' "$work/combined/app")" = "$(stat -c '%d:%i' "$old/candidate-r1/app")"
cd "$old/candidate-r1/app"
exec python3 -B "$work/build.py" "$@"
