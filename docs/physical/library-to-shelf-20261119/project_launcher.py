#!/usr/bin/env python3
"""Project private Library-harness case directories to timing-only JSON on stdout.

Usage: python3 project_launcher.py RAW_CASE_ROOT CASE [CASE ...]
For the archived formal batch, RAW_CASE_ROOT is runs/ab-r6/outputs/out and
the six selected names are candidate-01 through candidate-06 (no warmup).
"""

import argparse
import hashlib
import json
from pathlib import Path
import re
from statistics import median
import tarfile


SOURCES = ('request.json', 'shell.tsv', 'production-launcher-timing.tsv', 'lifecycle.jsonl')
PROBES = ('capture_display_and_portrait', 'publish_portrait', 'query_sleep_cover')


def require(condition, message):
    if not condition:
        raise ValueError(message)


def one(values, label):
    values = list(values)
    require(len(values) == 1, f'Expected one {label}; found {len(values)}')
    return values[0]


def uptime_ns(stamp):
    require(re.fullmatch(r'[0-9]+\.[0-9]{2}', stamp), 'Expected two-decimal /proc/uptime')
    whole, cs = stamp.split('.')
    return int(whole) * 1_000_000_000 + int(cs) * 10_000_000


def delta(end, start):
    require(type(end) is int and type(start) is int and end >= start > 0, 'Invalid clock order')
    return end - start


def launcher_events(content):
    result = []
    for line in content.splitlines():
        parts = line.split()
        require(len(parts) >= 3, 'Incomplete production launcher record')
        fields = dict(p.split('=', 1) for p in parts[2:])
        stamp = uptime_ns(parts[0])
        require(not result or stamp >= result[-1][0], 'Decreasing uptime in startup trace')
        require(fields['phase'] == 'none' and fields['sleep'] == '0', 'Not an awake initial launch')
        require(not result or fields['pid'] == result[0][3], 'Launcher identity changed')
        result.append((stamp, parts[1], int(fields['read_sequence']), fields['pid']))
        if parts[1] == 'native_take_done':
            break
    require(result and result[-1][1] == 'native_take_done', 'No initial native takeover')
    return result


def project(path):
    files = {name: (path / name).read_bytes() for name in SOURCES}
    hashes = {name: hashlib.sha256(data).hexdigest() for name, data in files.items()}
    request = json.loads(files['request.json'])
    origin, mono = request['boottime_ns'], request['monotonic_ns']
    require(type(origin) is int and type(mono) is int and origin >= mono > 0, 'Missing clock pair')
    shell = [line.split() for line in files['shell.tsv'].decode('ascii').splitlines()]
    entry, worker = [uptime_ns(one((parts[0] for parts in shell if len(parts) == 2 and parts[1] == label), label))
                     for label in ('admission_entry', 'admission_worker')]
    events = launcher_events(files['production-launcher-timing.tsv'].decode('ascii'))

    def mark(name, sequence=None):
        return one((t for t, label, seq, _ in events if label == name and (sequence is None or seq == sequence)), name)

    sample, ready, take, done = map(mark, ('startup_sample_begin', 'startup_sample_ready', 'native_take_begin', 'native_take_done'))
    partition = {
        'request_to_instrumented_entry': delta(entry, origin),
        'entry_to_instrumented_worker': delta(worker, entry),
        'worker_to_sample_begin_includes_fixed_wait': delta(sample, worker),
        'sample_begin_to_sample_ready': delta(ready, sample),
        'sample_ready_to_take_begin': delta(take, ready),
        'take_begin_to_take_done': delta(done, take),
    }
    native_total = delta(done, origin)
    require(sum(partition.values()) == native_total, 'Boot-clock partition does not reconcile')
    require(partition['worker_to_sample_begin_includes_fixed_wait'] >= 2_000_000_000 - 10_000_000,
            'Expected the original two-second settle; not a comparable production policy')

    probes = {}
    for seq, name in enumerate(PROBES, 1):
        go, result, reaped = (mark(label, seq) for label in ('native_read_go', 'native_read_ready', 'native_read_reaped'))
        require((sample if seq == 1 else ready) <= go <= result <= reaped <= (ready if seq == 1 else take),
                'Probe outside its enclosing startup stage')
        probes[name + '_go_to_ready'] = delta(result, go)
        probes[name + '_ready_to_reaped'] = delta(reaped, result)
    require([seq for _, label, seq, _ in events if label == 'native_read_go'] == [1, 2, 3], 'Unexpected startup probe sequence')

    life = [json.loads(line) for line in files['lifecycle.jsonl'].splitlines()]
    enter = one((r['monotonic_ns'] for r in life if (r['event'], r.get('stage')) == ('startup-stage', 'process_enter')), 'Rust entry')
    require(one((r for r in life if r['event'] == 'ready'), 'ready').get('route') == 'shelf', 'Expected Shelf route')
    rust_total = delta(enter, mono)
    return hashes, {'nonoverlapping_boot': partition, 'nested_probe': probes, 'other': {
        'request_to_native_take_done_boot': native_total,
        'request_to_rust_entry_monotonic': rust_total,
        'estimated_take_done_to_rust_entry_if_no_suspend': rust_total - native_total,
    }}


