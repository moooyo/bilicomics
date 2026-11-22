"""Rehearse the startup-only environment, ordinary app witness and exact reversible routes offline."""

import contextlib
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest import mock
import zipfile


HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import display_runner as display


CONTROL = display.CONTROL
PREVIOUS = CONTROL.module(HERE.parent / 'startup-prepared-handoff-20261122/test_handoff_runner.py')
STOCK = HERE.parents[2] / 'control/launch.sh'
FIX = PREVIOUS.FIX


class Route(FIX.RouteTest):
    def test_exact_switch_restores_and_only_removes_the_bound_single_leaf(self):
        self.shipping = STOCK.read_bytes()
        self.stock.write_bytes(self.shipping)
        (self.stage / 'shipping-launcher.sh').write_bytes(self.shipping)
        files = display.case_files(self.shipping, FIX.RUN)
        raw = (display.ROUTE.SOURCE / 'switch.sh').read_bytes()
        def verifier(script):
            return script.split(b'\nlauncher_verified() {\n', 1)[1].split(b'\n}\n\nclean_stage()', 1)[0]
        original = self.switch.read_bytes()
        fake = original.replace(verifier(raw), verifier(files['switch.sh']))
        for before, after in ((b'"data/$run.jsonl"|data/logs/launcher.log|', b'"data/$run.display-cost.json"|"data/$run.jsonl"|data/logs/launcher.log|'),
                              (b'! present "$stage/data/$run.jsonl" &&', b'! present "$stage/data/$run.display-cost.json" && ! present "$stage/data/$run.jsonl" &&')):
            self.assertIn(after, files['switch.sh'])
            self.assertEqual(fake.count(before), 1)
            fake = fake.replace(before, after)
        self.switch.write_bytes(fake)
        expected = files['launch.sh']
        unobserved = display.ROUTE.diagnostic_launcher(self.shipping, FIX.RUN)
        launch = self.stage / 'launch.sh'
        self.assertEqual(files['entry.sh'], display.ROUTE.entry(FIX.RUN))
        for reject in (unobserved, expected.replace(display.WAKE, display.WAKE.replace(b'capture_native', b'BILICOMICS_DISPLAY_COST_RUN=x capture_native')),
                       expected.replace(b' --exit-after-ms 2500 >&6', b' --exit-after-ms 3000 >&6')):
            launch.write_bytes(reject)
            self.invoke('activate')
            self.assertFalse(self.journal.exists())
            self.check_stock()
        launch.write_bytes(expected)
        leaf = self.stage / 'data' / (FIX.RUN + '.display-cost.json')
        leaf.touch()
        self.invoke('activate')
        leaf.unlink()
        self.invoke('activate', 'armed')
        leaf.write_bytes(b'incomplete evidence')
        self.invoke('restore', 'stock')
        foreign = self.stage / 'data' / ('bili-shelf-' + 'e' * 32 + '.display-cost.json')
        foreign.touch()
        self.invoke('cleanup')
        self.assertTrue(leaf.exists() and foreign.exists())
        foreign.unlink()
        self.invoke('cleanup', 'removed')
        self.check_stock()


