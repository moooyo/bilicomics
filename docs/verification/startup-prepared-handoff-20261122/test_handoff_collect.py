"""Isolated fixture tests; no Kindle, lease or network is used."""

import importlib.util
import json
from pathlib import Path
import subprocess
import tempfile
from types import SimpleNamespace
import unittest
from unittest import mock


HERE = Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location('handoff_collect', HERE / 'handoff_collect.py')
HANDOFF = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(HANDOFF)
BOOT = 'aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee'
TEST_OWNER = '00000000-0000-4000-8000-000000000001'
PID = 401
BASE = 9_007_199_254_750_101


def data(value):
    return (json.dumps(value, separators=(',', ':')) + '\n').encode('ascii')


def save(batch, leaf, value):
    (batch / leaf).write_bytes(data(value))


def new_case(root, ordinal):
    run = 'bili-shelf-' + f'{ordinal:032x}'
    path = root / ('bilicomics-interactive-' + run)
    path.mkdir(mode=0o700)
    save(path, 'admission.json', {'owner': TEST_OWNER, 'run': run, 'host_boot': BOOT})
    save(path, 'handoff-visit.json', {'schema': HANDOFF.SCHEMA, 'ordinal': ordinal, 'treatment': 'frozen-stock-prepared-handoff', 'run': run})
    save(path, 'handoff-inputs-verified.json', {'schema': 'bili-prepared-handoff-inputs-v1', 'run_id': run, 'ordinal': ordinal,
         'source': 'frozen-installed-control', 'comparison': 'complete-three-archive-byte-comparison'})
    for name in ('route.tar.gz', 'probe.tar.gz', 'native-guard.sh', 'askpass'):
        (path / name).write_bytes(b'synthetic')
    return {'ordinal': ordinal, 'run': run, 'batch': str(path)}


def complete(case, gap, source, *, optional=True, state='ready'):
    path, run = Path(case['batch']), case['run']
    base = BASE + case['ordinal'] * 2_000_000_000
    for leaf in ('route.tar.gz', 'probe.tar.gz', 'askpass'):
        (path / leaf).unlink()
    for leaf in ('attempted', 'click-intended', 'capture-attempted'):
        (path / leaf).touch()
    save(path, 'released.json', {'state': 'released', 'run_id': run, 'owner_thread': TEST_OWNER, 'owner_and_token_checked': True,
         'native_restored_and_connections_closed': True, 'device_disposition': 'original-native-restored'})
    save(path, 'origin.json', {'boot': BOOT})
    save(path, 'retired.json', {'natural': True, 'app_pid': PID})
    click = {'ok': True, 'target': 'open-card', 'clock': 'CLOCK_MONOTONIC', 'mode': 'XTest-button-1', 'hold_requested_ms': 50,
             'buttons_clear_after': True, 'up_sent': True, 'down_confirmed': True, 'down_before_ns': base - 50000005,
             'down_after_ns': base - 50000000, 'up_before_ns': base, 'up_after_ns': base + 4}
    save(path, 'observed-click.json', {'run_id': run, 'boot_id': BOOT, 'click': click})
    save(path, 'card.json', HANDOFF.BOUNDS.project_card_origin(click, run_id=run, boot_id=BOOT, process_id=PID))
    render = base + 1_000_000_000
    events = [('trace_started', base + 31, {'schema': HANDOFF.BOUNDS.TRACE_SCHEMA, 'run_id': run, 'boot_id': BOOT, 'clock': HANDOFF.BOUNDS.CLOCK}),
              ('shelf_rendered', render + 1, {'shelf_state': state, 'render_sequence': 4, 'render_order': 'render_then_wait', 'rendered_monotonic_ns': render})]
    if optional:
        events.extend([('shelf_submitted', render + 4, {'shelf_state': state, 'render_sequence': 4, 'update_marker': 41, 'submitted_monotonic_ns': render + 3}),
                       ('shelf_driver_completed', render + 6, {'shelf_state': state, 'render_sequence': 4, 'update_marker': 41, 'completed_monotonic_ns': render + 5})])
    events.append(('trace_stopped', render + 12, {'reason': 'finished', 'truncated': False}))
    rows = [{'schema_version': 1, 'process_id': PID, 'operation_id': 0, 'kind': 'startup' if stage.startswith('shelf_') else 'diagnostic',
             'stage': stage, 'monotonic_ns': moment, 'thread_cpu_ns': None, 'thread_id': PID, 'trace_elapsed_us': 0, 'data': raw}
            for stage, moment, raw in events]
    (path / (run + '.jsonl')).write_bytes(b''.join(map(data, rows)))
    name = HANDOFF.sidecar_name(run)
    save(path, name, {'schema': HANDOFF.SIDE_SCHEMA, 'run_id': run, 'boot_id': BOOT, 'process_id': PID, 'clock': HANDOFF.BOUNDS.CLOCK,
                      'prepared_monotonic_ns': base + 1000, 'received_monotonic_ns': base + 1000 + gap, 'receive_source': source})
    return path, name


