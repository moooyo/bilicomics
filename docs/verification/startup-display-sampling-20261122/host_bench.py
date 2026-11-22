"""Rehearse one stock display transaction in an isolated Linux userspace; never open a device."""

import argparse
import json
import os
from pathlib import Path
import select
import shutil
import socket
import statistics
import subprocess
import tempfile
import time
import tomllib


HERE = Path(__file__).resolve().parent
AUTO = ('scribe', 'ready:Y8:U:0', 5)
FIXED = ('pw6', 'ready:Y8:FIXED:0', 3)
SHELL_MARKS = ('begin', 'native_read_go', 'native_read_ready', 'native_read_reaped', 'parsed')
SEQUENCE = ('AB', 'BA', 'AB', 'BA', 'AB', 'BA')


def require(condition, message):
    if not condition:
        raise ValueError(message)


def function(text, name):
    prefix = name + '() {\n'
    require(text.count('\n' + prefix) == 1, 'Unexpected launcher function: ' + name)
    return prefix + text.partition('\n' + prefix)[2].partition('\n}\n')[0] + '\n}\n'


def make_shell(stock, path):
    text = stock.read_text()
    names = ('read_process', 'native_read_is_owned', 'native_read_group_is_owned', 'stop_native_read',
             'native_read_command', 'native_read_hex', 'capture_native_epdc_mode')
    shell = '#!/bin/sh\nset -eu\numask 077\n' + '\n'.join(function(text, name) for name in names)
    shell += '''
LOCK_DIR=$PWD/lock
APP_DIR=$FIXTURE_TOOLS
APP_GENERATION=1
LAUNCH_TOKEN=0123456789abcdef0123456789abcdef
NATIVE_READ_PID=''; NATIVE_READ_BIRTH=''; NATIVE_READ_SEQUENCE=0; NATIVE_READ_REAPED=1
mkdir "$LOCK_DIR"
printf '%s\\n' "$LAUNCH_TOKEN" > "$LOCK_DIR/nonce"
printf '1:%s\\n' "$LAUNCH_TOKEN" > "$LOCK_DIR/generation"
printf '%s\\n' "$$" > "$LOCK_DIR/pid"
launch_trace() { :; }
if [ "$FIXTURE_PROFILED" = 1 ]; then launch_trace() { printf 'mark %s\\n' "$1"; }; fi
printf 'armed\\n'
IFS= read -r request
if [ "$request" = empty ]; then printf 'empty\\n'; exit 0; fi
[ "$request" = read ] || exit 2
launch_trace begin
if capture_native_epdc_mode; then
    case "$NATIVE_CAPTURE_STATUS" in
        known) answer=ready:$NATIVE_EPDC_MODE:$NATIVE_PORTRAIT_ORIENTATION:$NATIVE_PORTRAIT_ROTATION;;
        not-active) answer=not-active;;
        *) exit 3;;
    esac
else answer=refused; fi
launch_trace parsed
printf 'result %s %s\\n' "$answer" "$NATIVE_READ_REAPED"
'''
    path.write_text(shell)


def instrument(path):
    text = path.read_text()
    for target, addition in (
        ('pub(crate) fn acquire() -> Result<Self> {', 'let _observer = crate::probe::span("acquire");'),
        ('pub(crate) fn recheck(&self) -> Result<()> {', 'let _observer = crate::probe::span("recheck");'),
        ('fn process(&self, pid: u32) -> Result<ProcessIdentity> {', 'crate::probe::count("process");'),
    ):
        require(text.count(target) == 1, 'Unexpected permit source instrumentation boundary')
        text = text.replace(target, target + '\n        #[cfg(feature = "profile")]\n        ' + addition)
    path.write_text(text)


