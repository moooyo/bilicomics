"""Validate and partition an ordinary Kindle Shelf's opt-in startup timing."""

import json
import re

LIMIT = 96 * 1024
CLOCK = 'CLOCK_MONOTONIC'
MAX_INT = (1 << 63) - 1
MARKS = ('process_enter', 'window_shown', 'workers_admitted', 'state_worker_begin', 'metadata_ready',
         'cover_gate_wait_begin', 'cover_gate_open', 'cover_gate_passed', 'state_produced', 'state_received',
         'app_attached', 'ui_ready')
GATE_SOURCES = {'cover_gate_from_ui': 'ui', 'cover_gate_from_send': 'send'}
EXTRA_MARKS = GATE_SOURCES.keys() | {'loading_send_accepted'}
SETUP = ('native_context', 'backend_setup', 'window_setup', 'runtime_setup')
METADATA = ('state-directory', 'state-client', 'state-store', 'state-preferences', 'state-cache-maintenance',
            'state-catalog', 'state-account', 'state-intents', 'state-resume-anchor', 'state-cover-selection')
ATTACH = ('attach-covers', 'attach-catalog', 'attach-account', 'attach-downloads', 'attach-route', 'attach-views')
JPEG_OUTCOMES = {'available', 'platform', 'device', 'helper', 'driver', 'diagnostics', 'busy', 'failed'}
SPAN_OUTCOMES = {**{name: {'ok'} for name in (*SETUP, *METADATA, 'metadata', 'app_attach', 'font_registration',
                                              'font_finish', 'font_prepare')},
                 **{name: {'success'} for name in ATTACH}, 'session_load': {'present'}, 'jpeg_capability': JPEG_OUTCOMES,
                 'state-cache-maintenance': {'ok', 'partial'}, 'state-covers': {'ok', 'partial'},
                 'attach-covers': {'success', 'partial'}}
FRAME_STAGES = {'active_io', 'buffer_setup', 'window_setup', 'portrait_observer', 'initial_fill', 'preceding_wait',
                'render', 'write', 'submit', 'completion_wait', 'lifecycle_complete'}
FRAME_FIELDS = {'kind', 'sequence', 'timing', 'initial', 'rendered', 'complete_replacement', 'preceding_marker',
                'accepted_marker', 'completed_marker', 'accepted', 'stages'}
HEADER = {'schema', 'run_id', 'boot_id', 'process_id', 'clock', 'shelf_render_ns', 'stages', 'frames'}


def require(condition, reason):
    if not condition:
        raise ValueError(reason)


def exact(record, fields):
    require(type(record) is dict and record.keys() == fields, 'Unexpected diagnostic fields or type')


def number(value, lower=1, upper=MAX_INT):
    return type(value) is int and lower <= value <= upper


def unique(pairs):
    result = dict(pairs)
    require(len(result) == len(pairs), 'Duplicate diagnostic key')
    return result


def constant(_):
    raise ValueError('Non-JSON diagnostic number')


def decode(blob):
    require(type(blob) is bytes and 0 < len(blob) <= LIMIT and blob.endswith(b'\n') and blob.count(b'\n') == 1,
            'Missing, partial or oversized diagnostic sidecar')
    return json.loads(blob.decode('ascii'), object_pairs_hook=unique, parse_constant=constant)


def ordered(*values):
    require(all(number(value) for value in values) and list(values) == sorted(values), 'Reversed or missing phase clocks')


def between(row, low, high):
    ordered(low, row['start_ns'], row['end_ns'], high)
    # The thread CPU reading follows each wall reading. Old kernels can also
    # advance thread CPU in discrete ticks; don't use wall-minus-CPU as an I/O measure.
    require(row['cpu_ns'] is None or number(row['cpu_ns'], 0, wall(row) + 20_000_000), 'Invalid thread CPU clock')
    return row


def wall(row):
    return row['end_ns'] - row['start_ns']


def total(rows):
    return {'wall_ns': sum(wall(row) for row in rows),
            'cpu_ns': sum(row['cpu_ns'] for row in rows) if all(row['cpu_ns'] is not None for row in rows) else None,
            'count': len(rows)}


