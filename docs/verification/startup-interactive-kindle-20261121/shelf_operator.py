"""Prepare, operate or close one leased, automatically activated Shelf-render study."""

import argparse
import fcntl
import getpass
import importlib.util
import json
import os
from pathlib import Path
import re
import shlex
import signal
import socket
import stat
import subprocess
import sys
import tarfile
import tempfile
import time
import warnings


HERE = Path(__file__).resolve().parent
HOST = Path('/var/tmp')
LEASE = Path('/tmp/codex-kindle-physical-test.lock')
RUN = re.compile(r'bili-shelf-[0-9a-f]{32}')
UUID = re.compile(r'[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}')
FILES = frozenset(('switch.sh', 'route-fs', 'run-id', 'entry.sh', 'entry-ready.sh', 'shipping-launcher.sh',
                   'document-launcher.sh', 'launch.sh', 'app/bilicomics', 'app/kindle-jpeg-worker',
                   'data/device-profile', 'data/device-allowlist', 'data/demo/state.sqlite3', 'cleanup-assets'))
PROBE = re.compile(r'(?:native-target|card-input|lib/[A-Za-z0-9._+-]+|models/(?:eng|chi_sim)\.traineddata)')
REQUIRED = {'native-target', 'card-input', 'lib/ld-linux-armhf.so.3', 'lib/libX11.so.6',
            'lib/libtesseract.so.5', 'lib/libc.so.6', 'models/eng.traineddata', 'models/chi_sim.traineddata'}
ASKPASS = b'#!/bin/sh\ncase "$1" in *assword:*) printf "%s\\n" "$BILI_ASKPASS";; *) exit 1;; esac\n'
MAX_TRACE = 16 * 1024
MEDIA_RESERVE = 64 * 1024 * 1024
PROBE_ACTIONS = {'navigation': 'read-navigation', 'card': 'identify-card', 'nav-home': 'request-target-tab',
                 'nav-library': 'request-target-tab', 'restore-pointer': 'restore-pointer', 'open-card': 'open-card'}
PROBE_PHASES = ('native-navigation', 'identify-card', 'card-release', 'restore-navigation')
PROBE_STEPS = ('native-guard', 'operation-request', 'response-validation', 'receipt-accepted')
DISPATCH_STATES = ('not-requested', 'dispatch-possible', 'receipt-confirmed')


class OperatorError(ValueError):
    """A public diagnostic; never include unfiltered device or transport output."""


class RecoveryRequired(OperatorError):
    pass


class SampleUnavailable(OperatorError):
    pass


def public_reason(error):
    return str(error) if isinstance(error, OperatorError) else 'operation-failed'


def require(condition, reason):
    if not condition:
        raise OperatorError(reason)


def save(path, data):
    write_once(path, (json.dumps(data, separators=(',', ':'), allow_nan=False) + '\n').encode('ascii'))


def write_once(path, data):
    with tempfile.NamedTemporaryFile(dir=path.parent) as output:
        output.write(data)
        output.flush()
        os.fsync(output.fileno())
        os.link(output.name, path)


def load(path):
    return json.loads(path.read_bytes())


def flag(path):
    path.touch(exist_ok=True)


def announce(message):
    try:
        print(message, flush=True)
    except OSError:
        pass


def plain(path, maximum):
    info = path.lstat()
    require(path.is_absolute() and path.resolve(strict=True) == path and stat.S_ISREG(info.st_mode)
            and info.st_uid == 0 and info.st_nlink == 1 and not info.st_mode & 0o022 and 0 < info.st_size <= maximum,
            'Expected an unchanged root-owned regular input')
    return path


def private(path):
    info = path.lstat()
    require(path.is_absolute() and path.resolve(strict=True) == path and stat.S_ISDIR(info.st_mode)
            and info.st_uid == 0 and stat.S_IMODE(info.st_mode) == 0o700, 'Expected a private host directory')
    return path


def configured_owner():
    owner = os.environ.get('BILICOMICS_STUDY_OWNER', '')
    require(UUID.fullmatch(owner), 'An independently approved study owner UUID is required')
    return owner


