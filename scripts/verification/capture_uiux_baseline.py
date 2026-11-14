#!/usr/bin/env python3
"""Capture reusable UI/UX scenes on test-env with real XTest pointer input.

Run inside a private 2800x2800 Xvfb display with a freshly built desktop binary.
Every invocation creates a new output directory and isolated demonstration data.
Existing baselines, physical Kindles, and live accounts are never opened.
"""

import argparse
import datetime
import hashlib
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import sys
import time
import traceback

from pointer_v2 import CONTEXT_KEYS, INPUT_DELIVERY_PROTOCOL, Pointer
from ui_refinement import PROFILES, matches, stop_owned_group, trace_records
from harness_signals import install_interruption_handlers

SCALE = 2


def write_json(path, value):
    path.write_text(json.dumps(value, ensure_ascii=False, indent=2) + '\n', encoding='utf-8')


class Session:
    def __init__(self, arguments, output, pointer, profile, name, route, landscape=False, offline=False):
        self.arguments = arguments
        self.output = output
        self.pointer = pointer
        self.profile = profile
        self.name = name
        self.route = route
        self.geometry = PROFILES[profile]
        self.logical_size = self.geometry['size']
        if landscape:
            self.logical_size = self.logical_size[::-1]
        self.landscape = landscape
        self.offline = offline
        self.directory = output / 'sessions' / (profile + '-' + name)
        self.directory.mkdir(parents=True, exist_ok=False)
        self.raw = output / 'raw' / profile
        self.raw.mkdir(parents=True, exist_ok=True)
        self.trace = self.directory / 'lifecycle.jsonl'
        self.process = None
        self.log = None
        self.window = None
        self.state = {}
        self.report = {'profile': profile, 'session': name, 'landscape': landscape,
                       'offline': offline, 'clicks': [], 'screenshots': [], 'complete': False}

    def records(self):
        return trace_records(self.trace)

    def latest(self, event, after=0):
        return next((row for row in reversed(self.records()[after:])
                     if row.get('event') == event and row.get('process_id') == self.process.pid), None)

    def wait(self, label, predicate, timeout=30):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if self.process.poll() is not None:
                raise RuntimeError('Application exited while waiting for ' + label)
            value = predicate()
            if value:
                return value
            time.sleep(0.05)
        latest = self.latest('ui-state')
        raise RuntimeError('Timed out waiting for ' + label + ': ' + json.dumps(latest, ensure_ascii=False))

    def observe(self, expected, after=0):
        def match():
            record = self.latest('ui-state', after)
            if not record or record.get('demo') is not True or record.get('view_model_fixture_only') is not False:
                return None
            return record if matches(record.get('state', {}), expected) else None
        record = self.wait('live UI ' + repr(expected), match)
        self.state = record['state']
        return record

    def reader_ready(self, expected=None):
        def match():
            row = self.latest('reader-ready')
            if not row or row.get('reader_loading') is not False:
                return None
            state = row.get('reader_state', {})
            frame = row.get('frame_size', [])
            if len(frame) != 2 or min(frame) <= 0 or state.get('rendering') is not False:
                return None
            return row if matches(state, expected) else None
        return self.wait('completed reader frame', match)

    def start(self):
        if hashlib.sha256(self.arguments.binary.read_bytes()).hexdigest() != self.arguments.expected_sha256:
            raise RuntimeError('The frozen application binary changed')
        environment = os.environ.copy()
        for key in CONTEXT_KEYS:
            environment.pop(key, None)
        environment['SLINT_SCALE_FACTOR'] = str(SCALE)
        command = [str(self.arguments.binary), '--demo', '--device', self.profile, '--screen', self.route,
                   '--data-dir', str(self.directory / 'data'), '--lifecycle-trace', str(self.trace),
                   '--capture', str(self.directory / 'final.png'), '--exit-after-ms', '240000']
        if self.landscape:
            command.append('--landscape')
        if self.offline:
            command.append('--offline')
        self.log = (self.directory / 'application.log').open('w', encoding='utf-8')
        self.process = subprocess.Popen(command, stdout=self.log, stderr=subprocess.STDOUT,
                                        env=environment, start_new_session=True)
        self.report.update({'command': command, 'process_id': self.process.pid, 'scale_factor': SCALE})
        self.report['ready_record'] = self.wait('same-process application readiness', lambda: self.latest('ready'), 60)
        self.observe({'route': self.route})
        self.window = self.wait('mapped application window', self.pointer.window)
        expected_size = tuple(value * SCALE for value in self.logical_size)
        if (self.window['width'], self.window['height']) != expected_size:
            raise RuntimeError('Unexpected capture size: ' + repr(self.window))
        self.report['window'] = self.window
        time.sleep(0.5)

    def snapshot(self, name, expected=None, reader=None):
        if expected:
            self.observe(expected)
        reader_record = self.reader_ready(reader) if self.state.get('route') == 'reader' else None
        target = self.raw / (name + '.png')
        if target.exists():
            raise RuntimeError('Refusing to overwrite an existing capture: ' + str(target))
        time.sleep(1.5)
        frames = self.directory / ('settle-' + name)
        frames.mkdir(exist_ok=False)
        last_hash = None
        stable_samples = 0
        started = time.monotonic()
        for sample in range(24):
            frame = frames / (str(sample).zfill(2) + '.png')
            image = self.pointer.capture(self.window, frame)
            stable_samples = stable_samples + 1 if image['sha256'] == last_hash else 1
            last_hash = image['sha256']
            if stable_samples >= 3:
                target.write_bytes(frame.read_bytes())
                break
            time.sleep(0.45)
        else:
            raise RuntimeError('The visible X11 pixels did not stabilize: ' + name)
        metadata = {'id': name, 'profile': self.profile, 'session': self.name,
                    'captured_at': datetime.datetime.now(datetime.timezone.utc).isoformat(),
                    'file': str(target.relative_to(self.output)),
                    'sha256': image['sha256'], 'width': self.window['width'], 'height': self.window['height'],
                    'logical_size': self.logical_size, 'scale_factor': SCALE,
                    'binary_sha256': self.arguments.expected_sha256, 'process_id': self.process.pid,
                    'source': image['source'], 'pixel_processing': 'None; untouched X11 RGB pixels',
                    'stability': {'minimum_settle_seconds': 1.5, 'identical_samples': stable_samples,
                                  'sampling_seconds': round(time.monotonic() - started, 3)},
                    'ui_record': self.latest('ui-state'), 'reader_record': reader_record,
                    'demo': True, 'live_account_used': False, 'physical_device_used': False}
        write_json(target.with_suffix('.json'), metadata)
        self.report['screenshots'].append(metadata)
        print(json.dumps({'captured': name, 'profile': self.profile,
                          'size': [self.window['width'], self.window['height']]}), flush=True)

    def click(self, name, point, expected=None, capture=None, reader=None):
        physical = tuple(round(value * SCALE) for value in point)
        if not (0 <= physical[0] < self.window['width'] and 0 <= physical[1] < self.window['height']):
            raise RuntimeError('Pointer target is outside the observed application window')
        prior = len(self.records())
        self.pointer.click(self.window, physical)
        self.report['clicks'].append({'name': name, 'logical_point': point, 'physical_point': physical,
                                     'window_id': self.window['id'], 'trace_records_before': prior})
        if expected:
            self.observe(expected, prior)
        if reader is not None:
            self.reader_ready(reader)
        if capture:
            self.snapshot(capture, reader=reader)

    def finish(self):
        os.kill(self.process.pid, signal.SIGTERM)
        if self.process.wait(timeout=25) != 0:
            raise RuntimeError('Application did not exit cleanly')
        final = json.loads((self.directory / 'final.json').read_text())
        if final.get('demo') is not True or final.get('backend') != 'desktop-software':
            raise RuntimeError('Final capture did not retain the expected renderer and fixture scope')
        self.report['final_metadata'] = final
        self.report['complete'] = True

    def close(self):
        self.report['cleanup'] = stop_owned_group(self.process)
        if self.log and not self.log.closed:
            self.log.close()
        write_json(self.directory / 'session.json', self.report)


