"""Run the unchanged full capture CLI in private hardware fixtures; do not create physical visit records."""

import ctypes
import json
import os
from pathlib import Path
import socket
import subprocess
import sys
import time
import uuid


ROOT = Path('/work')
TOOLS = ROOT/'inputs'
sys.path.insert(0, str(TOOLS/'studies/startup-display-sampling-20261122'))
import collect_display
import host_bench


def require(condition, message):
    if not condition:
        raise ValueError(message)


def now():
    return time.clock_gettime_ns(time.CLOCK_MONOTONIC)


def checked_ancestry(row, observed):
    matches = {'helper': ('helper_pid', 'helper_birth'), 'leader': ('native_read_pid', 'native_read_birth'), 'master': ('supervisor_pid', 'supervisor_birth')}
    for event in observed:
        for key, (pid, birth) in matches.items():
            require((row[pid], row[birth]) == (event[key]['pid'], event[key]['birth']), 'Recorded ownership differs from live getter ancestry')
    require(row['native_read_sequence'] == 1, 'Wrong fresh native-read transaction sequence')


def trial(profile, mode, executable_bytes, output):
    case = output / (profile+'-'+mode)
    case.mkdir(mode=0o700)
    run = 'bili-shelf-' + uuid.uuid4().hex
    home = Path('/mnt/us')/run
    (home/'app').mkdir(mode=0o700, parents=True)
    (home/'data').mkdir(mode=0o700)
    (home/'run-id').write_text(run+'\n')
    data = home/'data'/collect_display.name_for(run)
    executable = home/'app/bilicomics'
    template = Path('/mnt/us/fixture-binary')
    template.rename(executable)
    require(executable.stat().st_size == executable_bytes and executable.stat().st_nlink == 1, 'Wrong isolated executable')
    inventory = Path('/run/display-product/input-inventory')
    inventory.write_text('B: ABS=1000000\n' if profile == 'scribe' else 'B: ABS=1000003\n')
    require(inventory.read_bytes() == Path('/proc/bus/input/devices').read_bytes(), 'Private input inventory not mounted')
    audit = case/'fb-audit'
    audit.touch(exist_ok=False)
    env = {'PATH':str(TOOLS)+':'+os.defpath, 'LC_ALL':'C', 'BILICOMICS_HOME':str(home),'BILICOMICS_APPROVED_PROFILE':profile,
           'BILICOMICS_APPROVED_FIRMWARE':'fixture', 'FIXTURE_TOOLS':str(home/'app'), 'FIXTURE_PROFILED':'0','PROFILE':profile,
           'DISPLAY_FIXTURE_MODE':mode,'DISPLAY_FIXTURE_CASE':str(case),'DISPLAY_FIXTURE_FB_AUDIT':str(audit),'LD_PRELOAD':str(TOOLS/'fb-read.so')}
    if mode != 'selector-off':
        env['BILICOMICS_DISPLAY_COST_RUN'] = run
    shell = subprocess.Popen(['/bin/sh', str(output/'stock-capture.sh')], cwd=case, env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, start_new_session=True)
    try:
        require(host_bench.line(shell.stdout.fileno()) == 'armed', 'Stock fixture shell did not reach its read gate')
        started = now()
        shell.stdin.write(b'read\n'); shell.stdin.flush()
        if mode == 'revoke':
            gate = case/'getter-started'
            wait = time.monotonic() + 1.5
            while not gate.exists() and time.monotonic() < wait:
                time.sleep(.005)
            require(gate.is_file() and (case/'lock/native-read-go').is_file(), 'No live getter/permit to revoke')
            (case/'lock/native-read-go').unlink()
        stdout, stderr = shell.communicate(timeout=8)
        finished = now()
        require(shell.returncode == 0 and not stderr, 'Unexpected stock-shell capture status or diagnostics')
        host_bench.inspect_group()
        require(not (case/'lock/native-read-go').exists(), 'Native-read permission not retired')
        require((case/'getters').is_file(), 'No actual getter observed in the stock transaction')
        audit_rows = [json.loads(line) for line in (case/'getters').read_text().splitlines()]
        if mode in ('revoke','timeout'):
            require(stdout == b'result refused 1\n' and not any(data.parent.iterdir()) and audit.read_bytes() == b'' and [item['name'] for item in audit_rows] == ['power'], 'Failure emitted successful evidence or continued hardware I/O')
            require((case/'getter-started').is_file(), 'A timed refusal did not start the actual getter')
            require(not (case/'delayed-getter-finished').exists(), 'Delayed getter was allowed to return')
            if mode == 'revoke':
                require(finished-started < 1_700_000_000, 'Refusal after revocation was not distinguishable from the two-second budget')
            else:
                require(finished-started >= 1_800_000_000, 'Getter refused before the original timeout could be responsible')
            return {'passed':True,'scope':'withdrawn permission' if mode == 'revoke' else 'delayed getter and original budgets','stock_refused':True,'no_sidecar':True,'owned_group_reaped':True,'only_first_getter_started':True}
        expected = ['power','mode','holding','holding','power'] if profile=='scribe' else ['power','mode','power']
        require(stdout == (b'result ready:Y8:U:0 1\n' if profile=='scribe' else b'result ready:Y8:FIXED:0 1\n'), 'Stock shell did not parse the successful full product capture')
        require(audit.read_bytes() == b'VVFVVF' and [item['name'] for item in audit_rows] == expected, 'Unexpected real metadata and query sequence')
        require(all(item['master']['pid']==shell.pid for item in audit_rows), 'Not the owning stock shell')
        if mode=='selector-off':
            require(not any(data.parent.iterdir()), 'Unselected ordinary capture emitted a diagnostic')
            return {'passed':True,'stock_capture_succeeded':True,'no_sidecar':True,'owned_group_reaped':True}
        boot = Path('/proc/sys/kernel/random/boot_id').read_text().strip()
        require(data.is_file() and data.stat().st_mode & 0o777 == 0o600, 'Product did not create a private run-specific sidecar')
        capture = subprocess.run(['/bin/sh', '-c', collect_display.capture_command(run, str(home), boot)], capture_output=True, timeout=4)
        require(capture.returncode == 0 and not capture.stderr, 'The device-side collector rejected the product record')
        raw = capture.stdout
        require(raw == data.read_bytes(), 'The collector did not read the original product record')
        record = collect_display.decode(raw,run,boot,os.getpid())
        checked_ancestry(record,audit_rows)
        summary = collect_display.components(record,started,finished)
        require(summary['query_branch'] == ('automatic-five' if profile=='scribe' else 'fixed-three') and summary['process_stat_reads']>0, 'Collector did not validate matching actual counts and timings')
        return {'passed':True, 'natural_stock_capture_result':'ready', 'natural_branch':summary['query_branch'], 'queries':len(expected), 'metadata_passes':2, 'real_proc_reads_positive':True,
                'live_getters_witnessed_all_three_process_births':True, 'owned_group_reaped':True, 'original_collector_read_decode_and_components':True}
    finally:
        if shell.poll() is None:
            shell.terminate(); shell.wait(timeout=5)
        executable.rename(template)


