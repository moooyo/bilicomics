"""Export only the observations and startup fields needed to reproduce both desktop summaries."""

import argparse
import json
import os
from pathlib import Path
from statistics import median
import sys
import tarfile

sys.path.append(str(Path(__file__).resolve().parent.parent / 'startup-cover-worker-20261121'))
import stage_costs
import summarize_pairs


def select(record, keys):
    return {key: record[key] for key in keys.split()}


def write(path, value):
    with path.open('x', encoding='utf-8') as output:
        output.write(json.dumps(value, indent=2) + '\n')


def write_rows(path, rows):
    with path.open('x', encoding='utf-8') as output:
        for row in rows:
            output.write(json.dumps(row, separators=(',', ':')) + '\n')


def performance(source):
    for row in stage_costs.rows(source):
        stage = row['stage']
        if row['kind'] != 'startup' or stage not in ('process_enter', 'state_prepared', 'ui_ready', 'state-prepare-context', 'state-covers', 'attach-covers'):
            continue
        data = row['data']
        result = select(row, 'kind stage process_id thread_id monotonic_ns')
        if stage in ('process_enter', 'state_prepared'):
            result['data'] = {'observed': select(data['observed'], 'monotonic_ns')}
        elif stage == 'state-prepare-context':
            result['data'] = select(data, 'demo offline')
        elif stage in ('state-covers', 'attach-covers'):
            result['thread_cpu_ns'] = row['thread_cpu_ns']
            result['data'] = select(data, 'phase outcome wall_us cpu_us' if data['phase'] == 'end' else 'phase')
        else:
            result['data'] = {}
        yield result


def lifecycle(source):
    for row in stage_costs.rows(source):
        if row.get('stage') in ('process_enter', 'state_prepared'):
            yield select(row, 'event stage process_id monotonic_ns')
        elif row['event'] == 'ready':
            result = select(row, 'event process_id monotonic_ns route')
            result['reader_resources'] = select(row['reader_resources'], 'cover_resident_count cover_resident_bytes')
            yield result


def observation(raw):
    case = select(raw, 'case pid xid selected_arm process_executable_matches last_no_feedback_sample_ns feedback_sample_ns '
                  'last_non_shelf_sample_ns first_exact_shelf_sample_ns second_exact_shelf_sample_ns activity_states polls '
                  'full_xgetimage_intervals_ns poll_median_ns poll_max_before_match_ns visible_marker_count '
                  'ready_resident_covers ready_resident_bytes final_logical_rgb_matches')
    stages = raw['stages']
    case['stages'] = select(stages, 'process_ns producer_ns ui_record_ns ready_ns attach_gap_ns attach_outcomes '
                            'enter_to_feedback_bracket_ns enter_to_shelf_bracket_ns feedback_sample_to_producer_ns '
                            'producer_to_ui_ns ui_to_ready_ns startup_activity_pulses')
    for label in ('state-covers', 'attach-covers'):
        case['stages'][label] = select(stages[label], 'wall_us cpu_us')
    return case


def endpoints(report):
    values = {}
    for name in report['schedule'][2:]:
        case = report['cases'][name]
        stages = case['stages']
        enter = stages['process_ns']
        low, high = stages['enter_to_shelf_bracket_ns']
        timing = {label: stages[key] - enter for label, key in [('producer', 'producer_ns'), ('ui_record', 'ui_record_ns'), ('ready', 'ready_ns')]}
        timing.update(feedback_upper=stages['enter_to_feedback_bracket_ns'][1], full_shelf_upper=high,
                      shelf_uncertainty=high - low, ready_to_full_shelf_upper=high - timing['ready'])
        values[name] = timing
    keys = list(next(iter(values.values())))
    def milliseconds(samples):
        return {key: round(median(sample[key] for sample in samples) / 1e6, 3) for key in keys}
    deltas = []
    for pair in report['pairs']:
        control, candidate = [values[f'{arm}{pair["pair"]:02d}'] for arm in 'ab']
        deltas.append({'order': pair['order'], **{key: candidate[key] - control[key] for key in keys}})
    wins = sum(pair['B_minus_A_shelf_bounds_ns'][1] < 0 for pair in report['pairs'])
    losses = sum(pair['B_minus_A_shelf_bounds_ns'][0] > 0 for pair in report['pairs'])
    by_order = {group: milliseconds([delta for delta in deltas if group in ('all', delta['order'])]) for group in ('all', 'AB', 'BA')}
    earlier = all(median(delta['full_shelf_upper'] for delta in deltas if delta['order'] == group) < 0 for group in ('AB', 'BA'))
    return {'scope': 'Warmed offline ordinary desktop; time in ms. The full-window match is an observation upper bound.',
            'measured_pairs': len(deltas), 'definite_B_earlier': wins, 'definite_B_later': losses, 'overlap_or_touch': len(deltas) - wins - losses,
            'both_order_groups_first_observation_earlier': earlier, 'presentation_gate_passed': wins >= 5 and earlier,
            'arm_medians_ms': {arm: milliseconds([case for name, case in values.items() if name[0].upper() == arm]) for arm in ('A', 'B')},
            'median_paired_B_minus_A_ms': by_order, 'observed_shelf_bracket_width_ms': {
                'min': round(min(case['shelf_uncertainty'] for case in values.values()) / 1e6, 3),
                'max': round(max(case['shelf_uncertainty'] for case in values.values()) / 1e6, 3)}}