class ApprovedFixture(unittest.TestCase):
    def setUp(self):
        super().setUp()
        owner = mock.patch.dict(HANDOFF.CONTROL.os.environ, {'BILICOMICS_STUDY_OWNER': TEST_OWNER})
        owner.start()
        self.addCleanup(owner.stop)


class HandoffTests(ApprovedFixture):
    def setUp(self):
        super().setUp()
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)
        patch = mock.patch.object(HANDOFF.CONTROL, 'HOST', self.root)
        patch.start()
        self.addCleanup(patch.stop)
        self.cases = [new_case(self.root, ordinal) for ordinal in (1, 2, 3)]
        self.plan = {'schema': HANDOFF.SCHEMA, 'host_boot': BOOT, 'owner': TEST_OWNER, 'cases': self.cases}

    def test_three_predeclared_rows_and_inclusive_threshold(self):
        for case, gap, source in zip(self.cases, [249999999, 250000000, 250000001], ['maintenance_timer', 'runtime_wakeup', 'maintenance_timer']):
            complete(case, gap, source, optional=case['ordinal'] != 2)
        result = HANDOFF.analyze(self.plan)
        self.assertEqual((result['answer'], result['eligible_observations'], result['at_or_above_threshold']), ('observed', 3, 2))
        self.assertEqual([(row['producer_to_ui_receive_ns'], row['receive_source']) for row in result['visits']],
                         [(249999999, 'maintenance_timer'), (250000000, 'runtime_wakeup'), (250000001, 'maintenance_timer')])
        self.assertEqual(result['visits'][1]['card_to_shelf_renderer_interval_ns'], [999999996, 1000000000])
        self.assertIsNone(result['visits'][1]['card_to_same_marker_driver_interval_ns'])
        for private in (BOOT, self.cases[0]['run'], str(self.root), 'process_id', 'monotonic_ns', str(BASE)):
            self.assertNotIn(private, json.dumps(result))

    def test_all_below_only_describes_this_series(self):
        for case in self.cases:
            complete(case, 0, 'runtime_wakeup')
        result = HANDOFF.analyze(self.plan)
        self.assertEqual(result['answer'], 'not-observed-in-three')
        self.assertEqual(result['at_or_above_threshold'], 0)

    def test_missing_sidecar_does_not_erase_the_primary_or_count_as_zero(self):
        complete(self.cases[0], 9, 'maintenance_timer')
        path, name = complete(self.cases[1], 19, 'runtime_wakeup')
        (path / name).unlink()
        result = HANDOFF.analyze(self.plan)
        self.assertEqual((result['answer'], result['eligible_observations']), ('inconclusive', 1))
        self.assertIsNotNone(result['visits'][1]['card_to_shelf_renderer_interval_ns'])
        self.assertIsNone(result['visits'][1]['producer_to_ui_receive_ns'])
        self.assertEqual(result['visits'][2]['status'], 'not-attempted')
        with self.assertRaisesRegex(ValueError, 'Missing eligible'):
            HANDOFF.validate_closed(path)

    def test_no_observation_or_partial_hit_never_implies_three_valid_launches(self):
        self.assertEqual(HANDOFF.analyze(self.plan)['answer'], 'inconclusive')
        complete(self.cases[0], HANDOFF.LIMIT, 'maintenance_timer')
        partial = HANDOFF.analyze(self.plan)
        self.assertEqual((partial['answer'], partial['eligible_observations']), ('observed', 1))
        self.assertEqual([row['status'] for row in partial['visits']], ['observed', 'not-attempted', 'not-attempted'])

    def test_mismatched_run_clock_pid_unknown_source_extra_field_or_duplicate_rejected(self):
        case = self.cases[0]
        path, name = complete(case, HANDOFF.LIMIT, 'runtime_wakeup')
        original = (path / name).read_bytes()
        for changes in ({'boot_id': 'other'}, {'run_id': self.cases[1]['run']}, {'process_id': True}, {'process_id': PID + 1},
                        {'clock': 'CLOCK_REALTIME'}, {'receive_source': 'private source'}, {'private field': 'fixture'}, {'received_monotonic_ns': 1},
                        {'received_monotonic_ns': (1 << 64) - 1}, {'prepared_monotonic_ns': 1}):
            save(path, name, {**json.loads(original), **changes})
            with self.assertRaises(ValueError) as error:
                HANDOFF.validate_closed(path)
            self.assertNotIn('private', str(error.exception).replace('private content withheld', ''))
        (path / name).write_bytes(original.replace(b'"process_id":', b'"process_id":1,"process_id":'))
        with self.assertRaises(ValueError):
            HANDOFF.validate_closed(path)

    def test_non_ready_or_unbound_closed_visit_never_supplies_latency(self):
        case = self.cases[0]
        path, _ = complete(case, HANDOFF.LIMIT, 'runtime_wakeup', state='empty')
        row = HANDOFF.analyze(self.plan)['visits'][0]
        self.assertEqual(row['status'], 'closed-without-eligible-shelf')
        self.assertIsNone(row['at_least_250_ms'])
        closed = json.loads((path / 'released.json').read_bytes())
        save(path, 'released.json', {**closed, 'owner_thread': 'other'})
        with self.assertRaisesRegex(ValueError, 'closure'):
            HANDOFF.analyze(self.plan)

    def test_no_output_if_busy_or_visits_reordered(self):
        (Path(self.cases[0]['batch']) / 'attempted').touch()
        with self.assertRaisesRegex(ValueError, 'no exact release'):
            HANDOFF.analyze(self.plan)
        (Path(self.cases[0]['batch']) / 'attempted').unlink()
        complete(self.cases[1], HANDOFF.LIMIT, 'runtime_wakeup')
        with self.assertRaisesRegex(ValueError, 'follows'):
            HANDOFF.analyze(self.plan)

    def test_late_single_leaf_capture_is_write_once_and_not_a_timing_audit(self):
        case = self.cases[0]
        path, name = complete(case, 22, 'runtime_wakeup')
        original = (path / name).read_bytes()
        (path / name).unlink()
        visit = SimpleNamespace(path=path, run=case['run'], route='/mnt/us/' + case['run'], boot=BOOT, raw=mock.Mock(return_value=original))
        with self.assertRaises(ValueError):
            HANDOFF.collect(visit)
        visit.raw.assert_not_called()
        (path / 'released.json').unlink()
        with mock.patch.object(HANDOFF.BOUNDS, 'audit', side_effect=AssertionError('analysis before exact release')):
            HANDOFF.collect(visit)
        self.assertEqual(json.loads((path / name).read_bytes()), json.loads(original))
        with self.assertRaises(ValueError):
            HANDOFF.collect(visit)
        visit.raw.assert_called_once()

    def test_private_stage_links_or_foreign_markers_never_read(self):
        case = self.cases[0]
        run = case['run']
        path, name = complete(case, 15, 'runtime_wakeup')
        stage = self.root / run
        stage.mkdir(mode=0o700)
        (stage / 'data').mkdir(mode=0o700)
        (stage / 'run-id').write_bytes((run + '\n').encode('ascii'))
        leaf = stage / 'data' / name
        source = (path / name).read_bytes()
        leaf.write_bytes(source)
        route = '/mnt/us/' + run
        command = HANDOFF.capture_command(run, route).replace(route, str(stage))
        def capture():
            return subprocess.run(['sh', '-ec', command], capture_output=True, timeout=10)
        self.assertEqual(capture().stdout, source)
        leaf.unlink()
        leaf.symlink_to(path / name)
        self.assertNotEqual(capture().returncode, 0)
        self.assertEqual(capture().stdout, b'')
        leaf.unlink()
        leaf.write_bytes(source)
        (stage / 'run-id').write_text(self.cases[1]['run'] + '\n')
        self.assertNotEqual(capture().returncode, 0)
        self.assertEqual(capture().stdout, b'')


if __name__ == '__main__':
    unittest.main()
