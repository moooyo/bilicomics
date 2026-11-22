"""Qualify non-cover Shelf and menu pixels on independent ordinary, offline Scribe profiles.

Run once in the private runner mount after the stock executable, closed R seed,
manifest and ordinary empty-control capture exist. Stage the reviewed Scribe
02-shelf-filter.png from the 2026-11-14-v0.1.25 UI baseline as
/work/visual-qualified/frozen-menu.png. The per-case Xvfb servers run inside an
isolated network namespace. Only the aggregate receipt is public.
"""

import filecmp
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import sys
import time

from PIL import Image, ImageChops

import interactive_runner as runner
from desktop_pairs import FULL, SIZE, Sampler, now, region, require, require_private_network, require_retired
from interactive_runner import FIRST_TITLE, MENU_RESPONSE, POINT, SHELF_BAR, XTestInput, ordinary


ROOT = Path('/work')
WORK = ROOT / 'visual-qualified'
R = ROOT / 'r-seed'
PENDING = WORK / 'refs'
MENU_VIEWS = (1040, 356, 716, 130)
CLOSE = (440, 140)
REGIONS = {'shelf-bar': SHELF_BAR, 'first-title': FIRST_TITLE, 'closed-response': MENU_RESPONSE,
           'menu-response': MENU_RESPONSE, 'menu-views': MENU_VIEWS}
CASES = ('reference-no-click', 'cached-click', 'missing-click', 'signed-out-no-click')


def load(path):
    return json.loads(path.read_text(encoding='utf-8'))


def save(path, data):
    with path.open('x', encoding='utf-8') as output:
        json.dump(data, output, indent=2)
        output.write('\n')


def image(path):
    with Image.open(path) as source:
        return source.convert('RGB')


def nonblank(pixels):
    histogram = pixels.convert('L').histogram()
    return sum(histogram[:150]) > 50 and sum(histogram[190:]) > 50


def templates():
    return {key: image(PENDING / (key + '.png')).tobytes() for key in REGIONS}


def prepare_reference(frame):
    reviewed = image(WORK / 'frozen-menu.png')
    require(reviewed.size == frame.size == SIZE, 'Wrong Scribe geometry')
    PENDING.mkdir(mode=0o700)
    for key, (left, top, width, height) in REGIONS.items():
        menu = key.startswith('menu-')
        pixels = region(reviewed if menu else frame, (left, top + (674 if menu else 0), width, height))
        require(nonblank(pixels), f'Blank reference: {key}')
        pixels.save(PENDING / (key + '.png'))
    masks = templates()
    require(masks['closed-response'] != masks['menu-response'], 'Menu and ordinary captions are indistinguishable')


def checked_data(directory, name):
    directory.mkdir(mode=0o700)
    data = directory / 'data'
    ordinary.private_copy(R, data, True)
    files = [Path(row['file']) for row in load(ordinary.MANIFEST)['covers']]
    require(len(files) == len(set(files)) == 20 and all(item.parent == Path('covers') and item.suffix == '.png' for item in files), 'Wrong fixture cover list')
    if name == 'missing-click':
        for item in files:
            path = data / item
            require(path.resolve(strict=True).parent == data.resolve() / 'covers' and not path.is_symlink() and path.stat().st_nlink == 1, 'Cover outside the owned copy')
            path.unlink()
        require(not list((data / 'covers').iterdir()), 'Cached cover survived the missing-cover control')
    if name == 'signed-out-no-click':
        session = data / ordinary.SESSION
        require(not session.is_symlink() and session.is_file() and session.stat().st_nlink == 1, 'Expected one owned synthetic session')
        session.unlink()
    require(filecmp.cmp(R / 'state.sqlite3', data / 'state.sqlite3', shallow=False), 'Changed sibling database before launch')
    with sqlite3.connect(f'{(data / "state.sqlite3").as_uri()}?mode=ro&immutable=1', uri=True) as db:
        rows = db.execute("SELECT c.comic_id,c.payload FROM catalog_members m JOIN comics c ON c.account_key=m.account_key AND c.comic_id=m.comic_id WHERE m.scope='shelf' AND m.account_key='bili_424242' ORDER BY m.position").fetchall()
    expected = [(str(710000 + index), f'Archive Study {index:02d}') for index in range(1, 21)]
    require([(key, json.loads(payload)['title']) for key, payload in rows] == expected, 'Unexpected synthetic title list or account')


