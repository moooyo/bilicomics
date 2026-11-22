"""Qualify and compare warmed, ordinary, offline Shelf launches on private Scribe Xvfb servers."""

import argparse
from contextlib import closing, contextmanager
import json
import os
from pathlib import Path
import re
import shutil
import sqlite3
import stat
import subprocess
import sys
import time

from PIL import Image, ImageChops, ImageGrab

sys.path.append(str(Path(__file__).resolve().parent.parent / 'startup-cover-worker-20261121'))
import desktop_pairs as observer
from desktop_pairs import FULL, INTERVAL_NS, ROOT, SCHEDULE, SIZE, Sampler, now, one, require, rgb


FIXTURE = ROOT / 'direct-png-fixture'
POSITIVE, NEGATIVE = FIXTURE / 'data', FIXTURE / 'guest'
MANIFEST = FIXTURE / 'seed.json'
REFERENCES = ROOT / 'references'
QUALIFICATION = ROOT / 'qualification'
SHIM = ROOT / 'fixed-wall.so'
SESSION = Path('account/private/session.json')
ARMS = {'A': ROOT / 'control', 'B': ROOT / 'candidate'}
SOURCES = {'A': '9175002 + retained catalog inputs; control startup path',
           'B': 'same source and release settings as A + managed startup PNG direct decode candidate'}
XVFB = ['xvfb-run', '-a', '-s', '-screen 0 1860x2480x24 -nolisten tcp']


def load(path):
    return json.loads(path.read_text(encoding='utf-8'))


def save(path, data):
    with path.open('x', encoding='utf-8') as output:
        json.dump(data, output, indent=2)
        output.write('\n')


def records(path):
    return [json.loads(line) for line in path.read_bytes().splitlines()]


def ordinary_window(pid):
    tree = subprocess.check_output(['xwininfo', '-root', '-tree'], text=True)
    require('"BiliComics - Design Preview"' not in tree, 'Unexpected demo window')
    windows = re.findall(r'(0x[0-9a-fA-F]+) "BiliComics"', tree)
    require(len(windows) <= 1, 'Multiple ordinary application windows')
    if not windows:
        return None
    require(pid is not None, 'An application window exists before launch')
    info = subprocess.check_output(['xwininfo', '-id', windows[0]], text=True)
    if ('Map State: IsViewable' not in info or
            tuple(int(re.search(key + r':\s*(\d+)', info)[1]) for key in ('Width', 'Height')) != SIZE):
        return None
    require('Visual Class: TrueColor' in info, 'Expected a TrueColor application window')
    identity = subprocess.check_output(['xprop', '-id', windows[0], '_NET_WM_PID'], text=True)
    require(re.search(rf'= {pid}\s*$', identity), 'Ordinary window has the wrong process ID')
    return int(windows[0], 16)


observer.window_id = ordinary_window


def check_fixture_tree(directory, signed_in):
    expected_files = {Path('state.sqlite3'), *(Path(cover['file']) for cover in load(MANIFEST)['covers'])}
    expected_dirs = {Path('covers')}
    if signed_in:
        expected_files.add(SESSION)
        expected_dirs.update(SESSION.parents[:-1])
    require(len(expected_files) == 21 + signed_in, 'Missing synthetic fixture entries')
    files, directories = set(), set()
    require(not directory.is_symlink() and directory.is_dir() and directory.stat().st_mode & 0o077 == 0, 'Unsafe fixture root')
    for path in directory.rglob('*'):
        metadata = path.lstat()
        require(metadata.st_mode & 0o077 == 0 and metadata.st_uid == os.geteuid(), 'Expected a private synthetic fixture object')
        if stat.S_ISREG(metadata.st_mode):
            require(metadata.st_nlink == 1, 'Unexpected alias in the closed fixture')
            files.add(path.relative_to(directory))
        else:
            require(stat.S_ISDIR(metadata.st_mode), 'Unexpected link or special file in the fixture')
            directories.add(path.relative_to(directory))
    require(files == expected_files and directories == expected_dirs, 'Unexpected files in the closed fixture')
    return files


def private_copy(seed, destination, signed_in):
    files = check_fixture_tree(seed, signed_in)
    shutil.copytree(seed, destination)
    require(check_fixture_tree(destination, signed_in) == files and all(not (seed / leaf).samefile(destination / leaf)
            and (seed / leaf).stat().st_size == (destination / leaf).stat().st_size for leaf in files), 'Aliased or different case copy')


