"""Exercise the actual product poll partitions and stock shell on isolated synthetic hardware."""

import ctypes
import json
import os
from pathlib import Path
import socket
import stat
import subprocess
import sys
import time
import uuid


ROOT = Path('/work')
TOOLS = ROOT / 'inputs'
STUDIES = TOOLS / 'studies'
sys.path[:0] = [str(STUDIES / 'startup-display-poll-20261122'), str(STUDIES / 'startup-display-sampling-20261122')]
import collect_display as collector
import host_bench


CASES = (('scribe', 'selected'), ('pw6', 'selected'), ('scribe', 'selector-off'), ('scribe', 'revoke'), ('pw6', 'timeout'))
CHILDREN = ('command_poll_stdout_open', 'command_poll_after_eof')


def require(value, reason):
    if not value:
        raise ValueError(reason)


def verify_record(run, home, observations, before, after):
    data = home / 'data' / collector.name_for(run)
    require(list(data.parent.iterdir()) == [data] and stat.S_IMODE(data.lstat().st_mode) == 0o600, 'Not one exclusive poll sidecar')
    boot = Path('/proc/sys/kernel/random/boot_id').read_text().strip()
    transfer = subprocess.run(['/bin/sh', '-c', collector.capture_command(run, str(home), boot)], capture_output=True, timeout=4)
    require(transfer.returncode == 0 and not transfer.stderr and transfer.stdout == data.read_bytes(), 'Collector did not read the original product bytes')
    record = collector.decode(transfer.stdout, run, boot, os.getpid())
    require(record['schema'] == 'kindle-startup-display-poll-v1' and record['native_read_sequence'] == 1, 'Wrong study or transaction')
    identities = (('helper', 'helper'), ('leader', 'native_read'), ('master', 'supervisor'))
    for observed in observations:
        for role, key in identities:
            require((record[key+'_pid'], record[key+'_birth']) == (observed[role]['pid'], observed[role]['birth']), 'Private record does not match live getter ancestry')
    summary = collector.components(record, before, after)
    buckets = record['buckets']
    for field in ('count', 'elapsed_ns'):
        require(all(buckets[key][field] > 0 for key in CHILDREN), 'A controlled pipe state was not observed')
        require(sum(buckets[key][field] for key in CHILDREN) == buckets['command_poll'][field], 'Poll states do not exactly partition the observed parent')
        for key in (*CHILDREN, 'command_poll'):
            label = 'parent' if key == 'command_poll' else key.removeprefix('command_poll_')
            require(summary['poll_partition'][label][field] == buckets[key][field], 'Collector changed a measured quantity')
    return summary


