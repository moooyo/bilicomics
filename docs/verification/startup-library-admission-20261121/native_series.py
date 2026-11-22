"""Run the fixed Library-parent experiment using the existing reversible single-visit operator."""

import argparse
from contextlib import contextmanager
import difflib
import fcntl
import getpass
import importlib.util
import os
from pathlib import Path
import signal
import sys
import tarfile
import tempfile
import time
import warnings
import zipfile


HERE = Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location('shelf_series', HERE.parent / 'startup-shelf-ab-20261121/series.py')
BASE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(BASE)
CONTROL, ROUTE, BOUNDS = BASE.CONTROL, BASE.ROUTE, BASE.BOUNDS
require = CONTROL.require
SCHEMA = 'bili-library-admission-ab-v1'
ELF_SIZE = 74392336
STOCK_ZIP = Path('/var/tmp/bilicomics-shelf-render-20261121.tIiP8C4F/candidate/app/artifacts/verification/bilicomics-kindle-0.1.25-dev.zip')
INPUTS = ('application', 'worker', 'route_fs', 'shipping_launcher', 'candidate_launcher', 'document_launcher', 'native_kit', 'rollout_root')
MARK = 'launcher-arm.json'
HOLD = 'hold-between-visits'


def edited_stock(stock):
    begin = b'[ -x "$APP_DIR/bilicomics" ] ||'
    end = b'mkdir -p "$DATA_DIR/logs" "$DATA_DIR/fonts"\n'
    require(stock.count(begin) == stock.count(end) == 1, 'Unknown stock admission boundaries')
    first = stock.index(begin)
    last = stock.index(end, first) + len(end)
    body = stock[first:last].splitlines(keepends=True)
    awk_first = next(index for index, line in enumerate(body) if line.startswith(b'if ! awk -v profile='))
    awk_last = body.index(b'\' "$ALLOWLIST_FILE"; then\n')
    wrapped = b'check_launch_inputs() {\n' + b''.join(
        line if line == b'\n' or awk_first < index <= awk_last else b'    ' + line
        for index, line in enumerate(body)) + b'''}

if [ "${1:-}" = '--detached' ] && [ "${BILICOMICS_LIBRARY_PREFLIGHT_STDERR:-}" = 1 ]; then
    if [ -L /proc/self/fd/8 ]; then exec 2>&8; fi
    exec 8>&-
    check_launch_inputs
    exec 2>/dev/null
elif [ "${1:-}" != '--library' ]; then
    check_launch_inputs
fi
unset BILICOMICS_LIBRARY_PREFLIGHT_STDERR
'''
    original = b'        setsid sh "$SCRIPT_PATH" --detached >/dev/null 2>&1 </dev/null &\n        LIBRARY_WORKER_PID=$!\n'
    revised = (b'        if [ ! -L /proc/self/fd/2 ]; then exec 2>/dev/null; fi\n'
               + original.replace(b'setsid sh', b'BILICOMICS_LIBRARY_PREFLIGHT_STDERR=1 setsid sh').replace(b' >/dev/null', b' 8>&2 >/dev/null'))
    require(stock.count(original) == 1, 'Unknown stock Library worker invocation')
    candidate = (stock[:first] + wrapped + stock[last:]).replace(original, revised)
    diff = difflib.diff_bytes(difflib.unified_diff, stock.splitlines(keepends=True), candidate.splitlines(keepends=True),
                              fromfile=b'stock', tofile=b'worker-only', n=0)
    require((HERE / 'worker-only.patch').read_bytes() == b''.join(diff),
            'Edit differs from the independently frozen worker-only fixture')
    return candidate


