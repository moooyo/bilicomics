#!/bin/sh
# Build on the Linux worker. Firmware libraries stay on the Kindle.
set -eu
NATIVE_QUERY_SCRIPT_DIR=$(CDPATH= cd "$(dirname "$0")" && pwd)
NATIVE_QUERY_REPO=$(CDPATH= cd "$NATIVE_QUERY_SCRIPT_DIR/.." && pwd)
NATIVE_QUERY_OUTPUT=${1:?Pass the helper output path}
if [ -n "${ZIG_BIN_DIR:-}" ]; then
    NATIVE_QUERY_ZIG=$ZIG_BIN_DIR/zig
else
    NATIVE_QUERY_ZIG=zig
fi
mkdir -p "$(dirname "$NATIVE_QUERY_OUTPUT")"
"$NATIVE_QUERY_ZIG" cc -target arm-linux-gnueabihf.2.4 \
    -mcpu=cortex_a8 -mfpu=vfpv3-d16 -mfloat-abi=hard \
    -std=c11 -O2 -Wall -Wextra -Werror -fstack-protector-strong -fPIE -pie \
    -Wl,-z,relro,-z,now -s "$NATIVE_QUERY_REPO/src/native_query/worker.c" \
    -ldl -o "$NATIVE_QUERY_OUTPUT"
