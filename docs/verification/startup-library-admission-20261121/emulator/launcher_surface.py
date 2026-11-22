"""Exercise the real Library parent and worker through a fake native adapter and a real X11 Shelf."""

import argparse
import contextlib
import ctypes
import json
import os
from pathlib import Path
import re
import socket
import sqlite3
import subprocess
import sys
import time


ROOT = Path('/work/launcher-surface')
ORACLE = ROOT / 'oracle'
sys.path[:0] = [str(ORACLE), '/work/packaging/tests']
import desktop_pairs
import launcher_test
import profiles


SHIMS = {'A': Path('/work/control/launch.sh'), 'B': Path('/work/research-candidate/launch.sh')}
SHIM_BYTES = {'A': 80185, 'B': 80710}
QUALIFICATION = [('scribe', 'A'), ('scribe', 'B'), ('pw6', 'B'), ('pw6', 'A')]
FORMAL = [('scribe', arm) for pair in range(6) for arm in ('AB' if pair % 2 == 0 else 'BA')]
ADAPTER = r'''case "${1:-}" in
    --capture-native-display|--probe-native-portrait-framebuffer|--publish-native-portrait-orientation|--relay-power-events)
        exec "$SURFACE_HELPER" "$@" ;;
esac
test "$#" = 5 && test "$1" = --kindle && test "$2" = --device && test "$3" = "$SURFACE_PROFILE" && test "$4" = --data-dir && test "$5" = "$BILICOMICS_HOME/data"
test "$BILICOMICS_APPROVED_PROFILE" = "$3" && test "$BILICOMICS_APPROVED_FIRMWARE" = 5.20.1
test -n "$BILICOMICS_LAUNCH_GENERATION" && test -n "$BILICOMICS_LAUNCH_TOKEN" && test -d "$BILICOMICS_LIFECYCLE_DIR"
set -C; printf '%s\n' "$$" >"$SURFACE_PID.pending"; set +C
mv -T -- "$SURFACE_PID.pending" "$SURFACE_PID"
unset BILICOMICS_LIFECYCLE_DIR BILICOMICS_LAUNCH_GENERATION BILICOMICS_LAUNCH_TOKEN BILICOMICS_APPROVED_PROFILE BILICOMICS_APPROVED_FIRMWARE
exec "$SURFACE_ELF" --device "$3" --data-dir "$5" --demo --offline --lifecycle-trace "$SURFACE_LIFECYCLE" --exit-after-ms 1200'''


def require(condition, message):
    if not condition:
        raise ValueError(message)


def now():
    return time.clock_gettime_ns(time.CLOCK_MONOTONIC)


def write(path, value):
    with path.open('x') as output:
        output.write(json.dumps(value, indent=2) + '\n')


def geometry(profile):
    return profiles.PROFILES[profile]


