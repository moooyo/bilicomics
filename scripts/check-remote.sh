#!/bin/sh
# Run only on the designated remote host, from a newly uploaded source tree.
set -eu

ROOT=${1:?Supply the isolated remote root}
KINDLE=${2:-0}
CHECK_ONLY=${3:-0}
PHASES=${4:-core,defaults,pointer,reader,guest,startup,lifecycle,launcher}
case "$ROOT" in /*) ;; *) printf '%s\n' 'The remote root must be absolute.' >&2; exit 1 ;; esac
if [ -f /root/.cargo/env ]; then . /root/.cargo/env; fi
export BILICOMICS_REMOTE_VERIFICATION=1
set -- python3 "$ROOT/app/scripts/verification/run_suite.py" --workspace "$ROOT" --phases "$PHASES"
if [ -n "${CARGO_TARGET_DIR:-}" ]; then set -- "$@" --target-dir "$CARGO_TARGET_DIR"; fi
if [ "$CHECK_ONLY" = 1 ]; then set -- "$@" --check-only; fi
if [ "$KINDLE" = 1 ]; then set -- "$@" --build-kindle; fi
exec "$@"
