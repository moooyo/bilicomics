"""Exercise the orchestrator using synthetic SSH, input, stock and lease boundaries."""

import copy
import importlib.util
import io
import json
from pathlib import Path
import tarfile
import tempfile
from types import SimpleNamespace
import unittest
from unittest import mock


HERE = Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location('shelf_operator', HERE / 'shelf_operator.py')
operator = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(operator)
RUN = 'bili-shelf-' + 'a' * 32
BOOT = 'a0a0a0a0-bbbb-cccc-dddd-e0e0e0e0e0e0'
TEST_OWNER = '00000000-0000-4000-8000-000000000001'
CLICK = {'clock': 'CLOCK_MONOTONIC', 'target': 'open-card', 'ok': True, 'mode': 'XTest-button-1',
         'hold_requested_ms': 50, 'down_confirmed': True, 'buttons_clear_after': True, 'up_sent': True,
         'down_before_ns': 100000000, 'down_after_ns': 101000000, 'up_before_ns': 151000000, 'up_after_ns': 152000000}


def trace_row(stage, timestamp, data):
    return {'schema_version': 1, 'process_id': 201, 'operation_id': 0,
            'kind': 'diagnostic' if stage.startswith('trace_') else 'startup', 'stage': stage,
            'monotonic_ns': timestamp, 'thread_cpu_ns': None, 'thread_id': 201, 'trace_elapsed_us': 0, 'data': data}


def trace_bytes(rows):
    return b''.join(json.dumps(row).encode('ascii') + b'\n' for row in rows)


class LeaseFailure(Exception):
    def __init__(self, code, **progress):
        super().__init__(code)
        self.code, self.details = code, progress


class FakeLease:
    LeaseError = LeaseFailure

    def __init__(self, request, events):
        self.request, self.events = request, events
        self.held = False
        self.busy = False

    def acquire(self, request):
        assert request is self.request and not self.held
        self.events.append('acquire')
        if self.busy:
            raise LeaseFailure('lease_exists', lease_directory_created=False)
        request.token_file.write_text('fixture-token')
        self.held = True

    def status(self, request):
        assert self.held and request is self.request and request.token_file.read_text() == 'fixture-token'

    def release(self, request, *, native_restored_and_connections_closed=False):
        self.status(request)
        assert native_restored_and_connections_closed
        self.events.append('release')
        self.held = False
        return {'state': 'released', 'owner_thread': request.owner_thread, 'run_id': request.run_id,
                'owner_and_token_checked': True, 'native_restored_and_connections_closed': True}


