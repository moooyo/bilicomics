#!/bin/bash
# Host-only once on test-env, after restoring connectivity. No device action.
set -euo pipefail
umask 077
root=$(cd -P -- "$(dirname -- "$0")" && pwd -P)
cd "$root"
test "$(stat -c '%u:%a' "$root")" = "$(id -u):700"
test -n "${BILI_BASE_BUILD_ROOT:?Set the original retained build root}"
test -n "${BILI_BASE_ZIP:?Set the retained baseline package path}"
if git -C "$root" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    printf '%s\n' 'Deploy the three sources into a fresh private build directory outside the checkout.' >&2
    exit 1
fi
test -f build.py && test -f box.sh
for path in baseline combined upper work readonly-cache; do test ! -e "$path" && test ! -L "$path"; done
install -d -m 700 baseline combined combined/logs combined/tmp upper work readonly-cache
free=$(df --output=avail -B1 "$root" | tail -n 1)
test "$free" -gt 1750000000
cat > combined/candidate-sums.sha256 <<'SUMS'
e2d0dbc3c7c54547897454f0c1f046afff79fcc5e80f3f2390d2957f6cd64afe  packaging/kindle/launch.sh
eabdc9f0d92fbaf3f47311666b0e4b4b1d6555e2892073593956076235ea7630  packaging/tests/launcher_test.py
d51a9885ec6194b63d7e0dafb037320e912e5b769933416755807756393f24ee  scripts/verification/launcher_real_reader.py
8ddbaddb8bf7671ed676e161f29188946f95beab02e903154a2ea00a67e1e5c4  scripts/verification/physical/physical-watchdog.sh
ba2f4f1f36e710f9dec3d0e413e5d20913763bfb2a821d47c6f08c753c2dfeda  scripts/verification/test_physical_harness.py
c67bf1e2da5af78a166a101327a444ba6d4a3d10f896fb56be1c48c11994aebe  src/app/startup.rs
SUMS
cut -d' ' -f3 combined/candidate-sums.sha256 > combined/allowed-changes.txt
python3 -B build.py extract-baseline
python3 -B build.py before
python3 -B build.py extract-candidate
(cd combined/app && sha256sum -c ../candidate-sums.sha256)
df -h "$root"
