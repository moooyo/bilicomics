"""Recheck and normalize the sanitized display-sampling projection only."""

import argparse
import json
from pathlib import Path
import stat
import sys


SCHEMA = 'bili-display-sampling-observations-v1'
ARCHIVE_SCHEMA = 'bili-display-sampling-archive-v1'
MAX_PROJECTION = 1 << 20
TOP = ('launch_approval', 'permit_acquire', 'permit_initial', 'query_outer_pre', 'query_outer_post', 'metadata_outer_pre',
       'metadata_outer_post', 'metadata_1', 'metadata_2', 'commands_run')
COMMAND = ('query_inner_pre', 'query_inner_post', 'query_inner_periodic', 'command_spawn', 'command_read', 'command_try_wait', 'command_poll')
BUCKETS = set(TOP) | set(COMMAND) | {'metadata_checkpoint'}
RESIDUALS = {'helper_outside_top_level_buckets', 'commands_outside_measured_children', 'metadata_outside_measured_checkpoints'}
BRANCHES = {'automatic-five': 5, 'fixed-three': 3}
SCOPE = 'Single diagnostic baseline: counts and wall intervals are not a speedup, a pure hardware cost or optical timing.'
VISIT_STATUSES = {'observed', 'eligible-shelf-no-capture-breakdown', 'closed-without-eligible-shelf', 'not-attempted'}


def require(condition, reason):
    if not condition:
        raise ValueError(reason)


def exact_value(actual, expected):
    if type(actual) is not type(expected):
        return False
    if type(expected) is dict:
        return actual.keys() == expected.keys() and all(exact_value(actual[key], value) for key, value in expected.items())
    if type(expected) is list:
        return len(actual) == len(expected) and all(exact_value(item, value) for item, value in zip(actual, expected))
    return actual == expected


def natural(value):
    return type(value) is int and 0 <= value < 1 << 64


def count32(value):
    return type(value) is int and 0 <= value < 1 << 32


def median_three(values):
    require(len(values) == 3, 'A complete series must have three values')
    return sorted(values)[1]


def bounded_interval(value):
    if value is None:
        return None
    require(type(value) is list and len(value) == 2 and all(natural(item) for item in value) and value[0] <= value[1],
            'Invalid sanitized startup interval')
    return list(value)


def validate_helper(value):
    require(type(value) is dict and set(value) == {'query_branch', 'helper_elapsed_ns', 'process_stat_reads', 'buckets', 'residual_ns'},
            'Unexpected sanitized helper fields')
    branch = value['query_branch']
    require(branch in BRANCHES and natural(value['helper_elapsed_ns']) and count32(value['process_stat_reads'])
            and value['process_stat_reads'] > 0, 'Invalid sanitized helper summary')
    buckets = value['buckets']
    require(type(buckets) is dict and set(buckets) == BUCKETS, 'Unexpected sanitized bucket names')
    counts, elapsed = {}, {}
    for key, sample in buckets.items():
        require(type(sample) is dict and set(sample) == {'count', 'elapsed_ns'} and count32(sample['count'])
                and natural(sample['elapsed_ns']) and (sample['count'] > 0 or sample['elapsed_ns'] == 0),
                'Invalid sanitized bucket sample')
        counts[key], elapsed[key] = sample['count'], sample['elapsed_ns']

    queries = BRANCHES[branch]
    require(counts['commands_run'] == queries
            and all(counts[key] == queries for key in ('query_outer_pre', 'query_outer_post', 'query_inner_pre',
                                                        'query_inner_post', 'command_spawn'))
            and all(counts[key] == 1 for key in ('launch_approval', 'permit_acquire', 'permit_initial', 'metadata_1', 'metadata_2'))
            and counts['metadata_outer_pre'] == counts['metadata_outer_post'] == 2
            and counts['metadata_checkpoint'] == 6 and counts['command_read'] >= queries
            and counts['command_try_wait'] >= queries, 'Invalid sanitized bucket counts')
    residuals = {
        'helper_outside_top_level_buckets': value['helper_elapsed_ns'] - sum(elapsed[key] for key in TOP),
        'commands_outside_measured_children': elapsed['commands_run'] - sum(elapsed[key] for key in COMMAND),
        'metadata_outside_measured_checkpoints': elapsed['metadata_1'] + elapsed['metadata_2'] - elapsed['metadata_checkpoint'],
    }
    require(min(residuals.values()) >= 0 and type(value['residual_ns']) is dict and set(value['residual_ns']) == RESIDUALS
            and all(natural(item) for item in value['residual_ns'].values())
            and exact_value(value['residual_ns'], residuals), 'Sanitized residuals do not replay')
    return {
        'query_branch': branch,
        'helper_elapsed_ns': value['helper_elapsed_ns'],
        'process_stat_reads': value['process_stat_reads'],
        'buckets': {key: {'count': counts[key], 'elapsed_ns': elapsed[key]} for key in sorted(BUCKETS)},
        'residual_ns': residuals,
    }