def verify_stages(events, renderer_ns):
    require(type(events) is list and 0 < len(events) <= 160, 'Missing or too many application phases')
    marks, spans = {}, {name: [] for name in SPAN_OUTCOMES}
    for event in events:
        require(type(event) is dict and type(event.get('stage')) is str, 'Invalid application phase')
        kind, stage = event.get('kind'), event['stage']
        if kind == 'mark':
            exact(event, {'kind', 'stage', 'ns'})
            require((stage in MARKS or stage in EXTRA_MARKS) and stage not in marks and number(event['ns']),
                    'Invalid or repeated application landmark')
            marks[stage] = event['ns']
        else:
            exact(event, {'kind', 'stage', 'start_ns', 'end_ns', 'cpu_ns', 'outcome'})
            require(kind == 'span' and stage in spans and type(event['outcome']) is str
                    and event['outcome'] in SPAN_OUTCOMES[stage], 'Unexpected application work or outcome')
            spans[stage].append(event)
    sources = GATE_SOURCES.keys() & marks.keys()
    require(set(MARKS) <= marks.keys() and len(sources) <= 1 and ('loading_send_accepted' not in marks or sources)
            and all(1 <= len(group) <= (32 if name in ('attach-covers', 'font_registration') else 1)
                    for name, group in spans.items()), 'Incomplete or duplicated application phases')
    first = marks['process_enter']
    for rows in spans.values():
        for row in rows:
            between(row, first, renderer_ns)
        rows.sort(key=lambda row: row['start_ns'])
    single = {name: rows[0] for name, rows in spans.items()}

    def series(names, lo, hi):
        ordered(lo, *(value for name in names for row in spans[name] for value in (row['start_ns'], row['end_ns'])), hi)

    series(SETUP, first, marks['window_shown'])
    ordered(*(marks[name] for name in MARKS if name != 'cover_gate_open'), renderer_ns)
    # The worker may observe a released gate just before the UI logs its mark.
    ordered(marks['window_shown'], marks['cover_gate_open'], marks['state_received'])
    source = next(iter(sources), None)
    if source is not None:
        release = marks[source]
        ordered(marks['window_shown'], release, marks['cover_gate_passed'])
        ordered(release, marks['cover_gate_open'])
        if source == 'cover_gate_from_send':
            ordered(marks['window_shown'], marks.get('loading_send_accepted'), release)
    series(('jpeg_capability', 'metadata'), marks['state_worker_begin'], marks['metadata_ready'])
    series(METADATA, single['metadata']['start_ns'], single['metadata']['end_ns'])
    series(('session_load',), single['state-client']['start_ns'], single['state-client']['end_ns'])
    series(('state-covers',), marks['cover_gate_passed'], marks['state_produced'])
    series(('font_registration', 'font_finish'), marks['workers_admitted'], single['app_attach']['start_ns'])
    series(('font_prepare',), marks['workers_admitted'], renderer_ns)
    series(('app_attach',), marks['state_received'], marks['app_attached'])
    series(ATTACH, marks['app_attached'], marks['ui_ready'])
    cover_outcomes = [row['outcome'] for row in spans['attach-covers']]
    require(cover_outcomes == ['partial'] * (len(cover_outcomes) - 1) + ['success'], 'Cover attach batches did not complete')
    return marks, spans, GATE_SOURCES.get(source)