class Shell(unittest.TestCase):
    def test_startup_only_binding_reaches_the_child_but_never_wake_or_ui(self):
        files = display.case_files(STOCK.read_bytes(), FIX.RUN)
        launch = files['launch.sh']
        expected = next(line for line in launch.splitlines(keepends=True) if line.startswith(b'if ! ' + display.ENV_RUN))
        self.assertEqual(launch.count(display.ENV_RUN), 1)
        self.assertEqual(launch.replace(expected, display.START), display.ROUTE.diagnostic_launcher(STOCK.read_bytes(), FIX.RUN))
        def function(name):
            begin = name + b'() {\n'
            return begin + launch.split(begin, 1)[1].split(b'\n}\n', 1)[0] + b'\n}\n'
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            helper = root / 'bilicomics'
            helper.write_text('#!/bin/sh\nset -eu\nprintf "%s|%s\\n" "${BILICOMICS_DISPLAY_COST_RUN-unset}" "$*" >>"$log"\nprintf "%s\\n" "$result"\n')
            helper.chmod(0o700)
            script = (b'set -eu\n' + function(b'native_read_hex') + function(b'capture_native_epdc_mode') +
                      b'native_read_command() { "$@" >"$LOCK_DIR/native-read-output"; }\n' + expected +
                      b'    exit 41\nfi\n[ "$NATIVE_CAPTURE_STATUS" = known ] || exit 42\n' + display.WAKE +
                      b'"$APP_DIR/bilicomics" --ordinary-ui\n')
            env = {'PATH': os.defpath, 'APP_DIR': temporary, 'LOCK_DIR': temporary, 'APP_GENERATION': '1', 'LAUNCH_TOKEN': 'fixture',
                   'PROFILE': 'scribe', 'log': str(root / 'observed'), 'result': 'ready:Y8:U:0'}
            good = subprocess.run(['/bin/sh', '-s'], input=script, env=env, capture_output=True, timeout=5)
            self.assertEqual(good.returncode, 0, good.stderr)
            args = '--capture-native-display --kindle --device scribe'
            self.assertEqual((root / 'observed').read_text().splitlines(), [FIX.RUN + '|' + args, 'unset|' + args, 'unset|--ordinary-ui'])
            (root / 'observed').unlink()
            bad = subprocess.run(['/bin/sh', '-s'], input=script, env={**env, 'result': 'malformed'}, capture_output=True, timeout=5)
            self.assertEqual(bad.returncode, 41, bad.stderr)
            self.assertEqual((root / 'observed').read_text().splitlines(), [FIX.RUN + '|' + args])


