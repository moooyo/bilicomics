#!/bin/sh
# Run only the frozen entry-shell and static-package checks on test-env.
set -eu
umask 077
LIBRARY_ENTRY_ROOT=/root/bilicomics-native-library-entry-20261117-01a1d11a-r1
export PYTHONDONTWRITEBYTECODE=1
export PYTHONNOUSERSITE=1
export CARGO_NET_OFFLINE=true
export BILICOMICS_REMOTE_VERIFICATION=1
exec python3 -B -u "$LIBRARY_ENTRY_ROOT/runner-r1.py" "$@"
