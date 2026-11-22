"""Real shell filtering and offline replay over synthetic, private poll-study visits only."""

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
    item = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(item)
    return item


POLL = module(HERE / 'collect_display.py')
OLD = module(HERE.parent / 'startup-display-sampling-20261122/test_collect_display.py')
FIXTURE, BOOT, PID = OLD.FIXTURE, OLD.BOOT, OLD.PID


def record(run, start, queries=5, *, open_poll=(2, 120), eof_poll=(3, 280)):
    row = OLD.record(run, start, queries)
    row['schema'] = POLL.WIRE_SCHEMA
    buckets = row['buckets']
    for key, (count, duration) in zip(POLL.CHILDREN, (open_poll, eof_poll)):
        buckets[key] = {'count': count, 'elapsed_ns': duration}
    buckets['command_poll'] = {field: sum(buckets[key][field] for key in POLL.CHILDREN) for field in ('elapsed_ns', 'count')}
    return row


def wire(row):
    formatted = {key: row[key] for key in ('schema', 'clock', 'run_id', 'boot_id', *POLL.WIRE_IDENTITIES)}
    formatted['buckets'] = {key: {field: row['buckets'][key][field] for field in ('elapsed_ns', 'count')} for key in POLL.WIRE_BUCKETS}
    return FIXTURE.data(formatted)


def make_case(root, ordinal, complete=True, *, queries=5):
    case = OLD.display_case(root, ordinal, complete=complete, optional=ordinal != 2)
    run, batch = case['run'], Path(case['batch'])
    if complete:
        legacy = batch / OLD.DISPLAY.name_for(run)
        start = json.loads(legacy.read_bytes())['helper_started_monotonic_ns']
        legacy.unlink()
        FIXTURE.save(batch, POLL.name_for(run), record(run, start, queries))
    for name in ('display-visit.json', 'display-inputs-verified.json'):
        (batch / name).unlink()
    FIXTURE.save(batch, 'poll-visit.json', {'schema': POLL.SCHEMA, 'treatment': 'frozen-stock-display-poll', 'ordinal': ordinal, 'run': run})
    FIXTURE.save(batch, 'poll-inputs-verified.json', {'schema': 'bili-display-poll-inputs-v1', 'ordinal': ordinal, 'run_id': run,
                 'source': 'frozen-installed-control', 'comparison': 'complete-three-archive-byte-comparison'})
    return case


