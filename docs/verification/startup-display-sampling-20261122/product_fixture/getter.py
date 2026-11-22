#!/usr/bin/python3 -IB
import json
import os
from pathlib import Path
import sys
import time


def process(pid):
    tail = Path(f'/proc/{pid}/stat').read_text().rsplit(') ', 1)[1].split()
    return dict(pid=pid, ppid=int(tail[1]), group=int(tail[2]), session=int(tail[3]), birth=int(tail[19]))


commands = {('-s', 'com.lab126.powerd', 'state'): ('power', 'active'),
            ('-s', 'com.lab126.winmgr', 'epdcMode'): ('mode', 'Y8'),
            ('com.lab126.winmgr', 'accelerometer'): ('holding', 'U')}
name, output = commands[tuple(sys.argv[1:])]
case = Path(os.environ['DISPLAY_FIXTURE_CASE'])
helper = process(os.getppid())
leader = process(helper['group'])
master = process(leader['ppid'])
exe = Path(os.readlink(f'/proc/{helper["pid"]}/exe'))
if not (exe == Path(os.environ['BILICOMICS_HOME'])/'app/bilicomics' and helper['group'] == helper['session'] == leader['pid'] == leader['session']):
    raise SystemExit(93)
with (case/'getters').open('a') as evidence:
    evidence.write(json.dumps({'name': name, 'getter': process(os.getpid()), 'helper': helper, 'leader': leader, 'master': master})+'\n')
if os.environ['DISPLAY_FIXTURE_MODE'] in ('revoke', 'timeout'):
    with (case/'getter-started').open('x') as opened:
        opened.write('started\n')
    time.sleep(4)
    with (case/'delayed-getter-finished').open('x') as finished:
        finished.write('returned\n')
print(output)
