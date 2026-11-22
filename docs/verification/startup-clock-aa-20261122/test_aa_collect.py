"""Exercise the late numeric filter and closed A/A evidence binding without a device or lease."""

import importlib.util
import json
import os
from pathlib import Path
import subprocess
import tempfile
from types import SimpleNamespace
import unittest
from unittest import mock


HERE = Path(__file__).resolve().parent


def module(name):
    spec = importlib.util.spec_from_file_location(name, HERE / (name + '.py'))
    loaded = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(loaded)
    return loaded


COLLECT = module('aa_collect')
ANALYSIS = module('analyze_aa')
BOOT = 'aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee'
TEST_OWNER = '00000000-0000-4000-8000-000000000001'
RUN = 'bili-shelf-' + 'a' * 32
BASE = 9007199254750011
APP = 1345
SHIM = 1245
SCOPE = 'Application preparation or UI rendering; not physical panel completion'
ARCHIVED_LIFECYCLE = (
    b'{"elapsed_ms":0.0,"event":"startup-stage","monotonic_ns":148907796047412,"process_id":28377,"registered_fonts":null,"stage":"process_enter","timing_scope":"Application preparation or UI rendering; not physical panel completion"}\n'
    b'{"elapsed_ms":842.8573849999999,"event":"startup-stage","monotonic_ns":148908638899720,"process_id":28377,"registered_fonts":null,"stage":"state_prepared","timing_scope":"Application preparation or UI rendering; not physical panel completion"}\n'
)


def encoded(value):
    return json.dumps(value, separators=(',', ':')) + '\n'


def save(path, data):
    path.write_text(encoded(data), encoding='ascii')


def life(stage, stamp, pid=APP):
    return {'elapsed_ms': 0.0 if stage == 'process_enter' else 2.1, 'event': 'startup-stage', 'monotonic_ns': stamp,
            'process_id': pid, 'registered_fonts': None, 'stage': stage, 'timing_scope': SCOPE}


def mark(run, stage, pid, stamp):
    return {'schema': 'bili-startup-clock-mark-v1', 'run_id': run, 'clock': 'CLOCK_MONOTONIC', 'stage': stage,
            'pid': pid, 'monotonic_ns': stamp}


def capture_lines(base=BASE):
    return (f'picked {SHIM} 602\nshim_entry {SHIM} {base + 20}\napp_preinvoke {APP} {base + 30}\n'
            f'process_enter {APP} {base + 40}\nstate_prepared {APP} {base + 60}\n').encode('ascii')


