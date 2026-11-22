"""Prepare and run ten identical, independently leased native-card clock observations."""

import argparse
import getpass
import importlib.util
import os
from pathlib import Path
import shutil
import signal
import sys
import tarfile
import tempfile
import time
import uuid
import warnings
import zipfile


HERE = Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location('frozen_admission', HERE.parent / 'startup-library-admission-20261121/native_series.py')
FROZEN = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(FROZEN)
BASE, ROUTE = FROZEN.BASE, FROZEN.ROUTE
CONTROL = BASE.load_module(BASE.STUDY / 'shelf_operator.py')
CONTROL.FILES = CONTROL.FILES | {'clock-marks'}
require = CONTROL.require
SCHEMA = 'bili-shelf-clock-aa-v1'
MARK = 'clock-visit.json'
ADMITTED = 'clock-inputs-verified.json'
SCHEDULE = tuple((pair, arm) for pair, order in enumerate(('AB', 'AB', 'BA', 'AB', 'BA')) for arm in order)
INPUTS = ('application', 'worker', 'route_fs', 'clock_marks', 'shipping_launcher', 'document_launcher', 'native_kit', 'rollout_root')
EXECUTABLES = {'app/bilicomics', 'app/kindle-jpeg-worker', 'route-fs', 'clock-marks'}


def once(data, previous, updated):
    require(data.count(previous) == 1, 'Frozen clock route boundary changed')
    return data.replace(previous, updated)


def sources(args):
    stock = CONTROL.plain(args.shipping_launcher, 200_000).read_bytes()
    document = CONTROL.plain(args.document_launcher, 1024).read_bytes()
    with zipfile.ZipFile(CONTROL.plain(FROZEN.STOCK_ZIP, 150 << 20)) as archive:
        for leaf, expected in (('bilicomics/launch.sh', stock), ('documents/bilicomics.sh', document)):
            require(archive.namelist().count(leaf) == 1 and archive.getinfo(leaf).file_size == len(expected)
                    and archive.read(leaf) == expected, 'Not the frozen installed Library control')
    require(document == ROUTE.DOCUMENT and b'BILICOMICS_LIBRARY_PREFLIGHT_STDERR' not in stock,
            'Refused a different document or the uninstalled launcher candidate')
    return stock


def guard_for_clock(guard):
    needle = BASE.GUARD_BINDING
    return once(guard, needle, needle.replace(b' |', b' --lifecycle-trace "$stage/data/$run-lifecycle.jsonl" |'))


def exact_script(leaf, delimiter, content):
    require(delimiter not in content and content.endswith(b'\n'), 'Invalid frozen selector delimiter')
    return b'    cmp -s - "$stage/' + leaf + b'" <<\'' + delimiter + b"'\n" + content + delimiter + b'\n'


def clock_switch(source, launcher, shim):
    begin, end = b'launcher_verified() {\n', b'\n}\n\nclean_stage() {'
    require(source.count(begin) == source.count(end) == 1, 'Unknown reversible launcher verifier')
    first = source.index(begin) + len(begin)
    last = source.index(end, first)
    selector = exact_script(b'launch.sh', b'BILI_CLOCK_FROZEN_LAUNCHER', launcher) + b'    [ "$?" = 0 ] || return 1\n'
    selector += exact_script(b'entry.sh', b'BILI_CLOCK_FROZEN_SHIM', shim)
    source = source[:first] + selector + source[last:]
    for before, after in (
        (b'"$stage/switch.sh" "$tool"; do', b'"$stage/switch.sh" "$tool" "$stage/clock-marks"; do'),
        (b'cleanup-assets switch.sh route-fs; do', b'cleanup-assets switch.sh route-fs clock-marks; do'),
        (b'            switch.sh|route-fs|run-id|', b'            switch.sh|route-fs|clock-marks|run-id|'),
        (b'            picked/entry|', b'            picked/shim-entry.json|picked/app-preinvoke.json|"data/$run-lifecycle.jsonl"|picked/entry|'),
        (b'! present "$stage/data/$run.jsonl" && [ -x "$stage/app/bilicomics" ]',
         b'! present "$stage/data/$run.jsonl" && ! present "$stage/data/$run-lifecycle.jsonl" && '
         b'plain "$stage/clock-marks" && [ -x "$stage/clock-marks" ] && [ -x "$stage/app/bilicomics" ]'),
    ):
        source = once(source, before, after)
    return source


