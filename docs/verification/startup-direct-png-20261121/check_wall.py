"""Check the application-only realtime shim without changing the host or observer clock."""

import json
import os
import subprocess
import sys
import time


def stamps():
    return [time.clock_gettime_ns(clock) for clock in (time.CLOCK_REALTIME, time.CLOCK_MONOTONIC, time.CLOCK_THREAD_CPUTIME_ID)]


if sys.argv[1:] == ['child']:
    first = stamps()
    until = first[1] + 30_000_000
    while time.monotonic_ns() < until:
        pass
    time.sleep(.025)
    last = stamps()
    delta = [after - before for before, after in zip(first, last)]
    assert first[0] == last[0] == 1795233600 * 10**9 and delta[1] >= 50_000_000 and delta[2] > 0
    print(json.dumps({'fixed_realtime_ns': first[0], 'elapsed_child_monotonic_ns': delta[1], 'elapsed_child_thread_cpu_ns': delta[2]}))
else:
    assert 'LD_PRELOAD' not in os.environ
    before = stamps()
    child = json.loads(subprocess.check_output([sys.executable, '-B', __file__, 'child'], env=dict(os.environ, LD_PRELOAD='/work/fixed-wall.so')))
    after = stamps()
    assert after[0] > before[0] and before[0] != child['fixed_realtime_ns'] and after[1] - before[1] >= child['elapsed_child_monotonic_ns']
    child['external_parent_realtime_and_monotonic_advanced'] = True
    print(json.dumps(child, indent=2))
