"""Build/verify the selected 2026-11-19 candidate inside its private test-env overlay."""

import datetime
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import signal
import subprocess
import sys
import tarfile
import time
import zipfile

os.umask(0o077)
sys.dont_write_bytecode = True
WORK = Path(__file__).resolve().parent
OLD = Path(os.environ['BILI_BASE_BUILD_ROOT'])
OLDZIP = Path(os.environ['BILI_BASE_ZIP'])
CANDIDATE = '57265de60553ea97b01894592427d60c9008f850'
BASECOMMIT = 'd81b529fd8f84790d91dfe3a8396853c296a8280'
BASEZIP = 'c57d5fc3674f1a0de448b85beb2552ee0959b83756c3e8f62afb559bf62a4882'
BASEELF = '04fc6f93b4327911ca1a3bb5200175c9f2e13fb21ac5e56280012ff44b6e2e59'
BASEWORKER = '12c8988c6a93cade9923d54bef784b774e13cb5e3b042f2e72ea5ecb6b897beb'
BASEMAN = '8537123d6b1dc0c75a048b09d1204664988a6c1668ea35d721bfbac641ff49c6'
BASECOMPILER = '8dae7219954e57236434f834b51c6dd74256ad7e14e5b68f88cf91864f31c955'
BASEPROOF = '9c48a552b5a908dedc6bad6b75b0fc19da6a69f2699fc17079bdb62d5f37b10b'
BASEFIXTURE = '2a8351a3161973b9d7c76e5129bbfb16bb37b9f8ad7c24b4322f40bfce478b69'
COMBINED = WORK / 'combined'
CWD = OLD / 'candidate-r1/app'
TARGET = OLD / 'target'
S = COMBINED / 'selected'
LOGS = COMBINED / 'logs'
NATIVE = 'armv7-unknown-linux-musleabihf'
ENV = {k: v for k, v in os.environ.items() if not k.startswith(('CARGO_', 'RUST', 'SLINT_', 'BILICOMICS_')) and k not in ('DISPLAY','WAYLAND_DISPLAY')}
ENV.update(PATH='/root/.cargo/bin:' + os.environ['PATH'], CARGO_TARGET_DIR=str(TARGET), CARGO_BUILD_JOBS='2', CARGO_INCREMENTAL='0', CARGO_PROFILE_DEV_INCREMENTAL='false', CARGO_PROFILE_TEST_INCREMENTAL='false', CARGO_NET_OFFLINE='true', CARGO_TERM_COLOR='never', CARGO_TERM_VERBOSE='false', RUST_TEST_THREADS='1', RUSTC_WRAPPER='', RUSTC_WORKSPACE_WRAPPER='', PYTHONDONTWRITEBYTECODE='1', LC_ALL='C', TZ='UTC', TMPDIR=str(COMBINED/'tmp'))
records = []


def need(expr, note):
    if not expr:
        raise RuntimeError(note)


def digest(path):
    with path.open('rb') as f:
        return hashlib.file_digest(f, 'sha256').hexdigest()


def hd(b):
    return hashlib.sha256(b).hexdigest()


def new(path, value):
    with path.open('x') as out:
        if isinstance(value, str):
            out.write(value)
        else:
            json.dump(value, out, sort_keys=True, indent=2)
            out.write('\n')


def ident(path):
    return {'path': str(path), 'bytes': path.stat().st_size, 'sha256': digest(path)}


def now():
    return datetime.datetime.now(datetime.timezone.utc).isoformat()


def inputs(source):
    sys.path.insert(0, str(source / 'scripts/verification'))
    from native_protocol import build_input_manifest
    native = build_input_manifest(source)
    included = [p for p in source.rglob('*') if p.is_file() and not p.is_symlink() and not {'__pycache__', 'artifacts'}.intersection(p.relative_to(source).parts)]
    return native, {p.relative_to(source).as_posix():digest(p) for p in sorted(included)}


