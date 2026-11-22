"""Qualify the prepared-State observation in at most three fresh, reversible native launches."""

import argparse
from functools import cache
import importlib.util
import os
from pathlib import Path
import signal
import sys
import tempfile
import uuid
import zipfile


HERE = Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location('read_only_clock_utilities', HERE.parent / 'startup-clock-aa-20261122/aa_runner.py')
UTILS = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(UTILS)
FROZEN, BASE, ROUTE = UTILS.FROZEN, UTILS.BASE, UTILS.ROUTE
CONTROL = BASE.load_module(BASE.STUDY / 'shelf_operator.py')
require = CONTROL.require
SCHEMA = 'bili-prepared-handoff-v1'
MARK = 'handoff-visit.json'
ADMITTED = 'handoff-inputs-verified.json'
SLOTS = (1, 2, 3)
INPUTS = ('application', 'qualified_application', 'arm_qualification', 'worker', 'route_fs', 'shipping_launcher', 'document_launcher', 'native_kit', 'rollout_root')


def case_files(stock, run):
    files = ROUTE.case_files(stock, run, False)
    option = b' --prepared-handoff-trace "$DATA_DIR/' + run.encode('ascii') + b'.prepared-handoff.json"'
    launcher = UTILS.once(files['launch.sh'], b' --exit-after-ms 2500 >&6', option + b' --exit-after-ms 2500 >&6')
    switch = UTILS.once(files['switch.sh'], b"    order_option=''\n", b"    order_option='" + option + b"'\n")
    switch = UTILS.once(switch, b' --exit-after-ms 2500%s >&6%s', b'%s --exit-after-ms 2500 >&6%s')
    switch = UTILS.once(switch, b'"data/$run.jsonl"|data/logs/launcher.log|', b'"data/$run.prepared-handoff.json"|"data/$run.jsonl"|data/logs/launcher.log|')
    switch = UTILS.once(switch, b'! present "$stage/data/$run.jsonl" &&',
                        b'! present "$stage/data/$run.prepared-handoff.json" && ! present "$stage/data/$run.jsonl" &&')
    return {**files, 'launch.sh': launcher, 'switch.sh': switch}


def handoff_guard():
    source = CONTROL.plain(BASE.STUDY / 'host_tools/native_guard.sh', 128 << 10).read_bytes()
    before = BASE.GUARD_BINDING
    return UTILS.once(source, before, before.replace(b' --exit-after-ms', b' --prepared-handoff-trace "$stage/data/$run.prepared-handoff.json" --exit-after-ms'))


def plan_for(owner, boot):
    cases = []
    for ordinal in SLOTS:
        run = 'bili-shelf-' + uuid.uuid4().hex
        cases.append({'ordinal': ordinal, 'run': run, 'batch': str(CONTROL.HOST / ('bilicomics-interactive-' + run))})
    return {'schema': SCHEMA, 'owner': owner, 'host_boot': boot, 'cases': cases}


def checked(plan, owner, boot):
    require(set(plan) == {'schema', 'owner', 'host_boot', 'cases'} and (plan['schema'], plan['owner'], plan['host_boot']) == (SCHEMA, owner, boot)
            and type(plan['cases']) is list and len(plan['cases']) == len(SLOTS), 'Wrong handoff plan, owner, host boot or count')
    seen = set()
    for ordinal, case in zip(SLOTS, plan['cases']):
        run = case['run']
        require(set(case) == {'ordinal', 'run', 'batch'} and type(case['ordinal']) is int and case['ordinal'] == ordinal
                and CONTROL.RUN.fullmatch(run) and run not in seen and case['batch'] == str(CONTROL.HOST / ('bilicomics-interactive-' + run)),
                'Repeated, reordered or foreign handoff visit')
        seen.add(run)
    return plan['cases']


def binding(case):
    return {'schema': SCHEMA, 'treatment': 'frozen-stock-prepared-handoff', 'ordinal': case['ordinal'], 'run': case['run']}


def input_receipt(case):
    return {'schema': 'bili-prepared-handoff-inputs-v1', 'run_id': case['run'], 'ordinal': case['ordinal'],
            'source': 'frozen-installed-control', 'comparison': 'complete-three-archive-byte-comparison'}


def checked_batch(case, plan):
    path = CONTROL.private(Path(case['batch']))
    ticket = CONTROL.load(CONTROL.plain(path / 'admission.json', 65536))
    require((ticket['run'], ticket['owner'], ticket['host_boot']) == (case['run'], plan['owner'], plan['host_boot'])
            and CONTROL.load(CONTROL.plain(path / MARK, 1024)) == binding(case), 'Not the original handoff batch')
    return path, ticket


