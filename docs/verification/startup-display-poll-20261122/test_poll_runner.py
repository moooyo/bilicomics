"""Rehearse the independent poll selector, evidence roster, build gates and visit boundaries."""

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


HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import poll_runner as poll


BASIS = poll.CONTROL.module(HERE.parent / 'startup-display-sampling-20261122/test_display_runner.py')
FIX = BASIS.FIX


class Route(FIX.RouteTest):
    def test_poll_only_transitions_reject_old_argv_and_foreign_cleanup(self):
        stock = BASIS.STOCK.read_bytes()
        self.shipping = stock
        self.stock.write_bytes(stock)
        (self.stage / 'shipping-launcher.sh').write_bytes(stock)
        files = poll.case_files(stock, FIX.RUN)
        source = (poll.ROUTE.SOURCE / 'switch.sh').read_bytes()
        def verifier(script):
            return script.split(b'\nlauncher_verified() {\n', 1)[1].split(b'\n}\n\nclean_stage()', 1)[0]
        switch = self.switch.read_bytes()
        switch = switch.replace(verifier(source), verifier(files['switch.sh']))
        for before, after in ((b'"data/$run.jsonl"|data/logs/launcher.log|', b'"data/$run.display-poll.json"|"data/$run.jsonl"|data/logs/launcher.log|'),
                              (b'! present "$stage/data/$run.jsonl" &&', b'! present "$stage/data/$run.display-poll.json" && ! present "$stage/data/$run.jsonl" &&')):
            self.assertIn(after, files['switch.sh'])
            self.assertEqual(switch.count(before), 1)
            switch = switch.replace(before, after)
        self.switch.write_bytes(switch)
        launch = self.stage / 'launch.sh'
        for bad in (poll.SAMPLING.case_files(stock, FIX.RUN)['launch.sh'], files['launch.sh'].replace(poll.WAKE, poll.WAKE.replace(b'capture_native', b'BILICOMICS_DISPLAY_POLL_RUN=x capture_native'))):
            launch.write_bytes(bad)
            self.invoke('activate')
            self.assertFalse(self.journal.exists())
            self.check_stock()
        launch.write_bytes(files['launch.sh'])
        leaf = self.stage / 'data' / (FIX.RUN + '.display-poll.json')
        leaf.touch()
        self.invoke('activate')
        leaf.unlink()
        self.invoke('activate', 'armed')
        leaf.write_bytes(b'incomplete output')
        self.invoke('restore', 'stock')
        old = self.stage / 'data' / (FIX.RUN + '.display-cost.json')
        old.touch()
        self.invoke('cleanup')
        self.assertTrue(leaf.is_file() and old.is_file())
        old.unlink()
        self.invoke('cleanup', 'removed')
        self.check_stock()


class Shell(unittest.TestCase):
    def test_startup_only_binding_reaches_the_child_but_never_wake_or_ui(self):
        launch = poll.case_files(BASIS.STOCK.read_bytes(), FIX.RUN)['launch.sh']
        invocation = next(line for line in launch.splitlines(keepends=True) if line.startswith(b'if ! ' + poll.ENV_RUN))
        self.assertEqual(launch.count(poll.ENV_RUN), 1)
        self.assertEqual(launch.replace(invocation, poll.START), poll.ROUTE.diagnostic_launcher(BASIS.STOCK.read_bytes(), FIX.RUN))
        def function(name):
            start = name + b'() {\n'
            return start + launch.split(start, 1)[1].split(b'\n}\n', 1)[0] + b'\n}\n'
        script = (b'set -eu\n' + function(b'native_read_hex') + function(b'capture_native_epdc_mode') +
                  b'native_read_command() { "$@" >"$LOCK_DIR/native-read-output"; }\n' + invocation +
                  b'    exit 41\nfi\n[ "$NATIVE_CAPTURE_STATUS" = known ] || exit 42\n' + poll.WAKE + b'"$APP_DIR/bilicomics" --ordinary-ui\n')
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            probe = directory / 'bilicomics'
            probe.write_text('#!/bin/sh\nset -eu\nprintf "%s|%s|%s\\n" "${BILICOMICS_DISPLAY_POLL_RUN-unset}" "${BILICOMICS_DISPLAY_COST_RUN-unset}" "$*" >>"$out"\nprintf "%s\\n" "$result"\n')
            probe.chmod(0o700)
            out = directory / 'observations'
            env = {'PATH': os.defpath, 'APP_DIR': temporary, 'LOCK_DIR': temporary, 'APP_GENERATION': '1', 'LAUNCH_TOKEN': 'fixture',
                   'PROFILE': 'scribe', 'out': str(out), 'result': 'ready:Y8:U:0'}
            result = subprocess.run(['/bin/sh', '-s'], input=script, env=env, capture_output=True, timeout=5)
            self.assertEqual(result.returncode, 0, result.stderr)
            capture = '--capture-native-display --kindle --device scribe'
            self.assertEqual(out.read_text().splitlines(), [FIX.RUN + '|unset|' + capture, 'unset|unset|' + capture, 'unset|unset|--ordinary-ui'])
            out.unlink()
            result = subprocess.run(['/bin/sh', '-s'], input=script, env={**env, 'result': 'malformed'}, capture_output=True, timeout=5)
            self.assertEqual(result.returncode, 41, result.stderr)
            self.assertEqual(out.read_text().splitlines(), [FIX.RUN + '|unset|' + capture])


