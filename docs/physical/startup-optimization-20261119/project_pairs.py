#!/usr/bin/env python3
"""Read a closed, admitted 14-launch native Library comparison. No device or network access.

Usage: python3 -B project_pairs.py BATCH --admission-sha256 HASH --candidate-package-sha256 HASH
Both pins must come from pre-run admission, not from inference from measured timings.
"""

import argparse
import hashlib
import json
import re
import statistics
import sys
import tarfile
from pathlib import Path


sys.dont_write_bytecode = True
READERS = Path(__file__).resolve().parents[1] / 'library-to-shelf-20261119'
PINS = {'project_fresh.py': '79e3e1c7cec240f0072dd3495a9ee5f7dbae2529180858f3f107efb0f6cb173d',
        'project_archived.py': '827ab9da6c980fe7617f9744eeccd8c5e778aef41b19d06b91e0091a9b428115',
        'project_launcher.py': '78ab08320a0521069a6ad393e3844f1192e4dc259e3aa24cbc6d24c960c17fac'}
for _name, _pin in PINS.items():
    if hashlib.sha256((READERS / _name).read_bytes()).hexdigest() != _pin:
        raise ValueError('Stored evidence reader changed: ' + _name)
sys.path.insert(0, str(READERS))
from project_archived import bounds, ms, one, require
from project_fresh import DOC, LOGICAL_SHELF_SHA, RELEASED, RESTORED, Sources, all_true, case_row, check_capture
from project_launcher import uptime_ns


BASE_PINS = {'package_sha256': 'c57d5fc3674f1a0de448b85beb2552ee0959b83756c3e8f62afb559bf62a4882',
             'binary_sha256': '04fc6f93b4327911ca1a3bb5200175c9f2e13fb21ac5e56280012ff44b6e2e59',
             'launcher_sha256': '584ad6065565bf5529fd4ef17393a454a0db811824c685e104a360c82080dea4',
             'worker_sha256': '12c8988c6a93cade9923d54bef784b774e13cb5e3b042f2e72ea5ecb6b897beb'}
ARMS = ('A', 'B')
PAIRS = 6
CASES = [(('candidate-' + arm.lower() + f'{i:02}'), arm, i) for i in range(PAIRS+1)
         for arm in (ARMS if i == 0 or i % 2 else tuple(reversed(ARMS)))]
DIGEST = re.compile(r'[0-9a-f]{64}')
PROVEN_SHELF_RGB = 'b235922f95c7e6ef'
PROVEN_SPLASH_RGB = 'fe0d23ce1e398933'
DOCUMENT_SHA = '7ac07e2a4e2b790642ff88787deaf05988e1bb8786d418b45bfd092d64d023f4'


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def command_index(src, report, name, contents=None):
    idx, row = one(((i, r) for i, r in enumerate(report['commands']) if r['name'] == name), 'named command ' + name)
    require(row['returncode'] == 0 and row['interrupted'] is False and row['timeout'] is False, 'Unsuccessful guard: ' + name)
    if contents is not None:
        require(src.read('evidence/' + name + '.stdout', 512) == contents, 'Wrong guard identity: ' + name)
    return idx


def check_arms(report, admission, candidate_pin):
    arms = report['paired_arms']
    require(set(arms) == set(ARMS) and arms == admission['paired_arms'], 'Arm definitions disagree with admission')
    fields = (*BASE_PINS, 'instrumented_launcher_sha256', 'source_fingerprint', 'build_receipt_sha256', 'document_sha256')
    result = {}
    for arm, rec in arms.items():
        result[arm] = {k: rec[k] for k in fields}
        require(all(isinstance(v, str) and DIGEST.fullmatch(v) for v in result[arm].values()), 'Invalid arm provenance hash')
        variant = report['variants'][arm]
        require(all(variant[k] == rec[k] for k in fields) and rec['document_sha256'] == DOCUMENT_SHA and
                variant['entry_arguments'] == ['--library'] and variant['startup_arguments'] == ['--demo', '--offline', '--screen', 'shelf'],
                'Wrong formal document, launch mode, fixture, or runtime identity')
    require(all(result['A'][k] == v for k, v in BASE_PINS.items()), 'Control differs from frozen production reference')
    require(result['B']['package_sha256'] == candidate_pin and candidate_pin != BASE_PINS['package_sha256'], 'Candidate ZIP identity differs')
    require(any(result['B'][k] != result['A'][k] for k in ('binary_sha256', 'launcher_sha256')), 'No changed application or launcher')
    return result


