"""Capture one private Rust sidecar after retirement and audit three predeclared visits offline."""

import argparse
import importlib.util
import json
import os
from pathlib import Path


STUDY = Path(__file__).resolve().parent.parent / 'startup-interactive-kindle-20261121'


def module(name):
    spec = importlib.util.spec_from_file_location(name, STUDY / (name + '.py'))
    loaded = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(loaded)
    return loaded


CONTROL = module('shelf_operator')
BOUNDS = module('log_bounds')
require = CONTROL.require
SCHEMA = 'bili-prepared-handoff-v1'
SIDE_SCHEMA = 'kindle-prepared-handoff-v1'
LIMIT = 250_000_000
SOURCES = {'runtime_wakeup', 'maintenance_timer'}
FIELDS = {'schema', 'run_id', 'boot_id', 'process_id', 'clock', 'prepared_monotonic_ns', 'received_monotonic_ns', 'receive_source'}
PREPARED_FILES = {'admission.json', 'route.tar.gz', 'probe.tar.gz', 'native-guard.sh', 'askpass', 'handoff-visit.json'}


def read(batch, leaf, maximum=2048):
    return BOUNDS.read_json(CONTROL.plain(batch / leaf, maximum))


def present(batch, leaf):
    return os.path.lexists(batch / leaf)


def positive(value):
    return type(value) is int and 0 < value < 1 << 64


def sidecar_name(run):
    require(type(run) is str and CONTROL.RUN.fullmatch(run), 'Invalid prepared handoff run')
    return run + '.prepared-handoff.json'


def capture_command(run, route):
    name = sidecar_name(run)
    require(route == '/mnt/us/' + run, 'Wrong prepared handoff staging directory')
    return f'''root={route}
for dir in "$root" "$root/data"; do
 test -d "$dir" && test ! -L "$dir" && test "$(readlink -f "$dir")" = "$dir" && test "$(stat -c %u "$dir")" = 0 || exit 1
done
plain() {{ test -f "$1" && test ! -L "$1" && test "$(readlink -f "$1")" = "$1" && test "$(stat -c '%u:%h' "$1")" = 0:1; }}
plain "$root/run-id" && test "$(stat -c %s "$root/run-id")" -eq {len(run) + 1} && printf '%s\\n' {run} | cmp -s - "$root/run-id" || exit 1
leaf="$root/data/{name}"
plain "$leaf" || exit 1
size=$(stat -c %s "$leaf"); test "$size" -gt 0 && test "$size" -le 1024 || exit 1
head -c 1025 "$leaf"'''


def parse(data, run, boot, pid):
    require(type(data) is bytes and 0 < len(data) <= 1024 and data.endswith(b'\n') and len(data.splitlines()) == 1,
            'Missing or incomplete prepared handoff line')
    try:
        row = json.loads(data, object_pairs_hook=BOUNDS.unique_object, parse_constant=BOUNDS.reject_constant)
        require(type(row) is dict and set(row) == FIELDS and row['schema'] == SIDE_SCHEMA and row['clock'] == BOUNDS.CLOCK
                and type(run) is str and CONTROL.RUN.fullmatch(run) and type(boot) is str and CONTROL.UUID.fullmatch(boot)
                and row['run_id'] == run and row['boot_id'] == boot and positive(pid) and positive(row['process_id']) and row['process_id'] == pid
                and positive(row['prepared_monotonic_ns']) and positive(row['received_monotonic_ns'])
                and row['prepared_monotonic_ns'] <= row['received_monotonic_ns'] and row['receive_source'] in SOURCES,
                'Invalid prepared handoff schema or process binding')
    except (ValueError, KeyError, TypeError, UnicodeError):
        raise CONTROL.OperatorError('Invalid prepared handoff record; private content withheld') from None
    return row


def natural_pid(batch):
    retired = read(batch, 'retired.json')
    require(set(retired) == {'natural', 'app_pid'} and retired['natural'] is True and positive(retired['app_pid'])
            and not any(present(batch, leaf) for leaf in ('stop-intended', 'partial-trace', 'failure.json')),
            'No clean independent application PID for this handoff')
    return retired['app_pid']