def navigation(case):
    g = case.geometry
    wide = case.profile == 'scribe'
    width = g['size'][0]
    case.snapshot('01-shelf')
    case.click('Open shelf filter', g['filter'], {'catalog.shelf_menu': True}, '02-shelf-filter')
    case.click('Close shelf filter', g['filter_outside'], {'catalog.shelf_menu': False})
    case.click('Open bookstore', g['nav_store'], {'route': 'store'}, '03-store')
    case.click('Open search keyboard', g['search_left'], {'route': 'search', 'catalog.keyboard': True}, '04-search-keyboard')
    case.click('Search first recent term', (width / 2, 289 if wide else 227),
               {'route': 'search', 'catalog.keyboard': False}, '05-search-results')
    case.click('Open downloads', g['nav_downloads'], {'route': 'downloads'}, '07-downloads')
    case.click('Select downloads', g['download_select'], {'downloads.selecting': True})
    case.click('Select first task', g['download_first'], {'downloads.selected_count': 1}, '08-download-selection')
    case.click('Cancel selection', g['download_select'], {'downloads.selecting': False})
    case.click('Open account', g['nav_account'], {'route': 'account', 'account.section': 'account'}, '09-account')
    case.click('Open reading preferences', (257, 354) if wide else (width / 2, 202),
               {'account.section': 'reading'}, '10-account-reading')
    if wide:
        case.click('Open storage preferences', (535, 160), {'account.section': 'storage'}, '11-account-storage')
        case.click('Open download preferences', (396, 160), {'account.section': 'download'}, '12-account-download')
        case.click('Open system preferences', (674, 160), {'account.section': 'system'}, '13-account-system')
    else:
        case.click('Return to account', (60, 54), {'account.section': 'account'})
        case.click('Open storage preferences', (width / 2, 370), {'account.section': 'storage'}, '11-account-storage')
        case.click('Return to account again', (60, 54), {'account.section': 'account'})
        case.click('Open download preferences', (width / 2, 314), {'account.section': 'download'}, '12-account-download')


