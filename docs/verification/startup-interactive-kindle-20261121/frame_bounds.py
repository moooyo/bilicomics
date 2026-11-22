"""Bound a single annotated card-to-menu observation without receiving or reading its video."""

import argparse
from fractions import Fraction
import json
import math
import re


EVENTS = {
    'card_up': 'last native-card finger contact : first released frame',
    'shelf': 'last nonmatch : first identifiable expected Shelf with its local hit target visible',
    'probe_down': 'last clear frame : first fresh finger contact on the visible target',
    'probe_up': 'last probe finger contact : first released frame',
    'menu': 'last frame without the expected menu : first frame showing its distinct contents',
}
INTERVALS = {'card_to_shelf_ms': ('card_up', 'shelf'), 'shelf_to_probe_down_ms': ('shelf', 'probe_down'),
             'probe_hold_ms': ('probe_down', 'probe_up'), 'probe_up_to_menu_ms': ('probe_up', 'menu'),
             'card_to_menu_ms': ('card_up', 'menu')}
SOURCES = ('unknown', 'kindle-camera', 'kindle-operator', 'desktop-video', 'synthetic')


def rate(text):
    if not re.fullmatch(r'[0-9]+(?:\.[0-9]+|/[0-9]+)?', text):
        raise ValueError('FPS must be a positive decimal or ratio for a known constant-rate clip')
    try:
        value = Fraction(text)
    except ZeroDivisionError:
        raise ValueError('FPS denominator cannot be zero') from None
    if value <= 0:
        raise ValueError('FPS must be positive')
    return value


def bracket(text, fps):
    parts = text.split(':')
    if fps is None:
        if len(parts) != 2 or not all(re.fullmatch(r'[0-9]+(?:\.[0-9]+)?', item) for item in parts):
            raise ValueError('Use before:after frame PTS in nonnegative decimal milliseconds')
        before, after = map(Fraction, parts)
    else:
        if len(parts) not in (1, 2) or not all(re.fullmatch(r'[0-9]+', item) for item in parts):
            raise ValueError('Use a zero-based frame number or a before:after pair of frame numbers')
        after = int(parts[-1])
        before = int(parts[0]) if len(parts) == 2 else after - 1
        before, after = before * 1000 / fps, after * 1000 / fps
    if not 0 <= before < after:
        raise ValueError('Both observations must exist in the clip with a strictly increasing time')
    return before, after


def elapsed(start, end):
    lower = math.floor((end[0] - start[1]) * 1000)
    upper = math.ceil((end[1] - start[0]) * 1000)
    return [lower / 1000, upper / 1000]


def interpret(args):
    result = {'source_reported': args.source, 'timing_source': None, 'response_reported': args.report_only,
              **dict.fromkeys(INTERVALS), 'menu_vs_probe_up': None, 'reported_kindle_interactive_upper_ms': None,
              'exact_interactive_ms': None, 'software_only_ms': None}
    if args.report_only:
        if any(getattr(args, name) is not None for name in EVENTS):
            raise ValueError('A response-only observation must not include timing landmarks')
        return result
    if not args.single_clip or args.source == 'kindle-operator':
        raise ValueError('Timing requires observations from one uninterrupted clip; use --report-only for an eyewitness')
    fps = rate(args.fps) if args.fps is not None else None
    marks = {}
    for name in EVENTS:
        text = getattr(args, name)
        if text is None:
            raise ValueError(f'Missing --{name.replace("_", "-")} evidence')
        marks[name] = bracket(text, fps)
    for first, following in (('card_up', 'shelf'), ('shelf', 'probe_down'),
                             ('probe_down', 'probe_up'), ('probe_down', 'menu')):
        if marks[first][1] > marks[following][0]:
            raise ValueError(f'{first} must be confirmed before {following}; order is reversed or ambiguous')
    if marks['probe_up'][1] <= marks['menu'][0]:
        result['menu_vs_probe_up'] = 'after-release'
    elif marks['menu'][1] <= marks['probe_up'][0]:
        result['menu_vs_probe_up'] = 'before-release'
    else:
        result['menu_vs_probe_up'] = 'overlapping-brackets'
    for key, (first, last) in INTERVALS.items():
        if key != 'probe_up_to_menu_ms' or result['menu_vs_probe_up'] == 'after-release':
            result[key] = elapsed(marks[first], marks[last])
    result['timing_source'] = 'constant-rate-frames' if fps is not None else 'frame-pts-ms'
    result['response_reported'] = 'observed'
    if args.source == 'kindle-camera':
        result['reported_kindle_interactive_upper_ms'] = result['card_to_menu_ms'][1]
    return result


def parser():
    cli = argparse.ArgumentParser(description=__doc__, epilog='A lone CFR frame means its immediate predecessor was checked and still showed the old state. Never use mean FPS for variable-rate footage or splice separate launches.')
    cli.add_argument('--source', choices=SOURCES, default='unknown', help='Operator-reported provenance; no video or identity is verified')
    cli.add_argument('--single-clip', action='store_true', help='All marked states were observed in one uninterrupted recording of the same launch and window')
    units = cli.add_mutually_exclusive_group(required=True)
    units.add_argument('--fps', help='Known constant frame rate, for example 60 or 30000/1001')
    units.add_argument('--pts-ms', action='store_true', help='Use actual clip-local frame presentation timestamps in milliseconds for every boundary')
    units.add_argument('--report-only', choices=('observed', 'not-observed', 'unknown'), help='Record only whether the expected response was seen; no latency is inferred')
    for name, description in EVENTS.items():
        cli.add_argument('--' + name.replace('_', '-'), help=description)
    return cli


if __name__ == '__main__':
    cli = parser()
    try:
        print(json.dumps(interpret(cli.parse_args()), indent=2, allow_nan=False))
    except ValueError as error:
        cli.error(str(error))
