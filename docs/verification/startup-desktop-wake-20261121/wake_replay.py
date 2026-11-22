"""Extend the existing minimal ordinary-Shelf replay with this experiment's wake and activity evidence."""

import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile
import tempfile

for sibling in ('startup-direct-png-20261121', 'startup-cover-worker-20261121'):
    sys.path.append(str(Path(__file__).resolve().parent.parent / sibling))
import close_replay as base
import stage_costs
import summarize_pairs
from wake_pairs import FULL_DATA, KNOWN_SOURCES, SOURCES, TURN_DATA, diagnostics


BOUNDARIES = {'window_shown', 'first_scene_rendered', 'workers_started'}
QUEUES = {'font_worker_queued', 'state_worker_queued', 'font_worker_begin', 'state_worker_begin'}
HOOKS = {'desktop_software_full_submit': FULL_DATA, 'desktop_post_submit_turn': TURN_DATA}


def store(path, value):
    path.write_text(json.dumps(value, indent=2) + '\n', encoding='utf-8')


def extend(destination, source, name):
    perf = stage_costs.rows(destination / 'performance.jsonl')
    life = stage_costs.rows(destination / 'lifecycle.jsonl')
    for row in stage_costs.rows(source / 'performance.jsonl'):
        label = row['stage']
        if row['kind'] != 'startup' or label not in BOUNDARIES | QUEUES | set(HOOKS) | {'driver_first_entry', 'ui_worker_gate'}:
            continue
        data = row['data']
        if label in BOUNDARIES:
            projection = {'observed': base.select(data['observed'], 'monotonic_ns')}
        elif label in QUEUES:
            projection = {}
        elif label in HOOKS:
            projection = {key: data[key] for key in HOOKS[label]}
        else:
            projection = base.select(data, 'source render_notifier_registered first_scene_rendered')
            stage_costs.require(projection['source'] in KNOWN_SOURCES, 'Unexpected source in replay')
            if label == 'ui_worker_gate':
                projection['workers_admitted_by_ui'] = data['workers_admitted_by_ui']
        perf.append(dict(base.select(row, 'kind stage process_id thread_id monotonic_ns'), data=projection))
    for row in stage_costs.rows(source / 'lifecycle.jsonl'):
        if row.get('stage') in BOUNDARIES:
            life.append(base.select(row, 'event stage process_id monotonic_ns'))
    for filename, rows in (('performance.jsonl', perf), ('lifecycle.jsonl', life)):
        with (destination / filename).open('w', encoding='utf-8') as output:
            for row in sorted(rows, key=lambda value: value['monotonic_ns']):
                output.write(json.dumps(row, separators=(',', ':')) + '\n')
    raw = stage_costs.load(source / 'result.json')
    new = stage_costs.load(destination / 'result.json')
    stage_costs.require(new['case'] == raw['case'] == name and len(raw['initial_activity_sample_ns']) == 2, 'Wrong case or missing activity samples')
    new['initial_activity_sample_ns'] = list(raw['initial_activity_sample_ns'])
    new.update(base.select(raw, 'bootstrap_probe_mode process_probe_environment_verified'))
    record = diagnostics(destination)
    feedback = max(new['feedback_sample_ns'][1], new['initial_activity_sample_ns'][1])
    record.update(visible_before_workers_started=feedback <= record['workers_started_ns'],
                  visible_before_state_worker_queued=feedback <= record['state_worker_queued_ns'],
                  visible_before_state_prepared=feedback <= new['stages']['producer_ns'])
    stage_costs.require(record == raw['wake'], 'Wake projection differs from the original observations')
    new['wake'] = record
    store(destination / 'result.json', new)
    return new


def summaries(trial):
    audit = Path(__file__).resolve().with_name('audit_wake_sources.py')
    output = subprocess.check_output([sys.executable, '-B', audit, trial])
    return summarize_pairs.summarize(trial), stage_costs.summarize(trial, 'shelf', 10), json.loads(output)


def export(source, output):
    reference = summaries(source)
    output.mkdir(mode=0o700)
    destination = output / 'trial'
    with tempfile.TemporaryDirectory(dir=output) as temporary:
        ordinary = Path(temporary) / 'ordinary'
        base.export(source, ordinary)
        shutil.copytree(ordinary / 'trial', destination)
    report = stage_costs.load(destination / 'report.json')
    stage_costs.require(report['sources'] == SOURCES, 'Replay belongs to another pair of builds')
    schedule = report['schedule']
    for name in schedule:
        report['cases'][name] = extend(destination / name, source / name, name)
    store(destination / 'report.json', report)
    stage_costs.require(summaries(destination) == reference, 'Cropped evidence does not reproduce all original summaries')
    files = [destination / 'report.json', *(destination / name / leaf for name in schedule
             for leaf in ('performance.jsonl', 'lifecycle.jsonl', 'final.json', 'result.json'))]
    archive_path = output / 'desktop-wake-replay.tar.gz'
    with tarfile.open(archive_path, 'x:gz') as archive:
        for path in files:
            member = tarfile.TarInfo(path.relative_to(output).as_posix())
            member.mode, member.size = 0o600, path.stat().st_size
            with path.open('rb') as payload:
                archive.addfile(member, payload)
    with tempfile.TemporaryDirectory(dir=output) as temporary, tarfile.open(archive_path, 'r:gz') as archive:
        names = [path.relative_to(output).as_posix() for path in files]
        stage_costs.require(archive.getnames() == names, 'Unexpected replay archive members')
        for member in archive.getmembers():
            target = Path(temporary) / member.name
            target.parent.mkdir(parents=True, exist_ok=True)
            with archive.extractfile(member) as payload, target.open('xb') as copy:
                shutil.copyfileobj(payload, copy)
        stage_costs.require(summaries(Path(temporary) / 'trial') == reference, 'Extracted replay differs from the original')
    (output / 'shelf-pairs.md').write_text(reference[0] + '\n', encoding='utf-8')
    store(output / 'wake-audit.json', reference[2])
    endpoint = base.endpoints(report)
    endpoint['scope'] += ' Presentation gate here is endpoint-only; pre-worker feedback and correctness are independent required gates.'
    endpoint['all_14_title_and_activity_reads_before_workers'] = reference[2]['all_title_and_activity_reads_completed_before_workers']
    endpoint['desktop_presentation_and_feedback_gates_passed'] = endpoint['presentation_gate_passed'] and endpoint['all_14_title_and_activity_reads_before_workers']
    endpoint['formal_provenance_feedback_and_presentation_passed'] = reference[2]['formal_provenance_feedback_and_presentation_passed']
    store(output / 'shelf-summary.json', endpoint)
    store(output / 'replay.json', {'cases': len(schedule), 'archive_files': len(files), 'archive_bytes': archive_path.stat().st_size,
                                   'wake_audit_and_both_stage_and_endpoint_summaries_match_full_records_after_extraction': True})


if __name__ == '__main__':
    os.umask(0o077)
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('trial', type=Path)
    parser.add_argument('new_output', type=Path)
    args = parser.parse_args()
    export(args.trial, args.new_output)