def export(source, output):
    schedule = stage_costs.SCHEDULE
    raw_table = summarize_pairs.summarize(source)
    raw_costs = stage_costs.summarize(source, 'shelf', 10)
    original = stage_costs.load(source / 'report.json')
    report = select(original, 'clock sample_interval_ns schedule sources warmups_excluded')
    report['cases'] = {}
    report['pairs'] = [select(pair, 'pair order B_minus_A_shelf_bounds_ns B_minus_A_first_observed_shelf_ns '
                              'B_minus_A_worker_cpu_us B_minus_A_ui_attach_cpu_us') for pair in original['pairs']]
    output.mkdir(mode=0o700)
    destination = output / 'trial'
    destination.mkdir()
    for name in schedule:
        case, directory = source / name, destination / name
        directory.mkdir()
        result = observation(original['cases'][name])
        report['cases'][name] = result
        write(directory / 'result.json', result)
        final = stage_costs.load(case / 'final.json')
        sidecar = select(final, 'process_id route demo offline_requested offline backend profile')
        sidecar['startup'] = select(final['startup'], 'booting error')
        write(directory / 'final.json', sidecar)
        phases = list(performance(case / 'performance.jsonl'))
        events = list(lifecycle(case / 'lifecycle.jsonl'))
        stage_costs.require(len(phases) == 10 and len(events) == 3, 'Unexpected number of allowlisted records')
        write_rows(directory / 'performance.jsonl', phases)
        write_rows(directory / 'lifecycle.jsonl', events)
    write(destination / 'report.json', report)
    write(output / 'endpoint-summary.json', endpoints(report))
    stage_costs.require(summarize_pairs.summarize(destination) == raw_table and stage_costs.summarize(destination, 'shelf', 10) == raw_costs,
                        'Reduced inputs did not reproduce both complete source summaries')
    with (output / 'paired-timings.md').open('x', encoding='utf-8') as table:
        table.write(raw_table + '\n')
    files = [destination / 'report.json', *(destination / name / leaf for name in schedule
             for leaf in ('performance.jsonl', 'lifecycle.jsonl', 'final.json', 'result.json'))]
    archive_path = output / 'desktop-stage-replay.tar.gz'
    with tarfile.open(archive_path, 'x:gz') as archive:
        for path in files:
            info = tarfile.TarInfo(path.relative_to(output).as_posix())
            info.mode, info.size = 0o600, path.stat().st_size
            with path.open('rb') as value:
                archive.addfile(info, value)
    unpacked = output / 'unpacked'
    unpacked.mkdir()
    with tarfile.open(archive_path, 'r:gz') as archive:
        stage_costs.require(archive.getnames() == [path.relative_to(output).as_posix() for path in files], 'Unexpected export member')
        for item in archive.getmembers():
            target = unpacked / item.name
            target.parent.mkdir(parents=True, exist_ok=True)
            with archive.extractfile(item) as value, target.open('xb') as copy:
                copy.write(value.read())
    stage_costs.require(summarize_pairs.summarize(unpacked / 'trial') == raw_table and
                        stage_costs.summarize(unpacked / 'trial', 'shelf', 10) == raw_costs, 'Archive replay differs from full source records')
    write(output / 'replay.json', {'cases': len(schedule), 'archive_files': len(files), 'archive_bytes': archive_path.stat().st_size,
                                   'selected_startup_events_per_case': 10, 'selected_lifecycle_events_per_case': 3,
                                   'both_summaries_match_full_records_after_extraction': True})


if __name__ == '__main__':
    os.umask(0o077)
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('trial', type=Path)
    parser.add_argument('new_output', type=Path)
    args = parser.parse_args()
    export(args.trial, args.new_output)
