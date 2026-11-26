"""Exercise the bounded phase parser and the reversible ordinary route on Linux."""

import copy
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest import mock

import native_visit as native
import project_phases as phase


RUN = 'bili-shelf-' + 'a' * 32
BOOT = '1c6a9c92-a27e-4e96-966b-05c7bad70cc2'
PID = 401
BASE = '40:2:41ed:0:1/40:3:41ed:0:1/40:4:81a4:200:1'
VOLUME = '7:0:61:' + 'b' * 32
SEED = ['account/private/session.json', *[f'covers/{i:064x}.png' for i in range(20)], 'state.sqlite3']
RUNTIME = ('accounts', 'accounts/' + 'a' * 64, 'accounts/' + 'a' * 64 + '/pages')
ROOT = Path(__file__).resolve().parents[3]


def timing(start, end, cpu=None):
    return {'start_ns': start, 'end_ns': end, 'cpu_ns': cpu}


def span(name, start, end, outcome='ok'):
    return {'kind': 'span', 'stage': name, 'outcome': outcome, **timing(start, end, 2)}


def frame(sequence, start, end, *, initial=False, stages, preceding=None, accepted=None, complete=None):
    return {'kind': 'setup' if sequence == 0 else 'frame', 'sequence': sequence, 'timing': timing(start, end, 2),
            'initial': initial, 'rendered': 'render' in stages, 'complete_replacement': complete is not None,
            'preceding_marker': preceding, 'accepted_marker': accepted, 'completed_marker': complete,
            'accepted': True if accepted is not None else None,
            'stages': {name: stages.get(name) for name in phase.FRAME_STAGES}}


def example():
    marks = [10, 55, 80, 85, 180, 185, 90, 187, 225, 228, 245, 380]
    stages = [{'kind': 'mark', 'stage': name, 'ns': clock} for name, clock in zip(phase.MARKS, marks)]
    stages.extend(span(name, 12 + index * 10, 16 + index * 10) for index, name in enumerate(phase.SETUP))
    stages.extend([span('jpeg_capability', 90, 100, 'available'), span('metadata', 101, 178),
                   span('session_load', 111, 113, 'present'), span('state-covers', 190, 218),
                   span('font_prepare', 82, 88), span('font_registration', 160, 162),
                   span('font_registration', 200, 202), span('font_finish', 203, 206), span('app_attach', 230, 242)])
    stages.extend(span(name, 104 + index * 6, 108 + index * 6) for index, name in enumerate(phase.METADATA))
    stages.extend([span('attach-covers', 251, 256, 'partial'), span('attach-covers', 323, 329, 'success')])
    stages.extend(span(name, 333 + index * 8, 338 + index * 8, 'success') for index, name in enumerate(phase.ATTACH[1:]))
    frames = [frame(0, 60, 67, stages={name: timing(61 + index, 62 + index)
               for index, name in enumerate(('active_io', 'buffer_setup', 'window_setup', 'portrait_observer'))}),
              frame(1, 68, 87, initial=True, stages={'initial_fill': timing(69, 72), 'render': timing(73, 78),
                    'write': timing(79, 81), 'submit': timing(82, 84)}, accepted=7),
              frame(2, 260, 320, stages={'preceding_wait': timing(261, 293, 1), 'render': timing(294, 307),
                    'write': timing(308, 315), 'submit': timing(316, 318)}, preceding=7, accepted=8),
              frame(3, 384, 520, stages={'render': timing(385, 440, 40), 'preceding_wait': timing(441, 447, 1),
                    'write': timing(450, 455, 3), 'submit': timing(457, 460, 1), 'completion_wait': timing(462, 517, 2)},
                    preceding=8, accepted=9, complete=9)]
    return {'schema': 'kindle-startup-phases-v1', 'clock': phase.CLOCK, 'run_id': RUN, 'boot_id': BOOT, 'process_id': PID,
            'shelf_render_ns': 439, 'stages': stages, 'frames': frames}


