"""Exercise the poll-instrumented ARM product with real QEMU and no device endpoints."""

import argparse
import importlib.util
import json
import os
from pathlib import Path
import stat
import subprocess
import uuid


PREVIOUS = Path(__file__).resolve().parent.parent / 'startup-display-sampling-20261122/qualify_display_arm.py'
SPEC = importlib.util.spec_from_file_location('static_arm_checker', PREVIOUS)
CHECK = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CHECK)
require = CHECK.require


def read(path):
    info = path.lstat()
    require(stat.S_ISREG(info.st_mode) and info.st_uid == 0 and stat.S_IMODE(info.st_mode) == 0o600 and
            info.st_nlink == 1 and path.resolve(strict=True) == path and info.st_size <= 8192, 'Invalid qualification ticket')
    return json.loads(path.read_bytes())


def guard(profile, app):
    run = 'bili-shelf-' + uuid.uuid4().hex
    home = Path('/mnt/us') / run
    data = home / 'data'
    data.mkdir(mode=0o700, parents=True)
    trace = data / (run + '.jsonl')
    env = {'HOME': str(home), 'PATH': os.defpath, 'LC_ALL': 'C', 'BILICOMICS_HOME': str(home),
           'BILICOMICS_DISPLAY_POLL_RUN': run, 'BILICOMICS_APPROVED_PROFILE': profile,
           'BILICOMICS_APPROVED_FIRMWARE': '5.19.5'}
    command = ['/usr/bin/qemu-arm', app]
    helper = CHECK.trial([*command, '--capture-native-display', '--kindle', '--device', profile], env)
    require(helper.returncode == 1 and helper.stdout == b'' and b'Invalid native-read context' in helper.stderr
            and b'Missing native-read directory' in helper.stderr and not list(data.iterdir()),
            'Native helper did not refuse missing ownership without writing a sidecar')
    normal = CHECK.trial([*command, '--kindle', '--device', profile, '--data-dir', data, '--demo', '--offline',
                          '--shelf-render-trace', trace, '--exit-after-ms', '2500'], env)
    require(normal.returncode == 1 and b'Native output requires an owned launcher mode snapshot' in normal.stderr,
            'Normal application did not refuse missing launcher evidence')
    rows = [json.loads(line) for line in trace.read_bytes().splitlines()]
    require([row['stage'] for row in rows] == ['trace_started', 'trace_stopped'] and rows[0]['data']['run_id'] == run
            and rows[0]['process_id'] == rows[1]['process_id'] and list(data.iterdir()) == [trace],
            'Unexpected stage, sidecar or other file after refusal')
    return {'profile': profile, 'native_exit_code': normal.returncode, 'helper_exit_code': helper.returncode,
            'missing_owned_snapshot_rejected': True, 'helper_missing_permission_rejected': True,
            'helper_cost_sidecar_created': False, 'native_framebuffer_available': False, 'native_input_available': False,
            'shelf_rendered': False, 'shelf_trace_started': True, 'trace_record_count': len(rows)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', type=Path, required=True)
    args = parser.parse_args()
    os.umask(0o077)
    root = args.root
    require(root == Path('/work') and not Path('/dev/fb0').exists() and not Path('/dev/input').exists()
            and not Path('/mnt/us').exists(), 'Expected the private device-free Ubuntu namespace')
    interfaces = {line.split(':', 1)[0].strip() for line in Path('/proc/net/dev').read_text().splitlines()[2:]}
    require(interfaces == {'lo'}, 'Loopback-only network required')
    q = root / 'qualification'
    output = q / 'arm.json'
    require(not os.path.lexists(output), 'Refusing to overwrite a qualification')
    source, arm, host, product = (read(q / leaf) for leaf in ('source.json', 'arm-build.json', 'host.json', 'host-product.json'))
    binary = root / 'artifacts/bilicomics'
    size = CHECK.static_arm(binary)
    worker_size = CHECK.static_arm(root / 'artifacts/kindle-jpeg-worker')
    require(source['schema'] == 'bili-display-poll-source-v1' and source['selector'] == 'BILICOMICS_DISPLAY_POLL_RUN'
            and source['receipt_schema'] == 'kindle-startup-display-poll-v1' and source['receipt_suffix'] == '.display-poll.json'
            and source['old_overlay_reproduced'] is True and source['overlay_only'] is True
            and source['stock_guard_sources_compared'] is True and source['worker'] == {'bytes': worker_size,
                'unchanged_sources': True, 'identical_to_install_zip': True}, 'Wrong overlay or worker')
    for variant, build in (('arm', arm), ('host', host)):
        require(build['schema'] == 'bili-display-poll-' + variant + '-build-v1' and build['source_schema'] == source['schema']
                and build['passed'] is True and build['exit_status'] == 0 and build['retained_tmpfs_product_full_byte_match'] is True
                and build['network_isolated'] is True and build['shared_caches_read_only'] is True, 'Unqualified build')
    require(arm['compiled_bytes'] == size and host['compiled_bytes'] == product['host_elf_bytes']
            and product['schema'] == 'bili-display-poll-host-product-qualification-v1'
            and all(product[key] is True for key in ('passed', 'complete_product_elf', 'unchanged_permit',
                                                    'only_hardware_inputs_synthetic', 'collector_validated'))
            and product['physical_device_accessed'] is False and set(product['profiles']) == {'scribe', 'pw6'}
            and all(row[key] is True for row in product['profiles'].values()
                    for key in ('passed', 'poll_split_counts_sum', 'poll_split_elapsed_sum')), 'Missing full host product gates')
    env = {'LC_ALL': 'C', 'PATH': os.defpath}
    qemu = CHECK.trial(['/usr/bin/qemu-arm', '--version'], env)
    version = CHECK.trial(['/usr/bin/qemu-arm', binary, '--version'], env)
    require(qemu.returncode == 0 and version.returncode == 0 and version.stdout.strip() == b'bilicomics 0.1.25',
            'Unexpected emulator or application identity')
    subprocess.run(['mount', '-t', 'tmpfs', '-o', 'mode=755,size=4M,nodev,nosuid', 'tmpfs', '/mnt'], check=True)
    try:
        Path('/mnt/us').mkdir(mode=0o700)
        profiles = [guard(profile, binary) for profile in ('scribe', 'pw6')]
    finally:
        subprocess.run(['umount', '/mnt'], check=True)
    result = {'schema': 'bili-display-poll-arm-qualification-v1', 'passed': True, 'poll_branch_instrumentation': True,
              'physical_device_accessed': False, 'device_lease_accessed': False, 'cargo_bin': 'bilicomics',
              'cross_target': 'armv7-unknown-linux-musleabihf', 'static_elf32_arm_hard_float': True,
              'application_bytes': size, 'overlay_only': True, 'host_integration_passed': True, 'version_exit_code': 0,
              'cli_version': version.stdout.decode().strip(), 'qemu': qemu.stdout.decode().splitlines()[0],
              'loopback_only': True, 'worker': source['worker'], 'profiles': profiles}
    with output.open('x') as out:
        json.dump(result, out, indent=2)
        out.write('\n')


if __name__ == '__main__':
    main()
