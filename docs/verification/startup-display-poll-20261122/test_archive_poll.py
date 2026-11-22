"""Verify independent replay of sanitized display-poll projections."""

import copy
import importlib.util
import json
from pathlib import Path
import statistics
import subprocess
import sys
import tempfile
import unittest
from unittest import mock


HERE = Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location('display_poll_archive', HERE / 'archive_poll.py')
ARCHIVE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(ARCHIVE)
COLLECTOR_TEST_PATH = next(path for path in (
    HERE / 'test_collect_display.py',
    HERE.parent / 'docs/verification/startup-display-poll-20261122/test_collect_display.py',
) if path.is_file())
COLLECTOR_TEST_SPEC = importlib.util.spec_from_file_location('display_poll_collector_tests', COLLECTOR_TEST_PATH)
COLLECTOR_TESTS = importlib.util.module_from_spec(COLLECTOR_TEST_SPEC)
COLLECTOR_TEST_SPEC.loader.exec_module(COLLECTOR_TESTS)


def helper(queries=5, *, poll=(27, 200), stdout_open=(24, 175), after_eof=(3, 25)):
    counts = {
        'launch_approval': 1, 'permit_acquire': 1, 'permit_initial': 1,
        'query_outer_pre': queries, 'query_outer_post': queries, 'query_inner_pre': queries, 'query_inner_post': queries,
        'query_inner_periodic': 0, 'metadata_outer_pre': 2, 'metadata_outer_post': 2,
        'metadata_checkpoint': 6, 'metadata_1': 1, 'metadata_2': 1, 'commands_run': queries,
        'command_spawn': queries, 'command_read': queries, 'command_try_wait': queries, 'command_poll': poll[0],
        'command_poll_stdout_open': stdout_open[0], 'command_poll_after_eof': after_eof[0],
    }
    elapsed = {
        'launch_approval': 20, 'permit_acquire': 30, 'permit_initial': 10,
        'query_outer_pre': 25, 'query_outer_post': 25, 'query_inner_pre': 20, 'query_inner_post': 20,
        'query_inner_periodic': 0, 'metadata_outer_pre': 10, 'metadata_outer_post': 10,
        'metadata_checkpoint': 60, 'metadata_1': 70, 'metadata_2': 70, 'commands_run': 500,
        'command_spawn': 50, 'command_read': 40, 'command_try_wait': 60, 'command_poll': poll[1],
        'command_poll_stdout_open': stdout_open[1], 'command_poll_after_eof': after_eof[1],
    }
    buckets = {key: {'count': counts[key], 'elapsed_ns': elapsed[key]} for key in ARCHIVE.BUCKETS}
    residuals = {
        'helper_outside_top_level_buckets': 1000 - sum(elapsed[key] for key in ARCHIVE.TOP),
        'commands_outside_measured_children': elapsed['commands_run'] - sum(elapsed[key] for key in ARCHIVE.COMMAND),
        'metadata_outside_measured_checkpoints': elapsed['metadata_1'] + elapsed['metadata_2'] - elapsed['metadata_checkpoint'],
    }
    parent = {'count': poll[0], 'elapsed_ns': poll[1]}

    def child(sample):
        return {
            'count': sample[0],
            'elapsed_ns': sample[1],
            'share_of_poll_count': sample[0] / parent['count'] if parent['count'] else None,
            'share_of_poll_elapsed': sample[1] / parent['elapsed_ns'] if parent['elapsed_ns'] else None,
        }

    return {
        'query_branch': 'automatic-five' if queries == 5 else 'fixed-three',
        'helper_elapsed_ns': 1000,
        'process_stat_reads': 288,
        'buckets': buckets,
        'residual_ns': residuals,
        'poll_partition': {
            'parent': parent,
            'stdout_open': child(stdout_open),
            'after_eof': child(after_eof),
        },
    }


def source_complete_series(helpers):
    if len(helpers) != ARCHIVE.PLANNED_VISITS or len({row['query_branch'] for row in helpers}) != 1:
        return None
    parents = [row['poll_partition']['parent'] for row in helpers]
    result = {'query_branch': helpers[0]['query_branch'], 'visits': 3, 'poll_partition': {
        'parent': {field: statistics.median([parent[field] for parent in parents]) for field in ('count', 'elapsed_ns')}
    }}
    for name in ARCHIVE.POLL_CHILDREN:
        children = [row['poll_partition'][name] for row in helpers]
        result['poll_partition'][name] = {
            field: statistics.median([child[field] for child in children]) for field in ('count', 'elapsed_ns')
        }
        for field in ('share_of_poll_count', 'share_of_poll_elapsed'):
            samples = [child[field] for child in children]
            result['poll_partition'][name][field] = statistics.median(samples) if all(value is not None for value in samples) else None
    return result


