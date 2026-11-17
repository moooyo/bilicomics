#!/usr/bin/env python3
"""Recompute stored AB timing from this public bundle; no device access.

This bounded reader follows the frozen Library analyzer endpoints() selection
and its reviewed frame_observation() acceptance rules. Full original analyzer
source identities are recorded in manifest.json. It reads JSON files only.
"""

from decimal import Decimal
import json
from pathlib import Path
import statistics

ROOT = Path(__file__).resolve().parents[1]

def require(value, message):
    if not value:
        raise RuntimeError(message)

def load(path):
    return json.loads((ROOT / path).read_text(encoding='utf-8'))

def ms(end, origin):
    require(type(end) is int and type(origin) is int and end >= origin, 'Invalid clock order')
    return Decimal(end - origin) / Decimal(1000000)

def endpoint(case):
    records = case['performance_selected_records']
    ready = [r for r in records if (r['fields']['kind'], r['fields']['stage']) == ('startup', 'ui_ready')]
    require(len(ready) == 1, 'ui_ready must be unique')
    ready_ns = ready[0]['fields']['monotonic_ns']
    accepted = []
    for record in records:
        row = record['fields']
        if (row['kind'], row['stage']) != ('native', 'frame'):
            continue
        data = row['data']
        require(data.get('panel_completion_observed') is False, 'Unexpected panel-completion claim')
        submit = data['stages'].get('refresh_submit')
        send_ns = None
        if data.get('refresh_accepted') is True:
            require(isinstance(submit, dict), 'Accepted frame has no submit stage')
            start, end = submit['started_monotonic_ns'], submit['finished_monotonic_ns']
            require(type(start) is int and type(end) is int and start <= end <= row['monotonic_ns'], 'Invalid submit clock order')
            require(data.get('refresh_ioctl_returned') is True and type(data.get('refresh_ioctl_attempts')) is int
                    and data['refresh_ioctl_attempts'] >= 1, 'Accepted frame has no returned ioctl')
            send_ns = end
        render = data['stages'].get('slint_render')
        if data.get('rendered') is True and send_ns is not None and isinstance(render, dict) and isinstance(submit, dict):
            start, end = render['started_monotonic_ns'], render['finished_monotonic_ns']
            if type(start) is int and type(end) is int and ready_ns <= start <= end <= submit['started_monotonic_ns']:
                accepted.append((send_ns, record['source_line_1based']))
    require(accepted, 'No qualified first Shelf accepted SEND')
    send_ns, frame_line = min(accepted)
    origin = case['request']['monotonic_ns']
    value = {'software_ready_ms': ms(ready_ns, origin), 'first_shelf_send_ms': ms(send_ns, origin)}
    derived = case['derived_endpoints']
    require(ready[0]['source_line_1based'] == derived['trace_ready_line_1based']
            and frame_line == derived['trace_selected_frame_line_1based'], 'Selected source line differs')
    require(ready_ns == derived['ui_ready_monotonic_ns'] and send_ns == derived['first_shelf_send_accepted_monotonic_ns'], 'Selected endpoint differs')
    for key in value:
        require(value[key] == Decimal(derived[key]), 'Published endpoint differs')
    return value

def metric(rows, key):
    baseline = [rows['baseline-' + f'{i:02d}'][key] for i in range(1, 7)]
    candidate = [rows['candidate-' + f'{i:02d}'][key] for i in range(1, 7)]
    paired = [c - b for b, c in zip(baseline, candidate)]
    med_b, med_c = statistics.median(baseline), statistics.median(candidate)
    return {'baseline_values_ms': baseline, 'candidate_values_ms': candidate,
            'baseline_marginal_median_ms': med_b, 'candidate_marginal_median_ms': med_c,
            'marginal_median_candidate_minus_baseline_ms': med_c - med_b,
            'marginal_median_percent_change': (med_c - med_b) / med_b * 100,
            'paired_candidate_minus_baseline_ms': paired,
            'paired_median_candidate_minus_baseline_ms': statistics.median(paired),
            'median_pairwise_percent_change': statistics.median([(c - b) / b * 100 for b, c in zip(baseline, candidate)]),
            'paired_median_normalized_by_baseline_median_percent': statistics.median(paired) / med_b * 100,
            'candidate_faster_pairs': sum(d < 0 for d in paired), 'candidate_slower_pairs': sum(d > 0 for d in paired)}

def compare(actual, expected):
    if isinstance(actual, dict):
        require(set(actual) == set(expected), 'Statistic fields differ')
        for key in actual:
            compare(actual[key], expected[key])
    elif isinstance(actual, list):
        require(len(actual) == len(expected), 'Statistic roster differs')
        for left, right in zip(actual, expected):
            compare(left, right)
    else:
        require(Decimal(str(actual)) == Decimal(str(expected)), 'Exact decimal statistic differs')

summary = load('summary.json')
rows = {}
for item in summary['cases']:
    case = load(item['evidence'])
    require(case['case'] == item['case'], 'Case identity differs')
    rows[case['case']] = endpoint(case)
require(len(rows) == 14 and summary['schedule']['all_cases_retained'] is True
        and summary['schedule']['outliers_removed'] is False, 'Retained schedule differs')
result = {key: metric(rows, key) for key in ('first_shelf_send_ms', 'software_ready_ms')}
compare(result, load('independent-audit.json')['statistics'])
print(json.dumps({'scope': 'Stored public evidence only; no device access', 'retained_cases': len(rows),
                  'all_endpoints_and_exact_statistics_match': True, 'statistics': result}, indent=2, default=str))