def inputs(profile, executable, arm, harness, directory):
    profiles.elf(executable, 'x86_64')
    require(executable.is_relative_to('/work') and os.path.samefile(executable, executable.resolve()), 'Candidate application must be in the current workspace')
    require(SHIMS[arm].stat().st_size == SHIM_BYTES[arm], 'Not the frozen source launcher for this arm')
    launcher_test.SOURCE = SHIMS[arm]
    harness.setUp()
    harness.configure_native_profile(profile)
    original = harness.home / 'app/bilicomics'
    helper = harness.root / 'native-helper'
    original.rename(helper)
    harness.script(original, ADAPTER)
    data = harness.home / 'data/demo'
    data.mkdir(mode=0o700)
    with sqlite3.connect(data / 'state.sqlite3') as database:
        database.execute('CREATE TABLE preferences (account_key TEXT NOT NULL, key TEXT NOT NULL, payload TEXT NOT NULL, PRIMARY KEY(account_key,key))')
        database.execute('INSERT INTO preferences VALUES (?, ?, ?)', ('design-demo', 'preferences', json.dumps({'launch': 'shelf'})))
    home = harness.root / 'xdg'
    home.mkdir(mode=0o700)
    harness.env.update(HOME=str(home), XDG_CACHE_HOME=str(home/'cache'), XDG_CONFIG_HOME=str(home/'config'), XDG_DATA_HOME=str(home/'data'),
                       SURFACE_ELF=str(executable), SURFACE_PID=str(directory/'app-pid'), SURFACE_LIFECYCLE=str(directory/'lifecycle.jsonl'), SURFACE_HELPER=str(helper), SURFACE_PROFILE=profile,
                       SLINT_BACKEND='winit-software', WINIT_UNIX_BACKEND='x11', SLINT_SCALE_FACTOR='2', LC_ALL='C.UTF-8', TZ='UTC')
    require(harness.env['BILICOMICS_LAUNCH_TRACE'] == '0' and harness.env['TEST_STARTUP_SETTLE'] == 'true' and not harness.env.get('LD_PRELOAD'), 'Unexpected research or settle environment')
    require('/tmp/bilicomics-launcher.lock' not in harness.launcher.read_text(), 'Missing fixture-local launcher lock substitution')
    require('native-helper' not in harness.launcher.read_text() and '--kindle --device "$PROFILE" --data-dir "$DATA_DIR"' in harness.launcher.read_text(), 'Fixture changed the primary launcher flow')


def monitor(harness, directory, app, profile, child, origin, oracle, expected):
    size, boxes = geometry(profile)
    app_identity = None
    window = None
    lower = origin[0]
    views = 0
    worker = None
    for name in ('XGrabServer', 'XUngrabServer', 'XFlush'):
        getattr(oracle.api, name).argtypes = [ctypes.c_void_p]
        getattr(oracle.api, name).restype = ctypes.c_int
    deadline = time.monotonic() + 25
    while time.monotonic() < deadline and child.poll() is None:
        sample_start = now()
        if app_identity is None:
            try:
                token = (directory / 'app-pid').read_text().strip()
                require(token.isdigit(), 'Bad private application PID marker')
                candidate = int(token)
                if os.path.samefile(f'/proc/{candidate}/exe', app):
                    runner = harness.process_identity(candidate)['parent']
                    owner = harness.process_identity(runner)['parent']
                    worker = harness.admission_worker_identity(child)
                    require(worker is not None and owner == worker['pid'] and worker['recorded_parent'] == child.pid and harness.admission_worker_ready(worker), 'Application is not owned by this Library worker')
                    app_identity = candidate
            except (FileNotFoundError, ProcessLookupError):
                pass
        if app_identity is not None and window is None:
            window = profiles.mapped_window(app_identity, size)
        if window is not None:
            views += 1
            matched = True
            oracle.api.XGrabServer(oracle.display)
            try:
                for name, box in boxes.items():
                    interval, pixels = oracle.get(window, box)
                    if pixels != expected[name]:
                        lower = interval[0]
                        matched = False
                        break
            finally:
                oracle.api.XUngrabServer(oracle.display)
                oracle.api.XFlush(oracle.display)
            if matched:
                last = interval[1]
                break
        time.sleep(.005)
    else:
        raise ValueError('No non-cover Shelf observation while the synchronous Library parent was live')
    require(app_identity is not None and origin[0] <= lower <= last and sample_start <= last and worker, 'Unordered Shelf observation')
    return app_identity, {'observation_lower_ns': max(0, lower-origin[1]), 'observation_upper_ns': last-origin[0], 'launch_bracket_ns': origin[1]-origin[0],
                          'first_match_sample_cost_ns': last-sample_start, 'viewable_samples': views}


