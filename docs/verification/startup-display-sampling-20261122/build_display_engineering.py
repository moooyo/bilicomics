"""Build one frozen, private display sampling candidate under the shared off-device toolchain."""

import argparse
import json
import os
from pathlib import Path
import shutil
import signal
import stat
import subprocess
from datetime import datetime, timezone
import time
import zipfile


OVERLAY = {'src/platform/mod.rs', 'src/platform/native_capture.rs', 'src/platform/capture_cost.rs',
           'src/native_presentation/portrait_writer.rs'}
WORKER_FILES = ('src/bin/kindle-jpeg-worker.rs', 'src/native_media/worker.rs', 'src/native_media/jpeg.rs',
                'src/native_media/codec.c', 'src/native_media/mod.rs', 'scripts/native-codec-probe.c',
                'scripts/native-codec-bench.h')
LIMITS = {'free_root_kib': 1_415_578, 'free_memory_kib': 3_145_728, 'free_swap_kib': 1_572_864,
          'private_tree_kib': 4_194_304}


def require(value, message):
    if not value:
        raise ValueError(message)


def directory(path):
    info = path.lstat()
    require(path.is_absolute() and path.resolve(strict=True) == path and stat.S_ISDIR(info.st_mode)
            and stat.S_IMODE(info.st_mode) == 0o700 and info.st_uid == 0, 'Expected private build directory')
    return path


def write(path, value):
    with path.open('x') as stream:
        json.dump(value, stream, indent=2)
        stream.write('\n')


def files(root):
    return {file.relative_to(root).as_posix() for file in root.rglob('*') if file.is_file() and not file.is_symlink()}


def freeze(base, overlay, source, package, result):
    require(source.parent == Path('/dev/shm') and source.name.startswith('bilicomics-display-source-') and not list(source.iterdir()),
            'Expected an empty, exclusive RAM-backed source')
    require(files(overlay) == OVERLAY, 'Unexpected engineering overlay')
    for root in (base, overlay):
        require(all(not entry.is_symlink() and (entry.is_file() or entry.is_dir()) for entry in root.rglob('*')),
                'No links or special files in frozen source')
    incoming = files(base)
    for relative in sorted(incoming | OVERLAY):
        target = source / relative
        target.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
        selected = (overlay if relative in OVERLAY else base) / relative
        with selected.open('rb') as original, target.open('xb') as output:
            shutil.copyfileobj(original, output)
        require(target.read_bytes() == selected.read_bytes(), 'Working file is not the frozen input')
    require(files(source) == incoming | OVERLAY, 'Unexpected final compilation input')
    require(all((base / relative).read_bytes() == (package / 'app' / relative).read_bytes() for relative in WORKER_FILES),
            'JPEG worker or codec inputs differ from the installed package source')
    reference = package / 'inputs/kindle-jpeg-worker'
    roster = package / 'app/artifacts/verification/bilicomics-kindle-0.1.25-dev.zip'
    with zipfile.ZipFile(roster) as archive:
        member = 'bilicomics/app/kindle-jpeg-worker'
        require(archive.namelist().count(member) == 1 and reference.read_bytes() == archive.read(member),
                'Retained JPEG worker differs from the exact shipping member')
    destination = result / 'artifacts/kindle-jpeg-worker'
    with reference.open('rb') as original, destination.open('xb') as output:
        shutil.copyfileobj(original, output)
    destination.chmod(0o700)
    require(destination.read_bytes() == reference.read_bytes(), 'Incorrect JPEG worker in staged artifacts')
    write(result / 'qualification/source.json', {'overlay_only': True, 'original_regular_files': len(incoming),
          'frozen_regular_files': len(incoming | OVERLAY), 'overlay': sorted(OVERLAY),
          'worker': {'bytes': destination.stat().st_size, 'unchanged_sources': True, 'identical_to_install_zip': True},
          'worker_files_compared': list(WORKER_FILES)})


