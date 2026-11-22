#!/bin/bash
set -euo pipefail
umask 077
test "$#" -eq 3 && test "$(id -u)" -eq 0
workspace=$1
build=$2
case "$3" in r1|r2|r3) ;; *) exit 1 ;; esac
case "$workspace" in /var/tmp/bilicomics-display-poll-product-*) ;; *) exit 1 ;; esac
case "$build" in /var/tmp/bilicomics-display-poll-build-*) ;; *) exit 1 ;; esac
for root in "$workspace" "$build"; do
    test "$(dirname "$root")" = /var/tmp && test "$(realpath -e "$root")" = "$root"
    test "$(stat -c '%F:%u:%a' "$root")" = 'directory:0:700'
done
test ! -e "$workspace/reports/$3" && test ! -L "$workspace/reports/$3"
elf="$build/artifacts/host-bilicomics"
test "$(realpath -e "$elf")" = "$elf" && test "$(stat -c '%F:%u:%h:%a' "$elf")" = 'regular file:0:1:700'
bytes=$(python3 -IB - "$build" <<'PY'
import json, os, struct, sys
from pathlib import Path
root = Path(sys.argv[1])
with (root/'artifacts/host-bilicomics').open('rb') as stream:
    ident = stream.read(20)
    size = os.fstat(stream.fileno()).st_size
if ident[:7] != b'\x7fELF\x02\x01\x01' or struct.unpack_from('<H', ident, 18)[0] != 62:
    raise SystemExit('Expected the Linux x86-64 product')
source = json.loads((root/'qualification/source.json').read_bytes())
overlay = {'src/platform/mod.rs', 'src/platform/native_capture.rs', 'src/platform/capture_cost.rs', 'src/native_presentation/portrait_writer.rs'}
if not (source['schema'] == 'bili-display-poll-source-v1' and source['overlay_only'] is True and source['overlay'] == sorted(overlay)
        and source['selector'] == 'BILICOMICS_DISPLAY_POLL_RUN' and source['receipt_suffix'] == '.display-poll.json'
        and source['receipt_schema'] == 'kindle-startup-display-poll-v1'):
    raise SystemExit('Wrong frozen diagnostic source')
host = json.loads((root/'qualification/host.json').read_bytes())
if not (host['schema'] == 'bili-display-poll-host-build-v1' and host['passed'] is True and host['exit_status'] == 0
        and host['retained_tmpfs_product_full_byte_match'] is True and host['network_isolated'] is True
        and host['mode'] == 'host' and host['compiled_bytes'] == size and 0 < size < 390 * 1024**2
        and host['command'] == 'cargo build --locked --offline --no-default-features --features kindle -j1 --bin bilicomics'
        and host['worker_guard_controls'] == ['passed real display-helper permission refusal: scribe', 'passed real display-helper permission refusal: pw6']):
    raise SystemExit('Missing new full product build and permission refusals')
print(size)
PY
)
exec unshare --net bash -c 'set -eu; ip link set lo up; test "$(ip -o link show | wc -l)" -eq 1; exec /opt/kindle-toolchain/ubuntu-run "$1" bash /work/inputs/inside.sh "$3" "$4" < "$2"' isolated "$workspace" "$elf" "$bytes" "$3"
