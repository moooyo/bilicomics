"""Check ARM user-mode startup gates and externally presented desktop Shelf regions."""

import argparse
import ast
from collections import Counter
import json
import os
from pathlib import Path
import re
import socket
import sqlite3
import stat
import subprocess
import sys
import time

from PIL import Image


ROOT = Path('/work/emulator-surface')
PROFILES = {
    'scribe': ((1860, 2480), {'resume_title': (604, 200, 520, 66), 'filter_control': (1652, 754, 112, 112), 'active_shelf_navigation': (160, 2330, 148, 127)}),
    'pw6': ((1264, 1680), {'resume_title': (260, 82, 610, 66), 'filter_control': (1072, 68, 144, 96), 'active_shelf_navigation': (95, 1558, 126, 109)}),
}


def require(condition, message):
    if not condition:
        raise ValueError(message)


def store(path, payload):
    with path.open('x') as output:
        output.write(json.dumps(payload, indent=2) + '\n')


def elf(path, arch):
    require(path.is_absolute() and path.is_file(), 'Supply a completed, same-run application executable')
    with path.open('rb') as program:
        header = program.read(20)
    expected = (1, 40) if arch == 'arm' else (2, 62)
    require(header[:4] == b'\x7fELF' and header[5] == 1 and (header[4], int.from_bytes(header[18:20], 'little')) == expected, 'Unexpected ELF target')


def arm_case(profile, executable, directory):
    elf(executable, 'arm')
    require(not Path('/dev/fb0').exists() and not Path('/dev/input').exists(), 'Not the isolated user-mode runtime')
    arguments = ['qemu-arm', executable, '--device', profile]
    version = subprocess.run([*arguments, '--version'], capture_output=True, text=True, timeout=30, check=True)
    require(re.fullmatch(r'bilicomics \d+\.\d+\.\d+\s*', version.stdout), 'Unexpected ARM product version')
    launched = subprocess.run([*arguments, '--kindle', '--offline', '--data-dir', directory / 'data', '--lifecycle-trace', directory / 'lifecycle.jsonl'],
                              capture_output=True, text=True, timeout=30)
    require(launched.returncode != 0 and 'Native output requires an owned launcher mode snapshot' in launched.stderr, 'ARM native startup did not reject missing launcher ownership')
    stages = [json.loads(line) for line in (directory / 'lifecycle.jsonl').read_text().splitlines()]
    require(any(row.get('stage') == 'process_enter' or row.get('event') == 'process_enter' for row in stages), 'Missing actual ARM process-entry record')
    require(not any(row.get('event') == 'ready' for row in stages), 'Unexpected native presentation on the isolated user-mode host')
    return {'version': version.stdout.strip(), 'qemu': subprocess.check_output(['qemu-arm', '--version'], text=True).splitlines()[0], 'native_launch_exit': launched.returncode,
            'process_entry_recorded': True, 'expected_missing_native_launcher_guard': True, 'native_framebuffer_or_input_available': False, 'shelf_rendered': False}


def mapped_window(pid, size):
    tree = subprocess.check_output(['xwininfo', '-root', '-tree'], text=True)
    windows = re.findall(r'(0x[0-9a-fA-F]+) "BiliComics - Design Preview"', tree)
    require(len(windows) <= 1, 'Multiple application windows on a private display')
    if not windows:
        return None
    details = subprocess.check_output(['xwininfo', '-id', windows[0]], text=True)
    if 'Map State: IsViewable' not in details:
        return None
    geometry = tuple(int(re.search(key + r':\s*(\d+)', details)[1]) for key in ('Width', 'Height'))
    if geometry != size:
        return None
    identity = subprocess.check_output(['xprop', '-id', windows[0], '_NET_WM_PID'], text=True)
    require(re.search(rf'= {pid}\s*$', identity), 'Window PID does not match the candidate')
    return int(windows[0], 16)


def reference(profile, suffix, size, boxes):
    with Image.open(ROOT / f'{profile}-{suffix}.png') as image:
        require(image.size == size, 'Wrong reference geometry')
        frame = image.convert('RGB')
    return {name: frame.crop((left, top, left + width, top + height)).tobytes() for name, (left, top, width, height) in boxes.items()}


