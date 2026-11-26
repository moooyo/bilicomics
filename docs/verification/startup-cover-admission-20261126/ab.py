"""Compare startup cover admission on the ordinary offline Kindle Library route."""

import argparse
from functools import partial
import getpass
import importlib.util
import json
import os
from pathlib import Path
import signal
import sys
import uuid
import warnings


HERE = Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location('ordinary_visit', HERE.parent / 'startup-ordinary-phases-20261126/native_visit.py')
NATIVE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(NATIVE)
CORE, CLOCK, COLLECT, BOUNDS = NATIVE.CORE, NATIVE.CLOCK, NATIVE.COLLECT, NATIVE.BOUNDS
OVERLAP = NATIVE.OVERLAP
ANALYSIS = CORE.module(HERE.parent / 'startup-shelf-ab-20261121/analyze_shelf_ab.py')
require = CORE.require
SCHEMA = 'bili-ordinary-offline-cover-admission-ab-v1'
SCHEDULE, ENDPOINTS = OVERLAP.SCHEDULE, OVERLAP.ENDPOINTS
LAST_PAIR = SCHEDULE[-1][0]
EARLY = b' --early-startup-covers'
DECODE_OVERLAP = 'cover_decode_overlap_with_first_loading_completion_wait'
PREPARED = CLOCK.BASE.PREPARED | {'case.json', NATIVE.MOUNT}
EVIDENCE_ERRORS = (ValueError, KeyError, TypeError, OSError)
# Match the application's native idle reset, not a sleep inhibit or a setting
# change. Run synchronously before card dispatch, never during the timing window.
RESET_NATIVE_IDLE = '''test "$(lipc-get-prop com.lab126.appmgrd activeApp)" = com.lab126.KPPMainApp
test "$(lipc-get-prop -s com.lab126.powerd state)" = active
for app in bilicomics aidoku weread; do
    test ! -e "/tmp/$app-launcher.lock" && test ! -L "/tmp/$app-launcher.lock" || exit 1
done
lipc-set-prop -i com.lab126.powerd touchScreenSaverTimeout 1 >/dev/null
'''


class Visit(NATIVE.Visit):
    def step(self, phase):
        super().step(phase)
        if phase in ('media-capacity', 'stage-route', 'route-activate'):
            require(not os.path.lexists(self.path / 'click-intended') and self.boot is not None,
                    'An idle reset requires the original active native screen before input')
            require(self.guard('native') == self.boot, 'Native ownership changed before idle reset')
            self.raw(RESET_NATIVE_IDLE, deadline=12)


def arm_argv(source, option, suffix, arm):
    binding = option + suffix
    require(arm in ('A', 'B') and source.count(binding) == 1 and EARLY not in source, 'Unknown cover admission argv boundary')
    return source if arm == 'A' else CLOCK.once(source, binding, option + EARLY + suffix)


def route_files(stock, launcher, run, seed, runtime, *, arm):
    files = NATIVE.route_files(stock, launcher, run, seed, runtime)
    ordinary = files['launch.sh']
    option = NATIVE.NEW_ARGS.replace(b'@RUN@', run.encode('ascii'))
    staged = arm_argv(ordinary, option, b' >&6', arm)
    switch = files['switch.sh'] if staged == ordinary else CLOCK.once(files['switch.sh'], ordinary, staged)
    return {**files, 'launch.sh': staged, 'switch.sh': switch}


def guard(arm):
    return arm_argv(NATIVE.guard(), b' --startup-phases "$stage/data/$run.startup-phases.json"',
                    b' | cmp -s - "$proc/$app/cmdline"', arm)


def batch_options(case):
    return {'route_builder': partial(route_files, arm=case['arm']), 'guard_builder': partial(guard, case['arm'])}


def make_plan(args, owner, boot):
    cases = []
    for index, (pair, arm) in enumerate(SCHEDULE, 1):
        run = 'bili-shelf-' + uuid.uuid4().hex
        cases.append({'visit': index, 'pair': pair, 'arm': arm, 'state': 'ready', 'run': run,
                      'batch': str(CLOCK.BASE.batch_path(run))})
    return {'schema': SCHEMA, 'owner': owner, 'boot': boot, 'cases': cases,
            'inputs': {name: str(getattr(args, name)) for name in NATIVE.INPUTS}}


