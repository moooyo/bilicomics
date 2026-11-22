"""Observe an ordinary visible Shelf and one real local XTEST click, without waiting for readiness or covers."""

import argparse
from contextlib import closing, contextmanager
import ctypes
import json
from pathlib import Path
import os
import shutil
import subprocess
import sys
import time

from PIL import Image

for sibling in ('startup-cache-maintenance-20261121', 'startup-direct-png-20261121', 'startup-cover-worker-20261121'):
    sys.path.append(str(Path(__file__).resolve().parent.parent / sibling))
import ordinary_pairs as ordinary
import profile_runner as cache
from desktop_pairs import ACTIVITY, TITLE, Sampler, now, one, region, require, rgb


ROOT = Path('/work')
BINARY = ROOT / 'study'
REFS = ROOT / 'interaction-refs'
SHELF_BAR = (80, 70, 1692, 130)
FIRST_TITLE = (96, 632, 300, 50)
MENU_RESPONSE = (1040, 632, 716, 70)
POINT = (1708, 136)
SCHEDULE = ['w00', *(f's{index:02d}' for index in range(1, 7))]
POLL_NS = 4_000_000


class XTestInput:
    def __init__(self, sampler):
        self.api = sampler.api
        self.xtst = ctypes.CDLL('libXtst.so.6')
        self.display = sampler.display
        display, word, integer = ctypes.c_void_p, ctypes.c_ulong, ctypes.c_int
        output = ctypes.POINTER
        self.api.XDefaultRootWindow.argtypes, self.api.XDefaultRootWindow.restype = [display], word
        self.api.XDefaultScreen.argtypes, self.api.XDefaultScreen.restype = [display], integer
        self.api.XSync.argtypes = [display, integer]
        self.api.XGetInputFocus.argtypes = [display, output(word), output(integer)]
        self.api.XQueryPointer.argtypes = [display, word, output(word), output(word), output(integer), output(integer), output(integer), output(integer), output(ctypes.c_uint)]
        self.xtst.XTestFakeMotionEvent.argtypes = [display, integer, integer, integer, word]
        self.xtst.XTestFakeButtonEvent.argtypes = [display, ctypes.c_uint, integer, word]
        self.root = self.api.XDefaultRootWindow(self.display)
        self.screen = self.api.XDefaultScreen(self.display)

    def sync(self, callback, *args):
        beginning = now()
        require(callback(self.display, *args), 'XTEST request failed')
        self.api.XSync(self.display, 0)
        return [beginning, now()]

    def move(self, point):
        return self.sync(self.xtst.XTestFakeMotionEvent, self.screen, *point, 0)

    def locate(self, xid, point):
        def query(window):
            parent, child = ctypes.c_ulong(), ctypes.c_ulong()
            root_x, root_y, local_x, local_y = [ctypes.c_int() for _ in range(4)]
            buttons = ctypes.c_uint()
            require(self.api.XQueryPointer(self.display, window, *[ctypes.byref(value) for value in (parent, child, root_x, root_y, local_x, local_y, buttons)]), 'Pointer is not on study screen')
            return parent.value, child.value, (root_x.value, root_y.value), (local_x.value, local_y.value), buttons.value
        root, top, root_pos, _, mask = query(self.root)
        local_root, _, _, local_pos, _ = query(xid)
        require(root == local_root == self.root and top == xid and root_pos == local_pos == tuple(point) and not (mask & 256), 'Pointer is not over the unoccluded owned window or is already pressed')
        focused, mode = ctypes.c_ulong(), ctypes.c_int()
        self.api.XGetInputFocus(self.display, ctypes.byref(focused), ctypes.byref(mode))
        label = {1: 'pointer-root', self.root: 'display-root', xid: 'owned-window'}.get(focused.value)
        require(label is not None, 'Foreign input focus')
        return label

    def down(self):
        return self.sync(self.xtst.XTestFakeButtonEvent, 1, 1, 0)

    def up(self):
        return self.sync(self.xtst.XTestFakeButtonEvent, 1, 0, 0)

    def button_down(self):
        parent, child = ctypes.c_ulong(), ctypes.c_ulong()
        coordinates = [ctypes.c_int() for _ in range(4)]
        mask = ctypes.c_uint()
        require(self.api.XQueryPointer(self.display, self.root, *[ctypes.byref(value) for value in (parent, child, *coordinates, mask)]), 'Pointer query failed')
        return bool(mask.value & 256)

    def tap(self, xid, point, hold=.016):
        self.move(point)
        focus = self.locate(xid, point)
        pressed = self.down()
        time.sleep(hold)
        released = self.up()
        return {'focus': focus, 'press_ns': pressed, 'release_ns': released}


