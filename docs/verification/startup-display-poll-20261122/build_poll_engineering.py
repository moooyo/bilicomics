"""Freeze and build a private, bounded off-device display-poll candidate on test-env."""

import argparse
import json
import os
from pathlib import Path
import shutil
import signal
import stat
import subprocess
import time
import zipfile


STUDY = Path(__file__).resolve().parent
PREVIOUS = STUDY.parent / 'startup-display-sampling-20261122'
OVERLAY = {'src/platform/mod.rs', 'src/platform/native_capture.rs', 'src/platform/capture_cost.rs',
           'src/native_presentation/portrait_writer.rs'}
WORKER = ('src/bin/kindle-jpeg-worker.rs', 'src/native_media/worker.rs', 'src/native_media/jpeg.rs',
          'src/native_media/codec.c', 'src/native_media/mod.rs', 'scripts/native-codec-probe.c',
          'scripts/native-codec-bench.h')
SELECTOR = 'BILICOMICS_DISPLAY_POLL_RUN'
FLOORS = {'free_root_kib': 1_415_578, 'free_memory_kib': 3_145_728, 'free_swap_kib': 1_572_864}
HIGH, MAX, SWAP, TARGET = 6174015488, 7516192768, 2147483648, 3221225472
HOST = 'cargo build --locked --offline --no-default-features --features kindle -j1 --bin bilicomics'
ARM = 'cargo zigbuild --locked --offline -j1 --release --target armv7-unknown-linux-musleabihf --no-default-features --features kindle --bin bilicomics'


def require(condition, reason):
    if not condition:
        raise ValueError(reason)


def private(path):
    info = path.lstat()
    require(path.resolve(strict=True) == path and info.st_uid == 0 and stat.S_ISDIR(info.st_mode)
            and stat.S_IMODE(info.st_mode) == 0o700, 'Expected a private absolute directory')
    return path


def data(path):
    return json.loads(path.read_bytes())


def write(path, value):
    with path.open('x') as out:
        json.dump(value, out, indent=2)
        out.write('\n')


def roster(path):
    result = {}
    for entry in path.rglob('*'):
        info = entry.lstat()
        require(info.st_uid == 0 and (stat.S_ISDIR(info.st_mode) or stat.S_ISREG(info.st_mode) and info.st_nlink == 1),
                'Unexpected source file type or ownership')
        if stat.S_ISREG(info.st_mode):
            result[entry.relative_to(path).as_posix()] = [info.st_dev, info.st_ino, info.st_size, info.st_mtime_ns]
    return result


