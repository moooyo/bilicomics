"""Run independently leased, strictly offline ordinary Shelf timing visits."""

import argparse
import importlib.util
import io
import json
import os
from pathlib import Path
import re
import secrets
import signal
import sys
import tarfile
import time
import uuid


HERE = Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location('cover_overlap', HERE.parent / 'startup-cover-overlap-20261126/compare.py')
OVERLAP = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(OVERLAP)
CLOCK, CORE, BOUNDS, COLLECT = OVERLAP.CLOCK, OVERLAP.CONTROL, OVERLAP.AUDIT, OVERLAP.COLLECT
PHASE = CORE.module(HERE / 'project_phases.py')
require, once = CORE.require, CLOCK.once
SCHEMA = 'bili-ordinary-offline-shelf-phases-v1'
MOUNT = 'private_session_mount.sh'
MOUNT_INTENT, MOUNT_ID, MOUNT_RESTORED = ('mount-intent.json', 'mount-id.json', 'mount-restored')
SCHEDULE = ('ready', 'ready', 'ready', 'empty')
INPUTS = ('application', 'worker', 'stock', 'launcher', 'document', 'route_fs', 'clock_marks', 'detector', 'rollout_root',
          'fixture', 'runtime_roster')
CONTROL_FILES = CLOCK.CONTROL.FILES - {'data/demo/state.sqlite3'}
EXTRAS = ('data/state.sqlite3-shm', 'data/state.sqlite3-wal', 'data/state.sqlite3-journal')
DB, SESSION = 'state.sqlite3', 'account/private/session.json'
SEED_DIRS = {'account', 'account/private', 'covers'}
PERSONA = re.compile(r'accounts/[0-9a-f]{64}')
HEX = r'[0-9a-f]{32}'
OBJ = r'[0-9]{1,20}:[1-9][0-9]{0,19}:[0-9a-f]{1,8}:[0-9]{1,20}:[0-9]{1,20}'
BASE = rf'{OBJ}/{OBJ}/{OBJ}'
VOLUME = rf'[1-9][0-9]{{0,19}}:[0-9]{{1,20}}:[0-9]{{1,20}}:{HEX}'
NEW_ARGS = b' --startup-phases "$DATA_DIR/@RUN@.startup-phases.json"'


def route_files(stock, launcher, run, seed, runtime):
    bound = CLOCK.case_files(launcher, run)
    script = once(bound['launch.sh'], b' --demo --offline ', b' --offline ')
    script = once(script, b' --lifecycle-trace "$DATA_DIR/' + run.encode() + b'-lifecycle.jsonl"',
                  NEW_ARGS.replace(b'@RUN@', run.encode()))
    source = OVERLAP.ROUTE.case_files(stock, run, False)['switch.sh']
    switch = OVERLAP.current_stock_guard(CLOCK.clock_switch(source, script, bound['entry.sh']))
    switch = once(switch, b'"data/$run-lifecycle.jsonl"', b'"data/$run.startup-phases.json"')
    switch = once(switch, b'data/$run-lifecycle.jsonl', b'data/$run.startup-phases.json')
    roster = ''.join(f'data/{name}\n' for name in sorted(seed)).encode('ascii')
    roster_check = (b"    plain \"$stage/cleanup-assets\" && cmp -s - \"$stage/cleanup-assets\" <<'BILI_PHASE_SEED' || fail 'unexpected ordinary seed roster'\n"
                    + roster + b'BILI_PHASE_SEED\n')
    start = b'    plain "$stage/cleanup-assets" || fail \'missing cleanup roster\''
    end = b'    clean_tree "$stage" verify\n    clean_tree "$stage" prune'
    require(switch.count(start) == switch.count(end) == 1, 'Unknown cleanup boundary')
    switch = switch[:switch.index(start)] + roster_check + switch[switch.index(end):]
    switch = once(switch, b'data/demo|data/demo/fixtures|data/demo/fixtures/covers|data/demo/fixtures/pages',
                  '|'.join('data/' + name for name in sorted(SEED_DIRS | set(runtime))).encode())
    switch = once(switch, b'data/demo/state.sqlite3|data/demo/state.sqlite3-shm|data/demo/state.sqlite3-wal|data/demo/state.sqlite3-journal)',
                  ('|'.join(('data/' + DB, 'data/' + SESSION, *EXTRAS)) + ')').encode())
    start = b'            data/demo/fixtures/.demo-*.pending'
    end = b"            *) fail 'unknown staged contents; no bulk removal';;"
    require(switch.count(start) == switch.count(end) == 1, 'Unknown runtime cleanup boundary')
    switch = switch[:switch.index(start)] + b'''            data/covers/*.png)
                grep -Fxq "$relative" "$stage/cleanup-assets" || fail 'unrecognized ordinary cover';;
            data/account/private/.fixture-owner)
                [ "$operation:$2" = activate:verify ] && plain "$path" && [ "$(stat -c '%a:%s' "$path")" = 600:50 ] || fail 'unrecognized mount seal';
                continue;;
''' + switch[switch.index(end):]
    switch = once(switch, b'            directory "$stage/data/demo" && plain "$stage/data/demo/state.sqlite3" &&',
                  b'            directory "$stage/data/account/private" && directory "$stage/data/covers" && plain "$stage/data/state.sqlite3" &&')
    switch = once(switch, b'rename-probe data/fonts data/logs data/demo/fixtures data/demo/state.sqlite3-shm \\\n            data/demo/state.sqlite3-wal data/demo/state.sqlite3-journal',
                  ' '.join(('rename-probe', 'data/fonts', 'data/logs', *EXTRAS, *('data/' + name for name in runtime))).encode())
    switch = once(switch, b'clean_stage() {', mount_check() + b'''clean_stage() {
    mounts_match 0 || fail 'refusing cleanup with a staged mount'
'''.rstrip())
    switch = once(switch, b'        clean_tree "$stage" verify\n        launcher_verified',
                  b'''        mounts_match 1 && plain "$stage/data/account/private/.fixture-owner" || fail 'private mount not admitted'
''' + roster_check + b'''        while IFS= read -r leaf; do plain "$stage/$leaf" || fail 'missing seed file'; done < "$stage/cleanup-assets"
        clean_tree "$stage" verify
        launcher_verified''')
    return {**bound, 'launch.sh': script, 'switch.sh': switch, 'cleanup-assets': roster}


