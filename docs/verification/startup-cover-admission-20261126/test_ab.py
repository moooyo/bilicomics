"""Exercise the comparison controller off-device, with no Kindle transport or lease."""

import argparse
import importlib.util
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile
import unittest
from unittest import mock


HERE = Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location('ab', HERE / 'ab.py')
ab = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(ab)
ROOT = HERE.parents[2]
OWNER = '4c7af31d-dd42-4fb9-8e2d-07b1692ffdcf'
HOST_BOOT = '1c6a9c92-a27e-4e96-966b-05c7bad70cc2'
DEVICE_BOOT = '60f6aa5e-a91a-407b-9ebe-24820f61eab3'
RUN = 'bili-shelf-' + '7' * 32
SEED = [ab.NATIVE.SESSION, *[f'covers/{i:064x}.png' for i in range(20)], ab.NATIVE.DB]
RUNTIME = ['accounts', 'accounts/' + 'a' * 64, 'accounts/' + 'a' * 64 + '/pages']
MS = 1_000_000


def route(run, arm):
    shipping = (ROOT / 'packaging/kindle/launch.sh').read_bytes()
    return ab.route_files(shipping, shipping, run, SEED, RUNTIME, arm=arm)


def projected(case, *, advance_ms=125, source=None):
    pair, arm = case['pair'], case['arm']
    # Deliberately make the B warmup much slower; it may not affect the gate.
    a = 6000 * MS
    b = (6000 - (advance_ms if pair else -2000)) * MS
    base = a if arm == 'A' else b
    source = source or ('ui' if arm == 'A' else 'send')
    return {'visit': case['visit'], 'pair': pair, 'arm': arm, 'cover_gate_source': source,
            'intervals_ns': {ab.DECODE_OVERLAP: 0 if source == 'ui' else 150 * MS},
            'display_witness': {'first_loading_completion_matches_send': True},
            **{name: [base + index * 30 * MS, base + (index * 30 + 2) * MS] for index, name in enumerate(ab.ENDPOINTS)}}


class PlanFixture(unittest.TestCase):
    def setUp(self):
        work = tempfile.TemporaryDirectory(prefix='bili-cover-ab-fixture-')
        self.addCleanup(work.cleanup)
        self.root = Path(work.name).resolve()
        self.enterContext(mock.patch.object(ab.CORE, 'identity', return_value=(OWNER, HOST_BOOT)))
        self.enterContext(mock.patch.object(ab.CORE, 'configured_owner', return_value=OWNER))
        self.enterContext(mock.patch.object(ab.CLOCK.BASE, 'batch_path', side_effect=lambda run: self.root / run))
        self.inputs = argparse.Namespace(**{name: self.root / name for name in ab.NATIVE.INPUTS})
        self.plan = ab.make_plan(self.inputs, OWNER, HOST_BOOT)
        self.cases = ab.plan_cases(self.plan)

    def bind(self, case):
        path = Path(case['batch'])
        path.mkdir(mode=0o700)
        ab.CORE.save(path / 'case.json', {'case': case, 'seed_leaves': SEED})
        ab.CORE.save(path / 'admission.json', {'run': case['run'], 'owner': OWNER, 'host_boot': HOST_BOOT,
                    'rollout_root': self.plan['inputs']['rollout_root']})
        return path