def identity():
    owner = configured_owner()
    require(socket.gethostname().split('.')[0] == 'test-env' and os.geteuid() == 0 and os.environ.get('CODEX_THREAD_ID') == owner
            and Path('/var/tmp/kindle-build-coordination').is_dir(), 'Use the actual main thread on the test-env host')
    boot = Path('/proc/sys/kernel/random/boot_id').read_text().strip()
    require(UUID.fullmatch(boot), 'Missing host boot identity')
    return owner, boot


def module(path):
    spec = importlib.util.spec_from_file_location(path.stem, path)
    result = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(result)
    return result


def tools(path):
    private(path)
    for leaf in ('rollout.py', 'run-pair.py', 'prepare-pair.py', 'device_lease.py', 'decision.py'):
        plain(path / leaf, 1024 * 1024)
    sys.path.insert(0, str(path))
    return module(path / 'rollout.py')


def probe_roster(archive, receipt):
    names = receipt['members']
    require(type(names) is list and all(type(name) is str and PROBE.fullmatch(name) and '..' not in Path(name).parts
            and '.' not in name.split('/') for name in names) and len(names) == len(set(names)) <= 200
            and REQUIRED <= set(names) and type(receipt['bytes']) is int, 'Unqualified detector roster')
    with tarfile.open(archive, 'r:gz') as package:
        rows = package.getmembers()
        require(sorted(row.name for row in rows) == sorted(names) and all(row.type == tarfile.REGTYPE and row.name in names
                and not row.pax_headers and not row.linkname and row.uid == row.gid == 0
                and row.mode == (0o600 if row.name.startswith('models/') else 0o700)
                and 0 < row.size <= 64 * 1024 * 1024 for row in rows)
                and sum(row.size for row in rows) == receipt['bytes'] <= 200 * 1024 * 1024, 'Detector payload differs from its receipt')
    return sorted(names)