def usage(source):
    mem = {line.split(':')[0]: int(line.split()[1]) for line in Path('/proc/meminfo').read_text().splitlines()
           if line.startswith(('MemAvailable:', 'SwapFree:'))}
    root = os.statvfs('/var/tmp')
    private = int(subprocess.check_output(['du', '-sk', source], text=True).split()[0])
    return {'free_root_kib': root.f_frsize * root.f_bavail // 1024, 'free_memory_kib': mem['MemAvailable'],
            'free_swap_kib': mem['SwapFree'], 'private_tree_kib': private}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('mode', choices=('freeze', 'host', 'arm'))
    for name in ('source', 'result', 'base', 'overlay', 'package'):
        parser.add_argument('--' + name, type=Path, required=name in ('source', 'result'))
    args = parser.parse_args()
    os.umask(0o077)
    source, result = directory(args.source), directory(args.result)
    require(source.parent == Path('/dev/shm') and source.name.startswith('bilicomics-display-source-')
            and result.parent == Path('/var/tmp') and result.name.startswith('bilicomics-display-helper-'),
            'Incorrect working or evidence root')
    if args.mode == 'freeze':
        freeze(directory(args.base), directory(args.overlay), source, args.package, result)
        for relative in sorted(files(source)):
            destination = result / relative
            destination.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
            with (source / relative).open('rb') as inp, destination.open('xb') as out:
                shutil.copyfileobj(inp, out)
        (result / 'target').mkdir(mode=0o700)
    else:
        staged_build(source, result, args.mode)