def diagnostic_frames(records, identity, ready, shelf_send, shelf_marker):
    require(all(r['process_id'] == identity[0] for r in records), 'Mixed native trace process')
    ctx = one((r for r in records if r['kind']=='startup' and r['stage']=='state-prepare-context'), 'fixture context')
    all_true(ctx['data'], ('demo','offline'))
    stage_rows = [r for r in records if r['kind']=='startup']
    stamps = []
    for name,phase in (('attach-route','end'), ('attach-views','begin')):
        a = one((r for r in stage_rows if r['stage']==name and r.get('data',{}).get('phase')==phase), name+' '+phase)
        stamps.append(a['monotonic_ns'])
    attachment_end, views_begin = stamps
    require(attachment_end <= views_begin <= ready, 'Invalid attach ordering')
    frames=[]
    for r in records:
        if (r['kind'],r['stage']) != ('native','frame'): continue
        f=r['data']
        if f['rendered'] is not True or f['refresh_accepted'] is not True: continue
        require(f['rgb_fingerprint_algorithm']=='fnv1a64-rgb8-row-major' and re.fullmatch('[0-9a-f]{16}',f['rgb_fingerprint']) and
                f['refresh_errno'] is None and f['refresh_ioctl_returned'] is True, 'Missing comparable diagnostic frame')
        start, _ = bounds(f['stages']['slint_render'])
        _, end = bounds(f['stages']['refresh_submit'])
        if end <= shelf_send: frames.append((end,start,r['monotonic_ns'],f))
    frames.sort(key=lambda t: t[0]); end,start,_,shelf=frames[-1]
    require(end==shelf_send and start>=ready and shelf['accepted_update_marker']==shelf_marker and
            shelf['completed_update_marker']==shelf_marker and shelf['rgb_fingerprint']==PROVEN_SHELF_RGB,
            'First Shelf SEND differs from the accepted fixture image')
    require(all(frames[i][3]['render_sequence'] < frames[i+1][3]['render_sequence'] for i in range(len(frames)-1)), 'Disordered render sequence')
    boot = [f for submitted, _, _, f in frames if submitted < ready]
    require(boot and boot[0]['initial_frame'] is True and boot[0]['rgb_fingerprint'] == PROVEN_SPLASH_RGB,
            'First startup scene is not the reference loading frame')
    duplicate=[]; route_duplicates=[]
    for prior,cur in zip(frames,frames[1:]):
        end,start,recorded,f=cur
        if start>=ready or f['rgb_fingerprint']!=prior[3]['rgb_fingerprint'] or f['complete_frame_replacement'] is not True: continue
        if f['refresh_flashing'] is not True or f['refresh_waveform']!='gc16': continue
        require(f['driver_completion_observed'] is True and f['accepted_update_marker']==f['completed_update_marker'] and f['completion_errno'] is None,
                'Unproven duplicate completion')
        duplicate.append(f['render_sequence'])
        if start>=attachment_end and recorded<=views_begin:
            route_duplicates.append(f['render_sequence'])
    return {'accepted_startup_frames_before_shelf': len(frames)-1, 'accepted_splash_frames_before_ready': len(boot),
            'distinct_loading_fingerprints_before_ready': len({f['rgb_fingerprint'] for f in boot}),
            'duplicate_pre_ready_full_gc16': len(duplicate), 'duplicate_in_route_views_gap': len(route_duplicates),
            'route_end_to_views_begin_ms': ms(views_begin-attachment_end), 'shelf_render_sequence': shelf['render_sequence']}