class RouteTests(unittest.TestCase):
    def test_only_the_treatment_argument_and_run_paths_change(self):
        a, b = route(RUN, 'A'), route(RUN, 'B')
        self.assertEqual(a.keys(), b.keys())
        for name in a:
            with self.subTest(leaf=name):
                self.assertEqual(b[name].replace(ab.EARLY, b''), a[name])
                self.assertEqual(b[name].count(ab.EARLY), int(name in ('launch.sh', 'switch.sh')))
        self.assertIn(b['launch.sh'], b['switch.sh'])
        self.assertIn(a['launch.sh'], a['switch.sh'])
        other_run = RUN.replace('7', '8')
        self.assertEqual({name: blob.replace(other_run.encode(), RUN.encode()) for name, blob in route(other_run, 'B').items()}, b)
        for arm, files in (('A', a), ('B', b)):
            launcher = files['launch.sh']
            self.assertIn(b' --offline --shelf-render-trace', launcher)
            self.assertIn(b' --exit-after-ms 2500 --startup-phases', launcher)
            for option in (b'--demo', b'--screen shelf', b'--lifecycle-trace', b'--serial-shelf-render'):
                self.assertNotIn(option, launcher)
            for source in (ab.guard(arm), files['launch.sh'], files['switch.sh'], files['entry.sh'], files['entry-ready.sh']):
                subprocess.run(['sh', '-n'], input=source, check=True, timeout=5)
        self.assertEqual(ab.guard('B').replace(ab.EARLY, b''), ab.guard('A'))
        self.assertEqual(ab.guard('A'), ab.NATIVE.guard())
        self.assertRaises(ValueError, ab.guard, 'other')

    def test_shell_guard_matches_the_packed_nul_arg_vector_not_the_other_arm(self):
        with tempfile.TemporaryDirectory(prefix='bili-cover-argv-') as tmp:
            fake_process = Path(tmp) / '1'
            fake_process.mkdir()
            cmdline = fake_process / 'cmdline'
            tree = '/mnt/us/' + RUN
            vectors = {}
            checks = {}
            for arm in ('A', 'B'):
                launcher = route(RUN, arm)['launch.sh']
                launch_line, = [line for line in launcher.splitlines() if b'--startup-phases ' in line]
                argv = launch_line.split(b' -- ', 1)[1].split(b' >&6', 1)[0]
                env = {'APP_DIR': tree + '/app', 'PROFILE': 'scribe', 'DATA_DIR': tree + '/data', 'PATH': os.defpath}
                vectors[arm] = subprocess.check_output(['sh', '-c', b"printf '%s\\000' " + argv], env=env, timeout=5)
                guard_lines = ab.guard(arm).splitlines(keepends=True)
                index, = [i for i, line in enumerate(guard_lines) if b' "$stage/app/bilicomics" --kindle' in line]
                checks[arm] = b''.join(guard_lines[index:index + 2]).split(b' || return 1', 1)[0]
                self.assertIn(b'\x00--startup-phases\x00' + f'{tree}/data/{RUN}.startup-phases.json'.encode() + b'\x00', vectors[arm])
            env = {'stage': tree, 'run': RUN, 'proc': tmp, 'app': '1', 'PATH': os.defpath}
            for arm, other in (('A', 'B'), ('B', 'A')):
                vector = vectors[arm]
                samples = ((vector, True), (vectors[other], False), (vector + b'--demo\x00', False),
                           (vector.replace(RUN.encode(), b'other-run'), False),
                           (vector.replace(b'--offline\x00', b'--offline\x00--screen\x00shelf\x00'), False))
                for argv, result in samples:
                    with self.subTest(guard=arm, accepted=result):
                        cmdline.write_bytes(argv)
                        check = subprocess.run(['sh', '-c', checks[arm]], env=env, timeout=5)
                        self.assertEqual(check.returncode == 0, result)

    def test_loader_restores_import_path_even_if_an_import_fails(self):
        saved = sys.path[:]

        def load(fail):
            sys.path.insert(0, '/fixture/import/path')
            if fail:
                raise RuntimeError('fixture failure')
            return 7

        self.assertEqual(ab.scoped_load(load, False), 7)
        with self.assertRaises(RuntimeError):
            ab.scoped_load(load, True)
        self.assertEqual(sys.path, saved)