def mount_check():
    return rb'''mounts_match() {
    LC_ALL=C awk -v root="$stage" -v point="$stage/data/account/private" -v prefix="bili-private-$digits-" -v expected="$1" '
        { sep=7; while (sep<=NF && $sep!="-") sep++;
          if (NF<10 || sep+3>NF) bad=1; source=$(sep+2);
          if ($5==root || index($5,root "/")==1 || index(source,prefix)==1) {
            flags="," $6 ",";
            if ($5!=point || $4!="/" || $(sep+1)!="tmpfs" || index(source,prefix)!=1 || length(source)!=length(prefix)+16 || substr(source,length(prefix)+1)!~/^[0-9a-f]+$/ || flags!~/,ro,/ || flags!~/,nodev,/ || flags!~/,nosuid,/ || flags!~/,noexec,/) bad=1;
            count++;
          }
        } END { if (!NR || bad || count+0!=expected+0) exit 1 }' /proc/self/mountinfo
}

'''


def guard():
    original = (CLOCK.BASE.STUDY / 'host_tools/native_guard.sh').read_bytes()
    argv = CLOCK.BASE.GUARD_BINDING
    return once(once(original, b'--demo --offline', b'--offline'), argv,
                argv.replace(b' |', b' --startup-phases "$stage/data/$run.startup-phases.json" |'))


def seed_layout(files):
    require(type(files) is list and all(type(name) is str for name in files) and files == sorted(set(files)) and len(files) == 22
            and {DB, SESSION} <= set(files) and all(re.fullmatch(r'covers/[0-9a-f]{64}\.png', name)
                    for name in set(files) - {DB, SESSION}), 'Invalid synthetic fixture roster')
    return files


