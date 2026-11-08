#!/usr/bin/env python3
"""Capture the minimal PW and SC startup activity protection checks."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import time
from PIL import Image, ImageChops

parser = argparse.ArgumentParser()
parser.add_argument('--binary', type=Path, required=True)
parser.add_argument('--output', type=Path, required=True)
parser.add_argument('--expected-binary-sha256', required=True)
args = parser.parse_args()
selected_digest = hashlib.sha256(args.binary.read_bytes()).hexdigest()
if selected_digest != args.expected_binary_sha256.lower():
    raise SystemExit('The binary hash differs from the coordinator-selected snapshot.')
args.output.mkdir(parents=True, exist_ok=False)
records = []

def check(condition, message):
    if not condition:
        raise AssertionError(message)

def execute(name, device, fail=False):
    output = args.output / name
    output.mkdir()
    data = output / 'data'
    if fail:
        data.write_text('Controlled non-directory startup input.\n')
    command = ['xvfb-run','-a','-s','-screen 0 2200x3000x24',str(args.binary.resolve()),'--demo','--device',device,'--screen','reader','--data-dir',str(data.resolve()),'--startup-delay-ms','2000','--capture-startup',str((output / 'startup.png').resolve()),'--capture',str((output / 'ready.png').resolve()),'--exit-after-ms','2200']
    started = time.monotonic()
    with (output / 'stdout.log').open('w') as stdout, (output / 'stderr.log').open('w') as stderr:
        result = subprocess.run(command,stdout=stdout,stderr=stderr,timeout=25)
    frames = [json.loads((output / f'startup-{index}.json').read_text()) for index in range(2)]
    final = json.loads((output / 'ready.json').read_text())
    images = [Image.open(output / f'startup-{index}.png').convert('RGB') for index in range(2)]
    check(frames[0]['worker_started'] is False,'The first real frame was captured after worker admission')
    check(frames[0]['booting'] is True and frames[1]['booting'] is True,'Startup frames did not precede the application')
    check(frames[0]['animation_frame'] != frames[1]['animation_frame'],'The startup animation never advanced')
    check(frames[0]['process_id'] == frames[1]['process_id'] == final['process_id'],'Startup and application capture used different processes')
    check(frames[1]['worker_started'] is True,'The second activity frame did not overlap background preparation')
    expected_size = {'pw5': (618,824), 'scribe': (930,1240)}[device]
    check(all(image.size == expected_size for image in images),'The startup frames did not use the requested profile geometry')
    check(final['profile'] == device,'The final application used a different profile')
    width,height = images[0].size
    top = max(24,height / 2 - 150)
    box = (int((width-120)/2),int(top+212),int((width+120)/2),int(top+224))
    check(ImageChops.difference(images[0].crop(box),images[1].crop(box)).getbbox() is not None,'The indicator metadata changed without rendered pixel changes')
    if fail:
        check(result.returncode != 0,'Controlled preparation failure was reported as success')
        check(final['startup']['error'] is True and final['startup']['booting'] is True,'Startup failure did not retain the real error view')
        final_image = Image.open(output / 'ready.png').convert('RGB')
        check(all(value == (255,255,255) for value in final_image.crop(box).getdata()),'The loading indicator remained in the error view')
    else:
        check(result.returncode == 0,'The real startup/application process failed')
        check(final['startup']['booting'] is False and final['startup']['error'] is False,'The real startup never attached the application')
        check(final['route'] == 'reader' and final['reader']['loading'] is False,'Reader content did not become ready after startup')
        check(final['reader']['comic_title'] == '\u5c71\u6d77\u90ae\u5dee','The final Chinese reader model is incomplete')
    record = {'name':name,'profile':device,'expected_failure':fail,'exit_code':result.returncode,'seconds':round(time.monotonic()-started,3),'first_frame':frames[0],'second_frame':frames[1],'final':final,'indicator_pixels_changed':True,'passed':True}
    (output / 'result.json').write_text(json.dumps(record,indent=2,ensure_ascii=False)+'\n')
    records.append(record)
    print(json.dumps(record,ensure_ascii=False),flush=True)

try:
    execute('pw5-normal','pw5')
    execute('scribe-normal','scribe')
    report = {'binary_sha256':hashlib.sha256(args.binary.read_bytes()).hexdigest(),'passed':len(records),'total':2,'harness_sha256':hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),'coverage':'Real Rust desktop startup frames on isolated PW and SC Xvfb displays','limitations':['No live account, firmware emulator, native panel, or physical Kindle result is established'],'cases':records}
    (args.output / 'results.json').write_text(json.dumps(report,indent=2,ensure_ascii=False)+'\n')
except Exception as error:
    (args.output / 'failure.json').write_text(json.dumps({'error':str(error),'completed':records},indent=2,ensure_ascii=False)+'\n')
    raise