def base_tools():
    profile=OLD/'checks-r3/compiler-profile.json'
    proof=OLD/'checks-r3/arm-compilation-proof.json'
    need(digest(profile)==BASECOMPILER and digest(proof)==BASEPROOF and digest(OLD/'startup-boundaries-r1.py')==BASEFIXTURE, 'Retained compiler/fixture provenance differs')
    reference=json.loads(proof.read_bytes())['verbose_log']
    log=OLD/'checks-r3/logs/arm-build-cargo-verbose.log'
    need(Path(reference['path'])==log and digest(log)==reference['sha256'], 'Use the actual recorded build root and compiler evidence')


def extract(candidate):
    archive=WORK/('candidate-files.tar.gz' if candidate else 'baseline.tar.gz')
    destination=COMBINED/'app' if candidate else COMBINED
    expected=set((COMBINED/'allowed-changes.txt').read_text().splitlines())
    parents={str(p) for file in expected for p in Path(file).parents}
    with tarfile.open(archive, 'r:gz') as tar:
        need(tar.pax_headers.get('comment')==(CANDIDATE if candidate else BASECOMMIT), 'Git archive does not bind the required commit')
        members=tar.getmembers(); names=set(); files=set()
        for item in members:
            name=item.name.rstrip('/') if item.isdir() else item.name
            path=Path(name)
            need(not path.is_absolute() and '..' not in path.parts and str(path)==name and name not in names and (item.isfile() or item.isdir()), 'Unexpected archive member')
            names.add(name)
            need((name in (expected if item.isfile() else parents)) if candidate else (path.parts and path.parts[0]=='app'), 'Unexpected archive path')
            if item.isfile(): files.add(name)
        need(bool(files) and (not candidate or files==expected), 'Incomplete Git archive')
        tar.extractall(destination, members=members, filter='data')


def before():
    src = COMBINED / 'app'
    base_tools()
    native, initial = inputs(src)
    need(hd(native.encode()) == BASEMAN, 'Pristine git archive differs from previously compiled native inputs')
    need(digest(OLDZIP) == BASEZIP and digest(OLDZIP.parent/'bilicomics') == BASEELF and digest(OLDZIP.parent/'kindle-jpeg-worker') == BASEWORKER, 'Original artifacts differ')
    new(WORK/'baseline/source-inputs.sha256', native)
    new(WORK/'baseline/archive-inputs.json', initial)
    new(WORK/'baseline/provenance.json',{'package':ident(OLDZIP), 'application':ident(OLDZIP.parent/'bilicomics'), 'worker':ident(OLDZIP.parent/'kindle-jpeg-worker'), 'root':BASECOMMIT, 'source_fingerprint':BASEMAN,'inputs':len(native.splitlines()),'compiler_reference':ident(OLD/'checks-r3/compiler-profile.json')})
    print('Qualified baseline git sources and existing binaries:',len(native.splitlines()),len(initial),flush=True)


def cmd(name, arguments, *, cwd=CWD, env=None, lim=1600, record=records):
    log = LOGS/(name+'.log')
    started=now(); wall=time.monotonic()
    print('START',name,started,flush=True)
    with log.open('xb') as f:
        proc=subprocess.Popen([str(v) for v in arguments],cwd=cwd,env=env or ENV,stdout=f,stderr=subprocess.STDOUT,start_new_session=True)
        try:
            while True:
                try:
                    code=proc.wait(timeout=2);break
                except subprocess.TimeoutExpired:
                    need(time.monotonic()-wall<lim, 'Stage exceeded deadline: '+name)
                    need(shutil.disk_usage(WORK).free>1_300_000_000, 'Free disk safety threshold reached: '+name)
        except BaseException:
            if proc.poll() is None:
                os.killpg(proc.pid,signal.SIGTERM)
                try: proc.wait(timeout=6)
                except subprocess.TimeoutExpired:
                    os.killpg(proc.pid,signal.SIGKILL);proc.wait(timeout=6)
            raise
    record.append({'name':name,'started':started,'ended':now(),'elapsed_seconds':round(time.monotonic()-wall,3),'command':[str(x) for x in arguments], 'exit':code,'log':ident(log)})
    need(code==0, f'{name} returned {code}: {log}')
    print('DONE', name, record[-1]['elapsed_seconds'],flush=True)
    return log


