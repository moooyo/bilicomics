#!/usr/bin/env python3
"""Render actual demo application states for v2 comparison; never claim pixel acceptance automatically."""
import argparse
import concurrent.futures
import hashlib
import json
import os
from pathlib import Path
import subprocess
import time

parser=argparse.ArgumentParser()
parser.add_argument('--plan',type=Path,required=True)
parser.add_argument('--batch',required=True)
parser.add_argument('--binary',type=Path,required=True)
parser.add_argument('--output-root',type=Path,required=True)
parser.add_argument('--expected-sha256',required=True)
parser.add_argument('--optional',action='append',default=[])
parser.add_argument('--workers',type=int,choices=(1,2,3),default=1)
args=parser.parse_args()
plan=json.loads(args.plan.read_text())
cases=plan if isinstance(plan,list) else plan['cases']
binary=args.binary.resolve()
expected=hashlib.sha256(binary.read_bytes()).hexdigest()
if args.expected_sha256: assert expected==args.expected_sha256
output_root=args.output_root.resolve()
output_root.mkdir(parents=True,exist_ok=False)

def value_at(value,key):
    for part in key.split('.'):
        value=value.get(part) if isinstance(value,dict) else None
    return value

def records_at(path):
    if not path.exists(): return []
    content=path.read_text()
    return [json.loads(line) for line in content.splitlines() if line.strip()]

def render(case):
    name=case['name']
    status=case.get('status','planned')
    if case.get('capture') is False or status=='baseline_gap' or case.get('actions') is None:
        return {'name':name,'status':'baseline_gap','rendered':False,'gaps':case.get('gaps',[])}
    if (case.get('optional') or status in {'optional_mode_variant','related_optional'}) and name not in args.optional:
        return {'name':name,'status':'optional_not_requested','rendered':False,'gaps':case.get('gaps',[])}
    output=output_root/name
    output.mkdir(exist_ok=False)
    actions=case.get('actions',[])
    (output/'actions.json').write_text(json.dumps(actions,ensure_ascii=False))
    (output/'plan.json').write_text(json.dumps(case,indent=2,ensure_ascii=False)+'\n')
    command=['xvfb-run','-a','-s','-screen 0 2200x3000x24',str(binary),'--demo','--device',case['profile'],'--screen',case['screen'],'--data-dir',str(output/'data'),'--capture',str(output/'window.png'),'--lifecycle-trace',str(output/'lifecycle.jsonl'),'--exit-after-ms',str(case.get('exit_after_ms',2500))]
    if case.get('landscape'): command.append('--landscape')
    if actions: command += ['--actions',str(output/'actions.json')]
    env=os.environ.copy()
    for key in ['BILICOMICS_LIFECYCLE_DIR','BILICOMICS_LAUNCH_TOKEN','BILICOMICS_LAUNCH_GENERATION']:
        env.pop(key,None)
    started=time.monotonic()
    with (output/'stdout.log').open('w') as stdout,(output/'stderr.log').open('w') as stderr:
        result=subprocess.run(command,stdout=stdout,stderr=stderr,env=env,timeout=45)
    metadata=json.loads((output/'window.json').read_text()) if (output/'window.json').exists() else {}
    trace=records_at(output/'lifecycle.jsonl')
    checks={key:{'expected':expected_value,'actual':value_at(metadata,key),'passed':value_at(metadata,key)==expected_value} for key,expected_value in case.get('expected_metadata',{}).items()}
    ready=case.get('readiness',{})
    if isinstance(ready,str): ready={'event':ready}
    event=ready.get('event','ready')
    matched=[]
    for item in trace:
        if item.get('event')!=event or item.get('process_id')!=metadata.get('process_id'): continue
        if not all(value_at(item,key)==expected_value for key,expected_value in ready.get('expected_fields',{}).items()): continue
        if not all(item.get('reader_state',{}).get(key)==expected_value for key,expected_value in ready.get('reader_state',{}).items()): continue
        dimensions=item.get(ready.get('positive_dimensions',''),None)
        if ready.get('positive_dimensions') and not (dimensions and all(value>0 for value in dimensions)): continue
        matched.append(item)
    passed=result.returncode==0 and metadata.get('demo') is True and not metadata.get('startup',{}).get('booting',True) and bool(matched) and all(check['passed'] for check in checks.values())
    report={'name':name,'status':status,'rendered':True,'capture_passed':passed,'exit_code':result.returncode,'seconds':round(time.monotonic()-started,3),'binary_sha256':expected,'metadata':metadata,'assertions':checks,'ready_observation':matched[-1] if matched else None,'gaps':case.get('gaps',[]),'purpose':case.get('purpose','Targeted baseline scene'),'source_refs':case.get('source_refs',[])}
    if not passed: report['stderr']=(output/'stderr.log').read_text()[-5000:]
    (output/'result.json').write_text(json.dumps(report,indent=2,ensure_ascii=False)+'\n')
    print(json.dumps({'name':name,'capture_passed':passed,'status':status,'seconds':report['seconds'],'route':metadata.get('route'),'png':str(output/'window.png')}),flush=True)
    return report

results=[]
with concurrent.futures.ThreadPoolExecutor(max_workers=args.workers) as pool:
    for future in concurrent.futures.as_completed([pool.submit(render,case) for case in cases]):
        results.append(future.result())
report={'batch':args.batch,'workers':args.workers,'binary_sha256':expected,'rendered':sum(item['rendered'] for item in results),'capture_failures':sum(item.get('rendered',False) and not item.get('capture_passed',False) for item in results),'design_acceptance_claimed':False,'results':results}
(output_root/f'{args.batch}-results.json').write_text(json.dumps(report,indent=2,ensure_ascii=False)+'\n')
print(json.dumps({key:report[key] for key in ['batch','rendered','capture_failures','design_acceptance_claimed']}),flush=True)
raise SystemExit(1 if report['capture_failures'] else 0)