def plan_cases(plan):
    require(type(plan) is dict and plan.keys() == {'schema', 'owner', 'boot', 'cases', 'inputs'}
            and plan['schema'] == SCHEMA and plan['owner'] == CORE.configured_owner()
            and CORE.UUID.fullmatch(plan['boot']), 'Wrong comparison plan or owner')
    paths = plan['inputs']
    require(type(paths) is dict and paths.keys() == set(NATIVE.INPUTS)
            and all(type(value) is str and Path(value).is_absolute() for value in paths.values()), 'Invalid shared inputs')
    cases = plan['cases']
    require(type(cases) is list and len(cases) == len(SCHEDULE), 'Incomplete fixed comparison schedule')
    seen = set()
    for index, (case, (pair, arm)) in enumerate(zip(cases, SCHEDULE), 1):
        run = case['run']
        require(type(run) is str and CORE.RUN.fullmatch(run) and run not in seen and type(case['visit']) is int
                and type(case['pair']) is int and case == {'visit': index, 'pair': pair, 'arm': arm, 'state': 'ready',
                'run': run, 'batch': str(CLOCK.BASE.batch_path(run))}, 'Reused, reordered or unbound comparison slot')
        seen.add(run)
    return cases


def check_batch(inputs, case):
    NATIVE.check_batch(inputs, case, **batch_options(case))


def scoped_load(loader, *args):
    # The inherited tools loader inserts its sibling import path. Do not retain
    # an extra copy or change import precedence between visits.
    original = sys.path[:]
    try:
        return loader(*args)
    finally:
        sys.path[:] = original


def prepare(args):
    owner, boot = CORE.identity()
    plan = make_plan(args, owner, boot)
    cases = plan_cases(plan)
    # This inherited check expands the pinned SSH configuration, not a connection.
    scoped_load(CORE.tools, args.rollout_root).pinned('10.0.0.238')
    require(args.output.is_absolute() and args.output.resolve() == args.output, 'Expected a fresh private destination')
    args.output.mkdir(mode=0o700)
    CORE.private(args.output)
    pending = args.output / 'preparing.json'
    CORE.save(pending, plan)
    for case in cases:
        NATIVE.prepared_batch(args, case, owner, boot, **batch_options(case))
    # Both arms (including warmups) are compared byte for byte to the same
    # application, worker, account fixture and helpers before the plan is published.
    for case in cases:
        check_batch(args, case)
    os.rename(pending, args.output / 'plan.json')
    print(args.output / 'plan.json', flush=True)


def read_binding(plan, case):
    path = CORE.private(Path(case['batch']))
    binding = COLLECT.read_json(path / 'case.json', 8192)
    admission = COLLECT.read_json(path / 'admission.json', 64 << 10)
    require(binding == {'case': case, 'seed_leaves': NATIVE.seed_layout(binding['seed_leaves'])}
            and (admission['run'], admission['owner'], admission['host_boot'], admission['rollout_root']) ==
            (case['run'], plan['owner'], plan['boot'], plan['inputs']['rollout_root']), 'Different slot or host admission')
    return path


def observation(plan, case, previous):
    path = read_binding(plan, case)
    boot, row = NATIVE.observation(case)
    source = row['cover_gate_source']
    overlap = row['intervals_ns'][DECODE_OVERLAP]
    require(source in ('ui', 'send') and (case['arm'] == 'B' or source == 'ui')
            and row['display_witness']['first_loading_completion_matches_send'] is True
            and type(overlap) is int and overlap >= 0, 'Unqualified cover admission or initial loading SEND evidence')
    card = COLLECT.read_json(path / 'card.json', 2048)
    trace = BOUNDS.read_jsonl(CORE.plain(path / (case['run'] + '.jsonl'), CORE.MAX_TRACE))
    if previous is not None:
        require(boot == previous[0] and previous[1] < card['click']['down_before_ns'],
                'Library visits do not match one device boot and the declared order')
    return (boot, trace[-1]['monotonic_ns']), {**row, 'pair': case['pair'], 'arm': case['arm'],
            'candidate_used_ui_fallback': case['arm'] == 'B' and source == 'ui'}


def stopped_visit(plan, case):
    path = Path(case['batch'])
    returned = False
    try:
        read_binding(plan, case)
        run, _, host_boot, _ = COLLECT.closure(path, native_boot_required=False)
        returned = (run, host_boot) == (case['run'], plan['boot'])
    except EVIDENCE_ERRORS:
        pass
    return {'visit': case['visit'], 'pair': case['pair'], 'arm': case['arm'], 'qualified': False,
            'exact_native_return_verified': returned}


def scan(plan, cases):
    visits, stopped, previous = [], None, None
    for case in cases:
        path = Path(case['batch'])
        if not os.path.lexists(path):
            continue
        CORE.private(path)
        if not COLLECT.flag_present(path, 'attempted'):
            require(not any(os.path.lexists(path / leaf) for leaf in ('released.json', 'private-token', 'origin.json')),
                    'Unbound visit progress')
            continue
        require(stopped is None and len(visits) == case['visit'] - 1, 'Cannot skip, replace or reorder a visit')
        try:
            previous, row = observation(plan, case, previous)
        except EVIDENCE_ERRORS:
            stopped = stopped_visit(plan, case)
        else:
            visits.append(row)
    return visits, stopped, previous


