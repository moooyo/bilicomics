"""Compare two offline Scribe desktop executables on a private 24-bit Xvfb display."""

import argparse
import ctypes
import json
import os
from pathlib import Path
import re
import shutil
import sqlite3
import stat
from statistics import median
import subprocess
import sys
import time

from PIL import Image, ImageChops


ROOT = Path('/work')
SIZE = (1860, 2480)
TITLE = (490, 940, 880, 240)
ACTIVITY = (810, 1364, 240, 24)
SHELF_TEST = (0, 0, 640, 200)
FULL = (0, 0, *SIZE)
INTERVAL_NS = 4_000_000
SCHEDULE = ['a00', 'b00'] + [f'{arm}{pair:02d}' for pair in range(1, 7) for arm in ('ab' if pair % 2 else 'ba')]


def require(value, message):
    if not value:
        raise ValueError(message)


def now():
    return time.clock_gettime_ns(time.CLOCK_MONOTONIC)


def rgb(path):
    with Image.open(path) as source:
        require(source.size == SIZE, f'Unexpected reference size: {source.size}')
        return source.convert('RGB')


def region(image, box):
    left, top, width, height = box
    return image.crop((left, top, left + width, top + height))


def packed(image, box):
    return region(image, box).tobytes('raw', 'BGRX')


def visible_rgb(payload, size):
    return Image.frombytes('RGB', size, payload, 'raw', 'BGRX').tobytes()


def known_layout(layout, size):
    width, height = size
    return layout in [(width, height, 2, depth, 32, 0, 4 * width, 0xff0000, 0xff00, 0xff) for depth in (24, 32)]


class XImage(ctypes.Structure):
    _fields_ = [(name, ctypes.c_int) for name in ('width', 'height', 'xoffset', 'format')] + [('data', ctypes.c_void_p)] + [
        (name, ctypes.c_int) for name in ('byte_order', 'bitmap_unit', 'bitmap_bit_order', 'bitmap_pad', 'depth', 'bytes_per_line', 'bits_per_pixel')
    ] + [(name, ctypes.c_ulong) for name in ('red_mask', 'green_mask', 'blue_mask')]


class Sampler:
    def __init__(self):
        api = ctypes.CDLL('libX11.so.6')
        pointer = ctypes.c_void_p
        integer = ctypes.c_int
        unsigned = ctypes.c_uint
        word = ctypes.c_ulong
        api.XOpenDisplay.argtypes, api.XOpenDisplay.restype = [ctypes.c_char_p], pointer
        api.XCloseDisplay.argtypes = [pointer]
        api.XGetImage.argtypes = [pointer, word, integer, integer, unsigned, unsigned, word, integer]
        api.XGetImage.restype = ctypes.POINTER(XImage)
        api.XDestroyImage.argtypes, api.XDestroyImage.restype = [ctypes.POINTER(XImage)], integer
        self.api = api
        self.display = api.XOpenDisplay(None)
        require(self.display, 'Cannot open private X display')

    def close(self):
        self.api.XCloseDisplay(self.display)

    def get(self, window, box):
        left, top, width, height = box
        start = now()
        capture = self.api.XGetImage(self.display, window, left, top, width, height, 0xffffff, 2)
        end = now()
        require(capture, 'Window XGetImage failed')
        try:
            data = capture.contents
            layout = (data.width, data.height, data.format, data.depth, data.bits_per_pixel, data.byte_order,
                      data.bytes_per_line, data.red_mask, data.green_mask, data.blue_mask)
            require(known_layout(layout, (width, height)), f'Unexpected XImage layout: {layout}')
            return (start, end), visible_rgb(ctypes.string_at(data.data, width * height * 4), (width, height))
        finally:
            self.api.XDestroyImage(capture)


def window_id(pid):
    tree = subprocess.check_output(['xwininfo', '-root', '-tree'], text=True)
    ids = re.findall(r'(0x[0-9a-fA-F]+) "BiliComics - Design Preview"', tree)
    require(len(ids) <= 1, 'Multiple offline preview windows')
    if not ids:
        return None
    require(pid is not None, 'A preview window exists before launch')
    info = subprocess.check_output(['xwininfo', '-id', ids[0]], text=True)
    if 'Map State: IsViewable' not in info:
        return None
    if tuple(int(re.search(key + r':\s*(\d+)', info)[1]) for key in ('Width', 'Height')) != SIZE:
        return None
    identity = subprocess.check_output(['xprop', '-id', ids[0], '_NET_WM_PID'], text=True)
    require(re.search(rf'= {pid}\s*$', identity), f'Window does not identify process {pid}: {identity}')
    return int(ids[0], 16)


