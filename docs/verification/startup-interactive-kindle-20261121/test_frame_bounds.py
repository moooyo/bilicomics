"""Synthetic annotations only; no device, camera, media files, or UI are involved."""

import unittest

from frame_bounds import INTERVALS, interpret, parser


def calculate(source='synthetic', fps='25', single=True, **changes):
    marks = dict(card_up='10', shelf='110', probe_down='120', probe_up='122', menu='126')
    marks.update(changes)
    arguments = ['--source', source, '--fps', fps] if fps is not None else ['--source', source, '--pts-ms']
    if single:
        arguments.append('--single-clip')
    for name, value in marks.items():
        if value is not None:
            arguments.append(f'--{name.replace("_", "-")}={value}')
    return interpret(parser().parse_args(arguments))


class FrameBoundsTests(unittest.TestCase):
    def test_constant_rate_separates_human_wait_hold_and_response(self):
        result = calculate()
        self.assertEqual({key: result[key] for key in INTERVALS}, {
            'card_to_shelf_ms': [3960, 4040], 'shelf_to_probe_down_ms': [360, 440], 'probe_hold_ms': [40, 120],
            'probe_up_to_menu_ms': [120, 200], 'card_to_menu_ms': [4600, 4680]})
        self.assertEqual(result['menu_vs_probe_up'], 'after-release')
        self.assertIsNone(result['reported_kindle_interactive_upper_ms'])
        self.assertIsNone(result['exact_interactive_ms'])
        self.assertIsNone(result['software_only_ms'])

    def test_explicit_bounds_and_fractional_fps_round_outward(self):
        result = calculate(fps='30000/1001', card_up='0:1', shelf='2:4')
        self.assertEqual(result['card_to_shelf_ms'], [33.366, 133.467])
        self.assertEqual(result['timing_source'], 'constant-rate-frames')

    def test_variable_rate_uses_each_frame_timestamp(self):
        result = calculate(fps=None, card_up='5.125:17.2', shelf='97.19:119.2074',
                           probe_down='119.2074:160', probe_up='162:201', menu='201:267.4829')
        self.assertEqual(result['card_to_shelf_ms'], [79.99, 114.083])
        self.assertEqual(result['shelf_to_probe_down_ms'][0], 0)
        self.assertEqual(result['card_to_menu_ms'], [183.8, 262.358])
        self.assertEqual(result['timing_source'], 'frame-pts-ms')

    def test_only_device_camera_labels_a_reported_device_bound(self):
        for source in ('unknown', 'desktop-video', 'synthetic'):
            self.assertIsNone(calculate(source=source)['reported_kindle_interactive_upper_ms'])
        result = calculate(source='kindle-camera')
        self.assertEqual(result['reported_kindle_interactive_upper_ms'], result['card_to_menu_ms'][1])
        with self.assertRaises(ValueError):
            calculate(source='kindle-operator')

    def test_timing_needs_one_clip_and_all_five_events(self):
        with self.assertRaises(ValueError):
            calculate(single=False)
        with self.assertRaises(ValueError):
            calculate(probe_up=None)

    def test_early_input_and_preexisting_or_ambiguous_menu_are_rejected(self):
        for bad in ({'shelf': '8'}, {'probe_down': '109'}, {'probe_down': '109:111'},
                    {'probe_up': '120'}, {'menu': '119'}, {'menu': '120'}, {'menu': '119:123'}):
            with self.subTest(bad=bad), self.assertRaises(ValueError):
                calculate(**bad)

    def test_menu_in_same_frame_as_release_still_confirms_response(self):
        result = calculate(source='kindle-camera', fps='30', probe_up='126')
        self.assertEqual(result['menu_vs_probe_up'], 'overlapping-brackets')
        self.assertIsNone(result['probe_up_to_menu_ms'])
        self.assertEqual(result['card_to_menu_ms'], [3833.333, 3900])
        self.assertEqual(result['reported_kindle_interactive_upper_ms'], 3900)

    def test_menu_visible_before_release_still_confirms_response(self):
        result = calculate(source='kindle-camera', probe_up='130', menu='125')
        self.assertEqual(result['menu_vs_probe_up'], 'before-release')
        self.assertIsNone(result['probe_up_to_menu_ms'])
        self.assertEqual(result['card_to_menu_ms'], [4560, 4640])
        self.assertEqual(result['reported_kindle_interactive_upper_ms'], 4640)

    def test_invalid_fps_or_frame_numbers_are_rejected(self):
        for fps in ('0', '120/0', 'NaN', '29.9/2', '-1'):
            with self.subTest(fps=fps), self.assertRaises(ValueError):
                calculate(fps=fps)
        for value in ('0', '1:1', '3:2', '1.5', '0:1:2'):
            with self.subTest(value=value), self.assertRaises(ValueError):
                calculate(card_up=value)

    def test_variable_rate_requires_finite_increasing_brackets(self):
        for value in ('16', '1:1', '3:2', '-0.01:1', '0:NaN', '0:inf'):
            with self.subTest(value=value), self.assertRaises(ValueError):
                calculate(fps=None, card_up=value, shelf='50:60', probe_down='70:80', probe_up='90:100', menu='110:120')

    def test_eyewitness_has_no_invented_times_or_device_identity(self):
        for status in ('observed', 'not-observed', 'unknown'):
            result = interpret(parser().parse_args(['--report-only', status]))
            self.assertEqual((result['response_reported'], result['source_reported']), (status, 'unknown'))
            self.assertTrue(all(result[key] is None for key in (*INTERVALS, 'reported_kindle_interactive_upper_ms',
                                                                  'exact_interactive_ms', 'software_only_ms', 'timing_source',
                                                                  'menu_vs_probe_up')))
        with self.assertRaises(ValueError):
            interpret(parser().parse_args(['--report-only', 'observed', '--menu', '5']))


if __name__ == '__main__':
    unittest.main()
