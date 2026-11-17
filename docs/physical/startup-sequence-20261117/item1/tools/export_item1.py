#!/usr/bin/env python3
"""Export allowlisted stored item 1 evidence without device or runtime access."""

import hashlib
import json
from io import BytesIO
from pathlib import Path

from PIL import Image


WORK = Path('/root/bilicomics-startup-sequence-20261117-01a1d11a-r1')
AB = Path('/root/bilicomics-physical-startup-sequence-20261117-f028b769')
ASSISTED = Path('/root/bilicomics-physical-startup-sleep-20261117-a1067e98')
DERIVED = WORK / 'analysis-item1-f028b769'
TOOLS = WORK / 'tooling-tests'
STAGING = WORK / 'export-item1-inputs-r1'
OUT = WORK / 'export-item1-f028b769-r1'

CASES = (
    'baseline-warmup', 'candidate-warmup',
    'baseline-01', 'candidate-01', 'candidate-02', 'baseline-02',
    'baseline-03', 'candidate-03', 'candidate-04', 'baseline-04',
    'baseline-05', 'candidate-05', 'candidate-06', 'baseline-06',
)
RAW_NAMES = (
    'performance.jsonl', 'lifecycle.jsonl', 'shell.tsv',
    'memory-at-content.txt', 'request.json', 'post-business-gate.json',
    'recovery.json', 'final.json',
)
TOOL_NAMES = (
    'startup_ab.py', 'analyze_startup_ab.py', 'startup-clock.c',
    'test_startup_batch.py', 'test_startup_analysis.py', 'device_lease.py',
    'guarded-device-ssh.py', 'native_recovery_fragment.sh',
)
DERIVED_NAMES = {
    'README.md': ('analysis/critical-path.md', '5bfd7cf41132714adb6963ed7c1f2b2f7d8ba2b36c46fc9a8c524aae919a52ee'),
    'item1-analysis.json': ('analysis/item1-analysis.json', '51759a67d68d39e1523cdd451da95bbec652e4dc8e1dda09ea35200e2dee0698'),
    'timeline.csv': ('analysis/timeline.csv', 'c8798bc96cf2a17d1dea1d7cc56553fbce352ffda6908f1cceef679db9bda9d3'),
    'frame-spans.csv': ('analysis/frame-spans.csv', '5737257663add7b715547cbff095c6a49a1da1124526c1f34f106a7080b46e9c'),
    'analysis-manifest.json': ('analysis/analysis-manifest.json', 'ef1396023dd7b270010c9bbe0a7d8e90cf204064b7a4fe928f11d5741d7e7786'),
    'analyze_stored_item1.py': ('tools/analyze_stored_item1.py', '99b059a0155a9569a685558a2402940cc062eb58d850231b01f2ba4090fde16c'),
}
REPORT_HASH = '683dbd5cbca7cc682846238f59d3e5a2830196d05cf2434949239916487e5256'
ANALYSIS_HASH = 'fa74ed9b18b469e3ee5558cde6f40df599365c7a698727a81e5643c703d21261'
FROZEN_TOOLS_HASH = 'e297e17673f3e2ee57838654bacda2e573ebfb23c4f1bf7296ac03b567c6ecc7'
AUDIT_HASH = '8d247feb4c68412d6f78e12b85073b3c35bde4f34f280d02ebd4567e3b5f6a96'
FIXTURE_MANIFEST_HASH = 'eec7ef692df034b764eb510219ce23bf28c068d8fe9e931e752c62d2468f3a96'
FINALIZATION = 'device-batch-b1c511b360bd4bd3b818ed4179750186'


def sha(content):
    return hashlib.sha256(content).hexdigest()


def require(condition, label):
    if not condition:
        raise ValueError(label)


def select(value, fields):
    return {field: value[field] for field in fields}


