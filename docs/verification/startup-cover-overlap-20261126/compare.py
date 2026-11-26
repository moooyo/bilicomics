"""Compare only sleep-cover probe placement on the ordinary Scribe Library route."""

import argparse
import getpass
import importlib.util
import json
import math
import os
from pathlib import Path
import re
import shutil
import signal
import statistics
import sys
import tempfile
import warnings


HERE = Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location('clock_runner', HERE.parent / 'startup-clock-aa-20261122/aa_runner.py')
CLOCK = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CLOCK)
BASE, CONTROL, ROUTE = CLOCK.BASE, CLOCK.CONTROL, CLOCK.ROUTE
AUDIT, COLLECT = BASE.BOUNDS, CLOCK.clock_collection()
SCHEMA = 'bili-cover-probe-overlap-v1'
SCHEDULE = BASE.SCHEDULE
PRODUCTION_BYTES = 74940168  # November 25 clean installation of d293782.
ICON_DOCUMENT = ROUTE.DOCUMENT.replace(
    b'set -eu\n', b'# Icon: /mnt/us/bilicomics/assets/bilicomics-library-cover.png\nset -eu\n')
COVER_CALL = b'\nconfigure_cover_sleep\n'
SETTLE = b"wait_for_observer_settle || fail 'The native startup settling delay did not complete.'\n"
PLACEMENT = (b"# The loaded blanket modules determine sleep-cover eligibility, not display\n"
             b"# admission. Read them while the owned observer's timer is still running.\n"
             b'configure_cover_sleep\n')
APP_STAGES = ('process_enter', 'window_shown', 'workers_started', 'state_prepare_begin',
              'state_metadata_prepared', 'state_prepared', 'application_attached')
CLOCK_INTERVALS = {
    'shim_to_preinvoke': ('launch_entry', 'pre_app_invocation'),
    'preinvoke_to_main': ('pre_app_invocation', 'process_enter'),
    'main_to_window_shown': ('process_enter', 'window_shown'),
    'window_to_worker_admission': ('window_shown', 'workers_started'),
    'worker_admission_to_state_start': ('workers_started', 'state_prepare_begin'),
    'state_metadata_preparation': ('state_prepare_begin', 'state_metadata_prepared'),
    'metadata_to_prepared_state': ('state_metadata_prepared', 'state_prepared'),
    'prepared_to_ui_attached': ('state_prepared', 'application_attached'),
    'attached_to_first_shelf_render': ('application_attached', 'shelf_rendered'),
    'render_to_accepted_send': ('shelf_rendered', 'shelf_submitted'),
    'send_to_same_marker_completion': ('shelf_submitted', 'shelf_driver_completed'),
}
SHELL_MARKS = ('study_observer_armed', 'startup_logging_ready', 'study_cover_begin', 'study_cover_end',
               'study_observer_settled', 'startup_sample_begin', 'startup_sample_ready',
               'study_portrait_ready', 'native_take_begin', 'native_take_done')
COVER_READ_MARKS = ('native_read_go', 'native_read_ready', 'native_read_reaped')
COVER_OUTCOMES = ('study_cover_eligible', 'study_cover_fallback')
ENDPOINTS = ('renderer_return_interval_ns', 'accepted_send_interval_ns', 'driver_completion_interval_ns')
require = CONTROL.require
once = CLOCK.once


def overlap_launcher(stock):
    without_cover = once(stock, COVER_CALL, b'\n')
    return once(without_cover, SETTLE, PLACEMENT + SETTLE)


def current_stock_guard(switch):
    switch = once(switch, b'= 74340832 ]', f'= {PRODUCTION_BYTES} ]'.encode('ascii'))
    # Native Library owns this metadata directory. It is not a second scriptlet.
    # Never inspect its contents or include it in any stage cleanup roster.
    obsolete_rejection = b'            [Bb][Ii][Ll][Ii][Cc][Oo][Mm][Ii][Cc][Ss].[Ss][Dd][Rr]|'
    exact_sidecar = b'            bilicomics.sh.sdr) directory "$path" || return 1;;\n'
    return once(switch, obsolete_rejection, exact_sidecar + obsolete_rejection)


