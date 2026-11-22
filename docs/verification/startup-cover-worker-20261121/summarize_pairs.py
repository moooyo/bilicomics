"""Summarize an existing desktop cover trial from its observer report and performance traces."""

import argparse
import json
from pathlib import Path
from statistics import median


SCHEDULE = ['a00', 'b00'] + [f'{arm}{pair:02d}' for pair in range(1, 7) for arm in ('ab' if pair % 2 else 'ba')]
HEADINGS = ['Worker wall / CPU', 'UI 1 wall / CPU', 'UI 2 wall / CPU', 'UI gap', 'Producer', 'UI record',
            'Ready', 'Producer to UI', 'UI to ready', 'First match upper']


def require(condition, message):
    if not condition:
        raise ValueError(message)


def read_case(trial, name, case):
    pid, stages = case['pid'], case['stages']
    rows = [json.loads(line) for line in (trial / name / 'performance.jsonl').read_text().splitlines()]
    require(rows and all(row['process_id'] == pid for row in rows) and case['case'] == name
            and case['final_logical_rgb_matches'] is True and 'initial' in case['activity_states'], 'Wrong process or fixture')

    def events(stage):
        return [row for row in rows if (row['kind'], row['stage']) == ('startup', stage)]

    entered, = events('process_enter')
    prepared, = events('state_prepared')
    available, = events('ui_ready')
    enter, producer = (row['data']['observed']['monotonic_ns'] for row in (entered, prepared))
    consumed, ready = prepared['monotonic_ns'], available['monotonic_ns']
    require(all(row['thread_id'] == pid for row in (entered, prepared, available)) and
            [enter, producer, consumed, ready] == [stages[key] for key in ('process_ns', 'producer_ns', 'ui_record_ns', 'ready_ns')]
            and 0 < enter <= entered['monotonic_ns'] <= producer <= consumed <= ready, 'Different process or state clocks')

    def spans(stage, outcomes, main_thread):
        records = events(stage)
        require([row['data']['phase'] for row in records] == ['begin', 'end'] * len(outcomes) and
                [row['data']['outcome'] for row in records[1::2]] == outcomes, f'Invalid {stage} phases')
        result = []
        for start, finish in zip(records[::2], records[1::2]):
            begin, end = start['monotonic_ns'], finish['monotonic_ns']
            require(start['thread_id'] == finish['thread_id'] and (start['thread_id'] == pid) == main_thread
                    and enter <= begin <= end <= ready, f'Invalid {stage} clock or owner')
            costs = tuple(finish['data'][key] for key in ('wall_us', 'cpu_us'))
            require(all(type(cost) is int and cost >= 0 for cost in costs), f'Invalid {stage} costs')
            result.append((begin, end, costs))
        require([sum(item[2][index] for item in result) for index in (0, 1)] ==
                [stages[stage][key] for key in ('wall_us', 'cpu_us')], f'{stage} differs from report')
        return result

    worker, = spans('state-covers', ['success'], False)
    first, second = spans('attach-covers', ['partial', 'success'], True)
    gap = second[0] - first[1]
    require(worker[1] <= producer and consumed <= first[0] <= first[1] <= second[0] and
            gap == stages['attach_gap_ns'] and stages['attach_outcomes'] == ['partial', 'success'], 'Invalid cover order')
    no_feedback, feedback, miss, match, settled = (case[key] for key in ('last_no_feedback_sample_ns', 'feedback_sample_ns',
            'last_non_shelf_sample_ns', 'first_exact_shelf_sample_ns', 'second_exact_shelf_sample_ns'))
    require(all(0 < start <= end for start, end in (no_feedback, feedback, miss, match, settled)) and
            no_feedback[1] <= feedback[0] <= feedback[1] <= miss[0] <= miss[1] <= match[0] <= match[1] <= settled[0]
            and enter <= feedback[0] <= ready <= match[1], 'Invalid X11 sampling order')
    lower, upper = miss[0] - enter, match[1] - enter
    feedback_bounds = [max(no_feedback[0], enter) - enter, feedback[1] - enter]
    require([lower, upper] == stages['enter_to_shelf_bracket_ns'] and feedback_bounds == stages['enter_to_feedback_bracket_ns']
            and [producer - feedback[1], producer - feedback[0]] == stages['feedback_sample_to_producer_ns']
            and consumed - producer == stages['producer_to_ui_ns'] and ready - consumed == stages['ui_to_ready_ns'], 'Different intervals')
    values = [cost * 1000 for item in (worker, first, second) for cost in item[2]]
    values += [gap, producer - enter, consumed - enter, ready - enter, consumed - producer, ready - consumed, upper]
    return values, (lower, upper), feedback_bounds, enter, settled[1]


