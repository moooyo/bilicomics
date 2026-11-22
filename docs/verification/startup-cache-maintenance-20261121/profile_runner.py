"""Profile a single executable against two semantically identical, closed synthetic cache states."""

import argparse
from contextlib import closing
import json
import os
from pathlib import Path
import shutil
import sqlite3
import subprocess
import sys

for sibling in ('startup-direct-png-20261121', 'startup-cover-worker-20261121'):
    sys.path.append(str(Path(__file__).resolve().parent.parent / sibling))
import make_fixture as fixture
import ordinary_pairs as ordinary
from desktop_pairs import one, require, rgb


ROOT = Path('/work')
BINARY = ROOT / 'study'
ACCOUNT, KEY = fixture.FIXTURE_ACCOUNT, 'catalog_cache_recency'
SELECT = 'SELECT payload FROM preferences WHERE account_key=? AND key=?'
PHASES = 'timeout_setup begin_immediate recency durable delete_scopes eviction_candidates delete_details delete_members delete_comics reconcile recency_upsert commit'.split()
DELETES = ('delete_scopes', 'delete_details', 'delete_members', 'delete_comics')
SCHEDULE = ['f00', 'r00'] + [f'{arm}{index:02d}' for index in range(1, 7) for arm in ('fr' if index % 2 else 'rf')]
EXTRA = '719999'


def checkpoint(data):
    database = data / 'state.sqlite3'
    require(database.is_file() and not database.is_symlink(), 'Missing owned SQLite file')
    with closing(sqlite3.connect(database)) as connection:
        require(connection.execute('PRAGMA wal_checkpoint(TRUNCATE)').fetchone() == (0, 0, 0), 'Could not close the synthetic WAL')
    fixture.require_closed(database)


def text(data):
    with closing(fixture.open_closed(data / 'state.sqlite3')) as connection:
        result = connection.execute(SELECT, (ACCOUNT, KEY)).fetchall()
        require(len(result) == 1 and isinstance(result[0][0], str), 'Seed must already contain one recency TEXT')
        return result[0][0]


def other_rows(data):
    with closing(fixture.open_closed(data / 'state.sqlite3')) as connection:
        result = {}
        for table in sorted(fixture.TABLES):
            where, params = (' WHERE NOT(account_key=? AND key=?)', (ACCOUNT, KEY)) if table == 'preferences' else ('', ())
            result[table] = sorted(connection.execute(f'SELECT * FROM "{table}"' + where, params).fetchall())
        return result


def launch(directory, binary, populated=True, signed_in=True, performance=True):
    home = directory / 'home'
    home.mkdir(mode=0o700)
    require('LD_PRELOAD' not in os.environ and 'BILICOMICS_DESKTOP_PRESENT_PROBE' not in os.environ, 'Unexpected experimental process environment')
    args = [binary, '--offline', '--screen', 'shelf', '--device', 'scribe', '--data-dir', directory / 'data',
            '--lifecycle-trace', directory / 'lifecycle.jsonl', '--capture', directory / 'final.png', '--exit-after-ms', '800']
    if performance:
        args.extend(['--performance-trace', directory / 'performance.jsonl'])
    env = dict(os.environ, HOME=str(home), SLINT_BACKEND='winit-software', WINIT_UNIX_BACKEND='x11', SLINT_SCALE_FACTOR='2',
               XDG_CACHE_HOME=str(home / 'cache'), XDG_CONFIG_HOME=str(home / 'config'), XDG_DATA_HOME=str(home / 'xdg-data'))
    with (directory / 'application.log').open('xb') as log:
        process = subprocess.Popen(args, env=env, stdout=log, stderr=subprocess.STDOUT)
        try:
            require(os.path.samefile(f'/proc/{process.pid}/exe', binary), 'Wrong running executable')
            require(process.wait(30) == 0, 'Nonzero exit')
        finally:
            ordinary.observer.wait_child(process)
    if performance:
        ordinary.check_final(directory, process.pid, populated=populated)
    else:
        final = ordinary.load(directory / 'final.json')
        require(not (directory / 'performance.jsonl').exists() and final['process_id'] == process.pid and final['route'] == 'shelf'
                and final['demo'] is False and final['offline'] and final['offline_requested'] and final['backend'] == 'desktop-software'
                and final['profile'] == 'scribe' and not final['startup']['booting'] and not final['startup']['error'], 'Invalid performance-disabled control')
        ready = one(ordinary.records(directory / 'lifecycle.jsonl'), lambda row: row['event'] == 'ready', 'ordinary Ready')
        require(ready['process_id'] == process.pid and ready['route'] == 'shelf' and ready['reader_resources']['cover_resident_count'] == 10
                and ready['reader_resources']['cover_resident_bytes'] == 5324800, 'Wrong performance-disabled Ready')
    if populated:
        expected = list(range(1, 11)) if signed_in else [None] * 10
        require(ordinary.marker_codes(rgb(directory / 'final.png')) == expected, 'Wrong independent terminal Shelf marker codes')
    return process.pid