def route_for(stock, launcher, run):
    files = CLOCK.case_files(launcher, run)
    script = files['launch.sh']
    observer = b'if start_observer; then observer_started=1; fi\n'
    script = once(script, observer, observer + b'launch_trace study_observer_armed\n')
    script = once(script, COVER_CALL, b'\nlaunch_trace study_cover_begin\nconfigure_cover_sleep\n'
                  b'launch_trace study_cover_end\n'
                  b'if [ "$COVER_SLEEP_SUPPORTED" -eq 1 ]; then launch_trace study_cover_eligible; '
                  b'else launch_trace study_cover_fallback; fi\n')
    script = once(script, SETTLE, SETTLE + b'launch_trace study_observer_settled\n')
    publish = b"publish_native_portrait_orientation native-portrait-orientation || fail 'The startup portrait calibration is unavailable.'\n"
    script = once(script, publish, publish + b'launch_trace study_portrait_ready\n')
    shim = once(files['entry.sh'], b'unset BILICOMICS_LAUNCH_TRACE\n', b'export BILICOMICS_LAUNCH_TRACE=1\n')
    switch = CLOCK.clock_switch(ROUTE.case_files(stock, run, False)['switch.sh'], script, shim)
    switch = current_stock_guard(switch)
    switch = once(switch, b'|data/logs/launcher.log|', b'|data/logs/launcher-timing.tsv|data/logs/launcher.log|')
    return {**files, 'launch.sh': script, 'entry.sh': shim, 'entry-ready.sh': shim, 'switch.sh': switch}


def plan_cases(plan):
    owner, boot = CONTROL.identity()
    require(set(plan) == {'schema', 'owner', 'host_boot', 'cases'} and plan['schema'] == SCHEMA
            and (plan['owner'], plan['host_boot']) == (owner, boot) and len(plan['cases']) == len(SCHEDULE),
            'Wrong session or comparison plan')
    runs = set()
    for case, (pair, arm) in zip(plan['cases'], SCHEDULE):
        run = case['run']
        require(case == {'pair': pair, 'arm': arm, 'run': run, 'batch': str(BASE.batch_path(run))}
                and CONTROL.RUN.fullmatch(run) and run not in runs, 'Wrong comparison schedule or repeated run')
        runs.add(run)
    return plan['cases']


def guard():
    return CLOCK.guard_for_clock((BASE.STUDY / 'host_tools/native_guard.sh').read_bytes())


def prepare(args):
    owner, boot = CONTROL.identity()
    plan = {**BASE.make_plan(owner, boot), 'schema': SCHEMA}
    stock = CLOCK.sources(args)
    icon = CONTROL.plain(args.installed_document, 1024).read_bytes()
    require(icon == ICON_DOCUMENT, 'The installed card reference is not the known icon card')
    candidate = CONTROL.plain(args.candidate_launcher, 200_000).read_bytes()
    require(candidate == overlap_launcher(stock), 'Expected only the verified cover-probe movement')
    ROUTE.arm_executable(CONTROL.plain(args.clock_marks, 4 << 20))
    require(args.output.is_absolute() and args.output.resolve() == args.output, 'Expected a fresh private destination')
    args.output.mkdir(mode=0o700)
    cases = plan_cases(plan)
    if args.icon_probe:
        cases = cases[:1]
        plan = {'schema': SCHEMA + '-icon-probe', 'owner': owner, 'host_boot': boot, 'case': cases[0]}
    with tempfile.TemporaryDirectory(dir=args.output) as temporary:
        kit = Path(temporary) / 'kit'
        ROUTE.assemble(argparse.Namespace(**{**vars(args), 'output': kit, 'run': cases[0]['run']}), ROUTE.demo_roster())
        # The frozen route builder proves its historical input. The device
        # guard instead compares the one exact card from the clean install.
        (kit / 'document-launcher.sh').write_bytes(icon)
        shutil.copyfile(args.clock_marks, kit / 'clock-marks')
        (kit / 'clock-marks').chmod(0o700)
        for case in cases:
            launch = candidate if case['arm'] == 'B' else stock
            for name, data in route_for(stock, launch, case['run']).items():
                (kit / name).write_bytes(data)
            CONTROL.prepare(kit, args.native_kit, args.rollout_root)
            batch = Path(case['batch'])
            (batch / 'native-guard.sh').write_bytes(guard())
            expected = {name: kit / name for name in CONTROL.FILES - {'run-id'}}
            CLOCK.verify_archive(batch / 'route.tar.gz', expected)
            CONTROL.save(batch / 'cover-probe-case.json', case)
    CONTROL.save(args.output / 'plan.json', plan)
    CONTROL.announce(str(args.output / 'plan.json'))


