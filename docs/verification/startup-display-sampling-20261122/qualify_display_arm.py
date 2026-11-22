"""Exercise the guarded display helper on two offline ARM profiles without a Kindle framebuffer."""

import argparse
import json
import os
from pathlib import Path
import stat
import struct
import subprocess
import uuid


def require(condition, reason):
    if not condition:
        raise ValueError(reason)


def static_arm(path):
    info = path.lstat()
    require(stat.S_ISREG(info.st_mode) and info.st_nlink == 1 and path.resolve(strict=True) == path,
            'Expected separate regular ARM executable')
    with path.open('rb') as data:
        header = data.read(52)
        require(len(header) == 52 and header[:7] == b'\x7fELF\x01\x01\x01'
                and struct.unpack_from('<H', header, 16)[0] in (2, 3)
                and struct.unpack_from('<H', header, 18)[0] == 40
                and struct.unpack_from('<I', header, 36)[0] & 0x400,
                'Not a little-endian ARM hard-float ELF')
        offset, width, count = struct.unpack_from('<I', header, 28)[0], *struct.unpack_from('<HH', header, 42)
        require(52 <= offset and width == 32 and 0 < count <= 64 and offset + count * width <= info.st_size,
                'Incorrect program headers')
        data.seek(offset)
        entries = [struct.unpack_from('<I', data.read(width))[0] for _ in range(count)]
        require(1 in entries and 3 not in entries, 'ARM executable has no loadable program or requires an interpreter')
    return info.st_size


def trial(command, env):
    return subprocess.run(command, stdin=subprocess.DEVNULL, capture_output=True, env=env,
                          check=False, timeout=15)


def missing_guard(profile, binary, qemu, run):
    sys_home = Path('/mnt/us') / run
    (sys_home / 'data').mkdir(parents=True, mode=0o700)
    trace = sys_home / 'data' / (run + '.jsonl')
    sidecar = trace.with_name(run + '.display-cost.json')
    env = {'HOME': str(sys_home), 'PATH': os.defpath, 'LC_ALL': 'C', 'BILICOMICS_HOME': str(sys_home),
           'BILICOMICS_DISPLAY_COST_RUN': run, 'BILICOMICS_APPROVED_PROFILE': profile,
           'BILICOMICS_APPROVED_FIRMWARE': '5.19.5'}
    helper = trial([qemu, binary, '--capture-native-display', '--kindle', '--device', profile], env)
    require(helper.returncode == 1 and helper.stdout == b'' and b'Invalid native-read context' in helper.stderr
            and b'Missing native-read directory' in helper.stderr and not sidecar.exists(),
            'Helper did not refuse the missing native read permit')
    app = trial([qemu, binary, '--kindle', '--device', profile, '--data-dir', str(sys_home / 'data'), '--demo', '--offline',
                 '--shelf-render-trace', str(trace), '--exit-after-ms', '2500'], env)
    require(app.returncode == 1 and b'Native output requires an owned launcher mode snapshot' in app.stderr,
            'Normal application did not refuse absent native launcher evidence')
    rows = [json.loads(line) for line in trace.read_bytes().splitlines()]
    require([row['stage'] for row in rows] == ['trace_started', 'trace_stopped'] and rows[0]['data']['run_id'] == run
            and rows[0]['process_id'] == rows[1]['process_id'] and not sidecar.exists()
            and sorted(path.name for path in (sys_home / 'data').iterdir()) == [trace.name],
            'Unexpected diagnostic on an ineligible application')
    return {'profile': profile, 'native_exit_code': app.returncode, 'missing_owned_snapshot_rejected': True,
            'helper_exit_code': helper.returncode, 'helper_missing_permission_rejected': True,
            'helper_cost_sidecar_created': False, 'native_framebuffer_available': False,
            'native_input_available': False, 'shelf_rendered': False, 'shelf_trace_started': True,
            'trace_record_count': len(rows)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('binary', 'worker', 'build', 'output'):
        parser.add_argument('--' + name, type=Path, required=True)
    args = parser.parse_args()
    os.umask(0o077)
    require(not args.output.exists() and not Path('/dev/fb0').exists() and not Path('/dev/input').exists(),
            'Use a fresh output in the off-device Ubuntu environment')
    interfaces = {line.split(':', 1)[0].strip() for line in Path('/proc/net/dev').read_text().splitlines()[2:]}
    require(interfaces == {'lo'}, 'Use an isolated network namespace')
    require(not Path('/mnt/us').exists(), 'Refusing any existing /mnt/us path')
    size = static_arm(args.binary)
    worker_size = static_arm(args.worker)
    build = json.loads(args.build.read_text())
    require(build['host_integration_passed'] is True and build['overlay_only'] is True
            and build['worker']['bytes'] == worker_size and build['worker']['unchanged_sources'] is True
            and build['worker']['identical_to_install_zip'] is True, 'Missing exact source and worker provenance')
    qemu = '/usr/bin/qemu-arm'
    env = {'PATH': os.defpath, 'LC_ALL': 'C'}
    version = trial([qemu, args.binary, '--version'], env)
    require(version.returncode == 0 and version.stdout.strip() == b'bilicomics 0.1.25', 'Incorrect application version')
    emulator = trial([qemu, '--version'], env)
    require(emulator.returncode == 0, 'Missing QEMU user emulator')
    mounted = False
    try:
        subprocess.run(['mount', '-t', 'tmpfs', '-o', 'mode=755,size=4M,nodev,nosuid', 'tmpfs', '/mnt'], check=True)
        mounted = True
        Path('/mnt/us').mkdir(mode=0o700)
        profiles = [missing_guard(profile, args.binary, qemu, 'bili-shelf-' + uuid.uuid4().hex)
                    for profile in ('scribe', 'pw6')]
    finally:
        if mounted:
            subprocess.run(['umount', '/mnt'], check=True)
    proof = {'schema': 'bili-display-helper-arm-qualification-v1', 'passed': True, 'physical_device_accessed': False,
             'device_lease_accessed': False, 'cargo_bin': 'bilicomics', 'cross_target': 'armv7-unknown-linux-musleabihf',
             'static_elf32_arm_hard_float': True, 'application_bytes': size, 'host_integration_passed': True,
             'overlay_only': True, 'version_exit_code': version.returncode, 'cli_version': version.stdout.decode().strip(),
             'qemu': emulator.stdout.decode().splitlines()[0], 'loopback_only': True,
             'worker': build['worker'], 'profiles': profiles}
    with args.output.open('x') as out:
        json.dump(proof, out, indent=2)
        out.write('\n')


if __name__ == '__main__':
    main()
