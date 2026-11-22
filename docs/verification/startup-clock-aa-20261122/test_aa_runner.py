"""Rehearse stock byte selection, exact process witnessing, receipt binding and leased cleanup offline."""

import argparse
import contextlib
import io
import json
import os
from pathlib import Path
import shlex
import subprocess
import struct
import sys
import tarfile
import tempfile
import unittest
from unittest import mock
import zipfile


HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import aa_runner as clock


CONTROL = clock.CONTROL
ROUTE = clock.ROUTE
FIX = CONTROL.module(ROUTE.SOURCE / 'test_route.py')
GUARD = CONTROL.module(clock.BASE.STUDY / 'host_tools/test_native_guard.py')
STOCK = Path(os.environ.get('BILI_STUDY_STOCK_LAUNCHER', HERE.parents[2] / 'control/launch.sh'))
OWNER = 'fixture-owner'
BOOT = 'a0a0a0a0-bbbb-cccc-dddd-e0e0e0e0e0e0'


class RouteTests(FIX.RouteTest):
    def setUp(self):
        super().setUp()
        self.shipping = STOCK.read_bytes()
        self.stock.write_bytes(self.shipping)
        (self.stage / 'shipping-launcher.sh').write_bytes(self.shipping)
        files = clock.case_files(self.shipping, FIX.RUN)
        self.shim = files['entry.sh'].replace(b'/mnt/us', os.fsencode(self.media))
        (self.stage / 'entry.sh').write_bytes(self.shim)
        self.ready.write_bytes(self.shim)
        (self.stage / 'launch.sh').write_bytes(files['launch.sh'])
        self.switch.write_bytes(clock.clock_switch(self.switch.read_bytes(), files['launch.sh'], self.shim))
        (self.stage / 'clock-marks').write_bytes(b'unchanged helper fixture')
        (self.stage / 'clock-marks').chmod(0o700)

    def test_clock_exact_argv_and_bytes_only(self):
        approved = (self.stage / 'launch.sh').read_bytes()
        for refused in (approved.replace(b'--lifecycle-trace', b'--other'), approved + b'\n', self.shipping):
            (self.stage / 'launch.sh').write_bytes(refused)
            self.invoke('activate')
            self.assertFalse(self.journal.exists())
            self.check_stock()
        (self.stage / 'launch.sh').write_bytes(approved)
        shim = self.stage / 'entry.sh'
        shim.write_bytes(self.shim + b'\n')
        self.ready.write_bytes(shim.read_bytes())
        self.invoke('activate')
        self.assertFalse(self.journal.exists())
        self.check_stock()
        shim.write_bytes(self.shim)
        self.ready.write_bytes(self.shim)
        self.invoke('activate', 'armed')
        self.invoke('restore', 'stock')
        self.invoke('cleanup', 'removed')
        self.check_stock()

    def test_runtime_leaves_do_not_block_restore_even_when_measurement_was_incomplete(self):
        lifecycle = self.stage / ('data/' + FIX.RUN + '-lifecycle.jsonl')
        lifecycle.touch()
        self.invoke('activate')
        lifecycle.unlink()
        self.invoke('activate', 'armed')
        self.picked.mkdir()
        (self.picked / 'entry').write_text(f'{FIX.RUN} 9999999 1\n')
        for file in (self.picked / 'shim-entry.json', self.picked / 'app-preinvoke.json', lifecycle):
            file.write_bytes(b'incomplete diagnostic')
        self.invoke('restore', 'stock')
        self.check_stock()
        sentinel = self.picked / 'other-entry.json'
        sentinel.touch()
        self.invoke('cleanup')
        self.assertTrue(lifecycle.is_file() and sentinel.is_file())
        sentinel.unlink()
        self.invoke('cleanup', 'removed')
        self.check_stock()

    def test_replaced_helper_blocks_restore_and_no_path_is_overwritten(self):
        self.invoke('activate', 'armed')
        helper = self.stage / 'clock-marks'
        retained = self.root / 'system/clock-original'
        helper.rename(retained)
        helper.write_bytes(retained.read_bytes())
        self.invoke('restore')
        self.assertEqual(self.retained.stat().st_ino, self.stock_inode)
        helper.unlink()
        retained.rename(helper)
        self.invoke('restore', 'stock')
        self.invoke('cleanup', 'removed')
        self.check_stock()