def extract_stages(visit):
    stage = visit.route
    require(stage == '/mnt/us/' + visit.run and CONTROL.RUN.fullmatch(visit.run), 'Unbound stage request')
    life = f'{stage}/data/{visit.run}-lifecycle.jsonl'
    trace = f'{stage}/data/logs/launcher-timing.tsv'
    names = '|'.join(APP_STAGES)
    shell = '|'.join(SHELL_MARKS + COVER_OUTCOMES)
    # Only content-free records are read back; no UI state, titles or raw logs.
    data = visit.raw(f'''plain() {{
    test -f "$1" && test ! -L "$1" && test "$(readlink -f "$1")" = "$1" &&
        test "$(stat -c '%u:%h' "$1")" = 0:1 && test "$(stat -c %s "$1")" -le "$2"
}}
plain {life} 262144 && plain {trace} 16384 || exit 1
awk '/"event":"startup-stage"/ && /"stage":"({names})"/ {{print}}' {life}
awk -F '\\t' '$2 ~ /^(study_cover_begin|study_cover_end)$/ {{cover = ($2 == "study_cover_begin")}}
    $2 ~ /^({shell})$/ {{print $1, $2, $3}}
    cover && $2 ~ /^native_read_(go|ready|reaped)$/ {{print $1, $2, $3}}' {trace}''')
    require(0 < len(data) <= 8192, 'Invalid stage receipt length')
    app, launcher, launch_pid = {}, {}, None
    pid = CONTROL.load(visit.path / 'retired.json')['app_pid']
    for line in data.decode('ascii').splitlines():
        if line.startswith('{'):
            row = json.loads(line, object_pairs_hook=AUDIT.unique_object)
            name = row['stage']
            require(name in APP_STAGES and name not in app and row['process_id'] == pid
                    and row['event'] == 'startup-stage' and COLLECT.number(row['monotonic_ns']), 'Unbound app stage')
            app[name] = row['monotonic_ns']
        else:
            match = re.fullmatch(r'([0-9]{1,14})\.([0-9]{2}) ([a-z_]+) pid=([1-9][0-9]{0,9})', line)
            require(match is not None, 'Invalid launcher /proc/uptime record')
            seconds, fraction, name, number = match.groups()
            require(name in SHELL_MARKS + COVER_READ_MARKS + COVER_OUTCOMES and name not in launcher
                    and launch_pid in (None, number), 'Unbound launcher stage')
            launch_pid = number
            launcher[name] = int(seconds) * 1000 + int(fraction) * 10
    CONTROL.save(visit.path / 'cover-probe-stages.json', {'app_ns': app, 'launcher_uptime_ms': launcher})


CARD_REFUSAL = {'schema': 'bili-card-input-v1', 'dispatched': False}


class Visit(CLOCK.ClockVisit):
    def probe(self, mode, *point):
        reply = super().probe(mode, *point)
        if mode == 'open-card' and reply == CARD_REFUSAL and reply['dispatched'] is False:
            CONTROL.save(self.path / 'card-refused.json', reply)
            self.card_dispatch, self.probe_step = 'not-requested', 'receipt-accepted'
            raise CONTROL.OperatorError('Card input rejected before any button-down')
        return reply

    def retire(self):
        refusal = self.path / 'card-refused.json'
        if not refusal.exists():
            return super().retire()
        require(CONTROL.load(CONTROL.plain(refusal, 512)) == CARD_REFUSAL
                and not (self.path / 'observed-click.json').exists()
                and self.guard('native') == self.boot,
                'The original no-dispatch refusal could not be verified')
        self.raw(f'test ! -e {self.route}/picked && test ! -L {self.route}/picked')
        # The route verifier still requires no live picked entry before restore.

    def collect(self):
        super().collect()
        extract_stages(self)


