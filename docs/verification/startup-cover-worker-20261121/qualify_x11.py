"""Qualify the XImage visual and RGB projection using one untimed control launch."""

import ctypes
import json
import os
from pathlib import Path
import re
import subprocess
import time

from PIL import Image, ImageChops, ImageGrab

from desktop_pairs import FULL, ROOT, SIZE, Sampler, require, rgb, wait_child, window_id


def get_layout(sampler, window, box, planes):
    capture = sampler.api.XGetImage(sampler.display, window, *box, planes, 2)
    require(capture, 'XGetImage qualification failed')
    try:
        data = capture.contents
        layout = {key: getattr(data, key) for key in ('width', 'height', 'format', 'depth', 'bits_per_pixel', 'byte_order',
                                                       'bytes_per_line', 'red_mask', 'green_mask', 'blue_mask')}
        payload = ctypes.string_at(data.data, data.bytes_per_line * data.height)
        return layout, payload
    finally:
        sampler.api.XDestroyImage(capture)


def main():
    directory = ROOT / 'format-diagnostic'
    directory.mkdir()
    sampler = Sampler()
    env = dict(os.environ, SLINT_BACKEND='winit-software', WINIT_UNIX_BACKEND='x11', SLINT_SCALE_FACTOR='2')
    with (directory / 'application.log').open('xb') as log:
        process = subprocess.Popen([ROOT / 'control', '--demo', '--offline', '--screen', 'shelf', '--device', 'scribe',
                                    '--data-dir', directory / 'data', '--lifecycle-trace', directory / 'life.jsonl',
                                    '--capture', directory / 'final.png', '--exit-after-ms', '3500'], env=env, stdout=log, stderr=subprocess.STDOUT)
        try:
            deadline = time.monotonic() + 10
            while time.monotonic() < deadline:
                window = window_id(process.pid)
                if window:
                    break
                time.sleep(.005)
            require(window, 'No mapped owned window to qualify')
            info = subprocess.check_output(['xwininfo', '-id', hex(window)], text=True)
            lines = [line.strip() for line in info.splitlines() if any(key in line for key in ('Visual:', 'Visual Class:', 'Depth:', 'Map State:'))]
            left, top = (int(re.search(key + r':\s*(-?\d+)', info)[1]) for key in ('Absolute upper-left X', 'Absolute upper-left Y'))
            initial, _ = get_layout(sampler, window, (490, 940, 880, 240), 0xffffff)
            while time.monotonic() < deadline:
                path = directory / 'life.jsonl'
                if path.exists() and any(json.loads(line).get('event') == 'ready' for line in path.read_bytes().splitlines(keepends=True) if line.endswith(b'\n')):
                    break
                time.sleep(.01)
            else:
                raise ValueError('No real ready record to qualify')
            time.sleep(.3)
            sample, masked = get_layout(sampler, window, FULL, 0xffffff)
            complete, full_planes = get_layout(sampler, window, FULL, 0xffffffff)
            require(sample == complete, 'Plane masks changed the visual layout')
            (directory / 'actual-layout.json').write_text(json.dumps({'title': initial, 'final': sample, 'window': lines}, indent=2) + '\n')
            require((sample['width'], sample['height'], sample['bytes_per_line'], sample['bits_per_pixel'], sample['byte_order'],
                     sample['red_mask'], sample['green_mask'], sample['blue_mask']) == (*SIZE, 4 * SIZE[0], 32, 0, 0xff0000, 0xff00, 0xff), 'Unknown RGB format; inspect actual-layout.json')
            expected = rgb(ROOT / 'references/final-x11.png')
            for name, image in [('only color planes', Image.frombytes('RGB', SIZE, masked, 'raw', 'BGRX')),
                                ('all planes', Image.frombytes('RGB', SIZE, full_planes, 'raw', 'BGRX')),
                                ('visible root crop', ImageGrab.grab(bbox=(left, top, left + SIZE[0], top + SIZE[1]), xdisplay=os.environ['DISPLAY']).convert('RGB'))]:
                require(ImageChops.difference(image, expected).getbbox() is None, f'{name} does not match the complete reference')
            require(process.wait(12) == 0, 'Control qualification did not exit normally')
        finally:
            sampler.close()
            wait_child(process)
    require(ImageChops.difference(rgb(directory / 'final.png'), rgb(ROOT / 'references/final.png')).getbbox() is None, 'Final logical reference mismatch')
    final = json.loads((directory / 'final.json').read_text())
    require(final['process_id'] == process.pid and final['route'] == 'shelf' and final['demo'] and final['offline'], 'Wrong final control')
    result = {'scope': 'Untimed A-only format qualification, not a warmup or paired sample.', 'pid': process.pid, 'xid': window,
              'window': lines, 'first_mapped_title_layout': initial, 'complete_window_layout': sample,
              'fourth_byte_masked_range': [min(masked[3::4]), max(masked[3::4])], 'fourth_byte_all_planes_range': [min(full_planes[3::4]), max(full_planes[3::4])],
              'both_plane_masks_match_frozen_RGB': True, 'same_window_root_crop_matches': True, 'logical_final_matches': True, 'normal_exit': True}
    (directory / 'qualification.json').write_text(json.dumps(result, indent=2) + '\n')
    print(json.dumps(result), flush=True)


if __name__ == '__main__':
    os.umask(0o077)
    main()