def snapshot(sampler, pointer, xid):
    _, window = sampler.get(xid, FULL)
    _, root = sampler.get(pointer.root, FULL)
    frame = Image.frombytes('RGB', SIZE, window)
    root_image = Image.frombytes('RGB', SIZE, root)
    require(ImageChops.difference(frame.crop((0, 64, *SIZE)), root_image.crop((0, 64, *SIZE))).getbbox() is None, 'Foreign foreground or changing content')
    return frame


def visible(sampler, pointer, xid, key, masks):
    result, root = sampler.get(xid, REGIONS[key]), sampler.get(pointer.root, REGIONS[key])
    return result[1] == root[1] == masks[key]


def wait_for(child, predicate, reason):
    limit = now() + 8_000_000_000
    while child.poll() is None and now() < limit:
        value = predicate()
        if value:
            return value
        time.sleep(.015)
    raise ValueError(reason)


def dwell(child, predicate):
    start = now()
    reads = 0
    while True:
        require(child.poll() is None and predicate(), 'Expected state changed without input')
        reads += 1
        if now() - start >= 450_000_000 and reads >= 3:
            return reads
        time.sleep(.03)


def click(child, pointer, xid, point):
    pointer.move(point)
    focus = pointer.locate(xid, point)
    require(ordinary.ordinary_window(child.pid) == xid, 'Switch of owned window before click')
    pointer.down()
    require(pointer.button_down(), 'Primary press not observed by X11')
    time.sleep(.016)
    pointer.up()
    require(not pointer.button_down(), 'Primary release not observed by X11')
    return focus


def placeholder_patches(frame):
    patches = []
    for index in range(10):
        row, col = divmod(index, 5)
        center = (2 * (48 + 171 * col + 75), 2 * (32 + 72 + 4 + 292 * row + 100))
        patch = region(frame, (center[0] - 112, center[1] - 25, 224, 50))
        require(nonblank(patch) and patch.getpixel((0, 0)) == (221, 221, 221), 'No centered label on the expected gray placeholder')
        patches.append(patch.tobytes())
    require(all(patch == patches[0] for patch in patches), 'The ten missing-cover label panels differ')


