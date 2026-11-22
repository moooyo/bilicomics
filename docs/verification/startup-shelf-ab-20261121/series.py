"""Prepack and run the fixed Scribe A/B series with the existing single-visit controller."""

import argparse
import getpass
import importlib.util
import os
from pathlib import Path
import signal
import sys
import tempfile
import uuid
import warnings


HERE = Path(__file__).resolve().parent
STUDY = HERE.parent / 'startup-interactive-kindle-20261121'


def load_module(file):
    spec = importlib.util.spec_from_file_location(file.stem, file)
    value = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(value)
    return value


CONTROL = load_module(STUDY / 'shelf_operator.py')
ROUTE = load_module(STUDY / 'route/prepare_route.py')
BOUNDS = load_module(STUDY / 'log_bounds.py')
SCHEDULE = tuple((pair, arm) for pair, order in enumerate(('AB', 'AB', 'BA', 'AB', 'BA', 'AB', 'BA')) for arm in order)
PREPARED = {'admission.json', 'route.tar.gz', 'probe.tar.gz', 'native-guard.sh', 'askpass'}
GUARD_BINDING = b'--shelf-render-trace "$stage/data/$run.jsonl" --exit-after-ms 2500 | cmp -s - "$proc/$app/cmdline"'


def batch_path(run):
    return CONTROL.HOST / ('bilicomics-interactive-' + run)


def make_plan(owner, boot):
    cases = []
    for pair, arm in SCHEDULE:
        run = 'bili-shelf-' + uuid.uuid4().hex
        cases.append({'pair': pair, 'arm': arm, 'run': run, 'batch': str(batch_path(run))})
    return {'schema': 'bili-shelf-ab-v1', 'owner': owner, 'host_boot': boot, 'cases': cases}


def check_plan(plan, owner, boot):
    CONTROL.require(set(plan) == {'schema', 'owner', 'host_boot', 'cases'} and plan['schema'] == 'bili-shelf-ab-v1'
                    and (plan['owner'], plan['host_boot']) == (owner, boot), 'Different study, owner or host boot')
    cases = plan['cases']
    CONTROL.require(type(cases) is list and len(cases) == len(SCHEDULE), 'Incomplete predeclared series')
    seen = set()
    for case, (pair, arm) in zip(cases, SCHEDULE):
        CONTROL.require(set(case) == {'pair', 'arm', 'run', 'batch'} and type(case['pair']) is int
                        and (case['pair'], case['arm']) == (pair, arm) and CONTROL.RUN.fullmatch(case['run'])
                        and case['run'] not in seen and case['batch'] == str(batch_path(case['run'])),
                        'Repeated, reordered or unbound visit')
        seen.add(case['run'])
    return cases


def guard_for_arm(source, arm):
    CONTROL.require(arm in ('A', 'B') and source.count(GUARD_BINDING) == 1, 'Unknown process-witness binding')
    return (source.replace(GUARD_BINDING, GUARD_BINDING.replace(b' |', ROUTE.SERIAL_OPTION + b' |'))
            if arm == 'A' else source)


def prepare(args):
    owner, boot = CONTROL.identity()
    plan = make_plan(owner, boot)
    cases = check_plan(plan, owner, boot)
    destination = args.output
    CONTROL.require(destination.is_absolute() and destination.resolve() == destination, 'Expected a new absolute output directory')
    destination.mkdir(mode=0o700)
    CONTROL.private(destination)
    with tempfile.TemporaryDirectory(prefix='prepare-', dir=destination) as temporary:
        kit = Path(temporary) / 'kit'
        inputs = argparse.Namespace(**vars(args))
        inputs.output, inputs.run = kit, cases[0]['run']
        ROUTE.assemble(inputs, ROUTE.demo_roster(), serial_shelf_render=True)
        shipping = (kit / 'shipping-launcher.sh').read_bytes()
        source = CONTROL.plain(STUDY / 'host_tools/native_guard.sh', 128 * 1024).read_bytes()
        for case in cases:
            for name, data in ROUTE.case_files(shipping, case['run'], case['arm'] == 'A').items():
                (kit / name).write_bytes(data)
            CONTROL.prepare(kit, args.native_kit, args.rollout_root)
            guard = CONTROL.plain(Path(case['batch']) / 'native-guard.sh', 128 * 1024)
            CONTROL.require(guard.read_bytes() == source, 'Unexpected prepared process witness')
            if case['arm'] == 'A':
                guard.write_bytes(guard_for_arm(source, 'A'))
    CONTROL.save(destination / 'plan.json', plan)
    print(destination / 'plan.json', flush=True)