class Harness(unittest.TestCase):
    def setUp(self):
        self.stack = self.enterContext(contextlib.ExitStack())
        self.stack.callback(os.umask, os.umask(0o077))
        self.root = Path(self.stack.enter_context(tempfile.TemporaryDirectory())).resolve()
        self.stack.enter_context(mock.patch.object(CONTROL, 'HOST', self.root))
        self.stack.enter_context(mock.patch.object(CONTROL, 'identity', return_value=(PREVIOUS.OWNER, PREVIOUS.BOOT)))
        self.stack.enter_context(mock.patch.object(CONTROL, 'tools'))
        self.stack.enter_context(mock.patch.object(display.FROZEN, 'STOCK_ZIP', self.root / 'stock.zip'))
        self.stack.enter_context(mock.patch.object(PREVIOUS.study.FROZEN, 'STOCK_ZIP', self.root / 'stock.zip'))
        self.stack.enter_context(mock.patch.object(CONTROL, 'announce'))

    def inputs(self):
        args = PREVIOUS.Harness.inputs(self)
        worker = args.worker.read_bytes().ljust(426672, b'\0')
        args.worker.write_bytes(worker)
        with zipfile.ZipFile(display.FROZEN.STOCK_ZIP, 'w') as archive:
            archive.writestr('bilicomics/launch.sh', STOCK.read_bytes())
            archive.writestr('documents/bilicomics.sh', display.ROUTE.DOCUMENT)
            archive.writestr('bilicomics/app/kindle-jpeg-worker', worker)
        profile = {'native_exit_code': 1, 'helper_exit_code': 1, 'missing_owned_snapshot_rejected': True, 'helper_missing_permission_rejected': True,
                   'helper_cost_sidecar_created': False, 'native_framebuffer_available': False, 'native_input_available': False, 'shelf_rendered': False}
        report = {'schema': 'bili-display-helper-arm-qualification-v1', 'passed': True, 'physical_device_accessed': False, 'device_lease_accessed': False,
                  'cargo_bin': 'bilicomics', 'cross_target': 'armv7-unknown-linux-musleabihf', 'static_elf32_arm_hard_float': True,
                  'application_bytes': args.qualified_application.stat().st_size, 'host_integration_passed': True, 'overlay_only': True,
                  'version_exit_code': 0, 'cli_version': 'bilicomics 0.1.25', 'profiles': [{**profile, 'profile': name} for name in ('scribe', 'pw6')],
                  'worker': {'unchanged_sources': True, 'bytes': len(worker), 'identical_to_install_zip': True}}
        args.arm_qualification.write_text(json.dumps(report))
        args.host_qualification = self.root / 'host-product-qualification.json'
        CONTROL.save(args.host_qualification, {'schema': 'bili-display-host-product-qualification-v1', 'passed': True, 'complete_product_elf': True,
                                               'unchanged_permit': True, 'only_hardware_inputs_synthetic': True, 'physical_device_accessed': False,
                                               'collector_validated': True, 'profiles': {name: {'passed': True} for name in ('scribe', 'pw6')}})
        return args

    def test_whole_archives_stock_guard_and_rejected_writable_or_unqualified_payload(self):
        args = self.inputs()
        with contextlib.redirect_stdout(io.StringIO()):
            display.THREE.prepare(args)
        plan = CONTROL.load(args.output / 'plan.json')
        cases = display.THREE.checked(plan, PREVIOUS.OWNER, PREVIOUS.BOOT)
        self.assertEqual([case['ordinal'] for case in cases], [1, 2, 3])
        display.THREE.verify(args.output, plan, cases)
        guard = Path(cases[0]['batch']) / 'native-guard.sh'
        stock = display.stock_process_guard()
        self.assertEqual(guard.read_bytes(), stock)
        guard.write_bytes(stock + b'\n')
        with self.assertRaisesRegex(CONTROL.OperatorError, 'Altered'):
            display.THREE.verify(args.output, plan, cases)
        guard.write_bytes(stock)
        args.worker.chmod(0o666)
        with self.assertRaises(CONTROL.OperatorError):
            display.THREE.verify(args.output, plan, cases)
        args.worker.chmod(0o600)
        saved_host = args.host_qualification.read_bytes()
        args.host_qualification.write_text(json.dumps({**json.loads(saved_host), 'collector_validated': False}))
        with self.assertRaisesRegex(CONTROL.OperatorError, 'product-host'):
            display.THREE.verify(args.output, plan, cases)
        args.host_qualification.write_bytes(saved_host)
        report = json.loads(args.arm_qualification.read_bytes())
        report['profiles'][1]['helper_cost_sidecar_created'] = True
        args.arm_qualification.write_text(json.dumps(report))
        with self.assertRaisesRegex(CONTROL.OperatorError, 'unauthorized'):
            display.THREE.verify(args.output, plan, cases)

    def test_original_controller_cannot_accept_or_recover_new_plan(self):
        plan = display.THREE.plan_for(PREVIOUS.OWNER, PREVIOUS.BOOT)
        self.assertEqual(plan['schema'], 'bili-display-sampling-v1')
        self.assertNotEqual(display.MARK, PREVIOUS.study.MARK)
        with self.assertRaises(PREVIOUS.CONTROL.OperatorError):
            PREVIOUS.study.checked(plan, PREVIOUS.OWNER, PREVIOUS.BOOT)
        with self.assertRaises(CONTROL.OperatorError):
            display.THREE.checked({**plan, 'schema': 'bili-prepared-handoff-v1'}, PREVIOUS.OWNER, PREVIOUS.BOOT)
        self.assertTrue(all(display.input_receipt(case)['schema'] == 'bili-display-sampling-inputs-v1' for case in plan['cases']))


def load_tests(loader, tests, pattern):
    suite = loader.loadTestsFromTestCase(Shell)
    suite.addTests(loader.loadTestsFromTestCase(Harness))
    suite.addTest(Route('test_exact_switch_restores_and_only_removes_the_bound_single_leaf'))
    suite.addTest(PREVIOUS.GUARD.GuardTest('test_wait_witnesses_isolated_app_pid_outside_its_trace'))
    return suite


if __name__ == '__main__':
    unittest.main()
