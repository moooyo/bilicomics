"""Summarize application-side boundaries from the closed launcher A/B.

This replay accepts the anonymous projection only. It never opens a device,
reads a live trace, or interprets a wall-time interval as a blocking wait.
"""

import argparse
import json
from pathlib import Path
from statistics import median


SCHEMA = 'bili-cover-probe-overlap-v1-results'
MEASURED = [(pair, arm) for pair, order in enumerate(('AB', 'BA', 'AB', 'BA', 'AB', 'BA'), 1)
            for arm in order]
EXCLUDED = [(0, 'A'), (0, 'B')]
WINDOW = 'main_to_window_shown_ns'
ADMISSION = 'window_to_worker_admission_ns'
QUEUE = 'worker_admission_to_state_start_ns'
METADATA = 'state_metadata_preparation_ns'
PREPARATION = 'metadata_to_prepared_state_ns'
HANDOFF = 'prepared_to_ui_attached_ns'
ATTACH_AND_DRAW = 'attached_to_first_shelf_render_ns'
APP_SEGMENTS = (WINDOW, ADMISSION, QUEUE, METADATA, PREPARATION, HANDOFF, ATTACH_AND_DRAW)
DISPLAY_SEGMENTS = ('render_to_accepted_send_ns', 'send_to_same_marker_completion_ns')


def summarize(data):
    visits = data['visits']
    if (data['schema'] != SCHEMA or data['offline_demo'] is not True
            or data['warmups_excluded_from_comparison'] is not True
            or [(row['pair'], row['arm']) for row in visits] != EXCLUDED + MEASURED):
        raise ValueError('Expected the complete, ordered offline Scribe series')
    cases = []
    for row in visits[len(EXCLUDED):]:
        if row['native_restored_and_exact_lease_released'] is not True or row['same_process'] is not True:
            raise ValueError('Expected an audited closed visit')
        source = row['phase_durations']
        spans = {key: source[key] for key in APP_SEGMENTS + DISPLAY_SEGMENTS}
        if any(type(value) is not int or value < 0 for value in spans.values()):
            raise ValueError('Expected observed nonnegative nanosecond spans')
        # Join adjacent intervals within each launch, never their marginal medians.
        cases.append({'pair': row['pair'], 'arm': row['arm'],
                      'rust_main_to_first_shelf_render_ns': sum(spans[key] for key in APP_SEGMENTS),
                      'rust_main_to_worker_admission_ns': spans[WINDOW] + spans[ADMISSION], **spans})
    groups = {'all': cases, **{arm: [row for row in cases if row['arm'] == arm] for arm in ('A', 'B')}}
    summaries = {}
    for name, rows in groups.items():
        timings = {}
        for key in (key for key in rows[0] if key.endswith('_ns')):
            values = [row[key] for row in rows]
            timings[key] = {'median': median(values), 'range': [min(values), max(values)]}
        summaries[name] = timings
    return {'schema': 'bili-pre-shelf-replay-v1', 'scope': 'Archived physical visits; no new launches',
            'offline_demo': True, 'measured': len(cases), 'warmups_excluded': len(EXCLUDED),
            'units': 'ns', 'summary': summaries, 'visits': cases}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('projection', type=Path)
    args = parser.parse_args()
    print(json.dumps(summarize(json.loads(args.projection.read_bytes())), indent=2))


if __name__ == '__main__':
    main()
