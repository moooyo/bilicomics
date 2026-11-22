"""Exercise the fixed schedule, fresh archives, strict argv witness and stop rules without a device."""

import argparse
import contextlib
import copy
import importlib.util
import io
import json
import os
from pathlib import Path
import sqlite3
import struct
import subprocess
import tarfile
import tempfile
import unittest
from unittest import mock


SPEC = importlib.util.spec_from_file_location('series', Path(__file__).with_name('series.py'))
SERIES = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(SERIES)
CONTROL = SERIES.CONTROL
OWNER = 'fixture-owner'
BOOT = 'a0a0a0a0-bbbb-cccc-dddd-e0e0e0e0e0e0'


class SeriesTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        for patch in (mock.patch.object(CONTROL, 'HOST', self.root), mock.patch.object(CONTROL, 'identity', return_value=(OWNER, BOOT)),
                      mock.patch.object(CONTROL, 'announce')):
            patch.start()
            self.addCleanup(patch.stop)
        self.plan = SERIES.make_plan(OWNER, BOOT)

    def test_schedule_is_frozen_before_any_launch(self):
        cases = SERIES.check_plan(self.plan, OWNER, BOOT)
        self.assertEqual(''.join(case['arm'] for case in cases), 'AB' + 'ABBA' * 3)
        self.assertEqual([case['pair'] for case in cases], [number for number in range(7) for _arm in range(2)])
        self.assertEqual(len({case['run'] for case in cases}), 14)
        for changes in ({'pair': True}, {'arm': 'B'}, {'run': cases[1]['run'], 'batch': cases[1]['batch']},
                        {'run': '../foreign'}, {'batch': '/tmp/foreign'}):
            altered = copy.deepcopy(self.plan)
            altered['cases'][0].update(changes)
            with self.subTest(changes=changes), self.assertRaises(CONTROL.OperatorError):
                SERIES.check_plan(altered, OWNER, BOOT)
        with self.assertRaises(CONTROL.OperatorError):
            SERIES.check_plan(self.plan, OWNER, 'different-boot')

    def test_actual_process_comparison_refuses_the_other_arm_and_extra_arguments(self):
        source = (SERIES.STUDY / 'host_tools/native_guard.sh').read_bytes()
        stage = '/mnt/us/' + self.plan['cases'][0]['run']
        proc = self.root / 'proc'
        (proc / '125').mkdir(parents=True)
        common = [stage + '/app/bilicomics', '--kindle', '--device', 'scribe', '--data-dir', stage + '/data', '--demo', '--offline',
                  '--shelf-render-trace', stage + '/data/' + self.plan['cases'][0]['run'] + '.jsonl', '--exit-after-ms', '2500']
        env = {'stage': stage, 'app': '125', 'proc': str(proc), 'run': self.plan['cases'][0]['run'], 'PATH': os.defpath}
        for arm in ('A', 'B'):
            guard = SERIES.guard_for_arm(source, arm)
            begin = guard.index(b'''printf '%s\\000' "$stage/app/bilicomics"''')
            fragment = guard[begin:guard.index(b' || return 1', begin)].decode('ascii')
            for extra in ([], ['--serial-shelf-render'], ['--serial-shelf-render', '--offline'], ['--screen', 'shelf']):
                with self.subTest(arm=arm, extra=extra):
                    (proc / '125/cmdline').write_bytes(b'\0'.join(os.fsencode(value) for value in common + extra) + b'\0')
                    result = subprocess.run(['/bin/sh', '-c', fragment], env=env, capture_output=True, timeout=5)
                    self.assertEqual(result.returncode == 0, (arm == 'A' and extra == ['--serial-shelf-render']) or (arm == 'B' and not extra))
        with self.assertRaises(CONTROL.OperatorError):
            SERIES.guard_for_arm(source.replace(SERIES.GUARD_BINDING, b'changed'), 'A')

    def test_offline_preparation_reuses_one_synthetic_seed_and_one_arm_build(self):
        header = bytearray(84)
        header[:7] = b'\x7fELF\x01\x01\x01'
        struct.pack_into('<HH', header, 16, 2, 40)
        struct.pack_into('<I', header, 28, 52)
        struct.pack_into('<I', header, 36, 0x400)
        struct.pack_into('<HH', header, 42, 32, 1)
        struct.pack_into('<I', header, 52, 1)
        for name in ('application', 'worker', 'route-fs'):
            (self.root / name).write_bytes(header + name.encode())
        native = self.root / 'paired-kit.tar.gz'
        names = sorted(CONTROL.REQUIRED)
        with tarfile.open(native, 'x:gz') as archive:
            for name in names:
                member = tarfile.TarInfo(name)
                member.mode, member.size = (0o600 if name.startswith('models/') else 0o700), 1
                archive.addfile(member, io.BytesIO(b'x'))
        native.with_suffix('').with_suffix('.json').write_text(json.dumps({'members': names, 'bytes': len(names)}))
        shipping = SERIES.ROUTE.SOURCE.parents[3] / 'packaging/kindle'
        args = argparse.Namespace(application=self.root / 'application', worker=self.root / 'worker', route_fs=self.root / 'route-fs',
                                  shipping_launcher=shipping / 'launch.sh', document_launcher=shipping / 'document-launcher.sh',
                                  native_kit=native, rollout_root=self.root, output=self.root / 'series')
        with mock.patch.object(CONTROL, 'tools') as tools, mock.patch.object(CONTROL, 'Operator') as live, \
             contextlib.redirect_stdout(io.StringIO()):
            SERIES.prepare(args)
            live.assert_not_called()
            self.assertEqual(tools.return_value.pinned.call_count, 14)
        plan = CONTROL.load(args.output / 'plan.json')
        self.assertEqual({name.name for name in args.output.iterdir()}, {'plan.json'})
        SERIES.ready(SERIES.check_plan(plan, OWNER, BOOT), OWNER, BOOT)
        seeds = set()
        for case in plan['cases']:
            with tarfile.open(Path(case['batch']) / 'route.tar.gz', 'r:gz') as archive:
                self.assertEqual(set(archive.getnames()), CONTROL.FILES - {'run-id'})
                seeds.add(archive.extractfile('data/demo/state.sqlite3').read())
                for source, target in (('application', 'app/bilicomics'), ('worker', 'app/kindle-jpeg-worker'), ('route-fs', 'route-fs')):
                    self.assertEqual(archive.extractfile(target).read(), (self.root / source).read_bytes())
                bindings = SERIES.ROUTE.case_files(args.shipping_launcher.read_bytes(), case['run'], case['arm'] == 'A')
                for name in ('launch.sh', 'switch.sh', 'entry.sh', 'entry-ready.sh'):
                    self.assertEqual(archive.extractfile(name).read(), bindings[name])
        self.assertEqual(len(seeds), 1)
        sample = self.root / 'fixture.sqlite3'
        sample.write_bytes(seeds.pop())
        with contextlib.closing(sqlite3.connect(f'file:{sample}?mode=ro', uri=True)) as database:
            self.assertEqual(database.execute('SELECT * FROM preferences').fetchall(), [('design-demo', 'preferences', '{"launch": "shelf"}')])
        first, second = (Path(case['batch']) for case in plan['cases'][:2])
        (first / 'native-guard.sh').write_bytes((second / 'native-guard.sh').read_bytes())
        with self.assertRaisesRegex(CONTROL.OperatorError, 'witness'):
            SERIES.ready(plan['cases'], OWNER, BOOT)

    def test_each_next_visit_requires_previous_release_and_no_busy_retry(self):
        for refusal in (None, CONTROL.OperatorError('shared lease busy'), CONTROL.RecoveryRequired('recovery'), CONTROL.SampleUnavailable('no sample')):
            events = []

            def create(path):
                def execute(recover):
                    self.assertFalse(recover)
                    self.assertEqual(visit.read_password(), 'fixture')
                    events.append(('attempt', path))
                    if refusal is not None and len(events) == 3:
                        raise refusal

                visit = mock.Mock(execute=execute)
                return visit

            def closed(case, owner, previous):
                self.assertEqual(events[-1], ('attempt', Path(case['batch'])))
                self.assertEqual(owner, OWNER)
                self.assertEqual(previous, None if len(events) == 1 else BOOT)
                events.append(('closed', Path(case['batch'])))
                return BOOT

            with self.subTest(refusal=refusal), mock.patch.object(CONTROL, 'Operator', side_effect=create), \
                 mock.patch.object(SERIES, 'closed_case', side_effect=closed):
                if refusal is None:
                    SERIES.run_cases(self.plan, lambda: 'fixture')
                    self.assertEqual(len(events), 28)
                else:
                    with self.assertRaises(type(refusal)):
                        SERIES.run_cases(self.plan, lambda: 'fixture')
                    self.assertEqual([state for state, _path in events], ['attempt', 'closed', 'attempt'])

    def test_qualified_closure_requires_observed_order_same_boot_natural_exit_and_exact_release(self):
        case = self.plan['cases'][0]
        path = Path(case['batch'])
        path.mkdir(mode=0o700)
        release = {'owner_thread': OWNER, 'run_id': case['run'], 'state': 'released', 'device_disposition': 'original-native-restored',
                   'owner_and_token_checked': True, 'native_restored_and_connections_closed': True}
        sample = {'process_id': 123, 'run_id': case['run'], 'render_order': 'wait_then_render', 'renderer_return_interval_ns': [4, 5]}
        for name, data in (('released.json', release), ('retired.json', {'natural': True, 'app_pid': 123}), ('origin.json', {'boot': BOOT}),
                           ('card.json', {'process_id': 123, 'boot_id': BOOT, 'run_id': case['run']}), ('intervals.json', sample)):
            CONTROL.save(path / name, data)
        trace = path / (case['run'] + '.jsonl')
        frame = {'stage': 'shelf_rendered', 'process_id': 123, 'data': {'render_order': 'wait_then_render', 'shelf_state': 'ready'}}
        trace.write_text(json.dumps(frame) + '\n')
        self.assertEqual(SERIES.closed_case(case, OWNER, None), BOOT)
        self.assertEqual(SERIES.closed_case({**case, 'arm': 'B'}, OWNER, BOOT), BOOT)
        with self.assertRaisesRegex(CONTROL.OperatorError, 'boot'):
            SERIES.closed_case(case, OWNER, 'another-boot')
        for file, field, value in (('intervals.json', 'render_order', 'render_then_wait'), ('intervals.json', 'render_order', None),
                                   ('retired.json', 'natural', False), ('released.json', 'owner_and_token_checked', False)):
            original = (path / file).read_bytes()
            (path / file).write_text(json.dumps({**json.loads(original), field: value}))
            with self.subTest(file=file, field=field, value=value), self.assertRaises(CONTROL.OperatorError):
                SERIES.closed_case(case, OWNER, BOOT)
            (path / file).write_bytes(original)
        for arm, order, state, count, valid in (('A', 'wait_then_render', 'empty', 1, True), ('B', 'render_then_wait', 'ready', 1, True),
                                                ('A', 'initial_fill', 'ready', 1, False), ('B', 'initial_fill', 'empty', 1, False),
                                                ('A', 'wait_then_render', 'signed_out', 1, False), ('B', 'render_then_wait', 'signed_out', 1, False),
                                                ('B', 'wait_then_render', 'empty', 2, False)):
            data = {'render_order': order, 'shelf_state': state}
            (path / 'intervals.json').write_text(json.dumps({**sample, 'render_order': order}))
            trace.write_text((json.dumps({**frame, 'data': data}) + '\n') * count)
            with self.subTest(arm=arm, order=order, state=state, count=count):
                if valid:
                    self.assertEqual(SERIES.closed_case({**case, 'arm': arm}, OWNER, BOOT), BOOT)
                else:
                    with self.assertRaises(CONTROL.OperatorError):
                        SERIES.closed_case({**case, 'arm': arm}, OWNER, BOOT)

    def test_once_only_foreground_prompt_and_no_saved_password(self):
        path = self.root / 'plan.json'
        CONTROL.save(path, self.plan)
        with mock.patch.object(SERIES, 'ready'), mock.patch.object(SERIES.sys.stdin, 'isatty', return_value=False), \
             mock.patch.object(SERIES.getpass, 'getpass') as prompt, mock.patch.object(SERIES, 'run_cases') as run:
            with self.assertRaisesRegex(CONTROL.OperatorError, 'foreground'):
                SERIES.start(path)
            prompt.assert_not_called()
            run.assert_not_called()
        credentials = []

        def capture(plan, read_password):
            self.assertEqual(plan, self.plan)
            self.assertEqual(read_password(), 'fixture-never-written')
            credentials.append(read_password)

        with mock.patch.object(SERIES, 'ready'), mock.patch.object(SERIES.sys.stdin, 'isatty', return_value=True), \
             mock.patch.object(SERIES.getpass, 'getpass', return_value='fixture-never-written') as prompt, \
             mock.patch.object(SERIES, 'run_cases', side_effect=capture):
            SERIES.start(path)
            with self.assertRaisesRegex(CONTROL.OperatorError, 'unattempted'):
                SERIES.start(path)
            prompt.assert_called_once()
        self.assertEqual(credentials[0](), '')
        self.assertNotIn(b'fixture-never-written', b''.join(file.read_bytes() for file in self.root.iterdir()))


if __name__ == '__main__':
    unittest.main()