def ready(cases, owner, boot):
    source = CONTROL.plain(STUDY / 'host_tools/native_guard.sh', 128 * 1024).read_bytes()
    for case in cases:
        path = CONTROL.private(Path(case['batch']))
        CONTROL.require({file.name for file in path.iterdir()} == PREPARED, 'A prepared visit is incomplete or already attempted')
        admission = CONTROL.load(CONTROL.plain(path / 'admission.json', 65536))
        CONTROL.require((admission['run'], admission['owner'], admission['host_boot']) == (case['run'], owner, boot)
                        and CONTROL.plain(path / 'native-guard.sh', 128 * 1024).read_bytes() == guard_for_arm(source, case['arm'])
                        and CONTROL.plain(path / 'askpass', 256).read_bytes() == CONTROL.ASKPASS,
                        'Wrong prepared visit, witness or prompt')
        for name in ('route', 'probe'):
            archive = CONTROL.plain(path / (name + '.tar.gz'), 200 * 1024 * 1024)
            CONTROL.require(archive.stat().st_size == admission[name + '_bytes'], 'Prepared archive size changed')


def closed_case(case, owner, previous_boot):
    path = Path(case['batch'])
    released, retired, card, sample = (CONTROL.load(path / name) for name in
                                       ('released.json', 'retired.json', 'card.json', 'intervals.json'))
    CONTROL.require(released['owner_thread'] == owner and released['run_id'] == case['run'] and released['state'] == 'released'
                    and released['device_disposition'] == 'original-native-restored' and released['owner_and_token_checked'] is True
                    and released['native_restored_and_connections_closed'] is True, 'Exact native return and release not confirmed')
    CONTROL.require(not any((path / name).exists() for name in ('stop-intended', 'partial-trace', 'failure.json', 'private-token'))
                    and retired['natural'] is True and type(retired['app_pid']) is int and retired['app_pid'] > 0
                    and card['process_id'] == sample['process_id'] == retired['app_pid'] and card['run_id'] == sample['run_id'] == case['run']
                    and sample['renderer_return_interval_ns'] is not None, 'Visit lacks a complete natural primary observation')
    boot = CONTROL.load(path / 'origin.json')['boot']
    CONTROL.require(card['boot_id'] == boot and CONTROL.UUID.fullmatch(boot) and previous_boot in (None, boot), 'Changed device boot')
    order = sample.get('render_order')
    CONTROL.require((case['arm'], order) in (('A', 'wait_then_render'), ('B', 'render_then_wait'), ('B', 'wait_then_render')),
                    'Missing or inconsistent observed render order')
    projection = CONTROL.plain(path / (case['run'] + '.jsonl'), CONTROL.MAX_TRACE)
    frames = [row for row in BOUNDS.read_jsonl(projection) if row['stage'] == 'shelf_rendered']
    CONTROL.require(len(frames) == 1 and frames[0]['process_id'] == retired['app_pid']
                    and frames[0]['data'].get('render_order') == order and frames[0]['data'].get('shelf_state') in ('ready', 'empty'),
                    'Expected one render of the seeded Shelf with the audited order')
    return boot


def run_cases(plan, credential):
    previous_boot = None
    for position, case in enumerate(plan['cases'], 1):
        CONTROL.announce(f"visit={position}/{len(SCHEDULE)} pair={case['pair']} arm={case['arm']} batch={case['batch']}")
        visit = CONTROL.Operator(Path(case['batch']))
        visit.read_password = credential
        visit.execute(False)
        previous_boot = closed_case(case, plan['owner'], previous_boot)


def start(path):
    owner, boot = CONTROL.identity()
    directory = CONTROL.private(path.parent)
    CONTROL.require(path.name == 'plan.json' and not (directory / 'started.json').exists(), 'A fresh, unattempted series is required')
    plan = CONTROL.load(CONTROL.plain(path, 65536))
    ready(check_plan(plan, owner, boot), owner, boot)
    CONTROL.require(sys.stdin.isatty(), 'Use a private foreground terminal')
    with warnings.catch_warnings():
        warnings.simplefilter('error', getpass.GetPassWarning)
        password = getpass.getpass('Kindle authentication for this series (not saved): ')
    CONTROL.require(password, 'No authentication provided')
    try:
        CONTROL.save(directory / 'started.json', {'owner': owner, 'host_boot': boot})
        run_cases(plan, lambda: password)
    finally:
        password = ''
    CONTROL.announce('All predeclared visits closed; run the offline paired audit.')


def main():
    os.umask(0o077)
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='operation', required=True)
    preparation = commands.add_parser('prepare')
    for name in ('application', 'worker', 'route-fs', 'shipping-launcher', 'document-launcher', 'native-kit', 'rollout-root', 'output'):
        preparation.add_argument('--' + name, type=Path, required=True)
    commands.add_parser('start').add_argument('--plan', type=Path, required=True)
    args = parser.parse_args()
    if args.operation == 'prepare':
        prepare(args)
        return

    def interrupt(*unused):
        raise InterruptedError('Operation interrupted')

    signal.signal(signal.SIGHUP, interrupt)
    signal.signal(signal.SIGTERM, interrupt)
    start(args.plan)


if __name__ == '__main__':
    try:
        main()
    except CONTROL.RecoveryRequired as error:
        print(error, file=sys.stderr)
        sys.exit(3)
    except CONTROL.SampleUnavailable as error:
        print(error, file=sys.stderr)
        sys.exit(4)
    except CONTROL.OperatorError as error:
        print(error, file=sys.stderr)
        sys.exit(2)
    except (Exception, KeyboardInterrupt):
        sys.exit('Series stopped; raw output withheld. Check the last private batch before proceeding.')