def aggregate_interval(samples):
    if not samples:
        return None
    return [ANALYSIS.median_outward([value[0] for value in samples])[0],
            ANALYSIS.median_outward([value[1] for value in samples])[1]]


def comparison(visits):
    measured = [row for row in visits if row['pair'] > 0]
    pairs = []
    for pair in range(1, LAST_PAIR + 1):
        members = {row['arm']: row for row in measured if row['pair'] == pair}
        if members.keys() != {'A', 'B'}:
            break
        pairs.append({'pair': pair, 'order': ''.join(arm for number, arm in SCHEDULE if number == pair),
                      'b_minus_a_ns': {key: ANALYSIS.pair_delta(members['A'][key], members['B'][key]) for key in ENDPOINTS}})
    summary = {}
    for key in ENDPOINTS:
        deltas = [pair['b_minus_a_ns'][key] for pair in pairs if pair['b_minus_a_ns'][key] is not None]
        summary[key] = {'paired_median_b_minus_a_ns': aggregate_interval(deltas), 'pairs': len(deltas),
                        'b_earlier': sum(hi < 0 for lo, hi in deltas), 'b_later': sum(lo > 0 for lo, hi in deltas),
                        'overlapping': sum(lo <= 0 <= hi for lo, hi in deltas),
                        'arms': {arm: aggregate_interval([row[key] for row in measured if row['arm'] == arm and row[key] is not None])
                                 for arm in ('A', 'B')}}
    primary = summary[ENDPOINTS[0]]
    timing_gate = primary['pairs'] == LAST_PAIR and primary['b_earlier'] >= 5 and primary['paired_median_b_minus_a_ns'][1] <= -100_000_000
    fallbacks = [row['visit'] for row in visits if row['arm'] == 'B' and row['cover_gate_source'] == 'ui']
    complete = len(visits) == len(SCHEDULE)
    assigned_sources = complete and all(row['cover_gate_source'] == ('ui' if row['arm'] == 'A' else 'send') for row in visits)
    return {'pairs': pairs, 'summary': summary, 'series_complete': complete, 'first_shelf_gate_passed': timing_gate,
            'all_assigned_gate_sources_matched': assigned_sources, 'candidate_ui_fallback_visits': fallbacks,
            'comparison_gate_passed': timing_gate and assigned_sources}


def report(plan, cases):
    visits, stopped, _ = scan(plan, cases)
    return {'schema': SCHEMA + '-results', 'device': 'Scribe 2022', 'firmware': '5.19.5',
            'origin': 'ordinary native Library card release', 'offline': True, 'demo': False, 'synthetic_account': True,
            'optical_measurement': False, 'warmups_excluded_from_comparison': True, 'visits': visits,
            'stopped': stopped, **comparison(visits)}


def cut(pair):
    return 0 if pair == 0 else sum(number <= pair for number, _ in SCHEDULE)


def segment(plan, cases, after_pair, through_pair):
    return {'owner': plan['owner'], 'host_boot': plan['boot'], 'after_pair': after_pair, 'through_pair': through_pair,
            'first_run': cases[cut(after_pair)]['run'], 'last_run': cases[cut(through_pair) - 1]['run']}


def resume_after(directory, plan, cases, visits):
    after = visits[-1]['pair'] if visits else 0
    require(after < LAST_PAIR and len(visits) == cut(after), 'Progress is not at a resumable complete pair')
    if after:
        pause = COLLECT.read_json(directory / f'paused-after-{after}.json', 2048)
        earlier = pause['after_pair']
        require(type(earlier) is int and 0 <= earlier < after and pause == segment(plan, cases, earlier, after)
                and pause == COLLECT.read_json(directory / f'started-after-{earlier}.json', 2048),
                'No intentional pause after this complete pair')
    require(not os.path.lexists(directory / f'started-after-{after}.json'), 'Cannot resume an interrupted segment')
    return after


def start(directory, plan, cases, inputs, through_pair):
    require(sys.stdin.isatty() and not os.path.lexists(directory / 'discarded'), 'Use the original plan and a private terminal')
    visits, stopped, previous = scan(plan, cases)
    require(stopped is None, 'The series stopped on an unqualified visit; never retry or skip it')
    after = resume_after(directory, plan, cases, visits)
    require(after < through_pair <= LAST_PAIR, 'No fresh pairs up to the requested boundary')
    todo = cases[cut(after):cut(through_pair)]
    for case in todo:
        check_batch(inputs, case)
    with warnings.catch_warnings():
        warnings.simplefilter('error', getpass.GetPassWarning)
        password = getpass.getpass('Kindle authentication for this segment (not saved): ')
    require(password, 'No authentication provided')
    try:
        intent = segment(plan, cases, after, through_pair)
        CORE.save(directory / f'started-after-{after}.json', intent)
        for case in todo:
            CORE.announce(f"visit={case['visit']}/{len(SCHEDULE)} pair={case['pair']} arm={case['arm']} batch={case['batch']}")
            visit = scoped_load(Visit, Path(case['batch']))
            require(visit.boot is None, 'Expected a fresh device boot witness')
            # Refuse a changed device boot at the inherited preflight, before staging or input.
            if previous is not None:
                visit.boot = previous[0]
            visit.read_password = lambda: password
            visit.execute(False)
            previous, _ = observation(plan, case, previous)
        if through_pair < LAST_PAIR:
            CORE.save(directory / f'paused-after-{through_pair}.json', intent)
            CORE.announce(f'Intentional stop after complete pair {through_pair}; no lease is reserved.')
    finally:
        password = ''