def replay(projection):
    require(type(projection) is dict and set(projection) == {'schema', 'planned', 'observed', 'counts_by_query_branch',
            'complete_series_medians', 'visits', 'scope'} and projection['schema'] == SCHEMA
            and type(projection['planned']) is int and projection['planned'] == 3
            and projection['scope'] == SCOPE and type(projection['visits']) is list and len(projection['visits']) == 3,
            'Unexpected sanitized display projection')
    visits, measured, stopped = [], [], False
    branches = {'automatic-five': 0, 'fixed-three': 0}
    for ordinal, source in enumerate(projection['visits'], 1):
        require(type(source) is dict and set(source) == {'ordinal', 'status', 'helper', 'card_to_shelf_renderer_interval_ns',
                'card_to_accepted_send_interval_ns', 'card_to_same_marker_driver_interval_ns'}
                and type(source['ordinal']) is int and source['ordinal'] == ordinal
                and source['status'] in VISIT_STATUSES, 'Unexpected sanitized visit fields')
        require(not stopped or source['status'] == 'not-attempted', 'A visit follows an incomplete or unattempted slot')
        primary = bounded_interval(source['card_to_shelf_renderer_interval_ns'])
        accepted = bounded_interval(source['card_to_accepted_send_interval_ns'])
        driver = bounded_interval(source['card_to_same_marker_driver_interval_ns'])
        require(driver is None or accepted is not None, 'Driver completion lacks its accepted submission')
        helper = None if source['helper'] is None else validate_helper(source['helper'])
        if source['status'] == 'observed':
            require(helper is not None and primary is not None, 'Observed visit lacks its Shelf or helper result')
            measured.append(helper)
            branches[helper['query_branch']] += 1
        elif source['status'] == 'eligible-shelf-no-capture-breakdown':
            require(helper is None and primary is not None, 'Incomplete helper status has an invalid Shelf result')
        else:
            require(helper is None and primary is None and accepted is None and driver is None,
                    'Unqualified visit contains a Shelf or helper result')
        stopped = source['status'] != 'observed'
        visits.append({
            'ordinal': ordinal,
            'status': source['status'],
            'helper': helper,
            'card_to_shelf_renderer_interval_ns': primary,
            'card_to_accepted_send_interval_ns': accepted,
            'card_to_same_marker_driver_interval_ns': driver,
        })

    require(type(projection['observed']) is int and projection['observed'] == len(measured)
            and type(projection['counts_by_query_branch']) is dict
            and all(type(value) is int for value in projection['counts_by_query_branch'].values())
            and exact_value(projection['counts_by_query_branch'], branches), 'Sanitized observation counts do not replay')
    complete = None
    if len(measured) == 3 and len({row['query_branch'] for row in measured}) == 1:
        branch = measured[0]['query_branch']
        complete = {
            'query_branch': branch,
            'observations': 3,
            'helper_elapsed_ns': median_three([row['helper_elapsed_ns'] for row in measured]),
            'process_stat_reads': median_three([row['process_stat_reads'] for row in measured]),
            'buckets': {
                key: {field: median_three([row['buckets'][key][field] for row in measured]) for field in ('count', 'elapsed_ns')}
                for key in sorted(BUCKETS)
            },
            'residual_ns': {key: median_three([row['residual_ns'][key] for row in measured]) for key in sorted(RESIDUALS)},
        }
    require(exact_value(projection['complete_series_medians'], complete), 'Sanitized complete-series medians do not replay')
    return {
        'schema': ARCHIVE_SCHEMA,
        'planned': 3,
        'observed': len(measured),
        'counts_by_query_branch': branches,
        'complete_series_medians': complete,
        'visits': visits,
        'scope': SCOPE,
        'replay_scope': 'Independent arithmetic and schema check of a sanitized projection; raw provenance and release remain the collector’s responsibility.',
    }


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        require(key not in result, 'Duplicate JSON key in sanitized projection')
        result[key] = value
    return result


def reject_constant(_value):
    raise ValueError('Non-finite JSON number in sanitized projection')


def load_projection(path):
    if str(path) == '-':
        raw = sys.stdin.buffer.read(MAX_PROJECTION + 1)
    else:
        info = path.lstat()
        require(stat.S_ISREG(info.st_mode) and info.st_size <= MAX_PROJECTION, 'Expected a bounded sanitized projection file')
        with path.open('rb') as source:
            raw = source.read(MAX_PROJECTION + 1)
    require(0 < len(raw) <= MAX_PROJECTION, 'Sanitized projection is missing or oversized')
    return json.loads(raw, object_pairs_hook=unique_object, parse_constant=reject_constant)


if __name__ == '__main__':
    cli = argparse.ArgumentParser(description=__doc__)
    cli.add_argument('projection', help='Sanitized collector JSON file, or - for stdin')
    try:
        args = cli.parse_args()
        projection = '-' if args.projection == '-' else Path(args.projection)
        result = replay(load_projection(projection))
        print(json.dumps(result, indent=2, allow_nan=False))
    except (ValueError, TypeError, KeyError, OSError, UnicodeError):
        cli.exit(1, 'Unverified sanitized display projection; no archive emitted.\n')