def check_event(directory, pid, scenario):
    perf, life = (ordinary.records(directory / leaf) for leaf in ('performance.jsonl', 'lifecycle.jsonl'))
    def stages(name):
        return [row for row in perf if row['kind'] == 'startup' and row['stage'] == name]
    start, end = stages('state-cache-maintenance')
    inner, = stages('catalog_cache_maintenance')
    clock = lambda row: row['monotonic_ns']
    require(all(row['process_id'] == pid and row['thread_id'] == inner['thread_id'] != pid for row in (start, inner, end))
            and clock(start) <= clock(end) <= clock(inner) and start['data']['phase'] == 'begin' and end['data']['phase'] == 'end'
            and end['data']['outcome'] in ('partial', 'success'), 'Invalid single worker maintenance event or parent span')
    data = inner['data']
    require(data['outcome'] in ('committed', 'busy', 'error') and type(data['committed']) is bool and
            all(data[key] is None or type(data[key]) is bool for key in ('recency_present', 'timeout_restored')), 'Unknown maintenance schema')
    phase_names = [part['stage'] for part in data['phases']]
    require(phase_names == [label for label in PHASES if label in phase_names], 'Unordered, unknown or duplicate operations')
    phases = []
    for part in data['phases']:
        require(type(part['ok']) is bool, 'Unknown operation status')
        for key in ('wall_us', 'cpu_us'):
            require(type(part[key]) is int and part[key] >= 0, 'Missing Linux wall or thread CPU timing')
        has_count = part['ok'] and part['stage'] in (*DELETES, 'durable', 'eviction_candidates', 'recency_upsert')
        require((type(part['rows']) is int and part['rows'] >= 0) if has_count else part['rows'] is None, 'Invalid per-phase count')
        phases.append({key: part[key] for key in ('stage', 'ok', 'wall_us', 'cpu_us', 'rows')})
    counts = {part['stage']: part['rows'] for part in phases}
    matches = (data['outcome'] == 'committed' and data['committed'] is True and data['timeout_restored'] is True
               and data['recency_present'] is (scenario != 'missing') and end['data']['outcome'] == 'success'
               and phase_names == PHASES and all(part['ok'] for part in phases) and counts.get('durable') == 20)
    for name in (*DELETES, 'eviction_candidates'):
        expected = int(scenario == 'eviction' and name in ('eviction_candidates', 'delete_comics', 'delete_members'))
        matches = matches and counts.get(name) == expected
    matches = matches and counts.get('recency_upsert') == (0 if scenario in ('r', 'eviction') else 1)
    entry, = stages('process_enter')
    prepared, = stages('state_prepared')
    ready, = stages('ui_ready')
    entered, produced = (row['data']['observed']['monotonic_ns'] for row in (entry, prepared))
    require(all(row['thread_id'] == pid for row in (entry, prepared, ready)) and entered <= clock(start) <= clock(end) <= clock(inner) <= produced <= clock(prepared) <= clock(ready),
            'Startup clocks do not bracket the maintenance')
    for label, stamp in (('process_enter', entered), ('state_prepared', produced)):
        require([row['monotonic_ns'] for row in life if row.get('stage') == label] == [stamp], 'Different application producer clocks')
    receipt = one(life, lambda row: row['event'] == 'ready', 'ordinary Ready')
    require(receipt['route'] == 'shelf' and receipt['monotonic_ns'] >= clock(ready), 'Invalid lifecycle Ready')
    return {'pid': pid, 'worker_tid': inner['thread_id'], 'same_executable': True, 'row_hypothesis_matches': matches, 'ready_covers': 10, 'resident_bytes': 5324800,
            'final_marker_count': 10, 'process_enter_ns': entered, 'state_prepared_ns': produced, 'ui_ready_ns': clock(ready),
            'parent': {'begin_ns': clock(start), 'end_ns': clock(end), 'outcome': end['data']['outcome'], 'wall_us': end['data']['wall_us'], 'cpu_us': end['data']['cpu_us']},
            'maintenance': {'record_ns': clock(inner), 'outcome': data['outcome'], 'committed': data['committed'],
                            'recency_present': data['recency_present'], 'timeout_restored': data['timeout_restored'], 'phases': phases}}