def verify_frames(rows, shown, ready, renderer_ns, render_order, send, completed):
    require(type(rows) is list and 3 <= len(rows) <= 24, 'Expected native setup, startup feedback and Shelf work')
    require(render_order in ('render_then_wait', 'wait_then_render'), 'Not a normal Shelf draw')
    require(all(type(row) is dict for row in rows), 'Invalid native frame records')
    upper = rows[-1].get('timing', {})
    require(type(upper) is dict and number(upper.get('end_ns')), 'Missing native frame endpoint')
    previous_end = shown
    for sequence, row in enumerate(rows):
        exact(row, FRAME_FIELDS)
        require(row['kind'] == ('setup' if sequence == 0 else 'frame') and type(row['sequence']) is int
                and row['sequence'] == sequence and all(type(row[key]) is bool for key in ('initial', 'rendered', 'complete_replacement'))
                and (row['accepted'] is None or type(row['accepted']) is bool)
                and all(row[key] is None or number(row[key], 1, (1 << 32) - 1)
                        for key in ('preceding_marker', 'accepted_marker', 'completed_marker')), 'Invalid native frame identity')
        exact(row['timing'], {'start_ns', 'end_ns', 'cpu_ns'})
        start, end = row['timing']['start_ns'], row['timing']['end_ns']
        between(row['timing'], previous_end, upper['end_ns'])
        exact(row['stages'], FRAME_STAGES)
        measured = []
        for stage in row['stages'].values():
            if stage is not None:
                exact(stage, {'start_ns', 'end_ns', 'cpu_ns'})
                measured.append(between(stage, start, end))
        ordered(start, *(value for stage in sorted(measured, key=lambda stage: (stage['start_ns'], stage['end_ns']))
                         for value in (stage['start_ns'], stage['end_ns'])), end)
        require(row['rendered'] is (row['stages']['render'] is not None), 'Native draw has no render span')
        previous_end = end
    setup, shelf = rows[0], rows[-1]
    require(not setup['rendered'] and all(setup['stages'][key] is not None for key in
            ('active_io', 'buffer_setup', 'window_setup', 'portrait_observer')) and rows[1]['initial'] and rows[1]['rendered']
            and not shelf['initial'] and shelf['rendered'], 'Missing native setup, loading draw, or normal Shelf')
    draw, wait, write = (shelf['stages'][name] for name in ('render', 'preceding_wait', 'write'))
    require(all(value is not None for value in (draw, wait, write)), 'Incomplete Shelf draw transaction')
    ordered(ready, draw['start_ns'], renderer_ns, draw['end_ns'])
    if render_order == 'render_then_wait':
        ordered(draw['end_ns'], wait['start_ns'], wait['end_ns'], write['start_ns'])
    else:
        ordered(wait['end_ns'], draw['start_ns'], draw['end_ns'], write['start_ns'])
    if send is not None:
        submit = shelf['stages']['submit']
        require(shelf['accepted'] is True and shelf['accepted_marker'] == send[0] and submit is not None,
                'Shelf SEND does not match the profiled frame marker')
        ordered(write['end_ns'], submit['start_ns'], send[1], submit['end_ns'])
    if completed is not None:
        completion = shelf['stages']['completion_wait']
        require(send is not None and completed[0] == send[0] == shelf['completed_marker'] and completion is not None,
                'Shelf completion does not match the profiled frame marker')
        ordered(send[1], completion['start_ns'], completion['end_ns'], completed[1], shelf['timing']['end_ns'])
    return shelf


def intersect(lo, hi, spans):
    return sum(max(0, min(hi, row['end_ns']) - max(lo, row['start_ns'])) for row in spans)


