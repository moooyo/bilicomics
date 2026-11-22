"""Qualify the two clock wrappers through a real Library shell, Rust process and live X11 Shelf."""

import argparse
import contextlib
import ctypes
import json
import os
from pathlib import Path
import shutil
import socket
import sqlite3
import subprocess
import sys
import time
import uuid


ROOT = Path('/work')
TOOLS = ROOT / 'tooling'
INPUTS = ROOT / 'inputs'
sys.path[:0] = [str(TOOLS), str(TOOLS/'studies/startup-clock-aa-20261122')]
import aa_runner
import desktop_pairs
import launcher_test
import profiles


ADAPTER = r'''case "${1:-}" in
    --capture-native-display|--probe-native-portrait-framebuffer|--publish-native-portrait-orientation|--relay-power-events) exec "$AA_NATIVE_STUB" "$@" ;;
esac
test "$#" = 13 && test "$1" = --kindle && test "$2" = --device && test "$3" = "$AA_PROFILE" && test "$4" = --data-dir && test "$5" = "$BILICOMICS_HOME/data" && test "$6" = --demo && test "$7" = --offline && test "$8" = --shelf-render-trace && test "$9" = "$5/$AA_RUN.jsonl"
test "${10}" = --exit-after-ms && test "${11}" = 2500 && test "${12}" = --lifecycle-trace && test "${13}" = "$5/$AA_RUN-lifecycle.jsonl"
test "$BILICOMICS_APPROVED_PROFILE" = "$3" && test "$BILICOMICS_APPROVED_FIRMWARE" = 5.20.1
unset BILICOMICS_LIFECYCLE_DIR BILICOMICS_LAUNCH_GENERATION BILICOMICS_LAUNCH_TOKEN BILICOMICS_APPROVED_PROFILE BILICOMICS_APPROVED_FIRMWARE
exec "$AA_DESKTOP" --device "$3" --data-dir "$5" --demo --offline --exit-after-ms "${11}" --lifecycle-trace "${13}"'''


def require(condition, reason):
    if not condition:
        raise ValueError(reason)


def now():
    return time.clock_gettime_ns(time.CLOCK_MONOTONIC)


def mark(stage, label, run):
    leaf = {'shim_entry': 'shim-entry.json', 'app_preinvoke': 'app-preinvoke.json'}[label]
    path = stage/'picked'/leaf
    try:
        row = json.loads(path.read_text())
    except (FileNotFoundError, json.JSONDecodeError):
        return None
    require(set(row) == {'schema','run_id','clock','stage','pid','monotonic_ns'} and row['schema'] == 'bili-startup-clock-mark-v1' and row['run_id'] == run and row['clock'] == 'CLOCK_MONOTONIC' and row['stage'] == label and row['pid'] > 0 and row['monotonic_ns'] > 0, 'Invalid native-wrapper host receipt')
    return row