@contextmanager
def process(directory, seed, exit_after_ms=1200):
    data = directory / 'data'
    if seed is not None:
        ordinary.private_copy(seed, data, True)
    home = directory / 'home'
    home.mkdir(mode=0o700)
    require('LD_PRELOAD' not in os.environ and 'BILICOMICS_DESKTOP_PRESENT_PROBE' not in os.environ, 'Unexpected experimental environment')
    env = dict(os.environ, HOME=str(home), SLINT_BACKEND='winit-software', WINIT_UNIX_BACKEND='x11', SLINT_SCALE_FACTOR='2',
               XDG_CACHE_HOME=str(home / 'cache'), XDG_CONFIG_HOME=str(home / 'config'), XDG_DATA_HOME=str(home / 'xdg-data'))
    args = [BINARY, '--offline', '--device', 'scribe', '--data-dir', data, '--lifecycle-trace', directory / 'lifecycle.jsonl',
            '--capture', directory / 'final.png', '--exit-after-ms', str(exit_after_ms)]
    with (directory / 'application.log').open('xb') as log:
        origin = now()
        child = subprocess.Popen(args, env=env, stdout=log, stderr=subprocess.STDOUT)
        try:
            require(os.path.samefile(f'/proc/{child.pid}/exe', BINARY), 'Wrong study executable')
            yield child, origin
            require(child.wait(15) == 0, 'Nonzero preview exit')
        finally:
            ordinary.observer.wait_child(child)


def reference(name, box):
    with Image.open(REFS / (name + '.png')) as image:
        require(image.size == box[2:], 'Incorrect reference geometry')
        return image.convert('RGB').tobytes()


def read_final(directory, pid):
    final = ordinary.load(directory / 'final.json')
    require(not (directory / 'performance.jsonl').exists() and final['process_id'] == pid and final['profile'] == 'scribe'
            and final['backend'] == 'desktop-software' and final['route'] == 'shelf' and final['demo'] is False
            and final['offline_requested'] is True and final['offline'] is True and final['startup']['booting'] is False
            and final['startup']['error'] is False and final['snapshot_size'] == [1860, 2480], 'Wrong ordinary final scene')
    catalog = final['ui']['catalog']
    require(catalog['shelf_view'] == 'grid' and catalog['shelf_page'] == 1 and catalog['shelf_pages'] == 2, 'Wrong final catalog')
    life = ordinary.records(directory / 'lifecycle.jsonl')
    require(life and all(row['process_id'] == pid for row in life), 'Mixed lifecycle trace')
    entered = one(life, lambda row: row['event'] == 'startup-stage' and row.get('stage') == 'process_enter', 'process entry')['monotonic_ns']
    ready = one(life, lambda row: row['event'] == 'ready', 'ordinary lifecycle ready')
    require(ready['route'] == 'shelf' and entered <= ready['monotonic_ns'], 'Unexpected lifecycle ready')
    return final, entered, ready