class IdleAdmissionTests(PlanFixture):
    def visit(self):
        visit = object.__new__(ab.Visit)
        visit.path = self.root
        visit.boot = DEVICE_BOOT
        visit.detach = mock.Mock()
        visit.guard = mock.Mock(return_value=DEVICE_BOOT)
        visit.raw = mock.Mock()
        self.enterContext(mock.patch.object(ab.CORE, 'announce'))
        return visit

    def test_idle_reset_is_synchronous_and_only_before_the_timed_click(self):
        visit = self.visit()
        for phase in ('native-preflight', 'media-capacity', 'stage-detector', 'stage-route', 'stock-inspect',
                      'native-navigation', 'identify-card', 'route-activate', 'card-release'):
            visit.step(phase)
        self.assertEqual(visit.guard.call_args_list, [mock.call('native')] * 3)
        self.assertEqual(visit.raw.call_args_list, [mock.call(ab.RESET_NATIVE_IDLE, deadline=12)] * 3)
        ab.CORE.flag(self.root / 'click-intended')
        for phase in ('app-retirement', 'stock-restore', 'restore-navigation', 'capture-anonymous-records',
                      'remove-route-stage', 'remove-detector-stage', 'verify-native-return', 'exact-release'):
            visit.step(phase)
        self.assertEqual(visit.raw.call_count, 3)
        visit.detach.assert_called_once_with()

    def test_idle_reset_refuses_a_lost_device_or_a_dispatched_click(self):
        visit = self.visit()
        visit.guard.return_value = 'wrong-boot'
        with self.assertRaisesRegex(ab.CORE.OperatorError, 'ownership changed'):
            visit.step('route-activate')
        visit.raw.assert_not_called()
        ab.CORE.flag(self.root / 'click-intended')
        with self.assertRaisesRegex(ab.CORE.OperatorError, 'before input'):
            visit.step('route-activate')
        visit.raw.assert_not_called()

    def test_only_the_transient_native_property_is_written(self):
        root = str(self.root)
        get = self.root / 'lipc-get-prop'
        get.write_text('#!/bin/sh\ncase "$*" in\n'
                       '"com.lab126.appmgrd activeApp") printf "%s\\n" "$FRONT";;\n'
                       '"-s com.lab126.powerd state") printf "%s\\n" "$POWER";;\n'
                       '*) exit 7;; esac\n')
        put = self.root / 'lipc-set-prop'
        put.write_text('#!/bin/sh\nprintf "%s\\n" "$*" >>"$CALLS"\n')
        get.chmod(0o700)
        put.chmod(0o700)
        calls = self.root / 'calls'
        command = ab.RESET_NATIVE_IDLE.replace('/tmp/', root + '/')
        env = {**os.environ, 'PATH': root + ':' + os.defpath, 'CALLS': str(calls),
               'FRONT': 'com.lab126.KPPMainApp', 'POWER': 'active'}
        subprocess.run(['sh', '-ec', command], env=env, check=True, timeout=5)
        for unsafe in ({'POWER': 'screenSaver'}, {'FRONT': 'another-app'}):
            check = subprocess.run(['sh', '-ec', command], env={**env, **unsafe}, timeout=5)
            self.assertNotEqual(check.returncode, 0)
        (self.root / 'weread-launcher.lock').mkdir()
        self.assertNotEqual(subprocess.run(['sh', '-ec', command], env=env, timeout=5).returncode, 0)
        self.assertEqual(calls.read_text(), '-i com.lab126.powerd touchScreenSaverTimeout 1\n')


