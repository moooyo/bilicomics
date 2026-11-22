"""Exercise three fresh host visits and the single allowed device leaf without admitting a Kindle."""

import argparse
import contextlib
import io
import json
import os
from pathlib import Path
import shutil
import struct
import sys
import tarfile
import tempfile
import unittest
from unittest import mock
import zipfile


HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import handoff_runner as study


CONTROL = study.CONTROL
FIX = CONTROL.module(study.ROUTE.SOURCE / 'test_route.py')
GUARD = CONTROL.module(study.BASE.STUDY / 'host_tools/test_native_guard.py')
STOCK = HERE.parents[2] / 'control/launch.sh'
BOOT = 'a0a0a0a0-bbbb-cccc-dddd-e0e0e0e0e0e0'
OWNER = 'fixture-owner'


class Route(FIX.RouteTest):
    def test_approved_flag_reversible_route_and_missing_or_partial_sidecar(self):
        stock = STOCK.read_bytes()
        self.shipping = stock
        self.stock.write_bytes(stock)
        (self.stage / 'shipping-launcher.sh').write_bytes(stock)
        files = study.case_files(stock, FIX.RUN)
        old_launch = study.ROUTE.diagnostic_launcher(stock, FIX.RUN)
        self.assertEqual(files['entry.sh'], study.ROUTE.entry(FIX.RUN))
        self.assertEqual(files['launch.sh'].replace(b' --prepared-handoff-trace "$DATA_DIR/' + FIX.RUN.encode() + b'.prepared-handoff.json"', b''), old_launch)
        original = (study.ROUTE.SOURCE / 'switch.sh').read_bytes()
        adapted = self.switch.read_bytes()
        for old, new in zip(original.splitlines(keepends=True), files['switch.sh'].splitlines(keepends=True), strict=True):
            if old != new:
                self.assertEqual(adapted.count(old), 1)
                adapted = adapted.replace(old, new)
        self.switch.write_bytes(adapted)
        launch = self.stage / 'launch.sh'
        for rejected in (old_launch, files['launch.sh'].replace(b'.prepared-handoff.json', b'.unknown.json'), files['launch.sh'] + b'\n'):
            launch.write_bytes(rejected)
            self.invoke('activate')
            self.assertFalse(self.journal.exists())
            self.check_stock()
        launch.write_bytes(files['launch.sh'])
        leaf = self.stage / 'data' / (FIX.RUN + '.prepared-handoff.json')
        leaf.touch()
        self.invoke('activate')
        leaf.unlink()
        self.invoke('activate', 'armed')
        leaf.write_bytes(b'truncated')
        self.invoke('restore', 'stock')
        unknown = self.stage / 'data' / ('bili-shelf-' + 'c' * 32 + '.prepared-handoff.json')
        unknown.touch()
        self.invoke('cleanup')
        self.assertTrue(unknown.is_file() and leaf.is_file())
        unknown.unlink()
        self.invoke('cleanup', 'removed')
        self.check_stock()


class Guard(GUARD.GuardTest):
    def do_witness(self, option):
        stock = (study.BASE.STUDY / 'host_tools/native_guard.sh').read_bytes()
        before = study.BASE.GUARD_BINDING
        after = study.handoff_guard().split(b'--shelf-render-trace', 1)[1].split(b' || return 1', 1)[0]
        self.assertEqual(stock.count(before), 1)
        self.source = self.source.replace(before.decode(), '--shelf-render-trace' + after.decode())
        original = self.process

        def process(pid, parent, group, session, birth, args, **kwargs):
            if pid == 704:
                index = args.index('--exit-after-ms')
                args = [*args[:index], *option, *args[index:]]
            return original(pid, parent, group, session, birth, args, **kwargs)

        with mock.patch.object(self, 'process', side_effect=process):
            self.observe_app_then_retire()

    def test_exact_run_sidecar_argv_has_independent_pid(self):
        self.do_witness(['--prepared-handoff-trace', self.stage / 'data' / (GUARD.RUN + '.prepared-handoff.json')])

    def test_an_old_elf_invocation_does_not_get_a_pid(self):
        guard = self.guard
        with mock.patch.object(self, 'guard', side_effect=lambda op, success, output: guard(op, success=success, output='')):
            self.do_witness([])