def collect(visit):
    batch = CONTROL.private(visit.path)
    name = sidecar_name(visit.run)
    require(not present(batch, 'released.json') and not present(batch, name), 'Handoff collection is not fresh')
    pid = natural_pid(batch)
    card = read(batch, 'card.json')
    require((card['run_id'], card['boot_id'], card['process_id']) == (visit.run, visit.boot, pid)
            and positive(card['process_id']), 'Independent PID and original card differ')
    CONTROL.plain(batch / (visit.run + '.jsonl'), CONTROL.MAX_TRACE)
    record = parse(visit.raw(capture_command(visit.run, visit.route)), visit.run, visit.boot, pid)
    CONTROL.save(batch / name, record)


def software(batch, run, *, verified=True):
    binding = read(batch, 'handoff-visit.json')
    ordinal = binding['ordinal']
    require(type(ordinal) is int and 1 <= ordinal <= 3 and binding == {'schema': SCHEMA, 'treatment': 'frozen-stock-prepared-handoff',
            'ordinal': ordinal, 'run': run}, 'Wrong prepared handoff slot or treatment')
    if verified or present(batch, 'handoff-inputs-verified.json'):
        receipt = read(batch, 'handoff-inputs-verified.json')
        require(type(receipt['ordinal']) is int and receipt == {'schema': 'bili-prepared-handoff-inputs-v1', 'run_id': run, 'ordinal': ordinal,
                'source': 'frozen-installed-control', 'comparison': 'complete-three-archive-byte-comparison'}, 'Unverified handoff software')
    return ordinal


def closure(batch, run, host_boot):
    admission = read(batch, 'admission.json', 65536)
    release = read(batch, 'released.json')
    require(admission['run'] == release['run_id'] == run and admission['owner'] == release['owner_thread'] == CONTROL.configured_owner()
            and admission['host_boot'] == host_boot and CONTROL.UUID.fullmatch(host_boot) and not present(batch, 'private-token')
            and release['state'] == 'released' and release['owner_and_token_checked'] is True and release['native_restored_and_connections_closed'] is True
            and release['device_disposition'] in ('no-device-change', 'original-native-restored'), 'Exact handoff closure is unverified')
    return release['device_disposition']


def blank(ordinal, state):
    return {'ordinal': ordinal, 'status': state, 'receive_source': None, 'producer_to_ui_receive_ns': None, 'at_least_250_ms': None,
            'card_to_shelf_renderer_interval_ns': None, 'card_to_accepted_send_interval_ns': None, 'card_to_same_marker_driver_interval_ns': None}


def closed(batch, run, host_boot):
    CONTROL.private(batch)
    ordinal = software(batch, run)
    disposition = closure(batch, run, host_boot)
    row = blank(ordinal, 'closed-without-eligible-shelf')
    boot = read(batch, 'origin.json')['boot'] if present(batch, 'origin.json') else None
    require(boot is None and disposition == 'no-device-change' or type(boot) is str and CONTROL.UUID.fullmatch(boot), 'Invalid handoff device boot')
    if disposition == 'no-device-change' or not present(batch, 'click-intended'):
        require(not any(present(batch, leaf) for leaf in ('retired.json', 'card.json', 'observed-click.json', run + '.jsonl', sidecar_name(run)))
                and present(batch, 'failure.json'), 'Conflicting pre-launch and capture receipts')
        return row, boot
    retired = read(batch, 'retired.json')
    if retired['natural'] is False or retired['app_pid'] is None or any(present(batch, leaf) for leaf in ('failure.json', 'partial-trace')):
        require(type(retired['natural']) is bool and retired['natural'] is not present(batch, 'stop-intended') and
                (retired['app_pid'] is None or retired['natural'] and positive(retired['app_pid'])), 'Ambiguous retirement or recovery')
        return row, boot
    pid = natural_pid(batch)
    if not all(present(batch, leaf) for leaf in ('card.json', 'observed-click.json', run + '.jsonl')):
        return row, boot
    card = read(batch, 'card.json')
    observed = read(batch, 'observed-click.json')
    require(card['process_id'] == pid and positive(card['process_id']) and (card['run_id'], card['boot_id']) == (run, boot) and
            observed == {'run_id': run, 'boot_id': boot, 'click': card['click']}, 'Handoff card did not originate in this launch')
    trace = BOUNDS.narrow_rows(BOUNDS.read_jsonl(CONTROL.plain(batch / (run + '.jsonl'), CONTROL.MAX_TRACE)), run, boot, process_id=pid)
    audit = BOUNDS.audit(card, trace)
    frames = [event['data'] for event in trace if event['stage'] == 'shelf_rendered']
    if len(frames) != 1 or frames[0]['shelf_state'] != 'ready' or audit['render_order'] == 'initial_fill':
        return row, boot
    row.update(status='eligible-shelf-handoff-unavailable', card_to_shelf_renderer_interval_ns=audit['renderer_return_interval_ns'],
               card_to_accepted_send_interval_ns=audit['accepted_send_interval_ns'], card_to_same_marker_driver_interval_ns=audit['driver_completion_interval_ns'])
    if not present(batch, sidecar_name(run)):
        return row, boot
    record = parse(CONTROL.plain(batch / sidecar_name(run), 1024).read_bytes(), run, boot, pid)
    producer, received = record['prepared_monotonic_ns'], record['received_monotonic_ns']
    require(trace[0]['monotonic_ns'] <= producer <= received <= frames[0]['rendered_monotonic_ns'], 'Prepared handoff lies outside its first ready Shelf frame')
    gap = received - producer
    row.update(status='observed', receive_source=record['receive_source'], producer_to_ui_receive_ns=gap, at_least_250_ms=gap >= LIMIT)
    return row, boot