class Export:
    def __init__(self):
        require(not OUT.exists(), 'output_directory_already_exists')
        OUT.mkdir(parents=True)
        self.files = {}
        self.inputs = {}

    def read(self, path, expected=None):
        # Every caller supplies an explicit file or a fixed case/name product.
        content = path.read_bytes()
        identity = {'path': str(path), 'bytes': len(content), 'sha256': sha(content)}
        if expected is not None:
            require(identity['sha256'] == expected, 'source_identity_changed')
        self.inputs[str(path)] = identity
        return content, identity

    def json_input(self, path, expected=None):
        content, identity = self.read(path, expected)
        return json.loads(content), identity

    def write(self, name, content, sources, transformation, evidence_class):
        require(name not in self.files, 'duplicate_output_name')
        target = OUT / name
        require(target.is_relative_to(OUT), 'output_path_outside_export')
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(content)
        self.files[name] = {
            'bytes': len(content), 'sha256': sha(content),
            'evidence_class': evidence_class,
            'transformation': transformation,
            'sources': sources,
        }

    def json(self, name, value, sources, transformation, evidence_class='derived-stored-evidence'):
        self.write(name, (json.dumps(value, indent=2, sort_keys=True, allow_nan=False) + '\n').encode(),
                   sources, transformation, evidence_class)

    def copy(self, source, name, transformation, evidence_class, expected=None):
        content, identity = self.read(source, expected)
        self.write(name, content, [identity], transformation, evidence_class)
        return identity


