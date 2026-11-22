"""Replay ordinary desktop cover costs without timing or launching another process."""

import argparse
import json
from pathlib import Path
from statistics import median


SCHEDULE = ['a00', 'b00'] + [f'{arm}{pair:02d}' for pair in range(1, 7) for arm in ('ab' if pair % 2 else 'ba')]


def require(condition, message):
    if not condition:
        raise ValueError(message)


def load(path):
    return json.loads(path.read_text(encoding='utf-8'))


def rows(path):
    return [json.loads(line) for line in path.read_text(encoding='utf-8').splitlines()]


def read_case(directory, observer, route, expected_covers):
    name = directory.name
    pid = observer['pid']

    def check(condition, message):
        require(condition, f'{name}: {message}')

    check(observer == load(directory / 'result.json') and observer['case'] == name and type(pid) is int and pid > 0
          and observer['selected_arm'] == name[0].upper() and observer['process_executable_matches'] is True
          and observer['final_logical_rgb_matches'] is True,
          'case/arm/window identity or final pixels differ from the closed observer report')
    final = load(directory / 'final.json')
    performance, life = (rows(directory / leaf) for leaf in ('performance.jsonl', 'lifecycle.jsonl'))
    check(final['process_id'] == pid and all(row['process_id'] == pid for row in performance + life)
          and final['route'] == route and final['demo'] is False and final['offline_requested'] is True
          and final['offline'] is True and final['backend'] == 'desktop-software' and final['profile'] == 'scribe'
          and final['startup']['booting'] is False and final['startup']['error'] is False, 'wrong process or scene')

    def events(stage):
        return [row for row in performance if (row['kind'], row['stage']) == ('startup', stage)]

    entered, = events('process_enter')
    prepared, = events('state_prepared')
    available, = events('ui_ready')
    context, = events('state-prepare-context')
    ready_record, = [row for row in life if row['event'] == 'ready']
    enter, producer = (row['data']['observed']['monotonic_ns'] for row in (entered, prepared))
    consumer, ready = prepared['monotonic_ns'], available['monotonic_ns']
    check(context['data']['demo'] is False and context['data']['offline'] is True and ready_record['route'] == route
          and all(row['thread_id'] == pid for row in (entered, prepared, available))
          and 0 < enter <= producer <= consumer <= ready <= ready_record['monotonic_ns'], 'invalid startup clocks')
    for label, stamp in (('process_enter', enter), ('state_prepared', producer)):
        check([row['monotonic_ns'] for row in life if row.get('stage') == label] == [stamp], f'different {label} clock')
    resident = ready_record['reader_resources']
    check(resident['cover_resident_count'] == observer['ready_resident_covers'] == expected_covers > 0
          and resident['cover_resident_bytes'] == observer['ready_resident_bytes'] > 0,
          'unexpected prepared cover inventory at readiness')

    costs = {}
    for stage, main, earliest, latest in [('state-covers', False, enter, producer), ('attach-covers', True, consumer, ready)]:
        records = events(stage)
        starts, ends = records[::2], records[1::2]
        check(ends and [row['data']['phase'] for row in records] == ['begin', 'end'] * len(ends)
              and (main or len(ends) == 1) and [row['data']['outcome'] for row in ends] == ['partial'] * (len(ends) - 1) + ['success'],
              f'incomplete {stage} sequence')
        totals = {'wall_us': 0, 'cpu_us': 0}
        previous = earliest
        for start, end in zip(starts, ends):
            check(type(start['thread_id']) is int and start['thread_id'] > 0 and start['thread_id'] == end['thread_id']
                  and (start['thread_id'] == pid) == main and previous <= start['monotonic_ns'] <= end['monotonic_ns'] <= latest,
                  f'wrong {stage} owner or ordering')
            previous = end['monotonic_ns']
            for key, clock in [('wall_us', 'monotonic_ns'), ('cpu_us', 'thread_cpu_ns')]:
                value = end['data'][key]
                check(type(value) is int and 0 <= value <= (end[clock] - start[clock]) // 1000 + 1, f'invalid {stage} {key}')
                totals[key] += value
        check(all(value == observer['stages'][stage][key] for key, value in totals.items()), f'different {stage} observation')
        costs.update({('ui_attach_' if main else 'worker_') + key: value for key, value in totals.items()})
    costs['combined_cpu_us'] = costs['worker_cpu_us'] + costs['ui_attach_cpu_us']
    return {'pid': pid, 'process_enter_ns': enter, 'ui_ready_ns': ready, 'resident_bytes': resident['cover_resident_bytes'],
            'ui_batches': len(events('attach-covers')) // 2, 'costs': costs}


def medians(samples):
    return {key: median(sample[key] for sample in samples) for key in samples[0]}


def summarize(trial, route, expected_covers):
    report = load(trial / 'report.json')
    require(not (trial / 'failure.json').exists() and report['clock'] == 'CLOCK_MONOTONIC'
            and report['schedule'] == SCHEDULE and report['warmups_excluded'] == SCHEDULE[:2]
            and set(report['cases']) == set(SCHEDULE), 'expected a complete alternating 14-case trial')
    sources = report['sources']
    require(set(sources) == {'A', 'B'} and sources['A'] and sources['B'] and sources['A'] != sources['B'], 'unbound source arms')
    all_cases = {name: read_case(trial / name, report['cases'][name], route, expected_covers) for name in SCHEDULE}
    require(all(all_cases[first]['ui_ready_ns'] < all_cases[second]['process_enter_ns'] for first, second in zip(SCHEDULE, SCHEDULE[1:]))
            and len({case['resident_bytes'] for case in all_cases.values()}) == 1, 'overlapping launches or different cover bytes')
    cases = {name: all_cases[name] for name in SCHEDULE[2:]}
    pairs = []
    for first, second in zip(SCHEDULE[2::2], SCHEDULE[3::2]):
        control, candidate = [cases[name]['costs'] for name in sorted((first, second))]
        pairs.append({'pair': int(first[1:]), 'order': (first[0] + second[0]).upper(),
                      'B_minus_A': {key: candidate[key] - control[key] for key in control}})
    return {'scope': 'Warmed offline desktop; thread CPU in two measured stages, not end-to-end time or proof of a codec branch.',
            'sources': sources, 'route': route, 'warmups_excluded': SCHEDULE[:2], 'resident_covers': expected_covers,
            'cases': cases, 'pairs': pairs, 'arm_medians': {
                arm: medians([case['costs'] for name, case in cases.items() if name[0] == arm.lower()]) for arm in sources},
            'paired_medians_B_minus_A': {group: medians([row['B_minus_A'] for row in pairs if group in ('all', row['order'])])
                                         for group in ('all', 'AB', 'BA')},
            'pairs_with_lower_combined_cpu': sum(row['B_minus_A']['combined_cpu_us'] < 0 for row in pairs)}


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('trial', type=Path, help='Closed ordinary desktop trial containing report.json and raw per-case records')
    parser.add_argument('--route', default='shelf')
    parser.add_argument('--ready-covers', type=int, default=10)
    args = parser.parse_args()
    print(json.dumps(summarize(args.trial, args.route, args.ready_covers), indent=2))