def watch(sampler, pointer, child, templates):
    xid = None
    title = activity = initial_negative = last_negative = None
    deadline = now() + 8_000_000_000
    while now() < deadline and child.poll() is None:
        tick = now()
        if xid is None:
            xid = ordinary.ordinary_window(child.pid)
        if xid is not None:
            if title is None:
                stamp, actual = sampler.get(xid, TITLE)
                if actual == templates['feedback']:
                    title = stamp
                else:
                    initial_negative = stamp
            if title is not None and activity is None:
                stamp, actual = sampler.get(xid, ACTIVITY)
                if actual == templates['activity']:
                    activity = stamp
            collected = {}
            for label, window, box, template in [('bar_ns', xid, SHELF_BAR, 'shelf-bar'), ('title_ns', xid, FIRST_TITLE, 'first-title'),
                                                ('root_bar_ns', pointer.root, SHELF_BAR, 'shelf-bar'), ('root_title_ns', pointer.root, FIRST_TITLE, 'first-title')]:
                stamp, actual = sampler.get(window, box)
                if actual != templates[template]:
                    last_negative = stamp
                    break
                collected[label] = stamp
            else:
                shelf = dict(collected, last_negative_ns=last_negative)
                break
        time.sleep(max(0, (tick + POLL_NS - now()) / 1e9))
    else:
        raise ValueError('No fresh owned and visible functional Shelf signature')
    prepress, closed = sampler.get(xid, MENU_RESPONSE)
    prepress_root, closed_root = sampler.get(pointer.root, MENU_RESPONSE)
    require(closed == closed_root == templates['closed-response'] and closed != templates['menu-response'], 'Menu was not stably closed before the first click')
    require(os.path.samefile(f'/proc/{child.pid}/exe', BINARY), 'Executable identity lost before the click')
    focus = pointer.locate(xid, POINT)
    press = pointer.down()
    require(pointer.button_down(), 'X server did not observe the primary button press')
    time.sleep(.016)
    release = pointer.up()
    require(not pointer.button_down(), 'X server did not observe the primary button release')
    negative = prepress_root
    while now() < deadline and child.poll() is None:
        tick = now()
        first, actual = sampler.get(xid, MENU_RESPONSE)
        if actual == templates['menu-response']:
            visible, actual_root = sampler.get(pointer.root, MENU_RESPONSE)
            if actual_root == templates['menu-response']:
                break
            negative = visible
        else:
            negative = first
        time.sleep(max(0, (tick + POLL_NS - now()) / 1e9))
    else:
        raise ValueError('No visible menu response to the single physical-button event pair')
    time.sleep(.08)
    confirmation, stable = sampler.get(xid, MENU_RESPONSE)
    require(stable == templates['menu-response'] and ordinary.ordinary_window(child.pid) == xid, 'Visible menu response did not persist on the owned window')
    return {'pid': child.pid, 'xid': xid, 'feedback': {'last_negative_ns': initial_negative, 'title_ns': title, 'activity_ns': activity},
            'shelf': shelf, 'input': {'point': list(POINT), 'focus': focus, 'press_ns': press, 'release_ns': release,
            'pid_xid_verified': True, 'topmost_target_verified': True, 'server_press_verified': True, 'server_release_verified': True, 'hold_target_ms': 16},
            'response': {'prepress_closed_ns': prepress, 'prepress_root_closed_ns': prepress_root, 'last_negative_ns': negative,
                         'first_match_ns': first, 'root_match_ns': visible, 'confirmation_ns': confirmation}}


def execute(name):
    directory = (ROOT / 'trial' if name in SCHEDULE else ROOT / 'admission') / name
    directory.mkdir(mode=0o700)
    splash = rgb(ROOT / 'startup-0.png')
    templates = {'feedback': region(splash, TITLE).tobytes(), 'activity': region(splash, ACTIVITY).tobytes(),
                 **{key: reference(key, box) for key, box in [('shelf-bar', SHELF_BAR), ('first-title', FIRST_TITLE),
                                                             ('closed-response', MENU_RESPONSE), ('menu-response', MENU_RESPONSE)]}}
    require(ordinary.ordinary_window(None) is None, 'Dirty X server')
    sampler = Sampler()
    try:
        pointer = XTestInput(sampler)
        pointer.move(POINT)
        with process(directory, ROOT / 'r-seed') as (child, origin):
            result = watch(sampler, pointer, child, templates)
    finally:
        sampler.close()
        if (directory / 'data').is_dir():
            shutil.rmtree(directory / 'data')
    final, entered, record = read_final(directory, child.pid)
    catalog = final['ui']['catalog']
    require(catalog['shelf_menu'] is True, 'A visible click did not leave the menu open at normal exit')
    resident, ready = record['reader_resources'], record['monotonic_ns']
    require(resident['cover_resident_count'] == 10 and resident['cover_resident_bytes'] == 5324800, 'Wrong ready cover inventory')
    title = result['feedback']['title_ns']
    require(origin <= entered <= result['shelf']['bar_ns'][0] and (title is None or entered <= title[0])
            and entered <= ready <= result['response']['confirmation_ns'][1], 'Different application and observer clocks')
    result.update(case=name, launcher_ns=origin, process_enter_ns=entered, final={'route': final['route'], 'normal_exit': True, 'menu_open': True,
                  'offline': final['offline'], 'demo': final['demo'], 'profile': final['profile'], 'backend': final['backend'],
                  'shelf_page': catalog['shelf_page'], 'shelf_pages': catalog['shelf_pages'], 'ready_covers': 10, 'resident_bytes': 5324800, 'lifecycle_ready_ns': ready})
    ordinary.save(directory / 'case.json', result)