def observation(case):
    batch = Path(case['batch'])
    require(CONTROL.load(batch / 'cover-probe-case.json') == case, 'Unbound comparison case')
    run, boot, _, disposition = COLLECT.closure(batch)
    require(run == case['run'] and disposition == 'original-native-restored', 'The original device route was not restored')
    pid, card = COLLECT.visit_identity(batch, run, boot)
    require(CONTROL.load(batch / 'observed-click.json') == {'run_id': run, 'boot_id': boot, 'click': card['click']}, 'Different release origin')
    rows = AUDIT.narrow_rows(AUDIT.read_jsonl(batch / (run + '.jsonl')), run, boot, process_id=pid)
    audit = AUDIT.audit(card, rows)
    frames = [row['data'] for row in rows if row['stage'] == 'shelf_rendered']
    require(len(frames) == 1 and frames[0]['shelf_state'] == 'ready' and frames[0]['render_order'] != 'initial_fill', 'No ordinary populated Shelf')
    extra = CONTROL.load(batch / 'cover-probe-stages.json')
    clock = CONTROL.load(batch / COLLECT.SIDECAR)
    require(clock['run_id'] == run and clock['boot_id'] == boot and clock['app_pid'] == pid
            and clock['clock'] == AUDIT.CLOCK and audit == CONTROL.load(batch / 'intervals.json'), 'Unbound clock or Shelf evidence')
    app, shell = extra['app_ns'], extra['launcher_uptime_ms']
    outcomes = set(COVER_OUTCOMES) & shell.keys()
    require(set(APP_STAGES) == app.keys() and len(outcomes) == 1
            and set(SHELL_MARKS + COVER_READ_MARKS) | outcomes == shell.keys()
            and app['process_enter'] == clock['rust_main_entry'] and app['state_prepared'] == clock['state_produced'],
            'Incomplete phase observations or unsuccessful blanket read')
    stamps = {**app, **audit['landmarks_ns'], **{key: clock[key] for key in ('launch_entry', 'pre_app_invocation')}}
    lo, hi = AUDIT.validate_click(card['click'])
    times = [hi, stamps['launch_entry'], stamps['pre_app_invocation'], *(app[name] for name in APP_STAGES), stamps['shelf_rendered']]
    require(times == sorted(times), 'Unexpected application phase ordering')
    order = ['study_observer_armed', 'startup_logging_ready']
    cover = ['study_cover_begin', *COVER_READ_MARKS, 'study_cover_end', next(iter(outcomes))]
    if case['arm'] == 'B':
        order += cover
    order += ['study_observer_settled', 'startup_sample_begin', 'startup_sample_ready', 'study_portrait_ready']
    if case['arm'] == 'A':
        order += cover
    order += ['native_take_begin', 'native_take_done']
    ordered = [shell[name] for name in order]
    require(ordered == sorted(ordered), 'Cover probe did not run at the assigned launcher boundary')
    durations = {'release_to_shim_ns': AUDIT.interval(stamps['launch_entry'], lo, hi, 'shim entry')}
    for name, (begin, end) in CLOCK_INTERVALS.items():
        start, stop = stamps[begin], stamps[end]
        if start is not None and stop is not None:
            require(stop >= start, 'Reversed phase duration')
        durations[name + '_ns'] = None if start is None or stop is None else stop - start
    shell_spans = {
        'observer_arm_to_settle': ('study_observer_armed', 'study_observer_settled'),
        'observer_arm_to_cover_end': ('study_observer_armed', 'study_cover_end'),
        'cover_probe': ('study_cover_begin', 'study_cover_end'),
        'settled_to_takeover': ('study_observer_settled', 'native_take_begin'),
        'native_display_capture': ('startup_sample_begin', 'startup_sample_ready'),
        'native_portrait_publish': ('startup_sample_ready', 'study_portrait_ready'),
        'native_takeover': ('native_take_begin', 'native_take_done'),
    }
    return boot, {'pair': case['pair'], 'arm': case['arm'], **{name: audit[name] for name in ENDPOINTS},
                  'phase_durations': durations, 'launcher_uptime_intervals_ms': {
                      name: shell[end] - shell[begin] for name, (begin, end) in shell_spans.items()},
                  'probe_placement': 'during_settle' if case['arm'] == 'B' else 'after_portrait_publication',
                  'probe_finished_before_display_sample': case['arm'] == 'B',
                  'probe_finished_before_minimum_settle_deadline': case['arm'] == 'B'
                      and shell['study_cover_end'] - shell['study_observer_armed'] < 1950,
                  'custom_sleep_cover_eligible': 'study_cover_eligible' in outcomes, 'same_process': True,
                  'matched_marker': audit['driver_completion_interval_ns'] is not None,
                  'native_restored_and_exact_lease_released': True}