def case_files(stock, run):
    data = ROUTE.case_files(stock, run, False)
    label = run.encode('ascii')
    entry_exec = b'exec sh "$BILICOMICS_HOME/launch.sh" --library\n'
    shim = once(data['entry.sh'], entry_exec, b'exec "$BILICOMICS_HOME/clock-marks" ' + label +
                b' shim_entry "$BILICOMICS_HOME/picked/shim-entry.json" -- /bin/sh "$BILICOMICS_HOME/launch.sh" --library\n')
    app = b'"$APP_DIR/bilicomics" --kindle --device "$PROFILE" --data-dir "$DATA_DIR"'
    launcher = once(data['launch.sh'], app, b'"$BILICOMICS_HOME/clock-marks" ' + label +
                    b' app_preinvoke "$BILICOMICS_HOME/picked/app-preinvoke.json" -- ' + app)
    launcher = once(launcher, b' --exit-after-ms 2500 >&6', b' --exit-after-ms 2500 --lifecycle-trace "$DATA_DIR/' + label + b'-lifecycle.jsonl" >&6')
    return {**data, 'launch.sh': launcher, 'entry.sh': shim, 'entry-ready.sh': shim,
            'switch.sh': clock_switch(data['switch.sh'], launcher, shim)}


def make_plan(owner, boot):
    cases = []
    for pair, arm in SCHEDULE:
        run = 'bili-shelf-' + uuid.uuid4().hex
        cases.append({'pair': pair, 'arm': arm, 'run': run, 'batch': str(CONTROL.HOST / ('bilicomics-interactive-' + run))})
    return {'schema': SCHEMA, 'owner': owner, 'host_boot': boot, 'cases': cases}


def checked(plan, owner, boot):
    require(set(plan) == {'schema', 'owner', 'host_boot', 'cases'} and (plan['schema'], plan['owner'], plan['host_boot']) == (SCHEMA, owner, boot)
            and type(plan['cases']) is list and len(plan['cases']) == len(SCHEDULE), 'Wrong clock series, owner, boot or visit count')
    seen = set()
    for case, (pair, arm) in zip(plan['cases'], SCHEDULE):
        run = case['run']
        require(set(case) == {'pair', 'arm', 'run', 'batch'} and type(case['pair']) is int and (case['pair'], case['arm']) == (pair, arm)
                and CONTROL.RUN.fullmatch(run) and run not in seen and case['batch'] == str(CONTROL.HOST / ('bilicomics-interactive-' + run)),
                'Repeated, reordered or unbound clock visit')
        seen.add(run)
    return plan['cases']


def binding(case):
    return {'schema': SCHEMA, 'treatment': 'frozen-stock-clock', 'pair': case['pair'], 'arm': case['arm'], 'run': case['run']}


def input_receipt(case):
    return {'schema': 'bili-shelf-clock-inputs-v1', 'run_id': case['run'], 'pair': case['pair'], 'arm': case['arm'],
            'source': 'frozen-installed-control', 'comparison': 'complete-ten-archive-byte-comparison'}


def check_batch(case, plan):
    path = CONTROL.private(Path(case['batch']))
    admission = CONTROL.load(CONTROL.plain(path / 'admission.json', 65536))
    require((admission['run'], admission['owner'], admission['host_boot']) == (case['run'], plan['owner'], plan['host_boot'])
            and CONTROL.load(CONTROL.plain(path / MARK, 1024)) == binding(case), 'Wrong run or clock treatment binding')
    return path, admission