class Fixture(operator.Operator):
    def __init__(self, path):
        self.path, self.run = path, RUN
        self.route, self.detector = '/mnt/us/' + RUN, '/mnt/us/bili-probe-' + RUN
        self.admission = {'minimum_media_kib': 200000, 'native_roster': sorted(operator.REQUIRED), 'route_bytes': 3, 'probe_bytes': 3}
        self.phase, self.boot, self.password = 'admission', None, ''
        self.probe_action = self.probe_step = None
        self.tab_dispatch = self.card_dispatch = 'not-requested'
        self.tab, self.pointer = 'home', [47, 21]
        self.events, self.commands = [], []
        self.request = SimpleNamespace(owner_thread=TEST_OWNER, run_id=RUN, token_file=path / 'private-token')
        self.lease = FakeLease(self.request, self.events)
        self.selected_count, self.active, self.wait_result = 1, False, 'app_pid=201'
        self.refusal, self.boot_change, self.uncertain_click = None, False, False
        self.card_data = copy.deepcopy(CLICK)
        self.rows = [trace_row('trace_started', 200000000, {'schema': 'kindle-shelf-render-log-v1', 'clock': 'CLOCK_MONOTONIC',
                                                           'run_id': RUN, 'boot_id': BOOT}),
                     trace_row('shelf_rendered', 221000000, {'shelf_state': 'ready', 'render_sequence': 4,
                                                             'render_order': 'wait_then_render',
                                                             'rendered_monotonic_ns': 220000000}),
                     trace_row('shelf_submitted', 230000001, {'shelf_state': 'ready', 'render_sequence': 4, 'update_marker': 6,
                                                              'submitted_monotonic_ns': 230000000}),
                     trace_row('trace_stopped', 240000000, {'reason': 'finished', 'truncated': False})]
        self.trace_suffix = b''
        self.bounds = operator.module(HERE / 'log_bounds.py')
        self.shared = SimpleNamespace(paired=SimpleNamespace(selected=lambda item: item['selected']))
        self.stage_stamp = ['1:2']
        (path / 'route.tar.gz').write_bytes(b'kit')
        (path / 'probe.tar.gz').write_bytes(b'kit')
        (path / 'askpass').write_bytes(operator.ASKPASS)

    def guard(self, verb):
        self.lease.status(self.request)
        self.events.append(verb)
        if self.refusal == verb or (verb in ('wait', 'stop') and self.uncertain_click):
            raise operator.OperatorError('Guard refused')
        if verb == 'native':
            return 'nonmatching' if self.boot_change and self.boot is not None else BOOT
        if verb == 'wait':
            return self.wait_result
        return ''

    def switch(self, verb):
        self.lease.status(self.request)
        self.events.append(verb)
        if self.refusal == verb:
            raise operator.OperatorError('Switch refused')
        if verb == 'activate':
            self.active = True
        elif verb == 'restore':
            assert not (self.path / 'click-intended').exists() or (self.path / 'retired.json').exists()
            self.active = False
        elif verb == 'cleanup':
            assert not self.active and (self.path / 'route-cleaning.json').exists()
        return {'inspect': 'stock', 'activate': 'armed', 'restore': 'stock', 'cleanup': 'removed'}[verb]

    def stock_receipt(self):
        return self.stage_stamp

    def raw(self, command, data=None, deadline=25):
        self.lease.status(self.request)
        self.commands.append(command)
        if command.startswith('space='):
            self.events.append('capacity')
            if self.refusal == 'capacity':
                raise operator.OperatorError('Insufficient media space')
        if command.startswith('DISPLAY='):
            words = command.split()
            mode = words[words.index('--library-path') + 3]
            self.events.append(mode)
            if mode in ('navigation', 'card'):
                result = {'selected': self.tab, 'pointer': self.pointer[:], 'title': {'count': self.selected_count}}
            elif mode.startswith('nav-'):
                self.tab = mode[4:]
                self.pointer = [100, 100]
                result = {'ok': True, 'target': mode, 'buttons_clear_after': True}
            elif mode == 'open-card':
                assert self.active and (self.path / 'click-intended').is_file()
                if self.uncertain_click:
                    raise operator.OperatorError('Ambiguous native click')
                result = self.card_data
            elif mode == 'restore-pointer':
                self.pointer = [int(value) for value in words[-2:]]
                result = {'restored': True}
            else:
                raise AssertionError('Unexpected public input')
            return json.dumps(result).encode('ascii')
        if 'head -c' in command:
            self.events.append('project')
            return trace_bytes(self.rows) + self.trace_suffix
        return b''


class OperatorTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.path = Path(self.temp.name)
        self.batch = Fixture(self.path)
        for patch in (mock.patch.object(operator.sys.stdin, 'isatty', return_value=True),
                      mock.patch.object(operator.getpass, 'getpass', return_value='fixture-secret'),
                      mock.patch.object(operator.time, 'sleep'), mock.patch.object(operator, 'announce')):
            patch.start()
            self.addCleanup(patch.stop)
        self.expected_audit = self.batch.bounds.audit
        self.batch.bounds.audit = self.audit_after_return

    def audit_after_return(self, *arguments):
        self.assertFalse(self.batch.lease.held)
        self.assertTrue((self.path / 'released.json').is_file())
        self.assertFalse((self.path / 'private-token').exists())
        self.batch.events.append('analyze')
        return self.expected_audit(*arguments)

    def released(self):
        self.assertFalse(self.batch.lease.held)
        self.assertEqual(self.batch.events.count('release'), 1)
        self.assertFalse((self.path / 'private-token').exists())
        self.assertFalse((self.path / 'askpass').exists())
        self.assertEqual(self.batch.password, '')
        return operator.load(self.path / 'released.json')['device_disposition']

    def assert_probe_summary(self, phase, action, step, tab, card):
        self.assertEqual(operator.load(self.path / 'probe-failure.json'), {
            'schema': 'bili-native-probe-failure-v1', 'phase': phase, 'action': action, 'step': step,
            'tab_request': tab, 'card_request': card})

    def test_one_native_click_then_render_no_secondary_input(self):
        self.batch.execute(False)
        self.assertEqual(self.released(), 'original-native-restored')
        self.assertFalse((self.path / 'probe-failure.json').exists())
        events = self.batch.events
        self.assertEqual(events.count('open-card'), 1)
        self.assertLess(events.index('activate'), events.index('open-card'))
        self.assertLess(events.index('wait'), events.index('restore'))
        self.assertLess(events.index('restore'), events.index('cleanup'))
        self.assertLess(events.index('project'), events.index('release'))
        self.assertLess(events.index('release'), events.index('analyze'))
        self.assertEqual((self.batch.tab, self.batch.pointer), ('home', [47, 21]))
        self.assertFalse((self.path / 'navigation.json').exists())
        result = operator.load(self.path / 'intervals.json')
        self.assertEqual(result['renderer_return_interval_ns'], [68000000, 69000000])
        self.assertEqual(result['render_order'], 'wait_then_render')
        projected_rows = self.batch.bounds.read_jsonl(self.path / (RUN + '.jsonl'))
        self.assertEqual(projected_rows[1]['data']['render_order'], 'wait_then_render')
        self.assertEqual(set(operator.load(self.path / 'card.json')), self.batch.bounds.ORIGIN_FIELDS)
        self.assertIn(self.batch.detector + '/models', next(cmd for cmd in self.batch.commands if ' open-card ' in cmd))

    def test_busy_or_auth_failure_does_not_mutate(self):
        self.batch.lease.busy = True
        with self.assertRaisesRegex(operator.OperatorError, 'busy'):
            self.batch.execute(False)
        self.assertEqual(self.batch.events, ['acquire'])
        self.assertFalse(self.batch.commands or self.batch.lease.held)
        self.assertEqual(self.batch.password, '')
        with self.assertRaisesRegex(operator.OperatorError, 'new run'):
            self.batch.execute(False)

    def test_two_controllers_cannot_share_even_one_exact_lease(self):
        descriptor = operator.os.open(self.path, operator.os.O_RDONLY | operator.os.O_DIRECTORY)
        try:
            operator.fcntl.flock(descriptor, operator.fcntl.LOCK_EX | operator.fcntl.LOCK_NB)
            with self.assertRaisesRegex(operator.OperatorError, 'Another controller'):
                self.batch.execute(True)
            self.assertEqual(self.batch.events, [])
        finally:
            operator.os.close(descriptor)

    def test_native_or_authentication_blocked_releases_without_restoration_claim(self):
        self.batch.refusal = 'native'
        with self.assertRaisesRegex(operator.SampleUnavailable, 'native-preflight'):
            self.batch.execute(False)
        self.assertEqual(self.released(), 'no-device-change')
        self.assertEqual(self.batch.events, ['acquire', 'native', 'release'])

    def test_insufficient_capacity_prevents_upload(self):
        self.batch.refusal = 'capacity'
        with self.assertRaisesRegex(operator.SampleUnavailable, 'media-capacity'):
            self.batch.execute(False)
        self.assertEqual(self.released(), 'no-device-change')
        self.assertFalse((self.path / 'detector-intended').exists())

    def test_ambiguous_card_does_not_arm_or_click(self):
        self.batch.selected_count = 2
        with self.assertRaisesRegex(operator.SampleUnavailable, 'identify-card'):
            self.batch.execute(False)
        self.assertEqual(self.released(), 'original-native-restored')
        self.assertNotIn('activate', self.batch.events)
        self.assertNotIn('open-card', self.batch.events)
        self.assert_probe_summary('identify-card', 'identify-card', 'response-validation', 'receipt-confirmed', 'not-requested')

    def test_failed_guard_precedes_any_tab_or_card_request(self):
        guard = self.batch.guard
        def refuse(verb):
            if self.batch.phase == 'native-navigation':
                raise operator.OperatorError('Scoped SSH failed (255; withheld)')
            return guard(verb)
        with mock.patch.object(self.batch, 'guard', side_effect=refuse):
            with self.assertRaisesRegex(operator.SampleUnavailable, 'native-navigation'):
                self.batch.execute(False)
        self.released()
        self.assert_probe_summary('native-navigation', 'read-navigation', 'native-guard', 'not-requested', 'not-requested')
        self.assertNotIn('navigation', self.batch.events)
        self.assertNotIn('open-card', self.batch.events)

    def test_navigation_ssh_failure_after_tab_ack_preserves_the_first_failure_after_release(self):
        raw = self.batch.raw
        self.batch.ssh = ['mock-transport']
        def lost_observation(command, data=None, deadline=25):
            if self.batch.phase == 'native-navigation' and self.batch.tab == 'library' and command.startswith('DISPLAY='):
                return operator.Operator.raw(self.batch, command, data, deadline)
            return raw(command, data, deadline)
        response = SimpleNamespace(returncode=255, stdout=b'private response', stderr=b'private transport text coordinates 19,23')
        save = operator.save
        def check_order(path, payload):
            if path.name == 'probe-failure.json':
                self.assertFalse(self.batch.lease.held)
                self.assertTrue((self.path / 'released.json').is_file())
                self.assertTrue((self.path / 'failure.json').is_file())
            return save(path, payload)
        with mock.patch.object(self.batch, 'raw', side_effect=lost_observation), mock.patch.object(operator, 'save', side_effect=check_order), \
             mock.patch.object(operator.subprocess, 'run', return_value=response) as transport:
            with self.assertRaisesRegex(operator.SampleUnavailable, r'native-navigation: Scoped SSH failed \(255; withheld\)'):
                self.batch.execute(False)
            transport.assert_called_once()
        self.released()
        self.assert_probe_summary('native-navigation', 'read-navigation', 'operation-request', 'receipt-confirmed', 'not-requested')
        self.assertEqual(operator.load(self.path / 'failure.json'), {'phase': 'native-navigation', 'reason': 'Scoped SSH failed (255; withheld)'})
        self.assertEqual(self.batch.events.count('nav-library'), 1)
        self.assertFalse((self.path / 'activation-intended').exists() or (self.path / 'click-intended').exists())
        self.assertTrue((self.path / 'navigation-restored').is_file())

    def test_ambiguous_tab_request_is_not_retried_or_relabelled_during_recovery(self):
        raw = self.batch.raw
        def lose_tab_receipt(command, data=None, deadline=25):
            result = raw(command, data, deadline)
            if self.batch.phase == 'native-navigation' and command.startswith('DISPLAY=') and ' nav-library ' in command:
                raise operator.OperatorError('Scoped SSH failed (255; withheld)')
            return result
        with mock.patch.object(self.batch, 'raw', side_effect=lose_tab_receipt):
            with self.assertRaisesRegex(operator.SampleUnavailable, 'native-navigation'):
                self.batch.execute(False)
        self.released()
        self.assert_probe_summary('native-navigation', 'request-target-tab', 'operation-request', 'dispatch-possible', 'not-requested')
        self.assertEqual(self.batch.events.count('nav-library'), 1)
        self.assertNotIn('open-card', self.batch.events)
        self.assertEqual(self.batch.tab, 'home')

    def test_optional_detail_write_failure_never_prevents_return_or_changes_the_failure(self):
        self.batch.selected_count = 2
        save = operator.save
        def no_space(path, payload):
            if path.name == 'probe-failure.json':
                self.assertFalse(self.batch.lease.held)
                raise OSError('private disk path')
            return save(path, payload)
        with mock.patch.object(operator, 'save', side_effect=no_space):
            with self.assertRaisesRegex(operator.SampleUnavailable, 'identify-card'):
                self.batch.execute(False)
        self.assertEqual(self.released(), 'original-native-restored')
        self.assertTrue((self.path / 'failure.json').is_file())
        self.assertFalse((self.path / 'probe-failure.json').exists())

    def test_accepted_card_receipt_does_not_establish_a_recorded_origin_when_saving_it_fails(self):
        save = operator.save
        def fail_origin(path, payload):
            if path.name == 'observed-click.json':
                raise OSError('private path')
            return save(path, payload)
        with mock.patch.object(operator, 'save', side_effect=fail_origin):
            with self.assertRaisesRegex(operator.SampleUnavailable, 'card-release: operation-failed'):
                self.batch.execute(False)
        self.released()
        self.assert_probe_summary('card-release', 'open-card', 'receipt-accepted', 'receipt-confirmed', 'receipt-confirmed')
        self.assertEqual(self.batch.events.count('open-card'), 1)
        self.assertFalse((self.path / 'card.json').exists() or (self.path / 'intervals.json').exists())

    def test_rejected_volume_restored_before_release(self):
        self.batch.refusal = 'activate'
        with self.assertRaisesRegex(operator.SampleUnavailable, 'route-activate'):
            self.batch.execute(False)
        self.assertEqual(self.released(), 'original-native-restored')
        self.assertLess(self.batch.events.index('restore'), self.batch.events.index('cleanup'))
        self.assertNotIn('open-card', self.batch.events)

    def test_stock_refusal_preserves_safe_reason_then_offline_replays_original_observation(self):
        self.batch.ssh = ['fixture-only']
        switch = self.batch.switch

        def refuse_restore(verb):
            if verb == 'restore':
                return operator.Operator.raw(self.batch, ':')
            return switch(verb)

        response = SimpleNamespace(returncode=1, stdout=b'', stderr=(
            b'private unrelated detail\nroute: route paths or retained launcher do not match this run\n'))
        with mock.patch.object(self.batch, 'switch', side_effect=refuse_restore), \
             mock.patch.object(operator.subprocess, 'run', return_value=response):
            with self.assertRaisesRegex(operator.RecoveryRequired, r'stock-restore: Scoped SSH failed \(1; unrecognized-route\)') as refusal:
                self.batch.execute(False)
        self.assertNotIn('private unrelated detail', str(refusal.exception))
        self.assertTrue(self.batch.lease.held)
        self.assertEqual(operator.load(self.path / 'retired.json'), {'natural': True, 'app_pid': 201})
        self.assertFalse((self.path / 'stop-intended').exists() or (self.path / 'released.json').exists())
        with self.assertRaisesRegex(operator.SampleUnavailable, 'offline log_bounds.py replay'):
            self.batch.execute(True)
        self.assertEqual(self.released(), 'original-native-restored')
        self.assertEqual(self.batch.events.count('open-card'), 1)
        self.assertEqual(self.batch.events.count('acquire'), 1)
        self.assertFalse((self.path / 'intervals.json').exists() or (self.path / 'partial-trace').exists())
        replay = operator.subprocess.run([operator.sys.executable, '-IB', HERE / 'log_bounds.py', self.path / 'card.json',
                                          self.path / (RUN + '.jsonl')], check=True, capture_output=True)
        self.assertEqual(json.loads(replay.stdout)['renderer_return_interval_ns'], [68000000, 69000000])

    def test_unclassified_recovery_detail_is_not_exposed(self):
        with mock.patch.object(self.batch, 'stock_receipt', side_effect=ValueError('private exception detail')):
            with self.assertRaisesRegex(operator.RecoveryRequired, 'remove-route-stage: operation-failed') as refusal:
                self.batch.execute(False)
        self.assertNotIn('private exception detail', str(refusal.exception))
        self.assertTrue(self.batch.lease.held)

    def test_unknown_dispatch_cannot_restore_stock_or_release_until_same_recovery(self):
        self.batch.uncertain_click = True
        with self.assertRaises(operator.RecoveryRequired):
            self.batch.execute(False)
        self.assertTrue(self.batch.lease.held and self.batch.active)
        self.assertNotIn('restore', self.batch.events)
        self.assertNotIn('release', self.batch.events)
        self.assertTrue((self.path / 'click-intended').is_file())
        self.assertFalse((self.path / 'probe-failure.json').exists())
        self.batch.uncertain_click = False
        with self.assertRaisesRegex(operator.SampleUnavailable, 'recovered'):
            self.batch.execute(True)
        self.assertEqual(self.released(), 'original-native-restored')
        self.assertEqual(self.batch.events.count('open-card'), 1)
        self.assertEqual(self.batch.events.count('acquire'), 1)
        self.assertFalse((self.path / 'intervals.json').exists())
        self.assertFalse((self.path / 'probe-failure.json').exists())

    def test_missing_independent_pid_retains_anonymous_rows_only(self):
        self.batch.wait_result = ''
        with self.assertRaises(operator.SampleUnavailable):
            self.batch.execute(False)
        self.released()
        self.assertFalse((self.path / 'card.json').exists())
        self.assertTrue((self.path / (RUN + '.jsonl')).is_file())
        self.assertTrue((self.path / 'observed-click.json').is_file())
        self.assertFalse((self.path / 'intervals.json').exists())

    def test_private_or_duplicate_event_field_never_written_or_printed(self):
        self.batch.rows[1]['data']['must-not-export-title'] = 'private fixture value'
        with self.assertRaisesRegex(operator.SampleUnavailable, 'No qualified sample'):
            self.batch.execute(False)
        self.released()
        self.assertFalse((self.path / (RUN + '.jsonl')).exists())
        self.assertNotIn('private fixture value', ''.join(item.read_text() for item in self.path.iterdir() if item.is_file()))

    def test_extra_click_fields_are_not_persisted(self):
        self.batch.card_data['coordinates'] = [82, 124]
        with self.assertRaisesRegex(operator.SampleUnavailable, 'card-release'):
            self.batch.execute(False)
        self.released()
        self.assertFalse((self.path / 'card.json').exists() or (self.path / 'observed-click.json').exists())
        self.assertNotIn('coordinates', (self.path / 'failure.json').read_text())
        self.assert_probe_summary('card-release', 'open-card', 'response-validation', 'receipt-confirmed', 'dispatch-possible')
        self.assertEqual(self.batch.events.count('open-card'), 1)

    def test_partial_last_record_is_anonymous_but_not_qualified(self):
        self.batch.trace_suffix = b'{"untested-private'
        with self.assertRaises(operator.SampleUnavailable):
            self.batch.execute(False)
        self.released()
        self.assertTrue((self.path / 'partial-trace').is_file())
        self.assertNotIn(b'untested-private', (self.path / (RUN + '.jsonl')).read_bytes())
        self.assertFalse((self.path / 'intervals.json').is_file())

    def test_accepted_send_alone_is_not_primary_timing(self):
        self.batch.rows.pop(1)
        with self.assertRaisesRegex(operator.SampleUnavailable, 'Secondary evidence only'):
            self.batch.execute(False)
        self.released()
        result = operator.load(self.path / 'intervals.json')
        self.assertIsNone(result['renderer_return_interval_ns'])
        self.assertEqual(result['accepted_send_interval_ns'], [78000000, 79000000])

    def test_no_frame_returns_no_timing(self):
        self.batch.rows = [self.batch.rows[0], self.batch.rows[-1]]
        with self.assertRaisesRegex(operator.SampleUnavailable, 'No qualified Shelf frame'):
            self.batch.execute(False)
        self.released()
        result = operator.load(self.path / 'intervals.json')
        self.assertEqual(result['status'], 'no-qualified-frame')
        self.assertIsNone(result['renderer_return_interval_ns'])

    def test_link_change_or_unexpected_boot_must_not_release_as_restored(self):
        self.batch.boot_change = True
        with self.assertRaises(operator.RecoveryRequired):
            self.batch.execute(False)
        self.assertTrue(self.batch.lease.held)
        self.assertNotIn('open-card', self.batch.events)
        self.assertNotIn('release', self.batch.events)

    def test_interrupted_cleanup_accepts_absence_only_with_previous_receipt(self):
        operator.save(self.path / 'route-cleaning.json', self.batch.stage_stamp)
        self.batch.raw = mock.Mock(return_value=b'absent\n')
        self.batch.remove_route()
        self.assertNotIn('cleanup', self.batch.events)
        self.batch.raw = mock.Mock(return_value=b'present\n')
        self.batch.guard = mock.Mock(return_value=BOOT)
        self.batch.boot = BOOT
        self.batch.stage_stamp = ['1:99']
        with self.assertRaisesRegex(operator.OperatorError, 'original objects'):
            self.batch.remove_route()


