#!/usr/bin/env python3
"""Project a closed 1+5 native-click batch; never use its preflight request as a click.

Run python3 -B project_click_launcher.py BATCH on the stored raw case host.
Only allowlisted timing records and their hashes are exported.
"""

import argparse
import json
from pathlib import Path
from statistics import median

from project_fresh import CASES, RESTORED, Sources, all_true, check_click
from project_launcher import archive_provenance, delta, ms, project, require


def scaled(groups):
    return {group: {k: ms(v) for k, v in data.items()} for group, data in groups.items()}


def main(batch):
    src = Sources(batch)
    report = src.json('report.json')
    all_true(report, ('success', 'original_native_tab_restored', *RESTORED))
    require(report['route'] == 'click' and report['cases'] == report['completed_cases'] == CASES
            and report['first_in_batch'] == CASES[0] and report['repeated_launches'] == CASES[1:]
            and report['primary_rounds'] == len(CASES) - 1 and report['cancelled'] is False,
            'Not the predefined completed click cohort')
    cases = {case: Sources(src.path / 'outputs' / 'out' / case) for case in CASES}
    rows, repeated = [], []
    for i, name in enumerate(CASES):
        case = cases[name]
        hashes, data = project(case.path)
        case.hashes.update(hashes)
        r = case.json('request.json')
        origin, origin_boot = r['monotonic_ns'], r['boottime_ns']
        enter = origin + data['other']['request_to_rust_entry_monotonic']
        click = check_click(case.json('click.json'), origin, enter)
        before, after = click['up_before'], click['up_after']
        bracket = delta(after, before)
        to_after = delta(after, origin)

        # Bound drift across this entire interval with a second, independent dual-clock record.
        post_case, post_file = (CASES[i + 1], 'native-stable-start.json') if i + 1 < len(CASES) else (name, 'qualified-capture-request.json')
        post = cases[post_case].json(post_file)
        require(post['monotonic_ns'] > enter and post['boottime_ns'] > origin_boot, 'Clock recheck does not bracket Rust entry')
        offset_change = (post['boottime_ns'] - post['monotonic_ns']) - (origin_boot - origin)
        require(abs(offset_change) < 1_000_000, 'Offsets changed by >=1 ms; do not map this release onto uptime')

        boot = data['nonoverlapping_boot']
        entry = boot.pop('request_to_instrumented_entry') - to_after
        require(entry >= 0, 'Release did not precede the entry mark')
        native = data['other']['request_to_native_take_done_boot'] - to_after
        parts = {'estimated_up_after_to_entry': entry, **boot}
        require(sum(parts.values()) == native, 'Release partition does not reconcile')
        groups = {
            'partition_from_up_after': parts,
            'nested_probe': data['nested_probe'],
            'endpoints': {
                'estimated_up_before_to_entry': entry + bracket,
                'estimated_up_before_to_native_take_done': native + bracket,
                'up_before_to_rust_entry_monotonic': delta(enter, before),
                'estimated_up_after_to_native_take_done': native,
                'up_after_to_rust_entry_monotonic': delta(enter, after),
                'estimated_native_take_done_to_rust_entry': data['other']['estimated_take_done_to_rust_entry_if_no_suspend'],
            },
            'method_only': {'excluded_request_to_up_before': delta(before, origin), 'up_send_to_xsync': bracket},
        }
        rows.append({'case': name, 'ms': scaled(groups), 'clock_recheck': {
            'case': post_case, 'source': post_file, 'boot_minus_monotonic_offset_change_ns': offset_change}})
        if i:
            repeated.append(groups)
    for row in rows:
        row['source_sha256'] = cases[row['case']].hashes
    stats = {group: {key: dict(zip(('min', 'median', 'max'),
                                   map(ms, (min(values), median(values), max(values)))))
                     for key in repeated[0][group] for values in [[sample[group][key] for sample in repeated]]}
             for group in repeated[0]}
    return {
        'schema_version': 1, 'run': report['run_id'], 'report_sha256': src.hashes['report.json'],
        'verified_source_archive': archive_provenance(src.path / 'outputs.tar.gz', report['outputs_sha256'], rows),
        'scope': 'Native XTest Library tile click to isolated offline Shelf, same boot and distinct process launches. The first launch is separate, not a cold boot.',
        'origin': 'Primary partition starts after XSync confirms the release (click.json up_after_ns); companion up_before_ns endpoints start immediately before sends. Both are device CLOCK_MONOTONIC, not finger/digitizer timestamps. Preflight request.json precedes target checks and is excluded from all launch spans.',
        'clock': 'Native marks are /proc/uptime truncated to 10 ms. Estimated release boot time = request.boottime_ns + (click.up_after_ns - request.monotonic_ns), likewise for up_before. A second dual-clock record after Rust entry checks the per-case offset; all absolute offset changes must be <1 ms. The two clock reads are sequential, not simultaneous. A boot/monotonic gap is an estimate, not optical or precisely sub-centisecond.',
        'groups': {'partition_from_up_after': 'Consecutive non-overlapping intervals; first crosses the checked clock mapping and later marks are same-domain uptime. The research admission prolog is included before entry/worker marks. Worker to sample includes a fixed 2 s settle, not 2 s of proven removable work.',
                   'nested_probe': 'Parts of the two relevant native sample intervals, not additional durations.',
                   'endpoints': 'Standalone totals under each release bracket. Rust entry is always measured directly on CLOCK_MONOTONIC.',
                   'method_only': 'Excluded preflight duration and the release injection bracket, not extra launch stages.'},
        'statistics': 'Milliseconds, rounded after calculating per-case intervals; only the five repeats enter min, median and max. Marginal medians are not additive.',
        'first_in_batch': rows[0], 'repeated_summary_ms': stats, 'repeated_samples': rows[1:],
    }


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('batch', type=Path)
    print(json.dumps(main(parser.parse_args().batch), indent=2))