def compiler():
    base_tools()
    ref=json.loads((OLD/'checks-r3/compiler-profile.json').read_text())
    for info in ref['tools'].values():
        fixed=info.get('executable',info)
        need(digest(Path(fixed['path']))==fixed['sha256'], 'Pinned compiler changed')
    check={name:digest(Path(v.get('executable',v)['path'])) for name,v in ref['tools'].items()}
    return {'fingerprint':digest(OLD/'checks-r3/compiler-profile.json'),'matched_fixed_executables':check,'release_profile':ref['profile']}


def arm():
    src=COMBINED/'app'; initial=json.loads((WORK/'baseline/archive-inputs.json').read_text()); native, current=inputs(src)
    diff={k:{'was':initial.get(k),'now':current.get(k)} for k in sorted(initial.keys()|current.keys()) if current.get(k)!=initial.get(k)}
    expected=set((COMBINED/'allowed-changes.txt').read_text().splitlines())
    need(diff.keys()==expected and len(diff)>0,'Unreviewed source delta: '+repr(list(diff)))
    print('Candidate delta:', sorted(diff), flush=True)
    compiler_id=compiler()
    new(COMBINED/'source-inputs.sha256',native)
    new(COMBINED/'changed-files.json',diff)
    need(shutil.disk_usage(WORK).free>1_650_000_000,'Insufficient initial disk headroom')
    cmd('clean-arm-selected',['cargo','clean','--locked','--offline','--release','--target',NATIVE,'-p','bilicomics','-p','slint-backend-kindle'])
    cmd('arm-build',['sh','scripts/build-kindle.sh',str(OLD/'candidate-r1')],env=dict(ENV,CARGO_TERM_VERBOSE='true'))
    log=src/'artifacts/verification/kindle-build.log'; body=log.read_text(errors='replace'); shutil.copyfile(log,LOGS/'arm-cargo-verbose.log'); raw=LOGS/'arm-cargo-verbose.log'
    rust={}
    for crate,kind in [('bilicomics','lib'),('bilicomics','bin'),('slint_backend_kindle','lib'),('kindle_jpeg_worker','bin')]:
        lines=[l for l in body.splitlines() if 'Running `' in l and 'rustc --crate-name '+crate+' ' in l and '--crate-type '+kind+' ' in l and '--target '+NATIVE in l]
        need(len(lines)>0,'Missing real rustc invocation: '+crate+' '+kind); rust[crate+':'+kind]=lines[-1]
    need(all(s in rust['bilicomics:bin'] for s in ('-C opt-level=3','-C lto=thin','-C codegen-units=1','-C strip=symbols','src/main.rs','--extern slint_backend_kindle=')), 'ARM compiler flags differ')
    S.mkdir(mode=0o700)
    for f in ('bilicomics','kindle-jpeg-worker'):
        mo=COMBINED/'kindle-package/bilicomics/app'/f; need(digest(mo)==digest(TARGET/NATIVE/'release'/f),'Wrong staged ELF'); shutil.copy2(mo,S/f)
    zipname='bilicomics-kindle-0.1.25-dev.zip'; z=S/zipname
    for a in (src/'artifacts/verification'/zipname,(src/'artifacts/verification'/zipname).with_suffix('.manifest.json')): shutil.copy2(a,S/a.name)
    cmd('package-static',[sys.executable,'-B',src/'scripts/verification/verify_package.py','--archive',z,'--binary',S/'bilicomics','--worker',S/'kindle-jpeg-worker','--app-root',src,'--output',COMBINED/'package-static.json','--expected-binary-sha256',digest(S/'bilicomics'),'--expected-worker-sha256',digest(S/'kindle-jpeg-worker'),'--expected-archive-sha256',digest(z)])
    with zipfile.ZipFile(OLDZIP) as prior, zipfile.ZipFile(z) as cur:
        need(set(prior.namelist()) == set(cur.namelist()) and len(cur.namelist()) == 49, 'ZIP member roster differs')
        members={k:hd(cur.read(k)) for k in cur.namelist()}; delta=sorted(k for k in members if hd(prior.read(k))!=members[k]); print('ZIP differences:',delta,flush=True)
        need(set(delta)<=set(('bilicomics/app/bilicomics','bilicomics/app/kindle-jpeg-worker','bilicomics/launch.sh','notices/backend/PATCHES.md')), 'Unrelated package content changed')
    ns2, cur2=inputs(src); need(ns2==native and cur2==current, 'An input changed during build')
    receipt=COMBINED/'build-receipt.json'
    proof={'schema_version':1,'passed':True,'source_base_commit':BASECOMMIT,'source_changes':diff,'source_fingerprint':digest(COMBINED/'source-inputs.sha256'),'native_input_count':len(native.splitlines()),'actual_compiler':compiler_id,'rustc_invocations':rust,'cargo_verbose_log':ident(raw),'cargo_jobs':2,'disk_space_floor_bytes':1300000000,'overlay_lower_read_only':True,'actual_selected_source_snapshot':str(src),'package':ident(z),'binary':ident(S/'bilicomics'),'worker':ident(S/'kindle-jpeg-worker'),'source_stable_across_build':True,'archive_delta':delta,'archive_members':len(members),'launcher_sha256':members['bilicomics/launch.sh'],'document_sha256':members['documents/bilicomics.sh'],'package_check':ident(COMBINED/'package-static.json'),'steps':records,'physical_device_accessed':False}
    new(receipt,proof)
    emit={'complete':True,'package':str(z),'package_sha256':digest(z),'binary_sha256':digest(S/'bilicomics'),'worker_sha256':digest(S/'kindle-jpeg-worker'),'launcher_sha256':proof['launcher_sha256'],'document_sha256':proof['document_sha256'],'source_fingerprint':proof['source_fingerprint'],'build_receipt_path':str(receipt),'build_receipt_sha256':digest(receipt),'build_configuration':{'profile':'release','features':['kindle'],'default_features':False,'target':NATIVE,'lto':'thin','codegen_units':1,'strip':True},'source_base_commit':BASECOMMIT,'source_changes':list(diff)}
    emit.update(complete=False, source_candidate_commit=CANDIDATE)
    new(COMBINED/'arm-candidate.json',emit); print('ARM stage complete; software verification is still required.',flush=True)