def wait_child(process):
    if process.poll() is None:
        process.terminate()
        try:
            process.wait(3)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(3)


def require_private_network():
    interfaces = Path('/proc/net/dev').read_text().splitlines()[2:]
    require({row.split(':', 1)[0].strip() for row in interfaces} == {'lo'}, 'Expected an isolated network namespace')


def require_retired():
    deadline = now() + 2_000_000_000
    while True:
        live = []
        for path in Path('/proc').glob('[0-9]*/stat'):
            try:
                comm, state = path.read_bytes().split(b' (', 1)[1].rsplit(b') ', 1)
            except (FileNotFoundError, ProcessLookupError):
                continue
            if comm in (b'Xvfb', b'control', b'candidate') and not state.startswith(b'Z '):
                live.append((path.parent.name, comm))
        if not live:
            return
        require(now() <= deadline, f'Child remained live in the private PID namespace: {live}')
        time.sleep(.02)


def seed_files(directory):
    require(not directory.is_symlink() and {path.name for path in directory.iterdir()} == {'demo'}, 'Not an isolated synthetic seed')
    files = []
    for path in directory.rglob('*'):
        mode = path.lstat().st_mode
        require(stat.S_ISREG(mode) or stat.S_ISDIR(mode), 'Seed contains a link or special file')
        if stat.S_ISREG(mode):
            require(not path.name.endswith(('-wal', '-journal', '-shm')), 'Seed contains live database sidecars')
            files.append(str(path.relative_to(directory)))
    require('demo/state.sqlite3' in files, 'Missing synthetic metadata seed')
    return sorted(files)


def seed():
    require_private_network()
    directory = ROOT / 'seed'
    directory.mkdir()
    data = directory / 'data'
    env = dict(os.environ, SLINT_BACKEND='winit-software', WINIT_UNIX_BACKEND='x11', SLINT_SCALE_FACTOR='2')
    with (directory / 'application.log').open('xb') as log:
        subprocess.run(['xvfb-run', '-a', '-s', '-screen 0 1860x2480x24 -nolisten tcp', 'timeout', '-k', '2s', '30s', ROOT / 'control', '--demo', '--offline',
                        '--screen', 'shelf', '--device', 'scribe', '--data-dir', data, '--capture', directory / 'final.png', '--exit-after-ms', '800'],
                       env=env, stdout=log, stderr=subprocess.STDOUT, timeout=40, check=True)
    require_retired()
    final = json.loads((directory / 'final.json').read_text())
    require(final['route'] == 'shelf' and final['demo'] and final['offline_requested'] and final['offline'] and not final['startup']['error']
            and final['ui']['catalog']['shelf_pages'] == 2, 'Seed preconditioning did not reach the offline fixture Shelf')
    require(ImageChops.difference(rgb(directory / 'final.png'), rgb(ROOT / 'references/final.png')).getbbox() is None, 'Seed Shelf differs')
    require((data / 'demo/state.sqlite3').is_file(), 'No fixture database to checkpoint')
    database = sqlite3.connect(data / 'demo/state.sqlite3')
    try:
        require(tuple(database.execute('PRAGMA wal_checkpoint(TRUNCATE)').fetchone()) == (0, 0, 0), 'Could not close the synthetic WAL')
    finally:
        database.close()
    files = seed_files(data)
    (directory / 'receipt.json').write_text(json.dumps({'preconditioning_only': True, 'arm': 'A', 'files': files}, indent=2) + '\n')
    print(f'Preconditioning complete: {len(files)} closed synthetic files; not a timing sample.', flush=True)