def staged_build(source, result, mode):
    directory(result / 'qualification')
    q = result / 'qualification'
    provenance = json.loads((q / 'source.json').read_text())
    approved = [name for name in ('.cargo', 'src', 'ui', 'assets', 'vendor', 'tests', 'scripts', 'docs')]
    initial = usage(source)
    require(initial['free_memory_kib'] >= 7*1024**2 and initial['free_root_kib'] >= LIMITS['free_root_kib']+16*1024
            and initial['free_swap_kib'] >= 2*1024**2 and initial['private_tree_kib'] <= 1200*1024,
            'Insufficient headroom for isolated product build')
    for relative in OVERLAY:
        require((result/relative).read_bytes()==(source/relative).read_bytes()==(result/'overlay'/relative).read_bytes(), 'Frozen source changed')
    require(all((result/leaf).is_dir() for leaf in approved) and not (q/(mode+'.json')).exists(), 'Incomplete fresh admission')
    if mode=='arm': require(json.loads((q/'host.json').read_text())['passed'] is True, 'Host validation must precede the ARM cross-build')
    command = ('cargo build --locked --offline --no-default-features --features kindle -j1 --bin bilicomics' if mode=='host'
               else 'cargo zigbuild --locked --offline -j1 --release --target armv7-unknown-linux-musleabihf --no-default-features --features kindle --bin bilicomics')
    stage = '/work/target/debug/bilicomics' if mode=='host' else '/work/target/armv7-unknown-linux-musleabihf/release/bilicomics'
    shell = '''set -eu; umask 077; exec 7>&1; exec 1>&2; cd /work; test -z "$(ls -A target)";
mount -t tmpfs -o size=3072M,mode=700,nodev,nosuid tmpfs /work/target;
mkdir -m 700 /work/target/host-home /work/target/host-tmp;
export HOME=/work/target/host-home TMPDIR=/work/target/host-tmp CARGO_TARGET_DIR=/work/target CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0 CARGO_NET_OFFLINE=true RUST_TEST_THREADS=1 CARGO_PROFILE_TEST_DEBUG=0 CARGO_PROFILE_DEV_DEBUG=0;
rustc -vV; cargo -V;
'''
    cache_path = source / 'warm-deps.tgz'
    reuse_host_cache = mode == 'host' and cache_path.is_file()
    if reuse_host_cache:
        shell+='''tar -xzf - -C /work/target; exec </dev/null;
find /work/target/debug/deps -maxdepth 1 -type l -lname '/work/spill/*' -delete;
'''
    if mode == 'arm':
        shell+='cargo-zigbuild --version; zig version; '
    shell+='\n'+command+'; bin='+stage+';\n'
    if mode=='host':
        shell+='''test "$("$bin" --version)" = 'bilicomics 0.1.25';
for profile in scribe pw6; do rc=0; BILICOMICS_DISPLAY_COST_RUN=bili-shelf-0123456789abcdef0123456789abcdef BILICOMICS_APPROVED_PROFILE=$profile BILICOMICS_APPROVED_FIRMWARE=5.19.5 "$bin" --capture-native-display --kindle --device "$profile" > "$TMPDIR/out" 2> "$TMPDIR/err" || rc=$?;
test "$rc" = 1 && ! test -s "$TMPDIR/out" && grep -qF 'Invalid native-read context' "$TMPDIR/err" && grep -qF 'Missing native-read directory' "$TMPDIR/err" || exit 1; echo "passed real display-helper permission refusal: $profile"; done;
'''
    shell+='''echo "target_usage_kib=$(du -sk /work/target | cut -f1)"; cat "$bin" >&7; cmp -s -- "$bin" /proc/self/fd/7 || exit 1; echo 'exported-checked-complete-product'; exec 7>&-;'''
    export=source/(mode+'-product.elf'); log=q/(mode+'-durable-build.log')
    cmd=['systemd-run','--quiet','--collect','--scope','--property=MemoryHigh=3G','--property=MemoryMax=7G',
         '--property=MemorySwapMax=2G','--unit=bilicomics-display-'+mode+'-'+result.name.rsplit('-',1)[1],
         '/opt/kindle-toolchain/ubuntu-run','--build',str(result),'unshare','--net','taskset','-c',str(min(os.sched_getaffinity(0))),
         'sh','-ec',shell]
    now=time.monotonic(); lowest=initial.copy(); reason=None; interrupted=False

    def interrupt(signum, frame):
        nonlocal interrupted
        interrupted=True

    for sig in (signal.SIGTERM,signal.SIGHUP,signal.SIGINT): signal.signal(sig,interrupt)
    with open(cache_path if reuse_host_cache else os.devnull,'rb') as cache, log.open('xb') as logs, export.open('x+b') as output:
        with subprocess.Popen(cmd,stdin=cache,stdout=output,stderr=logs,start_new_session=True) as proc:
            try:
                while proc.poll() is None:
                    sample=usage(source)
                    for key,threshold in LIMITS.items():
                        largest=key=='private_tree_kib'; lowest[key]=(max if largest else min)(lowest[key],sample[key]);
                        if (largest and sample[key]>=threshold) or (not largest and sample[key]<=threshold): reason=key
                    if reason or interrupted:break
                    time.sleep(.5)
            finally:
                if proc.poll() is None:
                    subprocess.run(['systemctl','stop','bilicomics-display-'+mode+'-'+result.name.rsplit('-',1)[1]+'.scope'],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
                    try: os.killpg(proc.pid,signal.SIGTERM)
                    except ProcessLookupError:pass
                    try: proc.wait(timeout=10)
                    except subprocess.TimeoutExpired: os.killpg(proc.pid,signal.SIGKILL); proc.wait(timeout=10)
            code=proc.returncode
    lines=log.read_text(errors='replace').splitlines(); ok=(code==0 and reason is None and not interrupted and 'exported-checked-complete-product' in lines and export.stat().st_size>0)
    data={'mode':mode,'passed':ok,'command':command,'cargo_jobs':1,'incremental':False,'single_cpu':True,'shared_build_lock':True,'network_isolated':True,
          'memory_high_bytes':3*1024**3,'memory_max_bytes':7*1024**3,'swap_max_bytes':2*1024**3,'target_tmpfs_max_bytes':3*1024**3,
          'retained_tmpfs_product_full_byte_match':ok,'exit_status':code,'elapsed_seconds':round(time.monotonic()-now,1),'compiled_bytes':export.stat().st_size,
          'tool_versions':[line for line in lines if line.startswith(('rustc 1.','cargo 1.','cargo-zigbuild ','0.15.'))],
          'worker_guard_controls':[line for line in lines if line.startswith('passed real display-helper')], 'sampled_host_resource_extrema_kib':lowest,
          'target_at_export_kib':next((int(line.split('=')[1]) for line in lines if line.startswith('target_usage_kib=')),None),
          'global_limits_kib':LIMITS,'stopped_for':reason,'utc':datetime.now(timezone.utc).isoformat()}
    if not ok:
        write(q/(mode+'-durable-stop.json'),data)
        raise ValueError('Host-protected build did not qualify; no references published')
    export.chmod(0o700); write(q/(mode+'-export.json'), data)
    dest=result/'artifacts'/('host-bilicomics' if mode=='host' else 'bilicomics'); deadline=time.monotonic()+75
    while usage(source)['free_root_kib']-export.stat().st_size//1024<=LIMITS['free_root_kib']:
        if time.monotonic()>deadline:raise ValueError('Validated ELF remains in named host tmpfs; root destination lacks headroom')
        time.sleep(2)
    with export.open('rb') as inp,dest.open('xb') as out:shutil.copyfileobj(inp,out)
    require(subprocess.run(['cmp','-s','--',export,dest]).returncode==0,'Durable and final product differ')
    dest.chmod(0o700); write(q/('arm-build.json' if mode=='arm' else mode+'.json'),data)
    if mode=='arm':write(q/'build.json',{'host_integration_passed':True,'overlay_only':provenance['overlay_only'],'worker':provenance['worker'],'arm':data})
    print(json.dumps(data,indent=2),flush=True)


if __name__ == '__main__':
    main()