class Harness(BASIS.Harness):
    def setUp(self):
        super().setUp()
        for patch in (mock.patch.object(poll.CONTROL, 'HOST', self.root),
                      mock.patch.object(poll.CONTROL, 'identity', return_value=(BASIS.PREVIOUS.OWNER, BASIS.PREVIOUS.BOOT)),
                      mock.patch.object(poll.CONTROL, 'tools'), mock.patch.object(poll.FROZEN, 'STOCK_ZIP', self.root / 'stock.zip'),
                      mock.patch.object(poll.CONTROL, 'announce')):
            self.stack.enter_context(patch)

    def inputs(self):
        with mock.patch.object(BASIS.PREVIOUS, 'study', BASIS.display.THREE):
            args = super().inputs()
        arm = json.loads(args.arm_qualification.read_bytes())
        arm.update(schema='bili-display-poll-arm-qualification-v1', poll_branch_instrumentation=True)
        args.arm_qualification.write_text(json.dumps(arm))
        host = json.loads(args.host_qualification.read_bytes())
        host['schema'] = 'bili-display-poll-host-product-qualification-v1'
        for profile in host['profiles'].values():
            profile.update(poll_split_counts_sum=True, poll_split_elapsed_sum=True)
        args.host_qualification.write_text(json.dumps(host))
        return args

    def test_archive_and_true_build_proofs_do_not_mix_with_old_studies(self):
        args = self.inputs()
        with contextlib.redirect_stdout(io.StringIO()):
            poll.THREE.prepare(args)
        plan = poll.CONTROL.load(args.output / 'plan.json')
        cases = poll.THREE.checked(plan, BASIS.PREVIOUS.OWNER, BASIS.PREVIOUS.BOOT)
        poll.THREE.verify(args.output, plan, cases)
        self.assertTrue(all((Path(case['batch']) / 'native-guard.sh').read_bytes() == poll.SAMPLING.stock_process_guard() for case in cases))
        with self.assertRaises(BASIS.CONTROL.OperatorError):
            BASIS.display.THREE.checked(plan, BASIS.PREVIOUS.OWNER, BASIS.PREVIOUS.BOOT)
        for source, old_schema in ((args.arm_qualification, 'bili-display-helper-arm-qualification-v1'), (args.host_qualification, 'bili-display-host-product-qualification-v1')):
            original = source.read_bytes()
            source.write_text(json.dumps({**json.loads(original), 'schema': old_schema}))
            with self.assertRaises(poll.CONTROL.OperatorError):
                poll.THREE.verify(args.output, plan, cases)
            source.write_bytes(original)
        report = json.loads(args.host_qualification.read_bytes())
        report['profiles']['pw6']['poll_split_elapsed_sum'] = False
        args.host_qualification.write_text(json.dumps(report))
        with self.assertRaisesRegex(poll.CONTROL.OperatorError, 'poll-split'):
            poll.THREE.verify(args.output, plan, cases)

    def test_no_incomplete_or_repeated_visit_can_advance(self):
        with mock.patch.object(BASIS.PREVIOUS, 'study', poll.THREE), mock.patch.object(BASIS.PREVIOUS, 'CONTROL', poll.CONTROL):
            BASIS.PREVIOUS.Harness.test_foreground_no_early_start_no_replacement_and_exact_previous_closure(self)


def load_tests(loader, tests, pattern):
    suite = loader.loadTestsFromTestCase(Shell)
    suite.addTest(Route('test_poll_only_transitions_reject_old_argv_and_foreign_cleanup'))
    suite.addTests(Harness(name) for name in Harness.__dict__ if name.startswith('test_'))
    suite.addTest(BASIS.PREVIOUS.GUARD.GuardTest('test_wait_witnesses_isolated_app_pid_outside_its_trace'))
    return suite


if __name__ == '__main__':
    unittest.main()