def ms(ns):
    return round(ns / 1_000_000, 3)


def archive_provenance(path, expected, samples):
    digest = hashlib.sha256()
    with path.open('rb') as raw:
        for chunk in iter(lambda: raw.read(1024 * 1024), b''):
            digest.update(chunk)
        require(digest.hexdigest() == expected, 'Original output archive does not match the pinned digest')
        raw.seek(0)
        wanted = {f"out/{s['case']}/{name}": sha for s in samples for name, sha in s['source_sha256'].items()}
        count = len(wanted)
        seen = set()
        with tarfile.open(fileobj=raw, mode='r|gz') as archive:
            for entry in archive:
                if entry.name in wanted:
                    require(entry.name not in seen and entry.isfile(), 'Duplicate or nonregular timing member')
                    with archive.extractfile(entry) as source:
                        require(hashlib.sha256(source.read()).hexdigest() == wanted[entry.name], 'Timing member differs')
                    seen.add(entry.name)
        require(len(seen) == count, 'Missing timing archive member')
    return {'sha256': expected, 'matched_regular_members': count, 'member_pattern': 'out/CASE/SOURCE'}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('case_root', type=Path)
    parser.add_argument('cases', nargs='+')
    parser.add_argument('--outputs-archive', type=Path, help='Optionally bind the selected sources to a frozen outputs.tar.gz')
    parser.add_argument('--archive-sha256', help='Authoritative, independently recorded digest of that archive')
    args = parser.parse_args()
    require((args.outputs_archive is None) == (args.archive_sha256 is None), 'Archive path and pinned digest must be supplied together')
    require(len(set(args.cases)) == len(args.cases), 'Duplicate case')
    samples, tables = [], []
    for name in args.cases:
        require(re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9_-]{0,63}', name), 'Invalid case label')
        hashes, table = project(args.case_root / name)
        tables.append(table)
        samples.append({'case': name, 'source_sha256': hashes,
                        'ms': {group: {k: ms(v) for k, v in data.items()} for group, data in table.items()}})
    summary = {}
    for group, data in tables[0].items():
        summary[group] = {}
        for key in data:
            values = [table[group][key] for table in tables]
            summary[group][key] = dict(zip(('min', 'median', 'max'), map(ms, (min(values), median(values), max(values)))))
    print(json.dumps({
        'schema_version': 1, 'source': 'Stored Library-harness raw cases, no device access by this projection',
        'verified_source_archive': archive_provenance(args.outputs_archive, args.archive_sha256, samples) if args.outputs_archive else None,
        'scope': 'Awake, fresh-process Shelf launches with the original fixed two-second settle and three successful pre-takeover probes. Entry and worker are research prolog marks after admission-record publication; neither is a tap or an exact exec timestamp.',
        'clock': 'Request samples CLOCK_MONOTONIC then CLOCK_BOOTTIME before record close and dispatch. Launcher marks read /proc/uptime (boot time including suspend), truncated to 10 ms. Durations using an uptime mark have about 10 ms resolution; decimal places on the request origin are not greater landmark precision.',
        'groups': {'nonoverlapping_boot': 'Exclusive consecutive intervals on the boot clock, summing to each native total. The worker interval includes, but is not all, the fixed sleep. No speedup is inferred.',
                   'nested_probe': 'Within the sample or post-sample interval; not additional time. Go to ready includes child startup/command and supervisor polling; ready to reaped is observed cleanup. Gaps outside these markers remain in their parent interval.',
                   'other': 'The boot and monotonic totals are separately valid. Their difference estimates the final gap only assuming no new kernel suspend between request and Rust entry. It inherits uptime quantization and sequential request clock sampling skew; it is not a direct cross-clock timestamp.'},
        'statistics': 'Milliseconds; per-interval min, median and max. Marginal medians are not additive.',
        'summary_ms': summary, 'samples': samples,
    }, indent=2))


if __name__ == '__main__':
    main()