def inputs(args, case, *, route_builder=route_files):
    stock = CORE.plain(args.stock, 200_000).read_bytes()
    document = CORE.plain(args.document, 1024).read_bytes()
    launcher = CORE.plain(args.launcher, 200_000).read_bytes()
    # The retained product launcher is the stock script with the previously
    # qualified movement plus the exceptional read-reaping gate.
    guarded = (b'# Do not let a failed optional read leave a live owner of the shared reader\n'
               b'# records when the required display capture is about to reuse them.\n'
               b'if [ "$NATIVE_READ_REAPED" -ne 1 ]; then\n    stop_native_read\n'
               b'    [ "$NATIVE_READ_REAPED" -eq 1 ] || fail \'The startup sleep-cover reader did not retire.\'\nfi\n')
    require(document == OVERLAP.ICON_DOCUMENT and once(launcher, guarded, b'') == OVERLAP.overlap_launcher(stock),
            'Unexpected installed card or retained product launcher')
    for path in (args.application, args.worker, args.route_fs, args.clock_marks):
        OVERLAP.ROUTE.arm_executable(CORE.plain(path, 128 << 20))
    fixture = COLLECT.read_json(args.fixture, 8192)
    leaves = seed_layout(fixture['allowlisted_relative_files'])
    require(fixture['fixture_schema'] == 'bili-ordinary-offline-shelf-seed-v1' and fixture['mock_identity_validated'] is True
            and fixture['closed_checkpointed'] is True and fixture['counts'] == {'files_per_seed': 22, 'shelf_empty': 0,
            'shelf_filled': 20, 'store_both': 20}, 'Synthetic account qualification not present')
    kind = 'filled' if case['state'] == 'ready' else 'empty'
    seed = CORE.private(args.fixture.parent / kind / 'out/seed')
    require(str(seed) == fixture[kind + '_seed'] and {item.relative_to(seed).as_posix() for item in seed.rglob('*')}
            == set(leaves) | SEED_DIRS, 'Seed no longer matches the admitted layout')
    for directory in SEED_DIRS:
        CORE.private(seed / directory)
    runtime = COLLECT.read_json(args.runtime_roster, 4096)
    dirs = runtime['directories']
    account = [name for name in dirs if type(name) is str and PERSONA.fullmatch(name)]
    require(runtime['schema'] == 'bili-ordinary-offline-runtime-v1' and runtime['files'] == [] and len(account) == 1
            and len(dirs) == 3 and set(dirs) == {'accounts', account[0], account[0] + '/pages'}, 'Unknown runtime cleanup layout')
    data = {**route_builder(stock, launcher, case['run'], leaves, sorted(dirs)),
            'shipping-launcher.sh': stock, 'document-launcher.sh': document, 'data/device-profile': b'scribe\n',
            'data/device-allowlist': b'scribe 5.19.5\n', 'app/bilicomics': args.application,
            'app/kindle-jpeg-worker': args.worker, 'route-fs': args.route_fs, 'clock-marks': args.clock_marks}
    for leaf in leaves:
        path = CORE.plain(seed / leaf, 4096 if leaf == SESSION else 2_000_000)
        require(path.stat().st_mode & 0o077 == 0, 'Non-private fixture leaf')
        data['data/' + leaf] = path
    require(data.keys() == CONTROL_FILES | {'data/' + leaf for leaf in leaves}, 'Unexpected route members')
    return data, leaves