def sources(paths):
    stock = CONTROL.plain(paths.shipping_launcher, 200_000).read_bytes()
    document = CONTROL.plain(paths.document_launcher, 1024).read_bytes()
    with zipfile.ZipFile(CONTROL.plain(STOCK_ZIP, 150 << 20)) as archive:
        for member, expected in (('bilicomics/launch.sh', stock), ('documents/bilicomics.sh', document)):
            require(archive.namelist().count(member) == 1 and archive.getinfo(member).file_size == len(expected)
                    and archive.read(member) == expected, 'Stock input is not the retained installation source')
    require(document == ROUTE.DOCUMENT, 'Wrong stock document')
    candidate = edited_stock(stock)
    require(CONTROL.plain(paths.candidate_launcher, 200_000).read_bytes() == candidate,
            'Candidate differs from the reviewed parent-only transformation')
    require(CONTROL.plain(paths.application, 150 << 20).stat().st_size == ELF_SIZE, 'Wrong retained diagnostic application')
    return stock, candidate


def binding(case):
    return {'schema': 'bili-library-admission-arm-v1', 'run': case['run'], 'arm': case['arm'],
            'parent_admission': {'A': 'repeated', 'B': 'worker-only'}[case['arm']],
            'application_bytes': ELF_SIZE, 'default_render_order': True}


def case_files(stock, candidate, case):
    files = ROUTE.case_files(stock if case['arm'] == 'A' else candidate, case['run'], False)
    switch = files['switch.sh']
    begin, end = b'launcher_verified() {\n', b'\n}\n\nclean_stage() {'
    require(switch.count(begin) == switch.count(end) == 1, 'Unknown diagnostic verifier boundary')
    marker = b'BILICOMICS_EXPECTED_LIBRARY_LAUNCHER'
    launcher = files['launch.sh']
    require(marker not in launcher and launcher.endswith(b'\n'), 'Unexpected expected-launcher delimiter')
    first = switch.index(begin) + len(begin)
    tail = switch.index(end, first)
    files['switch.sh'] = (switch[:first] + b'    cmp -s - "$stage/launch.sh" <<\'' + marker + b"'\n"
                          + launcher + marker + switch[tail:])
    return files


def checked_plan(plan, owner, boot):
    require(plan['schema'] == SCHEMA, 'Wrong Library admission experiment')
    return BASE.check_plan({**plan, 'schema': 'bili-shelf-ab-v1'}, owner, boot)


def check_case_identity(case, plan):
    batch = CONTROL.private(Path(case['batch']))
    admission = CONTROL.load(CONTROL.plain(batch / 'admission.json', 65536))
    require((admission['run'], admission['owner'], admission['host_boot']) == (case['run'], plan['owner'], plan['host_boot'])
            and CONTROL.load(CONTROL.plain(batch / MARK, 1024)) == binding(case), 'Wrong batch or launcher arm binding')
    return batch, admission


def prepare(args):
    owner, boot = CONTROL.identity()
    plan = {**BASE.make_plan(owner, boot), 'schema': SCHEMA}
    cases = checked_plan(plan, owner, boot)
    stock, candidate = sources(args)
    destination = args.output
    require(destination.is_absolute() and destination.resolve() == destination, 'Expected a new absolute output directory')
    destination.mkdir(mode=0o700)
    CONTROL.private(destination)
    CONTROL.save(destination / 'plan.json', plan)
    CONTROL.save(destination / 'inputs.json', {name: str(getattr(args, name)) for name in INPUTS})
    with tempfile.TemporaryDirectory(dir=destination) as temporary:
        kit = Path(temporary) / 'kit'
        ROUTE.assemble(argparse.Namespace(**{**vars(args), 'output': kit, 'run': cases[0]['run']}), ROUTE.demo_roster())
        CONTROL.write_once(destination / 'seed.sqlite3', (kit / 'data/demo/state.sqlite3').read_bytes())
        for case in cases:
            for leaf, data in case_files(stock, candidate, case).items():
                (kit / leaf).write_bytes(data)
            CONTROL.prepare(kit, args.native_kit, args.rollout_root)
            CONTROL.save(Path(case['batch']) / MARK, binding(case))
    print(destination / 'plan.json', flush=True)


def matched_stream(stream, expected, size):
    if isinstance(expected, bytes):
        return size == len(expected) and stream.read() == expected
    if size != expected.stat().st_size:
        return False
    with expected.open('rb') as source:
        for chunk in iter(lambda: source.read(1 << 20), b''):
            if stream.read(len(chunk)) != chunk:
                return False
    return not stream.read(1)