def launcher_sample(src, arm, arms):
    tsv = src.read('production-launcher-timing.tsv', 65_536).decode('ascii').splitlines()
    marks = [(uptime_ns(row[0]),row[1]) for row in (ln.split('\t') for ln in tsv) if len(row)>1]
    require(all(b[0]>=a[0] for a,b in zip(marks,marks[1:])), 'Decreasing launcher clocks')
    shell = [ln.split() for ln in src.read('shell.tsv',32_768).decode('ascii').splitlines()]
    worker = uptime_ns(one((r[0] for r in shell if len(r)==2 and r[1]=='admission_worker'),'worker mark'))
    begin = one((t for t,k in marks if k=='startup_sample_begin'),'startup sample begin')
    take = one((t for t,k in marks if k=='native_take_done'),'display takeover')
    require(worker<=begin<=take, 'Invalid launcher ordering')
    # Neither a finished logger nor a shorter overlap is proof of earlier native readiness.
    logger = [t for t,k in marks if k == 'startup_logging_ready']
    require(len(logger) <= 1 and (not logger or worker <= logger[0] <= begin), 'Invalid startup logging marker')
    return {'worker_to_first_sample_ms': ms(begin-worker), 'worker_to_native_take_done_ms': ms(take-worker),
            'logging_ready_to_first_sample_ms': ms(begin-logger[0]) if logger else None,
            'original_launcher_bytes': arms[arm]['launcher_sha256']==BASE_PINS['launcher_sha256']}


def statistics_ns(values):
    return {k: ms(f(values)) for k,f in (('min',min),('median',statistics.median),('max',max))}


def timing_comparisons(measures):
    result={}
    for field in measures[(1,'A')]:
        av=[measures[(i, 'A')][field] for i in range(1, PAIRS + 1)]
        bv=[measures[(i, 'B')][field] for i in range(1, PAIRS + 1)]
        dv=[b-a for a,b in zip(av,bv)]; ma,mb=statistics.median(av),statistics.median(bv)
        result[field]={'A_ms':statistics_ns(av),'B_ms':statistics_ns(bv),'paired_B_minus_A_ms':statistics_ns(dv),
                       'each_pair_B_minus_A_ms':[ms(d) for d in dv],
                       'B_faster_pairs':sum(d<0 for d in dv),'B_slower_pairs':sum(d>0 for d in dv),'ties':sum(d==0 for d in dv),
                       'marginal_B_minus_A_ms':ms(mb-ma),'marginal_change_percent':round((mb-ma)/ma*100,3)}
    return result


def verify_archive(batch, expected, sources):
    path=batch/'outputs.tar.gz'
    require(path.is_file() and path.resolve(strict=True)==path and path.stat().st_size<=256*1024*1024, 'Invalid case archive')
    with path.open('rb') as raw:
        digest=hashlib.sha256()
        for block in iter(lambda: raw.read(262144), b''): digest.update(block)
        require(digest.hexdigest()==expected,'Case archive hash differs')
        raw.seek(0); found=set()
        with tarfile.open(fileobj=raw,mode='r:gz') as archive:
            for entry in archive:
                if entry.name not in sources: continue
                require(entry.isreg() and entry.name not in found and 0 < entry.size <= 8_388_608,'Bad selected case member')
                with archive.extractfile(entry) as v:
                    require(sha256(v.read(entry.size+1))==sources[entry.name], 'Different archived source: '+entry.name)
                found.add(entry.name)
        require(set(sources)==found, 'Missing selected case files')
    return {'sha256':expected,'selected_regular_members_verified':len(found)}


