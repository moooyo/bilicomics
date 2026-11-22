"""Independently replay and normalize a sanitized display-poll projection."""

import argparse
from fractions import Fraction
import json
from pathlib import Path
import stat
import sys


SCHEMA = 'bili-display-poll-observations-v1'
ARCHIVE_SCHEMA = 'bili-display-poll-archive-v1'
MAX_PROJECTION = 1 << 20
PLANNED_VISITS = 3
BRANCHES = {'automatic-five': 5, 'fixed-three': 3}
STATUSES = {'observed', 'eligible-shelf-no-poll-partition', 'closed-without-eligible-shelf', 'not-attempted'}
TOP = ('launch_approval', 'permit_acquire', 'permit_initial', 'query_outer_pre', 'query_outer_post', 'metadata_outer_pre',
       'metadata_outer_post', 'metadata_1', 'metadata_2', 'commands_run')
COMMAND = ('query_inner_pre', 'query_inner_post', 'query_inner_periodic', 'command_spawn', 'command_read', 'command_try_wait',
           'command_poll')
BUCKETS = set(TOP) | set(COMMAND) | {'metadata_checkpoint'}
RESIDUALS = {'helper_outside_top_level_buckets', 'commands_outside_measured_children', 'metadata_outside_measured_checkpoints'}
POLL_CHILDREN = ('stdout_open', 'after_eof')
POLL_CHILD_BUCKETS = {name: 'command_poll_' + name for name in POLL_CHILDREN}
BUCKETS |= set(POLL_CHILD_BUCKETS.values())
SCOPE = 'Partition of wall time in the existing poll syscall, not service latency, lost wakeups, savings or optical response.'
ACCOUNTING = ('command_poll is the measured parent bucket. stdout_open and after_eof are disjoint children that partition it; '
              'keep the parent in command residuals once and never add or subtract the children again.')


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
    require(len(values) == PLANNED_VISITS, 'A complete poll series must have three values')
    return sorted(values)[1]


def exact_ratio(numerator, denominator):
    if denominator == 0:
        return None
    return {'numerator': numerator, 'denominator': denominator}


def exact_fraction(value):
    if value is None:
        return None
    return {'numerator': value.numerator, 'denominator': value.denominator}


def bounded_interval(value):
    if value is None:
        return None
    require(type(value) is list and len(value) == 2 and all(natural(item) for item in value) and value[0] <= value[1],
            'Invalid sanitized startup interval')
    return list(value)


def validate_sample(sample):
    require(type(sample) is dict and set(sample) == {'count', 'elapsed_ns'} and count32(sample['count'])
            and natural(sample['elapsed_ns']) and (sample['count'] > 0 or sample['elapsed_ns'] == 0),
            'Invalid sanitized bucket sample')
    return {'count': sample['count'], 'elapsed_ns': sample['elapsed_ns']}


def source_share(sample, parent, field):
    denominator = parent[field]
    return None if denominator == 0 else sample[field] / denominator


def normalized_child(sample, parent):
    return {
        'count': sample['count'],
        'elapsed_ns': sample['elapsed_ns'],
        'share_of_poll_count': exact_ratio(sample['count'], parent['count']),
        'share_of_poll_elapsed': exact_ratio(sample['elapsed_ns'], parent['elapsed_ns']),
    }