def prepared_batch(args, case, owner, boot, *, route_builder=route_files, guard_builder=guard):
    selected, leaves = inputs(args, case, route_builder=route_builder)
    batch = Path(case['batch'])
    detector = CORE.plain(args.detector, 200 << 20)
    receipt = COLLECT.read_json(detector.with_suffix('').with_suffix('.json'), 64 << 10)
    native = CORE.probe_roster(detector, receipt)
    batch.mkdir(mode=0o700)
    size = receipt['bytes']
    with tarfile.open(batch / 'route.tar.gz', 'x:gz', format=tarfile.USTAR_FORMAT, compresslevel=1) as archive:
        for name, value in sorted(selected.items()):
            if name == 'run-id':
                continue
            member = tarfile.TarInfo(name)
            member.mode = 0o700 if name in CLOCK.EXECUTABLES else 0o600
            member.size = len(value) if type(value) is bytes else value.stat().st_size
            with io.BytesIO(value) if type(value) is bytes else value.open('rb') as source:
                archive.addfile(member, source)
            size += member.size
    CORE.write_once(batch / 'probe.tar.gz', detector.read_bytes())
    CORE.write_once(batch / 'native-guard.sh', guard_builder())
    CORE.write_once(batch / MOUNT, CORE.plain(HERE / MOUNT, 16 << 10).read_bytes())
    CORE.write_once(batch / 'askpass', CORE.ASKPASS)
    (batch / 'askpass').chmod(0o700)
    CORE.save(batch / 'admission.json', {'run': case['run'], 'owner': owner, 'host_boot': boot, 'rollout_root': str(args.rollout_root),
              'native_roster': native, 'route_bytes': (batch / 'route.tar.gz').stat().st_size,
              'probe_bytes': detector.stat().st_size, 'minimum_media_kib': (size + CORE.MEDIA_RESERVE + 1023) // 1024})
    CORE.save(batch / 'case.json', {'case': case, 'seed_leaves': leaves})


def check_batch(args, case, *, route_builder=route_files, guard_builder=guard):
    batch = CORE.private(Path(case['batch']))
    require({path.name for path in batch.iterdir()} == CLOCK.BASE.PREPARED | {'case.json', MOUNT}, 'Batch is no longer fresh')
    selected, leaves = inputs(args, case, route_builder=route_builder)
    owner, boot = CORE.identity()
    admission = COLLECT.read_json(batch / 'admission.json', 64 << 10)
    payload = CORE.plain(batch / 'route.tar.gz', 200 << 20)
    detector = CORE.plain(args.detector, 200 << 20)
    detector_copy = CORE.plain(batch / 'probe.tar.gz', 200 << 20)
    detector_receipt = COLLECT.read_json(detector.with_suffix('').with_suffix('.json'), 64 << 10)
    require((admission['run'], admission['owner'], admission['host_boot'], admission['rollout_root']) ==
            (case['run'], owner, boot, str(args.rollout_root)) and admission['route_bytes'] == payload.stat().st_size
            and admission['probe_bytes'] == detector.stat().st_size == detector_copy.stat().st_size
            and sorted(detector_receipt['members']) == admission['native_roster'], 'Changed route admission or roster')
    require(COLLECT.read_json(batch / 'case.json', 8192) == {'case': case, 'seed_leaves': leaves}
            and CORE.plain(batch / 'askpass', 256).read_bytes() == CORE.ASKPASS
            and CORE.plain(batch / MOUNT, 16 << 10).read_bytes() == CORE.plain(HERE / MOUNT, 16 << 10).read_bytes()
            and CORE.plain(batch / 'native-guard.sh', 128 << 10).read_bytes() == guard_builder(), 'Changed prepared program or mount')
    CLOCK.verify_archive(payload, {key: value for key, value in selected.items() if key != 'run-id'})
    with detector_copy.open('rb') as copy:
        require(CLOCK.FROZEN.matched_stream(copy, detector, detector.stat().st_size), 'Changed public-card detector')


def receipt(data, name, pattern):
    match = re.fullmatch(rf'{name}=({pattern})\n', data.decode('ascii'))
    require(match is not None, 'Unrecognized private-session mount receipt')
    return match[1]


def sidecar_command(run, stage, boot):
    # Reuse the pre-tested numeric shim/app bindings, but do not enable the
    # general lifecycle logger. Only the two clock helper records and the new
    # bounded opt-in sidecar are read, never the application's log or database.
    script = COLLECT.capture_command(run, stage, boot).split('\nlife=', 1)[0]
    return script + f'''
leaf="$root/data/{run}.startup-phases.json"
plain "$leaf" {PHASE.LIMIT} || exit 1
head -c {PHASE.LIMIT + 1} "$leaf"
test "$(cat /proc/sys/kernel/random/boot_id)" = {boot}'''


def parse_sidecar(raw, pid):
    require(type(raw) is bytes and 0 < len(raw) <= PHASE.LIMIT + 512, 'Invalid phase capture size')
    lines = raw.split(b'\n', 3)
    require(len(lines) == 4, 'Incomplete phase capture')
    values = []
    for label, line in zip((b'picked', b'shim_entry', b'app_preinvoke'), lines):
        match = re.fullmatch(label + rb' ([1-9][0-9]{0,9}) ([1-9][0-9]{0,18})', line)
        require(match is not None, 'Invalid clock helper receipt')
        values.append(tuple(map(int, match.groups())))
    ((entry, _), (shim, shim_ns), (app, invoke_ns)) = values
    require(entry == shim != app == pid and 0 < shim_ns <= invoke_ns <= PHASE.MAX_INT, 'Unbound shim or application clock')
    return {'launch_entry': shim_ns, 'pre_app_invocation': invoke_ns}, PHASE.decode(lines[-1])


class Visit(OVERLAP.Visit):
    def __init__(self, path):
        super().__init__(path)
        row = COLLECT.read_json(path / 'case.json', 8192)
        require(row['case']['run'] == self.run and row['case']['batch'] == str(path), 'Wrong prepared case')
        self.seed_files = {'data/' + name for name in seed_layout(row['seed_leaves'])}
        self.helper = CORE.plain(path / MOUNT, 16 << 10).read_bytes()
        require(self.helper == CORE.plain(HERE / MOUNT, 16 << 10).read_bytes(), 'Changed private mount helper')

    def ensure_tab(self, tab):
        require(tab in ('home', 'library'), 'Unknown native tab')
        requested = False
        for attempt in range(7):
            try:
                current = self.probe('navigation')
            except CORE.OperatorError as error:
                # A confirmed tab change can transiently unset native idle.
                # Repeat only its read-only admission guard, never a click or
                # an ambiguous SSH request, and only before the timed release.
                if not (requested and self.probe_step == 'native-guard'
                        and str(error) == 'Scoped SSH failed (1; native-not-idle)'):
                    raise
            else:
                current_tab = self.shared.paired.selected(current)
                self.probe_step = 'receipt-accepted'
                if current_tab == tab:
                    return current
                if not requested:
                    result = self.probe('nav-' + tab)
                    require(result.get('ok') is True and result.get('target') == 'nav-' + tab
                            and result.get('buttons_clear_after') is True, 'Native tab input unconfirmed')
                    requested = True
                    self.tab_dispatch, self.probe_step = 'receipt-confirmed', 'receipt-accepted'
            if attempt < 6:
                time.sleep(.5)
        raise CORE.OperatorError('Native tab not confirmed')

    def no_staged_mounts(self):
        self.raw(f'stage={self.route}; digits={self.run.removeprefix("bili-shelf-")}\n' + mount_check().decode() + '\nmounts_match 0')

    def mount_call(self, action, owner, *values):
        patterns = {'inspect-base': (), 'attach': (BASE,), 'inspect-ready': (), 'inspect-mounted': (), 'detach': (VOLUME, BASE)}
        require(type(owner) is str and re.fullmatch(HEX, owner) and CORE.UUID.fullmatch(self.boot) and action in patterns
                and len(values) == len(patterns[action]) and all(type(value) is str and re.fullmatch(pattern, value)
                for pattern, value in zip(patterns[action], values)), 'Invalid mount owner or operation')
        return self.raw(' '.join(('sh -s --', self.run, self.boot, action, owner, *values)), self.helper)

    def switch(self, verb):
        if verb == 'activate':
            require(COLLECT.flag_present(self.path, 'activation-intended') and not os.path.lexists(self.path / MOUNT_INTENT)
                    and super().switch('inspect') == 'stock' and self.guard('native') == self.boot,
                    'Private mount requires fresh native control and stock launcher')
            self.no_staged_mounts()
            owner = secrets.token_hex(16)
            base = receipt(self.mount_call('inspect-base', owner), 'base', BASE)
            CORE.save(self.path / MOUNT_INTENT, {'run': self.run, 'boot': self.boot, 'owner': owner, 'base': base})
            identity = receipt(self.mount_call('attach', owner, base), 'mounted', VOLUME)
            CORE.save(self.path / MOUNT_ID, {'identity': identity})
            require(receipt(self.mount_call('inspect-ready', owner), 'mounted', VOLUME) == identity, 'Private mount not ready')
        return super().switch(verb)

    def detach(self):
        intent = self.path / MOUNT_INTENT
        saved = self.path / MOUNT_ID
        if os.path.lexists(intent) and not COLLECT.flag_present(self.path, MOUNT_RESTORED):
            row = COLLECT.read_json(intent, 2048)
            require(row['run'] == self.run and row['boot'] == self.boot and self.guard('native') == self.boot
                    and COLLECT.flag_present(self.path, 'stock-restored') and COLLECT.flag_present(self.path, 'navigation-restored'),
                    'Mount removal requires the original run and fully restored native control')
            owner, base = row['owner'], row['base']
            try:
                present = receipt(self.mount_call('inspect-mounted', owner), 'mounted', VOLUME)
            except CORE.OperatorError:
                require(receipt(self.mount_call('inspect-base', owner), 'base', BASE) == base, 'Original seed base not confirmed')
            else:
                if os.path.lexists(saved):
                    require(COLLECT.read_json(saved, 2048) == {'identity': present}, 'Private mount identity has changed')
                else:
                    CORE.save(saved, {'identity': present})
                require(self.mount_call('detach', owner, present, base) == b'restored\n'
                        and receipt(self.mount_call('inspect-base', owner), 'base', BASE) == base, 'Private mount not restored')
            CORE.flag(self.path / MOUNT_RESTORED)
        self.no_staged_mounts()

    def step(self, phase):
        super().step(phase)
        if phase == 'remove-route-stage':
            self.detach()

    def cleanup(self):
        previous = CORE.FILES
        CORE.FILES = CONTROL_FILES | self.seed_files
        try:
            return super().cleanup()
        finally:
            CORE.FILES = previous

    def remove_route(self):
        previous = CORE.FILES
        CORE.FILES = CONTROL_FILES
        try:
            super().remove_route()
        finally:
            CORE.FILES = previous

    def collect(self):
        CORE.Operator.collect(self)
        COLLECT.visit_identity(self.path, self.run, self.boot)
        raw = self.raw(sidecar_command(self.run, self.route, self.boot))
        require(0 < len(raw) <= PHASE.LIMIT + 512, 'Invalid phase capture size')
        # This source is a fixed-schema numeric-only diagnostic; validate and
        # project after exact release, outside the device-operation batch.
        CORE.write_once(self.path / 'captured-phases.txt', raw)


def observation(case):
    path = Path(case['batch'])
    run, boot, _, disposition = COLLECT.closure(path)
    require(run == case['run'] and disposition == 'original-native-restored' and COLLECT.flag_present(path, MOUNT_RESTORED),
            'App, mount or native restoration is not confirmed')
    pid, card = COLLECT.visit_identity(path, run, boot)
    require(CORE.load(path / 'observed-click.json') == {'run_id': run, 'boot_id': boot, 'click': card['click']}, 'Different release origin')
    rows = BOUNDS.narrow_rows(BOUNDS.read_jsonl(CORE.plain(path / (run + '.jsonl'), CORE.MAX_TRACE)), run, boot, process_id=pid)
    audit = BOUNDS.audit(card, rows)
    require(audit == COLLECT.read_json(path / 'intervals.json', 4096), 'Different Shelf witness')
    events = {row['stage']: row['data'] for row in rows[1:-1]}
    draw = events.get('shelf_rendered')
    require(draw is not None and draw['shelf_state'] == case['state'] and audit['renderer_return_interval_ns'] is not None,
            'Expected the planned ordinary ready or empty Shelf')
    send, driver = events.get('shelf_submitted'), events.get('shelf_driver_completed')
    clocks, profile = parse_sidecar(CORE.plain(path / 'captured-phases.txt', PHASE.LIMIT + 512).read_bytes(), pid)
    projected = PHASE.project(profile, run=run, boot=boot, pid=pid, renderer_ns=draw['rendered_monotonic_ns'],
              render_order=audit['render_order'], send=(send['update_marker'], send['submitted_monotonic_ns']) if send else None,
              completed=(driver['update_marker'], driver['completed_monotonic_ns']) if driver else None)
    lo, hi = audit['card_release_bracket_ns']
    launch, invoke, main = clocks['launch_entry'], clocks['pre_app_invocation'], projected.pop('process_enter_ns')
    PHASE.ordered(lo, hi, launch, invoke, main, rows[0]['monotonic_ns'], draw['rendered_monotonic_ns'])
    endpoint = audit['landmarks_ns']
    return boot, {'visit': case['visit'], 'shelf_state': case['state'], 'render_order': audit['render_order'],
                  **{field: audit[field] for field in OVERLAP.ENDPOINTS}, 'original_native_restored': True,
                  'exact_lease_released': True, 'card_to_launcher_ns': [launch - hi, launch - lo],
                  'launcher_to_preinvoke_ns': invoke - launch, 'preinvoke_to_main_ns': main - invoke,
                  'render_to_send_ns': None if send is None else endpoint['shelf_submitted'] - endpoint['shelf_rendered'],
                  'send_to_driver_ns': None if driver is None else endpoint['shelf_driver_completed'] - endpoint['shelf_submitted'],
                  **projected}


def plan_cases(plan):
    owner, boot = CORE.identity()
    require((plan['schema'], plan['owner'], plan['boot']) == (SCHEMA, owner, boot) and len(plan['cases']) == len(SCHEDULE),
            'Wrong phase series or host identity')
    seen = set()
    for index, (state, case) in enumerate(zip(SCHEDULE, plan['cases']), 1):
        run = case['run']
        require(CORE.RUN.fullmatch(run) and run not in seen and case == {'visit': index, 'state': state, 'run': run,
                'batch': str(CLOCK.BASE.batch_path(run))}, 'Reused or reordered visit identity')
        seen.add(run)
    return plan['cases']


def main():
    os.umask(0o077)
    cli = argparse.ArgumentParser(description=__doc__)
    commands = cli.add_subparsers(dest='operation', required=True)
    setup = commands.add_parser('prepare')
    for key in (*INPUTS, 'output'):
        setup.add_argument('--' + key.replace('_', '-'), type=Path, required=True)
    for name in ('check', 'start', 'recover', 'report'):
        sub = commands.add_parser(name)
        sub.add_argument('--plan', type=Path, required=True)
        if name in ('start', 'recover'):
            sub.add_argument('--visit', type=int, choices=range(1, len(SCHEDULE) + 1), required=True)
    args = cli.parse_args()
    if args.operation == 'prepare':
        owner, boot = CORE.identity()
        CORE.tools(args.rollout_root).pinned('10.0.0.238')
        args.output.mkdir(mode=0o700)
        CORE.private(args.output)
        cases = []
        for index, state in enumerate(SCHEDULE, 1):
            run = 'bili-shelf-' + uuid.uuid4().hex
            cases.append({'visit': index, 'state': state, 'run': run, 'batch': str(CLOCK.BASE.batch_path(run))})
        CORE.save(args.output / 'plan.json', {'schema': SCHEMA, 'owner': owner, 'boot': boot, 'cases': cases,
                  'inputs': {name: str(getattr(args, name)) for name in INPUTS}})
        for case in cases:
            prepared_batch(args, case, owner, boot)
        print(args.output / 'plan.json', flush=True)
        return
    plan = COLLECT.read_json(args.plan, 8192)
    cases = plan_cases(plan)
    inputs = argparse.Namespace(**{name: Path(plan['inputs'][name]) for name in INPUTS})
    if args.operation == 'check':
        for case in cases:
            check_batch(inputs, case)
        print('All ordinary offline archives and guards match their qualified inputs.')
    elif args.operation == 'report':
        visits, stopped, device = [], None, None
        for case in cases:
            batch = Path(case['batch'])
            if not os.path.lexists(batch / 'attempted'):
                break
            try:
                boot, data = observation(case)
                require(device in (boot, None), 'Kindle rebooted during the series')
            except (OSError, ValueError, KeyError, TypeError):
                stopped = {'visit': case['visit'], 'qualified': False, 'exact_native_return_verified': False}
                try:
                    run, _, _, disposition = COLLECT.closure(batch, native_boot_required=False)
                    stopped['exact_native_return_verified'] = run == case['run'] and disposition in ('original-native-restored', 'no-device-change')
                except (OSError, ValueError, KeyError, TypeError):
                    pass
                break
            device = boot
            visits.append(data)
        print(json.dumps({'schema': SCHEMA + '-physical-results', 'device': 'Scribe 2022', 'firmware': '5.19.5',
              'origin': 'ordinary native Library card release', 'offline': True, 'demo': False,
              'synthetic_account': True, 'optical_measurement': False, 'visits': visits, 'stopped': stopped}, indent=2, allow_nan=False))
    else:
        signal.signal(signal.SIGHUP, OVERLAP.interrupted)
        signal.signal(signal.SIGTERM, OVERLAP.interrupted)
        case = cases[args.visit - 1]
        if args.operation == 'start':
            for previous in cases[:args.visit - 1]:
                observation(previous)
            check_batch(inputs, case)
        print(f"visit={case['visit']} expected={case['state']} batch={case['batch']}", flush=True)
        Visit(Path(case['batch'])).execute(args.operation == 'recover')
        observation(case)


if __name__ == '__main__':
    try:
        main()
    except CORE.RecoveryRequired as error:
        sys.exit(f'Recovery required: {error}')
    except CORE.OperatorError as error:
        sys.exit(str(error))
    except (OSError, ValueError, KeyError, TypeError, KeyboardInterrupt):
        sys.exit('Ordinary phase visit unavailable; no timing inferred. Raw output withheld.')