def main():
    export = Export()
    script_bytes, script_id = export.read(Path(__file__))
    report, report_id = export.json_input(AB / 'report.json', REPORT_HASH)
    analysis, analysis_id = export.json_input(AB / 'analysis.json', ANALYSIS_HASH)
    require(tuple(report['cases']) == CASES == tuple(report['completed_cases']), 'case_schedule_changed')
    require(tuple(row['case'] for row in analysis['cases']) == CASES, 'analysis_schedule_changed')
    require(report['success'] and analysis['status'] == 'passed', 'batch_not_qualified')
    rows = {row['case']: row for row in analysis['cases']}

    export.write('tools/export_item1.py', script_bytes, [script_id],
                 'Verbatim exporter source; execution performs fixed stored-file reads and derived artifact writes only.',
                 'exporter-source')
    export.copy(AB / 'analysis.json', 'analysis/frozen-analyzer.json',
                'Verbatim privacy-reviewed frozen analyzer output; derived from the ended raw batch, not a raw trace.',
                'derived-frozen-analysis', ANALYSIS_HASH)
    for source_name, (target_name, expected) in DERIVED_NAMES.items():
        export.copy(DERIVED / source_name, target_name,
                    'Verbatim final frozen stored-evidence analysis output or its source; not a new experiment or raw trace.',
                    'derived-stored-analysis' if source_name != 'analyze_stored_item1.py' else 'analysis-source', expected)
    for name in TOOL_NAMES:
        export.copy(TOOLS / name, 'tools/' + name,
                    'Verbatim privacy-reviewed public measurement source; no tool is executed by export.', 'measurement-source')
    frozen, frozen_id = export.json_input(TOOLS / 'frozen-tools-r1.json', FROZEN_TOOLS_HASH)
    export.copy(TOOLS / 'frozen-tools-r1.json', 'tools/frozen-tools-r1.json',
                'Verbatim public frozen tooling identity and historical host-only fixture-test record.', 'measurement-provenance', FROZEN_TOOLS_HASH)
    for name in TOOL_NAMES:
        require(export.files['tools/' + name]['sha256'] == frozen['files'][name]['sha256'], 'frozen_tool_identity_changed')
    export.copy(STAGING / 'PHYSICAL_AUDIT.md', 'analysis/physical-audit.md',
                'Verbatim independent stored-evidence audit; transferred unchanged from the local ignored measurement revision 1 document.',
                'independent-stored-evidence-audit', AUDIT_HASH)
    fixture_id = export.copy(AB / 'inputs/fixture-manifest.json', 'evidence/fixture-manifest.json',
                'Verbatim privacy-reviewed anonymous offline-demo fixture identities; no fixture payload or database is exported.',
                'fixture-identities', FIXTURE_MANIFEST_HASH)

    for case in CASES:
        for name in RAW_NAMES:
            expected = rows[case]['file_sha256'].get(name)
            require(expected is not None, 'missing_frozen_raw_identity')
            export.copy(AB / 'outputs/out' / case / name, 'cases/' + case + '/' + name,
                        'Verbatim privacy-reviewed stored file; complete original bytes, no field removal or record filtering.',
                        'physical-device-stored-raw', expected)

    report_fields = (
        'schema_version', 'run_id', 'scope', 'started_at_utc', 'finished_at_utc',
        'success', 'cancelled', 'cases', 'completed_cases', 'primary_rounds',
        'same_boot', 'input_roster_unchanged', 'install_unchanged', 'lighting_restored',
        'native_restored', 'temporary_stage_removed', 'transport_closed', 'lease_released',
        'preservation_scope', 'preservation_path_count', 'post_business_observation',
    )
    export.json('evidence/report.projection.json', {
        'derivation': 'Fixed allowlist projection; not original report bytes.',
        'source': report_id, 'fields': select(report, report_fields),
    }, [report_id], 'Fixed allowlist projection; command lines, owner fields, private bindings and native Home identities omitted.')
    release, release_id = export.json_input(AB / 'evidence/lease-released.json')
    release_fields = (
        'ok', 'operation', 'state', 'project', 'run_id', 'released_at_utc',
        'lease_directory_absent_after_release', 'native_restored_and_connections_closed',
        'only_owned_owner_json_removed_then_rmdir', 'owner_and_token_checked', 'private_token_retained',
    )
    export.json('evidence/lease-release.projection.json', {
        'derivation': 'Fixed allowlist projection; private ownership values and token digest excluded.',
        'source': release_id, 'fields': select(release, release_fields),
    }, [release_id], 'Fixed allowlist projection of the completed release receipt; not original bytes or current lease state.')
    closure, closure_id = export.json_input(AB / 'evidence/transport-closure.json')
    closure_fields = (
        'schema_version', 'run_id', 'connections_closed', 'device_polled', 'lease_accessed',
        'signals_sent', 'ssh_multiplexing', 'unreadable_host_process_count',
    )
    export.json('evidence/transport-closure.projection.json', {
        'derivation': 'Fixed allowlist projection; private ownership values excluded.',
        'source': closure_id, 'fields': select(closure, closure_fields),
        'active_owned_transport_count': len(closure['active_owned_transports']),
    }, [closure_id], 'Fixed allowlist projection of the completed host transport-closure receipt.')

    images = []
    for case in ('baseline-06', 'candidate-06'):
        for label, raw_name, field in (
            ('early', 'content-frame.raw', 'framebuffer_capture'),
            ('scheduled', 'settled-frame.raw', 'settled_framebuffer_capture'),
        ):
            observation = rows[case][field]
            raw, raw_id = export.read(AB / 'outputs/out' / case / raw_name, observation['file_sha256'])
            header, separator, pixels = raw.partition(b'\n')
            require(bool(separator), 'framebuffer_header_separator_missing')
            geometry = json.loads(header)
            require(geometry == observation['geometry'], 'framebuffer_geometry_changed')
            require(geometry['width'] == 1860 and geometry['height'] == 2480 and
                    geometry['bits_per_pixel'] == 8 and geometry['line_length'] == 1872 and
                    len(pixels) == 4612800 and sha(pixels) == observation['visible_raw_sha256'],
                    'framebuffer_visible_payload_changed')
            image = Image.frombytes('L', (geometry['width'], geometry['height']), pixels)
            stream = BytesIO()
            image.save(stream, format='PNG')
            png = stream.getvalue()
            image_name = 'images/' + case + '-' + label + '-memory.png'
            transformation = 'Lossless PNG encoding of all packed visible Gray8 memory bytes after the JSON header; no crop, resize, rotation, inversion, masking, tone correction or panel compensation.'
            export.write(image_name, png, [raw_id], transformation, 'derived-framebuffer-memory-image')
            sidecar = {
                'derivation': 'Lossless memory projection, not a panel photograph or optical completion observation.',
                'source': raw_id, 'source_header_bytes': len(header), 'source_header_sha256': sha(header),
                'source_separator_bytes': 1, 'source_visible_pixel_bytes': len(pixels),
                'source_visible_pixel_sha256': sha(pixels), 'geometry': geometry,
                'png': {'path': image_name, 'bytes': len(png), 'sha256': sha(png), 'mode': 'L', 'width': 1860, 'height': 2480},
                'transformation': transformation, 'complete_visible_frame_preserved': True,
                'padding_included': False, 'native_epdc_mode': observation['native_epdc_mode'],
                'native_epdc_mode_provenance': observation['mode_provenance'],
                'framebuffer_visual_inversion_applied': False, 'epdc_inversion_applied': False,
                'application_software_inversion_reapplied': False, 'physical_panel_update_confirmed': False,
                'capture_window': observation['capture_window'],
                'observation_timing': observation.get('observation_timing'),
                'application_liveness': rows[case]['capture_liveness_status'],
            }
            export.json(image_name[:-4] + '.json', sidecar, [raw_id, analysis_id],
                        'Derived image-provenance sidecar; original raw framebuffer bytes remain private.')
            images.append(image_name)

        logical_path = AB / 'outputs/out' / case / 'final.png'
        logical_id = export.copy(logical_path, 'images/' + case + '-final-logical.png',
                                 'Verbatim original logical Slint PNG before framebuffer compensation; no pixel or encoding changes.',
                                 'original-logical-image', rows[case]['file_sha256']['final.png'])
        export.json('images/' + case + '-final-logical.json', {
            'derivation': 'Derived provenance sidecar for an unchanged original logical PNG.',
            'source': logical_id, 'logical_snapshot': rows[case]['final_capture'],
            'physical_framebuffer_capture': False, 'physical_panel_update_confirmed': False,
            'pixel_masking_applied': False, 'encoding_changed': False,
        }, [logical_id, analysis_id], 'Derived provenance sidecar; image bytes are unchanged.')
        images.append('images/' + case + '-final-logical.png')

    assisted_run, assisted_run_id = export.json_input(ASSISTED / 'evidence/run/run.json')
    checkpoint, checkpoint_id = export.json_input(ASSISTED / 'evidence/run/before-sleep.checkpoint.json')
    finalization, finalization_id = export.json_input(ASSISTED / ('evidence/' + FINALIZATION + '.json'))
    assisted_release, assisted_release_id = export.json_input(ASSISTED / ('evidence/' + FINALIZATION + '.release.json'))
    assisted_closure, assisted_closure_id = export.json_input(ASSISTED / ('evidence/' + FINALIZATION + '.closure.json'))
    assisted_lifecycle_bytes, assisted_lifecycle_id = export.read(ASSISTED / 'evidence/run/lifecycle.jsonl')
    assisted_lifecycle = [json.loads(line) for line in assisted_lifecycle_bytes.splitlines() if line.strip()]
    require(assisted_run['phase'] == 'stopped' and assisted_run['screen'] == 'reader' and
            assisted_run['content_mode'] == 'demo' and
            assisted_run['binary_sha256'] == analysis['variants']['candidate']['binary_sha256'] and
            assisted_run['package_sha256'] == analysis['variants']['candidate']['package_sha256'],
            'assisted_scope_or_binary_changed')
    require(finalization['command'] == 'stop' and finalization['status'] == 'completed' and
            finalization['finalization'] == 'released' and finalization['harness_returncode'] == 0 and
            finalization['stage'] == 'lease_release', 'assisted_finalization_changed')
    require(assisted_release['ok'] and assisted_release['owner_and_token_checked'] and
            assisted_release['lease_directory_absent_after_release'] and
            assisted_release['native_restored_and_connections_closed'] and
            assisted_closure['connections_closed'] and not assisted_closure['active_owned_transports'] and
            assisted_release['run_id'] == assisted_closure['run_id'] == assisted_run['run_id'] == finalization['run_id'],
            'assisted_completed_recovery_or_release_changed')
    ready = [(index, row) for index, row in enumerate(assisted_lifecycle, 1) if row.get('event') == 'ready']
    require(len(ready) == 1, 'assisted_ready_count_changed')
    require(set(assisted_run['checkpoints']) == {'before-sleep'} and not assisted_run['observations'],
            'assisted_observation_membership_changed')
    require(checkpoint['reader_state']['page_number'] == 18 and checkpoint['ui']['route'] == 'reader',
            'assisted_before_sleep_anchor_changed')
    require(assisted_run['watchdog']['reason'] == 'launcher-exit' and
            str(assisted_run['watchdog']['launcher_exit_status']) == '0', 'assisted_exit_reason_changed')
    checkpoint_projection = {
        'derivation': 'Fixed allowlist projection of the before-sleep checkpoint; no image, titles, IDs, paths or private ownership values.',
        'source': checkpoint_id,
        'native_process': select(checkpoint['native_process'], ('pid', 'start_ticks', 'exe_sha256', 'demo_offline_native_arguments_verified')),
        'requested_checkpoint': checkpoint['requested_checkpoint'], 'native_epdc_mode': checkpoint['native_epdc_mode'],
        'latest_power': checkpoint['latest_power'], 'trace_records': checkpoint['trace_records'],
        'route': checkpoint['ui']['route'],
        'reader': select(checkpoint['reader_state'], (
            'page_number', 'page_index', 'left_page_number', 'right_page_number', 'route',
            'mode', 'fit', 'rotation', 'frame_width', 'frame_height', 'viewport_width',
            'viewport_height', 'loading' if 'loading' in checkpoint['reader_state'] else 'rendering',
            'transition', 'menu', 'reader_generation', 'refresh_mode',
        )),
        'ui_reader_loading': checkpoint['ui']['reader']['loading'],
        'ui_reader_transition': checkpoint['ui']['reader']['transition'],
        'framebuffer_image_exported': False,
    }
    export.json('assisted-attempt/before-sleep.projection.json', checkpoint_projection, [checkpoint_id],
                'Fixed allowlist field projection; not raw checkpoint bytes or evidence of sleep/wake/input.')
    ready_index, ready_row = ready[0]
    export.json('assisted-attempt/ready.projection.json', {
        'derivation': 'One fixed ready-event field projection from the stored lifecycle trace.',
        'source': assisted_lifecycle_id, 'source_line_number': ready_index,
        'fields': select(ready_row, ('event', 'elapsed_ms', 'monotonic_ns', 'process_id', 'route', 'native_epdc_mode', 'frame_size')),
    }, [assisted_lifecycle_id], 'Fixed allowlist projection of the original ready record; not physical input or optical completion.')
    assisted_run_fields = (
        'schema_version', 'evidence_class', 'run_id', 'phase', 'operator_confirmed_model',
        'profile', 'expected_firmware', 'physical_device_accessed', 'live_account_used',
        'remote_account_writes', 'input_injected_by_harness', 'hardware_support_accepted',
        'physical_panel_review', 'binary_sha256', 'package_sha256', 'content_mode', 'screen',
        'exit_after_ms', 'init_timeout_seconds', 'absolute_timeout_seconds',
        'assisted_physical_input', 'started_unix', 'performance_trace_enabled',
        'watchdog', 'recovery_checks',
    )
    export.json('assisted-attempt/run.projection.json', {
        'derivation': 'Fixed allowlist projection; this is not original run.json bytes.',
        'source': assisted_run_id, 'fields': select(assisted_run, assisted_run_fields),
        'checkpoint_names': sorted(assisted_run['checkpoints']),
        'stored_observation_count': len(assisted_run['observations']),
    }, [assisted_run_id], 'Fixed allowlist projection; all lease/transport bindings, executable paths, account UI and image data excluded.')
    export.json('assisted-attempt/finalization.projection.json', {
        'derivation': 'Fixed allowlist finalization projection.', 'source': finalization_id,
        'fields': select(finalization, ('schema_version', 'run_id', 'command', 'status', 'finalization', 'harness_returncode', 'stage')),
    }, [finalization_id], 'Fixed allowlist projection; private ownership and report binding paths excluded.')
    export.json('assisted-attempt/lease-release.projection.json', {
        'derivation': 'Fixed allowlist completed-release projection; private ownership values and token digest excluded.',
        'source': assisted_release_id, 'fields': select(assisted_release, release_fields),
    }, [assisted_release_id], 'Fixed allowlist projection; not original release bytes or current lease state.')
    export.json('assisted-attempt/transport-closure.projection.json', {
        'derivation': 'Fixed allowlist completed-closure projection; private ownership values excluded.',
        'source': assisted_closure_id,
        'fields': select(assisted_closure, closure_fields + ('checked_at_utc', 'scope')),
        'active_owned_transport_count': len(assisted_closure['active_owned_transports']),
    }, [assisted_closure_id], 'Fixed allowlist projection of the completed host transport-closure receipt.')
    assisted_summary = {
        'status': 'incomplete', 'operator_observation': 'no_operator_observation',
        'same_binary_as_ab': assisted_run['binary_sha256'] == analysis['variants']['candidate']['binary_sha256'],
        'launcher_scope': 'Isolated demo/offline reader through the production launcher, separate from the A/B native Scriptlet performance entry.',
        'before_sleep_page_number': 18, 'recorded_checkpoint_names': sorted(assisted_run['checkpoints']),
        'stored_observation_count': len(assisted_run['observations']),
        'resumed_checkpoint_present': False, 'after_input_checkpoint_present': False,
        'actual_operator_confirmation_received': False,
        'acceptance': {'sleep_wake': 'unverified', 'reading_continuation': 'unverified', 'physical_touch': 'unverified'},
        'bounded_exit_after_ms': assisted_run['exit_after_ms'],
        'watchdog': assisted_run['watchdog'], 'finalization': select(finalization, ('command', 'status', 'finalization', 'harness_returncode')),
        'exact_owned_release_recorded': assisted_release['ok'] and assisted_release['owner_and_token_checked'] and assisted_release['lease_directory_absent_after_release'],
        'released_at_utc': assisted_release['released_at_utc'],
        'interpretation': 'The bounded attempt and recovery succeeded; the requested sleep/wake/input observation did not occur in the received record and is not passed.',
    }
    export.json('assisted-attempt/summary.json', assisted_summary,
                [assisted_run_id, checkpoint_id, assisted_lifecycle_id, finalization_id, assisted_release_id, assisted_closure_id],
                'Derived bounded-attempt summary; absent operator feedback is preserved as incomplete, never inferred from a timer.')

    summary = {
        'schema_version': 1, 'evidence_class': 'stored-physical-device-offline-demo',
        'status': 'measured_batch_passed_not_final_adoption',
        'source_batch': str(AB), 'source_report': report_id, 'source_analysis': analysis_id,
        'variants': analysis['variants'], 'clock': analysis['clock'],
        'case_schedule': list(CASES), 'warmup_cases': analysis['warmup_cases'],
        'primary_cases': analysis['primary_cases'], 'measured_pairs': analysis['measured_pairs'],
        'paired_rounds': analysis['paired_rounds'], 'summary': analysis['summary'],
        'median_reductions': analysis['median_reductions'],
        'faster_pairs': {'content_send_ms': 5, 'software_ready_ms': 4},
        'cases': [select(rows[case], (
            'case', 'variant', 'warmup', 'included_in_primary_statistics', 'metrics',
            'request', 'post_business_gate', 'resource_observation_scope', 'memory', 'recovery',
            'first_startup_frame', 'first_business_frame', 'final_capture',
        )) for case in CASES],
        'restoration': analysis['restoration'], 'input_roster_unchanged': report['input_roster_unchanged'],
        'temporary_stage_removed': report['temporary_stage_removed'],
        'release': select(release, release_fields),
        'framebuffer_comparisons': analysis['framebuffer_comparisons'],
        'raw_framebuffer_capture_case_count': 2, 'raw_framebuffer_capture_file_count': 4,
        'final_logical_case_count': 14,
        'final_logical_business_pixels_match': analysis['final_logical_business_pixels_match'],
        'capture_windows': {case: {label: rows[case][field]['capture_window'] for label, field in (
            ('early', 'framebuffer_capture'), ('scheduled', 'settled_framebuffer_capture'))}
            for case in ('baseline-06', 'candidate-06')},
        'historical_host_fixture_checks': frozen['verification'],
        'fixture': frozen['fixture'], 'fixture_manifest': fixture_id,
        'historical_checks_scope': 'Previously completed mocked host-only tooling checks; distinct from physical results and not rerun by export.',
        'assisted_attempt': assisted_summary, 'limitations': analysis['limitations'],
        'followup_boundary': 'Candidate 03/06 contain an extra complete frame before Ready. A source followup is independent; this bundle does not measure it or assert final adoption.',
        'export_scope': 'Stored files only on test-env; no tests, builds, emulator, application, device transport, lease access, account access or new capture.',
    }
    export.json('summary.json', summary, [report_id, analysis_id, release_id, frozen_id, fixture_id,
                assisted_run_id, checkpoint_id, assisted_lifecycle_id, finalization_id,
                assisted_release_id, assisted_closure_id],
                'Derived bounded summary retaining all 14 cases, every measured pair, exact distributions and separate assisted incompleteness.')

    readme = """# Item 1 physical startup evidence, 2026-11-17

This fixed-allowlist export records an ended Scribe demo/offline A/B batch.
Export and its independent review read stored files on `test-env` only; no test,
build, application, device transport, input, lease operation or new capture ran.
The measured batch passed its frozen contract; it is **not final adoption** of
the startup candidate. The separate route-refresh followup has another source
identity and is outside this result.

Both variants use source `530f1516c332931059c52161b52d64e86488b3f1`, ELF
`04fc6f93b4327911ca1a3bb5200175c9f2e13fb21ac5e56280012ff44b6e2e59`, and ZIP
`1fe8ac38613d21904d187b41e973ff80302b0c78fe69b320e3d7143a31fd53ee`.
Only `--serial-startup-preparation` differs: baseline includes it; candidate
uses the default preparation sequence. [summary.json](summary.json) retains
the exact source fingerprint, compiler, package, worker, launcher, build and
fixture identities. This compares two paths in one ELF.

All 14 launches remain: two predefined warmups, then six balanced AB/BA pairs.
Only those warmups are omitted from primary distributions. No slow sample is
discarded, including candidate-03 Ready and candidate-06 Ready/SEND regressions.

| Primary endpoint, ms | Baseline median | Candidate median | Faster pairs |
| --- | ---: | ---: | ---: |
| First Shelf SEND accepted | 6696.776654 | 6465.361424 | 5/6 |
| Software Ready | 5905.555154 | 5691.152808 | 4/6 |

The clock is device `CLOCK_MONOTONIC`, from just before native launch exec to
`startup/ui_ready` or the first accepted rendered Shelf SEND whose Slint render
starts after Ready. This is a warm-process, single-boot experiment with retained
caches and profiling. It does not measure physical-tap latency, complete covers,
optical panel completion, cold boot, or a statistical population guarantee.

[Frozen analyzer output](analysis/frozen-analyzer.json), complete original
allowlisted [case files](cases/), [critical-path analysis](analysis/critical-path.md),
[timeline](analysis/timeline.csv), [frame spans](analysis/frame-spans.csv) and an
[independent physical audit](analysis/physical-audit.md) retain the full windows.
Candidate font-byte work overlaps the original first-frame completion wait;
metadata overlaps only partly. Cover pixels start after the original prepare
gate and checked completion. Candidate-03/06 each present an additional complete
frame between Route and Views before Ready. The record locates that cost but
does not identify the scheduling or instrumentation cause of crossing the
stage budget. The followup must be assessed independently.

Every case records normal recovery. The controller records restored native
control, selected installation identities, lighting, input roster and same
boot, removed its stage, closed owned transports and released the exact owned
lease at `2026-11-17T10:18:37.889218+00:00`. These are completed-event receipts,
not a claim about the current lease. Selected-file preservation is not a
whole-tree census. Native restoration is not new physical-touch acceptance.

The resource gate targets first business SEND +1500 ms. Actual procfs reads
follow that gate and any framebuffer capture. VmHWM is the kernel high-water
value when sampled, not a continuous startup-only or whole-lifetime peak.
Historical host-only fixture checks in the tooling manifest are separate from
these physical measurements and were not rerun during export.

## Six application images

| Case | Early memory | Scheduled memory | Original logical PNG |
| --- | --- | --- | --- |
| baseline-06 | [early](images/baseline-06-early-memory.png) | [scheduled](images/baseline-06-scheduled-memory.png) | [logical](images/baseline-06-final-logical.png) |
| candidate-06 | [early](images/candidate-06-early-memory.png) | [scheduled](images/candidate-06-scheduled-memory.png) | [logical](images/candidate-06-final-logical.png) |

The four Gray8 PNGs losslessly encode every one of the 4,612,800 visible memory
bytes at 1860 x 2480. Device stride is 1872; packed captures exclude padding.
No pixels are masked, cropped, resized, rotated, inverted or tone-corrected.
Sidecars bind the original raw/header/pixel identities, complete geometry,
compensation scope, capture bracket and PNG identity. Original raw framebuffer
files remain private. All four visible-pixel payloads match, and each complete
baseline/candidate comparison has zero differences. This covers **two cases
and four raw captures**, not 14 raw framebuffer captures.

| Case | Early request after SEND, ms | Scheduled request after SEND, ms | Scheduled helper bracket, ms |
| --- | ---: | ---: | ---: |
| baseline-06 | 560.504769 | 1500.400000 | 682.882923 |
| candidate-06 | 436.286462 | 1501.645385 | 709.222923 |

The early requests are delayed post-SEND observations, not the exact first-frame
instant. Scheduled pixels are observed within their recorded software bracket,
not instantaneously at +1500 ms; resource reads occur afterward. These are memory
projections, not camera photographs or optical panel acceptance. The two logical
PNGs retain original bytes before framebuffer compensation. All 14 original
logical snapshots have equal full RGB pixels in the frozen analysis.

## Separate incomplete assisted attempt

[The bounded assisted record](assisted-attempt/summary.json) uses the same ELF
with the isolated demo/offline reader through the production launcher. It is
separate from the Scriptlet performance entry. A before-sleep checkpoint records
reader page 18, but no actual operator key/touch feedback arrived within the
bounded attempt. Only `before-sleep` exists; there is no resumed or after-input
checkpoint or actual confirmation. The six-minute exit upper bound ended with
watchdog reason `launcher-exit`, status zero; stop, native recovery, transport
closure and exact lease release are recorded.

Its status is **incomplete / no_operator_observation**. Sleep/wake, reading
continuation and physical touch remain unverified. A timer or safe automatic
exit cannot supply those passes. Only fixed field projections and raw source
identities are exported; `run.json` is not copied in full.

## Export boundaries

`cases/` contains complete original bytes only for individually privacy-reviewed
performance/lifecycle/shell, memory, request/gate, recovery and demo snapshot
sidecar files. `analysis/` contains explicitly derived frozen analysis. Files
named `*.projection.json` are derived fixed-field projections and retain the
original input byte count and SHA-256; they must never be treated as raw originals.

The [export manifest](export-manifest.json) maps every payload to exact source
paths, byte counts, SHA-256 identities and transformations. Its own digest is in
[export-manifest.sha256](export-manifest.sha256). Public measurement source is
preserved under `tools/`; it is archival source, not permission to run it. The
tooling manifest identifies its original full freeze; only the declared sources
are included here, and its historical test logs are not exported. The
analysis manifest's filenames are relative to its original frozen analysis
directory; the outer export manifest maps their renamed export destinations.

No credentials, session files, signed URLs, private transport/ownership values,
native Home/library images, real account content, databases, ELF/ZIP payloads,
raw framebuffer files, full output archives or research archives are exported.
Fixture results, this offline-demo physical batch, the incomplete assisted
attempt, live-account behavior and optical/hardware acceptance remain separate.
"""
    export.write('README.md', readme.encode(), [script_id, report_id, analysis_id, assisted_run_id],
                 'New English export documentation derived from the fixed stored evidence and its stated limits.', 'export-documentation')
    manifest = {
        'schema_version': 1,
        'scope': 'Fixed-allowlist stored-evidence export only; source evidence unchanged; no device, lease, build, application or test execution.',
        'exporter': script_id,
        'source_roots': [str(AB), str(ASSISTED), str(DERIVED), str(TOOLS), str(STAGING)],
        'payload_file_count': len(export.files),
        'files': export.files,
        'manifest_self_binding': 'This JSON excludes itself and its SHA-256 sidecar; export-manifest.sha256 binds its exact bytes.',
    }
    manifest_bytes = (json.dumps(manifest, indent=2, sort_keys=True, allow_nan=False) + '\n').encode()
    (OUT / 'export-manifest.json').write_bytes(manifest_bytes)
    (OUT / 'export-manifest.sha256').write_text(sha(manifest_bytes) + '  export-manifest.json\n')
    print(json.dumps({'output': str(OUT), 'payload_file_count': len(export.files),
                      'total_file_count': len(export.files) + 2, 'manifest_bytes': len(manifest_bytes),
                      'manifest_sha256': sha(manifest_bytes), 'images': images}, sort_keys=True))


if __name__ == '__main__':
    main()