def process(profile, case, run):
    elf = INPUTS/'desktop'
    profiles.elf(elf, 'x86_64')
    require(elf.stat().st_size == 241479976 and (INPUTS/'stock-launch.sh').stat().st_size == 80185, 'Unexpected retained input')
    files = aa_runner.case_files((INPUTS/'stock-launch.sh').read_bytes(), run)
    transform = case/'produced-launch.sh'
    transform.write_bytes(files['launch.sh'])
    launcher_test.SOURCE = transform
    profiles.ROOT = INPUTS
    size, boxes = profiles.PROFILES[profile]
    expected = profiles.reference(profile, 'shelf', size, boxes)
    loading = profiles.reference(profile, 'loading', size, boxes)
    require(all(sum(pixel<128 for pixel in data[::3]) > 150 and data != loading[key] for key,data in expected.items()), 'Uninformative pixel references')
    with contextlib.ExitStack() as clean:
        fixture = launcher_test.LauncherLifecycleTests()
        clean.callback(fixture.doCleanups)
        fixture.setUp()
        stage = fixture.root/run
        fixture.home.rename(stage)
        fixture.home = stage
        fixture.configure_native_profile(profile)
        fixture.launcher.rename(stage/'launch.sh')
        entry = stage/'entry.sh'
        prefix = b'BILICOMICS_HOME=/mnt/us/' + run.encode()
        require(files['entry.sh'].count(prefix) == 1, 'Unexpected entry path')
        entry.write_bytes(files['entry.sh'].replace(prefix, b'BILICOMICS_HOME='+str(stage).encode()))
        fixture.launcher = entry
        (stage/'armed').mkdir(mode=0o700)
        shutil.copyfile(INPUTS/'clock-marks-host', stage/'clock-marks')
        (stage/'clock-marks').chmod(0o700)
        original = stage/'app/bilicomics'
        mock = fixture.root/'native-helper'
        original.rename(mock)
        fixture.script(original, ADAPTER)
        demo = stage/'data/demo'
        demo.mkdir(mode=0o700)
        with sqlite3.connect(demo/'state.sqlite3') as database:
            database.execute('CREATE TABLE preferences (account_key TEXT NOT NULL,key TEXT NOT NULL,payload TEXT NOT NULL,PRIMARY KEY(account_key,key))')
            database.execute('INSERT INTO preferences VALUES(?,?,?)', ('design-demo','preferences',json.dumps({'launch':'shelf'})))
        home = fixture.root/'home'
        home.mkdir(mode=0o700)
        fixture.env.update(AA_PROFILE=profile, AA_NATIVE_STUB=str(mock), AA_RUN=run, AA_DESKTOP=str(elf), BILICOMICS_HOME=str(stage), TEST_LAUNCHER=str(stage/'launch.sh'),
                           HOME=str(home), XDG_CONFIG_HOME=str(home/'config'), XDG_DATA_HOME=str(home/'data'), XDG_CACHE_HOME=str(home/'cache'), SLINT_SCALE_FACTOR='2', WINIT_UNIX_BACKEND='x11', SLINT_BACKEND='winit-software')
        checker = desktop_pairs.Sampler()
        clean.callback(checker.close)
        for method in ('XGrabServer','XUngrabServer','XFlush'):
            getattr(checker.api,method).argtypes = [ctypes.c_void_p]
            getattr(checker.api,method).restype = ctypes.c_int
        start = now()
        child = fixture.spawn_admission('--library')
        dispatched = now()
        last_nonmatch = start
        win = None
        app_pid = None
        try:
            deadline = time.monotonic() + 15
            while time.monotonic() < deadline and child.poll() is None:
                if app_pid is None:
                    receipt = mark(stage,'app_preinvoke',run)
                    if receipt:
                        candidate = receipt['pid']
                        try:
                            owned = os.path.samefile(f'/proc/{candidate}/exe', elf)
                        except (FileNotFoundError, ProcessLookupError):
                            owned = False
                        if owned:
                            worker = fixture.admission_worker_identity(child)
                            parent = fixture.process_identity(candidate)['parent']
                            require(worker and worker['pid'] == fixture.process_identity(parent)['parent'] and worker['recorded_parent'] == child.pid and fixture.admission_worker_ready(worker), 'App is not from the live owning Library worker')
                            shim = mark(stage,'shim_entry',run)
                            opened = (stage/'picked/entry').read_text().split()
                            require(shim and shim['pid'] == child.pid and opened == [run,str(child.pid),fixture.process_identity(child.pid)['birth']], 'Entry shim PID and live birth do not agree')
                            require(shim['monotonic_ns'] >= start and shim['monotonic_ns'] <= receipt['monotonic_ns'], 'Entry and app wrapper marks out of order')
                            app_pid = candidate
                if app_pid and win is None:
                    win = profiles.mapped_window(app_pid, size)
                if win is not None:
                    checker.api.XGrabServer(checker.display)
                    try:
                        for name, box in boxes.items():
                            span, pixels = checker.get(win, box)
                            if pixels != expected[name]:
                                last_nonmatch = span[0]
                                break
                        else:
                            matched = span[1]
                            break
                    finally:
                        checker.api.XUngrabServer(checker.display)
                        checker.api.XFlush(checker.display)
                time.sleep(.005)
            else:
                raise ValueError('Live desktop never reached the reference Shelf through both clock wrappers')
            child.communicate(timeout=18)
            require(child.returncode == 0 and fixture.live_admission_worker(fixture.admission_worker_identity(child)) is None and not (fixture.root/'lock').exists(), 'The fake platform was not cleanly retired')
            require(fixture.trace() == ['stop:statusbar','start:statusbar','xrefresh:'] and len(fixture.captures()) == 1, 'Missing single fixture takeover and restoration')
            records=list(map(json.loads,(stage/'data'/f'{run}-lifecycle.jsonl').read_text().splitlines()))
            require(all(row['process_id'] == app_pid for row in records), 'Lifecycle and clock marker PIDs disagree')
            mains=[row['monotonic_ns'] for row in records if row.get('stage')=='process_enter']
            require(len(mains)==1 and receipt['monotonic_ns'] <= mains[0] <= matched and last_nonmatch <= matched, 'No chronological matching Rust main and live Shelf')
            require([row.get('route') for row in records if row.get('event')=='ready']==['shelf'], 'Wrong same-process ready route')
            require(not (stage/'data'/f'{run}.jsonl').exists(), 'A native render log unexpectedly exists for the X11 application')
        finally:
            if child.poll() is None:
                fixture.finish_admission_entry(child)
    return {'model':profile, 'geometry':list(size), 'source_is_retained_stock':True, 'one_live_shell_and_birth':True, 'real_clock_wrapper_before_app_exec':True,
            'app_preinvoke_and_real_rust_main_pid_match':True, 'actual_window_pid_matches_main':True,'non_cover_rgb_regions':sorted(boxes),'normal_local_target':'shelf',
            'deltas_ns':{'dispatch_bracket_width':dispatched-start,'shim_to_app_preinvoke':receipt['monotonic_ns']-shim['monotonic_ns'],'app_preinvoke_to_rust_main':mains[0]-receipt['monotonic_ns'],
                         'rust_main_to_observed_x11_shelf':[max(0,last_nonmatch-mains[0]),matched-mains[0]]}, 'fake_native_restore_and_natural_exit':True,
            'card_release':None,'native_shelf_rendered':None,'native_update_submitted':None,'native_update_completed':None}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('profile', choices=['scribe','pw6'])
    parser.add_argument('attempt',type=int, choices=[1,2,3])
    args=parser.parse_args()
    os.umask(0o077)
    require(Path.cwd().resolve()==ROOT and [name for _,name in socket.if_nameindex()]==['lo'],'Expected an isolated loopback-only Ubuntu workspace')
    case=ROOT/'cases'/f'{args.profile}-{args.attempt}'
    case.mkdir(mode=0o700)
    try:
        report=process(args.profile,case,'bili-shelf-'+uuid.uuid4().hex)
        report['passed']=True
    except Exception as error:
        (case/'failed.txt').write_text(str(error)+'\n')
        raise
    with (case/'report.json').open('x') as stream:
        stream.write(json.dumps(report,indent=2)+'\n')
    print(json.dumps(report,sort_keys=True))


if __name__=='__main__':
    main()