class Harness(unittest.TestCase):
    def setUp(self):
        stack = self.enterContext(contextlib.ExitStack())
        stack.callback(os.umask, os.umask(0o077))
        self.root = Path(stack.enter_context(tempfile.TemporaryDirectory())).resolve()
        stack.enter_context(mock.patch.object(CONTROL, 'HOST', self.root))
        stack.enter_context(mock.patch.object(CONTROL, 'identity', return_value=(OWNER, BOOT)))
        stack.enter_context(mock.patch.object(CONTROL, 'tools'))
        stack.enter_context(mock.patch.object(study.FROZEN, 'STOCK_ZIP', self.root / 'stock.zip'))
        stack.enter_context(mock.patch.object(CONTROL, 'announce'))

    def inputs(self):
        members = {name: self.root / name for name in study.INPUTS}
        header = bytearray(84)
        header[:7] = b'\x7fELF\x01\x01\x01'
        struct.pack_into('<HH', header, 16, 2, 40)
        struct.pack_into('<I', header, 28, 52)
        struct.pack_into('<I', header, 36, 0x400)
        struct.pack_into('<HH', header, 42, 32, 1)
        struct.pack_into('<I', header, 52, 1)
        for field in ('application', 'worker', 'route_fs'):
            members[field].write_bytes(header + field.encode())
        members.update(qualified_application=self.root / 'artifacts/bilicomics', arm_qualification=self.root / 'qualification/arm.json')
        for name in ('artifacts', 'qualification'):
            (self.root / name).mkdir()
        shutil.copyfile(members['application'], members['qualified_application'])
        members['shipping_launcher'].write_bytes(STOCK.read_bytes())
        members['document_launcher'].write_bytes(study.ROUTE.DOCUMENT)
        with zipfile.ZipFile(study.FROZEN.STOCK_ZIP, 'x') as archive:
            for key, relative in (('shipping_launcher', 'bilicomics/launch.sh'), ('document_launcher', 'documents/bilicomics.sh'),
                                  ('worker', 'bilicomics/app/kindle-jpeg-worker')):
                archive.writestr(relative, members[key].read_bytes())
        members['native_kit'] = detector = self.root / 'paired-kit.tar.gz'
        with tarfile.open(detector, 'x:gz') as archive:
            for name in sorted(CONTROL.REQUIRED):
                member = tarfile.TarInfo(name)
                member.mode, member.size = (0o600 if name.startswith('models/') else 0o700), 1
                archive.addfile(member, io.BytesIO(b'x'))
        detector.with_suffix('').with_suffix('.json').write_text(json.dumps({'members': sorted(CONTROL.REQUIRED), 'bytes': len(CONTROL.REQUIRED)}))
        proof = {'schema': 'bili-prepared-handoff-arm-qualification-v1', 'passed': True, 'physical_device_accessed': False,
                 'device_lease_accessed': False, 'cargo_bin': 'bilicomics', 'cross_target': 'armv7-unknown-linux-musleabihf',
                 'static_elf32_arm_hard_float': True, 'application_bytes': members['application'].stat().st_size,
                 'cli': {'missing_shelf_trace_rejected': True, 'conflicting_lifecycle_rejected': True},
                 'worker': {'unchanged_sources': True, 'identical_to_install_zip': True, 'bytes': members['worker'].stat().st_size},
                 'profiles': [{'profile': profile, 'native_exit_code': 1, 'prepared_handoff_cli_recognized': True,
                               'missing_owned_snapshot_rejected': True, 'native_framebuffer_available': False, 'native_input_available': False,
                               'shelf_rendered': False} for profile in ('scribe', 'pw6')]}
        CONTROL.save(members['arm_qualification'], proof)
        return argparse.Namespace(**members, output=self.root / 'study')

    def test_three_visits_copy_qualified_arm_and_reject_changed_or_old_inputs(self):
        args = self.inputs()
        receipt = args.arm_qualification.read_bytes()
        proof = json.loads(receipt)
        del proof['profiles'][1]
        args.arm_qualification.write_text(json.dumps(proof))
        with self.assertRaisesRegex(CONTROL.OperatorError, 'profiles|profile'):
            study.prepare(args)
        args.arm_qualification.write_bytes(receipt)
        with contextlib.redirect_stdout(io.StringIO()):
            study.prepare(args)
        plan = CONTROL.load(args.output / 'plan.json')
        cases = study.checked(plan, OWNER, BOOT)
        self.assertEqual([case['ordinal'] for case in cases], [1, 2, 3])
        study.verify(args.output, plan, cases)
        args.application.write_bytes(b'X' + args.application.read_bytes()[1:])
        with self.assertRaisesRegex(CONTROL.OperatorError, 'independent byte copy'):
            study.verify(args.output, plan, cases)
        self.assertNotIn('clock-marks', CONTROL.FILES)
        with self.assertRaises(CONTROL.OperatorError):
            study.checked({**plan, 'schema': 'bili-shelf-clock-aa-v1'}, OWNER, BOOT)

    def test_foreground_no_early_start_no_replacement_and_exact_previous_closure(self):
        args = self.inputs()
        with contextlib.redirect_stdout(io.StringIO()):
            study.prepare(args)
        plan = CONTROL.load(args.output / 'plan.json')
        cases = study.checked(plan, OWNER, BOOT)
        first, second = (Path(case['batch']) for case in cases[:2])
        verified = set()

        def valid(path):
            if path not in verified:
                raise CONTROL.OperatorError('No exact release and observation')
            return BOOT

        def visit(path):
            def execute(recover):
                self.assertFalse(recover)
                (path / 'attempted').touch()
                if path == second:
                    raise CONTROL.OperatorError('busy')
                verified.add(path)
            return mock.Mock(execute=execute)

        with mock.patch.object(study, 'HandoffVisit', side_effect=visit) as invoked, mock.patch.object(sys.stdin, 'isatty', return_value=False):
            with self.assertRaisesRegex(CONTROL.OperatorError, 'foreground'):
                study.visit(args.output, plan, cases, 1, valid)
            self.assertFalse((args.output / 'started.json').exists())
            invoked.assert_not_called()
        with mock.patch.object(study, 'HandoffVisit', side_effect=visit), mock.patch.object(sys.stdin, 'isatty', return_value=True):
            with self.assertRaisesRegex(CONTROL.OperatorError, 'exact release'):
                study.visit(args.output, plan, cases, 3, valid)
            study.visit(args.output, plan, cases, 1, valid)
            self.assertEqual(verified, {first})
            self.assertEqual(CONTROL.load(first / study.ADMITTED), study.input_receipt(cases[0]))
            with self.assertRaisesRegex(CONTROL.OperatorError, 'busy'):
                study.visit(args.output, plan, cases, 2, valid)
            with self.assertRaisesRegex(CONTROL.OperatorError, 'retry'):
                study.visit(args.output, plan, cases, 2, valid)
            with self.assertRaisesRegex(CONTROL.OperatorError, 'exact release'):
                study.visit(args.output, plan, cases, 3, valid)


def load_tests(loader, tests, pattern):
    suite = loader.loadTestsFromTestCase(Harness)
    for cls in (Route, Guard):
        suite.addTests(cls(name) for name in cls.__dict__ if name.startswith('test_'))
    return suite


if __name__ == '__main__':
    unittest.main()
