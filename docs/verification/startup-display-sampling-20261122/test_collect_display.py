"""Check closed display-cost projections using private fixtures and a replacement media tree."""

import copy
import importlib.util
import json
from pathlib import Path
import subprocess
import tempfile
from types import SimpleNamespace
import unittest
from unittest import mock


HERE = Path(__file__).resolve().parent


def module(path):
    spec = importlib.util.spec_from_file_location(path.stem, path)
    loaded = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(loaded)
    return loaded


DISPLAY = module(HERE / 'collect_display.py')
FIXTURE = module(HERE.parent / 'startup-prepared-handoff-20261122/test_handoff_collect.py')
BOOT = FIXTURE.BOOT
PID = FIXTURE.PID


def record(run, start, queries=5):
    counters = {'launch_approval': (1, 20), 'permit_acquire': (1, 30), 'permit_initial': (1, 10),
                'query_outer_pre': (queries, 50), 'query_outer_post': (queries, 50), 'metadata_outer_pre': (2, 20),
                'metadata_outer_post': (2, 20), 'metadata_1': (1, 150), 'metadata_2': (1, 150),
                'metadata_checkpoint': (6, 115), 'commands_run': (queries, 700), 'query_inner_pre': (queries, 50),
                'query_inner_post': (queries, 50), 'query_inner_periodic': (0, 0), 'command_spawn': (queries, 100),
                'command_read': (2 * queries, 40), 'command_try_wait': (queries, 20), 'command_poll': (queries, 400)}
    return {'schema': 'kindle-startup-display-cost-v1', 'clock': 'CLOCK_MONOTONIC', 'run_id': run, 'boot_id': BOOT,
            'supervisor_pid': 501, 'native_read_pid': 511, 'helper_pid': 517, 'native_read_sequence': 1, 'supervisor_birth': 2930,
            'native_read_birth': 2937, 'helper_birth': 2937, 'process_stat_reads': 448, 'helper_started_monotonic_ns': start, 'helper_finished_monotonic_ns': start + 1400,
            'buckets': {key: {'count': count, 'elapsed_ns': duration} for key, (count, duration) in counters.items()}}


def rust_bytes(row):
    serialized = {key: row[key] for key in ('schema', 'clock', 'run_id', 'boot_id', *DISPLAY.WIRE_IDENTITIES)}
    serialized['buckets'] = {key: {field: row['buckets'][key][field] for field in ('elapsed_ns', 'count')} for key in DISPLAY.WIRE_BUCKETS}
    return FIXTURE.data(serialized)


def display_case(root, ordinal, *, complete=False, optional=True):
    case = FIXTURE.new_case(root, ordinal)
    path, run = Path(case['batch']), case['run']
    if complete:
        _, handoff = FIXTURE.complete(case, 1, 'runtime_wakeup', optional=optional)
        (path / handoff).unlink()
        trace = path / (run + '.jsonl')
        rows = [json.loads(line) for line in trace.read_bytes().splitlines()]
        start = rows[0]['monotonic_ns']
        rows[0]['monotonic_ns'] = start + 10000
        trace.write_bytes(b''.join(map(FIXTURE.data, rows)))
        FIXTURE.save(path, DISPLAY.name_for(run), record(run, start))
    (path / 'handoff-visit.json').unlink()
    (path / 'handoff-inputs-verified.json').unlink()
    FIXTURE.save(path, 'display-visit.json', {'schema': DISPLAY.SCHEMA, 'treatment': 'frozen-stock-display-sampling', 'ordinal': ordinal, 'run': run})
    FIXTURE.save(path, 'display-inputs-verified.json', {'schema': 'bili-display-sampling-inputs-v1', 'ordinal': ordinal, 'run_id': run,
                'source': 'frozen-installed-control', 'comparison': 'complete-three-archive-byte-comparison'})
    return case


