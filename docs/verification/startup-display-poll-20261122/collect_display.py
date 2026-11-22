"""Qualify three closed startup visits and partition the existing command poll wall interval."""

import argparse
import importlib.util
import json
from pathlib import Path
from statistics import median


HERE = Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location('frozen_display_cost', HERE.parent / 'startup-display-sampling-20261122/collect_display.py')
BASE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(BASE)
CONTROL, BOUNDS, NATIVE = BASE.CONTROL, BASE.BOUNDS, BASE.NATIVE
require, read, present = BASE.require, BASE.read, BASE.present
SCHEMA = 'bili-display-poll-v1'
WIRE_SCHEMA = 'kindle-startup-display-poll-v1'
CHILDREN = ('command_poll_stdout_open', 'command_poll_after_eof')
WIRE_BUCKETS = BASE.WIRE_BUCKETS + CHILDREN
WIRE_IDENTITIES = BASE.WIRE_IDENTITIES
MAX_RECORD = BASE.MAX_RECORD
FRESH = BASE.FRESH - {'display-visit.json'} | {'poll-visit.json'}


def name_for(run):
    require(type(run) is str and CONTROL.RUN.fullmatch(run), 'Invalid display poll run')
    return run + '.display-poll.json'


def capture_command(run, route, boot):
    leaf = name_for(run)
    require(route == '/mnt/us/' + run and type(boot) is str and CONTROL.UUID.fullmatch(boot), 'Unexpected display poll stage or boot')
    number = '(0|[1-9][0-9]*)'
    identities = ','.join(f'"{key}":{number}' for key in WIRE_IDENTITIES)
    fields = ','.join(fr'"{key}":\{{"elapsed_ns":{number},"count":{number}\}}' for key in WIRE_BUCKETS)
    pattern = fr'^\{{"schema":"{WIRE_SCHEMA}","clock":"CLOCK_MONOTONIC","run_id":"{run}","boot_id":"{boot}",{identities},"buckets":\{{{fields}\}}\}}$'
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


def decode(raw, run, boot, ui_pid):
    require(type(raw) is bytes and 0 < len(raw) <= MAX_RECORD and raw.endswith(b'\n') and len(raw.splitlines()) == 1,
            'Missing or incomplete display poll record')
    try:
        row = json.loads(raw, object_pairs_hook=BOUNDS.unique_object, parse_constant=BOUNDS.reject_constant)
        require(type(row) is dict and set(row) == BASE.PRIVATE_FIELDS and row['schema'] == WIRE_SCHEMA and type(row['buckets']) is dict
                and set(row['buckets']) == set(WIRE_BUCKETS), 'Wrong display poll schema or buckets')
        for key in CHILDREN:
            sample = row['buckets'][key]
            require(type(sample) is dict and set(sample) == {'count', 'elapsed_ns'} and BASE.count32(sample['count']) and BASE.natural(sample['elapsed_ns'])
                    and (sample['count'] != 0 or sample['elapsed_ns'] == 0), 'Invalid display poll child')
        common = {**row, 'schema': 'kindle-startup-display-cost-v1', 'buckets': {key: row['buckets'][key] for key in BASE.WIRE_BUCKETS}}
        common_bytes = (json.dumps(common, separators=(',', ':'), allow_nan=False) + '\n').encode('ascii')
        BASE.decode(common_bytes, run, boot, ui_pid)
    except (KeyError, TypeError, ValueError, UnicodeError):
        raise CONTROL.OperatorError('Invalid display poll receipt; private content withheld') from None
    return row


def components(record, release_after, trace_header):
    result = BASE.components(record, release_after, trace_header)
    buckets = result['buckets']
    parent = buckets['command_poll']
    require(all(sum(buckets[key][field] for key in CHILDREN) == parent[field] for field in ('count', 'elapsed_ns')),
            'Display poll children do not partition the original count and wall interval exactly')
    result['poll_partition'] = {'parent': dict(parent)}
    for key in CHILDREN:
        item = buckets[key]
        result['poll_partition'][key.removeprefix('command_poll_')] = {**item,
            'share_of_poll_count': item['count'] / parent['count'] if parent['count'] else None,
            'share_of_poll_elapsed': item['elapsed_ns'] / parent['elapsed_ns'] if parent['elapsed_ns'] else None}
    return result


