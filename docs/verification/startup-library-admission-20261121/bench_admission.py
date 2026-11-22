"""Measure Library entry to verified worker preflight on Linux; never start a device application."""

import argparse
import json
import os
from pathlib import Path
import select
import shutil
import statistics
import subprocess
import tempfile
import time


BOUNDARY = '# Do not remove another launcher\'s lock or attempt to repair its service state.\n'
LOCK_LINE = 'mkdir "$LOCK_DIR" 2>/dev/null || fail \'Another launcher owns the display lock.\'\n'
ORDERS = ('AB', 'BA', 'AB', 'BA', 'AB', 'BA')
KINDS = ('profile', 'firmware', 'allowlist', 'prepare_dirs')
RECEIPT = '''[ "$#" = 1 ] && [ "$1" = --detached ] || exit 93
printf '%s %s %s %s\\n' "$$" "$PPID" "$BILICOMICS_APPROVED_PROFILE" "$BILICOMICS_APPROVED_FIRMWARE" > "$BENCH_FIFO"
exit 0
'''


def require(condition, message):
    if not condition:
        raise ValueError(message)


def launcher_prefix(source):
    require(source.startswith('#!/bin/sh\n') and source.count(BOUNDARY + LOCK_LINE) == 1,
            'Expected one known detached-worker pre-lock boundary')
    prefix = source.partition(BOUNDARY + LOCK_LINE)[0]
    require(prefix.count('setsid sh "$SCRIPT_PATH" --detached') == 2,
            'Expected the real Library parent and default launch paths')
    for original, replacement, occurrences in (
        ('/etc/prettyversion.txt', '"$BENCH_FIRMWARE"', 2),
        ('/dev/fb0', '/dev/null', 3),
        ('/tmp/bilicomics-launcher.lock', '"$BENCH_LOCK"', 1),
    ):
        require(prefix.count(original) == occurrences, 'Unrecognized fixture path binding: ' + original)
        prefix = prefix.replace(original, replacement)
    return prefix + RECEIPT


def executable(path, body):
    path.write_text('#!/bin/sh\nset -eu\n' + body + '\n', encoding='ascii')
    path.chmod(0o700)


def counter_tools(directory):
    directory.mkdir()
    executable(directory / 'sh', '''[ "$#" = 2 ] && [ "$1" = "$BILICOMICS_HOME/launch.sh" ] && [ "$2" = --detached ] || exit 94
printf 'worker entered\\n' >> "$BENCH_COUNTS"
BENCH_ROLE=worker; export BENCH_ROLE
exec /bin/sh "$@"''')
    patterns = {
        'sed': '''case "$last" in
    "$BILICOMICS_HOME/data/device-profile") kind=profile;;
    "$BENCH_FIRMWARE") kind=firmware;;
    *) exit 94;; esac''',
        'awk': '''[ "$last" = "$BILICOMICS_HOME/data/device-allowlist" ] || exit 94
kind=allowlist''',
        'mkdir': '''[ "$#" = 3 ] && [ "$1" = -p ] && [ "$2" = "$BILICOMICS_HOME/data/logs" ] && [ "$3" = "$BILICOMICS_HOME/data/fonts" ] || exit 94
kind=prepare_dirs''',
    }
    for name, pattern in patterns.items():
        tool = shutil.which(name, path=os.defpath)
        require(tool is not None, 'Missing system tool: ' + name)
        executable(directory / name, 'for last do :; done\n' + pattern + '\n'
                   + 'printf \'%s %s\\n\' "$BENCH_ROLE" "$kind" >> "$BENCH_COUNTS"\n'
                   + f'exec "{tool}" "$@"')


class Fixture:
    def __init__(self, root):
        self.home = root / 'home'
        self.home.mkdir()
        self.data = self.home / 'data'
        self.data.mkdir()
        (self.home / 'app').mkdir()
        self.application = self.home / 'app/bilicomics'
        executable(self.application, 'exit 95')
        self.launcher = self.home / 'launch.sh'
        self.firmware = root / 'firmware'
        self.fifo = root / 'worker-receipt'
        os.mkfifo(self.fifo, 0o600)
        self.counts = root / 'counts'
        self.bin = root / 'bin'
        self.bin.mkdir()
        executable(self.bin / 'usleep', 'exit 96')
        self.env = dict(PATH=str(self.bin) + ':' + os.defpath, LC_ALL='C', HOME=str(root),
                        BILICOMICS_HOME=str(self.home), BENCH_FIFO=str(self.fifo),
                        BENCH_FIRMWARE=str(self.firmware), BENCH_LOCK=str(root / 'display-lock'))
        self.configure('valid')

    def configure(self, scenario):
        self.application.chmod(0o600 if scenario == 'missing-executable' else 0o700)
        (self.data / 'device-profile').write_text('unknown\n' if scenario == 'unknown-profile' else 'scribe\n')
        self.firmware.write_text('Invalid\n' if scenario == 'malformed-firmware' else 'Kindle 5.19.5 (fixture)\n')
        (self.data / 'device-allowlist').write_text('scribe 0\n' if scenario == 'unaccepted-firmware' else 'scribe 5.19.5\n')

    def launch(self, source, counted=None):
        self.launcher.write_text(source, encoding='ascii')
        env = self.env.copy()
        if counted is not None:
            env.update(PATH=str(counted) + ':' + env['PATH'], BENCH_ROLE='entry', BENCH_COUNTS=str(self.counts))
        read_fd = os.open(self.fifo, os.O_RDWR | os.O_NONBLOCK | os.O_CLOEXEC)
        try:
            start = time.monotonic_ns()
            with subprocess.Popen(['/bin/sh', str(self.launcher), '--library'], env=env,
                                  stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL) as entry:
                exit_fd = os.pidfd_open(entry.pid)
                try:
                    readable, _, _ = select.select([read_fd, exit_fd], [], [], 10)
                    require(readable, 'Library fixture exceeded its deadline')
                    try:
                        data = os.read(read_fd, 256)
                    except BlockingIOError:
                        data = b''
                    elapsed = time.monotonic_ns() - start
                    status = entry.wait(timeout=10)
                    if data:
                        words = data.decode('ascii').split()
                        require(len(words) == 4 and words[2:] == ['scribe', '5.19.5']
                                and int(words[0]) > 1 and int(words[0]) != entry.pid and int(words[1]) == entry.pid
                                and data.endswith(b'\n') and status == 0, 'Invalid detached worker receipt or status')
                        return status, elapsed
                    require(status != 0, 'Library entry succeeded without a qualified worker receipt')
                    return status, None
                finally:
                    os.close(exit_fd)
        finally:
            os.close(read_fd)