def make_seed():
    require(load(ROOT / 'bootstrap/receipt.json')['normal_exit'], 'No closed schema')
    FIXTURE.mkdir(mode=0o700)
    (FIXTURE / '.fixture-only').write_text('BILICOMICS DIRECT PNG OFFLINE FIXTURE V1\n', encoding='ascii')
    subprocess.run([sys.executable, '-B', ROOT / 'make_fixture.py', ROOT / 'bootstrap/data', POSITIVE, MANIFEST], check=True)
    subprocess.run([ROOT / 'seed-profile', 'encode'], cwd=ROOT, check=True)
    private_copy(POSITIVE, NEGATIVE, False)
    subprocess.run([ROOT / 'seed-profile', 'identity'], cwd=ROOT, check=True)
    check_fixture_tree(POSITIVE, True)
    check_fixture_tree(NEGATIVE, False)
    profile = load(MANIFEST)['profiles']['scribe']['shelf']
    require(profile['prepared_covers'] == profile['visible_covers_with_fixture_identity'] == 10
            and profile['visible_covers_without_identity'] == 0 and profile['pages'] == 2
            and profile['attachment_batches'] == [8, 2] and profile['prepared_rgba_bytes'] == 5324800, 'Unexpected Shelf profile')
    save(FIXTURE / 'receipt.json', {'closed': True, 'same_catalog_and_covers_before_identity': True,
                                    'production_encoded': True, 'synthetic_only': True, 'profile': profile})


@contextmanager
def launch(directory, arm, duration):
    require(ordinary_window(None) is None and 'LD_PRELOAD' not in os.environ, 'Dirty observer environment')
    home = directory / 'home'
    home.mkdir(mode=0o700)
    env = dict(os.environ, HOME=str(home), LD_PRELOAD=str(SHIM), SLINT_BACKEND='winit-software',
               WINIT_UNIX_BACKEND='x11', SLINT_SCALE_FACTOR='2', XDG_CONFIG_HOME=str(home / 'config'),
               XDG_CACHE_HOME=str(home / 'cache'), XDG_DATA_HOME=str(home / 'data'))
    command = [ARMS[arm], '--offline', '--screen', 'shelf', '--device', 'scribe', '--data-dir', directory / 'data',
               '--lifecycle-trace', directory / 'lifecycle.jsonl', '--performance-trace', directory / 'performance.jsonl',
               '--capture', directory / 'final.png', '--exit-after-ms', str(duration)]
    with (directory / 'application.log').open('xb') as log:
        origin = now()
        process = subprocess.Popen(command, env=env, stdout=log, stderr=subprocess.STDOUT)
        try:
            require(os.path.samefile(f'/proc/{process.pid}/exe', ARMS[arm]), 'Selected arm differs from the running executable')
            yield process, origin
            require(process.wait(15) == 0, 'Application exited abnormally')
        finally:
            observer.wait_child(process)


def check_final(directory, pid, populated=True):
    final = load(directory / 'final.json')
    require(final['process_id'] == pid and final['route'] == 'shelf' and final['demo'] is False
            and final['offline_requested'] is True and final['offline'] is True and final['backend'] == 'desktop-software'
            and final['profile'] == 'scribe' and not final['startup']['booting'] and not final['startup']['error']
            and final['snapshot_size'] == list(SIZE), 'Wrong final control or candidate')
    performance, life = (records(directory / leaf) for leaf in ('performance.jsonl', 'lifecycle.jsonl'))
    require(all(row['process_id'] == pid for row in performance + life), 'Mixed application traces')
    context = one(performance, lambda row: row['kind'] == 'startup' and row['stage'] == 'state-prepare-context', 'ordinary context')
    require(context['data']['demo'] is False and context['data']['offline'] is True
            and not any(row['stage'].startswith('state-demo-') for row in performance), 'Wrong worker context')
    if not populated:
        return
    catalog = final['ui']['catalog']
    require(catalog['shelf_view'] == 'grid' and catalog['shelf_page'] == 1 and catalog['shelf_pages'] == 2, 'Wrong Shelf page')
    ready = one(life, lambda row: row['event'] == 'ready', 'ordinary ready record')
    resident = ready['reader_resources']
    require(ready['route'] == 'shelf' and resident['cover_resident_count'] == 10 and resident['cover_resident_bytes'] == 5324800,
            'Wrong prepared cover inventory')
    for name, outcomes in (('state-covers', ['success']), ('attach-covers', ['partial', 'success'])):
        spans = [row for row in performance if row['kind'] == 'startup' and row['stage'] == name]
        require([row['data']['phase'] for row in spans] == ['begin', 'end'] * len(outcomes) and
                [row['data']['outcome'] for row in spans[1::2]] == outcomes, 'Wrong cover span sequence')