class DisplayTests(FIXTURE.ApprovedFixture):
    def setUp(self):
        super().setUp()
        private = tempfile.TemporaryDirectory()
        self.addCleanup(private.cleanup)
        self.root = Path(private.name)
        patch = mock.patch.object(DISPLAY.CONTROL, 'HOST', self.root)
        patch.start()
        self.addCleanup(patch.stop)

    def plan(self, number):
        cases = [display_case(self.root, ordinal, complete=ordinal <= number, optional=ordinal != 2) for ordinal in (1, 2, 3)]
        return {'schema': DISPLAY.SCHEMA, 'owner': FIXTURE.TEST_OWNER, 'host_boot': BOOT, 'cases': cases}

    def test_three_same_treatment_visits_narrow_projection_and_nested_residuals(self):
        plan = self.plan(3)
        result = DISPLAY.analyze(plan)
        self.assertEqual((result['planned'], result['observed']), (3, 3))
        row = result['visits'][1]
        self.assertEqual(row['card_to_shelf_renderer_interval_ns'], [999999996, 1000000000])
        self.assertIsNone(row['card_to_accepted_send_interval_ns'])
        self.assertIsNone(row['card_to_same_marker_driver_interval_ns'])
        helper = row['helper']
        self.assertEqual(helper['helper_elapsed_ns'], 1400)
        self.assertEqual(helper['residual_ns'], {'helper_outside_top_level_buckets': 200, 'commands_outside_measured_children': 40,
                         'metadata_outside_measured_checkpoints': 185})
        self.assertEqual(helper['buckets']['metadata_checkpoint']['count'], 6)
        self.assertEqual(result['complete_series_medians']['process_stat_reads'], 448)
        for private in (plan['cases'][0]['run'], BOOT, 'helper_pid', 'native_read_pid', str(self.root), 'helper_birth', 'helper_started_monotonic_ns'):
            self.assertNotIn(private, json.dumps(result))

    def test_identity_buckets_and_frame_scope_are_strict(self):
        run = 'bili-shelf-' + 'c' * 32
        original = record(run, 999, 3)
        for changes in ({'run_id': run.replace('c', 'f')}, {'boot_id': 'other'}, {'clock': 'CLOCK_REALTIME'},
                        {'helper_pid': PID}, {'native_read_pid': 517}, {'supervisor_birth': True}, {'native_read_sequence': 0},
                        {'helper_birth': 1234}, {'property-value': 'private-value'}, {'buckets': {'guard': 1}}):
            with self.assertRaises(ValueError) as error:
                DISPLAY.decode(FIXTURE.data({**original, **changes}), run, BOOT, PID)
            self.assertNotIn('private-value', str(error.exception))
        for value in (False, -1, 2**64, '100'):
            altered = copy.deepcopy(original)
            altered['buckets']['command_read']['elapsed_ns'] = value
            with self.assertRaises(ValueError):
                DISPLAY.decode(FIXTURE.data(altered), run, BOOT, PID)
        altered = copy.deepcopy(original)
        altered['process_stat_reads'] = 2**32
        with self.assertRaises(ValueError):
            DISPLAY.decode(FIXTURE.data(altered), run, BOOT, PID)
        altered = copy.deepcopy(original)
        altered['buckets']['command_read']['count'] = 2**32
        with self.assertRaises(ValueError):
            DISPLAY.decode(FIXTURE.data(altered), run, BOOT, PID)
        encoded = FIXTURE.data(original).replace(b'"helper_pid":', b'"helper_pid":50,"helper_pid":')
        with self.assertRaises(ValueError):
            DISPLAY.decode(encoded, run, BOOT, PID)
        self.assertEqual(DISPLAY.components(original, 998, 5000)['helper_elapsed_ns'], 1400)
        for lo, hi in ((1000, 6000), (0, 1000)):
            with self.assertRaises(ValueError):
                DISPLAY.components(original, lo, hi)

    def test_fixed_path_is_qualified_but_mixed_query_counts_have_no_pooled_median(self):
        plan = self.plan(3)
        for index in (1, 0, 2):
            case = plan['cases'][index]
            path = Path(case['batch'])
            leaf = DISPLAY.name_for(case['run'])
            stamp = json.loads((path / leaf).read_bytes())['helper_started_monotonic_ns']
            FIXTURE.save(path, leaf, record(case['run'], stamp, 3))
            self.assertEqual(DISPLAY.validate_closed(path), BOOT)
            result = DISPLAY.analyze(plan)
            self.assertEqual(result['observed'], 3)
            self.assertEqual(result['visits'][index]['helper']['query_branch'], 'fixed-three')
            self.assertIsNotNone(result['visits'][index]['card_to_shelf_renderer_interval_ns'])
            if index != 2:
                self.assertIsNone(result['complete_series_medians'])
        self.assertEqual(result['counts_by_query_branch'], {'automatic-five': 0, 'fixed-three': 3})
        self.assertEqual(result['complete_series_medians']['query_branch'], 'fixed-three')

    def test_elapsed_is_not_a_replacement_watchdog_decision(self):
        raw = record('bili-shelf-' + 'a' * 32, 100)
        raw['helper_finished_monotonic_ns'] = 2_000_000_201
        self.assertEqual(DISPLAY.components(raw, 5, 2_000_000_205)['helper_elapsed_ns'], 2_000_000_101)

    def test_impossible_counts_or_children_are_not_reported_as_residual(self):
        base = record('bili-shelf-' + 'a' * 32, 100)
        for key, field, number in (('commands_run', 'count', 4), ('permit_initial', 'count', 2), ('query_outer_post', 'count', 4),
                                   ('metadata_checkpoint', 'count', 5), ('command_try_wait', 'count', 1), ('command_poll', 'elapsed_ns', 702),
                                   ('metadata_checkpoint', 'elapsed_ns', 301), ('launch_approval', 'elapsed_ns', 1401)):
            sample = copy.deepcopy(base)
            sample['buckets'][key][field] = number
            with self.assertRaises(ValueError):
                DISPLAY.components(sample, 1, 2000)

    def test_late_unique_leaf_capture_and_separation_from_timing(self):
        case = self.plan(1)['cases'][0]
        run, path = case['run'], Path(case['batch'])
        name = DISPLAY.name_for(run)
        contents = (path / name).read_bytes()
        (path / name).unlink()
        call = mock.Mock(return_value=contents)
        visit = SimpleNamespace(run=run, path=path, route='/mnt/us/' + run, boot=BOOT, raw=call)
        with self.assertRaises(ValueError):
            DISPLAY.collect(visit)
        (path / 'released.json').unlink()
        with mock.patch.object(DISPLAY.BOUNDS, 'audit', side_effect=AssertionError('analysis before exact release')):
            DISPLAY.collect(visit)
        self.assertEqual((path / name).read_bytes(), contents)
        with self.assertRaises(ValueError):
            DISPLAY.collect(visit)
        call.assert_called_once()
        self.assertNotIn('launcher.log', call.call_args.args[0])

    def test_device_read_rejects_foreign_markers_hardlinks_and_size_overflow(self):
        run = 'bili-shelf-' + 'b' * 32
        route = '/mnt/us/' + run
        stage = self.root / run
        stage.mkdir(mode=0o700)
        (stage / 'data').mkdir()
        (stage / 'run-id').write_text(run + '\n')
        leaf = stage / 'data' / DISPLAY.name_for(run)
        contents = rust_bytes(record(run, 1000))
        leaf.write_bytes(contents)
        command = DISPLAY.capture_command(run, route, BOOT).replace(route, str(stage))
        def capture():
            return subprocess.run(['sh', '-ec', command], capture_output=True, timeout=8)
        self.assertEqual(capture().stdout, contents)
        for injected in (contents.replace(b'"schema":', b'"private-field":"must-not-transfer","schema":'),
                         contents.replace(b'"count":1', b'"count":1,"private-field":"must-not-transfer"', 1),
                         contents + b'must-not-transfer\n', contents[:-1], contents.replace(b'2,"count"', b'"secret","count"') + b'extra'):
            leaf.write_bytes(injected)
            result = capture()
            self.assertNotEqual(result.returncode, 0)
            self.assertEqual(result.stdout, b'')
        leaf.write_bytes(contents)
        (self.root / 'another-name').hardlink_to(leaf)
        self.assertNotEqual(capture().returncode, 0)
        (self.root / 'another-name').unlink()
        leaf.write_bytes(b'x' * (DISPLAY.MAX_RECORD + 1))
        self.assertNotEqual(capture().returncode, 0)
        leaf.write_bytes(contents)
        (stage / 'run-id').write_text('bili-shelf-' + 'a' * 32 + '\n')
        self.assertEqual(capture().stdout, b'')
        self.assertNotEqual(capture().returncode, 0)

    def test_missing_sidecar_keeps_primary_and_halts_unattempted_suffix(self):
        plan = self.plan(2)
        case = plan['cases'][1]
        path = Path(case['batch'])
        (path / DISPLAY.name_for(case['run'])).unlink()
        result = DISPLAY.analyze(plan)
        self.assertEqual([row['status'] for row in result['visits']], ['observed', 'eligible-shelf-no-capture-breakdown', 'not-attempted'])
        self.assertIsNotNone(result['visits'][1]['card_to_shelf_renderer_interval_ns'])
        self.assertIsNone(result['visits'][1]['helper'])
        self.assertIsNone(result['complete_series_medians'])
        with self.assertRaisesRegex(ValueError, 'stop without retry'):
            DISPLAY.validate_closed(path)
        (Path(plan['cases'][2]['batch']) / 'attempted').touch()
        with self.assertRaisesRegex(ValueError, 'no exact release'):
            DISPLAY.analyze(plan)

    def test_unverified_ownership_source_or_retirement_cannot_qualify(self):
        case = self.plan(1)['cases'][0]
        path = Path(case['batch'])
        source = path / 'display-inputs-verified.json'
        admitted = source.read_bytes()
        source.write_bytes(admitted.replace(b'complete-three-archive-byte-comparison', b'same-size'))
        with self.assertRaises(ValueError):
            DISPLAY.validate_closed(path)
        source.write_bytes(admitted)
        lease = path / 'released.json'
        original = json.loads(lease.read_bytes())
        FIXTURE.save(path, 'released.json', {**original, 'owner_and_token_checked': False})
        with self.assertRaises(ValueError):
            DISPLAY.validate_closed(path)
        FIXTURE.save(path, 'released.json', original)
        (path / 'stop-intended').touch()
        with self.assertRaises(ValueError):
            DISPLAY.validate_closed(path)


if __name__ == '__main__':
    unittest.main()
