"""Run the desktop bootstrap-wake comparison with the qualified ordinary Shelf fixture and sampler."""

from contextlib import contextmanager
import os
from pathlib import Path
import shutil
import subprocess
import sys

for sibling in ('startup-direct-png-20261121', 'startup-cover-worker-20261121'):
    sys.path.append(str(Path(__file__).resolve().parent.parent / sibling))
import ordinary_pairs as ordinary
from desktop_pairs import ACTIVITY, one, region, require, rgb


SOURCES = {'A': 'Shared research ELF; observe the first full software submit and its deferred turn without driving',
           'B': 'Same research ELF and deferred turn; wake the still-eligible desktop bootstrap driver'}
STOCK = ordinary.ROOT / 'stock'
RESEARCH = ordinary.ROOT / 'research'
MODE_KEY = 'BILICOMICS_DESKTOP_PRESENT_PROBE'
KNOWN_SOURCES = {'maintenance_timer', 'runtime_wakeup', 'desktop_startup_fallback'}
BASE_OBSERVE = ordinary.observe
BASE_SAMPLER = ordinary.Sampler
BASE_CAPTURE = ordinary.control_capture
BASE_LAUNCH = ordinary.launch
FULL_DATA = ('mode', 'width', 'height', 'ui_thread')
TURN_DATA = ('mode', 'delay_ms', 'workers_started_before', 'window_alive', 'booting', 'suspended', 'quitting',
             'cancelled', 'driver_available', 'will_drive', 'ui_thread')


def selected_mode(name):
    if name in ('check-a', 'check-b'):
        return {'check-a': 'observe', 'check-b': 'wake'}[name]
    if name in ordinary.SCHEDULE:
        return 'observe' if name[0] == 'a' else 'wake'
    return None


class ApplicationProcesses:
    def __init__(self, mode):
        self.mode = mode

    def __getattr__(self, name):
        return getattr(subprocess, name)

    def Popen(self, command, **options):
        expected = RESEARCH if self.mode is not None else STOCK
        require(command[0] == expected and MODE_KEY not in options['env'], 'Wrong executable or inherited research mode')
        if self.mode is not None:
            options['env'] = dict(options['env'], **{MODE_KEY: self.mode})
        return subprocess.Popen(command, **options)


@contextmanager
def qualified_launch(directory, arm, duration):
    with BASE_LAUNCH(directory, arm, duration) as (process, origin):
        yield process, origin
        if directory.name != 'bootstrap':
            mode = ordinary.subprocess.mode
            require(process.poll() is None, 'Application exited before post-confirmation identity check')
            blob = Path(f'/proc/{process.pid}/environ').read_bytes()
            require(blob, 'Live process environment is unreadable after the external window observations')
            key = MODE_KEY.encode() + b'='
            actual = [entry[len(key):] for entry in blob.split(b'\0') if entry.startswith(key)]
            require(actual == ([mode.encode()] if mode is not None else []), 'Live application has the wrong research mode')


def control_capture(directory, signed_in):
    result = BASE_CAPTURE(directory, signed_in)
    return dict(result, selected_arm='A0', bootstrap_probe_mode=None, process_probe_environment_verified=True)