def projection(data, **options):
    return phase.project(data, **{'run': RUN, 'boot': BOOT, 'pid': PID, 'renderer_ns': 439,
            'render_order': 'render_then_wait', 'send': (9, 459), 'completed': (9, 518), **options})


class PhaseTests(unittest.TestCase):
    def test_attachment_intervals_are_disjoint_and_shelf_wait_is_after_renderer(self):
        result = projection(example())
        times = result['intervals_ns']
        self.assertEqual(times['attached_to_ready'], 135)
        self.assertEqual(times['attached_loading_display_wait'], 32)
        self.assertEqual(times['attached_loading_frame_other'], 28)
        self.assertEqual(times['attached_unattributed_gap'] + result['attach_work']['wall_ns'] + 32 + 28, 135)
        self.assertEqual(result['attach_work']['count'], 7)
        self.assertEqual(result['shelf_frame_work']['preceding_wait']['wall_ns'], 6)
        self.assertEqual(times['shelf_mapping_wait_before_draw'], 0)
        self.assertEqual(times['shelf_draw_to_renderer_return'], 54)
        self.assertEqual(result['application_work']['font_registration']['count'], 2)

    def test_loading_wait_overlap_requires_its_own_completed_marker(self):
        data = example()
        first = data['frames'][1]
        first['timing']['end_ns'] = 189
        first['completed_marker'] = first['accepted_marker']
        first['stages']['completion_wait'] = timing(84, 186, 1)
        for row in data['stages']:
            if row['stage'] == 'font_registration' and row['start_ns'] == 160:
                row.update(start_ns=193, end_ns=195)
        result = projection(data)
        self.assertEqual(result['intervals_ns']['jpeg_overlap_with_first_loading_completion_wait'], 10)
        self.assertEqual(result['intervals_ns']['metadata_overlap_with_first_loading_completion_wait'], 77)
        self.assertEqual(result['intervals_ns']['cover_gate_overlap_with_first_loading_completion_wait'], 1)
        self.assertEqual(result['display_witness'], {'first_loading_completion_matches_send': True,
                         'shelf_preceding_marker_matches_last_loading_send': True})
        first['completed_marker'] = 6
        self.assertIsNone(projection(data)['intervals_ns']['cover_gate_overlap_with_first_loading_completion_wait'])

    def test_early_gate_and_overlap_are_bound_to_the_actual_loading_send(self):
        data = example()
        first = data['frames'][1]
        first['timing']['end_ns'] = 205
        first['completed_marker'] = first['accepted_marker']
        first['stages']['completion_wait'] = timing(86, 204, 1)
        for row in data['stages']:
            if row['stage'] == 'font_registration':
                delta = 46 if row['start_ns'] == 160 else 9
                row['start_ns'] += delta
                row['end_ns'] += delta
            if row['stage'] == 'font_finish':
                row.update(start_ns=212, end_ns=215)
            if row['stage'] == 'cover_gate_open':
                row['ns'] = 207
        send = {'kind': 'mark', 'stage': 'loading_send_accepted', 'ns': 84}
        winner = {'kind': 'mark', 'stage': 'cover_gate_from_send', 'ns': 85}
        data['stages'].extend([send, winner])
        result = projection(data)
        self.assertEqual(result['cover_gate_source'], 'send')
        self.assertEqual(result['intervals_ns']['cover_decode_overlap_with_first_loading_completion_wait'], 14)
        for replacement in (None, {**send, 'ns': 83}, {**send, 'ns': 205}):
            with self.subTest(replacement=replacement), self.assertRaises(ValueError):
                mutated = copy.deepcopy(data)
                mutated['stages'].remove(send)
                if replacement is not None:
                    mutated['stages'].append(replacement)
                projection(mutated)
        data['stages'].append({'kind': 'mark', 'stage': 'cover_gate_from_ui', 'ns': 205})
        with self.assertRaises(ValueError):
            projection(data)

    def test_wait_first_shelf_keeps_its_mapping_barrier_before_draw(self):
        data = example()
        data['frames'][-1]['stages'].update(render=timing(400, 440, 40), preceding_wait=timing(385, 399, 1))
        times = projection(data, render_order='wait_then_render')['intervals_ns']
        self.assertEqual(times['shelf_mapping_wait_before_draw'], 14)
        self.assertEqual(times['shelf_draw_to_renderer_return'], 39)

    def test_gate_records_from_distinct_threads_can_be_reversed_in_storage_order(self):
        data = example()
        for row in data['stages']:
            if row['stage'] == 'cover_gate_open':
                row['ns'] = 189
        data['stages'].reverse()
        self.assertEqual(projection(data)['intervals_ns']['cover_gate_wait'], 2)

    def test_unavailable_driver_evidence_is_not_synthesized(self):
        data = example()
        data['frames'][-1]['completed_marker'] = None
        data['frames'][-1]['stages']['completion_wait'] = None
        result = projection(data, completed=None)
        self.assertNotIn('completion_wait', result['shelf_frame_work'])
        self.assertEqual(result['intervals_ns']['main_to_renderer_return'], 429)

    def test_reject_partial_or_private_records_and_bad_bindings(self):
        for payload in (b'{"a":1,"a":2}\n', b'{"x":NaN}\n', b'{}', b'{}\n{}\n', b' ' * phase.LIMIT + b'\n'):
            with self.subTest(payload=payload[:20]), self.assertRaises(ValueError):
                phase.decode(payload)
        for options in ({'pid': 402}, {'send': (10, 459)}, {'completed': (10, 518)}, {'render_order': 'initial_fill'},
                        {'render_order': 'wait_then_render'}, {'renderer_ns': 440}):
            with self.subTest(**options), self.assertRaises(ValueError):
                projection(example(), **options)
        wrong_boot = example()
        wrong_boot['boot_id'] = '0' * 36
        broken = example()
        broken['stages'].pop(0)
        unsafe = example()
        unsafe['stages'][-1]['extra'] = 'unexpected'
        sequence = example()
        sequence['frames'][2]['sequence'] = 8
        overlap = example()
        overlap['stages'][-6].update(start_ns=267, end_ns=270)
        for record in (wrong_boot, broken, unsafe, sequence, overlap):
            with self.assertRaises(ValueError):
                projection(record)

    def test_pick_marks_match_the_application_and_preserve_the_numeric_sidecar(self):
        raw = b'picked 125 391\nshim_entry 125 4\napp_preinvoke 401 9\n' + json.dumps(example()).encode() + b'\n'
        times, data = native.parse_sidecar(raw, PID)
        self.assertEqual(times, {'launch_entry': 4, 'pre_app_invocation': 9})
        self.assertEqual(data, example())
        with self.assertRaises(ValueError):
            native.parse_sidecar(raw.replace(b'shim_entry 125', b'shim_entry 401'), PID)
        with self.assertRaises(ValueError):
            native.parse_sidecar(raw, 402)