class ArchiveTests(PlanFixture):
    def setUp(self):
        super().setUp()
        entries = sorted(ab.CORE.REQUIRED)
        with tarfile.open(self.inputs.detector, 'x:gz', format=tarfile.USTAR_FORMAT) as archive:
            for name in entries:
                member = tarfile.TarInfo(name)
                member.mode = 0o600 if name.startswith('models/') else 0o700
                member.size = 1
                archive.addfile(member, io.BytesIO(b'p'))
        self.inputs.detector = self.inputs.detector.rename(self.root / 'detector.tar.gz')
        ab.CORE.save(self.root / 'detector.json', {'members': entries, 'bytes': len(entries)})
        self.inputs.application.write_bytes(b'one exact application ELF fixture')
        self.enterContext(mock.patch.object(ab.NATIVE, 'inputs', side_effect=self.selected))

    def selected(self, args, case, *, route_builder):
        shipping = (ROOT / 'packaging/kindle/launch.sh').read_bytes()
        data = route_builder(shipping, shipping, case['run'], SEED, RUNTIME)
        for name in ab.NATIVE.CONTROL_FILES - data.keys():
            data[name] = self.inputs.application if name == 'app/bilicomics' else name.encode('ascii')
        data.update({'data/' + name: b'private synthetic seed leaf fixture' for name in SEED})
        return data, SEED

    def pack(self, case):
        ab.NATIVE.prepared_batch(self.inputs, case, OWNER, HOST_BOOT, **ab.batch_options(case))
        return Path(case['batch'])

    def test_archives_package_identical_arm_inputs_and_exact_guard(self):
        for case in self.cases[:2]:
            path = self.pack(case)
            ab.check_batch(self.inputs, case)
            other = 'A' if case['arm'] == 'B' else 'B'
            # The wrong guard fails even though both archives parse and both
            # are valid ordinary Shelf routes for their own arms.
            (path / 'native-guard.sh').write_bytes(ab.guard(other))
            with self.assertRaisesRegex(ValueError, 'program or mount'):
                ab.check_batch(self.inputs, case)
            (path / 'native-guard.sh').write_bytes(ab.guard(case['arm']))
        self.inputs.application.write_bytes(b'two exact application ELF fixture')
        for case in self.cases[:2]:
            with self.subTest(arm=case['arm']), self.assertRaisesRegex(ValueError, 'archive contents'):
                ab.check_batch(self.inputs, case)

    def test_wrong_argv_member_is_rejected_even_with_matching_archive_size_receipt(self):
        _, case = self.cases[:2]
        path = self.pack(case)
        archive_path = path / 'route.tar.gz'
        changed = path / 'injected.tar.gz'
        with tarfile.open(archive_path, 'r:gz') as source, tarfile.open(changed, 'x:gz', format=tarfile.USTAR_FORMAT) as output:
            for member in source:
                data = source.extractfile(member).read()
                if member.name == 'launch.sh':
                    data = data.replace(ab.EARLY, b' ' * len(ab.EARLY))
                output.addfile(member, io.BytesIO(data))
        changed.replace(archive_path)
        admission = path / 'admission.json'
        receipt = json.loads(admission.read_bytes())
        receipt['route_bytes'] = archive_path.stat().st_size
        admission.write_text(json.dumps(receipt))
        with self.assertRaisesRegex(ValueError, 'archive contents'):
            ab.check_batch(self.inputs, case)