def private_case(host, index, closed=True):
    pair, arm = COLLECT.SCHEDULE[index]
    run = 'bili-shelf-' + f'{index + 1:032x}'
    path = host / ('bilicomics-interactive-' + run)
    path.mkdir(mode=0o700)
    case = {'pair': pair, 'arm': arm, 'run': run, 'batch': str(path)}
    save(path / 'admission.json', {'run': run, 'host_boot': BOOT, 'owner': TEST_OWNER})
    save(path / 'clock-visit.json', {'schema': 'bili-shelf-clock-aa-v1', 'treatment': 'frozen-stock-clock',
                                    'pair': pair, 'arm': arm, 'run': run})
    save(path / 'clock-inputs-verified.json', {'schema': 'bili-shelf-clock-inputs-v1', 'run_id': run, 'pair': pair, 'arm': arm,
                                              'source': 'frozen-installed-control', 'comparison': 'complete-ten-archive-byte-comparison'})
    if not closed:
        for leaf in ('route.tar.gz', 'probe.tar.gz', 'native-guard.sh', 'askpass'):
            (path / leaf).write_bytes(b'fixture')
        return case
    for leaf in ('attempted', 'click-intended', 'capture-attempted'):
        (path / leaf).touch()
    base = BASE + index * 100_000_000
    clk = {'clock': 'CLOCK_MONOTONIC', 'target': 'open-card', 'ok': True, 'mode': 'XTest-button-1',
           'hold_requested_ms': 50, 'down_confirmed': True, 'buttons_clear_after': True, 'up_sent': True,
           'down_before_ns': base - 50_000_020, 'down_after_ns': base - 50_000_010, 'up_before_ns': base, 'up_after_ns': base + 10}
    save(path / 'observed-click.json', {'run_id': run, 'boot_id': BOOT, 'click': clk})
    save(path / 'retired.json', {'natural': True, 'app_pid': APP})
    save(path / 'card.json', COLLECT.BOUNDS.project_card_origin(clk, run_id=run, boot_id=BOOT, process_id=APP))
    save(path / 'origin.json', {'boot': BOOT})
    save(path / 'released.json', {'state': 'released', 'device_disposition': 'original-native-restored', 'owner_and_token_checked': True,
                                  'owner_thread': TEST_OWNER, 'native_restored_and_connections_closed': True, 'run_id': run})
    save(path / COLLECT.SIDECAR, COLLECT.parse_capture(capture_lines(base), run, BOOT, APP))
    stages = [('trace_started', 50, {'schema': COLLECT.BOUNDS.TRACE_SCHEMA, 'clock': COLLECT.BOUNDS.CLOCK, 'run_id': run, 'boot_id': BOOT}),
              ('shelf_rendered', 72, {'shelf_state': 'ready', 'render_order': 'render_then_wait', 'render_sequence': 9, 'rendered_monotonic_ns': base + 70}),
              ('shelf_submitted', 81, {'shelf_state': 'ready', 'render_sequence': 9, 'submitted_monotonic_ns': base + 80, 'update_marker': 5}),
              ('shelf_driver_completed', 91, {'shelf_state': 'ready', 'render_sequence': 9, 'completed_monotonic_ns': base + 90, 'update_marker': 5}),
              ('trace_stopped', 100, {'reason': 'finished', 'truncated': False})]
    rows = [{'schema_version': 1, 'process_id': APP, 'operation_id': 0, 'kind': 'diagnostic' if stage.startswith('trace_') else 'startup',
             'stage': stage, 'monotonic_ns': base + moment, 'thread_cpu_ns': None, 'thread_id': APP, 'trace_elapsed_us': 0, 'data': data}
            for stage, moment, data in stages]
    (path / (run + '.jsonl')).write_text(''.join(map(encoded, rows)), encoding='ascii')
    return case