def verify_archive(route, expected):
    remaining = dict(expected)
    with tarfile.open(route, 'r|gz') as archive:
        for member in archive:
            require(member.name in remaining and member.type == tarfile.REGTYPE and not member.linkname and not member.pax_headers
                    and member.uid == member.gid == 0 and member.mode == (0o700 if member.name in ('route-fs', 'app/bilicomics', 'app/kindle-jpeg-worker') else 0o600)
                    and matched_stream(archive.extractfile(member), remaining.pop(member.name), member.size), 'Wrong archived launcher arm, metadata or route bytes')
    require(not remaining, 'Incomplete route archive')


def ready(directory, plan, cases):
    saved = CONTROL.load(CONTROL.plain(directory / 'inputs.json', 8192))
    require(set(saved) == set(INPUTS), 'Invalid prepared inputs')
    paths = argparse.Namespace(**{name: Path(path) for name, path in saved.items()})
    stock, candidate = sources(paths)
    shared = {'shipping-launcher.sh': stock, 'document-launcher.sh': ROUTE.DOCUMENT, 'data/device-profile': b'scribe\n',
              'data/device-allowlist': b'scribe 5.19.5\n', 'cleanup-assets': ROUTE.demo_roster(),
              'data/demo/state.sqlite3': CONTROL.plain(directory / 'seed.sqlite3', 1 << 20),
              'app/bilicomics': paths.application, 'app/kindle-jpeg-worker': CONTROL.plain(paths.worker, 150 << 20),
              'route-fs': CONTROL.plain(paths.route_fs, 150 << 20)}
    guard = CONTROL.plain(BASE.STUDY / 'host_tools/native_guard.sh', 128 << 10).read_bytes()
    detector = CONTROL.plain(paths.native_kit, 200 << 20)
    for case in cases:
        batch, admission = check_case_identity(case, plan)
        require({leaf.name for leaf in batch.iterdir()} == BASE.PREPARED | {MARK}
                and CONTROL.plain(batch / 'native-guard.sh', 128 << 10).read_bytes() == guard
                and CONTROL.plain(batch / 'askpass', 256).read_bytes() == CONTROL.ASKPASS, 'Altered or attempted visit')
        probe = CONTROL.plain(batch / 'probe.tar.gz', 200 << 20)
        route = CONTROL.plain(batch / 'route.tar.gz', 200 << 20)
        require((route.stat().st_size, probe.stat().st_size) == (admission['route_bytes'], admission['probe_bytes']), 'Changed prepared archive size')
        with probe.open('rb') as stream:
            require(matched_stream(stream, detector, probe.stat().st_size), 'Detector differs from its qualified input')
        expected = {**shared, **case_files(stock, candidate, case)}
        del expected['run-id']
        verify_archive(route, expected)


def closed_case(case, plan, previous_boot):
    batch, _ = check_case_identity(case, plan)
    released, retired, card, stored, origin = (CONTROL.load(batch / leaf) for leaf in
        ('released.json', 'retired.json', 'card.json', 'intervals.json', 'origin.json'))
    require(released['owner_thread'] == plan['owner'] and released['run_id'] == case['run'] and released['state'] == 'released'
            and released['device_disposition'] == 'original-native-restored' and released['owner_and_token_checked'] is True
            and released['native_restored_and_connections_closed'] is True, 'Native return and exact release not confirmed')
    require(not any((batch / leaf).exists() for leaf in ('stop-intended', 'partial-trace', 'failure.json', 'private-token'))
            and retired['natural'] is True and type(retired['app_pid']) is int and retired['app_pid'] > 0
            and (card['process_id'], card['run_id'], card['boot_id']) == (retired['app_pid'], case['run'], origin['boot'])
            and previous_boot in (None, origin['boot']), 'Missing natural retirement or changed device boot')
    rows = BOUNDS.read_jsonl(CONTROL.plain(batch / (case['run'] + '.jsonl'), CONTROL.MAX_TRACE))
    measured = BOUNDS.audit(card, rows)
    require(measured == stored and measured['renderer_return_interval_ns'] is not None
            and [row['data']['shelf_state'] for row in rows if row['stage'] == 'shelf_rendered'] in (['ready'], ['empty']),
            'No unique normal Shelf render for this visit')
    return origin['boot']