def projection(branches=(5, 5, 5), *, statuses=('observed', 'observed', 'observed'),
               polls=((27, 200, 24, 175, 3, 25), (24, 210, 20, 170, 4, 40), (27, 220, 21, 120, 6, 100))):
    visits, helpers = [], []
    for ordinal, (queries, status, values) in enumerate(zip(branches, statuses, polls), 1):
        start = ordinal * 100
        accepted = None if ordinal == 2 else [start + 15, start + 25]
        driver = None if accepted is None or ordinal == 3 else [start + 30, start + 40]
        source_helper = helper(queries, poll=values[:2], stdout_open=values[2:4], after_eof=values[4:])
        if status == 'observed':
            helpers.append(source_helper)
        visits.append({
            'ordinal': ordinal,
            'status': status,
            'helper': source_helper if status == 'observed' else None,
            'card_to_shelf_renderer_interval_ns': [start, start + 10] if status in ('observed', 'eligible-shelf-no-poll-partition') else None,
            'card_to_accepted_send_interval_ns': accepted if status in ('observed', 'eligible-shelf-no-poll-partition') else None,
            'card_to_same_marker_driver_interval_ns': driver if status in ('observed', 'eligible-shelf-no-poll-partition') else None,
        })
    counts = {
        'automatic-five': sum(row['query_branch'] == 'automatic-five' for row in helpers),
        'fixed-three': sum(row['query_branch'] == 'fixed-three' for row in helpers),
    }
    return {
        'schema': ARCHIVE.SCHEMA,
        'planned': 3,
        'observed': len(helpers),
        'counts_by_query_branch': counts,
        'complete_series_medians': source_complete_series(helpers),
        'visits': visits,
        'scope': ARCHIVE.SCOPE,
    }


def walk_values(value):
    if type(value) is dict:
        for item in value.values():
            yield from walk_values(item)
    elif type(value) is list:
        for item in value:
            yield from walk_values(item)
    else:
        yield value