def recover(plan, cases, slot):
    case = cases[slot - 1]
    path = read_binding(plan, case)
    require(COLLECT.flag_present(path, 'attempted') and not os.path.lexists(path / 'released.json')
            and os.path.lexists(path / 'private-token') and CORE.plain(path / 'native-guard.sh', 128 << 10).read_bytes() == guard(case['arm']),
            'No original active token or exact guard for this slot; recovery will not acquire a lease')
    scoped_load(Visit, path).execute(True)


def discard(directory, plan, cases):
    _, stopped, _ = scan(plan, cases)
    if stopped is not None:
        require(not os.path.lexists(Path(cases[stopped['visit'] - 1]['batch']) / 'private-token'),
                'Close the original owned operation before discarding unused host payloads')
    # Disable continuation first. Only remove the exact, never-attempted host payloads.
    CORE.flag(directory / 'discarded')
    for case in cases:
        path = Path(case['batch'])
        if not os.path.lexists(path) or COLLECT.flag_present(path, 'attempted'):
            continue
        with CLOCK.FROZEN.exclusive_directory(read_binding(plan, case)):
            require({leaf.name for leaf in path.iterdir()} == PREPARED, 'Unrecognized unattempted batch files')
            leaves = [CORE.plain(path / name, 200 << 20) for name in PREPARED]
            for leaf in leaves:
                leaf.unlink()
            path.rmdir()


def main():
    os.umask(0o077)
    cli = argparse.ArgumentParser(description=__doc__)
    commands = cli.add_subparsers(dest='operation', required=True)
    setup = commands.add_parser('prepare')
    for key in (*NATIVE.INPUTS, 'output'):
        setup.add_argument('--' + key.replace('_', '-'), type=Path, required=True)
    for name in ('check', 'start', 'recover', 'report', 'discard'):
        command = commands.add_parser(name)
        command.add_argument('--plan', type=Path, required=True)
        if name == 'start':
            command.add_argument('--through-pair', type=int, choices=range(1, LAST_PAIR + 1), default=LAST_PAIR,
                                 help='Intentionally stop after a complete pair; start again only from that boundary')
        if name == 'recover':
            command.add_argument('--visit', type=int, choices=range(1, len(SCHEDULE) + 1), required=True)
    args = cli.parse_args()
    if args.operation == 'prepare':
        prepare(args)
        return
    signal.signal(signal.SIGHUP, OVERLAP.interrupted)
    signal.signal(signal.SIGTERM, OVERLAP.interrupted)
    directory = args.plan.parent
    with CLOCK.FROZEN.exclusive_directory(directory):
        require(args.plan.name == 'plan.json', 'Expected the original plan path')
        plan = COLLECT.read_json(args.plan, 8192)
        cases = plan_cases(plan)
        if args.operation == 'report':
            print(json.dumps(report(plan, cases), indent=2, allow_nan=False))
            return
        require((plan['owner'], plan['boot']) == CORE.identity(), 'Expected the original host boot and owner')
        inputs = argparse.Namespace(**{name: Path(value) for name, value in plan['inputs'].items()})
        if args.operation == 'check':
            for case in cases:
                check_batch(inputs, case)
            print('All ordinary A/B archives and guards match their qualified inputs.')
        elif args.operation == 'start':
            start(directory, plan, cases, inputs, args.through_pair)
        elif args.operation == 'recover':
            recover(plan, cases, args.visit)
        else:
            discard(directory, plan, cases)


if __name__ == '__main__':
    try:
        main()
    except CORE.RecoveryRequired as error:
        print(f'Recovery required: {error}', file=sys.stderr)
        sys.exit(3)
    except CORE.SampleUnavailable as error:
        print(error, file=sys.stderr)
        sys.exit(4)
    except CORE.OperatorError as error:
        print(error, file=sys.stderr)
        sys.exit(2)
    except (Exception, KeyboardInterrupt):
        sys.exit('Comparison stopped; check the original closed/active batch. Raw output withheld.')