def collect(visit):
    batch = CONTROL.private(visit.path)
    leaf = name_for(visit.run)
    require(not present(batch, 'released.json') and not present(batch, leaf), 'Display poll capture is not fresh')
    pid = NATIVE.natural_pid(batch)
    card = read(batch, 'card.json')
    require(type(card['process_id']) is int and (card['run_id'], card['boot_id'], card['process_id']) == (visit.run, visit.boot, pid),
            'Display poll capture lacks the original card and independently witnessed UI process')
    CONTROL.plain(batch / (visit.run + '.jsonl'), CONTROL.MAX_TRACE)
    CONTROL.save(batch / leaf, decode(visit.raw(capture_command(visit.run, visit.route, visit.boot)), visit.run, visit.boot, pid))


def binding(batch, run, *, verified):
    ticket = read(batch, 'poll-visit.json')
    ordinal = ticket['ordinal']
    require(type(ordinal) is int and 1 <= ordinal <= 3 and ticket == {'schema': SCHEMA, 'treatment': 'frozen-stock-display-poll',
            'ordinal': ordinal, 'run': run}, 'Unbound poll study slot')
    if verified or present(batch, 'poll-inputs-verified.json'):
        inputs = read(batch, 'poll-inputs-verified.json')
        require(type(inputs['ordinal']) is int and inputs == {'schema': 'bili-display-poll-inputs-v1', 'ordinal': ordinal, 'run_id': run,
                'source': 'frozen-installed-control', 'comparison': 'complete-three-archive-byte-comparison'}, 'Unverified poll study inputs')
    return ordinal


def closed(batch, run, host_boot):
    ordinal = binding(batch, run, verified=True)
    disposition = NATIVE.closure(batch, run, host_boot)
    result = BASE.blank(ordinal, 'closed-without-eligible-shelf')
    boot = read(batch, 'origin.json')['boot'] if present(batch, 'origin.json') else None
    require(boot is None and disposition == 'no-device-change' or type(boot) is str and CONTROL.UUID.fullmatch(boot), 'Invalid poll study device boot')
    if disposition == 'no-device-change' or not present(batch, 'click-intended'):
        require(present(batch, 'failure.json') and not any(present(batch, name) for name in ('retired.json', 'observed-click.json',
                'card.json', run + '.jsonl', name_for(run))), 'Unexpected poll evidence without launch')
        return result, boot
    retired = read(batch, 'retired.json')
    require(set(retired) == {'natural', 'app_pid'} and type(retired['natural']) is bool and retired['natural'] is not present(batch, 'stop-intended')
            and (retired['app_pid'] is None or retired['natural'] and NATIVE.positive(retired['app_pid'])), 'Unconfirmed application retirement')
    if not retired['natural'] or retired['app_pid'] is None or present(batch, 'failure.json') or present(batch, 'partial-trace'):
        return result, boot
    if not all(present(batch, name) for name in ('observed-click.json', 'card.json', run + '.jsonl')):
        return result, boot
    pid, card = NATIVE.natural_pid(batch), read(batch, 'card.json')
    require(type(card['process_id']) is int and (card['run_id'], card['boot_id'], card['process_id']) == (run, boot, pid)
            and read(batch, 'observed-click.json') == {'run_id': run, 'boot_id': boot, 'click': card['click']}, 'Foreign poll-study origin')
    trace = BOUNDS.narrow_rows(BOUNDS.read_jsonl(CONTROL.plain(batch / (run + '.jsonl'), CONTROL.MAX_TRACE)), run, boot, process_id=pid)
    shelf = BOUNDS.audit(card, trace)
    frames = [row['data'] for row in trace if row['stage'] == 'shelf_rendered']
    if len(frames) != 1 or frames[0]['shelf_state'] != 'ready' or shelf['render_order'] == 'initial_fill':
        return result, boot
    result.update(status='eligible-shelf-no-poll-partition', card_to_shelf_renderer_interval_ns=shelf['renderer_return_interval_ns'],
                  card_to_accepted_send_interval_ns=shelf['accepted_send_interval_ns'], card_to_same_marker_driver_interval_ns=shelf['driver_completion_interval_ns'])
    if present(batch, name_for(run)):
        record = decode(CONTROL.plain(batch / name_for(run), MAX_RECORD).read_bytes(), run, boot, pid)
        result.update(status='observed', helper=components(record, card['click']['up_after_ns'], trace[0]['monotonic_ns']))
    return result, boot