def freeze(root, previous, package):
    source, evidence = private(root / 'workspace'), private(root / 'qualification')
    inputs = roster(source)
    require(len(inputs) == 548 and 'src/shelf_render_trace.rs' in inputs and 'src/platform/capture_cost.rs' not in inputs
            and set(leaf.split('/')[0] for leaf in inputs) == {'.cargo', 'src', 'ui', 'assets', 'vendor', 'tests', 'scripts',
                                                              'docs', 'Cargo.toml', 'Cargo.lock', 'build.rs'}, 'Unexpected stock inputs')
    require({leaf for leaf in inputs if leaf.startswith('scripts/')} == set(WORKER[-2:]) and
            all(not leaf.startswith('docs/') or leaf.startswith('docs/research/aidoku-spatial-dither-20261116/')
                for leaf in inputs), 'Unexpected auxiliary input')
    for relative in (*WORKER, 'src/platform/native_capture.rs', 'src/native_presentation/portrait_writer.rs'):
        require((source / relative).read_bytes() == (package / 'app' / relative).read_bytes(), 'Stock guard/worker input changed')
    for script, output in ((PREVIOUS / 'stage_overlay.py', root / 'old-anchor-check'), (STUDY / 'stage_overlay.py', root / 'overlay')):
        subprocess.run(['python3', '-B', script, source, output], check=True)
        require(set(roster(output)) == OVERLAY, 'Unexpected generated Rust source')
    for relative in OVERLAY:
        require((root / 'old-anchor-check' / relative).read_bytes() == (previous / 'overlay' / relative).read_bytes(),
                'Previous measured overlay does not reproduce from the supplied stock anchors')
        src, dst = root / 'overlay' / relative, source / relative
        if relative in inputs:
            dst.unlink()
        with src.open('rb') as original, dst.open('xb') as target:
            shutil.copyfileobj(original, target)
        require(dst.read_bytes() == src.read_bytes(), 'Generated source was not frozen')
    selected = (source / 'src/platform/capture_cost.rs').read_text()
    require(all(value in selected for value in (SELECTOR, 'kindle-startup-display-poll-v1', '.display-poll.json',
                'command_poll_stdout_open', 'command_poll_after_eof')) and 'BILICOMICS_DISPLAY_COST_RUN' not in selected,
            'Wrong observation selector or partition')
    final = roster(source)
    require(set(final) == (inputs.keys() | OVERLAY) and all(final[key] == inputs[key] for key in inputs.keys() - OVERLAY),
            'Change outside the four generated Rust inputs')
    write(evidence / 'frozen-inputs.json', final)
    with zipfile.ZipFile(package / 'app/artifacts/verification/bilicomics-kindle-0.1.25-dev.zip') as archive:
        member = 'bilicomics/app/kindle-jpeg-worker'
        require(archive.namelist().count(member) == 1, 'Expected exactly one installed JPEG worker')
        shipped = archive.read(member)
    require(shipped == (package / 'inputs/kindle-jpeg-worker').read_bytes(), 'Shipping worker source disagreement')
    worker = root / 'artifacts/kindle-jpeg-worker'
    with worker.open('xb') as out:
        out.write(shipped)
    worker.chmod(0o700)
    require(worker.read_bytes() == shipped and all((source / leaf).read_bytes() == (package / 'app' / leaf).read_bytes()
            for leaf in WORKER), 'Worker is not unchanged')
    write(evidence / 'source.json', {'schema': 'bili-display-poll-source-v1', 'original_regular_files': len(inputs),
          'frozen_regular_files': len(final), 'overlay_only': True, 'overlay': sorted(OVERLAY), 'old_overlay_reproduced': True,
          'stock_guard_sources_compared': True, 'worker_files_compared': list(WORKER), 'selector': SELECTOR,
          'receipt_suffix': '.display-poll.json', 'receipt_schema': 'kindle-startup-display-poll-v1',
          'worker': {'bytes': len(shipped), 'unchanged_sources': True, 'identical_to_install_zip': True}})
    (source / 'target').mkdir(mode=0o700)


def usage():
    memory = {row.split(':', 1)[0]: int(row.split()[1]) for row in Path('/proc/meminfo').read_text().splitlines()
              if row.startswith(('MemAvailable:', 'SwapFree:'))}
    root = os.statvfs('/var/tmp')
    return {'free_root_kib': root.f_bavail * root.f_frsize // 1024, 'free_memory_kib': memory['MemAvailable'],
            'free_swap_kib': memory['SwapFree']}


SETUP = '''set -eu; umask 077; exec 7>&1; exec 1>&2; cd /; test -z "$(ls -A /work/target)";
mount --bind /work /work; mount -o remount,bind,ro /work;
mount -t tmpfs -o size=3072M,mode=700,nodev,nosuid tmpfs /work/target;
for prefix in /opt/kindle-toolchain/tools /opt/kindle-toolchain/cache; do mount --bind "$prefix" "$prefix"; mount -o remount,bind,ro "$prefix"; done;
base=/work/target/private; mkdir -m700 "$base";
export HOME="$base/home" TMPDIR="$base/tmp" CARGO_HOME="$base/cargo-home" ZIG_GLOBAL_CACHE_DIR="$base/zig" XDG_CACHE_HOME="$base/cache";
mkdir -m700 "$HOME" "$TMPDIR" "$CARGO_HOME" "$CARGO_HOME/registry" "$ZIG_GLOBAL_CACHE_DIR" "$XDG_CACHE_HOME";
for item in registry zig; do mkdir -m700 "$base/$item-upper" "$base/$item-work"; done;
mount -t overlay overlay -o "lowerdir=/opt/kindle-toolchain/tools/cargo/registry,upperdir=$base/registry-upper,workdir=$base/registry-work" "$CARGO_HOME/registry";
mount -t overlay overlay -o "lowerdir=/opt/kindle-toolchain/cache/zig,upperdir=$base/zig-upper,workdir=$base/zig-work" "$ZIG_GLOBAL_CACHE_DIR";
export CARGO_TARGET_DIR=/work/target CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0 CARGO_NET_OFFLINE=true RUSTUP_NO_UPDATE_CHECK=1 CARGO_PROFILE_DEV_DEBUG=0;
cd /work; rustc -vV; cargo -V; cargo-zigbuild --version; zig version; cargo metadata --locked --offline --no-deps --format-version 1 >/dev/null;
'''
HOST_CONTROL = '''test "$("$bin" --version)" = 'bilicomics 0.1.25';
for profile in scribe pw6; do rc=0; BILICOMICS_DISPLAY_POLL_RUN=bili-shelf-0123456789abcdef0123456789abcdef BILICOMICS_APPROVED_PROFILE="$profile" BILICOMICS_APPROVED_FIRMWARE=5.19.5 "$bin" --capture-native-display --kindle --device "$profile" >"$TMPDIR/out" 2>"$TMPDIR/err" || rc=$?;
test "$rc" = 1 && ! test -s "$TMPDIR/out" && grep -qF 'Invalid native-read context' "$TMPDIR/err" && grep -qF 'Missing native-read directory' "$TMPDIR/err" || exit 1; echo "passed real display-helper permission refusal: $profile"; done;
'''