class GuardTests(GUARD.GuardTest):
    def test_clock_wait_witnesses_only_exact_application_argv(self):
        original = self.process

        def with_lifecycle(pid, parent, group, session, birth, args, **kwargs):
            if pid == 704:
                args = [*args, '--lifecycle-trace', self.stage / 'data' / (GUARD.RUN + '-lifecycle.jsonl')]
            return original(pid, parent, group, session, birth, args, **kwargs)

        self.source = clock.guard_for_clock(self.source.encode('ascii')).decode('ascii')
        with mock.patch.object(self, 'process', side_effect=with_lifecycle):
            self.observe_app_then_retire()

    def test_clock_wait_does_not_bind_old_argv(self):
        self.source = clock.guard_for_clock(self.source.encode('ascii')).decode('ascii')
        observe = self.guard

        def missing(verb, *, success, output=''):
            self.assertEqual(output, 'app_pid=704\n')
            return observe(verb, success=success, output='')

        with mock.patch.object(self, 'guard', side_effect=missing):
            self.observe_app_then_retire()


class RunnerTests(unittest.TestCase):
    def setUp(self):
        stack = contextlib.ExitStack()
        self.addCleanup(stack.close)
        stack.callback(os.umask, os.umask(0o077))
        self.root = Path(stack.enter_context(tempfile.TemporaryDirectory())).resolve()
        stack.enter_context(mock.patch.object(CONTROL, 'HOST', self.root))
        stack.enter_context(mock.patch.object(CONTROL, 'identity', return_value=(OWNER, BOOT)))
        self.plan = clock.make_plan(OWNER, BOOT)

    def test_schedule_labels_do_not_change_treatment(self):
        cases = clock.checked(self.plan, OWNER, BOOT)
        self.assertEqual([(case['pair'], case['arm']) for case in cases], [(index, arm) for index, arms in enumerate(('AB', 'AB', 'BA', 'AB', 'BA')) for arm in arms])
        self.assertEqual(len({case['run'] for case in cases}), 10)
        outputs = [clock.case_files(STOCK.read_bytes(), case['run']) for case in cases]
        for name in outputs[0]:
            normalized = {output[name].replace(case['run'].encode(), b'RUN') for output, case in zip(outputs, cases)}
            self.assertEqual(len(normalized), 1, name)
        self.assertNotIn(b'--serial-shelf-render', outputs[0]['launch.sh'])
        self.assertNotIn(b'BILICOMICS_LIBRARY_PREFLIGHT_STDERR', outputs[0]['launch.sh'])
        changed = {**self.plan, 'cases': [*cases[:1], cases[0], *cases[2:]]}
        with self.assertRaises(CONTROL.OperatorError):
            clock.checked(changed, OWNER, BOOT)

    def inputs(self):
        header = bytearray(84)
        header[:7] = b'\x7fELF\x01\x01\x01'
        struct.pack_into('<HH', header, 16, 2, 40)
        struct.pack_into('<I', header, 28, 52)
        struct.pack_into('<I', header, 36, 0x400)
        struct.pack_into('<HH', header, 42, 32, 1)
        struct.pack_into('<I', header, 52, 1)
        paths = {key: self.root / key for key in clock.INPUTS}
        for field in ('application', 'worker', 'route_fs', 'clock_marks'):
            paths[field].write_bytes(header + field.encode())
        paths['shipping_launcher'].write_bytes(STOCK.read_bytes())
        paths['document_launcher'].write_bytes(ROUTE.DOCUMENT)
        stock_zip = self.root / 'stock.zip'
        with zipfile.ZipFile(stock_zip, 'x') as archive:
            archive.writestr('bilicomics/launch.sh', STOCK.read_bytes())
            archive.writestr('documents/bilicomics.sh', ROUTE.DOCUMENT)
        self.addCleanup(mock.patch.stopall)
        mock.patch.object(clock.FROZEN, 'STOCK_ZIP', stock_zip).start()
        paths['native_kit'] = detector = self.root / 'paired-kit.tar.gz'
        with tarfile.open(detector, 'x:gz') as archive:
            for name in sorted(CONTROL.REQUIRED):
                member = tarfile.TarInfo(name)
                member.mode, member.size = (0o600 if name.startswith('models/') else 0o700), 1
                archive.addfile(member, io.BytesIO(b'x'))
        detector.with_suffix('').with_suffix('.json').write_text(json.dumps({'members': sorted(CONTROL.REQUIRED), 'bytes': len(CONTROL.REQUIRED)}))
        return argparse.Namespace(**paths, output=self.root / 'series')

    def test_complete_seal_exact_sources_helper_and_scoped_unused_cleanup(self):
        args = self.inputs()
        stock = args.shipping_launcher.read_bytes()
        args.shipping_launcher.write_bytes(stock + b'\n')
        with self.assertRaisesRegex(CONTROL.OperatorError, 'frozen'):
            clock.prepare(args)
        self.assertFalse(args.output.exists())
        args.shipping_launcher.write_bytes(stock)
        with mock.patch.object(CONTROL, 'tools'), mock.patch.object(clock, 'ClockVisit') as live, contextlib.redirect_stdout(io.StringIO()):
            clock.prepare(args)
            plan = CONTROL.load(args.output / 'plan.json')
            cases = clock.checked(plan, OWNER, BOOT)
            clock.ready(args.output, cases, plan)
            live.assert_not_called()
        source = args.clock_marks.read_bytes()
        args.clock_marks.write_bytes(source[:-1] + b'!')
        with self.assertRaisesRegex(CONTROL.OperatorError, 'archive'):
            clock.ready(args.output, cases, plan)
        args.clock_marks.write_bytes(source)
        last = Path(cases[-1]['batch'])
        CONTROL.save(last / clock.ADMITTED, clock.input_receipt(cases[-1]))
        first = Path(cases[0]['batch'])
        (first / 'attempted').touch()
        clock.discard(plan, cases)
        self.assertTrue(first.is_dir())
        self.assertFalse(any(Path(case['batch']).exists() for case in cases[1:]))
        self.assertNotIn('clock-marks', clock.FROZEN.CONTROL.FILES)
        self.assertIn('clock-marks', CONTROL.FILES)

    def test_partial_payload_cleanup_names_only_new_owned_helper(self):
        case = self.plan['cases'][0]
        destination = '/mnt/us/' + case['run']
        cleanup = CONTROL.cleanup_payload(destination, case['run'], CONTROL.FILES)
        path = self.root / case['run']
        path.mkdir()
        (path / 'run-id').write_text(case['run'] + '\n')
        (path / 'clock-marks').write_bytes(b'partial')
        outsider = self.root / 'outside'
        outsider.touch()
        (path / 'unknown').symlink_to(outsider)
        cleanup = cleanup.replace(destination, shlex.quote(str(path)))
        self.assertNotEqual(subprocess.run(['sh', '-ec', cleanup], capture_output=True, timeout=5).returncode, 0)
        self.assertTrue((path / 'clock-marks').exists() and outsider.is_file())
        (path / 'unknown').unlink()
        subprocess.run(['sh', '-ec', cleanup], check=True, timeout=5)
        self.assertFalse(path.exists())
        self.assertTrue(outsider.is_file())

    def test_handoff_is_off_lease_and_stop_does_not_try_another_visit(self):
        cases = self.plan['cases']
        events = []
        hold = self.root / clock.FROZEN.HOLD
        hold.touch()

        def release_hold(_delay):
            self.assertEqual(events, [])
            hold.unlink()

        def visit(path):
            def execute(recover):
                self.assertFalse(recover)
                self.assertEqual(instance.read_password(), 'fixture')
                events.append(str(path))
                if path == Path(cases[1]['batch']):
                    raise CONTROL.OperatorError('busy; no reservation')

            instance = mock.Mock(execute=execute)
            return instance

        with mock.patch.object(clock.time, 'sleep', side_effect=release_hold), mock.patch.object(clock, 'ClockVisit', side_effect=visit), \
             mock.patch.object(clock, 'check_batch', side_effect=lambda case, plan: (Path(case['batch']), {})), \
             mock.patch.object(CONTROL, 'announce'), self.assertRaisesRegex(CONTROL.OperatorError, 'busy'):
            clock.sequence(self.root, self.plan, cases, lambda: 'fixture', lambda path: events.append('validated') or BOOT)
        self.assertEqual(events, [cases[0]['batch'], 'validated', cases[1]['batch']])


def load_tests(loader, tests, pattern):
    suite = unittest.TestSuite()
    suite.addTests(loader.loadTestsFromTestCase(RunnerTests))
    for cls in (RouteTests, GuardTests):
        suite.addTests(cls(name) for name in cls.__dict__ if name.startswith('test_'))
    return suite


if __name__ == '__main__':
    unittest.main()