def aggregate_interval(samples):
    return [math.floor(statistics.median([value[0] for value in samples])),
            math.ceil(statistics.median([value[1] for value in samples]))] if samples else None


def report(cases):
    visits, outcomes, device_boot = [], [], None
    for case in cases:
        path = Path(case['batch'])
        try:
            boot, visit = observation(case)
            require(device_boot in (None, boot), 'Device boot changed')
        except (ValueError, KeyError, TypeError, OSError):
            if any((path / leaf).exists() for leaf in ('attempted', 'released.json')):
                outcomes.append({'pair': case['pair'], 'arm': case['arm'], 'qualified': False,
                                 'released': (path / 'released.json').exists()})
            break
        device_boot = boot
        visits.append(visit)
        outcomes.append({'pair': case['pair'], 'arm': case['arm'], 'qualified': True, 'released': True})
    measured = [visit for visit in visits if visit['pair'] > 0]
    pairs = []
    for pair in range(1, 7):
        members = {visit['arm']: visit for visit in measured if visit['pair'] == pair}
        if members.keys() != {'A', 'B'}:
            continue
        delta = {}
        for name in ENDPOINTS:
            a, b = members['A'][name], members['B'][name]
            delta[name] = [b[0] - a[1], b[1] - a[0]] if a is not None and b is not None else None
        pairs.append({'pair': pair, 'b_minus_a_ns': delta})
    summaries = {}
    for name in ENDPOINTS:
        deltas = [pair['b_minus_a_ns'][name] for pair in pairs if pair['b_minus_a_ns'][name] is not None]
        summaries[name] = {'paired_median_b_minus_a_ns': aggregate_interval(deltas), 'pairs': len(deltas),
                           'b_earlier': sum(hi < 0 for lo, hi in deltas),
                           'b_later': sum(lo > 0 for lo, hi in deltas), 'overlapping': sum(lo <= 0 <= hi for lo, hi in deltas),
                           'arms': {arm: aggregate_interval([visit[name] for visit in measured if visit['arm'] == arm and visit[name] is not None])
                                    for arm in ('A', 'B')}}
    phase_medians = {}
    for arm in ('A', 'B'):
        samples = [visit for visit in measured if visit['arm'] == arm and any(p['pair'] == visit['pair'] for p in pairs)]
        medians = {}
        for field in ('phase_durations', 'launcher_uptime_intervals_ms'):
            keys = sorted({key for visit in samples for key in visit[field]})
            medians[field] = {}
            for key in keys:
                values = [visit[field][key] for visit in samples if visit[field][key] is not None]
                medians[field][key] = aggregate_interval(values) if values and isinstance(values[0], list) else (
                    statistics.median(values) if values else None)
        phase_medians[arm] = medians
    primary = summaries[ENDPOINTS[0]]
    complete = primary['pairs'] == 6
    outcomes_matched = all(len({v['custom_sleep_cover_eligible'] for v in measured if v['pair'] == p['pair']}) == 1 for p in pairs)
    early = [v for v in measured if v['arm'] == 'B' and v['probe_finished_before_minimum_settle_deadline']]
    timing_gate = complete and primary['b_earlier'] >= 5 and primary['paired_median_b_minus_a_ns'][1] <= -100_000_000
    return {'schema': SCHEMA + '-results', 'tested_schedule': outcomes, 'visits': visits, 'pairs': pairs,
            'summary': summaries, 'phase_medians_of_complete_pairs': phase_medians,
            'first_shelf_gate_passed': timing_gate, 'all_six_candidate_probes_overlapped': complete and len(early) == 6,
            'all_six_cover_eligibility_outcomes_matched': complete and outcomes_matched,
            'warmups_excluded_from_comparison': True, 'device': 'Scribe 2022', 'firmware': '5.19.5',
            'origin': 'ordinary native Library card release', 'offline_demo': True, 'optical_measurement': False}


