"""Rehearse both complete launcher-byte verifiers, storage and closure without a Kindle or lease."""

import argparse
import contextlib
import io
import json
import os
from pathlib import Path
import struct
import subprocess
import sys
import tarfile
import tempfile
import unittest
from unittest import mock
import zipfile


HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import native_series as series
sys.path.insert(0, str(series.BASE.STUDY))
import test_log_bounds as timing


CONTROL = series.CONTROL
STOCK = Path(os.environ.get('BILI_STUDY_STOCK_LAUNCHER', HERE.parents[2] / 'packaging/kindle/launch.sh'))
FIX = CONTROL.module(series.ROUTE.SOURCE / 'test_route.py')
OWNER = 'test-owner'
BOOT = 'ca593c55-db03-41d8-9230-c47cdc6b3b1a'


class NativeSeriesTests(unittest.TestCase):
    def setUp(self):
        cleanup = contextlib.ExitStack()
        self.addCleanup(cleanup.close)
        old_mask = os.umask(0o077)
        cleanup.callback(os.umask, old_mask)
        self.root = Path(cleanup.enter_context(tempfile.TemporaryDirectory())).resolve()
        cleanup.enter_context(mock.patch.object(CONTROL, 'HOST', self.root))
        cleanup.enter_context(mock.patch.object(CONTROL, 'identity', return_value=(OWNER, BOOT)))
        self.plan = {**series.BASE.make_plan(OWNER, BOOT), 'schema': series.SCHEMA}

    def test_schedule_and_experiment_are_immutable(self):
        self.assertEqual([row['arm'] for row in series.checked_plan(self.plan, OWNER, BOOT)], list('ABABBAABBAABBA'))
        for altered in ({**self.plan, 'schema': 'bili-shelf-ab-v1'}, {**self.plan, 'cases': self.plan['cases'][2:]},
                        {**self.plan, 'host_boot': 'another-boot'}):
            with self.assertRaises(CONTROL.OperatorError):
                series.checked_plan(altered, OWNER, BOOT)

    def test_candidate_must_match_the_independent_frozen_diff(self):
        frozen = (HERE / 'worker-only.patch').read_bytes()
        self.assertIn(b'\n+    exec 8>&-\n', frozen)
        (self.root / 'worker-only.patch').write_bytes(frozen.replace(b'\n+    exec 8>&-\n', b'\n+    :\n'))
        with mock.patch.object(series, 'HERE', self.root), self.assertRaisesRegex(CONTROL.OperatorError, 'independently frozen'):
            series.edited_stock(STOCK.read_bytes())

    def test_reviewed_transformation_and_device_verifier_compare_every_byte(self):
        stock = STOCK.read_bytes()
        candidate = series.edited_stock(stock)
        self.assertNotEqual(stock, candidate)
        for case in self.plan['cases'][:2]:
            arm = case['arm']
            files = series.case_files(stock, candidate, case)
            self.assertNotIn(b'--serial-shelf-render', files['launch.sh'])
            checker = files['switch.sh'].split(b'\nlauncher_verified() {\n', 1)[1].split(b'\nclean_stage() {\n', 1)[0]
            for label, staged, accepted in (('exact', files['launch.sh'], True), ('extra', files['launch.sh'] + b'\n', False),
                    ('unrelated', files['launch.sh'].replace(b'/dev/fb0', b'/dev/fb1'), False),
                    ('wrong-arm', series.case_files(stock, candidate, {**case, 'arm': 'B' if arm == 'A' else 'A'})['launch.sh'], False)):
                with self.subTest(arm=arm, label=label):
                    (self.root / 'launch.sh').write_bytes(staged)
                    checked = subprocess.run(['sh', '-s'], env={**os.environ, 'stage': str(self.root)},
                        input=b'launcher_verified() {\n' + checker + b'\nlauncher_verified\n', capture_output=True, timeout=3)
                    self.assertEqual(checked.returncode == 0, accepted, checked.stderr)

    def sealed_inputs(self):
        header = bytearray(84)
        header[:7] = b'\x7fELF\x01\x01\x01'
        struct.pack_into('<HH', header, 16, 2, 40)
        struct.pack_into('<I', header, 28, 52)
        struct.pack_into('<I', header, 36, 0x400)
        struct.pack_into('<HH', header, 42, 32, 1)
        struct.pack_into('<I', header, 52, 1)
        paths = {name: self.root / name for name in ('application', 'worker', 'route_fs', 'shipping_launcher', 'candidate_launcher', 'document_launcher')}
        for name in ('application', 'worker', 'route_fs'):
            paths[name].write_bytes(header + name.encode())
        paths['shipping_launcher'].write_bytes(STOCK.read_bytes())
        paths['candidate_launcher'].write_bytes(series.edited_stock(STOCK.read_bytes()))
        paths['document_launcher'].write_bytes(series.ROUTE.DOCUMENT)
        package = self.root / 'stock.zip'
        with zipfile.ZipFile(package, 'x') as archive:
            archive.writestr('bilicomics/launch.sh', STOCK.read_bytes())
            archive.writestr('documents/bilicomics.sh', series.ROUTE.DOCUMENT)
        native = self.root / 'paired-kit.tar.gz'
        with tarfile.open(native, 'x:gz') as archive:
            for name in sorted(CONTROL.REQUIRED):
                member = tarfile.TarInfo(name)
                member.mode, member.size = (0o600 if name.startswith('models/') else 0o700), 1
                archive.addfile(member, io.BytesIO(b'x'))
        native.with_suffix('').with_suffix('.json').write_text(json.dumps({'members': sorted(CONTROL.REQUIRED), 'bytes': len(CONTROL.REQUIRED)}))
        self.addCleanup(mock.patch.stopall)
        mock.patch.object(series, 'STOCK_ZIP', package).start()
        mock.patch.object(series, 'ELF_SIZE', paths['application'].stat().st_size).start()
        return argparse.Namespace(**paths, output=self.root / 'study', native_kit=native, rollout_root=self.root)

    def test_rejects_wrong_input_sources_before_creating_a_batch(self):
        args = self.sealed_inputs()
        original = args.candidate_launcher.read_bytes()
        for source in (args.shipping_launcher.read_bytes(), original + b'\n', original.replace(b'check_launch_inputs\n', b'skip_launch_inputs\n')):
            args.candidate_launcher.write_bytes(source)
            with self.assertRaisesRegex(CONTROL.OperatorError, 'reviewed'):
                series.prepare(args)
            self.assertFalse(args.output.exists())
        for path in (args.candidate_launcher, args.shipping_launcher):
            path.write_bytes(original)
        with self.assertRaisesRegex(CONTROL.OperatorError, 'retained installation'):
            series.prepare(args)

    def test_rejects_a_wrong_archive_member_of_the_same_size_and_an_executable_mode_on_shell_sources(self):
        for contents, mode, valid in ((b'ABC', 0o600, True), (b'BAD', 0o600, False), (b'ABC', 0o700, False)):
            route = self.root / 'route.tar.gz'
            with tarfile.open(route, 'w:gz') as archive:
                member = tarfile.TarInfo('launch.sh')
                member.size, member.mode = len(contents), mode
                archive.addfile(member, io.BytesIO(contents))
            with self.subTest(contents=contents, mode=mode):
                if valid:
                    series.verify_archive(route, {'launch.sh': b'ABC'})
                else:
                    with self.assertRaisesRegex(CONTROL.OperatorError, 'archive'):
                        series.verify_archive(route, {'launch.sh': b'ABC'})

    def test_both_real_reversible_routes_reject_the_opposite_arm_then_restore_the_original_object(self):
        old = STOCK.read_bytes()
        candidate = series.edited_stock(old)
        FIX.RouteTest.setUpClass()
        self.addCleanup(FIX.RouteTest.doClassCleanups)
        for case in self.plan['cases'][:2]:
            fixture = FIX.RouteTest()
            fixture.setUp()
            try:
                fixture.shipping = old
                fixture.stock.write_bytes(old)
                (fixture.stage / 'shipping-launcher.sh').write_bytes(old)
                profile = {**case, 'run': FIX.RUN}
                approved = series.case_files(old, candidate, profile)
                switch = fixture.switch.read_bytes()
                original = (series.ROUTE.SOURCE / 'switch.sh').read_bytes()
                def verifier(script):
                    return script.split(b'\nlauncher_verified() {\n', 1)[1].split(b'\n}\n\nclean_stage() {', 1)[0]
                before = verifier(original)
                self.assertEqual(switch.count(before), 1)
                fixture.switch.write_bytes(switch.replace(before, verifier(approved['switch.sh'])))
                other = {**profile, 'arm': 'A' if profile['arm'] == 'B' else 'B'}
                (fixture.stage / 'launch.sh').write_bytes(series.case_files(old, candidate, other)['launch.sh'])
                fixture.invoke('inspect', 'stock')
                fixture.invoke('activate')
                self.assertFalse(fixture.journal.exists())
                fixture.check_stock()
                (fixture.stage / 'launch.sh').write_bytes(approved['launch.sh'])
                fixture.invoke('activate', 'armed')
                self.assertEqual(fixture.retained.read_bytes(), old)
                self.assertEqual(fixture.retained.stat().st_ino, fixture.stock_inode)
                fixture.invoke('restore', 'stock')
                fixture.invoke('cleanup', 'removed')
                fixture.check_stock()
            finally:
                fixture.doCleanups()

    def test_full_archive_check_and_scoped_discard(self):
        args = self.sealed_inputs()
        with mock.patch.object(CONTROL, 'tools'), mock.patch.object(CONTROL, 'Operator') as operator, contextlib.redirect_stdout(io.StringIO()):
            series.prepare(args)
            plan = CONTROL.load(args.output / 'plan.json')
            cases = series.checked_plan(plan, OWNER, BOOT)
            series.ready(args.output, plan, cases)
            operator.assert_not_called()
            second = Path(cases[1]['batch'])
            receipt = second / series.MARK
            valid = receipt.read_bytes()
            receipt.write_text(json.dumps(series.binding({**cases[1], 'arm': 'A'})))
            with self.assertRaisesRegex(CONTROL.OperatorError, 'binding'):
                series.ready(args.output, plan, cases)
            receipt.write_bytes(valid)
            probe = second / 'probe.tar.gz'
            blob = probe.read_bytes()
            probe.write_bytes(bytes([blob[0] ^ 1]) + blob[1:])
            with self.assertRaisesRegex(CONTROL.OperatorError, 'Detector'):
                series.ready(args.output, plan, cases)
            probe.write_bytes(blob)
            first = Path(cases[0]['batch'])
            (first / 'attempted').touch()
            files = set(first.iterdir())
            foreign = second / 'unknown'
            foreign.touch()
            with self.assertRaisesRegex(CONTROL.OperatorError, 'Unsafe'):
                series.discard(plan, cases)
            self.assertTrue(foreign.exists())
            foreign.unlink()
            series.discard(plan, cases)
            self.assertEqual(set(first.iterdir()), files)
            self.assertFalse(any(Path(case['batch']).exists() for case in cases[1:]))

    def test_primary_from_older_elf_requires_no_observed_render_order(self):
        case = {**self.plan['cases'][0], 'run': timing.RUN_ID, 'batch': str(self.root / ('bilicomics-interactive-' + timing.RUN_ID))}
        batch = Path(case['batch'])
        batch.mkdir(mode=0o700)
        card = timing.origin()
        rows = timing.trace(render_order=None)
        records = {'admission.json': {'run': timing.RUN_ID, 'owner': OWNER, 'host_boot': BOOT}, series.MARK: series.binding(case),
                   'origin.json': {'boot': timing.BOOT_ID}, 'retired.json': {'natural': True, 'app_pid': timing.PID}, 'card.json': card,
                   'released.json': {'owner_thread': OWNER, 'run_id': timing.RUN_ID, 'state': 'released', 'owner_and_token_checked': True,
                                     'native_restored_and_connections_closed': True, 'device_disposition': 'original-native-restored'}}
        for leaf, record in records.items():
            CONTROL.save(batch / leaf, record)
        for current in (rows, timing.trace(render_order=None, include_send=False, include_driver=False)):
            (batch / (timing.RUN_ID + '.jsonl')).write_text(''.join(json.dumps(row) + '\n' for row in current))
            (batch / 'intervals.json').write_text(json.dumps(series.BOUNDS.audit(card, current)))
            self.assertEqual(series.closed_case(case, self.plan, None), timing.BOOT_ID)
        with self.assertRaisesRegex(CONTROL.OperatorError, 'device boot'):
            series.closed_case(case, self.plan, 'different')
        (batch / 'stop-intended').touch()
        with self.assertRaisesRegex(CONTROL.OperatorError, 'natural retirement'):
            series.closed_case(case, self.plan, None)

    def test_next_attempt_waits_only_at_returned_boundary_and_busy_stops(self):
        calls = []
        cases = self.plan['cases'][:3]
        def execute(case):
            if len(calls) == 1:
                self.assertEqual(case, Path(cases[1]['batch']))
                raise CONTROL.OperatorError('shared lease busy')
            def first(_recover):
                calls.append('attempt')
            return mock.Mock(execute=first)
        def closed(case, plan, boot):
            self.assertIsNone(boot)
            (self.root / series.HOLD).touch()
            return BOOT
        def clear(_duration):
            self.assertEqual(calls, ['attempt'])
            (self.root / series.HOLD).unlink()
        with mock.patch.object(series, 'ready'), mock.patch.object(series, 'closed_case', side_effect=closed), \
             mock.patch.object(CONTROL, 'Operator', side_effect=execute), mock.patch.object(series.sys.stdin, 'isatty', return_value=True), \
             mock.patch.object(series.getpass, 'getpass', return_value='fixture'), mock.patch.object(series.time, 'sleep', side_effect=clear) as pause, \
             contextlib.redirect_stdout(io.StringIO()):
            with self.assertRaisesRegex(CONTROL.OperatorError, 'busy'):
                series.start(self.root, self.plan, cases)
            pause.assert_called_once()
            with self.assertRaisesRegex(CONTROL.OperatorError, 'repeat'):
                series.start(self.root, self.plan, cases)


if __name__ == '__main__':
    unittest.main()
