#!/bin/sh
set -eu
[ "$(pwd -P)" = /work ] && [ -f /opt/kindle-toolchain/env.sh ]
lib=/work/runtime/usr/lib/arm-linux-gnueabihf
set -- -target arm-linux-gnueabihf.2.35 -std=c11 -O2 -Wall -Wextra -Werror -I/work/sdk/usr/include \
    native-target.c "$lib/libX11.so.6" "$lib/libtesseract.so.5" -Wl,--allow-shlib-undefined
zig cc "$@" -o native-target
zig cc -DBILI_NATIVE_INPUT=1 "$@" "$lib/libXtst.so.6" -o card-input
for program in native-target card-input; do
    readelf -h "$program" | grep -Eq 'Machine:.*ARM'
    readelf -h "$program" | grep -Eq 'Flags:.*hard-float ABI'
    qemu-arm -cpu cortex-a9 "$lib/ld-linux-armhf.so.3" --library-path "$lib" --list "/work/$program" >"/work/$program-libraries.txt"
done