def validate_helper(value):
    require(type(value) is dict and set(value) == {'query_branch', 'helper_elapsed_ns', 'process_stat_reads', 'buckets', 'residual_ns',
            'poll_partition'}, 'Unexpected sanitized display poll helper fields')
    branch = value['query_branch']
    require(type(branch) is str and branch in BRANCHES and natural(value['helper_elapsed_ns']) and count32(value['process_stat_reads'])
            and value['process_stat_reads'] > 0, 'Invalid sanitized display poll helper summary')
    buckets = value['buckets']
    require(type(buckets) is dict and set(buckets) == BUCKETS, 'Unexpected sanitized display poll bucket names')
    normalized_buckets = {key: validate_sample(sample) for key, sample in buckets.items()}
    counts = {key: sample['count'] for key, sample in normalized_buckets.items()}
    elapsed = {key: sample['elapsed_ns'] for key, sample in normalized_buckets.items()}

    queries = BRANCHES[branch]
    require(counts['commands_run'] == queries
            and all(counts[key] == queries for key in ('query_outer_pre', 'query_outer_post', 'query_inner_pre',
                                                        'query_inner_post', 'command_spawn'))
            and all(counts[key] == 1 for key in ('launch_approval', 'permit_acquire', 'permit_initial', 'metadata_1', 'metadata_2'))
            and counts['metadata_outer_pre'] == counts['metadata_outer_post'] == 2
            and counts['metadata_checkpoint'] == 6 and counts['command_read'] >= queries
            and counts['command_try_wait'] >= queries, 'Invalid sanitized display poll bucket counts')

    residuals = {
        'helper_outside_top_level_buckets': value['helper_elapsed_ns'] - sum(elapsed[key] for key in TOP),
        'commands_outside_measured_children': elapsed['commands_run'] - sum(elapsed[key] for key in COMMAND),
        'metadata_outside_measured_checkpoints': elapsed['metadata_1'] + elapsed['metadata_2'] - elapsed['metadata_checkpoint'],
    }
    require(min(residuals.values()) >= 0 and type(value['residual_ns']) is dict and set(value['residual_ns']) == RESIDUALS
            and all(natural(item) for item in value['residual_ns'].values())
            and exact_value(value['residual_ns'], residuals), 'Sanitized display poll residuals do not replay')

    partition = value['poll_partition']
    require(type(partition) is dict and set(partition) == {'parent', *POLL_CHILDREN},
            'Unexpected sanitized display poll partition fields')
    parent = validate_sample(partition['parent'])
    require(exact_value(parent, normalized_buckets['command_poll']),
            'Display poll partition parent differs from the original command_poll bucket')
    children = {}
    for name in POLL_CHILDREN:
        source = partition[name]
        require(type(source) is dict and set(source) == {'count', 'elapsed_ns', 'share_of_poll_count', 'share_of_poll_elapsed'},
                'Unexpected sanitized display poll child fields')
        sample = validate_sample({'count': source['count'], 'elapsed_ns': source['elapsed_ns']})
        require(exact_value(sample, normalized_buckets[POLL_CHILD_BUCKETS[name]]),
                'Display poll partition child differs from its original bucket copy')
        require(exact_value(source['share_of_poll_count'], source_share(sample, parent, 'count'))
                and exact_value(source['share_of_poll_elapsed'], source_share(sample, parent, 'elapsed_ns')),
                'Sanitized display poll child share does not replay')
        children[name] = sample
    require(sum(sample['count'] for sample in children.values()) == parent['count']
            and sum(sample['elapsed_ns'] for sample in children.values()) == parent['elapsed_ns'],
            'Display poll children do not partition the original count and wall interval exactly')

    return {
        'query_branch': branch,
        'helper_elapsed_ns': value['helper_elapsed_ns'],
        'process_stat_reads': value['process_stat_reads'],
        'buckets': {key: normalized_buckets[key] for key in sorted(BUCKETS)},
        'residual_ns': residuals,
        'poll_partition': {name: normalized_child(children[name], parent) for name in POLL_CHILDREN},
    }


def source_complete_series(helpers):
    if len(helpers) != PLANNED_VISITS or len({helper['query_branch'] for helper in helpers}) != 1:
        return None
    parent_samples = [helper['buckets']['command_poll'] for helper in helpers]
    result = {'query_branch': helpers[0]['query_branch'], 'visits': PLANNED_VISITS, 'poll_partition': {
        'parent': {field: median_three([sample[field] for sample in parent_samples]) for field in ('count', 'elapsed_ns')}
    }}
    for name in POLL_CHILDREN:
        child_samples = [helper['poll_partition'][name] for helper in helpers]
        result['poll_partition'][name] = {
            'count': median_three([sample['count'] for sample in child_samples]),
            'elapsed_ns': median_three([sample['elapsed_ns'] for sample in child_samples]),
        }
        for field, denominator_field in (('share_of_poll_count', 'count'), ('share_of_poll_elapsed', 'elapsed_ns')):
            values = [source_share(child, parent, denominator_field)
                      for child, parent in zip(child_samples, parent_samples)]
            result['poll_partition'][name][field] = median_three(values) if all(value is not None for value in values) else None
    return result