def inputs(args):
    stock = UTILS.sources(args)
    application = CONTROL.plain(args.application, 150 << 20)
    qualified = CONTROL.plain(args.qualified_application, 150 << 20)
    ticket = CONTROL.plain(args.arm_qualification, 16384)
    require(qualified.parent.name == 'artifacts' and qualified.name == 'bilicomics'
            and ticket == qualified.parent.parent / 'qualification/arm.json', 'Build reference and ARM qualification must share the original source run')
    proof = BASE.BOUNDS.read_json(ticket)
    size = qualified.stat().st_size
    require(proof['schema'] == 'bili-prepared-handoff-arm-qualification-v1' and proof['passed'] is True
            and proof['physical_device_accessed'] is False and proof['device_lease_accessed'] is False and proof['cargo_bin'] == 'bilicomics'
            and proof['cross_target'] == 'armv7-unknown-linux-musleabihf' and proof['static_elf32_arm_hard_float'] is True
            and type(proof['application_bytes']) is int and proof['application_bytes'] == size and proof['cli']['missing_shelf_trace_rejected'] is True
            and proof['cli']['conflicting_lifecycle_rejected'] is True, 'Missing qualification for the new ARM executable and command line')
    profiles = proof['profiles']
    require(len(profiles) == 2 and {entry['profile'] for entry in profiles} == {'scribe', 'pw6'}
            and all(type(entry['native_exit_code']) is int and entry['native_exit_code'] == 1 and entry['prepared_handoff_cli_recognized'] is True
                    and entry['missing_owned_snapshot_rejected'] is True and entry['native_framebuffer_available'] is False
                    and entry['native_input_available'] is False and entry['shelf_rendered'] is False for entry in profiles), 'Missing both off-device profile refusals')
    worker = CONTROL.plain(args.worker, 4 << 20)
    require(proof['worker']['unchanged_sources'] is True and proof['worker']['bytes'] == worker.stat().st_size
            and proof['worker']['identical_to_install_zip'] is True, 'Missing unchanged worker qualification')
    with zipfile.ZipFile(FROZEN.STOCK_ZIP) as archive:
        leaf = 'bilicomics/app/kindle-jpeg-worker'
        require(archive.namelist().count(leaf) == 1 and archive.getinfo(leaf).file_size == worker.stat().st_size
                and archive.read(leaf) == worker.read_bytes(), 'Worker differs from the exact installed package')
    with application.open('rb') as candidate:
        require(not application.samefile(qualified) and FROZEN.matched_stream(candidate, qualified, application.stat().st_size),
                'Staged application must be an independent byte copy of the qualified diagnostic build')
    return stock


def prepare(args):
    owner, boot = CONTROL.identity()
    plan = plan_for(owner, boot)
    cases = checked(plan, owner, boot)
    stock = inputs(args)
    directory = args.output
    require(directory.is_absolute() and directory.resolve() == directory, 'Expected a new absolute private directory')
    directory.mkdir(mode=0o700)
    CONTROL.private(directory)
    CONTROL.save(directory / 'plan.json', plan)
    CONTROL.save(directory / 'inputs.json', {name: str(getattr(args, name)) for name in INPUTS})
    with tempfile.TemporaryDirectory(dir=directory) as temporary:
        kit = Path(temporary) / 'kit'
        ROUTE.assemble(argparse.Namespace(**{**vars(args), 'run': cases[0]['run'], 'output': kit}), ROUTE.demo_roster())
        CONTROL.write_once(directory / 'seed.sqlite3', (kit / 'data/demo/state.sqlite3').read_bytes())
        for case in cases:
            for relative, data in case_files(stock, case['run']).items():
                (kit / relative).write_bytes(data)
            CONTROL.prepare(kit, args.native_kit, args.rollout_root)
            path = Path(case['batch'])
            guard = CONTROL.plain(path / 'native-guard.sh', 128 << 10)
            require(guard.read_bytes() == CONTROL.plain(BASE.STUDY / 'host_tools/native_guard.sh', 128 << 10).read_bytes(), 'Changed guard source')
            guard.write_bytes(handoff_guard())
            CONTROL.save(path / MARK, binding(case))
    print(directory / 'plan.json', flush=True)


def verify(directory, plan, cases, *, admitted=False):
    saved = CONTROL.load(CONTROL.plain(directory / 'inputs.json', 8192))
    require(set(saved) == set(INPUTS), 'Unrecognized handoff inputs')
    args = argparse.Namespace(**{key: Path(value) for key, value in saved.items()})
    stock = inputs(args)
    detector = CONTROL.plain(args.native_kit, 200 << 20)
    shared = {'shipping-launcher.sh': stock, 'document-launcher.sh': ROUTE.DOCUMENT, 'data/device-profile': b'scribe\n',
              'data/device-allowlist': b'scribe 5.19.5\n', 'data/demo/state.sqlite3': CONTROL.plain(directory / 'seed.sqlite3', 1 << 20),
              'cleanup-assets': ROUTE.demo_roster(), 'app/bilicomics': args.application, 'app/kindle-jpeg-worker': CONTROL.plain(args.worker, 150 << 20),
              'route-fs': CONTROL.plain(args.route_fs, 4 << 20)}
    for case in cases:
        path, ticket = checked_batch(case, plan)
        route, probe = (CONTROL.plain(path / name, 200 << 20) for name in ('route.tar.gz', 'probe.tar.gz'))
        if admitted:
            require(CONTROL.load(CONTROL.plain(path / ADMITTED, 1024)) == input_receipt(case), 'Missing original software admission')
        expected_leaves = BASE.PREPARED | {MARK} | ({ADMITTED} if admitted else set())
        require({leaf.name for leaf in path.iterdir()} == expected_leaves and ticket['rollout_root'] == str(args.rollout_root)
                and CONTROL.plain(path / 'askpass', 256).read_bytes() == CONTROL.ASKPASS
                and CONTROL.plain(path / 'native-guard.sh', 128 << 10).read_bytes() == handoff_guard()
                and (ticket['route_bytes'], ticket['probe_bytes']) == (route.stat().st_size, probe.stat().st_size), 'Altered or attempted handoff batch')
        with probe.open('rb') as data:
            require(FROZEN.matched_stream(data, detector, probe.stat().st_size), 'Changed qualified detector')
        expected = {**shared, **case_files(stock, case['run'])}
        del expected['run-id']
        FROZEN.verify_archive(route, expected)