class PreparationTests(unittest.TestCase):
    def test_live_owner_must_match_external_approval_not_just_current_thread(self):
        with mock.patch.object(operator.socket, 'gethostname', return_value='test-env'), mock.patch.object(operator.os, 'geteuid', return_value=0), \
             mock.patch.object(Path, 'is_dir', return_value=True), mock.patch.object(Path, 'read_text', return_value=BOOT):
            for config in ({}, {'CODEX_THREAD_ID': TEST_OWNER}, {'BILICOMICS_STUDY_OWNER': TEST_OWNER},
                           {'CODEX_THREAD_ID': BOOT, 'BILICOMICS_STUDY_OWNER': TEST_OWNER},
                           {'CODEX_THREAD_ID': '', 'BILICOMICS_STUDY_OWNER': ''},
                           {'CODEX_THREAD_ID': 'invalid', 'BILICOMICS_STUDY_OWNER': 'invalid'}):
                with self.subTest(config=config), mock.patch.dict(operator.os.environ, config, clear=True), self.assertRaises(operator.OperatorError):
                    operator.identity()
            with mock.patch.dict(operator.os.environ, {'CODEX_THREAD_ID': TEST_OWNER, 'BILICOMICS_STUDY_OWNER': TEST_OWNER}, clear=True):
                self.assertEqual(operator.identity(), (TEST_OWNER, BOOT))

    def test_refuse_traversal_links_duplicates_and_wrong_expanded_size(self):
        with tempfile.TemporaryDirectory() as temporary:
            target = Path(temporary) / 'source.tar.gz'
            baseline = sorted(operator.REQUIRED)
            for case in ('valid', 'traversal', 'symlink', 'duplicate', 'size'):
                names = baseline + (['lib/../unknown'] if case == 'traversal' else [baseline[0]] if case == 'duplicate' else [])
                with tarfile.open(target, 'w:gz') as archive:
                    for name in names:
                        item = tarfile.TarInfo(name)
                        item.mode, item.size = (0o600 if name.startswith('models/') else 0o700), 1
                        if case == 'symlink' and name == baseline[0]:
                            item.type, item.linkname = tarfile.SYMTYPE, 'another'
                        archive.addfile(item, io.BytesIO(b'x'))
                receipt = {'members': names, 'bytes': len(names) + (1 if case == 'size' else 0)}
                with self.subTest(case=case):
                    if case == 'valid':
                        self.assertEqual(operator.probe_roster(target, receipt), baseline)
                    else:
                        with self.assertRaises(operator.OperatorError):
                            operator.probe_roster(target, receipt)

    def test_no_arbitrary_cleanup_roots_or_entries(self):
        with self.assertRaises(operator.OperatorError):
            operator.cleanup_payload('/mnt/us/documents', RUN, operator.FILES)
        with self.assertRaises(operator.OperatorError):
            operator.cleanup_payload('/mnt/us/' + RUN, RUN, {'known/../../foreign'})
        command = operator.cleanup_payload('/mnt/us/' + RUN, RUN, operator.FILES)
        self.assertNotIn('rm -rf', command)
        self.assertNotIn('find ', command)
        self.assertIn('0:1', command)


if __name__ == '__main__':
    unittest.main()