def xvfb(name):
    try:
        subprocess.run([*ordinary.XVFB, sys.executable, '-B', __file__, name, '--inside'], check=True, timeout=40)
    finally:
        ordinary.observer.require_retired()
        require(not any(path.resolve(strict=False) == BINARY for path in Path('/proc').glob('[0-9]*/exe')), 'Application process remained live')


def main():
    os.umask(0o077)
    ordinary.observer.require_private_network()
    require(Path.cwd() == ROOT, 'Expected a private /work mount')
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=['seed', 'rehearse', 'trial', *SCHEDULE])
    parser.add_argument('--inside', action='store_true', help=argparse.SUPPRESS)
    args = parser.parse_args()
    name = args.action
    if args.inside:
        execute(name)
    elif name == 'seed':
        if not (ROOT / 'bootstrap/receipt.json').is_file():
            subprocess.run([sys.executable, '-B', ROOT / 'profile_runner.py', 'bootstrap'], check=True)
        require(ordinary.load(ROOT / 'bootstrap/receipt.json')['normal_exit'], 'No closed empty setup schema')
        subprocess.run([sys.executable, '-B', ROOT / 'profile_runner.py', 'make-seed'], check=True)
        (ROOT / 'setup').mkdir(mode=0o700)
        cache.start_xvfb('normalize')
        cache.seeds()
        cache.start_xvfb('qualify-r')
        with closing(cache.fixture.open_closed(ROOT / 'r-seed/state.sqlite3')) as connection:
            items = connection.execute(cache.SELECT, (cache.ACCOUNT, 'preferences')).fetchall()
            require(len(items) == 1, 'Expected one stored launch preference')
            preferences = json.loads(items[0][0])
            require(preferences['onboarded'] is True and preferences['launch'] == 'shelf', 'Profile does not naturally launch its Shelf')
        ordinary.save(ROOT / 'r-profile.json', {'product_written_recency': True, 'canonical_repeat_upsert_rows': 0,
                      'same_semantic_input': True, 'stored_launch_shelf': True, 'onboarded': True,
                      'private_fixture': True, 'terminal_cover_codes': list(range(1, 11))})
    elif name == 'rehearse':
        (ROOT / 'admission').mkdir(mode=0o700, exist_ok=True)
        xvfb(name)
    elif name == 'trial':
        require((ROOT / 'r-profile.json').is_file() and (REFS / 'qualified.json').is_file() and
                (ROOT / 'admission/rehearse/case.json').is_file(), 'Missing required visual and ordinary input qualification')
        trial = ROOT / 'trial'
        trial.mkdir(mode=0o700)
        results = {}
        for case in SCHEDULE:
            try:
                xvfb(case)
                results[case] = ordinary.load(trial / case / 'case.json')
            except Exception as error:
                ordinary.save(trial / 'failure.json', {'failed_case': case, 'error_type': type(error).__name__, 'completed': list(results)})
                raise
        ordinary.save(trial / 'results.json', {'schema': 'ordinary-shelf-interaction-v1', 'clock': 'CLOCK_MONOTONIC', 'poll_interval_ns': POLL_NS,
                      'schedule': SCHEDULE, 'excluded_warmups': SCHEDULE[:1], 'cases': results})
    else:
        raise ValueError('Individual formal cases must run under the fixed schedule')


if __name__ == '__main__':
    main()