def trial(profile, mode, case, script, size):
    run = 'bili-shelf-' + uuid.uuid4().hex
    home = Path('/mnt/us') / run
    (home/'app').mkdir(mode=0o700, parents=True)
    (home/'data').mkdir(mode=0o700)
    (home/'run-id').write_text(run+'\n')
    case.mkdir(mode=0o700)
    audit = case / 'fb-audit'
    audit.touch(exist_ok=False)
    inventory = Path('/run/display-product/input-inventory')
    inventory.write_text('B: ABS=1000000\n' if profile == 'scribe' else 'B: ABS=1000003\n')
    require(inventory.read_bytes() == Path('/proc/bus/input/devices').read_bytes(), 'Missing private inventory')
    executable = home / 'app/bilicomics'
    template = Path('/mnt/us/fixture-binary')
    template.rename(executable)
    require(executable.stat().st_size == size and executable.stat().st_nlink == 1, 'Wrong product executable')
    env = {'PATH': str(TOOLS)+':'+os.defpath, 'LC_ALL': 'C', 'PROFILE': profile, 'BILICOMICS_HOME': str(home),
           'BILICOMICS_APPROVED_PROFILE': profile, 'BILICOMICS_APPROVED_FIRMWARE': 'fixture', 'FIXTURE_TOOLS': str(home/'app'),
           'FIXTURE_PROFILED': '0', 'DISPLAY_FIXTURE_MODE': mode, 'DISPLAY_FIXTURE_CASE': str(case),
           'DISPLAY_FIXTURE_FB_AUDIT': str(audit), 'LD_PRELOAD': str(TOOLS/'fb-read.so')}
    if mode != 'selector-off':
        env['BILICOMICS_DISPLAY_POLL_RUN'] = run
    shell = subprocess.Popen(['/bin/sh', script], cwd=case, env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, start_new_session=True)
    try:
        require(host_bench.line(shell.stdout.fileno()) == 'armed', 'Stock supervisor did not arm')
        before = time.monotonic_ns()
        shell.stdin.write(b'read\n')
        shell.stdin.flush()
        if mode == 'revoke':
            gate, permission = case/'getter-started', case/'lock/native-read-go'
            limit = time.monotonic()+1.5
            while not gate.exists() and time.monotonic() < limit:
                time.sleep(.005)
            require(gate.is_file() and permission.is_file(), 'Cannot witness a live getter before withdrawal')
            permission.unlink()
        stdout, stderr = shell.communicate(timeout=8)
        after = time.monotonic_ns()
        require(shell.returncode == 0 and not stderr, 'Stock supervisor failed')
        host_bench.inspect_group()
        require(not (case/'lock/native-read-go').exists() and (case/'getters').is_file(), 'Capture or cleanup missing')
        observations = [json.loads(line) for line in (case/'getters').read_text().splitlines()]
        names = [row['name'] for row in observations]
        require(all(row['master']['pid'] == shell.pid for row in observations), 'Unexpected live supervisor')
        if mode in ('revoke', 'timeout'):
            require(stdout == b'result refused 1\n' and not list((home/'data').iterdir()) and audit.read_bytes() == b'', 'Refused capture published output')
            require(names == ['power'] and (case/'getter-started').is_file() and not (case/'delayed-getter-finished').exists(), 'Unexpected getter retirement')
            require(after-before < 1_700_000_000 if mode == 'revoke' else after-before >= 1_800_000_000, 'Unexpected original timeout/revocation control')
            return {'passed': True, 'stock_refused': True, 'no_sidecar': True, 'only_first_getter_started': True, 'owned_group_reaped': True}
        expected = ['power', 'mode', 'holding', 'holding', 'power'] if profile == 'scribe' else ['power', 'mode', 'power']
        accepted = b'result ready:Y8:U:0 1\n' if profile == 'scribe' else b'result ready:Y8:FIXED:0 1\n'
        require(stdout == accepted and names == expected and audit.read_bytes() == b'VVFVVF', 'Incomplete stock hardware capture')
        require((case/'mode-getter-finished').is_file(), 'Controlled getter did not finish after EOF')
        if mode == 'selector-off':
            require(not list((home/'data').iterdir()), 'Unselected ordinary capture emitted a diagnostic')
            return {'passed': True, 'stock_capture_succeeded': True, 'no_sidecar': True, 'owned_group_reaped': True}
        result = verify_record(run, home, observations, before, after)
        require(result['query_branch'] == ('automatic-five' if profile == 'scribe' else 'fixed-three') and result['process_stat_reads'] > 0, 'Wrong product branch')
        return {'passed': True, 'natural_branch': result['query_branch'], 'queries': len(expected), 'metadata_passes': 2,
                'live_getters_witnessed_all_three_process_births': True, 'owned_group_reaped': True,
                'controlled_getter_exercised_open_and_post_eof': True, 'both_poll_states_observed': True,
                'poll_split_counts_sum': True, 'poll_split_elapsed_sum': True}
    finally:
        if shell.poll() is None:
            shell.terminate()
            shell.wait(timeout=5)
        executable.rename(template)


def main():
    os.umask(0o077)
    require(len(sys.argv) == 3 and sys.argv[2] in ('r1', 'r2', 'r3') and Path.cwd() == ROOT and os.geteuid() == 0
            and socket.if_nameindex() == [(1, 'lo')], 'Unexpected test namespace')
    require(collector.WIRE_SCHEMA == 'kindle-startup-display-poll-v1' and collector.CHILDREN == CHILDREN, 'Wrong poll collector')
    require(ctypes.CDLL(None).prctl(36, 1, 0, 0, 0) == 0, 'Could not independently observe orphan retirement')
    output = ROOT / 'reports' / sys.argv[2]
    output.mkdir(mode=0o700)
    script = output / 'stock-capture.sh'
    host_bench.make_shell(TOOLS/'stock-launch.sh', script)
    for usec in (10_000, 20_000):
        before = time.monotonic_ns()
        subprocess.run([TOOLS/'usleep', str(usec)], check=True)
        require(time.monotonic_ns() - before >= usec*1_000, 'Shell clock delay not preserved')
    size = int(sys.argv[1])
    profiles, controls = {}, {}
    for profile, mode in CASES:
        case = profile+'-'+mode
        try:
            answer = trial(profile, mode, output/case, script, size)
        except Exception as error:
            (output/'failure.txt').write_text(case+': '+type(error).__name__+'\n')
            raise
        if mode == 'selected':
            profiles[profile] = answer
        else:
            controls[case] = answer
    report = {'schema': 'bili-display-poll-host-product-qualification-v1', 'passed': True, 'complete_product_elf': True,
              'unchanged_permit': True, 'only_hardware_inputs_synthetic': True, 'physical_device_accessed': False,
              'collector_validated': True, 'host_elf_bytes': size, 'profiles': profiles, 'controls': controls,
              'scope': 'Full Linux x86-64 Kindle-feature capture CLI under original stock permission and process controls. Both real poll states appear in each positive record, and their counts and nanoseconds independently partition the same parent exactly. The synthetic mode getter delays before output and after closing stdout. These are functional host checks, not measurements of firmware, input, Shelf presentation or a physical visit.'}
    with (output/'host-product-qualification.json').open('x') as file:
        file.write(json.dumps(report, indent=2)+'\n')
    print('Both product poll partitions and all three controls passed.')


if __name__ == '__main__':
    main()