@contextmanager
def exclusive_directory(directory):
    descriptor = os.open(CONTROL.private(directory), os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
        yield
    finally:
        os.close(descriptor)


def discard(plan, cases):
    for case in cases:
        batch = Path(case['batch'])
        if not batch.exists():
            continue
        with exclusive_directory(batch):
            if (batch / 'attempted').exists():
                continue
            check_case_identity(case, plan)
            require({leaf.name for leaf in batch.iterdir()} == BASE.PREPARED | {MARK}, 'Unsafe unattempted batch roster')
            files = [CONTROL.plain(batch / leaf, 200 << 20) for leaf in sorted(BASE.PREPARED | {MARK})]
            for path in files:
                path.unlink()
            batch.rmdir()


def start(directory, plan, cases):
    require(not (directory / 'started.json').exists(), 'Refusing to repeat or append a study')
    ready(directory, plan, cases)
    require(sys.stdin.isatty(), 'Private foreground terminal required')
    with warnings.catch_warnings():
        warnings.simplefilter('error', getpass.GetPassWarning)
        password = getpass.getpass('Kindle authentication for this series (not saved): ')
    require(password, 'No authentication provided')
    CONTROL.save(directory / 'started.json', {'owner': plan['owner'], 'host_boot': plan['host_boot']})
    previous = None
    try:
        for index, case in enumerate(cases, 1):
            if index > 1 and (directory / HOLD).exists():
                CONTROL.announce(f'Off-lease handoff before visit {index}; no reservation.')
                while (directory / HOLD).exists():
                    time.sleep(.5)
            CONTROL.announce(f"visit={index}/{len(cases)} pair={case['pair']} arm={case['arm']} batch={case['batch']}")
            visit = CONTROL.Operator(Path(case['batch']))
            visit.read_password = lambda: password
            visit.execute(False)
            previous = closed_case(case, plan, previous)
    finally:
        password = ''


def main():
    os.umask(0o077)
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='operation', required=True)
    fixture = commands.add_parser('materialize', help='Recreate the research-only B launcher in a new file')
    fixture.add_argument('--stock', type=Path, required=True)
    fixture.add_argument('--output', type=Path, required=True)
    preparation = commands.add_parser('prepare')
    for name in (*INPUTS, 'output'):
        preparation.add_argument('--' + name.replace('_', '-'), type=Path, required=True)
    for operation in ('start', 'check', 'discard'):
        commands.add_parser(operation).add_argument('--plan', type=Path, required=True)
    args = parser.parse_args()
    if args.operation == 'materialize':
        candidate = edited_stock(args.stock.read_bytes())
        with args.output.open('xb') as output:
            output.write(candidate)
        return
    if args.operation == 'prepare':
        prepare(args)
        return
    def interrupted(*unused):
        raise InterruptedError('Operation interrupted')
    signal.signal(signal.SIGHUP, interrupted)
    signal.signal(signal.SIGTERM, interrupted)
    with exclusive_directory(args.plan.parent):
        require(args.plan.name == 'plan.json', 'Expected registered plan path')
        plan = CONTROL.load(CONTROL.plain(args.plan, 65536))
        cases = checked_plan(plan, *CONTROL.identity())
        if args.operation == 'discard':
            discard(plan, cases)
        elif args.operation == 'check':
            ready(args.plan.parent, plan, cases)
            CONTROL.announce('All 14 prepared archives matched their original inputs.')
        else:
            start(args.plan.parent, plan, cases)


if __name__ == '__main__':
    try:
        main()
    except (CONTROL.RecoveryRequired, CONTROL.SampleUnavailable, CONTROL.OperatorError) as error:
        print(error, file=sys.stderr)
        sys.exit(3 if isinstance(error, CONTROL.RecoveryRequired) else 4 if isinstance(error, CONTROL.SampleUnavailable) else 2)
    except (Exception, KeyboardInterrupt):
        sys.exit('Operation stopped. Check the original private series and last visit; diagnostics withheld.')
