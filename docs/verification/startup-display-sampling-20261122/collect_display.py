"""Capture just the display-helper receipt after retirement; summarize only exactly released ordinary starts."""

import argparse
import importlib.util
import json
from pathlib import Path
from statistics import median


HERE = Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location('closed_native_visit', HERE.parent / 'startup-prepared-handoff-20261122/handoff_collect.py')
NATIVE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(NATIVE)
CONTROL, BOUNDS = NATIVE.CONTROL, NATIVE.BOUNDS
require, read, present = NATIVE.require, NATIVE.read, NATIVE.present
SCHEMA = 'bili-display-sampling-v1'
FRESH = {'admission.json', 'route.tar.gz', 'probe.tar.gz', 'native-guard.sh', 'askpass', 'display-visit.json'}
MAX_RECORD = 8192
PRIVATE_FIELDS = {'schema', 'run_id', 'boot_id', 'clock', 'supervisor_pid', 'supervisor_birth', 'native_read_pid', 'native_read_birth',
                  'native_read_sequence', 'helper_pid', 'helper_birth', 'helper_started_monotonic_ns', 'helper_finished_monotonic_ns', 'process_stat_reads', 'buckets'}
TOP = ('launch_approval', 'permit_acquire', 'permit_initial', 'query_outer_pre', 'query_outer_post', 'metadata_outer_pre',
       'metadata_outer_post', 'metadata_1', 'metadata_2', 'commands_run')
COMMAND = ('query_inner_pre', 'query_inner_post', 'query_inner_periodic', 'command_spawn', 'command_read', 'command_try_wait', 'command_poll')
BUCKETS = set(TOP) | set(COMMAND) | {'metadata_checkpoint'}
WIRE_IDENTITIES = ('supervisor_pid', 'supervisor_birth', 'native_read_pid', 'native_read_birth', 'native_read_sequence', 'helper_pid',
                   'helper_birth', 'helper_started_monotonic_ns', 'helper_finished_monotonic_ns', 'process_stat_reads')
WIRE_BUCKETS = ('launch_approval', 'permit_acquire', 'permit_initial', 'query_outer_pre', 'query_outer_post', 'query_inner_pre',
                'query_inner_post', 'query_inner_periodic', 'metadata_outer_pre', 'metadata_outer_post', 'metadata_checkpoint',
                'metadata_1', 'metadata_2', 'commands_run', 'command_spawn', 'command_read', 'command_try_wait', 'command_poll')


def natural(value):
    return type(value) is int and 0 <= value < 1 << 64


def count32(value):
    return type(value) is int and 0 <= value < 1 << 32


def decode(raw, run, boot, ui_pid):
    require(type(raw) is bytes and 0 < len(raw) <= MAX_RECORD and raw.endswith(b'\n') and len(raw.splitlines()) == 1,
            'Missing or incomplete display-cost record')
    try:
        row = json.loads(raw, object_pairs_hook=BOUNDS.unique_object, parse_constant=BOUNDS.reject_constant)
        require(type(row) is dict and set(row) == PRIVATE_FIELDS and (row['schema'], row['clock'], row['run_id'], row['boot_id']) ==
                ('kindle-startup-display-cost-v1', BOUNDS.CLOCK, run, boot) and CONTROL.RUN.fullmatch(run) and CONTROL.UUID.fullmatch(boot),
                'Invalid display helper identity or schema')
        pids = (ui_pid, row['supervisor_pid'], row['native_read_pid'], row['helper_pid'])
        require(all(NATIVE.positive(value) and value <= 0x7fffffff for value in pids) and len(set(pids)) == len(pids)
                and all(NATIVE.positive(row[key]) for key in ('native_read_sequence', 'supervisor_birth', 'native_read_birth', 'helper_birth',
                        'helper_started_monotonic_ns', 'helper_finished_monotonic_ns')) and
                count32(row['process_stat_reads']) and row['process_stat_reads'] > 0 and
                row['supervisor_birth'] <= row['native_read_birth'] <= row['helper_birth'], 'Invalid display helper ancestry identity')
        buckets = row['buckets']
        require(type(buckets) is dict and set(buckets) == BUCKETS, 'Unknown display timing buckets')
        for sample in buckets.values():
            require(type(sample) is dict and set(sample) == {'count', 'elapsed_ns'} and count32(sample['count']) and natural(sample['elapsed_ns']) and
                    (sample['count'] > 0 or sample['elapsed_ns'] == 0), 'Invalid display helper sample')
    except (KeyError, ValueError, TypeError, UnicodeError):
        raise CONTROL.OperatorError('Invalid display helper record; private content withheld') from None
    return row