def marker_codes(image):
    require(image.size == SIZE and image.mode == 'RGB', 'Marker oracle requires unmasked Scribe RGB pixels')
    covers = load(MANIFEST)['covers'][:10]
    result = []
    for slot, cover in enumerate(covers):
        row, column = divmod(slot, 5)
        center = (48 + (150 + 21) * column + 75, 32 + 72 + 4 + 292 * row + 100)
        factor = max(148 / cover['width'], 198 / cover['height'])
        bits = []
        for sample in cover['marker']['samples']:
            source_x, source_y = sample['xy']
            target_x = round(2 * (center[0] + (source_x - cover['width'] / 2) * factor))
            target_y = round(2 * (center[1] + (source_y - cover['height'] / 2) * factor))
            extrema = image.crop((target_x - 2, target_y - 2, target_x + 3, target_y + 3)).getextrema()
            bits.append(next((bit for bit, level in enumerate((32, 224))
                              if all(level - 8 <= low <= high <= level + 8 for low, high in extrema)), None))
        result.append(sum(bit << (4 - position) for position, bit in enumerate(bits)) if None not in bits else None)
    return result


def control_capture(directory, signed_in):
    sampler = Sampler()
    try:
        with launch(directory, 'A', 2400) as (process, _):
            deadline = now() + 10_000_000_000
            window = None
            while not window and now() < deadline and process.poll() is None:
                window = ordinary_window(process.pid)
                if not window:
                    time.sleep(.005)
            require(window, 'No mapped owned ordinary window')
            while now() < deadline and process.poll() is None:
                path = directory / 'lifecycle.jsonl'
                if path.exists() and any(json.loads(line).get('event') == 'ready' for line in path.read_bytes().splitlines(keepends=True)
                                         if line.endswith(b'\n')):
                    break
                time.sleep(.01)
            else:
                raise ValueError('No ordinary Ready before control qualification')
            time.sleep(.3)
            _, first = sampler.get(window, FULL)
            time.sleep(.08)
            _, second = sampler.get(window, FULL)
            require(first == second, 'Reference window is not stable')
            window_rgb = Image.frombytes('RGB', SIZE, second)
            info = subprocess.check_output(['xwininfo', '-id', hex(window)], text=True)
            left, top = (int(re.search(key + r':\s*(-?\d+)', info)[1]) for key in ('Absolute upper-left X', 'Absolute upper-left Y'))
            root_crop = ImageGrab.grab(bbox=(left, top, left + SIZE[0], top + SIZE[1]), xdisplay=os.environ['DISPLAY']).convert('RGB')
            require(ImageChops.difference(window_rgb, root_crop).getbbox() is None, 'Root crop disagrees with visible owned window')
    finally:
        sampler.close()
    check_final(directory, process.pid)
    logical = rgb(directory / 'final.png')
    visible_codes, logical_codes = marker_codes(window_rgb), marker_codes(logical)
    expected = list(range(1, 11)) if signed_in else [None] * 10
    save(directory / 'markers.json', {'visible': visible_codes, 'logical': logical_codes, 'expected': expected})
    require(visible_codes == logical_codes == expected, 'Unexpected cover codes in the actual visible/logical Shelf')
    if signed_in:
        for label, image in (('final-x11.png', window_rgb), ('final.png', logical)):
            with (REFERENCES / label).open('xb') as output:
                image.save(output, format='PNG')
    else:
        require(window_rgb.tobytes() != rgb(REFERENCES / 'final-x11.png').tobytes()
                and logical.tobytes() != rgb(REFERENCES / 'final.png').tobytes(), 'Positive endpoint accepted a signed-out Shelf')
        prompt_box = (320, 740, 1540, 1700)
        prompt = rgb(ROOT / 'bootstrap/final.png').crop(prompt_box)
        require(prompt.convert('L').histogram()[0] > 5000 and ImageChops.difference(logical.crop(prompt_box), prompt).getbbox() is None,
                'Populated signed-out Shelf does not show the actual empty-control login prompt')
    return {'pid': process.pid, 'xid': window, 'normal_exit': True, 'selected_arm': 'A', 'process_executable_matches': True,
            'marker_codes_x11': visible_codes, 'marker_codes_logical': logical_codes, 'full_window_repeated': True,
            'visible_root_crop_matches': True, 'route': 'shelf', 'page': 1, 'pages': 2, 'prepared_covers': 10,
            'resident_bytes': 5324800, 'attach_outcomes': ['partial', 'success'], 'fixture_identity_present': signed_in,
            'matches_empty_control_login_prompt': True if not signed_in else None}