class EvidenceTests(PlanFixture):
    def test_schedule_is_fixed_and_populated_and_arms_cannot_be_swapped(self):
        self.assertEqual(''.join(case['arm'] for case in self.cases), 'ABABBAABBAABBA')
        self.assertEqual({case['state'] for case in self.cases}, {'ready'})
        self.assertEqual(len({case['run'] for case in self.cases}), 14)
        for change in ({'arm': 'B'}, {'state': 'empty'}, {'batch': '/other'}, {'run': self.cases[1]['run']}):
            altered = [{**self.cases[0], **change}, *self.cases[1:]]
            with self.subTest(change=change), self.assertRaises(ValueError):
                ab.plan_cases({**self.plan, 'cases': altered})

    def test_binding_and_loading_witness_are_required_and_fallback_stays_b(self):
        a, b = self.cases[:2]
        for case in (a, b):
            path = self.bind(case)
            ab.CORE.save(path / 'card.json', {'click': {'down_before_ns': 200}})
            (path / (case['run'] + '.jsonl')).write_bytes(b'{"monotonic_ns":250}\n')
        self.enterContext(mock.patch.object(ab.NATIVE, 'observation'))
        ab.NATIVE.observation.return_value = (DEVICE_BOOT, projected(b, source='ui'))
        previous, row = ab.observation(self.plan, b, (DEVICE_BOOT, 175))
        self.assertEqual(previous, (DEVICE_BOOT, 250))
        self.assertEqual(row['arm'], 'B')
        self.assertEqual(row['cover_gate_source'], 'ui')
        self.assertTrue(row['candidate_used_ui_fallback'])
        for previous in ((DEVICE_BOOT, 200), (HOST_BOOT, 175)):
            with self.subTest(previous=previous), self.assertRaisesRegex(ValueError, 'declared order'):
                ab.observation(self.plan, b, previous)
        for changed in ({'cover_gate_source': None}, {'intervals_ns': {ab.DECODE_OVERLAP: None}},
                        {'display_witness': {'first_loading_completion_matches_send': False}}):
            ab.NATIVE.observation.return_value = (DEVICE_BOOT, {**projected(b), **changed})
            with self.subTest(changed=changed), self.assertRaisesRegex(ValueError, 'initial loading SEND'):
                ab.observation(self.plan, b, None)
        ab.NATIVE.observation.return_value = (DEVICE_BOOT, projected(a, source='send'))
        with self.assertRaisesRegex(ValueError, 'initial loading SEND'):
            ab.observation(self.plan, a, None)
        with self.assertRaisesRegex(ValueError, 'Different slot'):
            ab.observation(self.plan, {**b, 'arm': 'A'}, None)

    def test_gate_requires_six_pairs_five_wins_and_outward_upper_bound(self):
        visits = [projected(case, advance_ms=-4 if case['pair'] == 6 else 102) for case in self.cases]
        key = ab.ENDPOINTS[0]
        report = ab.comparison(visits)
        self.assertEqual([pair['order'] for pair in report['pairs']], ['AB', 'BA', 'AB', 'BA', 'AB', 'BA'])
        self.assertEqual(report['summary'][key]['paired_median_b_minus_a_ns'], [-104 * MS, -100 * MS])
        self.assertEqual(report['summary'][key]['b_earlier'], 5)
        self.assertTrue(report['comparison_gate_passed'])
        self.assertFalse(ab.comparison(visits[:-1])['first_shelf_gate_passed'])
        for visit in visits:
            if visit['arm'] == 'B' and visit['pair'] in (3, 5, 6):
                visit[key] = [value + 106 * MS for value in visit[key]]
        self.assertFalse(ab.comparison(visits)['first_shelf_gate_passed'])
        self.assertEqual(ab.aggregate_interval([[-11, -9], [-8, -8]]), [-10, -8])
        barely_lost = [projected(case, advance_ms=102) for case in self.cases]
        for visit in barely_lost:
            if visit['arm'] == 'B':
                visit[key][1] += 1
        self.assertFalse(ab.comparison(barely_lost)['first_shelf_gate_passed'])

    def test_secondary_endpoints_and_gate_mechanism_stay_independent(self):
        visits = [projected(case) for case in self.cases]
        render, send, driver = ab.ENDPOINTS
        control_b = next(row for row in visits if row['pair'] == 2 and row['arm'] == 'B')
        control_b['cover_gate_source'] = 'ui'
        control_b[driver] = None
        late_b = next(row for row in visits if row['pair'] == 4 and row['arm'] == 'B')
        late_b[send] = [value + 160 * MS for value in late_b[send]]
        report = ab.comparison(visits)
        self.assertTrue(report['first_shelf_gate_passed'])
        self.assertFalse(report['comparison_gate_passed'])
        self.assertEqual(report['candidate_ui_fallback_visits'], [control_b['visit']])
        self.assertEqual(report['summary'][render]['b_earlier'], 6)
        self.assertEqual(report['summary'][send]['b_later'], 1)
        self.assertEqual(report['summary'][driver]['pairs'], 5)
        self.assertIsNone(report['pairs'][1]['b_minus_a_ns'][driver])
        self.assertIsNotNone(report['pairs'][1]['b_minus_a_ns'][send])