def verify():
    src=COMBINED/'app'; receipt=json.loads((COMBINED/'build-receipt.json').read_text()); manifest,current=inputs(src); need(hd(manifest.encode())==receipt['source_fingerprint'],'Invalid current test sources')
    expected_inputs=json.loads((WORK/'baseline/archive-inputs.json').read_bytes())
    expected_inputs.update({name: entry['now'] for name,entry in receipt['source_changes'].items()})
    need(current==expected_inputs,'Test inputs differ from the ARM build')
    cmd('launcher-shell-syntax',['sh','-n',src/'packaging/kindle/launch.sh'])
    cmd('watchdog-shell-syntax',['sh','-n',src/'scripts/verification/physical/physical-watchdog.sh'])
    cmd('launcher-fixtures',[sys.executable,'-B',src/'scripts/verification/launcher_fixtures.py','--tests',src/'packaging/tests/launcher_test.py'],env=dict(ENV,BILICOMICS_REMOTE_VERIFICATION='1'),lim=650)
    cmd('watchdog-cleanup-fixtures',[sys.executable,'-B','-m','unittest','-v','test_physical_harness.PhysicalWatchdogLockCleanupTests'],cwd=src/'scripts/verification',env=dict(ENV,BILICOMICS_REMOTE_VERIFICATION='1'),lim=180)
    cmd('clean-desktop',['cargo','clean','--locked','--offline','--profile','dev','-p','bilicomics'])
    cmd('test-lib-bin',['cargo','test','--locked','--offline','--lib','--bins'],lim=1800)
    cmd('build-desktop',['cargo','build','--locked','--offline','-p','bilicomics','--bin','bilicomics'],lim=1200)
    desk=TARGET/'debug/bilicomics'
    fixture=OLD/'startup-boundaries-r1.py'
    cmd('startup-fixtures',[sys.executable,'-B',fixture,'--helper',src/'scripts/verification/startup_frames.py','--binary',desk,'--output',COMBINED/'startup-frames','--expected-binary-sha256',digest(desk)],env=dict(ENV,SLINT_BACKEND='winit-software'),lim=260)
    cmd('launcher-real-reader',['xvfb-run','-a','-s','-screen 0 2200x3000x24',sys.executable,'-B',src/'scripts/verification/launcher_real_reader.py','--workspace',COMBINED,'--binary',desk,'--output',COMBINED/'lifecycle','--expected-binary-sha256',digest(desk)],env=dict(ENV,SLINT_BACKEND='winit-software'),lim=260)
    cmd('package-regressions',[sys.executable,'-B','-m','unittest','-v','test_startup_package','test_spatial_dither_package','test_native_media_package','test_library_entry_package'],cwd=src/'scripts/verification',lim=180)
    ns2,cur2=inputs(src); need(ns2==manifest and cur2==current, 'Test inputs changed')
    testlog=(LOGS/'test-lib-bin.log').read_text(); counts=[list(map(int,r)) for r in re.findall(r'test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored',testlog)]
    expected=('resolved_startup_screen_distinguishes_plain_shelf_from_welcome_and_resume','exhausted_budget_yields_before_shelf_navigation_but_not_before_its_views','non_shelf_navigation_keeps_the_budget_boundary_before_views','shelf_redirect_to_login_keeps_its_own_view_boundary','suspension_before_shelf_navigation_does_not_consume_the_commit','failed_shelf_projection_does_not_publish_ready_or_retry_navigation')
    need(all('::'+t+' ... ok' in testlog for t in expected),'An actual candidate Shelf regression did not pass')
    need(sum(c[0] for c in counts)>800 and not any(c[1] for c in counts),'Incomplete app Rust tests')
    fixture_counts={}
    for label, floor in [('launcher-fixtures',152),('watchdog-cleanup-fixtures',6)]:
        summary=(LOGS/(label+'.log')).read_text(); matched=re.search(r'^Ran (\d+) tests? in',summary,re.M)
        need(matched and int(matched[1])>=floor and re.search(r'^OK$',summary,re.M),'Incomplete fixture run: '+label)
        fixture_counts[label]=int(matched[1])
    software=COMBINED/'software.json'
    new(software, {'passed':True,'rust_suites_counts_passed_failed_ignored':counts,'fixture_counts':fixture_counts,'steps':records,'startup_fixture':ident(fixture),'app_source_fingerprint':receipt['source_fingerprint'],'desktop_sha256':digest(desk),'physical_device_accessed':False})
    emit=json.loads((COMBINED/'arm-candidate.json').read_bytes())
    need(emit['source_candidate_commit']==CANDIDATE and emit['build_receipt_sha256']==digest(COMBINED/'build-receipt.json') and digest(Path(emit['package']))==emit['package_sha256'], 'Selected ARM build no longer matches its receipt')
    emit.update(complete=True,software_receipt_sha256=digest(software))
    new(COMBINED/'candidate.json',emit); print('SOFTWARE VERIFIED',counts,flush=True)


if __name__=='__main__':
    need(sys.platform=='linux' and WORK.is_dir(), 'Use only designated remote workspace')
    need(OLD.is_absolute() and OLD.resolve()==OLD and OLDZIP.is_absolute() and OLDZIP.resolve()==OLDZIP, 'Set the retained absolute paths')
    need(not WORK.is_relative_to(OLD) and not OLD.is_relative_to(WORK), 'Use a distinct workspace outside retained builds')
    if sys.argv[1] in ('arm','verify'):
        need(os.readlink('/proc/self/ns/mnt')!=os.readlink('/proc/1/ns/mnt') and CWD.samefile(COMBINED/'app') and subprocess.check_output(['findmnt','-no','FSTYPE','--target',str(TARGET)]).strip()==b'overlay', 'Use box.sh in its private mount namespace')
    if sys.argv[1]=='extract-baseline': extract(False)
    elif sys.argv[1]=='extract-candidate': extract(True)
    elif sys.argv[1]=='base-tools': base_tools()
    elif sys.argv[1]=='before': before()
    elif sys.argv[1]=='arm': arm()
    elif sys.argv[1]=='verify': verify()
    else: raise SystemExit('Specify before, arm or verify')