class ArchivePollTests(unittest.TestCase):
    def test_replays_24_of_27_without_rounding_or_parent_duplication(self):
        result = ARCHIVE.replay(projection())
        first = result['visits'][0]['helper']
        self.assertEqual(first['buckets']['command_poll'], {'count': 27, 'elapsed_ns': 200})
        self.assertEqual(first['poll_partition']['stdout_open'], {
            'count': 24, 'elapsed_ns': 175,
            'share_of_poll_count': {'numerator': 24, 'denominator': 27},
            'share_of_poll_elapsed': {'numerator': 175, 'denominator': 200},
        })
        self.assertNotIn('parent', first['poll_partition'])
        self.assertEqual(first['residual_ns']['commands_outside_measured_children'], 110)
        summary = result['complete_series_medians']['poll_partition']
        self.assertEqual(summary['parent'], {'count': 27, 'elapsed_ns': 210})
        self.assertEqual(summary['stdout_open']['share_of_poll_count'], {'numerator': 5, 'denominator': 6})
        self.assertEqual(summary['stdout_open']['share_of_poll_elapsed'], {'numerator': 17, 'denominator': 21})
        self.assertEqual(summary['after_eof']['share_of_poll_count'], {'numerator': 1, 'denominator': 6})
        self.assertFalse(any(type(value) is float for value in walk_values(result)))

    def test_zero_parent_produces_undefined_shares_not_zero(self):
        source = projection(polls=((0, 0, 0, 0, 0, 0),) * 3)
        result = ARCHIVE.replay(source)
        first = result['visits'][0]['helper']
        self.assertEqual(first['buckets']['command_poll'], {'count': 0, 'elapsed_ns': 0})
        self.assertIsNone(first['poll_partition']['stdout_open']['share_of_poll_count'])
        self.assertIsNone(first['poll_partition']['after_eof']['share_of_poll_elapsed'])
        self.assertIsNone(result['complete_series_medians']['poll_partition']['stdout_open']['share_of_poll_count'])

    def test_rejects_count_and_time_partition_errors(self):
        for field in ('count', 'elapsed_ns'):
            source = projection()
            visit = source['visits'][0]['helper']
            parent = visit['poll_partition']['parent']
            child = visit['poll_partition']['stdout_open']
            child[field] += 1
            child['share_of_poll_count'] = child['count'] / parent['count']
            child['share_of_poll_elapsed'] = child['elapsed_ns'] / parent['elapsed_ns']
            with self.subTest(field=field), self.assertRaisesRegex(ValueError, 'partition'):
                ARCHIVE.replay(source)

    def test_preserves_optional_send_and_primary_visit_data_without_medians(self):
        source = projection(statuses=('observed', 'observed', 'eligible-shelf-no-poll-partition'))
        source['observed'] = 2
        source['counts_by_query_branch'] = {'automatic-five': 2, 'fixed-three': 0}
        source['complete_series_medians'] = None
        result = ARCHIVE.replay(source)
        self.assertIsNone(result['complete_series_medians'])
        self.assertEqual(result['counts_by_query_branch']['automatic-five'], 2)
        self.assertIsNone(result['visits'][1]['card_to_accepted_send_interval_ns'])
        self.assertIsNone(result['visits'][1]['card_to_same_marker_driver_interval_ns'])
        self.assertEqual(result['visits'][2]['card_to_shelf_renderer_interval_ns'], [300, 310])
        self.assertEqual(len(result['visits']), 3)

    def test_mixed_query_branches_keep_visits_but_do_not_pool(self):
        source = projection(branches=(5, 5, 3))
        self.assertIsNone(source['complete_series_medians'])
        result = ARCHIVE.replay(source)
        self.assertIsNone(result['complete_series_medians'])
        self.assertEqual(result['counts_by_query_branch'], {'automatic-five': 2, 'fixed-three': 1})
        self.assertEqual([visit['helper']['query_branch'] for visit in result['visits']],
                         ['automatic-five', 'automatic-five', 'fixed-three'])

    def test_rejects_driver_without_send_and_tampered_parent_or_summary(self):
        mutations = (
            lambda value: value['visits'][0].update({'card_to_accepted_send_interval_ns': None}),
            lambda value: value['visits'][0]['helper']['poll_partition']['parent'].update({'count': 26}),
            lambda value: value['complete_series_medians']['poll_partition']['parent'].update({'count': 26}),
        )
        for mutate in mutations:
            source = copy.deepcopy(projection())
            mutate(source)
            with self.assertRaises(ValueError):
                ARCHIVE.replay(source)

    def test_requires_the_exact_twenty_bucket_poll_schema(self):
        source = projection()
        self.assertEqual(len(ARCHIVE.BUCKETS), 20)
        for visit in source['visits']:
            buckets = visit['helper']['buckets']
            buckets.pop(ARCHIVE.POLL_CHILD_BUCKETS['stdout_open'])
            buckets.pop(ARCHIVE.POLL_CHILD_BUCKETS['after_eof'])
        with self.assertRaisesRegex(ValueError, 'bucket names'):
            ARCHIVE.replay(source)

    def test_cli_emits_the_independent_archive_schema(self):
        raw = json.dumps(projection(), separators=(',', ':'), allow_nan=False).encode('ascii')
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'projection.json'
            path.write_bytes(raw)
            result = subprocess.run([sys.executable, '-IB', str(HERE / 'archive_poll.py'), str(path)],
                                    capture_output=True, check=False)
        self.assertEqual(result.returncode, 0, result.stderr.decode('utf-8', 'replace'))
        report = json.loads(result.stdout)
        self.assertEqual(report['schema'], 'bili-display-poll-archive-v1')
        self.assertEqual(report['observed'], 3)
        self.assertIn('never add or subtract the children again', report['accounting'])

    def test_real_collector_analyze_projection_roundtrips_through_archive_cli(self):
        collector = COLLECTOR_TESTS.POLL
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            fixture_owner = COLLECTOR_TESTS.FIXTURE.TEST_OWNER
            with mock.patch.object(collector.CONTROL, 'HOST', root), mock.patch.dict(collector.CONTROL.os.environ, {'BILICOMICS_STUDY_OWNER': fixture_owner}):
                cases = [COLLECTOR_TESTS.make_case(root, ordinal) for ordinal in (1, 2, 3)]
                plan = {
                    'schema': collector.SCHEMA,
                    'owner': fixture_owner,
                    'host_boot': COLLECTOR_TESTS.BOOT,
                    'cases': cases,
                }
                source = collector.analyze(plan)

            self.assertEqual(source['observed'], 3)
            for visit in source['visits']:
                buckets = visit['helper']['buckets']
                partition = visit['helper']['poll_partition']
                for child_name in ARCHIVE.POLL_CHILDREN:
                    self.assertEqual(
                        buckets[ARCHIVE.POLL_CHILD_BUCKETS[child_name]],
                        {field: partition[child_name][field] for field in ('count', 'elapsed_ns')},
                    )

            payload = json.dumps(source, separators=(',', ':'), allow_nan=False).encode('ascii')
            projection_path = root / 'collector-projection.json'
            projection_path.write_bytes(payload)
            accepted = subprocess.run([sys.executable, '-IB', str(HERE / 'archive_poll.py'), str(projection_path)],
                                      capture_output=True, check=False)
            self.assertEqual(accepted.returncode, 0, accepted.stderr.decode('utf-8', 'replace'))
            archive = json.loads(accepted.stdout)
            self.assertEqual(archive['schema'], ARCHIVE.ARCHIVE_SCHEMA)
            for visit in archive['visits']:
                buckets = visit['helper']['buckets']
                partition = visit['helper']['poll_partition']
                self.assertEqual(set(buckets), ARCHIVE.BUCKETS)
                for child_name in ARCHIVE.POLL_CHILDREN:
                    self.assertEqual(
                        buckets[ARCHIVE.POLL_CHILD_BUCKETS[child_name]],
                        {field: partition[child_name][field] for field in ('count', 'elapsed_ns')},
                    )

            tampered = copy.deepcopy(source)
            tampered['visits'][0]['helper']['buckets'][ARCHIVE.POLL_CHILD_BUCKETS['stdout_open']]['count'] += 1
            projection_path.write_text(json.dumps(tampered, separators=(',', ':'), allow_nan=False), encoding='ascii')
            rejected = subprocess.run([sys.executable, '-IB', str(HERE / 'archive_poll.py'), str(projection_path)],
                                      capture_output=True, check=False)
            self.assertNotEqual(rejected.returncode, 0)
            self.assertEqual(rejected.stdout, b'')


if __name__ == '__main__':
    unittest.main()