class PollTests(FIXTURE.ApprovedFixture):
    def setUp(self):
        super().setUp()
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        patched = mock.patch.object(POLL.CONTROL, 'HOST', self.root)
        patched.start()
        self.addCleanup(patched.stop)
        self.run_id = 'bili-shelf-' + '9' * 32

    def plan(self, count=3, branches=(5, 5, 5)):
        return {'schema': POLL.SCHEMA, 'owner': FIXTURE.TEST_OWNER, 'host_boot': BOOT,
                'cases': [make_case(self.root, index, index <= count, queries=branches[index - 1]) for index in (1, 2, 3)]}

    def test_exact_partition_does_not_reduce_commands_residual_twice(self):
        row = record(self.run_id, 100)
        checked = POLL.decode(wire(row), self.run_id, BOOT, PID)
        result = POLL.components(checked, 90, 2000)
        self.assertEqual(result['residual_ns']['commands_outside_measured_children'], 40)
        self.assertEqual(result['poll_partition']['stdout_open'], {'count': 2, 'elapsed_ns': 120, 'share_of_poll_count': .4, 'share_of_poll_elapsed': .3})
        self.assertEqual(result['poll_partition']['after_eof'], {'count': 3, 'elapsed_ns': 280, 'share_of_poll_count': .6, 'share_of_poll_elapsed': .7})
        for field in ('count', 'elapsed_ns'):
            invalid = copy.deepcopy(checked)
            invalid['buckets'][POLL.CHILDREN[0]][field] += 1
            with self.assertRaisesRegex(ValueError, 'partition'):
                POLL.components(invalid, 90, 2000)

    def test_absent_parent_or_zero_clock_interval_means_undefined_share_not_zero(self):
        empty = record(self.run_id, 100, open_poll=(0, 0), eof_poll=(0, 0))
        breakdown = POLL.components(POLL.decode(wire(empty), self.run_id, BOOT, PID), 99, 2000)['poll_partition']
        self.assertEqual(breakdown['parent'], {'count': 0, 'elapsed_ns': 0})
        self.assertIsNone(breakdown['stdout_open']['share_of_poll_elapsed'])
        self.assertIsNone(breakdown['after_eof']['share_of_poll_count'])
        coarse = record(self.run_id, 100, open_poll=(1, 0), eof_poll=(0, 0))
        breakdown = POLL.components(POLL.decode(wire(coarse), self.run_id, BOOT, PID), 99, 2000)['poll_partition']
        self.assertEqual(breakdown['stdout_open']['share_of_poll_count'], 1)
        self.assertIsNone(breakdown['stdout_open']['share_of_poll_elapsed'])

    def test_reject_old_writer_bad_bucket_types_overflow_private_fields_and_unbound_pids(self):
        original = record(self.run_id, 100)
        bad = [OLD.FIXTURE.data(OLD.record(self.run_id, 100)), wire(original).replace(b'"native_read_pid":', b'"native_read_pid":0,"native_read_pid":'),
               wire(original).replace(b'"command_poll_after_eof":', b'"unknown-key":'), wire(original)[:-1],
               FIXTURE.data({**original, 'helper_pid': PID}), FIXTURE.data({**original, 'boot_id': 'not-this-boot'})]
        for value in (True, 'private fixture text', 1 << 32, -1):
            row = copy.deepcopy(original)
            row['buckets'][POLL.CHILDREN[0]]['count'] = value
            bad.append(FIXTURE.data(row))
        for raw in bad:
            with self.assertRaises(ValueError) as failure:
                POLL.decode(raw, self.run_id, BOOT, PID)
            self.assertNotIn('private fixture text', str(failure.exception))

    def test_exact_device_wire_filters_before_transmitting_only_one_numeric_record(self):
        run = self.run_id
        stage = self.root / run
        (stage / 'data').mkdir(parents=True)
        (stage / 'run-id').write_text(run + '\n')
        path = stage / 'data' / POLL.name_for(run)
        route = '/mnt/us/' + run
        cmd = POLL.capture_command(run, route, BOOT).replace(route, str(stage))
        def visit(raw):
            path.write_bytes(raw)
            return subprocess.run(['sh', '-ec', cmd], capture_output=True, timeout=10)
        data = wire(record(run, 2**53 + 312))
        self.assertEqual(visit(data).stdout, data)
        for raw in (data.replace(b'"clock":', b'"extra-private":"do-not-return","clock":'),
                    data + b'illegal private text\n', data.replace(b'after_eof', b'post_exit'), data[:-1],
                    data.replace(b'"count":1', b'"count":1,"count":1', 1), b'x' * 8193 + b'\n'):
            result = visit(raw)
            self.assertNotEqual(result.returncode, 0)
            self.assertEqual(result.stdout, b'')
        path.unlink()
        foreign = self.root / 'other-file'
        foreign.write_bytes(data)
        path.hardlink_to(foreign)
        result = subprocess.run(['sh', '-ec', cmd], capture_output=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(result.stdout, b'')

    def test_three_visits_with_per_visit_fractions_not_ratios_of_medians(self):
        plan = self.plan()
        for case, (opened, eof) in zip(plan['cases'], (((1, 0), (2, 100)), ((2, 20), (1, 0)), ((1, 50), (3, 400)))):
            batch, run = Path(case['batch']), case['run']
            leaf = POLL.name_for(run)
            start = json.loads((batch / leaf).read_bytes())['helper_started_monotonic_ns']
            row = record(run, start, open_poll=opened, eof_poll=eof)
            row['buckets']['commands_run']['elapsed_ns'] += 60
            FIXTURE.save(batch, leaf, row)
        result = POLL.analyze(plan)
        self.assertEqual((result['observed'], result['counts_by_query_branch']['automatic-five']), (3, 3))
        self.assertEqual(result['complete_series_medians']['poll_partition']['stdout_open']['share_of_poll_elapsed'], 50 / 450)
        self.assertNotEqual(result['complete_series_medians']['poll_partition']['stdout_open']['share_of_poll_elapsed'], 20 / 100)
        self.assertIsNotNone(result['visits'][1]['card_to_shelf_renderer_interval_ns'])
        self.assertIsNone(result['visits'][1]['card_to_same_marker_driver_interval_ns'])
        for item in (self.run_id, plan['cases'][0]['run'], BOOT, 'native_read_pid', 'helper_birth', str(self.root)):
            self.assertNotIn(item, json.dumps(result))

    def test_three_and_five_are_both_eligible_but_not_pooled(self):
        plan = self.plan(branches=(3, 5, 3))
        for case in plan['cases']:
            self.assertEqual(POLL.validate_closed(case['batch']), BOOT)
        result = POLL.analyze(plan)
        self.assertEqual(result['counts_by_query_branch'], {'automatic-five': 1, 'fixed-three': 2})
        self.assertIsNone(result['complete_series_medians'])

    def test_no_sidecar_no_partition_unknown_release_or_holes_do_not_count(self):
        plan = self.plan(2)
        case = plan['cases'][1]
        batch = Path(case['batch'])
        (batch / POLL.name_for(case['run'])).unlink()
        result = POLL.analyze(plan)
        self.assertEqual([row['status'] for row in result['visits']], ['observed', 'eligible-shelf-no-poll-partition', 'not-attempted'])
        self.assertIsNotNone(result['visits'][1]['card_to_shelf_renderer_interval_ns'])
        self.assertIsNone(result['complete_series_medians'])
        (Path(plan['cases'][2]['batch']) / 'attempted').touch()
        with self.assertRaisesRegex(ValueError, 'without release'):
            POLL.analyze(plan)
        (Path(plan['cases'][2]['batch']) / 'attempted').unlink()
        (batch / 'private-token').touch()
        with self.assertRaises(ValueError):
            POLL.analyze(plan)

    def test_no_capture_until_natural_exit_and_do_not_analyze_under_lease(self):
        case = self.plan(1)['cases'][0]
        batch, run = Path(case['batch']), case['run']
        leaf = batch / POLL.name_for(run)
        sample = json.loads(leaf.read_bytes())
        (batch / 'released.json').unlink()
        leaf.unlink()
        visit = SimpleNamespace(path=batch, run=run, route='/mnt/us/' + run, boot=BOOT, raw=mock.Mock(return_value=wire(sample)))
        (batch / 'stop-intended').touch()
        with self.assertRaises(ValueError):
            POLL.collect(visit)
        visit.raw.assert_not_called()
        (batch / 'stop-intended').unlink()
        with mock.patch.object(POLL, 'components', side_effect=AssertionError('analysis before release')):
            POLL.collect(visit)
        self.assertEqual(json.loads(leaf.read_bytes()), sample)
        with self.assertRaises(ValueError):
            POLL.collect(visit)
        visit.raw.assert_called_once()


if __name__ == '__main__':
    unittest.main()