def observe(directory, name, arm):
    pixels = observer.reference_pixels(rgb(REFERENCES / 'startup-0.png'), rgb(REFERENCES / 'final-x11.png'))
    sampler = Sampler()
    try:
        with launch(directory, arm, 800) as (process, origin):
            observed = observer.watch(sampler, process, pixels, origin)
            try:
                stamp = now()
                status = Path(f'/proc/{process.pid}/status').read_text()
                memory = {key: int(value) for key, value in re.findall(r'^(VmRSS|VmHWM):\s*(\d+) kB$', status, re.M)}
                observed['post_confirmation_memory_kib'] = {'time_ns': stamp, **memory} if len(memory) == 2 else None
            except FileNotFoundError:
                observed['post_confirmation_memory_kib'] = None
    finally:
        sampler.close()
    check_final(directory, process.pid)
    logical = rgb(directory / 'final.png')
    require(ImageChops.difference(logical, rgb(REFERENCES / 'final.png')).getbbox() is None and
            marker_codes(logical) == list(range(1, 11)), 'Final ordinary logical image or distinct markers differ')
    observed.update(case=name, selected_arm=arm, process_executable_matches=True, final_logical_rgb_matches=True,
                    visible_marker_count=10, ready_resident_covers=10, ready_resident_bytes=5324800)
    observed['stages'] = observer.case_metrics(directory, observed)
    return observed


def run_inside(name):
    directory = (ROOT / 'trial' if name in SCHEDULE else QUALIFICATION) / name
    if name == 'bootstrap':
        directory = ROOT / 'bootstrap'
    directory.mkdir(mode=0o700)
    if name == 'bootstrap':
        with launch(directory, 'A', 800) as (process, _):
            pass
        check_final(directory, process.pid, populated=False)
        return
    signed_in = name != 'negative'
    private_copy(POSITIVE if signed_in else NEGATIVE, directory / 'data', signed_in)
    try:
        if name in ('reference', 'negative'):
            result = control_capture(directory, signed_in)
        else:
            arm = name[-1].upper() if name.startswith('check-') else name[0].upper()
            result = observe(directory, name, arm)
        save(directory / 'result.json', result)
    finally:
        shutil.rmtree(directory / 'data')


def xvfb(name):
    try:
        subprocess.run([*XVFB, sys.executable, '-B', __file__, name, '--inside'], check=True, timeout=45)
    finally:
        observer.require_retired()


def qualification_summary():
    signed_in, signed_out = (load(QUALIFICATION / name / 'result.json') for name in ('reference', 'negative'))
    checks = {}
    for arm in ARMS:
        case = load(QUALIFICATION / f'check-{arm.lower()}' / 'result.json')
        require(case['selected_arm'] == arm and case['process_executable_matches'] and case['final_logical_rgb_matches']
                and case['visible_marker_count'] == 10 and case['stages']['attach_outcomes'] == ['partial', 'success'],
                'Missing exact natural qualification')
        checks[arm] = {'pid': case['pid'], 'xid': case['xid'], 'selected_executable_matches': True,
                       'binary_bytes': ARMS[arm].stat().st_size, 'normal_exit': True, 'initial_activity_matched': True,
                       'first_and_confirming_full_window_matches': True, 'final_logical_rgb_matches': True,
                       'ready_resident_covers': case['ready_resident_covers'], 'ready_resident_bytes': case['ready_resident_bytes']}
    covers = load(MANIFEST)['covers']
    require(len(covers) == 20 and load(FIXTURE / 'receipt.json')['production_encoded'], 'Unqualified cache fixture')
    save(QUALIFICATION / 'summary.json', {'scope': 'Untimed qualification only; none of these launches are timed samples.',
         'empty_control_schema': load(ROOT / 'bootstrap/receipt.json'), 'clock': load(ROOT / 'wall-qualification.json'),
         'seed': {'synthetic_covers': 20, 'width': 320, 'height': 416, 'straight_rgba8': True,
                  'input_file_bytes': sum(cover['encoded_bytes'] for cover in covers),
                  'production_encoded_file_bytes': sum((NEGATIVE / cover['file']).stat().st_size for cover in covers),
                  'all_reencoded_pixels_equal': True, 'copied_signed_out_seed_before_mock_identity': True,
                  'no_network_or_real_identity': True},
         'positive_reference': signed_in, 'signed_out_negative': signed_out, 'natural_confirmations': checks})