class FilterTests(unittest.TestCase):
    def setUp(self):
        self.addCleanup(mock.patch.stopall)
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.stage = Path(temporary.name) / RUN
        self.stage.mkdir(mode=0o700)
        for leaf in ('picked', 'data'):
            (self.stage / leaf).mkdir()
        self.boot = self.stage.parent / 'synthetic-boot'
        self.boot.write_text(BOOT + '\n')
        (self.stage / 'run-id').write_text(RUN + '\n')
        (self.stage / 'picked/entry').write_text(f'{RUN} {SHIM} 602\n')
        self.entry = self.stage / 'picked/shim-entry.json'
        self.preinvoke = self.stage / 'picked/app-preinvoke.json'
        save(self.entry, mark(RUN, 'shim_entry', SHIM, BASE + 20))
        save(self.preinvoke, mark(RUN, 'app_preinvoke', APP, BASE + 30))
        self.lifecycle = self.stage / 'data' / (RUN + '-lifecycle.jsonl')
        self.lifecycle.write_text(encoded(life('process_enter', BASE + 40)) + encoded({'event': 'ready', 'private': 'never-disclose-this'})
                                  + encoded(life('state_prepared', BASE + 60)))

    def run_filter(self):
        route = '/mnt/us/' + RUN
        cmd = COLLECT.capture_command(RUN, route, BOOT).replace(route, str(self.stage)).replace('/proc/sys/kernel/random/boot_id', str(self.boot))
        return subprocess.run(['sh', '-ec', cmd], capture_output=True, timeout=10)

    def test_actual_posix_filter_retains_only_allowlisted_precise_numbers(self):
        result = self.run_filter()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout, capture_lines())
        projection = COLLECT.parse_capture(result.stdout, RUN, BOOT, APP)
        self.assertEqual(projection['rust_main_entry'], BASE + 40)
        self.assertEqual(projection['state_produced'], BASE + 60)
        self.assertNotIn('state_consumed', projection)

    def test_unknown_private_field_or_duplicate_candidate_never_transmitted(self):
        for bad in (encoded({**life('process_enter', BASE), 'private-info': 'never-disclose-this'}),
                    encoded(life('process_enter', BASE)) * 2,
                    encoded(life('process_enter', BASE)).replace('"monotonic_ns":', '"monotonic_ns":17,"monotonic_ns":')):
            with self.subTest(case=bad[:12]):
                self.lifecycle.write_text(bad)
                result = self.run_filter()
                self.assertNotEqual(result.returncode, 0)
                self.assertNotIn(b'never-disclose-this', result.stdout + result.stderr)

    def test_links_boot_change_and_wrong_marker_run_are_refused(self):
        self.boot.write_text('wrong\n')
        self.assertNotEqual(self.run_filter().returncode, 0)
        self.boot.write_text(BOOT + '\n')
        save(self.preinvoke, mark('bili-shelf-' + 'f' * 32, 'app_preinvoke', APP, BASE))
        self.assertNotEqual(self.run_filter().returncode, 0)
        save(self.preinvoke, mark(RUN, 'app_preinvoke', APP, BASE + 30))
        self.entry.unlink()
        self.entry.symlink_to(self.preinvoke)
        self.assertNotEqual(self.run_filter().returncode, 0)
        self.entry.unlink()
        os.link(self.preinvoke, self.entry)
        self.assertNotEqual(self.run_filter().returncode, 0)

    def test_missing_state_is_optional_but_process_entry_is_required(self):
        save(self.lifecycle, life('process_enter', BASE + 40))
        result = self.run_filter()
        self.assertEqual(result.returncode, 0)
        self.assertIsNone(COLLECT.parse_capture(result.stdout, RUN, BOOT, APP)['state_produced'])
        save(self.lifecycle, life('state_prepared', BASE + 60))
        self.assertNotEqual(self.run_filter().returncode, 0)

    def test_frozen_serde_lifecycle_bytes_match_the_busybox_filter(self):
        self.lifecycle.write_bytes(ARCHIVED_LIFECYCLE)
        result = self.run_filter()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue(result.stdout.endswith(b'process_enter 28377 148907796047412\nstate_prepared 28377 148908638899720\n'))

    def test_reject_wrong_bound_pid_in_each_clock_and_malformed_receipts(self):
        original = capture_lines()
        for raw in (original.replace(b'picked 1245', b'picked 1246'), original.replace(b'app_preinvoke 1345', b'app_preinvoke 1400'),
                    original.replace(b'process_enter 1345', b'process_enter 1400'), original.replace(b'state_prepared 1345', b'state_prepared 1400'),
                    original + b'process_enter 1345 2\n', original.replace(b'app_preinvoke', b'unknown-stage'), original[:-1], b'x' * 513):
            with self.assertRaises(ValueError):
                COLLECT.parse_capture(raw, RUN, BOOT, APP)


