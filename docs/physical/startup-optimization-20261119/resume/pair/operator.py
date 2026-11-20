#!/usr/bin/env python3
"""One fixed, fail-closed, two-package native Library click comparison on test-env."""

import argparse
import hashlib
import importlib.util
import inspect
import io
import ipaddress
import json
import os
from pathlib import Path
import re
import secrets
import socket
import stat
import subprocess
import sys
import tarfile
import zipfile

sys.dont_write_bytecode = True
os.umask(0o077)
SHARED_LEASE = '/tmp/codex-kindle-physical-test.lock'
FIELDS = ('binary_sha256', 'worker_sha256', 'launcher_sha256', 'document_sha256')
ZIP_FIELDS = dict(zip(('bilicomics/app/bilicomics', 'bilicomics/app/kindle-jpeg-worker',
                       'bilicomics/launch.sh', 'documents/bilicomics.sh'), FIELDS))
BASE_ZIP_SHA = 'c57d5fc3674f1a0de448b85beb2552ee0959b83756c3e8f62afb559bf62a4882'
OLD_ADMISSION_SHA = 'ff79272b72dabd0fc46ee3a525bed11c15686ddaf9c005de0464dfd1076fb6de'
PLAN = [('candidate-a00', 'A', 0), ('candidate-b00', 'B', 0)]
for i in range(1, 7):
    PLAN.extend((f'candidate-{letter.lower()}{i:02}', letter, i) for letter in ('AB' if i % 2 else 'BA'))
CASES = [name for name, _, _ in PLAN]
ARM = {name: option for name, option, _ in PLAN}


def need(condition, why):
    if not condition:
        raise ValueError(why)


def required_env(name):
    value = os.environ.get(name)
    need(value and value.strip() == value, 'Missing or invalid explicit input: ' + name)
    return value


def canonical_dir(name):
    path = Path(required_env(name))
    need(path.is_absolute() and path.resolve(strict=True) == path and path.is_dir() and
         path != path.parent, 'Use an existing, canonical directory: ' + name)
    return path


def separate(a, b):
    return a != b and a not in b.parents and b not in a.parents


TEST_HOST = required_env('BILI_PAIR_TEST_HOST')
need(sys.platform == 'linux' and os.geteuid() == 0 and socket.gethostname() == TEST_HOST,
     'Run only on the explicitly identified Linux test-env host')
WORK = canonical_dir('BILI_PAIR_WORK')
OLD = canonical_dir('BILI_PAIR_HISTORY')
need(separate(WORK, OLD) and Path.cwd() == WORK and Path(__file__).resolve().parent == WORK and
     WORK.stat().st_uid == 0 and stat.S_IMODE(WORK.stat().st_mode) == 0o700,
     'Copy the sources into a separate root-owned mode-0700 work directory and run there')
OWNER = required_env('BILI_PAIR_OWNER')
need(re.fullmatch(r'[0-9a-f]{8}-(?:[0-9a-f]{4}-){3}[0-9a-f]{12}', OWNER) and
     required_env('CODEX_THREAD_ID') == OWNER, 'Forward the actual receiving conversation ID; no default owner')
DEVICE = required_env('BILI_PAIR_DEVICE')
need(str(ipaddress.IPv4Address(DEVICE)) == DEVICE, 'Supply the authorized Kindle IPv4 address')
PLUGIN = OLD / 'click-r2/plugin'


def sha(b):
    return hashlib.sha256(b).hexdigest()


def frozen(path, expected=None, limit=240 * 1024**2):
    path = Path(path)
    need(path.is_absolute() and path.resolve(strict=True) == path and path.is_file() and
         0 < path.stat().st_size <= limit, 'Unsafe or missing frozen input: ' + path.name)
    data = path.read_bytes()
    if expected is not None:
        need(re.fullmatch('[0-9a-f]{64}', expected) and sha(data) == expected, 'Changed input: ' + path.name)
    return data