def reference_pixels(expected_splash, expected_shelf):
    shelf_probe = region(expected_shelf, SHELF_TEST).tobytes()
    require(region(expected_splash, SHELF_TEST).tobytes() != shelf_probe, 'Shelf prefilter does not distinguish splash')
    return region(expected_splash, TITLE).tobytes(), region(expected_splash, ACTIVITY).tobytes(), shelf_probe, expected_shelf.tobytes()


def watch(sampler, process, pixels, origin):
    window = None
    feedback = matching = previous_miss = settled = None
    last_no_feedback = (origin, origin)
    activities, periods, reads = [], [], []
    previous_poll = None
    splash_title, initial_bar_bytes, shelf_probe, shelf_pixels = pixels
    initial_bar = Image.frombytes('RGB', ACTIVITY[2:], initial_bar_bytes)
    while now() - origin < 12_000_000_000 and process.poll() is None:
        tick = now()
        if window is None:
            window = window_id(process.pid)
        if window is not None:
            if previous_poll is not None:
                periods.append(tick - previous_poll)
            previous_poll = tick
            if feedback is None:
                stamp, actual = sampler.get(window, TITLE)
                if actual == splash_title:
                    feedback = stamp
                else:
                    last_no_feedback = stamp
            if feedback and not matching:
                stamp, actual = sampler.get(window, ACTIVITY)
                if actual == initial_bar_bytes and 'initial' not in activities:
                    activities.append('initial')
                elif 'changed' not in activities and actual != initial_bar_bytes:
                    bar = Image.frombytes('RGB', initial_bar.size, actual)
                    if bar.histogram() == initial_bar.histogram() and ImageChops.difference(initial_bar, bar).getbbox():
                        activities.append('changed')
            stamp, actual = sampler.get(window, SHELF_TEST)
            if actual == shelf_probe:
                stamp, actual = sampler.get(window, FULL)
                reads.append(stamp[1] - stamp[0])
                if actual == shelf_pixels:
                    require(feedback and previous_miss, 'No earlier feedback and non-Shelf sample in this window')
                    if matching is None:
                        matching = stamp
                    else:
                        settled = stamp
                        break
                else:
                    require(matching is None, 'First complete Shelf did not remain complete')
                    previous_miss = stamp
            else:
                require(matching is None, 'First complete Shelf did not remain complete')
                previous_miss = stamp
        time.sleep(max(0, (tick + INTERVAL_NS - now()) / 1e9) if not matching else .08)
    require(all((window, feedback, previous_miss, matching, settled)), 'Missing startup or complete Shelf sample')
    require('initial' in activities, 'No splash activity pixels observed in the initial state')
    return {'pid': process.pid, 'xid': window, 'last_no_feedback_sample_ns': last_no_feedback, 'feedback_sample_ns': feedback, 'last_non_shelf_sample_ns': previous_miss,
            'first_exact_shelf_sample_ns': matching, 'second_exact_shelf_sample_ns': settled, 'activity_states': activities,
            'full_xgetimage_intervals_ns': reads, 'polls': len(periods) + 1,
            'poll_median_ns': int(median(periods[:-1] or periods)), 'poll_max_before_match_ns': max(periods[:-1] or periods)}


def one(rows, predicate, name):
    found = [row for row in rows if predicate(row)]
    require(len(found) == 1, f'Expected one {name}, got {len(found)}')
    return found[0]


def valid_x11_order(enter, feedback, miss, ready, match):
    return (enter <= feedback[0] <= feedback[1] <= match[0] <= match[1] and feedback[0] <= ready <= match[1]
            and feedback[0] <= miss[1] <= match[0] and miss[0] <= miss[1])