def main():
    os.umask(0o077)
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='operation', required=True)
    preparation = commands.add_parser('prepare')
    for name in (*CLOCK.INPUTS, 'installed_document', 'candidate_launcher', 'output'):
        preparation.add_argument('--' + name.replace('_', '-'), type=Path, required=True)
    preparation.add_argument('--icon-probe', action='store_true', help='Prepare a single no-launch icon check')
    for mode in ('start', 'report', 'recover'):
        command = commands.add_parser(mode)
        command.add_argument('--plan', type=Path, required=True)
        if mode == 'recover':
            command.add_argument('--visit', type=int, required=True)
        if mode == 'start':
            command.add_argument('--through-pair', type=int, choices=range(1, 7), default=6,
                                 help='Pause after this complete pair for off-lease device coordination')
    args = parser.parse_args()
    if args.operation == 'prepare':
        prepare(args)
        return
    plan = CONTROL.load(CONTROL.plain(args.plan, 65536))
    cases = plan_cases(plan)
    if args.operation == 'report':
        print(json.dumps(report(cases), indent=2))
        return
    signal.signal(signal.SIGHUP, interrupted)
    signal.signal(signal.SIGTERM, interrupted)
    if args.operation == 'recover':
        require(1 <= args.visit <= len(cases), 'Unknown visit')
        Visit(Path(cases[args.visit - 1]['batch'])).execute(True)
        return
    directory = CONTROL.private(args.plan.parent)
    descriptor = os.open(directory, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC)
    try:
        CONTROL.fcntl.flock(descriptor, CONTROL.fcntl.LOCK_EX | CONTROL.fcntl.LOCK_NB)
        start(cases, args.through_pair)
    finally:
        os.close(descriptor)


def start(cases, through_pair):
    require(sys.stdin.isatty(), 'Expected a private terminal')
    private_guard = guard()
    pending, previous_boot = [], None
    for index, case in enumerate(cases, 1):
        path = CONTROL.private(Path(case['batch']))
        if (path / 'attempted').exists():
            require(not pending, 'Cannot skip or replace a visit')
            boot, _ = observation(case)
            require(previous_boot in (None, boot), 'The Kindle rebooted during the comparison')
            previous_boot = boot
            continue
        require({leaf.name for leaf in path.iterdir()} == BASE.PREPARED | {'cover-probe-case.json'}
                and CONTROL.load(path / 'cover-probe-case.json') == case
                and (path / 'native-guard.sh').read_bytes() == private_guard, 'Changed prepared visit')
        pending.append((index, case))
    todo = [(index, case) for index, case in pending if case['pair'] <= through_pair]
    require(todo, 'No fresh visits up to the requested pair')
    with warnings.catch_warnings():
        warnings.simplefilter('error', getpass.GetPassWarning)
        password = getpass.getpass('Kindle authentication for this series (not saved): ')
    require(password, 'No authentication provided')
    try:
        for index, case in todo:
            CONTROL.announce(f"visit={index}/{len(cases)} pair={case['pair']} arm={case['arm']} batch={case['batch']}")
            visit = Visit(Path(case['batch']))
            visit.read_password = lambda: password
            visit.execute(False)
            boot, _ = observation(case)
            require(previous_boot in (None, boot), 'The Kindle rebooted during the comparison')
            previous_boot = boot
    finally:
        password = ''


def interrupted(*unused):
    raise InterruptedError('Comparison interrupted')


if __name__ == '__main__':
    try:
        main()
    except CONTROL.RecoveryRequired as error:
        sys.exit(f'Recovery required: {error}')
    except CONTROL.OperatorError as error:
        sys.exit(str(error))
    except (OSError, ValueError, KeyError, TypeError, KeyboardInterrupt):
        sys.exit('Comparison stopped; check the original closed/active batch. Raw output withheld.')