def prepare(args):
    owner, boot = CONTROL.identity()
    plan = make_plan(owner, boot)
    cases = checked(plan, owner, boot)
    stock = sources(args)
    ROUTE.arm_executable(CONTROL.plain(args.clock_marks, 4 << 20))
    destination = args.output
    require(destination.is_absolute() and destination.resolve() == destination, 'Expected a new absolute private output directory')
    destination.mkdir(mode=0o700)
    CONTROL.private(destination)
    CONTROL.save(destination / 'plan.json', plan)
    CONTROL.save(destination / 'inputs.json', {name: str(getattr(args, name)) for name in INPUTS})
    with tempfile.TemporaryDirectory(dir=destination) as scratch:
        kit = Path(scratch) / 'kit'
        ROUTE.assemble(argparse.Namespace(**{**vars(args), 'output': kit, 'run': cases[0]['run']}), ROUTE.demo_roster())
        shutil.copyfile(args.clock_marks, kit / 'clock-marks')
        (kit / 'clock-marks').chmod(0o700)
        CONTROL.write_once(destination / 'seed.sqlite3', (kit / 'data/demo/state.sqlite3').read_bytes())
        guard = CONTROL.plain(BASE.STUDY / 'host_tools/native_guard.sh', 128 << 10).read_bytes()
        for case in cases:
            for leaf, data in case_files(stock, case['run']).items():
                (kit / leaf).write_bytes(data)
            CONTROL.prepare(kit, args.native_kit, args.rollout_root)
            batch = Path(case['batch'])
            witness = CONTROL.plain(batch / 'native-guard.sh', 128 << 10)
            require(witness.read_bytes() == guard, 'Unknown prepared process identity guard')
            witness.write_bytes(guard_for_clock(guard))
            CONTROL.save(batch / MARK, binding(case))
    print(destination / 'plan.json', flush=True)


def verify_archive(path, expected):
    remaining = dict(expected)
    with tarfile.open(path, 'r|gz') as archive:
        for member in archive:
            require(member.name in remaining and member.type == tarfile.REGTYPE and not member.linkname and not member.pax_headers
                    and member.uid == member.gid == 0 and member.mode == (0o700 if member.name in EXECUTABLES else 0o600)
                    and FROZEN.matched_stream(archive.extractfile(member), remaining.pop(member.name), member.size), 'Changed clock archive contents')
    require(not remaining, 'Incomplete clock route')


def ready(directory, cases, plan):
    saved = CONTROL.load(CONTROL.plain(directory / 'inputs.json', 8192))
    require(set(saved) == set(INPUTS), 'Unrecognized clock inputs')
    args = argparse.Namespace(**{name: Path(path) for name, path in saved.items()})
    stock = sources(args)
    shared = {'shipping-launcher.sh': stock, 'document-launcher.sh': ROUTE.DOCUMENT,
              'data/device-profile': b'scribe\n', 'data/device-allowlist': b'scribe 5.19.5\n', 'cleanup-assets': ROUTE.demo_roster(),
              'data/demo/state.sqlite3': CONTROL.plain(directory / 'seed.sqlite3', 1 << 20),
              'app/bilicomics': CONTROL.plain(args.application, 150 << 20), 'app/kindle-jpeg-worker': CONTROL.plain(args.worker, 150 << 20),
              'route-fs': CONTROL.plain(args.route_fs, 4 << 20), 'clock-marks': CONTROL.plain(args.clock_marks, 4 << 20)}
    guard = guard_for_clock(CONTROL.plain(BASE.STUDY / 'host_tools/native_guard.sh', 128 << 10).read_bytes())
    probe = CONTROL.plain(args.native_kit, 200 << 20)
    for case in cases:
        path, admission = check_batch(case, plan)
        require({leaf.name for leaf in path.iterdir()} == BASE.PREPARED | {MARK} and admission['rollout_root'] == str(args.rollout_root)
                and CONTROL.plain(path / 'native-guard.sh', 128 << 10).read_bytes() == guard
                and CONTROL.plain(path / 'askpass', 256).read_bytes() == CONTROL.ASKPASS, 'Changed or attempted clock visit')
        payload = CONTROL.plain(path / 'route.tar.gz', 200 << 20)
        detector = CONTROL.plain(path / 'probe.tar.gz', 200 << 20)
        with detector.open('rb') as stream:
            require((payload.stat().st_size, detector.stat().st_size) == (admission['route_bytes'], admission['probe_bytes'])
                    and FROZEN.matched_stream(stream, probe, detector.stat().st_size), 'Changed qualified detector or route size')
        expected = {**shared, **case_files(stock, case['run'])}
        del expected['run-id']
        verify_archive(payload, expected)


def clock_collection():
    return BASE.load_module(HERE / 'aa_collect.py')


class ClockVisit(CONTROL.Operator):
    def collect(self):
        super().collect()
        clock_collection().collect(self)