def row(cells):
    return '| ' + ' | '.join(map(str, cells)) + ' |'


def format_ms(values):
    return ' / '.join(f'{value / 1e6:.3f}' for value in values)


def metrics(values):
    return [format_ms(values[start:start + 2]) for start in (0, 2, 4)] + [format_ms([value]) for value in values[6:]]


def table(headers, data):
    return [row(headers), row(['---'] * len(headers)), *(row(cells) for cells in data)]


def summarize(trial):
    report = json.loads((trial / 'report.json').read_text())
    require(not (trial / 'failure.json').exists() and report['clock'] == 'CLOCK_MONOTONIC' and report['schedule'] == SCHEDULE
            and report['warmups_excluded'] == SCHEDULE[:2] and set(report['cases']) == set(SCHEDULE)
            and len(report['pairs']) == 6, 'Expected one closed, balanced six-pair desktop trial')
    cases = {name: read_case(trial, name, report['cases'][name]) for name in SCHEDULE}
    require(all(cases[before][4] <= cases[after][3] for before, after in zip(SCHEDULE, SCHEDULE[1:])), 'Overlapping launches')
    paired, groups, wins = [], {'AB': [], 'BA': []}, 0
    for pair, original in enumerate(report['pairs'], 1):
        control, candidate = [cases[f'{arm}{pair:02d}'] for arm in 'ab']
        deltas = [new - old for old, new in zip(control[0], candidate[0])]
        limits = [candidate[1][0] - control[1][1], candidate[1][1] - control[1][0]]
        order = 'AB' if pair % 2 else 'BA'
        require(original['pair'] == pair and original['order'] == order and original['B_minus_A_shelf_bounds_ns'] == limits
                and original['B_minus_A_first_observed_shelf_ns'] == deltas[-1]
                and original['B_minus_A_worker_cpu_us'] * 1000 == deltas[1]
                and original['B_minus_A_ui_attach_cpu_us'] * 1000 == deltas[3] + deltas[5], 'Different pair projection')
        win = limits[1] < 0
        wins += win
        paired.append([pair, order, *metrics(deltas), format_ms(limits), 'yes' if win else 'no'])
        groups[order].append(deltas)
    medians = {order: [median(column) for column in zip(*values)] for order, values in groups.items()}
    orders_earlier = all(values[-1] < 0 for values in medians.values())
    passed = wins >= 5 and orders_earlier
    lines = ['# Desktop cover timing', '', 'All values are milliseconds. Event times and Shelf bounds are relative to process entry.',
             'The first X11 match is an observation upper bound, not a driver receipt. Warmups a00 and b00 are excluded.', '',
             *table(['Case', *HEADINGS, 'Feedback lower / upper', 'Shelf lower / upper'],
                    ([name, *metrics(cases[name][0]), format_ms(cases[name][2]), format_ms(cases[name][1])] for name in SCHEDULE[2:])),
             '', '## Within-pair changes (B minus A)', '',
             *table(['Pair', 'Order', *HEADINGS, 'Shelf delta lower / upper', 'Definite B win'], paired), '',
             '## Median paired change by order', '', *table(['Order', *HEADINGS], ([order, *metrics(values)] for order, values in medians.items())), '',
             f'Desktop timing gate: **{"PASS" if passed else "FAIL"}**. Non-overlapping B wins: {wins}/6; both order-group first-match medians earlier: {orders_earlier}.',
             'The slow-feedback/failure fixtures, memory, full regressions and physical admission are separate gates.']
    return '\n'.join(lines)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('trial', type=Path)
    print(summarize(parser.parse_args().trial))