def run_trial():
    for name in ('reference', 'negative', 'check-a', 'check-b'):
        require((QUALIFICATION / name / 'result.json').is_file(), 'Missing required qualification')
    out = ROOT / 'trial'
    out.mkdir(mode=0o700)
    results = {}
    for name in SCHEDULE:
        try:
            xvfb(name)
            results[name] = load(out / name / 'result.json')
        except Exception as error:
            save(out / 'failure.json', {'failed_case': name, 'error_type': type(error).__name__, 'completed_cases': list(results)})
            raise
    pairs = []
    for number in range(1, 7):
        control, candidate = [results[f'{arm}{number:02d}']['stages'] for arm in 'ab']
        low_a, high_a = control['enter_to_shelf_bracket_ns']
        low_b, high_b = candidate['enter_to_shelf_bracket_ns']
        pairs.append({'pair': number, 'order': 'AB' if number % 2 else 'BA', 'B_minus_A_shelf_bounds_ns': [low_b - high_a, high_b - low_a],
                      'B_minus_A_first_observed_shelf_ns': high_b - high_a,
                      'B_minus_A_worker_cpu_us': candidate['state-covers']['cpu_us'] - control['state-covers']['cpu_us'],
                      'B_minus_A_ui_attach_cpu_us': candidate['attach-covers']['cpu_us'] - control['attach-covers']['cpu_us']})
    save(out / 'report.json', {'clock': 'CLOCK_MONOTONIC', 'sample_interval_ns': INTERVAL_NS, 'schedule': SCHEDULE,
                               'sources': SOURCES, 'warmups_excluded': SCHEDULE[:2], 'cases': results, 'pairs': pairs,
                               'scope': 'Warmed ordinary offline synthetic Shelf; ten distinct managed cover PNGs visible.'})


def main():
    os.umask(0o077)
    observer.require_private_network()
    require(Path.cwd() == ROOT and load(ROOT / 'wall-qualification.json')['external_parent_realtime_and_monotonic_advanced'], 'Wrong isolated workspace')
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=['bootstrap', 'make-seed', 'reference', 'negative', 'check-a', 'check-b', 'qualification-summary', 'trial', *SCHEDULE])
    parser.add_argument('--inside', action='store_true', help=argparse.SUPPRESS)
    args = parser.parse_args()
    if args.inside:
        require(args.action not in ('make-seed', 'qualification-summary', 'trial'), 'Invalid Xvfb action')
        run_inside(args.action)
    elif args.action == 'make-seed':
        make_seed()
    elif args.action == 'qualification-summary':
        qualification_summary()
    elif args.action == 'trial':
        run_trial()
    else:
        require(args.action not in SCHEDULE, 'Timed cases are launched only by the fixed schedule')
        QUALIFICATION.mkdir(exist_ok=True, mode=0o700)
        xvfb(args.action)
        if args.action == 'bootstrap':
            with closing(sqlite3.connect(ROOT / 'bootstrap/data/state.sqlite3')) as connection:
                require(connection.execute('PRAGMA wal_checkpoint(TRUNCATE)').fetchone() == (0, 0, 0), 'Could not close the empty control database')
            save(ROOT / 'bootstrap/receipt.json', {'normal_exit': True, 'schema_from_control': True, 'isolated_network': True})


if __name__ == '__main__':
    main()