route_test = native.CLOCK.BASE.load_module(native.CLOCK.BASE.STUDY / 'route/test_route.py')


class OrdinaryRouteTest(route_test.RouteTest):
    def setUp(self):
        super().setUp()
        with (self.home / 'app/bilicomics').open('r+b') as app:
            app.truncate(native.OVERLAP.PRODUCTION_BYTES)
        (self.stage / 'data/demo/state.sqlite3').unlink()
        (self.stage / 'data/demo').rmdir()
        self.doc.write_bytes(native.OVERLAP.ICON_DOCUMENT)
        (self.stage / 'document-launcher.sh').write_bytes(native.OVERLAP.ICON_DOCUMENT)
        self.suffix = self.stage / 'data/account/private/.fixture-owner'
        self.mountinfo = self.root / 'system/mountinfo'
        self.mountinfo.write_text('1 0 0:1 / / rw - tmpfs root rw\n')
        stock = self.shipping
        launcher = native.OVERLAP.overlap_launcher(stock)
        self.files = native.route_files(stock, launcher, RUN, SEED, RUNTIME)
        for leaf in SEED:
            out = self.stage / 'data' / leaf
            out.parent.mkdir(exist_ok=True, parents=True)
            out.write_bytes(b'isolated fixture content')
        self.shim = self.files['entry.sh'].replace(b'/mnt/us', str(self.media).encode())
        for leaf, content in self.files.items():
            path = self.stage / leaf
            if leaf == 'switch.sh':
                for source, target in (('/mnt/us', self.media), ('/etc/prettyversion.txt', self.root / 'system/firmware'),
                        ('/sys/class/graphics/fb0/bits_per_pixel', self.root / 'system/bpp'),
                        ('/sys/class/graphics/fb0/virtual_size', self.root / 'system/size'),
                        ('-c /dev/fb0', f'-f "{self.root}/system/framebuffer"'),
                        ('/proc/self/mountinfo', self.mountinfo)):
                    content = content.replace(source.encode(), str(target).encode())
            elif leaf in ('entry.sh', 'entry-ready.sh', 'launch.sh'):
                for source, target in (('/mnt/us', self.media), ('/etc/prettyversion.txt', self.root / 'system/firmware'),
                        ('/sys/class/graphics/fb0/bits_per_pixel', self.root / 'system/bpp'),
                        ('/sys/class/graphics/fb0/virtual_size', self.root / 'system/size'),
                        ('-c /dev/fb0', f'-f "{self.root}/system/framebuffer"')):
                    content = content.replace(source.encode(), str(target).encode())
            path.write_bytes(content)
        (self.stage / 'clock-marks').touch(mode=0o700)

    def stage_mount(self):
        self.suffix.write_bytes(b'0' * 50)
        self.suffix.chmod(0o600)
        with self.mountinfo.open('a') as out:
            out.write(f'7 1 0:61 / {self.suffix.parent} ro,nosuid,nodev,noexec - tmpfs bili-private-{RUN[11:]}-0000000000000000 rw,size=64k\n')

    def test_offline_route_requires_mount_and_retains_original_files(self):
        self.invoke('inspect', 'stock')
        self.invoke('activate')
        self.stage_mount()
        self.invoke('activate', 'armed')
        self.invoke('restore', 'stock')
        self.invoke('cleanup')
        self.suffix.unlink()
        self.mountinfo.write_text('1 0 0:1 / / rw - tmpfs root rw\n')
        run_outputs = [f'{RUN}.startup-phases.json', f'{RUN}.jsonl', 'state.sqlite3-wal', 'state.sqlite3-shm']
        for name in run_outputs:
            (self.stage / 'data' / name).write_bytes(b'synthetic')
        (self.stage / 'data' / RUNTIME[-1]).mkdir(parents=True)
        self.invoke('cleanup', 'removed')
        self.assertEqual(self.stock.stat().st_ino, self.stock_inode)
        self.assertEqual(self.stock.read_bytes(), self.shipping)
        self.assertEqual(self.doc.stat().st_ino, self.doc_before.st_ino)

    def test_unlisted_cover_and_argv_tamper_are_rejected_before_publication(self):
        self.stage_mount()
        unknown = self.stage / 'data/covers/unknown.png'
        unknown.touch()
        self.invoke('activate')
        unknown.unlink()
        launcher = self.stage / 'launch.sh'
        launcher.write_bytes(launcher.read_bytes().replace(b' --offline ', b' --demo --offline '))
        self.invoke('activate')
        self.assertFalse(self.journal.exists())
        self.assertEqual(self.stock.stat().st_ino, self.stock_inode)

    def test_guard_and_collection_share_exact_arguments_and_names(self):
        for source in (native.guard(), native.sidecar_command(RUN, '/mnt/us/' + RUN, BOOT).encode(), *self.files.values()):
            if source.startswith((b'#!/bin/sh', b'test ')):
                subprocess.run(['sh', '-n'], input=source, check=True)
        self.assertNotIn(b'--demo --offline', self.files['launch.sh'])
        self.assertNotIn(b'--screen shelf', self.files['launch.sh'])
        self.assertIn(b'--startup-phases', self.files['launch.sh'])
        self.assertIn(b'--startup-phases', native.guard())

    @classmethod
    def suite(cls):
        return unittest.TestSuite(cls(name) for name in ('test_offline_route_requires_mount_and_retains_original_files',
            'test_unlisted_cover_and_argv_tamper_are_rejected_before_publication', 'test_guard_and_collection_share_exact_arguments_and_names'))