def sequence(directory, plan, cases, credential, validate):
    previous = None
    for index, case in enumerate(cases, 1):
        if (directory / FROZEN.HOLD).exists():
            CONTROL.announce(f'Off-lease handoff before visit {index}; no reservation.')
            while (directory / FROZEN.HOLD).exists():
                time.sleep(.5)
        CONTROL.announce(f"visit={index}/{len(cases)} pair={case['pair']} slot={case['arm']} batch={case['batch']}")
        path, _admission = check_batch(case, plan)
        visit = ClockVisit(path)
        visit.read_password = credential
        visit.execute(False)
        boot = validate(path)
        require(type(boot) is str and CONTROL.UUID.fullmatch(boot) and previous in (None, boot), 'Changed device boot during clock series')
        previous = boot


def start(directory, plan, cases):
    require(not (directory / 'started.json').exists(), 'Cannot repeat or append clock visits')
    collector = clock_collection()
    require(callable(collector.collect) and callable(collector.validate_closed), 'Missing clock capture or offline verifier')
    ready(directory, cases, plan)
    require(sys.stdin.isatty(), 'Private foreground terminal required')
    with warnings.catch_warnings():
        warnings.simplefilter('error', getpass.GetPassWarning)
        password = getpass.getpass('Kindle authentication for this series (not saved): ')
    require(password, 'No authentication provided')
    try:
        CONTROL.save(directory / 'started.json', {'owner': plan['owner'], 'host_boot': plan['host_boot']})
        for case in cases:
            CONTROL.save(Path(case['batch']) / ADMITTED, input_receipt(case))
        sequence(directory, plan, cases, lambda: password, collector.validate_closed)
    finally:
        password = ''


def discard(plan, cases):
    for case in cases:
        path = Path(case['batch'])
        if not path.exists():
            continue
        with FROZEN.exclusive_directory(path):
            if (path / 'attempted').exists():
                continue
            check_batch(case, plan)
            names = {entry.name for entry in path.iterdir()}
            expected = BASE.PREPARED | {MARK}
            if ADMITTED in names:
                require(CONTROL.load(CONTROL.plain(path / ADMITTED, 1024)) == input_receipt(case), 'Unbound clock admission')
                expected |= {ADMITTED}
            require(names == expected, 'Unknown unattempted host files')
            leaves = [CONTROL.plain(path / name, 200 << 20) for name in expected]
            for leaf in leaves:
                leaf.unlink()
            path.rmdir()


def main():
    os.umask(0o077)
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='operation', required=True)
    prep = commands.add_parser('prepare')
    for name in (*INPUTS, 'output'):
        prep.add_argument('--' + name.replace('_', '-'), type=Path, required=True)
    for operation in ('check', 'start', 'discard', 'recover'):
        command = commands.add_parser(operation)
        command.add_argument('--plan', required=True, type=Path)
        if operation == 'recover':
            command.add_argument('--batch', required=True, type=Path)
    args = parser.parse_args()
    if args.operation == 'prepare':
        prepare(args)
        return

    def interrupted(*unused):
        raise InterruptedError('Operation interrupted')

    signal.signal(signal.SIGHUP, interrupted)
    signal.signal(signal.SIGTERM, interrupted)
    directory = args.plan.parent
    with FROZEN.exclusive_directory(directory):
        require(args.plan.name == 'plan.json', 'Expected original plan path')
        plan = CONTROL.load(CONTROL.plain(args.plan, 65536))
        cases = checked(plan, *CONTROL.identity())
        if args.operation == 'check':
            ready(directory, cases, plan)
            collector = clock_collection()
            require(callable(collector.collect) and callable(collector.validate_closed), 'Clock collector not ready')
            CONTROL.announce('Ten clock archives and their original inputs match; no device admission attempted.')
        elif args.operation == 'discard':
            discard(plan, cases)
        elif args.operation == 'recover':
            matches = [case for case in cases if Path(case['batch']) == args.batch]
            require(len(matches) == 1, 'Not an original clock batch')
            path, _admission = check_batch(matches[0], plan)
            ClockVisit(path).execute(True)
        else:
            start(directory, plan, cases)


if __name__ == '__main__':
    try:
        main()
    except (CONTROL.RecoveryRequired, CONTROL.SampleUnavailable, CONTROL.OperatorError) as error:
        print(error, file=sys.stderr)
        sys.exit(3 if isinstance(error, CONTROL.RecoveryRequired) else 4 if isinstance(error, CONTROL.SampleUnavailable) else 2)
    except (Exception, KeyboardInterrupt):
        sys.exit('Clock series stopped; check the original private batch. Diagnostic details withheld.')
