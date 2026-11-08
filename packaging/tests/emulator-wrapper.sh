#!/bin/sh
# Install only in the isolated firmware-emulator guest, never in a product bundle.
set -eu

run_root=${BILICOMICS_HOME:?}
binary=$(cat "$run_root/data/logs/test-binary")
case "$binary" in
    bilicomics-????????????)
        case "${binary#bilicomics-}" in *[!0-9a-f]*) exit 2 ;; esac
        ;;
    *) exit 2 ;;
esac
test_case=$(cat "$run_root/data/logs/test-case")
printf '%s\n' "$$" > "$run_root/data/logs/native.pid"
case "$test_case" in
    startup-1|startup-2|startup-3)
        exec "$run_root/app/$binary" "$@" --demo --offline --screen shelf \
            --capture-startup "$run_root/data/logs/startup.png" \
            --startup-delay-ms 1800 --exit-after-ms 6000 \
            --capture "$run_root/data/logs/final-native.png" \
            --lifecycle-trace "$run_root/data/logs/lifecycle.jsonl"
        ;;
    interactive|reader|sleep|recovery)
        exec "$run_root/app/$binary" "$@" --demo --offline --screen shelf \
            --capture "$run_root/data/logs/final-native.png" \
            --lifecycle-trace "$run_root/data/logs/lifecycle.jsonl" \
            --exit-after-ms 600000
        ;;
    *) exit 2 ;;
esac