def components(record, release_after, trace_header):
    start, finish = record['helper_started_monotonic_ns'], record['helper_finished_monotonic_ns']
    require(release_after <= start <= finish <= trace_header, 'Display capture is not inside the native card-to-UI startup window')
    total = finish - start
    buckets = record['buckets']
    count = {key: value['count'] for key, value in buckets.items()}
    time = {key: value['elapsed_ns'] for key, value in buckets.items()}
    nqueries = count['commands_run']
    require(nqueries in (3, 5) and all(count[key] == nqueries for key in ('query_outer_pre', 'query_outer_post', 'query_inner_pre', 'query_inner_post', 'command_spawn'))
            and all(count[key] == 1 for key in ('launch_approval', 'permit_acquire', 'permit_initial', 'metadata_1', 'metadata_2'))
            and count['metadata_outer_pre'] == count['metadata_outer_post'] == 2 and count['metadata_checkpoint'] == 6
            and count['command_read'] >= nqueries and count['command_try_wait'] >= nqueries, 'Unexpected successful display capture counts')
    top_residual = total - sum(time[key] for key in TOP)
    command_residual = time['commands_run'] - sum(time[key] for key in COMMAND)
    metadata_residual = time['metadata_1'] + time['metadata_2'] - time['metadata_checkpoint']
    require(min(top_residual, command_residual, metadata_residual) >= 0, 'Display child intervals exceed their measured parent')
    return {'query_branch': 'automatic-five' if nqueries == 5 else 'fixed-three', 'helper_elapsed_ns': total,
            'process_stat_reads': record['process_stat_reads'], 'buckets': record['buckets'], 'residual_ns': {
        'helper_outside_top_level_buckets': top_residual, 'commands_outside_measured_children': command_residual,
        'metadata_outside_measured_checkpoints': metadata_residual}}


def name_for(run):
    require(type(run) is str and CONTROL.RUN.fullmatch(run), 'Invalid display sampling run')
    return run + '.display-cost.json'


def capture_command(run, route, boot):
    leaf = name_for(run)
    require(route == '/mnt/us/' + run and type(boot) is str and CONTROL.UUID.fullmatch(boot), 'Unexpected display study stage or boot')
    number = '(0|[1-9][0-9]*)'
    identity = ','.join(f'"{key}":{number}' for key in WIRE_IDENTITIES)
    buckets = ','.join(fr'"{key}":\{{"elapsed_ns":{number},"count":{number}\}}' for key in WIRE_BUCKETS)
    pattern = fr'^\{{"schema":"kindle-startup-display-cost-v1","clock":"CLOCK_MONOTONIC","run_id":"{run}","boot_id":"{boot}",{identity},"buckets":\{{{buckets}\}}\}}$'
    return f'''root={route}
for dir in "$root" "$root/data"; do
 test -d "$dir" && test ! -L "$dir" && test "$(readlink -f "$dir")" = "$dir" && test "$(stat -c %u "$dir")" = 0 || exit 1
done
plain() {{ test -f "$1" && test ! -L "$1" && test "$(readlink -f "$1")" = "$1" && test "$(stat -c '%u:%h' "$1")" = 0:1; }}
plain "$root/run-id" && test "$(stat -c %s "$root/run-id")" -eq {len(run) + 1} && printf '%s\\n' {run} | cmp -s - "$root/run-id" || exit 1
leaf="$root/data/{leaf}"
plain "$leaf" || exit 1
size=$(stat -c %s "$leaf"); test "$size" -gt 0 && test "$size" -le {MAX_RECORD} && test "$(wc -l <"$leaf")" -eq 1 || exit 1
LC_ALL=C awk 'NR == 1 && $0 ~ /{pattern}/ {{ line=$0; valid=1 }} END {{ if (NR != 1 || !valid) exit 1; print line }}' "$leaf"'''