def build(root, export_root, mode):
    source, evidence, export_root = private(root / 'workspace'), private(root / 'qualification'), private(export_root)
    require(export_root.parent == Path('/dev/shm') and export_root.name == root.name + '-exports', 'Wrong durable exporter')
    require(data(evidence / 'source.json')['schema'] == 'bili-display-poll-source-v1' and
            roster(source) == data(evidence / 'frozen-inputs.json'), 'Unrecognized or changed build source')
    expected = data(evidence / 'source.json')
    require(all((root / 'overlay' / leaf).read_bytes() == (source / leaf).read_bytes() for leaf in OVERLAY), 'Changed overlay')
    arm = mode == 'arm'
    if arm:
        host = data(evidence / 'host.json')
        require(host['schema'] == 'bili-display-poll-host-build-v1' and host['passed'] is True, 'New host binary must be built first')
    command = ARM if arm else HOST
    program = '/work/target/armv7-unknown-linux-musleabihf/release/bilicomics' if arm else '/work/target/debug/bilicomics'
    shell = SETUP + command + '\nbin=' + program + '\n' + ('' if arm else HOST_CONTROL) + '''
echo "target_usage_kib=$(du -sk /work/target | cut -f1)"; cat "$bin" >&7; cmp -s "$bin" /proc/self/fd/7; echo 'exported-checked-complete-product'; exec 7>&-;'''
    initial = usage()
    require(initial['free_root_kib'] > FLOORS['free_root_kib'] + 8192 and initial['free_memory_kib'] >= 7 * 1024**2
            and initial['free_swap_kib'] >= 2 * 1024**2 and shutil.disk_usage(export_root).free >= 256 * 1024**2,
            'Wait off-device for admission headroom; build has not started')
    unit = 'bilicomics-poll-' + mode + '-' + root.name.rsplit('-', 1)[1]
    cmd = ['systemd-run', '--quiet', '--collect', '--scope', '--property=MemoryHigh=' + str(HIGH),
           '--property=MemoryMax=' + str(MAX), '--property=MemorySwapMax=' + str(SWAP), '--unit=' + unit,
           '/opt/kindle-toolchain/ubuntu-run', '--build', str(source), 'unshare', '--net', 'taskset', '-c',
           str(min(os.sched_getaffinity(0))), 'sh', '-ec', shell]
    lowest, reason = initial.copy(), []
    for signum in (signal.SIGHUP, signal.SIGINT, signal.SIGTERM):
        signal.signal(signum, lambda number, _: reason.append('signal ' + str(number)))
    exported = export_root / (mode + '.elf')
    log = evidence / (mode + '-build.log')
    started = time.monotonic()
    with exported.open('x+b') as out, log.open('xb') as logs:
        with subprocess.Popen(cmd, stdin=subprocess.DEVNULL, stdout=out, stderr=logs, start_new_session=True) as proc:
            try:
                while proc.poll() is None and not reason:
                    used = usage()
                    for key, floor in FLOORS.items():
                        lowest[key] = min(lowest[key], used[key])
                        if used[key] <= floor:
                            reason.append(key)
                    if time.monotonic() - started > 3600:
                        reason.append('wall-clock bound')
                    if not reason:
                        time.sleep(0.5)
            finally:
                if proc.poll() is None:
                    subprocess.run(['systemctl', 'stop', unit + '.scope'], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
                    if proc.poll() is None:
                        try:
                            os.killpg(proc.pid, signal.SIGTERM)
                        except ProcessLookupError:
                            pass
    lines = log.read_text(errors='replace').splitlines()
    passed = proc.returncode == 0 and not reason and 'exported-checked-complete-product' in lines and exported.stat().st_size > 0
    proof = {'schema': 'bili-display-poll-' + mode + '-build-v1', 'mode': mode, 'passed': passed, 'command': command,
             'compiled_bytes': exported.stat().st_size, 'exit_status': proc.returncode, 'cargo_jobs': 1, 'incremental': False,
             'single_cpu': True, 'shared_build_lock': True, 'network_isolated': True, 'shared_caches_read_only': True,
             'memory_high_bytes': HIGH, 'memory_max_bytes': MAX, 'swap_max_bytes': SWAP, 'target_tmpfs_max_bytes': TARGET,
             'retained_tmpfs_product_full_byte_match': passed, 'sampled_host_resource_extrema_kib': lowest,
             'global_limits_kib': FLOORS, 'elapsed_seconds': round(time.monotonic() - started, 1), 'stopped_for': reason,
             'tool_versions': [line for line in lines if line.startswith(('rustc 1.', 'cargo 1.', 'cargo-zigbuild ', '0.15.'))],
             'target_at_export_kib': next((int(line.split('=')[1]) for line in lines if line.startswith('target_usage_kib=')), None),
             'worker_guard_controls': [line for line in lines if line.startswith('passed real display-helper')], 'source_schema': expected['schema']}
    if not passed:
        write(evidence / (mode + '-stop.json'), proof)
        raise ValueError('Build did not qualify; no executable published')
    write(evidence / (mode + '-completed-build.json'), proof)
    require(roster(source) == data(evidence / 'frozen-inputs.json'), 'Inputs changed during build')
    write(evidence / (mode + '-export.json'), proof)
    publish(root, export_root, mode)


def publish(root, export_root, mode):
    evidence = private(root / 'qualification')
    require(private(export_root).parent == Path('/dev/shm') and export_root.name == root.name + '-exports', 'Wrong exporter')
    proof = data(evidence / (mode + '-export.json'))
    path = export_root / (mode + '.elf')
    size = path.stat().st_size
    require(path.resolve(strict=True) == path and path.is_file() and proof['passed'] is True and proof['compiled_bytes'] == size,
            'Missing validated durable product')
    require(usage()['free_root_kib'] > FLOORS['free_root_kib'] + (size + 1023) // 1024 + 8192,
            'Retained validated ELF on tmpfs; wait for sufficient publication space')
    target = private(root / 'artifacts') / ('host-bilicomics' if mode == 'host' else 'bilicomics')
    with path.open('rb') as src, target.open('xb') as out:
        shutil.copyfileobj(src, out)
    require(subprocess.run(['cmp', '-s', '--', path, target]).returncode == 0, 'Durable and published product differ')
    target.chmod(0o700)
    write(evidence / ('host.json' if mode == 'host' else 'arm-build.json'), proof)
    path.unlink()
    print(json.dumps(proof), flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('mode', choices=('freeze', 'host', 'arm', 'publish-host', 'publish-arm'))
    for name in ('root', 'previous', 'package', 'exports'):
        parser.add_argument('--' + name, type=Path, required=name == 'root')
    args = parser.parse_args()
    os.umask(0o077)
    root = private(args.root)
    require(root.parent == Path('/var/tmp') and root.name.startswith('bilicomics-display-poll-build-'), 'Wrong private build root')
    if args.mode == 'freeze':
        freeze(root, private(args.previous), args.package)
    elif args.mode.startswith('publish-'):
        publish(root, args.exports, args.mode.split('-')[1])
    else:
        build(root, args.exports, args.mode)


if __name__ == '__main__':
    main()
