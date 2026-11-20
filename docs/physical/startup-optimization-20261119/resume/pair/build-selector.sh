#!/bin/sh
# Run only on test-env, outside a device lease. No network or Kindle access.
set -eu
umask 077
work=${BILI_PAIR_WORK:?Set the new private absolute workspace on test-env}
history=${BILI_PAIR_HISTORY:?Identify the separate, existing read-only click research root}
test_host=${BILI_PAIR_TEST_HOST:?Identify the authorized test-env hostname from trusted inventory}
[ "$(uname -s)" = Linux ] && [ "$(hostname)" = "$test_host" ] && [ "$(id -u)" = 0 ] || exit 2
[ "$(pwd -P)" = "$work" ] && [ "$(readlink -f "$0")" = "$work/build-selector.sh" ] &&
    [ "$(stat -c '%u:%a' "$work")" = 0:700 ] && [ -d "$history" ] &&
    [ "$(readlink -f "$history")" = "$history" ] && [ "$history" != / ] && [ "$work" != / ] || exit 2
case "$work/" in "$history/"*) exit 2;; esac
case "$history/" in "$work/"*) exit 2;; esac
for f in select-arms.in.sh pair-fs.c test-select-arms.py; do [ -f "$f" ] && [ ! -L "$f" ] || exit 2; done
test ! -e pair-fs && test ! -L pair-fs || exit 2
python3 -IB test-select-arms.py
if command -v zig >/dev/null 2>&1; then
    set -- zig cc -target arm-linux-musleabihf
elif command -v arm-linux-gnueabihf-gcc >/dev/null 2>&1; then
    set -- arm-linux-gnueabihf-gcc -march=armv7-a -mfpu=vfpv3-d16 -mfloat-abi=hard
else
    printf '%s\n' 'No private ARM hard-float cross compiler in PATH.' >&2; exit 2
fi
build=$(mktemp -d "$work/selector-build-XXXXXX")
"$@" -std=c11 -O2 -Wall -Wextra -Werror -static -s "$work/pair-fs.c" -o "$build/pair-fs"
readelf -hW "$build/pair-fs" >"$build/elf-header.txt"
readelf -lW "$build/pair-fs" >"$build/elf-programs.txt"
grep -Eq 'Class:.*ELF32' "$build/elf-header.txt"
grep -Eq 'Machine:.*ARM' "$build/elf-header.txt"
grep -Eq 'Flags:.*hard-float ABI' "$build/elf-header.txt"
rc=0; grep -Eq 'INTERP|Requesting program interpreter' "$build/elf-programs.txt" || rc=$?
test "$rc" = 1 || exit 2
test "$(wc -c <"$build/pair-fs")" -le 4194304
if command -v qemu-arm >/dev/null 2>&1; then
    rc=0; qemu-arm -cpu cortex-a9 "$build/pair-fs" held /unsupported >"$build/arm-smoke.out" 2>"$build/arm-smoke.err" || rc=$?
    test "$rc" = 1 && test ! -s "$build/arm-smoke.out" && grep -Fxq 'pair-fs: unbound run' "$build/arm-smoke.err" || exit 2
fi
# Atomic absent-only host publication. Cell selectors require st_nlink == 1 on-device,
# and tar extraction creates new regular files there.
ln "$build/pair-fs" "$work/pair-fs"
rm "$build/pair-fs"
sha256sum select-arms.in.sh pair-fs.c pair-fs test-select-arms.py