def collect(visit):
    batch = CONTROL.private(visit.path)
    leaf = name_for(visit.run)
    require(not present(batch, 'released.json') and not present(batch, leaf), 'Display capture was closed or already collected')
    pid = NATIVE.natural_pid(batch)
    card = read(batch, 'card.json')
    require(type(card['process_id']) is int and (card['run_id'], card['boot_id'], card['process_id']) == (visit.run, visit.boot, pid),
            'Display capture does not have a run-bound card and independent UI PID')
    CONTROL.plain(batch / (visit.run + '.jsonl'), CONTROL.MAX_TRACE)
    record = decode(visit.raw(capture_command(visit.run, visit.route, visit.boot)), visit.run, visit.boot, pid)
    CONTROL.save(batch / leaf, record)


def binding(batch, run, *, verified):
    record = read(batch, 'display-visit.json')
    ordinal = record['ordinal']
    require(type(ordinal) is int and 1 <= ordinal <= 3 and record == {'schema': SCHEMA, 'treatment': 'frozen-stock-display-sampling',
            'ordinal': ordinal, 'run': run}, 'Wrong frozen display case or treatment')
    if verified or present(batch, 'display-inputs-verified.json'):
        receipt = read(batch, 'display-inputs-verified.json')
        require(type(receipt['ordinal']) is int and receipt == {'schema': 'bili-display-sampling-inputs-v1', 'ordinal': ordinal, 'run_id': run,
                'source': 'frozen-installed-control', 'comparison': 'complete-three-archive-byte-comparison'}, 'Display archive admission not verified')
    return ordinal


def blank(ordinal, status):
    return {'ordinal': ordinal, 'status': status, 'helper': None, 'card_to_shelf_renderer_interval_ns': None,
            'card_to_accepted_send_interval_ns': None, 'card_to_same_marker_driver_interval_ns': None}


def closed(batch, run, host_boot):
    ordinal = binding(batch, run, verified=True)
    disposition = NATIVE.closure(batch, run, host_boot)
    result = blank(ordinal, 'closed-without-eligible-shelf')
    boot = read(batch, 'origin.json')['boot'] if present(batch, 'origin.json') else None
    require(boot is None and disposition == 'no-device-change' or type(boot) is str and CONTROL.UUID.fullmatch(boot), 'Invalid display visit boot')
    if disposition == 'no-device-change' or not present(batch, 'click-intended'):
        require(present(batch, 'failure.json') and not any(present(batch, name) for name in ('retired.json', 'observed-click.json',
                'card.json', run + '.jsonl', name_for(run))), 'Conflicting display evidence before launch')
        return result, boot
    retired = read(batch, 'retired.json')
    require(set(retired) == {'natural', 'app_pid'} and type(retired['natural']) is bool and retired['natural'] is not present(batch, 'stop-intended')
            and (retired['app_pid'] is None or retired['natural'] and NATIVE.positive(retired['app_pid'])), 'Unresolved display visit retirement')
    if not retired['natural'] or retired['app_pid'] is None or present(batch, 'failure.json') or present(batch, 'partial-trace'):
        return result, boot
    if not all(present(batch, name) for name in ('observed-click.json', 'card.json', run + '.jsonl')):
        return result, boot
    pid, card = NATIVE.natural_pid(batch), read(batch, 'card.json')
    require(type(card['process_id']) is int and (card['run_id'], card['boot_id'], card['process_id']) == (run, boot, pid)
            and read(batch, 'observed-click.json') == {'run_id': run, 'boot_id': boot, 'click': card['click']}, 'Foreign display visit click')
    rows = BOUNDS.narrow_rows(BOUNDS.read_jsonl(CONTROL.plain(batch / (run + '.jsonl'), CONTROL.MAX_TRACE)), run, boot, process_id=pid)
    shelf = BOUNDS.audit(card, rows)
    frames = [row['data'] for row in rows if row['stage'] == 'shelf_rendered']
    if len(frames) != 1 or frames[0]['shelf_state'] != 'ready' or shelf['render_order'] == 'initial_fill':
        return result, boot
    result.update(status='eligible-shelf-no-capture-breakdown', card_to_shelf_renderer_interval_ns=shelf['renderer_return_interval_ns'],
                  card_to_accepted_send_interval_ns=shelf['accepted_send_interval_ns'], card_to_same_marker_driver_interval_ns=shelf['driver_completion_interval_ns'])
    if present(batch, name_for(run)):
        record = decode(CONTROL.plain(batch / name_for(run), MAX_RECORD).read_bytes(), run, boot, pid)
        result.update(status='observed', helper=components(record, card['click']['up_after_ns'], rows[0]['monotonic_ns']))
    return result, boot