def run_case(name):
    directory = WORK / name
    checked_data(directory, name)
    require_private_network()
    sampler = Sampler()
    try:
        pointer = XTestInput(sampler)
        pointer.move(POINT)
        with runner.process(directory, None, exit_after_ms=6500) as (child, _):
            xid = wait_for(child, lambda: ordinary.ordinary_window(child.pid), 'No owned window')
            def shelf_ready_for_reference():
                path = directory / 'lifecycle.jsonl'
                return path.exists() and any(row.get('event') == 'ready' and row.get('process_id') == child.pid for row in ordinary.records(path))
            if name in ('reference-no-click', 'signed-out-no-click'):
                wait_for(child, shelf_ready_for_reference, 'Missing lifecycle in static reference setup')
                time.sleep(.30)
            if name == 'reference-no-click':
                frame = snapshot(sampler, pointer, xid)
                require(ordinary.marker_codes(frame) == list(range(1, 11)), 'Wrong populated ordinary reference')
                prepare_reference(frame)
            masks = templates()
            def closed():
                return all(visible(sampler, pointer, xid, key, masks) for key in ('shelf-bar', 'first-title', 'closed-response')) and not visible(sampler, pointer, xid, 'menu-response', masks)
            def opened():
                return all(visible(sampler, pointer, xid, key, masks) for key in ('menu-response', 'menu-views'))
            taps = []
            if name == 'signed-out-no-click':
                frame = snapshot(sampler, pointer, xid)
                prompt = (320, 740, 1220, 960)
                expected_login = region(image(ROOT / 'bootstrap/final.png'), prompt)
                require(nonblank(expected_login) and region(frame, prompt).tobytes() == expected_login.tobytes(), 'Wrong ordinary login negative control')
                reads = dwell(child, lambda: not (visible(sampler, pointer, xid, 'shelf-bar', masks) and visible(sampler, pointer, xid, 'first-title', masks)) and not opened())
            else:
                wait_for(child, closed, 'No populated non-cover Shelf signature')
                pointer.locate(xid, POINT)
                reads = dwell(child, closed)
                frame = snapshot(sampler, pointer, xid)
                if name in ('cached-click', 'missing-click'):
                    taps.append(click(child, pointer, xid, POINT))
                    wait_for(child, opened, 'Normal X11 click did not open the ordinary menu')
                    time.sleep(.1)
                    require(opened(), 'Menu did not remain open after release')
                    taps.append(click(child, pointer, xid, CLOSE))
                    wait_for(child, closed, 'Outside click did not restore the populated Shelf')
                    require(ordinary.ordinary_window(child.pid) == xid, 'The return used another window')
            codes = ordinary.marker_codes(frame)
            expected_codes = list(range(1, 11)) if name in ('cached-click', 'reference-no-click') else [None] * 10
            require(codes == expected_codes and not pointer.button_down(), 'Unexpected cover codes or held button')
            if name == 'missing-click':
                placeholder_patches(frame)
        final = load(directory / 'final.json')
        require(final['process_id'] == child.pid and final['backend'] == 'desktop-software' and final['profile'] == 'scribe' and final['route'] == 'shelf' and not final['demo'] and final['offline_requested'] and final['offline'] and not final['startup']['error'] and not final['startup']['booting'], 'Wrong final ordinary Shelf')
        catalog = final['ui']['catalog']
        require(not catalog['shelf_menu'] and catalog['shelf_view'] == 'grid' and (name == 'signed-out-no-click' or (catalog['shelf_page'], catalog['shelf_pages']) == (1, 2)), 'Wrong final Shelf controls')
        ready = [row for row in ordinary.records(directory / 'lifecycle.jsonl') if row.get('event') == 'ready' and row['process_id'] == child.pid]
        require(len(ready) == 1 and ready[0]['route'] == 'shelf', 'Wrong same-process lifecycle')
        resident = ready[0]['reader_resources']['cover_resident_count']
        require(resident == (0 if name == 'missing-click' else 10), 'Unexpected resident image count')
        logical = image(directory / 'final.png')
        require(ordinary.marker_codes(logical) == expected_codes, 'Final cover state changed')
        if name == 'missing-click':
            placeholder_patches(logical)
            require(not list((directory / 'data/covers').glob('*.png')), 'Offline image cache unexpectedly repopulated')
        save(directory / 'receipt.json', {'case': name, 'same_owned_window': True, 'normal_exit': True, 'signed_in': name != 'signed-out-no-click',
             'catalog_titles_unchanged_before_launch': 20, 'ready_resident_covers': resident, 'visible_cover_codes': codes,
             'identical_labeled_placeholders': 10 if name == 'missing-click' else None, 'closed_populated_shelf_matched': name != 'signed-out-no-click',
             'empty_control_login_prompt_matched': name == 'signed-out-no-click', 'withheld_floor_ms': 450, 'withheld_observations': reads,
             'open_response_observed_while_withheld': False, 'button_pairs': len(taps), 'focus_at_click': taps,
             'owned_window_and_root_menu_response_after_click': True if taps else None, 'opened_menu_still_visible_100ms_later': True if taps else None,
             'closed_after_outside_click': True if taps else None, 'final_shelf_menu_open': catalog['shelf_menu']})
    finally:
        sampler.close()


def main():
    os.umask(0o077)
    if sys.argv[1:]:
        require(len(sys.argv) == 2 and sys.argv[1] in CASES, 'Unknown qualification case')
        run_case(sys.argv[1])
        return
    require(WORK.is_dir() and not PENDING.exists() and not runner.REFS.exists(), 'Refusing to overwrite qualification evidence')
    results = []
    for case in CASES:
        with (WORK / (case + '.log')).open('xb') as log:
            status = subprocess.run([*ordinary.XVFB, sys.executable, '-B', __file__, case], stdout=log, stderr=subprocess.STDOUT, timeout=40)
        require_retired()
        require(status.returncode == 0, f'Qualification stopped at {case}; see its private case log')
        results.append(load(WORK / case / 'receipt.json'))
    result = {'scope': 'Independent non-timing stock desktop Scribe with synthetic ordinary offline data. The missing-image case is not delayed decoding, network latency or device qualification.',
              'reference': 'The closed signatures come from a verified populated ordinary R launch. Open ShelfMenu rows also match the reviewed 2026-11-14 Scribe filter capture after correcting only the 337 logical pixel resume-card offset.',
              'geometry': {'size': SIZE, 'regions_xywh': REGIONS, 'open_point': POINT, 'qualification_only_close_point': CLOSE},
              'cases': results, 'isolated_xvfb_and_processes_retired': True, 'physical_device_used': False}
    save(PENDING / 'qualified.json', result)
    PENDING.rename(runner.REFS)


if __name__ == '__main__':
    main()