def reading(case):
    g = case.geometry
    wide = case.profile == 'scribe'
    case.snapshot('06-detail')
    case.click('Continue reading from details', (365, 341) if wide else (220, 201),
               {'route': 'reader', 'reader.loading': False}, reader={'page_index': 17, 'fit': 'page'})
    case.snapshot('14-reader-page', reader={'fit': 'page', 'presented_fit': 'page', 'menu': False})
    case.click('Open reading controls', g['center'], {'reader.menu': True, 'reader.sheet': ''},
               '15-reader-controls', {'menu': True, 'fit': 'page'})
    for index, fit in enumerate(('original', 'height', 'width')):
        fit_index = ('original', 'page', 'height', 'width').index(fit)
        case.click('Select fit ' + fit, (g['choice_x'][fit_index], g['zoom_y']),
                   {'reader.fit': fit}, reader={'fit': fit, 'presented_fit': fit})
        case.click('Hide controls for fit ' + fit, g['center'], {'reader.menu': False})
        case.snapshot(str(16 + index) + '-reader-' + fit, reader={'fit': fit, 'presented_fit': fit, 'menu': False})
        case.click('Show controls after fit ' + fit, g['center'], {'reader.menu': True})
    case.click('Restore page fit', (g['choice_x'][1], g['zoom_y']), {'reader.fit': 'page'},
               reader={'fit': 'page', 'presented_fit': 'page'})
    anchor_before = case.reader_ready()['reader_state']
    case.click('Open full chapter directory', (g['choice_x'][0], g['tool_y']),
               {'reader.sheet': 'toc'}, '19-chapter-directory', {'sheet': 'toc', 'page_index': 17})
    case.click('Return from directory', g['directory_back'], {'reader.menu': False, 'reader.sheet': ''},
               reader={'sheet': '', 'page_index': 17})
    anchor_after = case.reader_ready()['reader_state']
    anchor_keys = ('comic_id', 'episode_id', 'page_id', 'page_index', 'source_x', 'source_y', 'fit')
    before = {key: anchor_before.get(key) for key in anchor_keys}
    after = {key: anchor_after.get(key) for key in anchor_keys}
    if before != after:
        raise RuntimeError('The chapter directory changed the reading anchor')
    case.report['directory_anchor'] = {'before': before, 'after': after, 'preserved': True}
    for name, tool_index, sheet in (('20-reader-settings', 1, 'display'), ('21-reader-light', 3, 'light')):
        case.click('Open controls for ' + sheet, g['center'], {'reader.menu': True})
        case.click('Open sheet ' + sheet, (g['choice_x'][tool_index], g['tool_y']),
                   {'reader.sheet': sheet}, name, {'sheet': sheet})
        case.click('Close sheet ' + sheet, (g['size'][0] / 2, 160), {'reader.menu': False, 'reader.sheet': ''})