def validate_closed(batch_path):
    batch = CONTROL.private(Path(batch_path))
    admission = read(batch, 'admission.json', 65536)
    row, boot = closed(batch, admission['run'], admission['host_boot'])
    require(row['status'] == 'observed', 'No qualified display decomposition; stop without retry')
    return boot


def analyze(plan):
    require(type(plan) is dict and set(plan) == {'schema', 'owner', 'host_boot', 'cases'} and plan['schema'] == SCHEMA
            and plan['owner'] == CONTROL.configured_owner() and CONTROL.UUID.fullmatch(plan['host_boot']) and type(plan['cases']) is list
            and len(plan['cases']) == 3, 'Invalid three-visit display plan')
    visits, seen, last_boot, stopped = [], set(), None, False
    for ordinal, case in enumerate(plan['cases'], 1):
        run = case['run']
        require(set(case) == {'ordinal', 'run', 'batch'} and type(case['ordinal']) is int and case['ordinal'] == ordinal
                and CONTROL.RUN.fullmatch(run) and run not in seen and case['batch'] == str(CONTROL.HOST / ('bilicomics-interactive-' + run)),
                'Repeated or misplaced display visit')
        seen.add(run)
        batch = CONTROL.private(Path(case['batch']))
        admission = read(batch, 'admission.json', 65536)
        require((admission['run'], admission['owner'], admission['host_boot']) == (run, plan['owner'], plan['host_boot'])
                and binding(batch, run, verified=False) == ordinal, 'Display visit and original plan differ')
        if not present(batch, 'released.json'):
            names = {entry.name for entry in batch.iterdir()}
            require(names in (FRESH, FRESH | {'display-inputs-verified.json'}), 'Attempted or unknown visit has no exact release')
            stopped = True
            row = blank(ordinal, 'not-attempted')
        else:
            require(not stopped, 'Visit follows an incomplete or unattempted display slot')
            row, boot = closed(batch, run, plan['host_boot'])
            require(boot is None or last_boot in (None, boot), 'Device reboot between display visits')
            if boot is not None:
                last_boot = boot
            stopped = row['status'] != 'observed'
        visits.append(row)
    measured = [row['helper'] for row in visits if row['status'] == 'observed']
    by_branch = {branch: sum(item['query_branch'] == branch for item in measured) for branch in ('automatic-five', 'fixed-three')}
    return {'schema': 'bili-display-sampling-observations-v1', 'planned': 3, 'observed': len(measured),
            'counts_by_query_branch': by_branch,
            'complete_series_medians': {'query_branch': measured[0]['query_branch'], 'observations': 3,
                 'helper_elapsed_ns': median([row['helper_elapsed_ns'] for row in measured]),
                 'process_stat_reads': median([row['process_stat_reads'] for row in measured]),
                 'buckets': {key: {field: median([row['buckets'][key][field] for row in measured]) for field in ('count', 'elapsed_ns')}
                             for key in sorted(BUCKETS)},
                 'residual_ns': {key: median([row['residual_ns'][key] for row in measured]) for key in measured[0]['residual_ns']}}
                if 3 in by_branch.values() else None,
            'visits': visits, 'scope': 'Single diagnostic baseline: counts and wall intervals are not a speedup, a pure hardware cost or optical timing.'}


if __name__ == '__main__':
    cli = argparse.ArgumentParser(description=__doc__)
    cli.add_argument('--plan', type=Path, required=True)
    try:
        path = cli.parse_args().plan
        CONTROL.private(path.parent)
        print(json.dumps(analyze(read(path.parent, path.name, 65536)), indent=2, allow_nan=False))
    except (ValueError, TypeError, KeyError, OSError, UnicodeError):
        cli.exit(1, 'Unverified display measurement, closure or evidence; no output emitted.\n')