def instrument_commands(path):
    original = path.read_text()
    before = '            ensure_deadline(deadline)?;\n            check_context()?;\n            ensure_deadline(deadline)?;'
    after = '            let _command = crate::probe::span("command_inclusive");\n' + before
    child = 'let child = Command::new(program)'
    child_end = '.stderr(Stdio::null())\n                .spawn()?;'
    permit = 'self.commands.run(&self.lipc, arguments, deadline, || {\n'
    for snippet in (before, child, child_end, permit):
        require(original.count(snippet) == 1, 'Unknown nested capture diagnostic site')
    text = original.replace(before, after).replace(child, 'let child = { let _spawn = crate::probe::span("command_spawn"); Command::new(program)')
    text = text.replace(child_end, child_end[:-1] + ' };').replace(permit, permit + '                let _guard = crate::probe::span("command_recheck");\n')
    path.write_text(text)


def build(source, work, command_study=False):
    crate = work / 'crate'
    directory = crate / 'src'
    (directory / 'platform').mkdir(parents=True)
    (directory / 'native_presentation').mkdir()
    for leaf in ('platform/native_capture.rs', 'native_presentation.rs', 'native_presentation/portrait_writer.rs'):
        shutil.copyfile(source / 'src' / leaf, directory / leaf)
    shutil.copyfile(HERE / 'host_fixture.rs', directory / 'main.rs')
    instrument(directory / 'native_presentation/portrait_writer.rs')
    if command_study:
        instrument_commands(directory / 'platform/native_capture.rs')
    approval = (source / 'src/platform/mod.rs').read_text().split('fn validate_native_launch_approval(', 1)[1].split('\n}\n', 1)[0]
    (directory / 'launch_approval.rs').write_text('fn validate_native_launch_approval(' + approval + '\n}\n')
    lock = tomllib.loads((source / 'Cargo.lock').read_text())
    deps = {key: [row['version'] for row in lock['package'] if row['name'] == key] for key in ('anyhow', 'libc')}
    require(all(len(value) == 1 for value in deps.values()), 'Ambiguous locked dependencies')
    (crate / 'Cargo.toml').write_text('[package]\nname="display-fixture"\nversion="0.0.0"\nedition="2024"\n'
                                     '[features]\ndefault=["kindle"]\nkindle=[]\nprofile=[]\n[dependencies]\n'
                                     + ''.join(f'{key}="={value[0]}"\n' for key, value in deps.items()))
    target = work / 'target'
    env = {**os.environ, 'CARGO_BUILD_JOBS': '1', 'CARGO_TARGET_DIR': str(target), 'CARGO_INCREMENTAL': '0'}
    builds = [('profiled', ['--features', 'profile'])] if command_study else [('plain', []), ('profiled', ['--features', 'profile'])]
    for name, features in builds:
        subprocess.run(['cargo', 'build', '--offline', '-j1', '--release', *features], env=env, cwd=crate, check=True, timeout=180)
        tool = work / name
        tool.mkdir(mode=0o700)
        shutil.copy2(target / 'release/display-fixture', tool / 'bilicomics')
        for alias in ('lipc-get-prop', 'usleep'):
            os.link(tool / 'bilicomics', tool / alias)


def line(fd):
    result = bytearray()
    while len(result) < 128 and select.select([fd], [], [], 6)[0]:
        char = os.read(fd, 1)
        if char == b'\n':
            return result.decode('ascii')
        if not char:
            break
        result += char
    raise ValueError('Missing complete bounded shell receipt')


def inspect_group():
    children = Path('/proc/self/task') / str(os.getpid()) / 'children'
    deadline = time.monotonic() + 2
    while time.monotonic() < deadline:
        while True:
            try:
                reaped, _ = os.waitpid(-1, os.WNOHANG)
            except ChildProcessError:
                return
            if not reaped:
                break
        if not children.read_text().strip():
            return
        time.sleep(.02)
    raise ValueError('A fixture descendant survived the stock group retirement')