def diagnostics(directory):
    perf = [row for row in ordinary.records(directory / 'performance.jsonl') if row['kind'] == 'startup']
    life = ordinary.records(directory / 'lifecycle.jsonl')
    pid = one(perf, lambda row: row['stage'] == 'process_enter', 'process entry')['process_id']
    require(all(row['process_id'] == pid for row in perf + life), 'Mixed startup processes')
    require(not any(row['stage'] == 'desktop_submit_probe_unavailable' for row in perf), 'Private software submit probe unavailable')

    def stage(name):
        return one(perf, lambda row: row['stage'] == name, name)

    def callback(name, admission=False):
        row = stage(name)
        data = row['data']
        require(row['thread_id'] == pid and data['source'] in KNOWN_SOURCES and
                all(type(data[key]) is bool for key in ('render_notifier_registered', 'first_scene_rendered')), 'Unknown callback provenance')
        result = {key: data[key] for key in ('source', 'render_notifier_registered', 'first_scene_rendered')}
        if admission:
            require(data['workers_admitted_by_ui'] is True, 'Desktop UI did not own worker admission')
            result['workers_admitted_by_ui'] = True
        return dict(monotonic_ns=row['monotonic_ns'], **result)

    def observed(name, optional=False):
        journal = [row for row in life if row.get('stage') == name]
        samples = [row for row in perf if row['stage'] == name]
        if optional and not journal and not samples:
            return None
        require(len(journal) == len(samples) == 1 and samples[0]['thread_id'] == pid and
                journal[0]['monotonic_ns'] == samples[0]['data']['observed']['monotonic_ns'], 'Different startup producer clocks')
        return journal[0]['monotonic_ns']

    first, gate = callback('driver_first_entry'), callback('ui_worker_gate', True)
    record = {'pid': pid, 'process_enter_ns': observed('process_enter'), 'window_shown_ns': observed('window_shown'),
              'first_scene_rendered_ns': observed('first_scene_rendered', optional=True), 'workers_started_ns': observed('workers_started'),
              'driver_first_entry': first, 'ui_worker_gate': gate}
    for label in ('font_worker_queued', 'state_worker_queued', 'font_worker_begin', 'state_worker_begin'):
        row = stage(label)
        require((row['thread_id'] == pid) == label.endswith('_queued'), 'Invalid worker/queue ownership')
        record[label + '_ns'] = row['monotonic_ns']
    for label, fields in (('desktop_software_full_submit', FULL_DATA), ('desktop_post_submit_turn', TURN_DATA)):
        rows = [row for row in perf if row['stage'] == label]
        if not rows:
            record[label] = None
            continue
        require(len(rows) == 1 and rows[0]['thread_id'] == pid, 'Missing unique UI software submit or deferred turn')
        row = rows[0]
        record[label] = {'monotonic_ns': row['monotonic_ns'], **{field: row['data'][field] for field in fields}}
    require(record['process_enter_ns'] <= record['window_shown_ns'] <= first['monotonic_ns'] <= record['workers_started_ns'] <=
            record['font_worker_queued_ns'] <= record['state_worker_queued_ns'] <= gate['monotonic_ns'], 'Inconsistent registration/admission order')
    require(record['font_worker_queued_ns'] <= record['font_worker_begin_ns'] and
            record['state_worker_queued_ns'] <= record['state_worker_begin_ns'], 'Worker started before its queue record')
    return record


def observe(directory, name, arm):
    initial_pixels = region(rgb(ordinary.REFERENCES / 'startup-0.png'), ACTIVITY).tobytes()
    activity = []

    class ActivityObserver(BASE_SAMPLER):
        def get(self, window, box):
            stamp, actual = super().get(window, box)
            if box == ACTIVITY and not activity and actual == initial_pixels:
                activity.append(stamp)
            return stamp, actual

    ordinary.Sampler = ActivityObserver
    try:
        result = BASE_OBSERVE(directory, name, arm)
    finally:
        ordinary.Sampler = BASE_SAMPLER
    require(len(activity) == 1, 'Initial loading indicator has no observed read interval')
    result['initial_activity_sample_ns'] = activity[0]
    record = diagnostics(directory)
    mode = selected_mode(name)
    result.update(bootstrap_probe_mode=mode, process_probe_environment_verified=True)
    if mode is None:
        require(name == 'check-stock' and all(record[key] is None for key in ('desktop_software_full_submit', 'desktop_post_submit_turn')),
                'Research telemetry in the stock control')
        result['selected_arm'] = 'A0'
    else:
        for key in ('desktop_software_full_submit', 'desktop_post_submit_turn'):
            require(record[key] is not None and record[key]['mode'] == mode, 'Application telemetry does not match the measured mode')
    require(record['pid'] == result['pid'] and result['feedback_sample_ns'][1] <= activity[0][0] and
            activity[0][1] <= result['first_exact_shelf_sample_ns'][0], 'Activity did not follow the identified loading title')
    last_positive = max(result['feedback_sample_ns'][1], activity[0][1])
    result['wake'] = dict(record, visible_before_workers_started=last_positive <= record['workers_started_ns'],
                          visible_before_state_worker_queued=last_positive <= record['state_worker_queued_ns'],
                          visible_before_state_prepared=last_positive <= result['stages']['producer_ns'])
    return result