class NavigationTests(unittest.TestCase):
    def setUp(self):
        self.visit = object.__new__(native.Visit)
        self.visit.shared = mock.Mock()
        self.visit.shared.paired.selected.side_effect = lambda receipt: receipt['selected']
        self.visit.tab_dispatch = 'not-requested'
        self.visit.probe_step = None
        self.busy = native.CORE.OperatorError('Scoped SSH failed (1; native-not-idle)')
        self.home = {'selected': 'home'}
        self.library = {'selected': 'library'}
        self.receipt = {'target': 'nav-library', 'ok': True, 'buttons_clear_after': True}

    def use(self, responses):
        remaining = iter(responses)

        def probe(mode):
            target, step, response = next(remaining)
            self.assertEqual(mode, target)
            self.visit.probe_step = step
            if isinstance(response, Exception):
                raise response
            return response

        self.visit.probe = mock.Mock(side_effect=probe)

    def test_confirmed_tab_change_only_retries_the_read_only_idle_guard(self):
        self.use([('navigation', '', self.home), ('nav-library', '', self.receipt),
                  ('navigation', 'native-guard', self.busy), ('navigation', '', self.library)])
        with mock.patch.object(native.time, 'sleep'):
            self.assertEqual(self.visit.ensure_tab('library'), self.library)
        self.assertEqual(self.visit.probe.call_args_list.count(mock.call('nav-library')), 1)

    def test_never_retries_an_initial_busy_guard_or_ambiguous_request(self):
        for responses in ([('navigation', 'native-guard', self.busy)],
                          [('navigation', '', self.home), ('nav-library', 'operation-request', self.busy)],
                          [('navigation', '', self.home), ('nav-library', '', self.receipt),
                           ('navigation', 'operation-request', self.busy)]):
            with self.subTest(responses=responses), mock.patch.object(native.time, 'sleep'):
                self.use(responses)
                with self.assertRaises(native.CORE.OperatorError):
                    self.visit.ensure_tab('library')
                self.assertEqual(self.visit.probe.call_count, len(responses))

    def test_idle_wait_remains_bounded_and_never_resends_the_tab_click(self):
        self.use([('navigation', '', self.home), ('nav-library', '', self.receipt),
                  *[('navigation', 'native-guard', self.busy)] * 6])
        with mock.patch.object(native.time, 'sleep'), self.assertRaisesRegex(native.CORE.OperatorError, 'Native tab not confirmed'):
            self.visit.ensure_tab('library')
        self.assertEqual(self.visit.probe.call_count, 8)
        self.assertEqual(self.visit.probe.call_args_list.count(mock.call('nav-library')), 1)