def project(batch, admission_pin, candidate_pin):
    src=Sources(batch); report=src.json('report.json'); run=report['run_id']
    require(re.fullmatch(r'lib-p-[0-9a-f]{8}',run) and report.get('finished_at_utc') and report.get('cancelled') is False,
            'Missing closed paired run')
    all_true(report, ('success','original_native_tab_restored',*RESTORED))
    release=src.json('evidence/lease-released.json')
    require(release['run_id']==run and release['operation']=='release' and release['state']=='released', 'Wrong closed device lease')
    all_true(release,RELEASED)
    names=[case for case,_,_ in CASES]
    require(report['route']=='click' and report['native_uri']=='file://'+DOC and report['cases']==names==report['completed_cases']
            and report['first_in_batch']==names[:2] and report['primary_rounds']==PAIRS and report['repeated_launches']==names[2:]
            and report['post_business_observation']['capture_cases']==names[-2:], 'Plan was not completed exactly')
    admission=src.json('pair-admission.json')
    require(src.hashes['pair-admission.json']==admission_pin and admission['run_id']==run and admission['route']=='click'
            and admission['dry_only'] is False and admission['schedule']==[list(v) for v in CASES], 'Wrong pinned paired admission')
    fixture=sha256(src.read('inputs/fixture-manifest.json', 1_048_576))
    require(fixture==admission['frozen_files']['inputs/fixture-manifest.json']
            and DIGEST.fullmatch(admission['plugin_sha256']['assets/dispatch.sh'])
            and DIGEST.fullmatch(admission['input_provenance']['candidate_input_sha256']), 'Missing fixture, click, or build pins')
    arms=check_arms(report,admission,candidate_pin)
    restore_pos=command_index(src, report, 'click-restore-recovery', b'native-tab-and-pointer-restored\n')
    measures={}; cases=[]; current_end=0; last_command=-1; seen=set(); members={}
    for name,which,num in CASES:
        arm=arms[which]
        require(report['case_metadata'][name] == {'software_variant': which, 'measurement_role': 'warmup' if num==0 else 'primary', 'pair': num},
                'Wrong arm or measurement role')
        expected=(f"{which} {arm['instrumented_launcher_sha256']} {arm['binary_sha256']} {arm['worker_sha256']}\n").encode()
        idx=command_index(src,report,'arm-'+name,expected)
        doc=command_index(src,report,'document-identity-'+name)
        guard=command_index(src,report,'click-prepare-'+name,b'library-and-expected-bili-card-confirmed\n')
        active=command_index(src,report,'case-'+name)
        require(last_command<idx<doc<guard<active<restore_pos,'Missing per-case execution order'); last_command=active
        cs=Sources(src.path/'outputs/out'/name)
        row,groups,identity,(origin,finish)=case_row(cs,report,name)
        require(identity not in seen and origin>current_end,'Overlapping case or reused application'); seen.add(identity); current_end=finish
        value=cs.read('final.png',8_388_608); require(sha256(value)==LOGICAL_SHELF_SHA, 'Logical final differs from complete demo Shelf')
        point=groups['xtest_up_before_to']; click=cs.json('click.json'); up=click['up_before_ns']; send=up+point['shelf_send']; ready=up+point['ui_ready']
        frame=diagnostic_frames(cs.records('performance.jsonl'),identity,ready,send,row['shelf_accepted_and_completed_marker'])
        launch=launcher_sample(cs,which,arms)
        compact=dict(point, **{k:groups['application_intervals'][k] for k in ('process_enter_to_ui_ready','ui_ready_to_shelf_send','shelf_send_to_driver_complete')})
        compact['xsync_up_to_shelf_send']=groups['xtest_up_after_to']['shelf_send']; compact['xsync_up_to_shelf_driver_complete']=groups['xtest_up_after_to']['shelf_driver_complete']
        compact['down_before_to_shelf_send']=groups['xtest_down_before_to']['shelf_send']; compact['down_before_to_shelf_driver_complete']=groups['xtest_down_before_to']['shelf_driver_complete']
        measures[(num,which)]=compact
        res={'case':name,'arm':which,'pair':None if num==0 else num,'marker':row['shelf_accepted_and_completed_marker'],
             'ms':{k:ms(v) for k,v in compact.items()},'input_brackets_ms':{k:ms(v) for k,v in groups['input_brackets'].items()},
             'startup_frames':frame,'launcher':launch}
        if num==PAIRS:
            cap=check_capture(cs,finish); require(cap['matches_qualified_offline_shelf'] is True,'Actual framebuffer differs from complete demo Shelf')
            res['later_framebuffer']=cap
        res['case_source_sha256']=dict(cs.hashes); cases.append(res)
        for f,d in cs.hashes.items(): members['out/'+name+'/'+f]=d
    receipts=verify_archive(src.path, report['outputs_sha256'],members)
    a=arms['A']
    restored=f"A {a['instrumented_launcher_sha256']} {a['binary_sha256']} {a['worker_sha256']}\n".encode()
    require(last_command < command_index(src, report, 'restore-original-arm-for-cleanup', restored) < restore_pos,
            'Missing restoration of original selected program before native cleanup')
    return {'schema_version':1,'run_id':run,'arms':arms,'paired_cases':PAIRS,'first_two_warmups_included_in_statistics':False,
            'source_archive':receipts,'source_sha256':src.hashes,'frozen_reader_sha256':PINS,'native_tab_and_pointer_restored':True,'exact_lease_released':True,
            'candidate_input_sha256':admission['input_provenance']['candidate_input_sha256'],
            'fixture_manifest_sha256':fixture, 'click_dispatch_sha256':admission['plugin_sha256']['assets/dispatch.sh'],
            'qualification':{'distinct_process_birth_pairs':len(seen),'all_first_shelf_rgb_fingerprint':PROVEN_SHELF_RGB,
                             'all_initial_app_splash_fingerprint':PROVEN_SPLASH_RGB,
                             'first_frame_comparison':'Initial and first Shelf images compare 64-bit trace fingerprints; final PNG and two separately bracketed framebuffer captures compare complete SHA-256 identities. These are different endpoints.',
                             'all_final_logical_shelf_png_sha256':LOGICAL_SHELF_SHA,'both_last_arm_captures_match_old_qualified_shelf':True},
            'timing':'Device XTest on verified named card in the native Kindle Library. Default origin is immediately before sending button-up; XSync-confirmed-up and button-down retained separately. Software ready, accepted full Shelf SEND and driver wait for the same positive marker, not optical settling or physical finger input.',
            'statistics':'Milliseconds rounded after calculation from raw monotonic nanoseconds. Six contiguous balanced pairs. Delta B minus A is negative when the candidate was faster, positive when slower. Differences of marginal medians and medians of paired differences are distinct. No outlier exclusions, replacement runs, or automatic adoption verdict.',
            'scope':'One boot on one Scribe, new processes with retained caches, offline demo Shelf, full diagnostic profiling. No live-account, reboot, interaction-ready, optical or product-wide claim.',
            'warmups':cases[:2],'paired_launches':cases[2:],'comparisons':timing_comparisons(measures),
            'duplicate_gc16_cases_by_arm':{arm:sum(v['arm']==arm and v['startup_frames']['duplicate_in_route_views_gap']>0 for v in cases[2:]) for arm in ARMS}}


if __name__=='__main__':
    parser=argparse.ArgumentParser(description=__doc__); parser.add_argument('batch',type=Path)
    parser.add_argument('--admission-sha256',required=True); parser.add_argument('--candidate-package-sha256',required=True)
    options=parser.parse_args()
    print(json.dumps(project(options.batch,options.admission_sha256,options.candidate_package_sha256),indent=2))