def validate_closed(batch_path):
    batch = CONTROL.private(Path(batch_path))
    admission = read(batch, 'admission.json', 65536)
    row, boot = closed(batch, admission['run'], admission['host_boot'])
    require(row['status'] == 'observed', 'Missing eligible handoff observation; do not retry or extend')
    return boot


def analyze(plan):
    require(set(plan) == {'schema', 'owner', 'host_boot', 'cases'} and plan['schema'] == SCHEMA and plan['owner'] == CONTROL.configured_owner()
            and CONTROL.UUID.fullmatch(plan['host_boot']) and type(plan['cases']) is list and len(plan['cases']) == 3, 'Invalid three-visit handoff plan')
    rows, seen, device_boot, ended = [], set(), None, False
    for ordinal, case in enumerate(plan['cases'], 1):
        run = case['run']
        require(set(case) == {'ordinal', 'run', 'batch'} and type(case['ordinal']) is int and case['ordinal'] == ordinal
                and CONTROL.RUN.fullmatch(run) and run not in seen and case['batch'] == str(CONTROL.HOST / ('bilicomics-interactive-' + run)),
                'Duplicate or misplaced handoff visit')
        seen.add(run)
        batch = CONTROL.private(Path(case['batch']))
        admission = read(batch, 'admission.json', 65536)
        require(admission['run'] == run and admission['owner'] == plan['owner'] and admission['host_boot'] == plan['host_boot']
                and software(batch, run, verified=False) == ordinal, 'Handoff plan and visit do not match')
        if not present(batch, 'released.json'):
            names = {leaf.name for leaf in batch.iterdir()}
            require(names == PREPARED_FILES or names == PREPARED_FILES | {'handoff-inputs-verified.json'},
                    'An attempted or unknown handoff visit has no exact release')
            ended = True
            row = blank(ordinal, 'not-attempted')
        else:
            require(not ended, 'A handoff visit follows a stopped or unattempted slot')
            row, boot = closed(batch, run, plan['host_boot'])
            require(boot is None or device_boot in (None, boot), 'Handoff study crossed a device reboot')
            if boot is not None:
                device_boot = boot
            ended = row['status'] != 'observed'
        rows.append(row)
    measured = [row for row in rows if row['status'] == 'observed']
    hits = sum(row['at_least_250_ms'] for row in measured)
    return {'schema': 'bili-prepared-handoff-observations-v1', 'threshold_ns': LIMIT, 'planned_visits': 3, 'eligible_observations': len(measured),
            'at_or_above_threshold': hits, 'answer': 'observed' if hits else 'not-observed-in-three' if len(measured) == 3 else 'inconclusive',
            'visits': rows, 'scope': 'One diagnostic build and offline Shelf seed. The gap is not a saving, an optical delay or proof of a lost wake.'}


if __name__ == '__main__':
    cli = argparse.ArgumentParser(description=__doc__)
    cli.add_argument('--plan', type=Path, required=True)
    try:
        plan_path = cli.parse_args().plan
        CONTROL.private(plan_path.parent)
        print(json.dumps(analyze(read(plan_path.parent, plan_path.name, 65536)), indent=2, allow_nan=False))
    except (ValueError, TypeError, KeyError, OSError, UnicodeError):
        cli.exit(1, 'Unverified handoff plan, release or measurement; no result emitted.\n')