def route_seed(directory):
    package = directory / 'route-stage'
    demo = package / 'data/demo'
    demo.mkdir(mode=0o700, parents=True)
    tree = ast.parse((ROOT / 'prepare_route.py').read_bytes())
    functions = {item.name: item for item in tree.body if isinstance(item, ast.FunctionDef)}
    body = functions['assemble'].body
    blocks = [position for position, item in enumerate(body) if isinstance(item, ast.Assign) and [getattr(target, 'id', '') for target in item.targets] == ['seed']]
    require(len(blocks) == 1, 'Expected exactly one route seed definition')
    seed = body[blocks[0]:]
    require([type(item) for item in seed] == [ast.Assign, ast.With, ast.Expr], 'The route seed recipe changed; inspect it before running')
    bindings = {'output': package, 'SOURCE': Path('/work/app/docs/verification/startup-interactive-kindle-20261121/route'), 'Path': Path, 'sqlite3': sqlite3, 'stat': stat, 'json': json, 're': re}
    selected = [functions[name] for name in ('require', 'regular', 'demo_roster')] + seed
    exec(compile(ast.Module(body=selected, type_ignores=[]), '<reviewed route data seed only>', 'exec'), bindings)
    require(bindings['seed'] == demo / 'state.sqlite3', 'Unexpected route seed location')
    with sqlite3.connect(f'file:{demo / "state.sqlite3"}?mode=ro', uri=True) as db:
        require(db.execute('SELECT account_key, key, payload FROM preferences').fetchall() == [('design-demo', 'preferences', json.dumps({'launch': 'shelf'}))], 'Changed synthetic route input')
    (package / 'cleanup-assets').write_bytes(bindings['demo_roster']())
    (package / 'data/device-profile').write_text('scribe\n')
    (package / 'data/device-allowlist').write_text('scribe 5.19.5\n')
    return package


def route_roster(package, *, switch_source=None, projection='roster.json'):
    switch = (switch_source or ROOT / 'switch.sh').read_text()
    simple = [re.search(r'^' + name + r'\(\) \{[^\n]*\}$', switch, re.MULTILINE) for name in ('fail', 'present', 'directory', 'plain')]
    walker = re.search(r'^clean_tree\(\) \(\n.*?^\)$', switch, re.MULTILINE | re.DOTALL)
    require(walker and all(simple), 'Cannot identify the unchanged route cleanup verifier')
    commands = '\n'.join(['set -eu', 'export LC_ALL=C', *(entry[0] for entry in simple), walker[0], 'stage=$1', 'run=unused-synthetic-render', 'clean_tree "$stage" verify'])
    checked = subprocess.run(['/bin/sh', '-c', commands, 'offline-roster-only', str(package)], capture_output=True, text=True, timeout=30)
    data = package / 'data'
    directories, leaves, relative_files = [], Counter(), set()
    for path in sorted(data.rglob('*')):
        relative = path.relative_to(package).as_posix()
        require(not path.is_symlink(), 'Unexpected symlink in synthetic application data')
        if path.is_dir():
            directories.append(relative)
            continue
        require(path.is_file() and path.stat().st_nlink == 1, 'Unexpected file type in synthetic application data')
        relative_files.add(relative)
        if relative.startswith('data/demo/fixtures/'):
            leaves[str(Path(relative).parent / ('*' + path.suffix))] += 1
        else:
            leaves[relative] += 1
    roster = set((package / 'cleanup-assets').read_text().splitlines())
    actual = {name for name in relative_files if name.startswith('data/demo/fixtures/')}
    report = {'initial_preferences_rows': 1, 'route_preparer_seed_statements_reused': True, 'runtime_directories': directories, 'runtime_leaf_categories': dict(leaves),
              'expected_bundled_leaf_count': len(roster), 'materialized_bundled_leaf_count': len(actual), 'no_unlisted_bundled_leaf': actual <= roster,
              'raw_route_clean_tree_verify_exit': checked.returncode, 'physical_route_operations_executed': False}
    store(package.parent / projection, report)
    require(checked.returncode == 0 and actual <= roster, 'Runtime data outside the route cleanup allowlist; see private roster.json')
    return report