@cache
def collector():
    return BASE.load_module(HERE / 'handoff_collect.py')


class HandoffVisit(CONTROL.Operator):
    def __init__(self, batch):
        self.capture_handoff = collector().collect
        super().__init__(batch)

    def collect(self):
        super().collect()
        self.capture_handoff(self)


def visit(directory, plan, cases, ordinal, verify_closed):
    require(sys.stdin.isatty(), 'Private foreground terminal required')
    previous = None
    for earlier in cases[:ordinal - 1]:
        path, _ticket = checked_batch(earlier, plan)
        current = verify_closed(path)
        require(type(current) is str and CONTROL.UUID.fullmatch(current) and previous in (None, current), 'Changed device boot')
        previous = current
    remaining = cases[ordinal - 1:]
    require(all(not os.path.lexists(Path(case['batch']) / 'attempted') for case in remaining), 'Cannot retry or skip an attempted visit')
    path, _ticket = checked_batch(remaining[0], plan)
    if ordinal == 1:
        require(not os.path.lexists(directory / 'started.json'), 'Handoff experiment already started')
        verify(directory, plan, cases)
        CONTROL.save(directory / 'started.json', {'owner': plan['owner'], 'host_boot': plan['host_boot']})
        for case in cases:
            CONTROL.save(Path(case['batch']) / ADMITTED, input_receipt(case))
    else:
        require(CONTROL.load(CONTROL.plain(directory / 'started.json', 1024)) == {'owner': plan['owner'], 'host_boot': plan['host_boot']}
                and CONTROL.load(CONTROL.plain(path / ADMITTED, 1024)) == input_receipt(remaining[0]), 'Missing original software admission')
        verify(directory, plan, [remaining[0]], admitted=True)
    CONTROL.announce(f"handoff visit={ordinal}/3 batch={path}")
    HandoffVisit(path).execute(False)
    current = verify_closed(path)
    require(type(current) is str and CONTROL.UUID.fullmatch(current) and previous in (None, current), 'Changed device boot')


def main():
    os.umask(0o077)
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='operation', required=True)
    prep = commands.add_parser('prepare')
    for key in (*INPUTS, 'output'):
        prep.add_argument('--' + key.replace('_', '-'), required=True, type=Path)
    for operation in ('check', 'visit', 'recover'):
        command = commands.add_parser(operation)
        command.add_argument('--plan', required=True, type=Path)
        if operation == 'visit':
            command.add_argument('--ordinal', required=True, type=int, choices=SLOTS)
        if operation == 'recover':
            command.add_argument('--ordinal', required=True, type=int, choices=SLOTS)
    args = parser.parse_args()
    if args.operation == 'prepare':
        prepare(args)
        return

    def interrupt(*unused):
        raise InterruptedError('Operation interrupted')

    signal.signal(signal.SIGHUP, interrupt)
    signal.signal(signal.SIGTERM, interrupt)
    with FROZEN.exclusive_directory(args.plan.parent):
        require(args.plan.name == 'plan.json', 'Expected the original handoff plan')
        plan = CONTROL.load(CONTROL.plain(args.plan, 65536))
        cases = checked(plan, *CONTROL.identity())
        if args.operation == 'check':
            verify(args.plan.parent, plan, cases)
            require(callable(collector().validate_closed), 'Missing handoff validator')
        elif args.operation == 'recover':
            path, _ticket = checked_batch(cases[args.ordinal - 1], plan)
            HandoffVisit(path).execute(True)
        else:
            visit(args.plan.parent, plan, cases, args.ordinal, collector().validate_closed)


if __name__ == '__main__':
    try:
        main()
    except (CONTROL.RecoveryRequired, CONTROL.SampleUnavailable, CONTROL.OperatorError) as error:
        print(error, file=sys.stderr)
        sys.exit(3 if isinstance(error, CONTROL.RecoveryRequired) else 4 if isinstance(error, CONTROL.SampleUnavailable) else 2)
    except (Exception, KeyboardInterrupt):
        sys.exit('Handoff experiment stopped; check the original plan and visit. Raw diagnostics withheld.')