def trial(work, profile, mode='good', profiled=False, empty=False):
    label, expected, _ = profile
    with tempfile.TemporaryDirectory(dir=work) as name:
        case = Path(name)
        tool = work / ('profiled' if profiled else 'plain')
        env = {'PATH': str(tool) + ':' + os.defpath, 'LC_ALL': 'C', 'PROFILE': label, 'BILICOMICS_APPROVED_PROFILE': label,
               'BILICOMICS_APPROVED_FIRMWARE': 'fixture', 'FIXTURE_GETTER': str(tool / 'lipc-get-prop'), 'FIXTURE_MODE': mode,
               'FIXTURE_TOOLS': str(tool), 'FIXTURE_PROFILED': '1' if profiled else '0'}
        with subprocess.Popen(['/bin/sh', work / 'capture.sh'], cwd=case, env=env, stdin=subprocess.PIPE, start_new_session=True,
                              stdout=subprocess.PIPE, stderr=subprocess.DEVNULL) as child:
            try:
                fd = child.stdout.fileno()
                require(line(fd) == 'armed', 'Shell did not reach the fixture boundary')
                start = time.clock_gettime_ns(time.CLOCK_MONOTONIC)
                os.write(child.stdin.fileno(), b'empty\n' if empty else b'read\n')
                marks = {}
                while True:
                    received = line(fd)
                    now = time.clock_gettime_ns(time.CLOCK_MONOTONIC)
                    if not received.startswith('mark '):
                        break
                    key = received[5:]
                    require(key not in marks and key in SHELL_MARKS, 'Unexpected or repeated shell clock')
                    marks[key] = now
                duration = now - start
                expected = 'empty' if empty else 'result ' + (expected if mode == 'good' else
                           'not-active' if mode.startswith('inactive-') else 'refused') + ' 1'
                require(received == expected, f'{label}/{mode}: expected {expected}, got {received}')
                if mode == 'timeout':
                    require(1_800_000_000 <= duration < 4_000_000_000, 'Deadline was not the observed refusal')
                if mode == 'revoke':
                    require(70_000_000 <= duration < 1_800_000_000, 'Live revocation did not precede the deadline')
                if mode in ('timeout', 'revoke'):
                    require((case / 'getter-pid').is_file(), 'Blocking getter never entered the fixture')
                require(child.wait(timeout=3) == 0, 'Shell did not exit cleanly')
            finally:
                if child.poll() is None:
                    child.kill()
                    child.wait(timeout=3)
                inspect_group()
        result = {'request_to_reply_ns': duration}
        if profiled and mode == 'good':
            require(tuple(marks) == SHELL_MARKS, 'Observed an incomplete shell transaction')
            intervals = {a + '_to_' + b: marks[b] - marks[a] for a, b in zip(SHELL_MARKS, SHELL_MARKS[1:])}
            rows = [row.split() for row in (case / 'capture.tsv').read_text().splitlines()]
            require(len({row[0] for row in rows}) == len(rows), 'Repeated diagnostic totals')
            counts = {key: (int(first), int(last)) for key, first, last in rows}
            helper_start, helper_end = counts.pop('helper')
            children = [tuple(map(int, row.split())) for row in (case / 'getter.tsv').read_text().splitlines()]
            require(len(children) == profile[2] and counts['metadata'][0] == 2, 'Wrong fixture hardware or property counts')
            require(all(helper_start <= a <= b <= helper_end for a, b in children) and
                    all(left[1] <= right[0] for left, right in zip(children, children[1:])), 'Getter spans mixed launches')
            result.update(shell_receive_intervals_ns=intervals, helper_elapsed_ns=helper_end-helper_start,
                          shell_go_receipt_to_helper_entry_ns=helper_start-marks['native_read_go'],
                          helper_return_to_shell_ready_receipt_ns=marks['native_read_ready']-helper_end,
                          getter_entries=len(children), getter_body_ns=sum(b-a for a,b in children),
                          acquire_count=counts['acquire'][0], acquire_ns=counts['acquire'][1],
                          rechecks=counts['recheck'][0], recheck_ns=counts['recheck'][1],
                          process_identity_reads=counts['process'][0], metadata_samples=counts['metadata'][0])
            if 'command_inclusive' in counts:
                calls, command_ns = counts['command_inclusive']
                require(calls == counts['command_spawn'][0] == len(children), 'Missing actual command span')
                spawn_ns, guard_ns = counts['command_spawn'][1], counts['command_recheck'][1]
                require(0 < guard_ns <= counts['recheck'][1] + 1000000 and spawn_ns + guard_ns <= command_ns,
                        'Invalid nested command spans')
                result.update(command_calls=calls, command_inclusive_ns=command_ns, command_spawn_ns=spawn_ns,
                              command_guard_count=counts['command_recheck'][0], command_guard_ns=guard_ns,
                              command_remainder_ns=command_ns - spawn_ns - guard_ns)
        return result