def start_xvfb(name):
    try:
        subprocess.run([*ordinary.XVFB, sys.executable, '-B', __file__, name, '--inside'], check=True, timeout=40)
    finally:
        ordinary.observer.require_retired()
        for path in Path('/proc').glob('[0-9]*/exe'):
            try:
                target = path.readlink()
            except (FileNotFoundError, ProcessLookupError, PermissionError):
                continue
            require(target != BINARY, 'A study process is still alive')


def inside(name):
    if name == 'bootstrap':
        directory = ROOT / 'bootstrap'
        directory.mkdir(mode=0o700)
        launch(directory, BINARY, populated=False)
        return
    case = (ROOT / 'trial' if name in SCHEDULE else ROOT / 'setup') / name
    case.mkdir(mode=0o700)
    scenario = name[0] if name in SCHEDULE else name.removeprefix('qualify-')
    source = {'normalize': ordinary.POSITIVE, 'untraced': ordinary.POSITIVE, 'guest': ordinary.NEGATIVE, 'missing': ordinary.POSITIVE,
              'eviction': ROOT / 'r-seed', 'f': ROOT / 'f-seed', 'r': ROOT / 'r-seed'}[scenario]
    data = case / 'data'
    ordinary.private_copy(source, data, scenario != 'guest')
    try:
        if scenario in ('missing', 'eviction'):
            with closing(sqlite3.connect(data / 'state.sqlite3')) as db, db:
                if scenario == 'missing':
                    require(db.execute('DELETE FROM preferences WHERE account_key=? AND key=?', (ACCOUNT, KEY)).rowcount == 1, 'No recency to remove')
                else:
                    body = json.dumps({'id': EXTRA, 'title': 'Disposable Eviction Study', 'favorite': False, 'updated_at': 0, 'last_read_at': 0})
                    db.execute('INSERT INTO comics(account_key,comic_id,payload,updated_at,last_read_at) VALUES(?,?,?,0,0)', (ACCOUNT, EXTRA, body))
                    scope = json.loads(text(source))['scopes'][0]
                    db.execute('INSERT INTO catalog_members(account_key,scope,comic_id,position) VALUES(?,?,?,20)', (ACCOUNT, scope, EXTRA))
            checkpoint(data)
        pid = launch(case, BINARY, signed_in=scenario != 'guest', performance=scenario != 'untraced')
        if scenario == 'guest':
            ordinary.save(case / 'case.json', {'case': name, 'pid': pid, 'ready_covers': 10, 'visible_marker_count': 0})
            return
        checkpoint(data)
        if scenario == 'untraced':
            saved = text(data)
            expected = (ROOT / 'setup/product-recency.private').read_text()
            require(saved == expected, 'Traced and untraced product paths normalized to different text')
            ordinary.save(case / 'case.json', {'case': name, 'pid': pid, 'performance_tracing_enabled': False,
                          'same_closed_product_recency_text': True, 'ready_covers': 10, 'final_marker_count': 10})
            return
        result = check_event(case, pid, 'f' if scenario == 'normalize' else scenario)
        if scenario == 'eviction':
            with closing(fixture.open_closed(data / 'state.sqlite3')) as db:
                result['eviction_removed_from_database'] = db.execute('SELECT count(*) FROM comics WHERE account_key=? AND comic_id=?', (ACCOUNT, EXTRA)).fetchone() == (0,)
        original, final = text(ordinary.POSITIVE), text(data)
        result['semantics_preserved'] = json.loads(original) == json.loads(final)
        result['expected_final_representation'] = original != final if scenario == 'normalize' else final == text(ROOT / 'r-seed')
        ordinary.save(case / 'case.json', dict(case=name, scenario=scenario, **result))
        require(result['semantics_preserved'], 'Product maintenance changed semantic recency; stop rather than replacing this case')
        if scenario == 'normalize':
            require(result['expected_final_representation'] and result['row_hypothesis_matches'], 'No qualified product normalization')
            with (ROOT / 'setup/product-recency.private').open('x') as output:
                output.write(final)
        require(result.get('eviction_removed_from_database', True), 'Expected genuine eviction not observed')
    finally:
        shutil.rmtree(data)


