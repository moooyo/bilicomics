"""Exercise the exact installed-card gate and both staged launcher treatments."""

import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest import mock

import compare


route_tests = compare.BASE.load_module(compare.BASE.STUDY / 'route/test_route.py')
ROOT = Path(__file__).resolve().parents[3]


class IconCardRoute(route_tests.RouteTest):
    def setUp(self):
        super().setUp()
        self.switch.write_bytes(compare.current_stock_guard(self.switch.read_bytes()))
        with (self.home / 'app/bilicomics').open('r+b') as app:
            app.truncate(compare.PRODUCTION_BYTES)
        self.doc.write_bytes(compare.ICON_DOCUMENT)
        (self.stage / 'document-launcher.sh').write_bytes(compare.ICON_DOCUMENT)
        self.sidecar = self.doc.with_name(self.doc.name + '.sdr')

    def test_icon_card_and_owned_metadata_survive_activation_restore_and_cleanup(self):
        self.sidecar.mkdir()
        contents = self.sidecar / 'synthetic-metadata'
        contents.write_bytes(b'Not application data')
        identity = self.sidecar.stat().st_ino
        self.invoke('inspect', 'stock')
        self.invoke('activate', 'armed')
        self.invoke('restore', 'stock')
        self.invoke('cleanup', 'removed')
        self.assertEqual(self.stock.stat().st_ino, self.stock_inode)
        self.assertEqual(self.stock.read_bytes(), self.shipping)
        self.assertEqual(self.doc.stat().st_ino, self.doc_before.st_ino)
        self.assertEqual(self.doc.read_bytes(), compare.ICON_DOCUMENT)
        self.assertEqual(self.sidecar.stat().st_ino, identity)
        self.assertEqual(contents.read_bytes(), b'Not application data')

    def test_alternate_card_wrong_bytes_and_untrusted_sidecars_are_refused(self):
        for path, kind in (
            (self.doc.with_name('BiliComics.sh'), 'card'),
            (self.sidecar, 'file'),
            (self.sidecar, 'symlink'),
            (self.doc.with_name('BiliComics.sh.SDR'), 'dir'),
            (self.doc.with_name('bilicomics.sdr'), 'dir'),
        ):
            with self.subTest(path=path.name, kind=kind):
                if kind == 'card':
                    path.write_bytes(compare.ICON_DOCUMENT)
                elif kind == 'file':
                    path.touch()
                elif kind == 'symlink':
                    path.symlink_to(self.root / 'system', target_is_directory=True)
                else:
                    path.mkdir()
                self.invoke('activate')
                self.assertFalse(self.journal.exists())
                if kind == 'dir':
                    path.rmdir()
                else:
                    path.unlink()
        self.doc.write_bytes(compare.ROUTE.DOCUMENT)
        self.invoke('activate')
        self.assertFalse(self.journal.exists())
        self.assertEqual(self.stock.read_bytes(), self.shipping)


class TreatmentTests(unittest.TestCase):
    def test_paired_gate_excludes_warmup_and_uses_matching_endpoints(self):
        def observation(case):
            value = (6000 - (125 if case['arm'] == 'B' else 0)) * 1_000_000
            row = {'pair': case['pair'], 'arm': case['arm'],
                   'phase_durations': {'release_to_shim_ns': [400, 402]},
                   'launcher_uptime_intervals_ms': {'cover_probe': 100},
                   'probe_finished_before_minimum_settle_deadline': case['arm'] == 'B',
                   'custom_sleep_cover_eligible': True}
            for i, key in enumerate(compare.ENDPOINTS):
                row[key] = [value + i * 30_000_000, value + i * 30_000_000 + 2_000_000]
            return 'boot', row

        cases = [{'pair': pair, 'arm': arm, 'batch': '/unused'} for pair, arm in compare.SCHEDULE]
        with mock.patch.object(compare, 'observation', side_effect=observation):
            result = compare.report(cases)
        self.assertEqual(len(result['visits']), 14)
        self.assertEqual(len(result['pairs']), 6)
        self.assertTrue(result['first_shelf_gate_passed'] and result['all_six_candidate_probes_overlapped'])
        self.assertTrue(result['all_six_cover_eligibility_outcomes_matched'])
        self.assertEqual(result['summary'][compare.ENDPOINTS[0]]['paired_median_b_minus_a_ns'], [-127_000_000, -123_000_000])

    def test_only_explicit_pre_press_refusal_can_restore_without_retirement(self):
        with tempfile.TemporaryDirectory() as work:
            visit = object.__new__(compare.Visit)
            visit.path = Path(work)
            visit.route = '/mnt/us/' + route_tests.RUN
            visit.boot = '1c6a9c92-a27e-4e96-966b-05c7bad70cc2'
            visit.guard = mock.Mock(return_value=visit.boot)
            visit.raw = mock.Mock(return_value=b'')
            with mock.patch.object(compare.CONTROL.Operator, 'probe', return_value=compare.CARD_REFUSAL):
                with self.assertRaises(compare.CONTROL.OperatorError):
                    visit.probe('open-card')
            refusal = visit.path / 'card-refused.json'
            self.assertEqual(json.loads(refusal.read_bytes()), compare.CARD_REFUSAL)
            with mock.patch.object(compare.CONTROL.Operator, 'retire') as original:
                visit.retire()
                original.assert_not_called()
                visit.raw.assert_called_once_with(f'test ! -e {visit.route}/picked && test ! -L {visit.route}/picked')
                refusal.unlink()
                visit.retire()
                original.assert_called_once_with()

    def test_exact_card_reference_and_shell_treatments(self):
        shipping = (ROOT / 'packaging/kindle/launch.sh').read_bytes()
        card = (ROOT / 'packaging/kindle/document-launcher.sh').read_bytes()
        self.assertEqual(compare.ICON_DOCUMENT, card)
        for optimized in (False, True):
            with self.subTest(optimized=optimized):
                treatment = compare.overlap_launcher(shipping) if optimized else shipping
                files = compare.route_for(shipping, treatment, route_tests.RUN)
                for name in ('entry.sh', 'launch.sh', 'switch.sh'):
                    subprocess.run(['sh', '-n'], input=files[name], check=True)
                script = files['launch.sh']
                begin = script.index(b'launch_trace study_cover_begin\n')
                settled = script.index(b'launch_trace study_observer_settled\n')
                self.assertEqual(begin < settled, optimized)
                self.assertIn(script, files['switch.sh'])
                self.assertIn(files['entry.sh'], files['switch.sh'])


def suite():
    names = ('test_icon_card_and_owned_metadata_survive_activation_restore_and_cleanup',
             'test_alternate_card_wrong_bytes_and_untrusted_sidecars_are_refused')
    return unittest.TestSuite([*(IconCardRoute(name) for name in names),
                               unittest.defaultTestLoader.loadTestsFromTestCase(TreatmentTests)])


if __name__ == '__main__':
    raise SystemExit(not unittest.TextTestRunner(verbosity=2).run(suite()).wasSuccessful())