def main():
    os.umask(0o077)
    require(Path.cwd()==ROOT and len(sys.argv)==3 and sys.argv[2] in {'r1','r2','r3'} and os.geteuid()==0 and [name for _,name in socket.if_nameindex()]==['lo'], 'Unexpected qualification namespace')
    require(ctypes.CDLL(None).prctl(36,1,0,0,0)==0,'Cannot observe owned descendant retirement')
    output=ROOT/'reports'/sys.argv[2]
    output.mkdir(mode=0o700)
    host_bench.make_shell(TOOLS/'stock-launch.sh',output/'stock-capture.sh')
    for usec in (10000,20000):
        begin=now(); subprocess.run([TOOLS/'usleep',str(usec)],check=True); require(now()-begin>=usec*1000,'Invalid poll delay')
    size=int(sys.argv[1]); observations={}
    for profile, mode in [('scribe','selected'),('pw6','selected'),('scribe','selector-off'),('scribe','revoke'),('pw6','timeout')]:
        try:
            observations[profile+'-'+mode]=trial(profile,mode,size,output)
        except Exception as error:
            (output/'failure.txt').write_text(profile+' '+mode+': '+str(error)+'\n')
            raise
    report={'schema':'bili-display-host-product-qualification-v1','passed':True,'complete_product_elf':True,'unchanged_permit':True,'only_hardware_inputs_synthetic':True,
            'physical_device_accessed':False,'collector_validated':True,'host_elf_bytes':size,'profiles':{name:observations[name+'-selected'] for name in ('scribe','pw6')},
            'controls':{name:result for name,result in observations.items() if not name.endswith('-selected')},
            'scope':'Full Linux x86-64 Kindle-feature product, original launcher functions and actual PID/session/permission checks. The private framebuffer substitutes only GET_VAR/GET_FIX on its own file, the private input inventory selects automatic/fixed behavior, and a separate executable supplies synthetic LIPC answers. The production collector raw read, decode and components operate on actual product bytes in a real host command interval; no card, UI retirement, firmware, release or physical sample is fabricated.'}
    with (output/'host-product-qualification.json').open('x') as stream: stream.write(json.dumps(report,indent=2)+'\n')
    print('Both actual product records, their ownership, and all three controls passed.')


if __name__=='__main__':
    main()