def main():
    import ctypes
    cli = argparse.ArgumentParser(description=__doc__)
    for flag in ('source', 'stock', 'output'):
        cli.add_argument('--' + flag, type=Path, required=True)
    cli.add_argument('--command-spans', action='store_true', help='Run only the six separate nested command diagnostics')
    args = cli.parse_args()
    require(sys_platform() and {name for _,name in socket.if_nameindex()} == {'lo'} and not Path('/dev/fb0').exists()
            and not Path('/dev/input').exists(), 'Use only an isolated test-env Ubuntu fixture without native I/O')
    require(not args.output.exists() and args.stock.stat().st_size == 80185, 'Use a fresh output and the retained stock shell')
    os.umask(0o077)
    require(ctypes.CDLL(None, use_errno=True).prctl(36, 1, 0, 0, 0) == 0, 'Cannot adopt own fixture descendants')
    report = {'schema':'bili-host-display-sampling-v1','clock':'CLOCK_MONOTONIC','physical_device':False,
              'real_framebuffer':False,'real_lipc':False,'removed_authorization_checks':0,'profiles':{}}
    with tempfile.TemporaryDirectory(dir='/tmp', prefix='display-sampling-') as name:
        work = Path(name)
        build(args.source.resolve(strict=True), work, args.command_spans)
        for binary in (('profiled',) if args.command_spans else ('plain', 'profiled')):
            env = {**os.environ, 'PATH': str(work / binary) + ':' + os.defpath}
            command = subprocess.run(['/bin/sh', '-c', 'command -v usleep'], env=env, capture_output=True, text=True, check=True)
            require(command.stdout.strip() == str(work / binary / 'usleep'), 'Wrong polling utility')
            for delay in (10000, 20000):
                start = time.clock_gettime_ns(time.CLOCK_MONOTONIC)
                subprocess.run(['usleep', str(delay)], env=env, capture_output=True, check=True)
                require(time.clock_gettime_ns(time.CLOCK_MONOTONIC) - start >= delay * 1000, 'Truncated poll sleep')
        make_shell(args.stock, work/'capture.sh')
        if args.command_spans:
            report['schema'] = 'bili-host-display-command-spans-v1'
            report['profiles'] = {p[0]: [trial(work,p,profiled=True) for _ in range(3)] for p in (AUTO,FIXED)}
            with args.output.open('x') as output: json.dump(report,output,indent=2); output.write('\n')
            return
        for profile in (AUTO,FIXED):
            case = report['profiles'][profile[0]] = {}
            case['qualification'] = {mode:trial(work,profile,mode) for mode in
                ('good','inactive-first','inactive-last','bad-mode','fail-mode','timeout','revoke')}
        for profile in (AUTO,FIXED):
            case = report['profiles'][profile[0]]
            case['empty_exchange_ns'] = [trial(work,profile,empty=True)['request_to_reply_ns'] for _ in range(6)]
            case['warmups_ns'] = [trial(work,profile)['request_to_reply_ns'] for _ in range(2)]
            pairs = []
            for order in SEQUENCE:
                arms = {label:trial(work,profile)['request_to_reply_ns'] for label in order}
                pairs.append({'order':order,**arms,'B_minus_A_ns':arms['B']-arms['A']})
            case['plain_pairs_ns'] = pairs
            case['profiled_samples'] = [trial(work,profile,profiled=True) for _ in range(3)]
            case['all_plain_median_ns'] = statistics.median(row[key] for row in pairs for key in ('A','B'))
    with args.output.open('x') as output: json.dump(report,output,indent=2); output.write('\n')
    for name,case in report['profiles'].items(): print(name, 'plain median ns:',case['all_plain_median_ns'])


def sys_platform():
    return os.uname().sysname == 'Linux'


if __name__ == '__main__':
    main()
