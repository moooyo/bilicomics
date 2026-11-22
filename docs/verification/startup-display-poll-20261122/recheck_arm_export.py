"""Recheck a completed ARM export whose original source postflight found five private Zig cache leaves."""

import argparse
import json
import os
from pathlib import Path
import re
import stat
import subprocess
import sys
import tarfile
import tempfile

import build_poll_engineering as build


def verified_sources(root, previous, manifest):
    workspace = root / 'workspace'
    actual = build.roster(workspace)
    extra = set(actual) - set(manifest)
    build.require(all(actual.get(leaf) == identity for leaf, identity in manifest.items()), 'A frozen compilation input changed')
    prefix = 'target/private/zig/'
    cache = {leaf.removeprefix(prefix): (workspace / leaf).read_bytes() for leaf in extra if leaf.startswith(prefix)}
    build.require(len(extra) == len(cache) == 5 and cache.get('h/timestamp') == b'' and
                  len([name for name, value in cache.items() if re.fullmatch(r'h/[0-9a-f]{32}\.txt', name) and value == b'']) == 2
                  and all(len(value) <= 128 for value in cache.values()), 'Unexpected additional files')
    source_ref = b'  /work/target/private/cache/cargo-zigbuild/0.24.0/.intentionally-empty-file.'
    depfiles = {value for name, value in cache.items() if re.fullmatch(r'tmp/[0-9a-f]{15,16}-\.intentionally-empty-file\.o\.d', name)}
    build.require(depfiles == {b'.intentionally-empty-file.o: \\\n' + source_ref + lang + b'\n' for lang in (b'c', b'cpp')},
                  'Unexpected data in the Zig probe dependency files')
    for name in extra:
        entry = workspace / name
        info = entry.lstat()
        build.require(entry.resolve(strict=True) == entry and stat.S_ISREG(info.st_mode) and info.st_uid == 0
                      and info.st_nlink == 1 and stat.S_IMODE(info.st_mode) == 0o600, 'Invalid cache leaf')
    expected = set(manifest) - {'src/platform/capture_cost.rs'}
    seen = set()
    with tempfile.TemporaryDirectory(prefix='poll-anchor-', dir=root) as tmp:
        working = Path(tmp)
        stock = working / 'stock'
        stock.mkdir(mode=0o700)
        with tarfile.open(fileobj=sys.stdin.buffer, mode='r|') as stream:
            for member in stream:
                if member.isdir():
                    continue
                leaf = member.name
                build.require(member.isfile() and leaf in expected and leaf not in seen and 0 <= member.size < 32 << 20,
                              'Unexpected member in independent stock source stream')
                seen.add(leaf)
                raw = stream.extractfile(member).read()
                if leaf in build.OVERLAY:
                    dest = stock / leaf
                    dest.parent.mkdir(parents=True, mode=0o700, exist_ok=True)
                    dest.write_bytes(raw)
                else:
                    build.require(raw == (workspace / leaf).read_bytes(), 'Fresh stock bytes differ from the build input')
        build.require(seen == expected and len(seen) == 548, 'Missing stock source')
        for generator, reference in ((build.PREVIOUS / 'stage_overlay.py', previous / 'overlay'),
                                     (build.STUDY / 'stage_overlay.py', workspace)):
            overlay = working / generator.parent.name
            subprocess.run(['python3', '-B', generator, stock, overlay], check=True)
            build.require(set(build.roster(overlay)) == build.OVERLAY and all((overlay / leaf).read_bytes() ==
                          (reference / leaf).read_bytes() for leaf in build.OVERLAY), 'A regenerated overlay differs')
    return sorted(extra)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for key in ('root', 'previous', 'exports'):
        parser.add_argument('--' + key, type=Path, required=True)
    args = parser.parse_args()
    os.umask(0o077)
    root, previous, exports = (build.private(path) for path in (args.root, args.previous, args.exports))
    build.require(root.parent == Path('/var/tmp') and root.name.startswith('bilicomics-display-poll-build-') and
                  exports.parent == Path('/dev/shm') and exports.name == root.name + '-exports', 'Wrong build identities')
    q = build.private(root / 'qualification')
    build.require(not (q / 'arm-stop.json').exists() and not (q / 'arm-export.json').exists() and
                  not (root / 'artifacts/bilicomics').exists(), 'Not a completed but unpublished ARM source postflight')
    logpath = q / 'arm-build.log'
    binary = exports / 'arm.elf'
    for path in (binary, logpath):
        info = path.lstat()
        build.require(path.resolve(strict=True) == path and stat.S_ISREG(info.st_mode) and info.st_uid == 0 and
                      info.st_nlink == 1 and stat.S_IMODE(info.st_mode) == 0o600, 'Untrusted source log or export')
    unit = 'bilicomics-poll-arm-' + root.name.rsplit('-', 1)[1] + '.scope'
    raw = subprocess.check_output(['journalctl', '--unit', unit, '--output=json', '--no-pager'], text=True)
    rows = [json.loads(line) for line in raw.splitlines()]
    build.require(len(rows) == 3 and len({row['INVOCATION_ID'] for row in rows}) == 1 and all(row['UNIT'] == unit for row in rows),
                  'Ambiguous systemd build invocation')
    start, end, resource = rows
    begin_ns, finish_ns = (int(row['__REALTIME_TIMESTAMP']) * 1000 for row in (start, end))
    build.require(start['JOB_RESULT'] == 'done' and build.ARM in start['MESSAGE'] and
                  str(root / 'workspace') in start['MESSAGE'] and 'cmp -s' in start['MESSAGE'] and
                  'exported-checked-complete-product' in start['MESSAGE'] and end['MESSAGE'] == unit + ': Deactivated successfully.',
                  'Wrong scope, build, export check or scope result')
    birth = [int(subprocess.check_output(['stat', '-c', '%W', '--', path])) for path in (binary, logpath)]
    build.require(birth[0] == birth[1] and abs(begin_ns // 10**9 - birth[0]) <= 2 and
                  begin_ns < binary.stat().st_mtime_ns <= logpath.stat().st_mtime_ns <= finish_ns and
                  logpath.stat().st_mtime_ns - binary.stat().st_mtime_ns < 2 * 10**9, 'Export does not match the timed build log')
    lines = logpath.read_text().splitlines()
    build.require(len(lines) > 10 and lines[-1] == 'exported-checked-complete-product' and
                  lines[-3].strip().startswith('Finished `release` profile [optimized]') and
                  re.fullmatch(r'target_usage_kib=[1-9][0-9]+', lines[-2]), 'Missing original successful Cargo and full-byte export checks')
    manifest = build.data(q / 'frozen-inputs.json')
    build.require(len(manifest) == 549, 'Wrong manifest')
    unexpected = verified_sources(root, previous, manifest)
    recovered = {'schema': 'bili-display-poll-arm-postflight-recheck-v1', 'initial_source_postflight': 'refused',
                 'original_condition': 'Five private target cache leaves are not frozen application inputs.',
                 'extra_target_only': unexpected, 'frozen_inputs_unchanged': len(manifest), 'fresh_stock_files_compared': 548,
                 'unchanged_stock_files_compared_by_bytes': len(manifest) - len(build.OVERLAY), 'both_four_file_overlays_regenerated': True,
                 'original_elf_was_byte_compared_inside_build_namespace': True,
                 'systemd_cpu_usage_ns': int(resource['CPU_USAGE_NSEC']), 'systemd_memory_peak_bytes': int(resource['MEMORY_PEAK']),
                 'systemd_swap_peak_bytes': int(resource['MEMORY_SWAP_PEAK']), 'scope_completed_successfully': True,
                 'original_watchdog_extrema_persisted': False}
    proof = {'schema': 'bili-display-poll-arm-build-v1', 'mode': 'arm', 'passed': True, 'command': build.ARM,
             'compiled_bytes': binary.stat().st_size, 'exit_status': 0, 'cargo_jobs': 1, 'incremental': False,
             'single_cpu': True, 'shared_build_lock': True, 'network_isolated': True, 'shared_caches_read_only': True,
             'memory_high_bytes': build.HIGH, 'memory_max_bytes': build.MAX, 'swap_max_bytes': build.SWAP,
             'target_tmpfs_max_bytes': build.TARGET, 'retained_tmpfs_product_full_byte_match': True,
             'sampled_host_resource_extrema_kib': None, 'global_limits_kib': build.FLOORS,
             'elapsed_seconds': round((int(end['__MONOTONIC_TIMESTAMP']) - int(start['__MONOTONIC_TIMESTAMP'])) / 10**6, 3),
             'elapsed_source': 'systemd scope start to successful deactivation', 'postflight': recovered['schema'],
             'stopped_for': [], 'tool_versions': [line for line in lines if line.startswith(('rustc 1.', 'cargo 1.', 'cargo-zigbuild ', '0.15.'))],
             'target_at_export_kib': int(lines[-2].split('=')[1]), 'worker_guard_controls': [], 'source_schema': 'bili-display-poll-source-v1'}
    build.write(q / 'arm-postflight.json', recovered)
    build.write(q / 'arm-export.json', proof)
    build.publish(root, exports, 'arm')


if __name__ == '__main__':
    main()