def seeds():
    stable = (ROOT / 'setup/product-recency.private').read_text()
    fresh = text(ordinary.POSITIVE)
    require(stable != fresh and json.loads(stable) == json.loads(fresh), 'Cannot isolate recency representation')
    original_rows = other_rows(ordinary.POSITIVE)
    for label, payload in [('f', fresh), ('r', stable)]:
        seed = ROOT / (label + '-seed')
        ordinary.private_copy(ordinary.POSITIVE, seed, True)
        with closing(sqlite3.connect(seed / 'state.sqlite3')) as db, db:
            require(db.execute('UPDATE preferences SET payload=? WHERE account_key=? AND key=?', (payload, ACCOUNT, KEY)).rowcount == 1, 'Missing seed recency')
        checkpoint(seed)
        require(text(seed) == payload and other_rows(seed) == original_rows, 'Other logical SQLite rows differ')
    count = {key: len(value) for key, value in json.loads(fresh).items()}
    require(count == {'scopes': 1, 'comics': 20, 'details': 0}, 'Unexpected synthetic recency')
    ordinary.save(ROOT / 'setup/seeds.json', {'source_recency_present': True, 'semantic_values_equal': True, 'raw_text_equal': False,
                  'fresh_utf8_bytes': len(fresh.encode()), 'product_utf8_bytes': len(stable.encode()), 'recency_counts': count,
                  'removing_fixture_json_spacing_matches_product': json.dumps(json.loads(fresh), separators=(',', ':'), ensure_ascii=False) == stable,
                  'all_other_database_rows_identical': True, 'same_private_synthetic_profile_source': True})


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=['bootstrap', 'make-seed', 'qualify', 'trial', 'normalize', 'untraced', 'guest', 'qualify-f', 'qualify-r', 'missing', 'eviction', *SCHEDULE])
    parser.add_argument('--inside', action='store_true', help=argparse.SUPPRESS)
    args = parser.parse_args()
    os.umask(0o077)
    ordinary.observer.require_private_network()
    require(Path.cwd() == ROOT, 'Expected a private /work mount')
    name = args.action
    if args.inside:
        inside(name)
    elif name == 'bootstrap':
        start_xvfb(name)
        checkpoint(ROOT / 'bootstrap/data')
        ordinary.save(ROOT / 'bootstrap/receipt.json', {'normal_exit': True, 'empty_schema_only': True})
    elif name == 'make-seed':
        ordinary.make_seed()
    elif name == 'qualify':
        (ROOT / 'setup').mkdir(mode=0o700)
        for item in ('normalize', 'untraced', 'guest'):
            start_xvfb(item)
        seeds()
        for item in ('qualify-f', 'qualify-r', 'missing', 'eviction'):
            start_xvfb(item)
            case = ordinary.load(ROOT / 'setup' / item / 'case.json')
            require(case['row_hypothesis_matches'] and case['expected_final_representation'], 'Untimed input identities or row hypotheses are not qualified')
        ordinary.save(ROOT / 'setup/qualification.json', {'seeds': ordinary.load(ROOT / 'setup/seeds.json'), 'cases': {
                      key: ordinary.load(ROOT / 'setup' / key / 'case.json') for key in ('normalize', 'untraced', 'guest', 'qualify-f', 'qualify-r', 'missing', 'eviction')}})
    elif name == 'trial':
        require((ROOT / 'setup/qualification.json').is_file(), 'No qualified inputs')
        trial = ROOT / 'trial'
        trial.mkdir(mode=0o700)
        results = {}
        for item in SCHEDULE:
            try:
                start_xvfb(item)
                results[item] = ordinary.load(trial / item / 'case.json')
                values = list(results.values())
                require(len(values) < 2 or values[-2]['ui_ready_ns'] < values[-1]['process_enter_ns'], 'Overlapping launches')
            except Exception as error:
                ordinary.save(trial / 'failure.json', {'failed_case': item, 'failure_type': type(error).__name__, 'completed': list(results)})
                raise
        ordinary.save(trial / 'projection.json', {'schema': 'ordinary-cache-maintenance-profile-v1',
                      'scope': 'Same ordinary executable, different representations of the same recency; not an optimization or display endpoint.',
                      'clock': 'CLOCK_MONOTONIC and worker thread CPU', 'phase_cost_unit': 'microseconds', 'event_stamp_unit': 'nanoseconds',
                      'schedule': SCHEDULE, 'excluded_warmups': SCHEDULE[:2], 'cases': results})
    else:
        raise ValueError('Individual formal cases must run under the fixed schedule')


if __name__ == '__main__':
    main()