def validate_closed(batch_path):
    batch = CONTROL.private(Path(batch_path))
    admission = read(batch, 'admission.json', 65536)
    row, boot = closed(batch, admission['run'], admission['host_boot'])
    require(row['status'] == 'observed', 'Unavailable poll partition; stop without replacement')
    return boot


def three_visit_medians(helpers):
    sections = {}
    for name in ('parent', 'stdout_open', 'after_eof'):
        sections[name] = {}
        for key in helpers[0]['poll_partition'][name]:
            terms = [item['poll_partition'][name][key] for item in helpers]
            sections[name][key] = median(terms) if all(term is not None for term in terms) else None
    return {'query_branch': helpers[0]['query_branch'], 'visits': 3, 'poll_partition': sections}


def analyze(plan):
    require(type(plan) is dict and set(plan) == {'schema', 'owner', 'host_boot', 'cases'} and plan['schema'] == SCHEMA
            and plan['owner'] == CONTROL.configured_owner() and CONTROL.UUID.fullmatch(plan['host_boot']) and type(plan['cases']) is list
            and len(plan['cases']) == 3, 'Invalid three-visit poll plan')
    visits, runs, last_boot, stopped = [], set(), None, False
    for ordinal, case in enumerate(plan['cases'], 1):
        run = case['run']
        require(set(case) == {'ordinal', 'run', 'batch'} and type(case['ordinal']) is int and case['ordinal'] == ordinal
                and CONTROL.RUN.fullmatch(run) and run not in runs and case['batch'] == str(CONTROL.HOST / ('bilicomics-interactive-' + run)), 'Invalid poll slot')
        runs.add(run)
        batch = CONTROL.private(Path(case['batch']))
        admission = read(batch, 'admission.json', 65536)
        require((admission['run'], admission['owner'], admission['host_boot']) == (run, plan['owner'], plan['host_boot'])
                and binding(batch, run, verified=False) == ordinal, 'Poll plan differs from original admission')
        if not present(batch, 'released.json'):
            require({entry.name for entry in batch.iterdir()} in (FRESH, FRESH | {'poll-inputs-verified.json'}), 'Unknown or attempted poll slot without release')
            stopped = True
            row = BASE.blank(ordinal, 'not-attempted')
        else:
            require(not stopped, 'Poll visit follows a stopped or unattempted slot')
            row, boot = closed(batch, run, plan['host_boot'])
            require(boot is None or last_boot in (None, boot), 'Device reboot between poll visits')
            if boot is not None:
                last_boot = boot
            stopped = row['status'] != 'observed'
        visits.append(row)
    helpers = [row['helper'] for row in visits if row['status'] == 'observed']
    counts = {branch: sum(item['query_branch'] == branch for item in helpers) for branch in ('automatic-five', 'fixed-three')}
    return {'schema': 'bili-display-poll-observations-v1', 'planned': 3, 'observed': len(helpers), 'counts_by_query_branch': counts,
            'complete_series_medians': three_visit_medians(helpers) if 3 in counts.values() else None, 'visits': visits,
            'scope': 'Partition of wall time in the existing poll syscall, not service latency, lost wakeups, savings or optical response.'}


if __name__ == '__main__':
    cli = argparse.ArgumentParser(description=__doc__)
    cli.add_argument('--plan', type=Path, required=True)
    try:
        plan_path = cli.parse_args().plan
        CONTROL.private(plan_path.parent)
        print(json.dumps(analyze(read(plan_path.parent, plan_path.name, 65536)), indent=2, allow_nan=False))
    except (ValueError, TypeError, KeyError, OSError, UnicodeError):
        cli.exit(1, 'Unverified poll plan, visit closure or data; no output emitted.\n')