class SequenceTests(PlanFixture):
    def setUp(self):
        super().setUp()
        self.recorded = []
        self.closures = {}
        self.check = self.enterContext(mock.patch.object(ab, 'check_batch'))
        self.password = self.enterContext(mock.patch.object(ab.getpass, 'getpass', return_value='fixture-only'))
        self.enterContext(mock.patch.object(ab.sys.stdin, 'isatty', return_value=True))
        self.enterContext(mock.patch.object(ab.CORE, 'announce'))
        self.runner = self.enterContext(mock.patch.object(ab, 'Visit', side_effect=self.visit))
        self.evidence = self.enterContext(mock.patch.object(ab, 'observation', side_effect=self.observation))
        self.failure = None

    def visit(self, path):
        case = next(case for case in self.cases if Path(case['batch']) == path)
        visit = mock.Mock(boot=None)

        def execute(recover):
            self.assertFalse(recover)
            self.assertEqual(visit.read_password(), 'fixture-only')
            self.recorded.append((case['visit'], visit.boot))
            path.mkdir(mode=0o700)
            ab.CORE.flag(path / 'attempted')
            if self.failure is not None and case['visit'] == self.failure[0]:
                raise self.failure[1]
            self.closures[case['visit']] = projected(case)

        visit.execute.side_effect = execute
        return visit

    def observation(self, plan, case, previous):
        selected = self.closures[case['visit']]
        latest = 100 * case['visit']
        if previous is not None:
            self.assertEqual(previous, (DEVICE_BOOT, latest - 100))
        return (DEVICE_BOOT, latest), selected

    def start(self, through):
        ab.start(self.root, self.plan, self.cases, self.inputs, through)

    def test_resume_only_after_an_intentional_complete_pair(self):
        self.start(1)
        self.assertEqual(self.recorded, [(1, None), (2, DEVICE_BOOT), (3, DEVICE_BOOT), (4, DEVICE_BOOT)])
        pause = self.root / 'paused-after-1.json'
        self.assertTrue(pause.is_file())
        with self.assertRaisesRegex(ValueError, 'No fresh pairs'):
            self.start(1)
        self.start(6)
        self.assertEqual([index for index, _ in self.recorded], list(range(1, 15)))
        self.assertEqual(self.recorded[4][1], DEVICE_BOOT)
        self.assertEqual(self.password.call_count, 2)
        self.assertNotIn(b'fixture-only', (self.root / 'started-after-1.json').read_bytes())
        with self.assertRaisesRegex(ValueError, 'complete pair'):
            self.start(6)

    def test_missing_pause_does_not_authorize_continuation_even_after_a_full_pair(self):
        self.start(1)
        (self.root / 'paused-after-1.json').unlink()
        with self.assertRaises(OSError):
            self.start(6)
        self.assertEqual(len(self.recorded), 4)

    def test_busy_or_timeout_in_midpair_attempt_stops_and_never_retries(self):
        for error in (ab.CORE.OperatorError('Shared lease busy'), subprocess.TimeoutExpired('fixture', 5)):
            with self.subTest(error=type(error).__name__):
                self.failure = (4, error)
                with self.assertRaises(type(error)):
                    self.start(1)
                with self.assertRaisesRegex(ValueError, 'unqualified visit'):
                    self.start(6)
                self.assertEqual(len(self.recorded), 4)
                self.assertFalse((self.root / 'paused-after-1.json').exists())
                result = ab.report(self.plan, self.cases)
                self.assertEqual(result['stopped']['visit'], 4)
                self.assertEqual(result['pairs'], [])
                self.assertEqual(len(result['visits']), 3)
                for case in self.cases[:4]:
                    path = Path(case['batch'])
                    (path / 'attempted').unlink()
                    path.rmdir()
                (self.root / 'started-after-0.json').unlink()
                self.recorded.clear()
                self.closures.clear()

    def test_after_first_release_invalid_evidence_stops_the_series(self):
        self.evidence.side_effect = ValueError('Unqualified fixture gate evidence')
        with self.assertRaisesRegex(ValueError, 'Unqualified fixture'):
            self.start(6)
        self.assertEqual(self.recorded, [(1, None)])
        with self.assertRaisesRegex(ValueError, 'unqualified visit'):
            self.start(6)

    def test_recover_requires_the_original_active_attempt_and_arm_guard(self):
        case = self.cases[1]
        path = self.bind(case)
        with self.assertRaises(ValueError):
            ab.recover(self.plan, self.cases, 2)
        self.runner.assert_not_called()
        ab.CORE.flag(path / 'attempted')
        ab.CORE.flag(path / 'private-token')
        (path / 'native-guard.sh').write_bytes(ab.guard('A'))
        with self.assertRaises(ValueError):
            ab.recover(self.plan, self.cases, 2)
        self.runner.assert_not_called()
        (path / 'native-guard.sh').write_bytes(ab.guard('B'))
        visit = mock.Mock()
        self.runner.side_effect = None
        self.runner.return_value = visit
        ab.recover(self.plan, self.cases, 2)
        self.runner.assert_called_once_with(path)
        visit.execute.assert_called_once_with(True)
        ab.CORE.flag(path / 'released.json')
        with self.assertRaises(ValueError):
            ab.recover(self.plan, self.cases, 2)


if __name__ == '__main__':
    unittest.main(verbosity=2)