def observe(arm, profile, app, directory):
    profiles.ROOT = ORACLE
    size, boxes = geometry(profile)
    expected = profiles.reference(profile, 'shelf', size, boxes)
    splash = profiles.reference(profile, 'loading', size, boxes)
    require(all(sum(value < 128 for value in pixels[::3]) >= 150 and sum(first != second for first, second in zip(pixels[::3], splash[name][::3])) >= 150 for name, pixels in expected.items()), 'Reference does not distinguish the chosen Shelf regions')
    harness = launcher_test.LauncherLifecycleTests()
    with contextlib.ExitStack() as cleanup:
        cleanup.callback(harness.doCleanups)
        inputs(profile, app, arm, harness, directory)
        sampler = desktop_pairs.Sampler()
        cleanup.callback(sampler.close)
        origin_start = now()
        child = harness.spawn_admission('--library')
        origin = (origin_start, now())
        try:
            app_pid, measurement = monitor(harness, directory, app, profile, child, origin, sampler, expected)
            child.communicate(timeout=20)
            require(child.returncode == 0, 'Library parent/worker did not finish naturally')
            require(harness.live_admission_worker(harness.admission_worker_identity(child)) is None and not (harness.root/'lock').exists(), 'Fixture launcher lock or worker outlived the run')
            require(harness.trace() == ['stop:statusbar', 'start:statusbar', 'xrefresh:'], 'Fake native display was not restored exactly once')
            require(len(harness.captures()) == 1, 'Native-helper fixture was not reached once')
            rows = list(map(json.loads, (directory/'lifecycle.jsonl').read_text().splitlines()))
            require(all(row.get('process_id') == app_pid for row in rows), 'Not a single real desktop application process')
            ready = [row for row in rows if row.get('event') == 'ready']
            enters = [row['monotonic_ns'] for row in rows if row.get('stage') == 'process_enter']
            require(len(ready) == 1 and ready[0].get('route') == 'shelf' and len(enters) == 1, 'Unexpected route or lifecycle identity')
            require(origin[0] <= enters[0] <= origin[0]+measurement['observation_upper_ns'], 'App process-entry did not precede the successful Shelf observation')
            measurement['entry_upper_ns'] = enters[0]-origin[0]
            measurement['entry_lower_ns'] = max(0, enters[0]-origin[1])
        finally:
            if child.poll() is None:
                harness.finish_admission_entry(child)
    return {'profile': profile, 'arm': arm, 'application_bytes': app.stat().st_size, 'source_launcher_bytes': SHIMS[arm].stat().st_size,
            'geometry': list(size), 'verified_real_executable_and_process_chain': True, 'same_pid_ready_route': 'shelf', 'live_window_non_cover_regions': list(boxes),
            'natural_worker_and_parent_exit': True, 'single_mock_display_takeover_and_restoration': True, **measurement}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('phase', choices=('qualification', 'formal'))
    parser.add_argument('number', type=int)
    parser.add_argument('executable', type=Path)
    parser.add_argument('series', choices=('r1', 'r2', 'r3'))
    args = parser.parse_args()
    os.umask(0o077)
    require(Path.cwd().resolve() == ROOT and [name for _, name in socket.if_nameindex()] == ['lo'], 'Not in the isolated fixture namespace')
    schedule = QUALIFICATION if args.phase == 'qualification' else FORMAL
    require(1 <= args.number <= len(schedule), 'Case outside its fixed schedule')
    series = ROOT/'results'/args.series
    series.mkdir(mode=0o700, exist_ok=True)
    if args.phase == 'formal':
        for position in range(1, 5):
            require(json.loads((series/f'qualification-{position:02d}'/'result.json').read_text())['status'] == 'passed', 'Qualification not complete')
    if args.number > 1:
        require(json.loads((series/f'{args.phase}-{args.number-1:02d}'/'result.json').read_text())['status'] == 'passed', 'Preceding case failed or was skipped')
    profile, arm = schedule[args.number-1]
    directory = series/f'{args.phase}-{args.number:02d}'
    directory.mkdir(mode=0o700)
    result = {'series': args.series, 'phase': args.phase, 'number': args.number, 'arm': arm, 'profile': profile}
    try:
        result['measurement'] = observe(arm, profile, args.executable, directory)
        result['status'] = 'passed'
    except Exception as error:
        result.update(status='failed', reason=str(error))
        write(directory/'result.json', result)
        raise
    write(directory/'result.json', result)
    print(json.dumps(result, sort_keys=True))


if __name__ == '__main__':
    main()