def simple(case, name):
    case.snapshot(name)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--expected-sha256', required=True)
    parser.add_argument('--source-commit', required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--stages', nargs='+', choices=('navigation', 'reading', 'extras'),
                        default=['navigation', 'reading', 'extras'])
    parser.add_argument('--profiles', nargs='+', choices=list(PROFILES), default=['pw6', 'scribe'])
    args = parser.parse_args()
    if not sys.platform.startswith('linux'):
        parser.error('Run this capture harness only on remote Linux test-env')
    if not re.fullmatch(r'[0-9a-f]{64}', args.expected_sha256):
        parser.error('Supply the exact lowercase executable SHA-256')
    if not re.fullmatch(r'[0-9a-f]{40}', args.source_commit):
        parser.error('Supply the captured source commit identity')
    if not args.output.is_absolute():
        parser.error('Supply an absolute fresh output directory')
    args.binary = args.binary.resolve(strict=True)
    if not args.binary.is_file() or not os.access(args.binary, os.X_OK):
        parser.error('The selected desktop executable is unavailable')
    if hashlib.sha256(args.binary.read_bytes()).hexdigest() != args.expected_sha256:
        parser.error('The executable does not match the selected build SHA-256')
    args.stages = list(dict.fromkeys(args.stages))
    args.profiles = list(dict.fromkeys(args.profiles))
    output = args.output.resolve()
    os.umask(0o077)
    output.mkdir(mode=0o700, parents=True, exist_ok=False)
    install_interruption_handlers()
    pointer = None
    report = {'stages': args.stages, 'binary_sha256': args.expected_sha256, 'source_commit': args.source_commit,
              'renderer': 'desktop-software', 'scale_factor': SCALE, 'input_protocol': INPUT_DELIVERY_PROTOCOL,
              'harness_sha256': hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
              'physical_device_used': False, 'live_account_used': False, 'sessions': [], 'errors': [],
              'visual_review_status': 'pending'}
    try:
        pointer = Pointer()
        for profile in args.profiles:
            plans = []
            if 'navigation' in args.stages:
                plans.append(('navigation', 'shelf', navigation, False, False))
            if 'reading' in args.stages:
                plans.append(('reading', 'detail', reading, False, False))
            if 'extras' in args.stages:
                plans.extend([
                    ('welcome', 'welcome', lambda case: simple(case, '22-welcome'), False, False),
                    ('login', 'login', lambda case: simple(case, '23-login'), False, False),
                    ('purchase', 'purchase', lambda case: simple(case, '24-purchase'), False, False),
                    ('landscape', 'reader', lambda case: simple(case, '25-reader-landscape'), True, False),
                    ('offline', 'store', lambda case: simple(case, '26-store-offline'), False, True),
                ])
            for name, route, callback, landscape, offline in plans:
                case = Session(args, output, pointer, profile, name, route, landscape, offline)
                try:
                    case.start()
                    callback(case)
                    case.finish()
                except Exception as error:
                    case.report['error'] = str(error)
                    report['errors'].append({'profile': profile, 'session': name, 'error': str(error)})
                    print(traceback.format_exc(), flush=True)
                finally:
                    case.close()
                    report['sessions'].append(case.report)
                    write_json(output / 'capture-report.json', report)
    finally:
        if pointer is not None:
            pointer.close()
        write_json(output / 'capture-report.json', report)
    print(json.dumps({'stages': args.stages, 'screenshots': sum(len(case['screenshots']) for case in report['sessions']),
                      'errors': len(report['errors'])}), flush=True)
    return 1 if report['errors'] else 0


if __name__ == '__main__':
    raise SystemExit(main())
