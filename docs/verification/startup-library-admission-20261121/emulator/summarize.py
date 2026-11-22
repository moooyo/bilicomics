"""Project a complete fixed Library/X11 surrogate schedule without exporting private fixture data."""

import argparse
import json
from pathlib import Path
from statistics import median


def require(condition, message):
    if not condition:
        raise ValueError(message)


def case(root, phase, number, arm, profile):
    row = json.loads((root/f'{phase}-{number:02d}'/'result.json').read_text())
    require((row['series'], row['phase'], row['number'], row['arm'], row['profile'], row['status']) == (root.name, phase, number, arm, profile, 'passed'), 'Unexpected or failed case')
    measured = row['measurement']
    require(measured['same_pid_ready_route'] == 'shelf' and measured['verified_real_executable_and_process_chain'] and measured['natural_worker_and_parent_exit'] and measured['single_mock_display_takeover_and_restoration'], 'Missing route, identity or retirement evidence')
    require(set(measured['live_window_non_cover_regions']) == {'resume_title','filter_control','active_shelf_navigation'} and measured['viewable_samples'] > 0, 'Incomplete live Shelf evidence')
    lower, upper = measured['observation_lower_ns'], measured['observation_upper_ns']
    entry = [measured['entry_lower_ns'], measured['entry_upper_ns']]
    require(0 <= lower <= upper and 0 <= entry[0] <= entry[1] <= upper, 'Invalid observation interval')
    return {'arm': arm, 'profile': profile, 'shelf_ns': [lower, upper], 'process_entry_ns': entry, 'dispatch_width_ns': measured['launch_bracket_ns'],
            'source_launcher_bytes': measured['source_launcher_bytes'], 'executable_bytes': measured['application_bytes']}


def difference(left, right):
    return [left[0] - right[1], left[1] - right[0]]


def millisecond_bounds(values):
    return [round(median(item[index] for item in values) / 1_000_000, 6) for index in (0, 1)]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('series', type=Path)
    parser.add_argument('output', type=Path)
    args = parser.parse_args()
    root = args.series
    qualification_order = [('scribe','A'), ('scribe','B'), ('pw6','B'), ('pw6','A')]
    qualifications = [case(root, 'qualification', number, arm, profile) for number, (profile,arm) in enumerate(qualification_order, 1)]
    pairs = []
    entries = []
    for index, order in enumerate(('AB','BA','AB','BA','AB','BA')):
        rows = {arm: case(root, 'formal', 2*index+position, arm, 'scribe') for position,arm in enumerate(order, 1)}
        small, large = difference(rows['B']['shelf_ns'], rows['A']['shelf_ns'])
        entries.extend(rows.values())
        pairs.append({'pair': index+1, 'order': order, 'a_ns': rows['A']['shelf_ns'], 'b_ns': rows['B']['shelf_ns'], 'b_minus_a_ns': [small,large],
                      'entry_b_minus_a_ns': difference(rows['B']['process_entry_ns'],rows['A']['process_entry_ns']), 'inference': 'B earlier' if large < 0 else 'B later' if small > 0 else 'unresolved'})
    all_cases = [*entries, *qualifications]
    require(len({row['executable_bytes'] for row in all_cases}) == 1 and all(len({row['source_launcher_bytes'] for row in all_cases if row['arm']==arm}) == 1 for arm in 'AB'), 'Executable or script identity changed')
    require({entry.name for entry in root.iterdir() if entry.is_dir()} == {f'formal-{n:02d}' for n in range(1,13)} | {f'qualification-{n:02d}' for n in range(1,5)}, 'Unexpected case entries')
    result = {'scope':'Loopback-only Linux launcher lifecycle fixture, actual host desktop executable and first externally matching non-cover X11 Shelf. Not Kindle card release, physical rendering, driver SEND or panel visibility.',
              'series':root.name,'application_bytes':all_cases[0]['executable_bytes'], 'source_launcher_bytes':{record['arm']:record['source_launcher_bytes'] for record in all_cases},
              'excluded_qualifications':[{'profile':record['profile'], 'arm':record['arm'], 'passed':True} for record in qualifications],
              'formal_pairs':pairs, 'median_launch_to_shelf_ms':{name:millisecond_bounds([row['shelf_ns'] for row in entries if row['arm']==name]) for name in 'AB'},
              'median_paired_b_minus_a_ms':millisecond_bounds([row['b_minus_a_ns'] for row in pairs]), 'paired_order_medians_ms':{order:millisecond_bounds([row['b_minus_a_ns'] for row in pairs if row['order']==order]) for order in ('AB','BA')},
              'paired_process_entry_median_b_minus_a_ms':millisecond_bounds([row['entry_b_minus_a_ns'] for row in pairs]),
              'paired_classification':{value:sum(row['inference']==value for row in pairs) for value in ('B earlier','B later','unresolved')},'maximum_dispatch_bracket_ns':max(row['dispatch_width_ns'] for row in entries)}
    with args.output.open('x') as stream:
        stream.write(json.dumps(result,indent=2)+'\n')


if __name__ == '__main__':
    main()
