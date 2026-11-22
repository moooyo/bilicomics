"""Verify the independently replayed archive accepts only sanitized projections."""

import copy
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


HERE = Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location('display_projection_archive', HERE / 'archive_display_projection.py')
ARCHIVE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(ARCHIVE)


def helper(elapsed_ns, process_stat_reads):
    counts = {
        'launch_approval': 1, 'permit_acquire': 1, 'permit_initial': 1,
        'query_outer_pre': 5, 'query_outer_post': 5, 'query_inner_pre': 5, 'query_inner_post': 5,
        'query_inner_periodic': 0, 'metadata_outer_pre': 2, 'metadata_outer_post': 2,
        'metadata_checkpoint': 6, 'metadata_1': 1, 'metadata_2': 1, 'commands_run': 5,
        'command_spawn': 5, 'command_read': 5, 'command_try_wait': 5, 'command_poll': 5,
    }
    durations = {
        'launch_approval': 10, 'permit_acquire': 20, 'permit_initial': 5,
        'query_outer_pre': 15, 'query_outer_post': 15, 'query_inner_pre': 50, 'query_inner_post': 50,
        'query_inner_periodic': 0, 'metadata_outer_pre': 4, 'metadata_outer_post': 4,
        'metadata_checkpoint': 60, 'metadata_1': 70, 'metadata_2': 70, 'commands_run': 350,
        'command_spawn': 100, 'command_read': 20, 'command_try_wait': 30, 'command_poll': 40,
    }
    buckets = {key: {'count': counts[key], 'elapsed_ns': durations[key]} for key in ARCHIVE.BUCKETS}
    top = elapsed_ns - sum(durations[key] for key in ARCHIVE.TOP)
    commands = durations['commands_run'] - sum(durations[key] for key in ARCHIVE.COMMAND)
    metadata = durations['metadata_1'] + durations['metadata_2'] - durations['metadata_checkpoint']
    return {
        'query_branch': 'automatic-five',
        'helper_elapsed_ns': elapsed_ns,
        'process_stat_reads': process_stat_reads,
        'buckets': buckets,
        'residual_ns': {
            'helper_outside_top_level_buckets': top,
            'commands_outside_measured_children': commands,
            'metadata_outside_measured_checkpoints': metadata,
        },
    }


def projection():
    visits = []
    for ordinal in range(1, 4):
        visits.append({
            'ordinal': ordinal,
            'status': 'observed',
            'helper': helper(700 + 100 * (ordinal - 1), 50 + 2 * (ordinal - 1)),
            'card_to_shelf_renderer_interval_ns': [100 * ordinal, 100 * ordinal + 10],
            'card_to_accepted_send_interval_ns': None,
            'card_to_same_marker_driver_interval_ns': None,
        })
    median_helper = helper(800, 52)
    return {
        'schema': ARCHIVE.SCHEMA,
        'planned': 3,
        'observed': 3,
        'counts_by_query_branch': {'automatic-five': 3, 'fixed-three': 0},
        'complete_series_medians': {
            'query_branch': median_helper['query_branch'],
            'observations': 3,
            'helper_elapsed_ns': median_helper['helper_elapsed_ns'],
            'process_stat_reads': median_helper['process_stat_reads'],
            'buckets': {
                key: {'count': sample['count'], 'elapsed_ns': sample['elapsed_ns']}
                for key, sample in median_helper['buckets'].items()
            },
            'residual_ns': median_helper['residual_ns'],
        },
        'visits': visits,
        'scope': ARCHIVE.SCOPE,
    }


class ArchiveTests(unittest.TestCase):
    def test_replays_residuals_medians_and_only_public_fields(self):
        result = ARCHIVE.replay(projection())
        self.assertEqual(result['schema'], 'bili-display-sampling-archive-v1')
        self.assertEqual(result['complete_series_medians']['helper_elapsed_ns'], 800)
        self.assertEqual(result['complete_series_medians']['process_stat_reads'], 52)
        self.assertEqual(result['complete_series_medians']['residual_ns'], {
            'commands_outside_measured_children': 60,
            'helper_outside_top_level_buckets': 237,
            'metadata_outside_measured_checkpoints': 80,
        })
        for private in ('run_id', 'boot_id', 'process_id', 'helper_pid', 'helper_started_monotonic_ns'):
            self.assertNotIn(private, str(result))

    def test_keeps_primary_shelf_observations_when_breakdown_is_missing(self):
        source = projection()
        source['visits'][2]['status'] = 'eligible-shelf-no-capture-breakdown'
        source['visits'][2]['helper'] = None
        source['observed'] = 2
        source['counts_by_query_branch'] = {'automatic-five': 2, 'fixed-three': 0}
        source['complete_series_medians'] = None
        result = ARCHIVE.replay(source)
        self.assertIsNotNone(result['visits'][2]['card_to_shelf_renderer_interval_ns'])
        self.assertIsNone(result['visits'][2]['helper'])
        self.assertIsNone(result['complete_series_medians'])

    def test_rejects_private_fields_bad_residuals_and_out_of_range_counts(self):
        for mutate in (
            lambda value: value.update({'run_id': 'private'}),
            lambda value: value['visits'][0]['helper']['residual_ns'].update({'commands_outside_measured_children': 0}),
            lambda value: value['visits'][0]['helper']['buckets']['command_read'].update({'count': 2**32}),
            lambda value: value['complete_series_medians'].update({'helper_elapsed_ns': 800.0}),
        ):
            source = copy.deepcopy(projection())
            mutate(source)
            with self.assertRaises(ValueError):
                ARCHIVE.replay(source)

    def test_cli_accepts_a_sanitized_projection_and_emits_the_archive_schema(self):
        source = json.dumps(projection(), separators=(',', ':')).encode('ascii')
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'projection.json'
            path.write_bytes(source)
            result = subprocess.run([sys.executable, '-IB', str(HERE / 'archive_display_projection.py'), str(path)],
                                    capture_output=True, check=False)
        self.assertEqual(result.returncode, 0, result.stderr.decode('utf-8', 'replace'))
        report = json.loads(result.stdout)
        self.assertEqual(report['schema'], 'bili-display-sampling-archive-v1')
        self.assertEqual(report['complete_series_medians']['helper_elapsed_ns'], 800)

    def test_rejects_a_visit_after_an_incomplete_slot(self):
        source = projection()
        source['visits'][0]['status'] = 'eligible-shelf-no-capture-breakdown'
        source['visits'][0]['helper'] = None
        with self.assertRaisesRegex(ValueError, 'follows an incomplete'):
            ARCHIVE.replay(source)


if __name__ == '__main__':
    unittest.main()