def archived_complete_series(helpers):
    source = source_complete_series(helpers)
    if source is None:
        return None
    parent_samples = [helper['buckets']['command_poll'] for helper in helpers]
    result = {'query_branch': source['query_branch'], 'visits': PLANNED_VISITS, 'poll_partition': {
        'parent': {field: median_three([sample[field] for sample in parent_samples]) for field in ('count', 'elapsed_ns')}
    }}
    for name in POLL_CHILDREN:
        child_samples = [helper['poll_partition'][name] for helper in helpers]
        parent_samples = [helper['buckets']['command_poll'] for helper in helpers]
        result['poll_partition'][name] = {
            'count': median_three([sample['count'] for sample in child_samples]),
            'elapsed_ns': median_three([sample['elapsed_ns'] for sample in child_samples]),
            'share_of_poll_count': exact_fraction(median_three([
                Fraction(child['count'], parent['count']) for child, parent in zip(child_samples, parent_samples)
            ]) if all(parent['count'] for parent in parent_samples) else None),
            'share_of_poll_elapsed': exact_fraction(median_three([
                Fraction(child['elapsed_ns'], parent['elapsed_ns']) for child, parent in zip(child_samples, parent_samples)
            ]) if all(parent['elapsed_ns'] for parent in parent_samples) else None),
        }
    return result


def replay(projection):
    require(type(projection) is dict and set(projection) == {'schema', 'planned', 'observed', 'counts_by_query_branch',
            'complete_series_medians', 'visits', 'scope'} and projection['schema'] == SCHEMA
            and type(projection['planned']) is int and projection['planned'] == PLANNED_VISITS
            and projection['scope'] == SCOPE and type(projection['visits']) is list
            and len(projection['visits']) == PLANNED_VISITS, 'Unexpected sanitized display poll projection')
    visits, measured, stopped = [], [], False
    branches = {'automatic-five': 0, 'fixed-three': 0}
    for ordinal, source in enumerate(projection['visits'], 1):
        require(type(source) is dict and set(source) == {'ordinal', 'status', 'helper', 'card_to_shelf_renderer_interval_ns',
                'card_to_accepted_send_interval_ns', 'card_to_same_marker_driver_interval_ns'}
                and type(source['ordinal']) is int and source['ordinal'] == ordinal
                and type(source['status']) is str and source['status'] in STATUSES, 'Unexpected sanitized display poll visit fields')
        require(not stopped or source['status'] == 'not-attempted', 'A visit follows an incomplete or unattempted poll slot')
        primary = bounded_interval(source['card_to_shelf_renderer_interval_ns'])
        accepted = bounded_interval(source['card_to_accepted_send_interval_ns'])
        driver = bounded_interval(source['card_to_same_marker_driver_interval_ns'])
        require(driver is None or accepted is not None, 'Driver completion lacks its accepted submission')
        helper = None if source['helper'] is None else validate_helper(source['helper'])
        if source['status'] == 'observed':
            require(helper is not None and primary is not None, 'Observed poll visit lacks its Shelf or helper result')
            measured.append(helper)
            branches[helper['query_branch']] += 1
        elif source['status'] == 'eligible-shelf-no-poll-partition':
            require(helper is None and primary is not None, 'Incomplete poll status has an invalid Shelf result')
        else:
            require(helper is None and primary is None and accepted is None and driver is None,
                    'Unqualified poll visit contains a Shelf or helper result')
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
            and set(projection['counts_by_query_branch']) == set(branches)
            and all(type(value) is int for value in projection['counts_by_query_branch'].values())
            and exact_value(projection['counts_by_query_branch'], branches), 'Sanitized poll observation counts do not replay')
    source_medians = source_complete_series(measured)
    require(exact_value(projection['complete_series_medians'], source_medians),
            'Sanitized complete-series poll medians do not replay')
    return {
        'schema': ARCHIVE_SCHEMA,
        'planned': PLANNED_VISITS,
        'observed': len(measured),
        'counts_by_query_branch': branches,
        'complete_series_medians': archived_complete_series(measured),
        'visits': visits,
        'scope': SCOPE,
        'accounting': ACCOUNTING,
        'replay_scope': 'Independent arithmetic and schema check of a sanitized projection; raw provenance and release remain the collector’s responsibility.',
    }


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        require(key not in result, 'Duplicate JSON key in sanitized display poll projection')
        result[key] = value
    return result


def reject_constant(_value):
    raise ValueError('Non-finite JSON number in sanitized display poll projection')


def load_projection(path):
    if str(path) == '-':
        raw = sys.stdin.buffer.read(MAX_PROJECTION + 1)
    else:
        info = path.lstat()
        require(stat.S_ISREG(info.st_mode) and info.st_size <= MAX_PROJECTION,
                'Expected a bounded sanitized display poll projection file')
        with path.open('rb') as source:
            raw = source.read(MAX_PROJECTION + 1)
    require(0 < len(raw) <= MAX_PROJECTION, 'Sanitized display poll projection is missing or oversized')
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
        cli.exit(1, 'Unverified sanitized display poll projection; no archive emitted.\n')