def case_metrics(path, observed):
    def load(name):
        return [json.loads(line) for line in (path / name).read_text().splitlines()]

    perf, life = load('performance.jsonl'), load('lifecycle.jsonl')
    pid = observed['pid']
    require(all(row['process_id'] == pid for row in perf + life), 'Mixed process records')
    ready = one(perf, lambda row: (row['kind'], row['stage']) == ('startup', 'ui_ready') and row['thread_id'] == pid, 'UI readiness')['monotonic_ns']
    prepared = one(perf, lambda row: (row['kind'], row['stage']) == ('startup', 'state_prepared') and row['thread_id'] == pid, 'UI state record')
    producer, consumer = prepared['data']['observed']['monotonic_ns'], prepared['monotonic_ns']
    require(one(life, lambda row: row.get('stage') == 'state_prepared', 'producer confirmation')['monotonic_ns'] == producer, 'Producer clocks disagree')
    enter = one(life, lambda row: row.get('stage') == 'process_enter', 'process entry')['monotonic_ns']
    require(enter < producer <= consumer <= ready and one(life, lambda row: row['event'] == 'ready', 'ready route')['route'] == 'shelf', 'Invalid startup state')
    results = {}
    for label, on_main, outcomes in (('state-covers', False, ['success']), ('attach-covers', True, ['partial', 'success'])):
        spans = [row for row in perf if (row['kind'], row['stage']) == ('startup', label) and row['monotonic_ns'] <= ready]
        require([row['data']['phase'] for row in spans] == ['begin', 'end'] * len(outcomes) and
                [row['data']['outcome'] for row in spans[1::2]] == outcomes, f'Unexpected spans: {label}')
        for begin, end in zip(spans[::2], spans[1::2]):
            require(begin['thread_id'] == end['thread_id'] and (begin['thread_id'] == pid) == on_main and
                    begin['monotonic_ns'] <= end['monotonic_ns'] and all(type(end['data'][key]) is int and end['data'][key] >= 0
                    for key in ('wall_us', 'cpu_us')), 'Invalid stage clock or thread')
        results[label] = {key: sum(row['data'][key] for row in spans[1::2]) for key in ('wall_us', 'cpu_us')}
        if on_main:
            require(consumer <= spans[0]['monotonic_ns'] <= spans[1]['monotonic_ns'] <= spans[2]['monotonic_ns'] <= spans[3]['monotonic_ns'] <= ready, 'Invalid attach ordering')
            results['attach_gap_ns'] = spans[2]['monotonic_ns'] - spans[1]['monotonic_ns']
            results['attach_outcomes'] = outcomes
        else:
            require(spans[-1]['monotonic_ns'] <= producer, 'Cover work continued past state production')
    feedback_start, feedback_end = observed['feedback_sample_ns']
    miss_start, miss_end = observed['last_non_shelf_sample_ns']
    match_start, match_end = observed['first_exact_shelf_sample_ns']
    require(valid_x11_order(enter, (feedback_start, feedback_end), (miss_start, miss_end), ready, (match_start, match_end)),
            'X11 observations did not bracket UI readiness and Shelf')
    results.update(process_ns=enter, ready_ns=ready, producer_ns=producer, ui_record_ns=consumer,
                   enter_to_feedback_bracket_ns=[max(enter, observed['last_no_feedback_sample_ns'][0]) - enter, feedback_end - enter],
                   enter_to_shelf_bracket_ns=[miss_start - enter, match_end - enter],
                   feedback_sample_to_producer_ns=[producer - feedback_end, producer - feedback_start],
                   producer_to_ui_ns=consumer - producer, ui_to_ready_ns=ready - consumer,
                   startup_activity_pulses=sum((row['kind'], row['stage']) == ('startup', 'startup_activity_pulse') for row in perf))
    return results