def xvfb(name):
    try:
        subprocess.run([*ordinary.XVFB, sys.executable, '-B', __file__, name, '--inside'], check=True, timeout=45)
    finally:
        ordinary.observer.require_retired()
        for path in Path('/proc').glob('[0-9]*/exe'):
            try:
                target = path.readlink()
            except (FileNotFoundError, ProcessLookupError, PermissionError):
                continue
            require(target not in (STOCK, RESEARCH), 'An application process remained in the private namespace')


def check_stock(inside):
    require(Path.cwd() == ordinary.ROOT and ordinary.load(ordinary.ROOT / 'wall-qualification.json')['external_parent_realtime_and_monotonic_advanced'], 'Wrong stock workspace')
    ordinary.observer.require_private_network()
    if not inside:
        xvfb('check-stock')
        return
    case = ordinary.QUALIFICATION / 'check-stock'
    case.mkdir(mode=0o700)
    ordinary.private_copy(ordinary.POSITIVE, case / 'data', True)
    try:
        result = observe(case, 'check-stock', 'A')
        ordinary.save(case / 'result.json', result)
        keys = ('case', 'pid', 'selected_arm', 'process_executable_matches', 'process_probe_environment_verified', 'bootstrap_probe_mode',
                'feedback_sample_ns', 'initial_activity_sample_ns', 'visible_marker_count', 'ready_resident_bytes', 'final_logical_rgb_matches', 'wake')
        ordinary.save(ordinary.QUALIFICATION / 'stock-diagnostics.json', {key: result[key] for key in keys})
    finally:
        shutil.rmtree(case / 'data')


def main():
    os.umask(0o077)
    require(MODE_KEY not in os.environ, 'The outer environment must not select the research mode')
    name = sys.argv[1] if len(sys.argv) > 1 else ''
    mode = selected_mode(name)
    if name in ('qualification-summary', 'trial') or mode is not None:
        ordinary.ARMS = {'A': RESEARCH, 'B': RESEARCH}
    else:
        ordinary.ARMS = {'A': STOCK, 'B': STOCK}
    ordinary.subprocess = ApplicationProcesses(mode)
    ordinary.launch = qualified_launch
    ordinary.SOURCES = SOURCES
    ordinary.control_capture = control_capture
    ordinary.observe = observe
    ordinary.xvfb = xvfb
    if name == 'check-stock':
        require(sys.argv[2:] in ([], ['--inside']), 'Invalid stock control action')
        check_stock(bool(sys.argv[2:]))
        return
    if sys.argv[1:] == ['trial']:
        require((ordinary.QUALIFICATION / 'stock-diagnostics.json').is_file(), 'Missing independent populated stock qualification')
    ordinary.main()
    if sys.argv[1:] == ['qualification-summary']:
        directory = ordinary.QUALIFICATION
        values = {name: ordinary.load(directory / name / 'result.json') for name in ('check-a', 'check-b')}
        public = {key: {field: value[field] for field in ('case', 'selected_arm', 'bootstrap_probe_mode', 'process_probe_environment_verified',
                                                           'pid', 'feedback_sample_ns', 'initial_activity_sample_ns', 'wake')}
                  for key, value in values.items()}
        ordinary.save(directory / 'wake-qualification.json', {'scope': 'Untimed populated ordinary Shelf qualification, not a warmup or paired sample.',
                      'same_formal_executable': ordinary.ARMS['A'].samefile(ordinary.ARMS['B']), 'cases': public})


def export_shelf_qualification():
    summary = ordinary.load(ordinary.QUALIFICATION / 'summary.json')
    if (ordinary.QUALIFICATION / 'setup-admission-v1').is_dir():
        for key in ('positive_reference', 'signed_out_negative'):
            summary[key].pop('process_probe_environment_verified', None)
            summary[key]['probe_verification_scope'] = 'Stock image qualification predating the post-confirmation environment check; no live mode claim.'
    ordinary.save(ordinary.QUALIFICATION / 'shelf-export.json', summary)


if __name__ == '__main__':
    if sys.argv[1:] == ['export-shelf-qualification']:
        os.umask(0o077)
        export_shelf_qualification()
    else:
        main()
