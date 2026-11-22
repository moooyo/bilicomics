"""Qualify the two native flag combinations and shared desktop Shelf geometry off-device."""

import argparse
import ast
import json
import os
from pathlib import Path
import socket
import sqlite3
import subprocess
import sys
import uuid


STAGE = Path('/work/emulator-surface')
ORACLE = STAGE / 'oracle'
sys.path.insert(0, str(ORACLE))
import profiles as surface


def require(condition, message):
    if not condition:
        raise ValueError(message)


def seed(directory):
    package = directory / 'route-stage'
    demo = package / 'data/demo'
    demo.mkdir(mode=0o700, parents=True)
    path = demo / 'state.sqlite3'
    recipe = [item for item in ast.parse((ORACLE / 'prepare_route.py').read_bytes()).body if isinstance(item, ast.FunctionDef) and item.name == 'assemble']
    require(len(recipe) == 1, 'Expected one route preparer')
    body = recipe[0].body
    offsets = [index for index, item in enumerate(body) if isinstance(item, ast.Assign) and [getattr(target, 'id', '') for target in item.targets] == ['seed']]
    require(len(offsets) == 1, 'Expected the reviewed single-row seed')
    selected = body[offsets[0]:]
    require([type(item) for item in selected] == [ast.Assign, ast.With, ast.Expr], 'The seed recipe changed; inspect it first')
    symbols = {'output': package, 'sqlite3': sqlite3, 'json': json}
    exec(compile(ast.Module(body=selected, type_ignores=[]), '<route preferences only>', 'exec'), symbols)
    require(symbols['seed'] == path, 'Unexpected route seed destination')
    with sqlite3.connect(f'file:{path}?mode=ro', uri=True) as database:
        require(database.execute('SELECT account_key, key, payload FROM preferences').fetchall() == [('design-demo', 'preferences', json.dumps({'launch': 'shelf'}))], 'Changed synthetic route input')
    assets = Path('/work/assets/demo')
    files = sorted(path.relative_to(assets).as_posix() for path in assets.rglob('*') if path.is_file() and not path.is_symlink())
    require('manifest.json' in files and 'catalog.json' in files, 'Missing the source-matched fixture assets')
    (package / 'cleanup-assets').write_text(''.join(f'data/demo/fixtures/{name}\n' for name in files))
    return package


def identify(executable, architecture):
    surface.elf(executable, architecture)
    path = executable.resolve(strict=True)
    require(path.is_relative_to('/work') and not path.is_relative_to(STAGE) and not executable.is_symlink(), 'Give the current build owner-qualified application path')
    return path


def arm_case(profile, arm, application, case):
    binary = identify(application, 'arm')
    require(not Path('/dev/fb0').exists() and not Path('/dev/input').exists(), 'Not a no-device user-mode sandbox')
    common = ['qemu-arm', binary, '--device', profile]
    run = 'bili-shelf-' + uuid.uuid4().hex
    trace = case / (run + '.jsonl')
    life = case / 'lifecycle.jsonl'
    version = subprocess.run([*common, '--version'], check=True, capture_output=True, text=True, timeout=30).stdout.strip()
    controls = {}
    if arm == 'a':
        for missing, remaining in [('kindle', ['--shelf-render-trace', trace]), ('shelf-render-trace', ['--kindle'])]:
            rejected = subprocess.run([*common, '--serial-shelf-render', *remaining], capture_output=True, text=True, timeout=30)
            require(rejected.returncode == 2 and 'required arguments were not provided' in rejected.stderr and f'--{missing}' in rejected.stderr and not trace.exists(), 'Invalid diagnostic flag combination was admitted')
            controls['missing_' + missing.replace('-', '_')] = rejected.returncode
    package = seed(case)
    args = [*common, '--kindle', '--shelf-render-trace', trace, *(['--serial-shelf-render'] if arm == 'a' else []), '--demo', '--offline', '--data-dir', package / 'data', '--lifecycle-trace', life, '--exit-after-ms', '2500']
    launched = subprocess.run(args, capture_output=True, text=True, timeout=30)
    require(launched.returncode == 1 and 'Native output requires an owned launcher mode snapshot' in launched.stderr, 'Did not reach the native launcher-ownership guard')
    diagnostics = list(map(json.loads, trace.read_text().splitlines()))
    lifecycle = list(map(json.loads, life.read_text().splitlines()))
    require([entry.get('stage') for entry in diagnostics] == ['trace_started', 'trace_stopped'], 'Missing private diagnostic or unexpected rendering on the user-mode host')
    header, footer = diagnostics
    require(header['data'].get('run_id') == run and header['data'].get('schema') == 'kindle-shelf-render-log-v1' and header['data'].get('clock') == 'CLOCK_MONOTONIC', 'Wrong synthetic trace identity')
    require(footer['data'].get('reason') == 'finished' and footer['data'].get('truncated') is False, 'Incomplete trace closure')
    require(all(row.get('process_id') == header.get('process_id') for row in [*diagnostics, *lifecycle]) and header.get('process_id', 0) > 0, 'Different process identities')
    require([row.get('stage') for row in lifecycle] == ['process_enter'] and not any(row.get('event') == 'ready' for row in lifecycle), 'Unexpected stages before native admission')
    return {'version': version, 'qemu': subprocess.check_output(['qemu-arm', '--version'], text=True).splitlines()[0], 'serial_flag': arm == 'a', 'expected_missing_flag_exits': controls,
            'native_guard_exit': launched.returncode, 'matched_process_entry': True, 'same_run_trace': ['trace_started', 'trace_stopped'],
            'native_shelf_rendered': False, 'render_order_observed': False, 'framebuffer_submission': False}