def execute_case(name, directory):
    directory.mkdir()
    source = ROOT / 'seed/data'
    receipt = json.loads((ROOT / 'seed/receipt.json').read_text())
    require(receipt['preconditioning_only'] and receipt['files'] == seed_files(source), 'Unexpected closed seed')
    copied = directory / 'data'
    shutil.copytree(source, copied)
    require(seed_files(copied) == receipt['files'] and all(not (source / leaf).samefile(copied / leaf)
            for leaf in receipt['files']), 'Writable case data aliases or differs from seed inventory')
    require(window_id(None) is None, 'Dirty X server')
    expected_splash, expected_shelf = [rgb(ROOT / 'references' / file) for file in ('startup-0.png', 'final-x11.png')]
    pixels = reference_pixels(expected_splash, expected_shelf)
    sampler = Sampler()
    command = [ROOT / ('control' if name[0] == 'a' else 'candidate'), '--demo', '--offline', '--screen', 'shelf', '--device', 'scribe',
               '--data-dir', directory / 'data', '--lifecycle-trace', directory / 'lifecycle.jsonl', '--performance-trace', directory / 'performance.jsonl',
               '--capture', directory / 'final.png', '--exit-after-ms', '800']
    env = dict(os.environ, SLINT_BACKEND='winit-software', WINIT_UNIX_BACKEND='x11', SLINT_SCALE_FACTOR='2')
    with (directory / 'application.log').open('xb') as log:
        origin = now()
        process = subprocess.Popen(command, env=env, stdout=log, stderr=subprocess.STDOUT)
        try:
            observed = watch(sampler, process, pixels, origin)
            try:
                stamp = now()
                status = Path(f'/proc/{process.pid}/status').read_text()
                memory = {key: int(value) for key, value in re.findall(r'^(VmRSS|VmHWM):\s*(\d+) kB$', status, re.M)}
                observed['post_confirmation_memory_kib'] = {'time_ns': stamp, **memory} if len(memory) == 2 else None
            except FileNotFoundError:
                observed['post_confirmation_memory_kib'] = None
            require(process.wait(15) == 0, 'Nonzero exit')
        finally:
            sampler.close()
            wait_child(process)
    final = json.loads((directory / 'final.json').read_text())
    require(final['process_id'] == process.pid and final['route'] == 'shelf' and final['demo'] and final['offline'] and final['offline_requested']
            and final['backend'] == 'desktop-software' and final['profile'] == 'scribe' and not final['startup']['booting']
            and not final['startup']['error'] and final['ui']['catalog']['shelf_pages'] == 2, 'Wrong final Shelf')
    require(ImageChops.difference(rgb(directory / 'final.png'), rgb(ROOT / 'references' / 'final.png')).getbbox() is None, 'Final logical RGB mismatch')
    observed['case'] = name
    observed['final_logical_rgb_matches'] = True
    observed['stages'] = case_metrics(directory, observed)
    (directory / 'result.json').write_text(json.dumps(observed, indent=2) + '\n')
    print(name, observed['stages']['enter_to_shelf_bracket_ns'], flush=True)


def experiment():
    require_private_network()
    out = ROOT / 'trial'
    out.mkdir()
    results = {}
    for name in SCHEDULE:
        attempt = subprocess.run(['xvfb-run', '-a', '-s', '-screen 0 1860x2480x24 -nolisten tcp', sys.executable, '-B', __file__, '--case', name], timeout=40)
        if attempt.returncode:
            (out / 'failure.json').write_text(json.dumps({'failed_case': name, 'returncode': attempt.returncode, 'completed_cases': list(results)}, indent=2) + '\n')
            sys.exit(attempt.returncode)
        results[name] = json.loads((out / name / 'result.json').read_text())
        require_retired()
    pairs = []
    for number in range(1, 7):
        control, candidate = [results[f'{arm}{number:02d}']['stages'] for arm in 'ab']
        low_a, high_a = control['enter_to_shelf_bracket_ns']
        low_b, high_b = candidate['enter_to_shelf_bracket_ns']
        pairs.append({'pair': number, 'order': 'AB' if number % 2 else 'BA', 'B_minus_A_shelf_bounds_ns': [low_b - high_a, high_b - low_a],
                      'B_minus_A_first_observed_shelf_ns': high_b - high_a,
                      'B_minus_A_worker_cpu_us': candidate['state-covers']['cpu_us'] - control['state-covers']['cpu_us'],
                      'B_minus_A_ui_attach_cpu_us': candidate['attach-covers']['cpu_us'] - control['attach-covers']['cpu_us']})
    (out / 'report.json').write_text(json.dumps({'clock': 'CLOCK_MONOTONIC', 'sample_interval_ns': INTERVAL_NS, 'schedule': SCHEDULE,
                           'seed': 'Separate first A launch in a new directory, closed synthetic demo seed copied without hard links before each run.',
                           'warmups_excluded': SCHEDULE[:2], 'cases': results, 'pairs': pairs}, indent=2) + '\n')


if __name__ == '__main__':
    os.umask(0o077)
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--case', choices=SCHEDULE)
    parser.add_argument('--prepare-seed', action='store_true')
    args = parser.parse_args()
    if args.prepare_seed:
        seed()
    elif args.case:
        execute_case(args.case, ROOT / 'trial' / args.case)
    else:
        experiment()