def project(data, *, run, boot, pid, renderer_ns, render_order, send, completed):
    exact(data, HEADER)
    require(type(run) is str and re.fullmatch(r'bili-shelf-[0-9a-f]{32}', run)
            and type(boot) is str and re.fullmatch(r'[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}', boot)
            and number(pid, 1, (1 << 31) - 1) and number(data['process_id']) and number(renderer_ns)
            and number(data['shelf_render_ns']) and (data['schema'], data['clock'], data['run_id'], data['boot_id'],
            data['process_id'], data['shelf_render_ns']) == ('kindle-startup-phases-v1', CLOCK, run, boot, pid, renderer_ns),
            'Diagnostic sidecar belongs to a different run, boot, process or frame')
    for update in (send, completed):
        require(update is None or (type(update) is tuple and len(update) == 2 and number(update[0], 1, (1 << 32) - 1)
                                  and number(update[1])), 'Invalid independent marker witness')
    marks, spans, gate_source = verify_stages(data['stages'], renderer_ns)
    shelf = verify_frames(data['frames'], marks['window_shown'], marks['ui_ready'], renderer_ns, render_order, send, completed)
    draw = shelf['stages']['render']
    loading = data['frames'][1:-1]  # The first sampled frame is I/O and buffer setup, not a draw.
    loading_transactions = [frame['timing'] for frame in loading]
    loading_stages = {name: [frame['stages'][name] for frame in loading if frame['stages'][name] is not None]
                      for name in FRAME_STAGES}
    display_waits = loading_stages['preceding_wait'] + loading_stages['completion_wait']
    attached, ready = marks['app_attached'], marks['ui_ready']
    attach_spans = [row for name in ATTACH for row in spans[name]]
    attach_work = total(attach_spans)
    first_loading = loading[0]
    first_wait = first_loading['stages']['completion_wait']
    first_accepted = first_loading['accepted'] is True and first_loading['accepted_marker'] is not None
    first_completed = (first_wait is not None and first_accepted
                       and first_loading['accepted_marker'] == first_loading['completed_marker'])
    loading_send = marks.get('loading_send_accepted')
    if gate_source is not None and (first_completed or loading_send is not None):
        submit = first_loading['stages']['submit']
        require(first_accepted and submit is not None and loading_send == submit['end_ns'],
                'Initial SEND landmark does not match the first accepted loading frame')
    last_loading = loading[-1]
    startup_wait = intersect(marks['window_shown'], marks['cover_gate_open'], display_waits)

    def first_wait_overlap(lo, hi):
        return intersect(lo, hi, [first_wait]) if first_completed else None

    jpeg = spans['jpeg_capability'][0]
    metadata = spans['metadata'][0]
    covers = spans['state-covers'][0]
    attach_loading = intersect(attached, ready, loading_transactions)
    attach_wait = intersect(attached, ready, display_waits)
    attached_gap = ready - attached - attach_work['wall_ns'] - attach_loading
    require(attached_gap >= 0 and attach_wait <= attach_loading and all(
            intersect(row['start_ns'], row['end_ns'], loading_transactions) == 0 for row in attach_spans),
            'Overlapping UI attachment and native frame observations')
    intervals = {
        'main_to_window_shown': marks['window_shown'] - marks['process_enter'],
        'window_to_admission': marks['workers_admitted'] - marks['window_shown'],
        'window_to_loading_draw_transaction': first_loading['timing']['start_ns'] - marks['window_shown'],
        'window_to_cover_gate_open': marks['cover_gate_open'] - marks['window_shown'],
        'window_to_cover_gate_display_wait_overlap': startup_wait,
        'admission_to_state_worker': marks['state_worker_begin'] - marks['workers_admitted'],
        'state_worker_to_produced': marks['state_produced'] - marks['state_worker_begin'],
        'jpeg_overlap_with_first_loading_completion_wait': first_wait_overlap(jpeg['start_ns'], jpeg['end_ns']),
        'metadata_overlap_with_first_loading_completion_wait': first_wait_overlap(metadata['start_ns'], metadata['end_ns']),
        'metadata_ready_to_gate_waiter': marks['cover_gate_wait_begin'] - marks['metadata_ready'],
        'cover_gate_wait': marks['cover_gate_passed'] - marks['cover_gate_wait_begin'],
        'cover_gate_overlap_with_first_loading_completion_wait': first_wait_overlap(marks['cover_gate_wait_begin'], marks['cover_gate_passed']),
        'cover_gate_to_decode': covers['start_ns'] - marks['cover_gate_passed'],
        'cover_decode_overlap_with_first_loading_completion_wait': first_wait_overlap(covers['start_ns'], covers['end_ns']),
        'decode_to_produced': marks['state_produced'] - covers['end_ns'],
        'state_produced_to_received': marks['state_received'] - marks['state_produced'],
        'state_queue_display_wait_overlap': intersect(marks['state_produced'], marks['state_received'], display_waits),
        'state_received_to_attached': attached - marks['state_received'],
        'attached_to_ready': ready - attached,
        'attached_loading_frame_other': attach_loading - attach_wait,
        'attached_loading_display_wait': attach_wait,
        'attached_unattributed_gap': attached_gap,
        'ready_to_shelf_draw': draw['start_ns'] - ready,
        'shelf_mapping_wait_before_draw': intersect(ready, draw['start_ns'], [shelf['stages']['preceding_wait']]),
        'shelf_draw_to_renderer_return': renderer_ns - draw['start_ns'],
        'main_to_renderer_return': renderer_ns - marks['process_enter'],
    }
    return {
        'process_enter_ns': marks['process_enter'],
        'cover_gate_source': gate_source,
        'attach_work': attach_work,
        'intervals_ns': intervals,
        'application_work': {name: {**total(rows), 'outcome': rows[-1]['outcome']} for name, rows in spans.items()},
        'native_setup': {name: total([stage]) for name, stage in data['frames'][0]['stages'].items() if stage is not None},
        'display_witness': {'first_loading_completion_matches_send': first_completed,
                            'shelf_preceding_marker_matches_last_loading_send': last_loading['accepted'] is True
                            and last_loading['accepted_marker'] is not None
                            and last_loading['accepted_marker'] == shelf['preceding_marker']},
        'first_loading_frame': total([first_loading['timing']]),
        'first_loading_frame_work': {name: total([stage]) for name, stage in first_loading['stages'].items() if stage is not None},
        'pre_shelf_frames': total(loading_transactions),
        'pre_shelf_display_waits': total(display_waits),
        'pre_shelf_frame_work': {name: total(rows) for name, rows in sorted(loading_stages.items()) if rows},
        'shelf_frame_work': {name: total([stage]) for name, stage in shelf['stages'].items() if stage is not None},
    }