def desktop_case(profile, application, case):
    binary = identify(application, 'x86_64')
    surface.ROOT = ORACLE
    surface.route_seed = seed
    observed = surface.desktop_case(profile, binary, case, from_route=True)
    require(observed['screen_override'] is False and observed['same_process_ready_route'] == 'shelf' and not observed['user_input_injected'], 'The ordinary route did not reach Shelf without input')
    return {'common_desktop_path_not_an_arm': True, 'geometry': observed['geometry'], 'same_process_ready_route': 'shelf', 'screen_override': False, 'end_status': observed['exit_status'],
            'mapped_window_pid_verified': observed['owned_viewable_window'], 'matched_non_cover_regions': sorted(observed['visible_non_cover_regions']), 'all_regions_differ_from_loading': observed['all_regions_distinct_from_loading'],
            'all_region_bytes_match': observed['all_referenced_region_bytes_match'], 'injected_input': False, 'bundled_fixture_files': observed['fixture_data_audit']['materialized_bundled_leaf_count']}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('kind', choices=('arm', 'desktop'))
    parser.add_argument('profile', choices=('scribe', 'pw6'))
    parser.add_argument('mode', choices=('a', 'b', 'shared'))
    parser.add_argument('executable', type=Path)
    parser.add_argument('--attempt', choices=(1, 2, 3), type=int, default=1)
    args = parser.parse_args()
    os.umask(0o077)
    require(os.geteuid() == 0 and Path.cwd().resolve() == STAGE and [name for _, name in socket.if_nameindex()] == ['lo'], 'Not the loopback-only run workspace')
    require((args.kind == 'desktop') == (args.mode == 'shared'), 'The desktop cannot toggle native render order')
    case = STAGE / 'results' / f'{args.kind}-{args.profile}-{args.mode}-{args.attempt}'
    case.mkdir(mode=0o700)
    result = {'kind': args.kind, 'profile': args.profile, 'mode': args.mode, 'attempt': args.attempt}
    try:
        before = args.executable.stat()
        result['application_bytes'] = before.st_size
        result['observation'] = desktop_case(args.profile, args.executable, case) if args.kind == 'desktop' else arm_case(args.profile, args.mode, args.executable, case)
        after = args.executable.stat()
        require((before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns) == (after.st_dev, after.st_ino, after.st_size, after.st_mtime_ns), 'The executable changed while running')
        result['status'] = 'passed'
    except Exception as error:
        result.update(status='failed', reason=str(error))
        surface.store(case / 'result.json', result)
        raise
    surface.store(case / 'result.json', result)
    print(json.dumps(result, sort_keys=True))


if __name__ == '__main__':
    main()