def prepare(kit, detector, rollout_root):
    owner, boot = identity()
    private(kit)
    run = plain(kit / 'run-id', 128).read_text().rstrip('\n')
    require(RUN.fullmatch(run) and (kit / 'run-id').read_bytes() == (run + '\n').encode(), 'Invalid diagnostic run')
    receipt = load(plain(detector.with_suffix('').with_suffix('.json'), 64 * 1024))
    native = probe_roster(plain(detector, 200 * 1024 * 1024), receipt)
    directories = {'app', 'data', 'data/demo'}
    require({file.relative_to(kit).as_posix() for file in kit.rglob('*')} == FILES | directories, 'Unexpected route kit roster')
    for directory in directories:
        private(kit / directory)
    expanded = receipt['bytes']
    for leaf in FILES:
        expanded += plain(kit / leaf, 150 * 1024 * 1024).stat().st_size
    shared = tools(rollout_root)
    shared.pinned('10.0.0.238')
    stage = HOST / ('bilicomics-interactive-' + run)
    stage.mkdir(mode=0o700)
    with tarfile.open(stage / 'route.tar.gz', 'x:gz', format=tarfile.USTAR_FORMAT, compresslevel=1) as output:
        for leaf in sorted(FILES - {'run-id'}):
            output.add(kit / leaf, arcname=leaf, recursive=False)
    (stage / 'probe.tar.gz').write_bytes(detector.read_bytes())
    (stage / 'native-guard.sh').write_bytes(plain(HERE / 'host_tools/native_guard.sh', 128 * 1024).read_bytes())
    (stage / 'askpass').write_bytes(ASKPASS)
    (stage / 'askpass').chmod(0o700)
    save(stage / 'admission.json', {'run': run, 'owner': owner, 'host_boot': boot, 'rollout_root': str(rollout_root),
                                    'native_roster': native, 'route_bytes': (stage / 'route.tar.gz').stat().st_size,
                                    'probe_bytes': (stage / 'probe.tar.gz').stat().st_size,
                                    'minimum_media_kib': (expanded + MEDIA_RESERVE + 1023) // 1024})
    print(stage)


def cleanup_payload(root, run, leaves):
    require(RUN.fullmatch(run) and root in ('/mnt/us/' + run, '/mnt/us/bili-probe-' + run), 'Invalid cleanup target')
    parents = set()
    for leaf in leaves:
        parents.update(str(parent) for parent in Path(leaf).parents if str(parent) != '.')
    allowed = set(leaves) | parents | {'run-id'}
    require(all(re.fullmatch(r'[A-Za-z0-9._+/-]+', item) and all(part not in ('', '.', '..') for part in item.split('/'))
                for item in allowed) and parents, 'Invalid cleanup roster')
    files = ' '.join(sorted(set(leaves) - {'run-id'}))
    dirs = ' '.join(sorted(parents, key=lambda item: (-item.count('/'), item)))
    return f'''trap '' HUP
if [ ! -e {root} ] && [ ! -L {root} ]; then exit 0; fi
plain() {{ [ -f "$1" ] && [ ! -L "$1" ] && [ "$(stat -c '%u:%h' "$1")" = 0:1 ]; }}
directory() {{ [ -d "$1" ] && [ ! -L "$1" ] && [ "$(stat -c %u "$1")" = 0 ]; }}
directory {root} && [ "$(readlink -f {root})" = {root} ] && cd {root} || exit 1
plain run-id && printf '%s\\n' {run} | cmp -s - run-id || exit 1
verify() (
 for item in "$1"/* "$1"/.[!.]* "$1"/..?*; do
  [ -e "$item" ] || [ -L "$item" ] || continue
  case "$item" in
   {'|'.join('./'+item for item in sorted(parents))}) directory "$item" && verify "$item" || exit 1;;
   {'|'.join('./'+item for item in sorted(allowed - parents))}) plain "$item" || exit 1;;
   *) exit 1;;
  esac
 done
)
verify . || exit 1
for file in {files}; do if [ -f "$file" ]; then rm "$file"; fi; done
for dir in {dirs}; do if [ -d "$dir" ]; then rmdir "$dir"; fi; done
rm run-id && cd / && rmdir {root}'''


class Operator:
    def __init__(self, path):
        owner, boot = identity()
        self.path = private(path)
        self.admission = load(plain(path / 'admission.json', 65536))
        self.run = self.admission['run']
        require(RUN.fullmatch(self.run) and path == HOST / ('bilicomics-interactive-' + self.run)
                and (owner, boot) == (self.admission['owner'], self.admission['host_boot']), 'Wrong run, host boot or owner')
        self.shared = tools(Path(self.admission['rollout_root']))
        self.lease = self.shared.device_lease
        self.request = self.lease.LeaseRequest(LEASE, path / 'private-token', owner, self.run)
        self.ssh = self.shared.pinned('10.0.0.238')
        self.guard_script = plain(path / 'native-guard.sh', 128 * 1024).read_bytes()
        self.bounds = module(HERE / 'log_bounds.py')
        self.route = '/mnt/us/' + self.run
        self.detector = '/mnt/us/bili-probe-' + self.run
        self.password = ''
        self.boot = (load(path / 'origin.json')['boot'] if (path / 'origin.json').exists() else None)
        require(self.boot is None or UUID.fullmatch(self.boot), 'Invalid saved device boot')
        self.phase = 'admission'
        self.probe_action = self.probe_step = None
        self.tab_dispatch = self.card_dispatch = 'not-requested'

    def step(self, phase):
        self.phase = phase
        self.probe_step = None
        announce(f'phase={phase}')

    def probe_failure(self):
        if self.phase not in PROBE_PHASES or self.probe_step not in PROBE_STEPS or self.probe_action not in PROBE_ACTIONS.values():
            return None
        if self.card_dispatch in DISPATCH_STATES and self.tab_dispatch in DISPATCH_STATES:
            return {'schema': 'bili-native-probe-failure-v1', 'phase': self.phase, 'action': self.probe_action,
                    'step': self.probe_step, 'tab_request': self.tab_dispatch, 'card_request': self.card_dispatch}
        return None

    def read_password(self):
        return getpass.getpass('Kindle authentication (not saved): ')

    def raw(self, command, data=None, deadline=25):
        self.lease.status(self.request)
        if self.boot is not None:
            command = f'test "$(cat /proc/sys/kernel/random/boot_id)" = {self.boot} || exit 1\n' + command
        response = subprocess.run([*self.ssh, 'kindle-physical', 'sh -c ' + shlex.quote('set -eu\n' + command)],
                                  input=data, stdin=subprocess.DEVNULL if data is None else None, capture_output=True,
                                  timeout=deadline, env={'HOME': '/root', 'LC_ALL': 'C',
                                  'PATH': os.defpath, 'SSH_ASKPASS': str(self.path / 'askpass'), 'DISPLAY': ':askpass',
                                  'SSH_ASKPASS_REQUIRE': 'force', 'BILI_ASKPASS': self.password})
        if response.returncode != 0:
            known = {b'Unexpected device or lock namespace.': 'device-profile-or-lock-namespace',
                     b'Native control is not established.': 'native-not-idle',
                     b'Retirement of the run and native restoration have not been established.': 'retirement-not-proven',
                     b'route: installed document, application or supported device profile differs': 'stock-profile-mismatch',
                     b'route: route paths or retained launcher do not match this run': 'unrecognized-route',
                     b'route: staged launcher does not match the single approved argv edit': 'launcher-argv-mismatch',
                     b'route: route volume not qualified; production launcher was not moved': 'volume-not-qualified',
                     b'route: transition interrupted; inspect and restore only': 'route-transition-incomplete'}
            reason = next((known[line] for line in response.stderr.splitlines() if line in known), 'withheld')
            raise OperatorError(f'Scoped SSH failed ({response.returncode}; {reason})')
        return response.stdout

    def guard(self, verb):
        require(verb in ('native', 'wait', 'stop'), 'Invalid native guard verb')
        return self.raw(f'sh -s -- {self.run} {verb}', self.guard_script, 75).decode('ascii').strip()

    def switch(self, verb):
        require(verb in ('inspect', 'activate', 'restore', 'cleanup'), 'Invalid route verb')
        values = self.raw(f'sh {self.route}/switch.sh {self.run} {verb}', deadline=120).decode('ascii').splitlines()
        require(len(values) == 2 and values[0] == 'run=' + self.run and values[1].startswith('route='), 'Unbound switch receipt')
        return values[1][6:]

    def upload(self, target, archive, journal):
        self.raw(f'test ! -e {target} && test ! -L {target}')
        flag(self.path / (journal + '-intended'))
        result = self.raw(f'umask 077; if ! mkdir {target}; then echo not-created; exit 0; fi\n'
                          f'cd {target}; set -C; printf "%s\\n" {self.run} >run-id')
        if result == b'not-created\n':
            flag(self.path / (journal + '-not-created'))
            raise OperatorError('Exclusive device staging directory could not be created')
        require(result == b'', 'Unexpected stage creation result')
        self.raw(f'test ! -L {target} && test "$(readlink -f {target})" = {target} && cd {target} || exit 1\n'
                 f'test -f run-id && test ! -L run-id && test "$(stat -c \'%u:%h\' run-id)" = 0:1 && '
                 f'printf "%s\\n" {self.run} | cmp -s - run-id || exit 1\n'
                 'for node in ./* ./.[!.]* ./..?*; do test -e "$node" || test -L "$node" || continue; '
                 'test "$node" = ./run-id || exit 1; done\ntar -xzf -',
                 archive, 240)

    def probe(self, mode, *point):
        require(mode in PROBE_ACTIONS, 'Unsupported native control')
        self.probe_action, self.probe_step = PROBE_ACTIONS[mode], 'native-guard'
        require(self.guard('native') == self.boot, 'Native control or device boot changed')
        kit = self.detector
        arguments = [kit + '/models']
        if mode == 'restore-pointer':
            require(len(point) == 2 and all(type(value) is int for value in point) and 0 <= point[0] < 1860 and 0 <= point[1] < 2480,
                    'Unqualified saved pointer')
            arguments = [str(value) for value in point]
        program = kit + ('/native-target' if mode in ('navigation', 'card') else '/card-input')
        command = shlex.join([kit + '/lib/ld-linux-armhf.so.3', '--inhibit-cache', '--library-path', kit + '/lib', program, mode, *arguments])
        self.probe_step = 'operation-request'
        if mode == 'open-card':
            self.card_dispatch = 'dispatch-possible'
            flag(self.path / 'click-intended')
        elif mode in ('nav-home', 'nav-library'):
            self.tab_dispatch = 'dispatch-possible'
        data = self.raw('DISPLAY=:0 OMP_THREAD_LIMIT=1 LC_ALL=C ' + command, deadline=55)
        self.probe_step = 'response-validation'
        require(len(data) <= 4096, 'Oversized native observation')
        return json.loads(data, object_pairs_hook=self.bounds.unique_object, parse_constant=self.bounds.reject_constant)

    def ensure_tab(self, tab):
        require(tab in ('home', 'library'), 'Unknown native tab')
        for attempt in range(5):
            observation = self.probe('navigation')
            selected = self.shared.paired.selected(observation)
            self.probe_step = 'receipt-accepted'
            if selected == tab:
                return observation
            if attempt == 0:
                result = self.probe('nav-' + tab)
                require(result.get('ok') is True and result.get('target') == 'nav-' + tab and
                        result.get('buttons_clear_after') is True, 'Native tab input unconfirmed')
                self.tab_dispatch, self.probe_step = 'receipt-confirmed', 'receipt-accepted'
            time.sleep(.5)
        raise OperatorError('Native tab not confirmed')

    def measure(self, route_data, probe_data):
        self.step('native-preflight')
        boot = self.guard('native')
        require(UUID.fullmatch(boot), 'Missing device boot from native preflight')
        self.boot = boot
        save(self.path / 'origin.json', {'boot': boot})
        self.step('media-capacity')
        minimum = self.admission['minimum_media_kib']
        require(type(minimum) is int and MEDIA_RESERVE // 1024 < minimum < 1024 * 1024, 'Invalid media admission')
        self.raw(f'''space=$(df -Pk /mnt/us | awk 'END {{print $(NF-2)}}')
case "$space" in ''|*[!0-9]*) exit 1;; esac
test "$space" -ge {minimum} && test ! -e {self.route} && test ! -L {self.route} &&
    test ! -e {self.detector} && test ! -L {self.detector}''')
        self.step('stage-detector')
        self.upload(self.detector, probe_data, 'detector')
        self.step('stage-route')
        self.upload(self.route, route_data, 'route')
        self.step('stock-inspect')
        require(self.switch('inspect') == 'stock', 'Production route is not stock')
        self.step('native-navigation')
        original = self.probe('navigation')
        tab, point = self.shared.paired.selected(original), original['pointer']
        require(tab in ('home', 'library') and type(point) is list and len(point) == 2 and all(type(value) is int for value in point)
                and 0 <= point[0] < 1860 and 0 <= point[1] < 2480, 'Invalid original pointer')
        self.probe_step = 'receipt-accepted'
        save(self.path / 'navigation.json', {'tab': tab, 'pointer': point})
        self.ensure_tab('library')
        self.step('identify-card')
        card = self.probe('card')
        require(self.shared.paired.selected(card) == 'library' and type(card['title']['count']) is int
                and card['title']['count'] == 1, 'Expected exactly one identified public card')
        self.probe_step = 'receipt-accepted'
        self.step('route-activate')
        flag(self.path / 'activation-intended')
        require(self.switch('activate') == 'armed', 'Route did not arm')
        self.step('card-release')
        click = self.probe('open-card')
        self.bounds.validate_click(click)
        require(click['up_before_ns'] <= click['up_after_ns'], 'Reversed native button release bracket')
        self.card_dispatch, self.probe_step = 'receipt-confirmed', 'receipt-accepted'
        save(self.path / 'observed-click.json', {'run_id': self.run, 'boot_id': boot, 'click': click})

    def retire(self):
        record = self.path / 'retired.json'
        if record.exists():
            return
        self.step('app-retirement')
        try:
            result = self.guard('wait')
            natural = True
        except (ValueError, OSError, subprocess.SubprocessError):
            self.step('scoped-native-recovery')
            require(not (self.path / 'stop-intended').exists(), 'The single scoped stop was already attempted')
            flag(self.path / 'stop-intended')
            result = self.guard('stop')
            natural = False
        match = re.fullmatch(r'app_pid=([1-9][0-9]{0,8})', result) if natural else None
        save(record, {'natural': natural, 'app_pid': int(match.group(1)) if match else None})

    def collect(self):
        filename = self.run + '.jsonl'
        leaf = self.route + '/data/' + filename
        retired = load(self.path / 'retired.json')
        pid = retired['app_pid']
        click_path = self.path / 'observed-click.json'
        if pid is not None and click_path.is_file():
            observed = load(click_path)
            require(observed['run_id'] == self.run and observed['boot_id'] == self.boot, 'Unbound saved card receipt')
            card = self.bounds.project_card_origin(observed['click'], run_id=self.run, boot_id=self.boot, process_id=pid)
            save(self.path / 'card.json', card)
        data = self.raw(f'test -f {leaf} && test ! -L {leaf} && test "$(readlink -f {leaf})" = {leaf} && '
                        f'test "$(stat -c \'%u:%h\' {leaf})" = 0:1 || exit 1\n'
                        f'size=$(stat -c %s {leaf}); test "$size" -gt 0 && test "$size" -le {MAX_TRACE} || exit 1\n'
                        f'head -c {MAX_TRACE + 1} {leaf}')
        require(0 < len(data) <= MAX_TRACE, 'Empty or oversized diagnostic')
        if not data.endswith(b'\n'):
            flag(self.path / 'partial-trace')
            data = data.rpartition(b'\n')[0]
        rows = [json.loads(line, object_pairs_hook=self.bounds.unique_object, parse_constant=self.bounds.reject_constant)
                for line in data.splitlines()]
        projected = self.bounds.narrow_rows(rows, self.run, self.boot, process_id=pid)
        write_once(self.path / filename, b''.join((json.dumps(row, separators=(',', ':'), allow_nan=False) + '\n').encode('ascii')
                                                for row in projected))

    def stock_receipt(self):
        root = '/mnt/us/bilicomics'
        directories = (self.route, root, root + '/app', root + '/data', '/mnt/us/documents')
        files = (root + '/launch.sh', root + '/app/bilicomics', root + '/app/kindle-jpeg-worker',
                 root + '/data/device-profile', root + '/data/device-allowlist', '/mnt/us/documents/bilicomics.sh')
        data = self.raw(f'''for depth in {' '.join(directories)}; do
test ! -L "$depth" && test "$(readlink -f "$depth")" = "$depth" && test -d "$depth" || exit 1
stat -c '%d:%i' "$depth"; done
for file in {' '.join(files)}; do
test ! -L "$file" && test -f "$file" && test "$(stat -c '%u:%h' "$file")" = 0:1 || exit 1
stat -c '%d:%i:%f:%s:%h:%Y' "$file"; done''').decode('ascii').splitlines()
        require(len(data) == len(directories) + len(files) and all(re.fullmatch(r'[0-9]+:[1-9][0-9]*', row)
                for row in data[:len(directories)]) and all(re.fullmatch(r'[0-9]+:[1-9][0-9]*:[a-f0-9]+:[0-9]+:1:[0-9]+', row)
                for row in data[len(directories):]), 'Invalid stock identity receipt')
        return data

    def remove_route(self):
        ticket = self.path / 'route-cleaning.json'
        if not ticket.exists():
            save(ticket, self.stock_receipt())
            require(self.switch('cleanup') == 'removed', 'Diagnostic route stage not removed')
            return
        state = self.raw(f'if test ! -e {self.route} && test ! -L {self.route}; then echo absent; else echo present; fi')
        if state == b'absent\n':
            return
        require(state == b'present\n' and self.guard('native') == self.boot and self.stock_receipt() == load(ticket),
                'Interrupted route cleanup no longer matches the original objects')
        control_files = FILES - {'data/demo/state.sqlite3'}
        prerequisites = ' '.join(sorted(control_files - {'run-id'}))
        tail = self.raw(f'''cd {self.route} || exit 1
for file in run-id {prerequisites}; do if ! test -f "$file"; then echo tail; exit 0; fi; done
echo complete''')
        if tail == b'complete\n':
            require(self.switch('cleanup') == 'removed', 'Diagnostic route stage not removed')
        else:
            require(tail == b'tail\n', 'Unrecognized route cleanup phase')
            self.raw(f'if test ! -e {self.route}/run-id && test ! -L {self.route}/run-id; then rmdir {self.route}; exit $?; fi\n'
                     + cleanup_payload(self.route, self.run, control_files | {'prepared', 'prepared.pending'}))

    def cleanup(self):
        if not (self.path / 'detector-intended').exists() or (self.path / 'detector-not-created').exists():
            return 'no-device-change'
        if (self.path / 'click-intended').exists():
            self.retire()
        if (self.path / 'activation-intended').exists() and not (self.path / 'stock-restored').exists():
            self.step('stock-restore')
            require(self.switch('restore') == 'stock', 'Production launcher not restored')
            flag(self.path / 'stock-restored')
        if (self.path / 'navigation.json').exists() and not (self.path / 'navigation-restored').exists():
            self.step('restore-navigation')
            self.tab_dispatch = 'not-requested'
            original = load(self.path / 'navigation.json')
            after = self.ensure_tab(original['tab'])
            if after['pointer'] != original['pointer']:
                require(self.probe('restore-pointer', *original['pointer'])['restored'] is True, 'Pointer restore failed')
                self.probe_step = 'receipt-accepted'
                after = self.probe('navigation')
            require(self.shared.paired.selected(after) == original['tab'] and after['pointer'] == original['pointer'], 'Native navigation differs')
            self.probe_step = 'receipt-accepted'
            flag(self.path / 'navigation-restored')
        if (self.path / 'retired.json').exists() and not (self.path / 'capture-attempted').exists():
            self.step('capture-anonymous-records')
            try:
                flag(self.path / 'capture-attempted')
                self.collect()
            except Exception:
                announce('No qualified capture; continuing restoration.')
        if (self.path / 'route-intended').exists() and not (self.path / 'route-not-created').exists():
            self.step('remove-route-stage')
            if (self.path / 'activation-intended').exists():
                self.remove_route()
            else:
                self.raw(cleanup_payload(self.route, self.run, FILES))
        self.step('remove-detector-stage')
        self.raw(cleanup_payload(self.detector, self.run, self.admission['native_roster']))
        self.step('verify-native-return')
        require(self.guard('native') == self.boot, 'Native restoration not confirmed')
        return 'original-native-restored'

    def analyze(self):
        release = load(self.path / 'released.json')
        require(release['state'] == 'released' and release['owner_and_token_checked'] is True
                and release['run_id'] == self.run and release['owner_thread'] == self.request.owner_thread,
                'Offline analysis requires the exact release receipt')
        try:
            require(not (self.path / 'partial-trace').exists(), 'Incomplete final record')
            result = self.bounds.audit(self.bounds.read_json(self.path / 'card.json'),
                                       self.bounds.read_jsonl(self.path / (self.run + '.jsonl')))
        except (ValueError, OSError, TypeError, KeyError):
            raise SampleUnavailable('No qualified sample; do not infer a timing') from None
        save(self.path / 'intervals.json', result)
        if result['renderer_return_interval_ns'] is None:
            if result['accepted_send_interval_ns'] is None:
                raise SampleUnavailable('No qualified Shelf frame; no elapsed time is available')
            raise SampleUnavailable('Secondary evidence only; no qualified primary renderer interval')
        announce('First Shelf-render interval available in intervals.json (not clickability or optical timing).')

    def execute(self, recover):
        descriptor = os.open(self.path, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC)
        try:
            try:
                fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError:
                raise OperatorError('Another controller is already active for this exact private batch') from None
            self.execute_exclusive(recover)
        finally:
            os.close(descriptor)

    def execute_exclusive(self, recover):
        require(not (self.path / 'released.json').exists(), 'This run is already closed')
        route_data = probe_data = None
        if recover:
            require((self.path / 'attempted').exists(), 'No attempted run to recover')
            self.lease.status(self.request)
        else:
            require(not (self.path / 'attempted').exists() and not (self.path / 'private-token').exists(), 'A new run is required')
            route_data = plain(self.path / 'route.tar.gz', 200 * 1024 * 1024).read_bytes()
            probe_data = plain(self.path / 'probe.tar.gz', 200 * 1024 * 1024).read_bytes()
            require((len(route_data), len(probe_data)) == (self.admission['route_bytes'], self.admission['probe_bytes']), 'Changed preparation')
        require(plain(self.path / 'askpass', 256).read_bytes() == ASKPASS, 'Wrong private credential prompt')
        require(sys.stdin.isatty(), 'Use a private foreground terminal')
        with warnings.catch_warnings():
            warnings.simplefilter('error', getpass.GetPassWarning)
            self.password = self.read_password()
        require(self.password, 'No authentication provided')
        failure = probe_detail = None
        try:
            if not recover:
                flag(self.path / 'attempted')
                try:
                    self.lease.acquire(self.request)
                except self.lease.LeaseError as error:
                    if error.code in ('lease_exists', 'operation_busy') and not any(error.details.values()):
                        for leaf in ('askpass', 'route.tar.gz', 'probe.tar.gz'):
                            (self.path / leaf).unlink(missing_ok=True)
                        raise OperatorError('Shared lease busy; no device contact, reservation or retry') from None
                    raise RecoveryRequired('Lease acquisition incomplete; no device contact. Inspect only this admission') from None
            announce(f'Owned batch: {self.request.owner_thread} {self.run}')
            try:
                if not recover:
                    self.measure(route_data, probe_data)
            except (Exception, KeyboardInterrupt) as error:
                failure = {'phase': self.phase, 'reason': public_reason(error)}
                probe_detail = self.probe_failure()
            finally:
                try:
                    state = self.cleanup()
                    self.step('exact-release')
                    released = self.lease.release(self.request, native_restored_and_connections_closed=True)
                except (Exception, KeyboardInterrupt) as error:
                    raise RecoveryRequired(f'Exact release not confirmed at {self.phase}: {public_reason(error)}; '
                                           'use recover with the original batch only') from None
                receipt = {key: released[key] for key in ('state', 'owner_thread', 'run_id', 'owner_and_token_checked',
                                                          'native_restored_and_connections_closed')}
                receipt['device_disposition'] = state
                announce(f'{state}; exact lease released.')
                try:
                    save(self.path / 'released.json', receipt)
                finally:
                    for leaf in ('private-token', 'askpass', 'navigation.json', 'route.tar.gz', 'probe.tar.gz'):
                        (self.path / leaf).unlink(missing_ok=True)
        finally:
            self.password = ''
        if failure is not None:
            save(self.path / 'failure.json', failure)
            if probe_detail is not None:
                try:
                    save(self.path / 'probe-failure.json', probe_detail)
                except OSError:
                    announce('Optional probe failure detail unavailable.')
            raise SampleUnavailable(f"Stopped at {failure['phase']}: {failure['reason']}; exact lease released")
        if recover:
            raise SampleUnavailable('Original batch recovered and released; no automatic timing claim. '
                                    'For an independently witnessed natural exit and complete evidence, '
                                    'see the offline log_bounds.py replay in OPERATOR.md.')
        self.analyze()


def main():
    os.umask(0o077)
    parser = argparse.ArgumentParser(description=__doc__)
    command = parser.add_subparsers(dest='operation', required=True)
    preparation = command.add_parser('prepare')
    for name in ('kit','native-kit','rollout-root'):
        preparation.add_argument('--' + name, type=Path, required=True)
    for name in ('start', 'recover'):
        command.add_parser(name).add_argument('--batch', type=Path, required=True)
    arguments = parser.parse_args()
    if arguments.operation == 'prepare':
        prepare(arguments.kit, arguments.native_kit, arguments.rollout_root)
        return
    def interrupted(*unused):
        raise InterruptedError('Operation interrupted')
    signal.signal(signal.SIGTERM, interrupted)
    signal.signal(signal.SIGHUP, interrupted)
    Operator(arguments.batch).execute(arguments.operation == 'recover')


if __name__ == '__main__':
    try:
        main()
    except RecoveryRequired as error:
        print(error, file=sys.stderr)
        sys.exit(3)
    except SampleUnavailable as error:
        print(error, file=sys.stderr)
        sys.exit(4)
    except OperatorError as error:
        print(error, file=sys.stderr)
        sys.exit(2)
    except (Exception, KeyboardInterrupt):
        sys.exit('Host operation did not complete; raw output withheld. Check the original private batch before proceeding.')