def controls(root, sources):
    probes = root / 'counters'
    counter_tools(probes)
    result = {}
    for scenario, expected in (('valid', KINDS), ('unknown-profile', KINDS[:1]),
                               ('malformed-firmware', KINDS[:2]), ('unaccepted-firmware', KINDS[:3]),
                               ('missing-executable', ())):
        visits = {}
        for arm in ('A', 'B'):
            with tempfile.TemporaryDirectory(dir=root) as scratch:
                fixture = Fixture(Path(scratch))
                fixture.configure(scenario)
                fixture.counts.touch()
                status, elapsed = fixture.launch(sources[arm], counted=probes)
                passed = scenario == 'valid'
                require(status == (0 if passed else 1) and (elapsed is not None) == passed, 'Invalid scenario status')
                require(all((fixture.data / name).exists() == passed for name in ('logs', 'fonts'))
                        and not Path(fixture.env['BENCH_LOCK']).exists(), 'Unexpected fixture mutation')
                expect_entry = list(expected) if arm == 'A' else []
                expect_worker = list(expected) if arm == 'B' or passed else []
                lines = fixture.counts.read_text().splitlines()
                require(lines == [f'entry {name}' for name in expect_entry]
                        + (['worker entered'] if arm == 'B' or passed else [])
                        + [f'worker {name}' for name in expect_worker], f'Unexpected {arm}/{scenario} call sequence')
                visits[arm] = {'entry_calls': expect_entry, 'worker_calls': expect_worker,
                               'worker_entered': 'worker entered' in lines, 'status': status,
                               'prelock_worker_receipt': passed}
        result[scenario] = visits
    return result


def block_summary(pairs):
    require(tuple(pair['order'] for pair in pairs) == ORDERS, 'Incomplete or reordered fixed block')
    deltas = [pair['B_minus_A_ns'] for pair in pairs]
    wins = sum(delta < 0 for delta in deltas)
    median = statistics.median(deltas)
    return {'B_faster': wins, 'median_paired_B_minus_A_ns': median,
            'ordered_paired_B_minus_A_medians_ns': {
                order: statistics.median(pair['B_minus_A_ns'] for pair in pairs if pair['order'] == order)
                for order in ('AB', 'BA')},
            'passes_host_gate': wins >= 5 and median <= -1_000_000}


def time_block(root, sources):
    fixture = Fixture(root)
    for name in ('logs', 'fonts'):
        (fixture.data / name).mkdir()

    def observation(arm):
        status, elapsed = fixture.launch(sources[arm])
        require(status == 0 and elapsed is not None, 'Invalid timed visit; no replacement allowed')
        return elapsed

    warmups = {arm: observation(arm) for arm in ('A', 'B')}
    pairs = []
    for order in ORDERS:
        samples = {arm: observation(arm) for arm in order}
        pairs.append({'order': order, 'A_ns': samples['A'], 'B_ns': samples['B'],
                      'B_minus_A_ns': samples['B'] - samples['A']})
    require(not Path(fixture.env['BENCH_LOCK']).exists(), 'Timed fixture attempted a display lock')
    return {'warmups_excluded_ns': warmups, 'pairs': pairs, **block_summary(pairs)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('baseline', 'candidate', 'output'):
        parser.add_argument('--' + name, type=Path, required=True)
    args = parser.parse_args()
    os.umask(0o077)
    require(not args.output.exists(), 'Expected a fresh output file')
    launchers = {arm: path.read_text(encoding='ascii') for arm, path in (('A', args.baseline), ('B', args.candidate))}
    sources = {arm: launcher_prefix(value) for arm, value in launchers.items()}
    require(sources['A'] != sources['B'], 'Expected different Library preflight paths')
    with tempfile.TemporaryDirectory(prefix='bili-library-admission-') as scratch:
        root = Path(scratch)
        control = controls(root, sources)
        blocks = []
        for _ in range(2):
            with tempfile.TemporaryDirectory(dir=root) as block:
                blocks.append(time_block(Path(block), sources))
    result = {'scope': 'Linux host fixture: launch of sh launch.sh --library to detached pre-lock success receipt; no native card, app, framebuffer I/O or Shelf time.',
              'clock': 'host CLOCK_MONOTONIC', 'native_card_to_shelf_render': None, 'native_same_frame_accepted_send': None,
              'controlled_tools_outside_timing': control, 'blocks': blocks,
              'passes_both_host_gates': all(block['passes_host_gate'] for block in blocks)}
    with args.output.open('x', encoding='utf-8') as output:
        json.dump(result, output, indent=2)
        output.write('\n')


if __name__ == '__main__':
    main()