def module(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    instance = importlib.util.module_from_spec(spec)
    sys.modules[name] = instance
    spec.loader.exec_module(instance)
    return instance


frozen(OLD / 'fresh.py', '945824e45cc11384051435b87dcedd13744e14e5b4300e44f3d40686c8e0fa6d')
frozen(OLD / 'fresh_click.py', '2f9a67d17b08b3c8928245c26e3891645c0bf410b898bc71ab480217a4b443d9')
history_admission = json.loads(frozen(OLD / 'click-r2/admission.json', OLD_ADMISSION_SHA))
need(isinstance(history_admission.get('owner'), str) and OWNER != history_admission['owner'],
     'Do not reuse the historical session owner for this handoff')
f = module('fresh', OLD/'fresh.py')
need(f.WORK == OLD and all(separate(WORK, path) for path in (f.OLD, f.R7, f.QUAL)),
     'Historical tools are not relocatable: keep their verified original paths separate from new work')
c = module('paired_click_previous', OLD/'fresh_click.py')
need(c.f is f and c.ROOT == OLD and Path(f.__file__).resolve() == OLD/'fresh.py',
     'Historical click adapter did not import the pinned source')
f.OWNER = OWNER
q = f.q
f.NAMES = CASES
spec_template = WORK / 'select-arms.in.sh'


def bound_modules():
    need(c.f is f and f.OWNER == OWNER, 'The receiving owner is not bound to the historical module')
    n, bounded, reg = f.modules()
    need(n.OWNER == OWNER and str(n.LEASE) == str(n.base.LEASE_DIRECTORY) == SHARED_LEASE and
         separate(WORK, n.INVENTORY) and separate(WORK, n.OLD), 'Wrong native owner, lease or host roots')
    template = json.loads(frozen(n.INVENTORY/'private/transport-template.json', limit=8192))
    need(set(template) == set(n.CONNECTION_KEYS) and template['host'] == DEVICE,
         'The reviewed private connection is not for the explicitly authorized device')
    return n, bounded, reg


def bound_transport(n, batch, conf):
    tx = json.loads(frozen(batch/'private/transport.json', limit=8192))
    template = json.loads(frozen(n.INVENTORY/'private/transport-template.json', limit=8192))
    need(set(template) == set(n.CONNECTION_KEYS) and all(tx.get(k) == v for k,v in template.items()) and
         tx['host'] == DEVICE and conf['owner_thread'] == tx['owner_thread'] == f.OWNER == n.OWNER == OWNER and
         conf['root'] == str(batch) and tx['lease_run_id'] == conf['run_id'] and
         re.fullmatch('lib-p-[0-9a-f]{8}', conf['run_id']) and tx['lease_dir'] == SHARED_LEASE and
         tx['lease_tool_directory'] == str(batch/'tools') and tx['lease_token_file'] == str(batch/'private/lease-token'),
         'The owner, device, private transport, exact shared lease, or run-specific paths differ')
    # Bind the reviewed SSH trust store too. Do not copy or print any connection material.
    known_hosts = sha(frozen(tx['known_hosts'], limit=65536))
    with Path('/proc/sys/kernel/random/boot_id').open(encoding='ascii') as stream:
        boot = stream.read(80).strip()
    need(re.fullmatch(r'[0-9a-f]{8}-(?:[0-9a-f]{4}-){3}[0-9a-f]{12}', boot), 'Test host boot identity unavailable')
    return sha(json.dumps([TEST_HOST, boot, DEVICE, known_hosts, str(WORK), str(OLD), OWNER, conf['run_id']],
                          separators=(',', ':')).encode())


def compared_case(n, route):
    # Keep every R7 click, abort and entry/worker identity boundary; capture the last pair on both arms.
    src = inspect.getsource(c.click_case)
    src = f.substitute(src, 'case_name == self.config[\'cases\'][-1]', 'case_name in self.config[\'cases\'][-2:]')
    env = dict(c.__dict__)
    exec(compile(src, 'paired-click-case', 'exec'), env)
    return env['click_case'](n, route)


f.get_case = compared_case


def package(path, expected_hash, baseline_members=None):
    body = frozen(path, expected_hash, 100*1024**2)
    with zipfile.ZipFile(io.BytesIO(body)) as z:
        names = z.namelist()
        need(len(names) == len(set(names)) == 49, 'A complete 49-member qualified formal ZIP is required')
        need(all(not v.startswith('/') and '..' not in Path(v).parts and '\\' not in v and
                 stat.S_IFMT(z.getinfo(v).external_attr >> 16) in (0, stat.S_IFREG) and
                 z.getinfo(v).file_size <= 125*1024**2 for v in names), 'Unsafe package members')
        need(baseline_members is None or set(names) == baseline_members, 'Candidate package schema differs')
        need(z.testzip() is None, 'Package entry CRC validation failed')
        elems = {key:z.read(key) for key in ZIP_FIELDS}
        for key in list(elems)[:2]:
            data = elems[key]
            need(data[:7] == b'\x7fELF\x01\x01\x01' and data[18:20] == b'(\0', 'Expected ARMv7 ELF')
        return elems,set(names)


def prepare(args, n):
    batch = args.batch
    need(batch.is_absolute() and batch.parent == WORK and batch.parent.resolve(strict=True) == WORK and
         not os.path.lexists(batch), 'Use a fresh private workspace under the paired root')
    config = json.loads(frozen(f.R7/'config.frozen.json', f.PINS[f.R7/'config.frozen.json']))
    need(config['package_sha256'] == BASE_ZIP_SHA, 'Wrong historical candidate')
    a, roster = package(config['package'], BASE_ZIP_SHA)
    metadata = {k:config[k] for k in ('package_sha256','source_fingerprint',*FIELDS)}
    need(all(sha(a[nm]) == metadata[k] for nm,k in ZIP_FIELDS.items()), 'Historical package member differs')
    metadata['build_receipt_sha256'] = config['provenance_inputs']['current_software_summary']['sha256']
    frozen(config['provenance_inputs']['current_software_summary']['path'],metadata['build_receipt_sha256'])
    metadata['product_source'] = '39fac822a982868078201e605e50bdacf1dcf3ed (runtime 530f1516c332931059c52161b52d64e86488b3f1)'
    if args.fixture:
        b, meta_b = a,dict(metadata)
        input_src = {'fixture':True}
    else:
        receipt = json.loads(frozen(args.candidate_receipt, args.candidate_receipt_sha))
        need(receipt.get('complete') is True, 'Candidate build receipt is not a successful final receipt')
        meta_b = {k:receipt[k] for k in ('package_sha256','source_fingerprint',*FIELDS)}
        need(all(re.fullmatch('[0-9a-f]{64}', v) for v in meta_b.values()), 'Invalid candidate pins')
        b,_ = package(receipt['package'], meta_b['package_sha256'], roster)
        need(all(sha(b[nm]) == meta_b[k] for nm,k in ZIP_FIELDS.items()), 'Candidate package differs from build receipt')
        need(meta_b['source_fingerprint'] != metadata['source_fingerprint'] or meta_b['launcher_sha256'] != metadata['launcher_sha256'], 'No candidate source change')
        need((meta_b['binary_sha256'],meta_b['launcher_sha256']) != (metadata['binary_sha256'],metadata['launcher_sha256']), 'No code or launcher difference')
        need(meta_b['document_sha256'] == metadata['document_sha256'], 'Both native documents must match')
        frozen(receipt['build_receipt_path'],receipt['build_receipt_sha256'])
        meta_b['build_receipt_sha256'] = receipt['build_receipt_sha256']
        meta_b['product_source'] = 'Candidate working-tree build, identified by source_fingerprint and build receipt'
        input_src = {'candidate_input_sha256':args.candidate_receipt_sha, 'candidate_receipt_path':str(args.candidate_receipt)}
    old_admit = json.loads(frozen(OLD/'click-r2/admission.json',OLD_ADMISSION_SHA))
    click_files = c.plugin_files(PLUGIN)
    need({k:sha(v) for k,v in click_files.items()} == old_admit['plugin_sha256'], 'Click detector or protocol changed')
    arms = {'A':metadata,'B':meta_b}
    cfg = WORK/('base-'+secrets.token_hex(4)+'.json')
    config['owner_thread'] = OWNER
    f.jwrite(cfg,config)
    src = inspect.getsource(n.prepare)
    src = f.substitute(src,"'lib-' + kind + '-20261117-' + secrets.token_hex(4)", "'lib-p-' + secrets.token_hex(4)")
    src = f.substitute(src, '    for name, value in members.items():', '    add_pair_assets(members, launcher, stage, runtime, run_id, cases)\n    for name, value in members.items():')
    src = f.substitute(src, "    json_new(root / 'private/config.json', frozen)","    add_pair_metadata(frozen)\n    json_new(root / 'private/config.json', frozen)")
    selector_template = frozen(spec_template,limit=24576).decode()
    pair_fs = frozen(WORK/'pair-fs', limit=4*1024**2)
    need(pair_fs[:7] == b'\x7fELF\x01\x01\x01' and pair_fs[18:20] == b'(\0', 'Expected the test-only ARM filesystem primitive')
    def add_assets(members, launcher_a, stage, runtime, run, cases):
        launcher_b=n.instrument_launcher(b['bilicomics/launch.sh'],stage,runtime,run,cases,'ab')
        arms['A']['instrumented_launcher_sha256'] = sha(launcher_a)
        arms['B']['instrumented_launcher_sha256'] = sha(launcher_b)
        selector = selector_template.replace('@RUN@',run)
        for key in arms:
            for label,field in (('L','instrumented_launcher_sha256'),('APP','binary_sha256'),('W','worker_sha256')):
                selector = selector.replace('@'+key+'_'+label+'@',arms[key][field])
        need(not re.search('@[AB]_|@RUN@',selector), 'Unresolved selector token')
        n.selector_sha = sha(selector.encode())
        members.update({'canonical/.bench-arm':b'A\n','park-B/.bench-arm':b'B\n','park-B/.native-library-owner':(run+'\n').encode(),
                        'park-B/app/bilicomics':b['bilicomics/app/bilicomics'],'park-B/app/kindle-jpeg-worker':b['bilicomics/app/kindle-jpeg-worker'],
                        'park-B/launch.sh':launcher_b,'tools/select-arms.sh':selector.encode(),'tools/pair-fs':pair_fs})
        members.update({'tools/click/'+k.split('/',1)[1]:v for k,v in click_files.items() if k.startswith('assets/')})
    def add_metadata(conf):
        conf.update(paired_arms=arms, selector_sha256=n.selector_sha, pair_fs_sha256=sha(pair_fs), dry_only=bool(args.fixture),
                    case_metadata={k:{'software_variant':v,'measurement_role':('warmup' if p==0 else 'primary'),'pair':p} for k,v,p in PLAN},
                    variants={k:{**v,'entry_arguments':['--library'],'startup_arguments':['--demo','--offline','--screen','shelf']} for k,v in arms.items()})
    n.add_pair_assets=add_assets; n.add_pair_metadata=add_metadata
    exec(compile(src,'paired-prepare','exec'),n.__dict__)
    n.prepare(cfg,batch,'ab')
    (batch/'plugin/assets').mkdir(mode=0o700,parents=True)
    for k,data in click_files.items(): n.new_file(batch/'plugin'/k,data,0o700)
    conf=json.loads((batch/'private/config.json').read_bytes())
    binding=bound_transport(n,batch,conf)
    paths=('inputs/payload.tar.gz','inputs/fixture-manifest.json','private/config.json','private/transport.json')
    manifest={'paired_arms':arms, 'schema_version':1,'run_id':conf['run_id'],'owner':OWNER,'batch':str(batch),'route':'click','schedule':PLAN,
              'input_provenance':input_src,'driver_sha256':sha(Path(__file__).read_bytes()),'selector_template_sha256':sha(selector_template.encode()),
              'source_pins':{str(k):v for k,v in f.PINS.items()},'frozen_files':{k:sha((batch/k).read_bytes()) for k in paths},
              'plugin_sha256':old_admit['plugin_sha256'],'dry_only':bool(args.fixture),'detector_sha256':c.KIT,
              'execution_binding_sha256':binding}
    f.jwrite(batch/'pair-admission.json',manifest)
    return sha((batch/'pair-admission.json').read_bytes())


def host_or_run(args,n,bounded,reg):
    batch=args.batch; need(batch.parent==WORK and batch.resolve(strict=True)==batch,'Wrong batch location')
    manifest=json.loads(frozen(batch/'pair-admission.json',args.admission_sha))
    need(manifest['driver_sha256']==sha(Path(__file__).read_bytes()) and manifest['selector_template_sha256']==sha(frozen(spec_template)) and
         manifest['source_pins']=={str(k):v for k,v in f.PINS.items()} and manifest['owner']==OWNER and manifest['batch']==str(batch) and
         [tuple(r) for r in manifest['schedule']]==PLAN,'Unbound host inputs')
    for k,v in manifest['frozen_files'].items(): frozen(batch/k,v)
    conf=json.loads((batch/'private/config.json').read_bytes()); need(conf['cases']==CASES and conf['paired_arms']==manifest['paired_arms'] and
        conf['dry_only']==manifest['dry_only'] and re.fullmatch('lib-p-[0-9a-f]{8}',conf['run_id']) and conf['run_id']==manifest['run_id'],'Wrong paired schedule')
    need(bound_transport(n,batch,conf)==manifest['execution_binding_sha256'], 'This admission does not bind the current host, owner and authorized device')
    need(not (batch/'report.json').exists() and not (batch/'private/lease-token').exists(),'Consumed batch identity, prepare a new run')
    need(args.command=='check' or not manifest['dry_only'],'Host fixtures may never access the device')
    need({k:sha(v) for k,v in c.plugin_files(batch/'plugin').items()}==manifest['plugin_sha256'],'Unbound input protocol')
    plugin=module('paired_click_plugin',batch/'plugin/plugin.py')
    def sink(record):
        if args.command=='run':
            with (batch/'private/guard-events.jsonl').open('a') as target:
                target.write(json.dumps({k:v for k,v in record.items() if k!='snapshot'},sort_keys=True)+'\n')
    parent=c.click_class(n,bounded,reg,manifest,plugin,sink)
    class Pair(parent):
        def __init__(self, path):
            super().__init__(path)
            need(self.owner==OWNER and self.run_id==conf['run_id'] and self.root==batch and
                 self.transport==batch/'private/transport.json', 'The inherited lease controller lost its current binding')
            self.record.update(scope='Matched native Library card clicks on two specified instrumented product packages. Same offline Shelf and full-screen marker; no optical claim.',
                paired_arms=self.config['paired_arms'],first_in_batch=CASES[:2],repeated_launches=CASES[2:],primary_rounds=6,
                post_business_observation={'capture_after_content_seconds':2,'capture_cases':CASES[-2:], 'scope':'Same final pair, after primary endpoints'})

        def select_arm(self,arm,phase,name):
            need(arm in ('A','B') and phase in ('select','restore','probe'),'Invalid arm')
            script=self.guard()+n.native_functions()+'native_ready\n'+f'''test ! -e /tmp/bilicomics-launcher.lock && test ! -L /tmp/bilicomics-launcher.lock
test "$(save_native | sort)" = {q(self.native_ids.decode().rstrip())}
f={q(self.stage+'/tools/select-arms.sh')}
test -f "$f" && test ! -L "$f" && test "$(sha256sum "$f" | cut -d ' ' -f 1)" = {q(self.config['selector_sha256'])}
h={q(self.stage+'/tools/pair-fs')}
test -f "$h" && test ! -L "$h" && test "$(sha256sum "$h" | cut -d ' ' -f 1)" = {q(self.config['pair_fs_sha256'])}
sh "$f" {arm} {phase}
'''
            result=self.command(script,name,timeout=60)
            vals=self.config['paired_arms'][arm]; fields=('instrumented_launcher_sha256','binary_sha256','worker_sha256')
            need(result==(' '.join([arm,*[vals[k] for k in fields]])+'\n').encode(),'Unverified actual arm selection')
            for k in fields: self.config[k]=vals[k]

        def prepare_device(self):
            self.command('test -d /mnt/us && test ! -L /mnt/us\n'
                         +'''test "$(lipc-get-prop com.lab126.appmgrd activeApp)" = com.lab126.KPPMainApp
test "$(lipc-get-prop -s com.lab126.powerd state)" = active
for lock in /tmp/bilicomics-launcher.lock /tmp/aidoku-launcher.lock /tmp/weread-launcher.lock; do test ! -e "$lock" && test ! -L "$lock"; done
space=$(df -Pk /mnt/us | awk 'END {print $4}')
case "$space" in ''|*[!0-9]*) exit 1;; esac
test "$space" -ge 512000
''','workspace-space-before')
            return super().prepare_device()

        def install_document(self, name):
            if name==CASES[0]: self.select_arm('A','probe','private-fs-capability-before-case')
            if ARM[name]=='B': self.b_selection_requested=True
            self.select_arm(ARM[name],'select','arm-'+name)
            return super().install_document(name)

        def finish_unopened_install(self):
            need(not self.document_installed and not self.active_case and not self.record['completed_cases'] and
                 self.record['sidecar_candidates_initially_absent']==list(n.SIDECARS),'Unexpected early installation failure')
            # Partial setup may not yet have both owner roots.
            checked=n.native_functions()+'native_ready\n'+f'''test "$(save_native | sort)" = {q(self.native_ids.decode().rstrip())}
test ! -e /tmp/bilicomics-launcher.lock && test ! -L /tmp/bilicomics-launcher.lock
stage={q(self.stage)}; runtime={q(self.runtime)}; run={q(self.run_id)}
for path in {q(n.DOCUMENT)} {q(n.UPPER_DOCUMENT)} {' '.join(map(q,n.SIDECARS))}; do test ! -e "$path" && test ! -L "$path"; done
for dir in "$stage" "$runtime"; do
 if test -e "$dir" || test -L "$dir"; then
  test -d "$dir" && test ! -L "$dir" && test "$(readlink -f "$dir")" = "$dir"
  test -f "$dir/owner" && test ! -L "$dir/owner" && test "$(cat "$dir/owner")" = "$run"
 fi
done
test ! -e "$runtime/dispatch" && test ! -L "$runtime/dispatch"
test ! -e "$runtime/dispatch.pending" && test ! -L "$runtime/dispatch.pending"
for case_name in {' '.join(CASES)}; do
 for leaf in "$stage/out/$case_name"/* "$stage/out/$case_name"/.[!.]* "$stage/out/$case_name"/..?*; do
  test -e "$leaf" || test -L "$leaf" || continue
  test "${{leaf##*/}}" = owner && test -f "$leaf" && test ! -L "$leaf"
 done
 if test -e "$runtime/$case_name"; then
  test -d "$runtime/$case_name" && test ! -L "$runtime/$case_name" && test -z "$(ls -A "$runtime/$case_name")"
 fi
done
home={q(n.CANONICAL)}
if test -e "$home" || test -L "$home"; then
 test -d "$home" && test ! -L "$home" && test "$(readlink -f "$home")" = "$home"
 test -f "$home/.native-library-owner" && test ! -L "$home/.native-library-owner" && test "$(cat "$home/.native-library-owner")" = "$run"
 test -f "$home/.bench-arm" && test ! -L "$home/.bench-arm" && test "$(cat "$home/.bench-arm")" = A
'''
            for name,key in [('launch.sh','instrumented_launcher_sha256'),('app/bilicomics','binary_sha256'),('app/kindle-jpeg-worker','worker_sha256')]:
                checked+=f'''test -f "$home/{name}" && test ! -L "$home/{name}" && test "$(sha256sum "$home/{name}" | cut -d ' ' -f 1)" = {q(self.config['paired_arms']['A'][key])}\n'''
            checked+='fi\n'
            self.command(checked,'unopened-owned-install-context',timeout=50)
            self.catalog('unopened-catalog-confirmed-empty',empty=True)
            self.registration('unopened-registration-confirmed')
            self.compare_protected('before-unopened-cleanup')
            # Repeat every namespace and identity gate immediately before removing any initially absent owned path.
            self.command(checked+'for dir in "$home" "$stage" "$runtime"; do if test -d "$dir"; then rm -rf -- "$dir"; fi; test ! -e "$dir" && test ! -L "$dir"; done\n',
                         'remove-unopened-owned-install',timeout=60)
            self.restore_lighting('unopened-restore-lighting')
            need(self.command('cat /proc/sys/kernel/random/boot_id','unopened-boot-final')==self.boot and
                 self.input_roster('unopened-input-final')==self.roster,'Original boot or input changed')
            self.command(n.native_functions()+'native_ready\ntest "$(save_native | sort)" = '+q(self.native_ids.decode().rstrip()),'unopened-native-final')
            self.compare_protected('after-unopened-cleanup')
            self.record.update(unopened_partial_install_recovered=True,native_restored=True,lighting_restored=True,
                catalog_restored=True,registration_unchanged=True,temporary_stage_removed=True,install_unchanged=True,
                same_boot=True,input_roster_unchanged=True)

        def finish_device(self):
            self.abort_active()
            if self.device_writes_started and not self.canonical_installed:
                return self.finish_unopened_install()
            if self.canonical_installed and getattr(self,'b_selection_requested',False):
                self.select_arm('A','restore','restore-original-arm-for-cleanup')
            return super().finish_device()
    Pair.case=parent.case
    widths=f.dry_cases(n,bounded,Pair,batch)
    need(len(widths)==14 and max(widths)<8192,'Unsafe SSH bounds')
    # Exercise new arm switching commands and interruption-before-first-case rollback only against a host stub.
    specimen=Pair(batch); specimen.native_ids=b'x'*128; specimen.boot=b'boot'; specimen.roster=b'roster'
    specimen.record['sidecar_candidates_initially_absent']=list(n.SIDECARS)
    checks=[]
    def fake(script,name,data=None,timeout=30,required=True):
        text=bounded.framed_command(specimen.marker,script);need(len(text)<8192,'Synthetic recovery command exceeds SSH limit: '+name)
        subprocess.run(['sh','-n'],input=script.encode(),check=True);checks.append((name,len(text)))
        if name.startswith('dry-'):
            which=name.split('-')[1]; pin=conf['paired_arms'][which]
            return (' '.join([which,pin['instrumented_launcher_sha256'],pin['binary_sha256'],pin['worker_sha256']])+'\n').encode()
        if name=='unopened-boot-final': return specimen.boot
        return b''
    specimen.command=fake
    specimen.catalog=lambda *a,**kw:[];specimen.registration=lambda *a:None;specimen.compare_protected=lambda *a:None
    specimen.restore_lighting=lambda *a:None;specimen.input_roster=lambda *a:specimen.roster
    for arm,phase in [('A','probe'),('A','select'),('B','select'),('A','restore')]:
        specimen.select_arm(arm,phase,'dry-'+arm+'-'+phase)
        need(all(specimen.config[k]==conf['paired_arms'][arm][k] for k in ('instrumented_launcher_sha256','binary_sha256','worker_sha256')),'Wrong selected cleanup identity')
    specimen.finish_unopened_install()
    need(len(checks)==8 and specimen.record['unopened_partial_install_recovered'],'Incomplete independent simulation')
    if args.command=='check':
        print(json.dumps({'constructed':len(widths),'arm_and_recovery_commands':len(checks),'max_bytes':max(max(widths),max(w for _,w in checks)),'lease_acquired':False,'device_accessed':False}));return
    obj=Pair(batch)
    with n.base.Cancellation() as cancellation:
        obj.run(cancellation)


def main():
    args=argparse.ArgumentParser(description=__doc__);commands=args.add_subparsers(dest='command',required=True)
    p=commands.add_parser('prepare');p.add_argument('--batch',type=Path,required=True);p.add_argument('--fixture',action='store_true');p.add_argument('--candidate-receipt',type=Path);p.add_argument('--candidate-receipt-sha')
    for mode in ('check','run'):
        p=commands.add_parser(mode);p.add_argument('--batch',type=Path,required=True);p.add_argument('--admission-sha',required=True)
    args=args.parse_args(); n,bounded,reg=bound_modules()
    if args.command=='prepare':
        need(args.fixture or args.candidate_receipt and args.candidate_receipt_sha,'Candidate build admission is required')
        digest=prepare(args,n)
        p=argparse.Namespace(batch=args.batch,admission_sha=digest,command='check')
        n,bounded,reg=bound_modules(); host_or_run(p,n,bounded,reg)
        print(json.dumps({'prepared':str(args.batch),'admission_sha256':digest,'device_accessed':False,'dry_only':args.fixture}));return
    host_or_run(args,n,bounded,reg)


if __name__=='__main__': main()