class ClosedTests(unittest.TestCase):
    def setUp(self):
        owner = mock.patch.dict(os.environ, {'BILICOMICS_STUDY_OWNER': TEST_OWNER})
        owner.start()
        self.addCleanup(owner.stop)
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.host = Path(temporary.name)
        patched = mock.patch.object(COLLECT.CONTROL, 'HOST', self.host)
        patched.start()
        self.addCleanup(patched.stop)
        self.cases = [private_case(self.host, index, index < 4) for index in range(10)]
        self.first = Path(self.cases[0]['batch'])

    def test_offline_replay_requires_independent_original_owner_without_a_live_thread(self):
        with mock.patch.dict(os.environ, {'BILICOMICS_STUDY_OWNER': TEST_OWNER}, clear=True):
            self.assertEqual(COLLECT.validate_closed(self.first), BOOT)
        for env in ({}, {'CODEX_THREAD_ID': TEST_OWNER}, {'BILICOMICS_STUDY_OWNER': BOOT}):
            with self.subTest(env=env), mock.patch.dict(os.environ, env, clear=True), self.assertRaises(ValueError):
                COLLECT.validate_closed(self.first)

    def test_ten_planned_four_closed_replay_through_statistical_contract(self):
        public = COLLECT.series_projection(self.cases)
        case = public['cases'][0]
        self.assertEqual(case['clock_offset_ns']['release_before'], 0)
        self.assertEqual(case['clock_offset_ns']['state_produced'], 60)
        self.assertIsNone(case['clock_offset_ns']['state_consumed'])
        self.assertTrue(case['evidence']['same_launcher_and_elf'])
        result = ANALYSIS.analyze(public)
        self.assertEqual((result['warmups_excluded'], result['complete_measured_pairs']), (2, 1))
        self.assertEqual(result['groups']['all']['state_producer_to_ui_observation']['pairs'], 0)
        printable = encoded(public)
        for private in (BOOT, self.cases[0]['run'], str(self.host), str(BASE), 'shim_pid', 'process_id', 'shim_start_ticks'):
            self.assertNotIn(private, printable)

    def test_open_lease_mismatched_boot_or_changed_retirement_cannot_normalize(self):
        token = self.first / 'private-token'
        token.touch()
        with self.assertRaises(ValueError):
            COLLECT.validate_closed(self.first)
        token.unlink()
        save(self.first / 'origin.json', {'boot': 'f' * 36})
        with self.assertRaises(ValueError):
            COLLECT.validate_closed(self.first)
        save(self.first / 'origin.json', {'boot': BOOT})
        save(self.first / 'retired.json', {'natural': False, 'app_pid': APP})
        with self.assertRaises(ValueError):
            COLLECT.validate_closed(self.first)

    def test_collection_requires_retirement_and_a_fresh_projected_shelf(self):
        destination = self.first / COLLECT.SIDECAR
        destination.unlink()
        run = self.cases[0]['run']
        raw = mock.Mock(return_value=capture_lines())
        visit = SimpleNamespace(run=run, route='/mnt/us/' + run, path=self.first, boot=BOOT, raw=raw)
        with self.assertRaises(ValueError):
            COLLECT.collect(visit)
        raw.assert_not_called()
        (self.first / 'released.json').unlink()
        COLLECT.collect(visit)
        raw.assert_called_once()
        self.assertEqual(json.loads(destination.read_text())['app_pid'], APP)
        with self.assertRaises(ValueError):
            COLLECT.collect(visit)
        self.assertEqual(raw.call_count, 1)

    def test_ungated_software_or_different_slot_is_refused(self):
        proof = self.first / 'clock-inputs-verified.json'
        altered = json.loads(proof.read_text())
        altered['comparison'] = 'same-size'
        save(proof, altered)
        with self.assertRaises(ValueError):
            COLLECT.series_projection(self.cases)

    def test_old_elf_needs_a_qualified_renderer_but_not_the_later_order_field(self):
        trace = self.first / (self.cases[0]['run'] + '.jsonl')
        rows = [json.loads(line) for line in trace.read_bytes().splitlines()]
        del rows[1]['data']['render_order']
        trace.write_text(''.join(map(encoded, rows)))
        qualified = COLLECT.normalize_closed(self.first)
        self.assertTrue(qualified['evidence']['qualified_shelf'])
        self.assertNotIn('render_order', encoded(qualified))
        for row in rows[1:-1]:
            row['data']['shelf_state'] = 'signed_out'
        trace.write_text(''.join(map(encoded, rows)))
        with self.assertRaisesRegex(ValueError, 'qualified seeded Shelf'):
            COLLECT.validate_closed(self.first)
        trace.write_text(''.join(map(encoded, [rows[0], rows[-1]])))
        with self.assertRaisesRegex(ValueError, 'qualified seeded Shelf'):
            COLLECT.validate_closed(self.first)
        for row in rows[1:-1]:
            row['data']['shelf_state'] = 'ready'
        rows[1]['data']['render_order'] = 'initial_fill'
        trace.write_text(''.join(map(encoded, rows)))
        with self.assertRaisesRegex(ValueError, 'qualified seeded Shelf'):
            COLLECT.validate_closed(self.first)

    def test_chronology_and_missing_closure_cannot_create_false_prefix(self):
        (Path(self.cases[4]['batch']) / 'attempted').touch()
        with self.assertRaisesRegex(ValueError, 'no exact release'):
            COLLECT.series_projection(self.cases)
        (Path(self.cases[4]['batch']) / 'attempted').unlink()
        receipt = self.first / COLLECT.SIDECAR
        record = json.loads(receipt.read_text())
        record['rust_main_entry'] = BASE + 49_000_000
        save(receipt, record)
        with self.assertRaisesRegex(ValueError, 'ordering failed'):
            COLLECT.series_projection(self.cases)

    def test_closed_terminal_missing_capture_retains_prefix_but_never_skips_past_it(self):
        last = Path(self.cases[3]['batch'])
        (last / COLLECT.SIDECAR).unlink()
        prefix = COLLECT.series_projection(self.cases)
        self.assertEqual(len(prefix['cases']), 3)
        self.assertEqual(ANALYSIS.analyze(prefix)['unpaired_measured_visits'], 1)
        following = Path(self.cases[4]['batch'])
        (following / 'attempted').touch()
        with self.assertRaisesRegex(ValueError, 'no exact release'):
            COLLECT.series_projection(self.cases)
        (following / 'attempted').unlink()
        (Path(self.cases[2]['batch']) / COLLECT.SIDECAR).unlink()
        with self.assertRaisesRegex(ValueError, 'closed visit after'):
            COLLECT.series_projection(self.cases)

    def test_terminal_does_not_relax_release_or_app_retirement(self):
        last = Path(self.cases[3]['batch'])
        (last / COLLECT.SIDECAR).unlink()
        release = json.loads((last / 'released.json').read_text())
        save(last / 'released.json', {**release, 'owner_and_token_checked': False})
        with self.assertRaises(ValueError):
            COLLECT.series_projection(self.cases)
        save(last / 'released.json', release)
        (last / 'stop-intended').touch()
        with self.assertRaisesRegex(ValueError, 'retirement'):
            COLLECT.series_projection(self.cases)
        save(last / 'retired.json', {'natural': False, 'app_pid': None})
        self.assertEqual(len(COLLECT.series_projection(self.cases)['cases']), 3)

    def test_failure_before_any_contact_requires_real_release_not_busy_inference(self):
        last = Path(self.cases[4]['batch'])
        for leaf in ('route.tar.gz', 'probe.tar.gz', 'askpass'):
            (last / leaf).unlink()
        (last / 'attempted').touch()
        with self.assertRaisesRegex(ValueError, 'no exact release'):
            COLLECT.series_projection(self.cases)
        save(last / 'released.json', {'state': 'released', 'device_disposition': 'no-device-change', 'owner_and_token_checked': True,
                                     'owner_thread': TEST_OWNER, 'native_restored_and_connections_closed': True,
                                     'run_id': self.cases[4]['run']})
        save(last / 'failure.json', {'phase': 'native-preflight', 'reason': 'operation-failed'})
        self.assertEqual(len(COLLECT.series_projection(self.cases)['cases']), 4)


if __name__ == '__main__':
    unittest.main()