def desktop_case(profile, executable, directory, from_route=False):
    sys.path.append(str(ROOT))
    from desktop_pairs import Sampler

    elf(executable, 'x86_64')
    size, boxes = PROFILES[profile]
    wanted = reference(profile, 'shelf', size, boxes)
    loading = reference(profile, 'loading', size, boxes)
    expected = {}
    for name in boxes:
        ink = sum(value < 128 for value in wanted[name][::3])
        require(ink >= 150 and sum(first != second for first, second in zip(wanted[name][::3], loading[name][::3])) >= 150, 'Shelf evidence is blank or matches the startup reference')
        expected[name] = {'x_y_width_height': list(boxes[name]), 'reference_ink_pixels': ink}
    home = directory / 'home'
    home.mkdir(mode=0o700)
    environment = dict(os.environ, HOME=str(home), SLINT_BACKEND='winit-software', WINIT_UNIX_BACKEND='x11', SLINT_SCALE_FACTOR='2',
                       XDG_CACHE_HOME=str(home / 'cache'), XDG_CONFIG_HOME=str(home / 'config'), XDG_DATA_HOME=str(home / 'data'))
    require(not any(environment.get(key) for key in ('LD_PRELOAD', 'BILICOMICS_DESKTOP_PRESENT_PROBE')), 'Unexpected research environment')
    package = route_seed(directory) if from_route else directory
    arguments = [executable, '--demo', '--offline', '--device', profile, *([] if from_route else ['--screen', 'shelf']), '--data-dir', package / 'data', '--lifecycle-trace', directory / 'lifecycle.jsonl', '--exit-after-ms', '2500' if from_route else '2000']
    observer = Sampler()
    viewable = None
    matched = False
    with (directory / 'application.log').open('x') as log:
        child = subprocess.Popen(arguments, env=environment, stdout=log, stderr=subprocess.STDOUT)
        try:
            require(os.path.samefile(f'/proc/{child.pid}/exe', executable), 'Unexpected desktop application executable')
            deadline = time.monotonic() + 60
            while time.monotonic() < deadline and child.poll() is None:
                if viewable is None:
                    viewable = mapped_window(child.pid, size)
                if viewable is not None:
                    if all(observer.get(viewable, box)[1] == wanted[name] for name, box in boxes.items()):
                        matched = True
                        break
                time.sleep(.025)
            status = child.wait(60)
            require(status == 0, 'Nonzero desktop application exit; private stderr is retained')
            require(matched and viewable is not None, 'The same-run X11 surface did not show all reference regions')
        finally:
            if child.poll() is None:
                child.kill()
                child.wait(10)
            observer.close()
    ready = [row for row in map(json.loads, (directory / 'lifecycle.jsonl').read_text().splitlines()) if row.get('event') == 'ready']
    require(len(ready) == 1 and ready[0].get('process_id') == child.pid and ready[0].get('route') == 'shelf', 'No matching same-process Shelf lifecycle data')
    result = {'geometry': list(size), 'scale': 2, 'stock_winit_software_x11': True, 'owned_viewable_window': True, 'same_process_ready_route': 'shelf', 'exit_status': status,
              'visible_non_cover_regions': expected, 'all_referenced_region_bytes_match': True, 'all_regions_distinct_from_loading': True, 'demo_offline': True,
              'screen_override': not from_route, 'user_input_injected': False, 'native_card_release_observed': False, 'includes_native_driver_or_panel': False}
    if from_route:
        result['fixture_data_audit'] = route_roster(package)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('kind', choices=('arm', 'desktop', 'route'))
    parser.add_argument('profile', choices=PROFILES)
    parser.add_argument('executable', type=Path)
    parser.add_argument('attempt', type=int, choices=range(1, 5))
    args = parser.parse_args()
    os.umask(0o077)
    require(Path.cwd().resolve() == ROOT and [name for _, name in socket.if_nameindex()] == ['lo'], 'Run inside the private loopback-only Ubuntu workspace')
    directory = ROOT / f'{args.kind}-{args.profile}-{args.attempt}'
    directory.mkdir(mode=0o700)
    result = {'kind': args.kind, 'profile': args.profile, 'attempt': args.attempt, 'executable_bytes': args.executable.stat().st_size}
    try:
        require(args.kind != 'route' or args.profile == 'scribe', 'The installed route fixture targets Scribe')
        observation = arm_case(args.profile, args.executable, directory) if args.kind == 'arm' else desktop_case(args.profile, args.executable, directory, args.kind == 'route')
        result.update(status='passed', observation=observation)
    except Exception as error:
        result.update(status='failed', reason=str(error))
        store(directory / 'result.json', result)
        raise
    store(directory / 'result.json', result)
    print(json.dumps(result, sort_keys=True))


if __name__ == '__main__':
    main()
