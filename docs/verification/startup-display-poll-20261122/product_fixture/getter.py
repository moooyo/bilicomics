#!/usr/bin/python3 -IB
"""Control hardware response timing without intercepting the application's poll or wait."""

import os
from pathlib import Path
import runpy
import sys
import time


delayed = (os.environ['DISPLAY_FIXTURE_MODE'] in ('selected', 'selector-off')
           and sys.argv[1:] == ['-s', 'com.lab126.winmgr', 'epdcMode'])
if delayed:
    time.sleep(0.045)
runpy.run_path('/work/inputs/stock-getter.py', run_name='__main__')
if delayed:
    sys.stdout.flush()
    os.close(1)
    time.sleep(0.075)
    with (Path(os.environ['DISPLAY_FIXTURE_CASE']) / 'mode-getter-finished').open('x') as marker:
        marker.write('returned after closing stdout\n')
    os._exit(0)
