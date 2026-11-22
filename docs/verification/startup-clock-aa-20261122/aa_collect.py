"""Capture narrow clocks after an A/A visit and normalize only verified closed visits."""

import importlib.util
import argparse
import json
import os
from pathlib import Path
import re
import stat


HERE = Path(__file__).resolve().parent
STUDY = HERE.parent / 'startup-interactive-kindle-20261121'


def load_module(path):
    spec = importlib.util.spec_from_file_location(path.stem, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


CONTROL = load_module(STUDY / 'shelf_operator.py')
BOUNDS = load_module(STUDY / 'log_bounds.py')
require = CONTROL.require
SIDECAR = 'clock-projection.json'
SCHEMA = 'bili-shelf-aa-visit-clocks-v1'
SERIES = 'bili-shelf-clock-aa-projection-v1'
SCHEDULE = tuple((pair, arm) for pair, order in enumerate(('AB', 'AB', 'BA', 'AB', 'BA')) for arm in order)
LIMIT = (1 << 63) - 1
RAW_FIELDS = {'schema', 'run_id', 'boot_id', 'clock', 'app_pid', 'shim_pid', 'shim_start_ticks',
              'launch_entry', 'pre_app_invocation', 'rust_main_entry', 'state_produced'}


def read_json(path, maximum):
    return BOUNDS.read_json(CONTROL.plain(path, maximum))


def number(value):
    return type(value) is int and 0 < value <= LIMIT


def capture_command(run, route, boot):
    require(type(run) is str and CONTROL.RUN.fullmatch(run) and route == '/mnt/us/' + run
            and type(boot) is str and CONTROL.UUID.fullmatch(boot), 'Invalid A/A device capture binding')
    script = r'''test "$(cat /proc/sys/kernel/random/boot_id)" = @BOOT@ || exit 1
root=@ROOT@
for dir in "$root" "$root/picked" "$root/data"; do
    test -d "$dir" && test ! -L "$dir" && test "$(readlink -f "$dir")" = "$dir" && test "$(stat -c %u "$dir")" = 0 || exit 1
done
plain() {
    test -f "$1" && test ! -L "$1" && test "$(readlink -f "$1")" = "$1" && test "$(stat -c '%u:%h' "$1")" = 0:1 || return 1
    size=$(stat -c %s "$1") && test "$size" -gt 0 && test "$size" -le "$2"
}
plain "$root/run-id" 128 && printf '%s\n' @RUN@ | cmp -s - "$root/run-id" || exit 1
entry="$root/picked/entry"
plain "$entry" 96 && IFS=' ' read -r name pid birth extra <"$entry" && test "$name" = @RUN@ && test -z "$extra" || exit 1
case "$pid" in ''|0*|*[!0-9]*) exit 1;; esac
case "$birth" in ''|0*|*[!0-9]*) exit 1;; esac
printf '%s %s %s\n' @RUN@ "$pid" "$birth" | cmp -s - "$entry" || exit 1
printf 'picked %s %s\n' "$pid" "$birth"
mark() {
    plain "$1" 256 && test "$(wc -l <"$1")" -eq 1 || return 1
    LC_ALL=C awk -v run=@RUN@ -v stage="$2" '
      BEGIN { prefix="{\"schema\":\"bili-startup-clock-mark-v1\",\"run_id\":\"" run "\",\"clock\":\"CLOCK_MONOTONIC\",\"stage\":\"" stage "\",\"pid\":" }
      { tail=substr($0,length(prefix)+1); if (NR != 1 || index($0,prefix) != 1 || tail !~ /^[1-9][0-9]*,"monotonic_ns":[1-9][0-9]*}$/) exit 1;
        sub(/}$/, "", tail); split(tail,fields,/,"monotonic_ns":/); printf "%s %s %s\n",stage,fields[1],fields[2]; }
      END { if (NR != 1) exit 1; }' "$1"
}
mark "$root/picked/shim-entry.json" shim_entry && mark "$root/picked/app-preinvoke.json" app_preinvoke || exit 1
life="$root/data/@RUN@-lifecycle.jsonl"
plain "$life" 262144 || exit 1
LC_ALL=C awk '
    index($0,"\"stage\":\"process_enter\"") || index($0,"\"stage\":\"state_prepared\"") {
        if ($0 !~ /^\{"elapsed_ms":[0-9]+([.][0-9]+)?([eE][+-]?[0-9]+)?,"event":"startup-stage","monotonic_ns":[1-9][0-9]*,"process_id":[1-9][0-9]*,"registered_fonts":null,"stage":"(process_enter|state_prepared)","timing_scope":"Application preparation or UI rendering; not physical panel completion"}$/) exit 1;
        split($0,fields,","); stamp=fields[3]; sub(/^"monotonic_ns":/,"",stamp); pid=fields[4]; sub(/^"process_id":/,"",pid);
        if (fields[6] == "\"stage\":\"process_enter\"") { stage="process_enter"; ++entry; if (entry != 1 || fields[1] !~ /:0[.]0$/) exit 1; }
        else { stage="state_prepared"; ++prepared; if (prepared != 1) exit 1; }
        printf "%s %s %s\n",stage,pid,stamp;
    }
    END { if (entry != 1) exit 1; }' "$life" || exit 1
test "$(cat /proc/sys/kernel/random/boot_id)" = @BOOT@'''
    return script.replace('@RUN@', run).replace('@ROOT@', route).replace('@BOOT@', boot)


def parse_capture(data, run, boot, app_pid):
    require(type(data) is bytes and 0 < len(data) <= 512 and data.endswith(b'\n')
            and CONTROL.RUN.fullmatch(run) and CONTROL.UUID.fullmatch(boot) and number(app_pid), 'Invalid A/A numeric receipt')
    rows = {}
    for line in data[:-1].split(b'\n'):
        match = re.fullmatch(rb'(picked|shim_entry|app_preinvoke|process_enter|state_prepared) ([1-9][0-9]{0,9}) ([1-9][0-9]{0,18})', line)
        require(match is not None, 'Invalid A/A numeric record')
        key = match[1].decode('ascii')
        require(key not in rows, 'Repeated A/A numeric record')
        rows[key] = (int(match[2]), int(match[3]))
    required = {'picked', 'shim_entry', 'app_preinvoke', 'process_enter'}
    require(required <= rows.keys() <= required | {'state_prepared'} and all(number(value) for pair in rows.values() for value in pair),
            'Incomplete or out-of-range A/A clocks')
    shim_pid, birth = rows['picked']
    require(rows['shim_entry'][0] == shim_pid != app_pid and all(rows[key][0] == app_pid for key in
            ('app_preinvoke', 'process_enter', 'state_prepared') if key in rows), 'A/A clocks do not match independent process identities')
    return {'schema': SCHEMA, 'run_id': run, 'boot_id': boot, 'clock': BOUNDS.CLOCK, 'app_pid': app_pid,
            'shim_pid': shim_pid, 'shim_start_ticks': birth, 'launch_entry': rows['shim_entry'][1],
            'pre_app_invocation': rows['app_preinvoke'][1], 'rust_main_entry': rows['process_enter'][1],
            'state_produced': rows['state_prepared'][1] if 'state_prepared' in rows else None}


def visit_identity(batch, run, boot):
    retired = read_json(batch / 'retired.json', 512)
    card = read_json(batch / 'card.json', 2048)
    require(not any(os.path.lexists(batch / name) for name in ('stop-intended', 'partial-trace', 'failure.json')) and
            set(retired) == {'app_pid', 'natural'} and retired['natural'] is True and number(retired['app_pid']) and
            card['run_id'] == run and card['boot_id'] == boot and type(card['process_id']) is int and card['process_id'] == retired['app_pid'],
            'A/A visit has no run-bound origin and naturally retired independent application witness')
    return retired['app_pid'], card


def collect(visit):
    batch = CONTROL.private(visit.path)
    require(not (batch / 'released.json').exists() and not os.path.lexists(batch / SIDECAR), 'A/A capture is not fresh')
    pid, _ = visit_identity(batch, visit.run, visit.boot)
    CONTROL.plain(batch / (visit.run + '.jsonl'), CONTROL.MAX_TRACE)
    command = capture_command(visit.run, visit.route, visit.boot)
    projection = parse_capture(visit.raw(command), visit.run, visit.boot, pid)
    CONTROL.save(batch / SIDECAR, projection)


def software_binding(batch, run):
    binding = read_json(batch / 'clock-visit.json', 1024)
    verified = read_json(batch / 'clock-inputs-verified.json', 1024)
    pair, arm = binding['pair'], binding['arm']
    require(type(pair) is int and (pair, arm) in SCHEDULE and
            binding == {'schema': 'bili-shelf-clock-aa-v1', 'treatment': 'frozen-stock-clock', 'pair': pair, 'arm': arm, 'run': run}
            and type(verified['pair']) is int and verified == {'schema': 'bili-shelf-clock-inputs-v1', 'run_id': run,
            'pair': pair, 'arm': arm, 'source': 'frozen-installed-control', 'comparison': 'complete-ten-archive-byte-comparison'},
            'A/A visit lacks the frozen treatment and pre-acquisition archive comparison receipts')
    return pair, arm


def closure(batch, *, native_boot_required=True):
    CONTROL.private(batch)
    admission = read_json(batch / 'admission.json', 65536)
    run = admission['run']
    boot = read_json(batch / 'origin.json', 512)['boot'] if native_boot_required or os.path.lexists(batch / 'origin.json') else None
    release = read_json(batch / 'released.json', 2048)
    require(type(run) is str and CONTROL.RUN.fullmatch(run) and (type(boot) is str and CONTROL.UUID.fullmatch(boot)
            or boot is None and not native_boot_required and release['device_disposition'] == 'no-device-change')
            and type(admission['host_boot']) is str and CONTROL.UUID.fullmatch(admission['host_boot'])
            and not os.path.lexists(batch / 'private-token') and admission['owner'] == CONTROL.configured_owner()
            and (release['owner_thread'], release['run_id']) == (admission['owner'], run) and release['state'] == 'released'
            and release['device_disposition'] in ('original-native-restored', 'no-device-change') and release['owner_and_token_checked'] is True
            and release['native_restored_and_connections_closed'] is True, 'A/A normalization requires exact release and native restoration')
    return run, boot, admission['host_boot'], release['device_disposition']


def normalized(batch):
    run, boot, host_boot, disposition = closure(batch)
    require(disposition == 'original-native-restored', 'A/A launch has no confirmed native return')
    pid, card = visit_identity(batch, run, boot)
    assignment = software_binding(batch, run)
    observed = read_json(batch / 'observed-click.json', 2048)
    require(observed == {'run_id': run, 'boot_id': boot, 'click': card['click']}, 'A/A origin does not match the original card release')
    projection = read_json(batch / SIDECAR, 2048)
    require(set(projection) == RAW_FIELDS and (projection['schema'], projection['clock'], projection['run_id'], projection['boot_id'])
            == (SCHEMA, BOUNDS.CLOCK, run, boot) and number(projection['app_pid']) and projection['app_pid'] == pid
            and number(projection['shim_pid']) and projection['shim_pid'] != pid and number(projection['shim_start_ticks'])
            and all(number(projection[key]) for key in ('launch_entry', 'pre_app_invocation', 'rust_main_entry'))
            and (projection['state_produced'] is None or number(projection['state_produced'])), 'Unbound or invalid A/A clock sidecar')
    trace = CONTROL.plain(batch / (run + '.jsonl'), CONTROL.MAX_TRACE)
    rows = BOUNDS.narrow_rows(BOUNDS.read_jsonl(trace), run, boot, process_id=pid)
    audit = BOUNDS.audit(card, rows)
    frames = [row['data'] for row in rows if row['stage'] == 'shelf_rendered']
    require(len(frames) == 1 and frames[0]['shelf_state'] in ('ready', 'empty') and audit['renderer_return_interval_ns'] is not None
            and audit['render_order'] in (None, 'wait_then_render', 'render_then_wait'),
            'A/A launch lacks the qualified seeded Shelf return')
    clocks = {'release_before': card['click']['up_before_ns'], 'release_after': card['click']['up_after_ns'],
              'launch_entry': projection['launch_entry'], 'pre_app_invocation': projection['pre_app_invocation'],
              'rust_main_entry': projection['rust_main_entry'], 'trace_started': rows[0]['monotonic_ns'],
              'shelf_rendered': audit['landmarks_ns']['shelf_rendered'], 'shelf_submitted': audit['landmarks_ns']['shelf_submitted'],
              'driver_completed': audit['landmarks_ns']['shelf_driver_completed'],
              'state_produced': projection['state_produced'], 'state_consumed': None}
    ordered = [clocks[key] for key in ('release_before', 'release_after', 'launch_entry', 'pre_app_invocation', 'rust_main_entry',
                                       'trace_started', 'state_produced', 'shelf_rendered', 'shelf_submitted', 'driver_completed')
               if clocks[key] is not None]
    require(all(number(value) for value in ordered) and ordered == sorted(ordered), 'A/A monotonic landmark ordering failed')
    evidence = {'same_run': True, 'same_boot': True, 'same_process': True, 'same_launcher_and_elf': True, 'qualified_shelf': True,
                'accepted_send': clocks['shelf_submitted'] is not None, 'same_marker_driver': clocks['driver_completed'] is not None,
                'natural_exit': True, 'native_restored': True, 'lease_released': True}
    return run, boot, {'clocks': clocks, 'evidence': evidence}, assignment, host_boot


def normalize_closed(batch_path):
    return normalized(Path(batch_path))[2]


def validate_closed(batch_path):
    return normalized(Path(batch_path))[1]


def flag_present(batch, name):
    path = batch / name
    if not os.path.lexists(path):
        return False
    info = path.lstat()
    require(stat.S_ISREG(info.st_mode) and info.st_nlink == 1 and info.st_uid == 0 and info.st_size == 0
            and not info.st_mode & 0o022, 'Unsafe A/A progress marker')
    return True


def terminal_without_sample(batch, case, expected_host, expected_device):
    run, boot, host_boot, disposition = closure(batch, native_boot_required=False)
    require(case['run'] == run and software_binding(batch, run) == (case['pair'], case['arm']) and expected_host in (None, host_boot)
            and (boot is None or expected_device in (None, boot)), 'Terminal A/A visit belongs to a different plan or boot')
    if not flag_present(batch, 'attempted'):
        return False
    sent = flag_present(batch, 'click-intended')
    failure = os.path.lexists(batch / 'failure.json')
    if failure:
        record = read_json(batch / 'failure.json', 2048)
        require(set(record) == {'phase', 'reason'} and record['phase'] in ('native-preflight', 'media-capacity', 'stage-detector',
                'stage-route', 'stock-inspect', 'native-navigation', 'identify-card', 'route-activate', 'card-release')
                and type(record['reason']) is str, 'Terminal A/A failure is not a bound measurement phase')
    if not sent:
        return failure and not any(os.path.lexists(batch / name) for name in
               ('retired.json', 'stop-intended', 'observed-click.json', 'card.json', run + '.jsonl', SIDECAR, 'intervals.json'))
    if disposition != 'original-native-restored':
        return False
    retired = read_json(batch / 'retired.json', 512)
    stopped = flag_present(batch, 'stop-intended')
    require(set(retired) == {'natural', 'app_pid'} and type(retired['natural']) is bool and retired['natural'] is not stopped
            and (retired['app_pid'] is None or not stopped and number(retired['app_pid'])), 'Ambiguous terminal application retirement')
    if not flag_present(batch, 'capture-attempted'):
        return False
    required = ('card.json', 'observed-click.json', run + '.jsonl', SIDECAR)
    if retired['app_pid'] is None or failure or flag_present(batch, 'partial-trace') or any(not os.path.lexists(batch / name) for name in required):
        return True
    pid, card = visit_identity(batch, run, boot)
    rows = BOUNDS.narrow_rows(BOUNDS.read_jsonl(CONTROL.plain(batch / (run + '.jsonl'), CONTROL.MAX_TRACE)), run, boot, process_id=pid)
    audit = BOUNDS.audit(card, rows)
    return audit['renderer_return_interval_ns'] is None or any(row['stage'] == 'shelf_rendered' and
           (row['data']['shelf_state'] == 'signed_out' or row['data'].get('render_order') == 'initial_fill') for row in rows)


def series_projection(cases, *, host_boot=None):
    require(type(cases) is list and len(cases) == len(SCHEDULE), 'A/A projection requires the complete predeclared plan')
    require(host_boot is None or type(host_boot) is str and CONTROL.UUID.fullmatch(host_boot), 'Invalid prepared host boot')
    rows, seen, origin, previous_end, device_boot, done = [], set(), None, None, None, False
    for case, assignment in zip(cases, SCHEDULE):
        require(type(case) is dict and set(case) == {'pair', 'arm', 'run', 'batch'} and type(case['pair']) is int
                and (case['pair'], case['arm']) == assignment and type(case['run']) is str and CONTROL.RUN.fullmatch(case['run'])
                and case['run'] not in seen and case['batch'] == str(CONTROL.HOST / ('bilicomics-interactive-' + case['run'])),
                'Repeated, misplaced or unbound A/A slot')
        seen.add(case['run'])
        batch = CONTROL.private(Path(case['batch']))
        if not os.path.lexists(batch / 'released.json'):
            untouched = {'admission.json', 'clock-visit.json', 'clock-inputs-verified.json', 'route.tar.gz', 'probe.tar.gz',
                         'native-guard.sh', 'askpass'}
            require({leaf.name for leaf in batch.iterdir()} == untouched, 'Attempted or unrecognized A/A slot has no exact release')
            pending = read_json(batch / 'admission.json', 65536)
            require(pending['run'] == case['run'] and pending['owner'] == CONTROL.configured_owner() and CONTROL.UUID.fullmatch(pending['host_boot'])
                    and host_boot in (None, pending['host_boot']) and software_binding(batch, case['run']) == assignment,
                    'Untouched A/A suffix no longer matches its prepared slot')
            done = True
            continue
        require(not done, 'A/A plan contains a closed visit after an unattempted slot')
        try:
            run, boot, row, bound_assignment, prepared_boot = normalized(batch)
        except (FileNotFoundError, ValueError, KeyError, TypeError):
            if not terminal_without_sample(batch, case, host_boot, device_boot):
                raise
            done = True
            continue
        require(case['run'] == run and bound_assignment == assignment and device_boot in (None, boot) and host_boot in (None, prepared_boot),
                'A/A run, slot, host or device boot changed')
        clocks = row['clocks']
        require(previous_end is None or previous_end < clocks['release_before'], 'A/A visit clocks overlap or repeat')
        previous_end = max(value for value in clocks.values() if value is not None)
        if origin is None:
            origin, device_boot, host_boot = clocks['release_before'], boot, prepared_boot
        offsets = {key: value - origin if value is not None else None for key, value in clocks.items()}
        rows.append({'pair': case['pair'], 'arm': case['arm'], 'clock_offset_ns': offsets, 'evidence': row['evidence']})
    return {'schema': SERIES, 'clock': BOUNDS.CLOCK, 'cases': rows}


if __name__ == '__main__':
    cli = argparse.ArgumentParser(description='Read the original private A/A plan and print only a closed, anonymized projection.')
    cli.add_argument('--plan', type=Path, required=True)
    try:
        plan_path = cli.parse_args().plan
        CONTROL.private(plan_path.parent)
        plan = read_json(plan_path, 65536)
        require(set(plan) == {'schema', 'owner', 'host_boot', 'cases'} and plan['schema'] == 'bili-shelf-clock-aa-v1'
                and plan['owner'] == CONTROL.configured_owner() and CONTROL.UUID.fullmatch(plan['host_boot']), 'Unexpected A/A plan')
        print(json.dumps(series_projection(plan['cases'], host_boot=plan['host_boot']), separators=(',', ':'), allow_nan=False))
    except (ValueError, KeyError, TypeError, OSError, UnicodeError):
        cli.exit(1, 'Unqualified A/A plan, slot or clock evidence; no projection emitted.\n')