class PrivateMountTest(unittest.TestCase):
    def test_cleanup_can_recover_a_lost_mount_reply_before_route_activation(self):
        with tempfile.TemporaryDirectory() as work:
            visit = object.__new__(native.Visit)
            visit.path, visit.run, visit.boot = Path(work), RUN, BOOT
            visit.guard = mock.Mock(return_value=BOOT)
            visit.no_staged_mounts = mock.Mock()
            owner = 'a' * 32
            native.CORE.save(visit.path / native.MOUNT_INTENT,
                             {'run': RUN, 'boot': BOOT, 'owner': owner, 'base': BASE})
            native.CORE.flag(visit.path / 'stock-restored')
            native.CORE.flag(visit.path / 'navigation-restored')
            visit.mount_call = mock.Mock(side_effect=[f'mounted={VOLUME}\n'.encode(), b'restored\n', f'base={BASE}\n'.encode()])
            visit.detach()
            self.assertEqual(visit.mount_call.call_args_list, [mock.call('inspect-mounted', owner),
                             mock.call('detach', owner, VOLUME, BASE), mock.call('inspect-base', owner)])
            self.assertTrue(native.COLLECT.flag_present(visit.path, native.MOUNT_RESTORED))
            visit.no_staged_mounts.assert_called_once()

    def test_exact_owner_can_detach_its_read_only_session_and_restore_the_base(self):
        # The caller runs this fixture in a separate private mount namespace.
        self.assertEqual(os.environ.get('BILI_FIXTURE_MOUNT_ISOLATED'), 'yes')
        self.assertEqual(os.geteuid(), 0)
        with tempfile.TemporaryDirectory(prefix='bili-synthetic-mount-') as temporary:
            media = Path(temporary).resolve()
            stage = media / RUN
            account = stage / 'data/account/private'
            account.mkdir(parents=True, mode=0o700)
            (stage / 'run-id').write_text(RUN + '\n')
            base_file = account / 'session.json'
            base_file.write_text('fictional-mount-fixture\n')
            before = base_file.stat().st_ino
            source = (native.HERE / native.MOUNT).read_bytes().replace(b'media=/mnt/us', b'media=' + os.fsencode(media))
            boot = Path('/proc/sys/kernel/random/boot_id').read_text().strip()
            owner = 'a' * 32

            def run(action, *args, who=owner, okay=True):
                result = subprocess.run(['sh', '-s', '--', RUN, boot, action, who, *args], input=source, capture_output=True, timeout=12)
                if okay:
                    self.assertEqual(result.returncode, 0, result.stderr)
                else:
                    self.assertNotEqual(result.returncode, 0)
                return result.stdout

            proof = native.receipt(run('inspect-base'), 'base', native.BASE)
            run('attach', BASE, okay=False)
            (stage / 'armed').mkdir()
            run('attach', proof, okay=False)
            (stage / 'armed').rmdir()
            token = native.receipt(run('attach', proof), 'mounted', native.VOLUME)
            try:
                self.assertEqual(native.receipt(run('inspect-ready'), 'mounted', native.VOLUME), token)
                self.assertEqual(base_file.read_text(), 'fictional-mount-fixture\n')
                self.assertEqual(base_file.stat().st_mode & 0o777, 0o600)
                self.assertIn(' ro,', next(line for line in Path('/proc/self/mountinfo').read_text().splitlines() if str(account) in line))
                run('detach', token, proof, who='c' * 32, okay=False)
                run('detach', VOLUME, proof, okay=False)
            finally:
                self.assertEqual(run('detach', token, proof), b'restored\n')
            self.assertEqual(native.receipt(run('inspect-base'), 'base', native.BASE), proof)
            self.assertEqual(base_file.stat().st_ino, before)


if __name__ == '__main__':
    suites = [unittest.defaultTestLoader.loadTestsFromTestCase(PhaseTests), OrdinaryRouteTest.suite(),
              unittest.defaultTestLoader.loadTestsFromTestCase(NavigationTests)]
    if os.environ.get('BILI_FIXTURE_MOUNT_ISOLATED') == 'yes':
        suites.append(unittest.defaultTestLoader.loadTestsFromTestCase(PrivateMountTest))
    raise SystemExit(not unittest.TextTestRunner(verbosity=2).run(unittest.TestSuite(suites)).wasSuccessful())
