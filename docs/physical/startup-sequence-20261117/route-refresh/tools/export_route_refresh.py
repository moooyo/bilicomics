#!/usr/bin/env python3
"""Export only fixed stored route-refresh evidence; never invoke runtime tools."""

import hashlib
import json
from io import BytesIO
from pathlib import Path

from PIL import Image


WORK = Path('/root/bilicomics-startup-sequence-20261117-01a1d11a-r1')
BATCH = Path('/root/bilicomics-physical-startup-route-20261117-196c8a73')
TOOLS = WORK / 'tooling-tests-r2'
DERIVED = WORK / 'analysis-route-196c8a73'
INPUTS = WORK / 'export-route-inputs-r1'
OUTPUT = WORK / 'export-route-196c8a73-r1'
REPORT_SHA = 'dec81ee7cd2d7e9032735bbd07dfe9c900cc783754542c2602a54e615e933091'
ANALYSIS_SHA = 'de6cd8fa9b7d2f4faa6e5ac876543cd8464f480abf917f6f8714a35cc86f9751'
FIXTURE_SHA = 'eec7ef692df034b764eb510219ce23bf28c068d8fe9e931e752c62d2468f3a96'
FROZEN_TOOLS_SHA = '621ad00e2c79d83b35a96c89fbd6e7d689e5d91319ba8672232509c3f498f32d'
AUDIT_SHA = '09ed26d05906214cfdfd69846748f20065cad3b2b5f5a89bff294d6eee12bdce'
CASES = (
    'baseline-warmup', 'candidate-warmup',
    'baseline-01', 'candidate-01', 'candidate-02', 'baseline-02',
    'baseline-03', 'candidate-03', 'candidate-04', 'baseline-04',
    'baseline-05', 'candidate-05', 'candidate-06', 'baseline-06',
)
RAW_NAMES = ('performance.jsonl', 'lifecycle.jsonl', 'shell.tsv', 'memory-at-content.txt',
             'request.json', 'post-business-gate.json', 'recovery.json', 'final.json')
TOOL_NAMES = ('startup_ab.py', 'analyze_startup_ab.py', 'startup-clock.c',
              'test_startup_batch.py', 'test_startup_analysis.py', 'device_lease.py',
              'guarded-device-ssh.py', 'native_recovery_fragment.sh')
DERIVED_NAMES = {
    'README.md': ('analysis/critical-path.md', '213d4f47b945fba8961fe1d7c9345c3c92f3248e615000313a3143f67d63b5fa'),
    'route-analysis.json': ('analysis/route-analysis.json', '973e19a44826f34498cdef6edba2bf16f5957174e35e602fafc16972d7788ef0'),
    'timeline.csv': ('analysis/timeline.csv', 'dda14ae1c9ac69e98246766a66beb640319752de0052d342a13713cf267237f6'),
    'frame-spans.csv': ('analysis/frame-spans.csv', '904dde0d7b6860c47836cff43caee7deeb8ea2011c281c21f355dd17ec39642a'),
    'analysis-manifest.json': ('analysis/analysis-manifest.json', '69e00addf64d147625fd170fc72e83aa6affb37b50be4107e3344bb53435539e'),
    'analyze_stored_route.py': ('tools/analyze_stored_route.py', '6bba5a54bc92782c2059ea08e0409efa33739c62842afbd683137f23c432b764'),
}


def sha(content):
    return hashlib.sha256(content).hexdigest()


def require(condition, label):
    if not condition:
        raise ValueError(label)


def select(value, names):
    return {name: value[name] for name in names}


class Export:
    def __init__(self):
        require(not OUTPUT.exists(), 'new_output_required')
        OUTPUT.mkdir(parents=True)
        self.files = {}

    def read(self, path, expected=None):
        content = path.read_bytes()
        identity = {'path': str(path), 'bytes': len(content), 'sha256': sha(content)}
        if expected is not None:
            require(identity['sha256'] == expected, 'frozen_input_changed')
        return content, identity

    def json_input(self, path, expected=None):
        content, identity = self.read(path, expected)
        return json.loads(content), identity

    def write(self, name, content, sources, transformation, evidence_class):
        require(name not in self.files and not Path(name).is_absolute() and '..' not in Path(name).parts,
                'unsafe_or_duplicate_output')
        target = OUTPUT / name
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(content)
        self.files[name] = {'bytes': len(content), 'sha256': sha(content), 'sources': sources,
                            'transformation': transformation, 'evidence_class': evidence_class}

    def json(self, name, value, sources, transformation, evidence_class='derived-stored-evidence'):
        content = (json.dumps(value, indent=2, sort_keys=True, allow_nan=False) + '\n').encode()
        self.write(name, content, sources, transformation, evidence_class)

    def copy(self, path, name, transformation, evidence_class, expected=None):
        content, identity = self.read(path, expected)
        self.write(name, content, [identity], transformation, evidence_class)
        return identity


def main():
    require(len(FROZEN_TOOLS_SHA) == len(AUDIT_SHA) == 64, 'final_audit_and_tool_identities_required')
    export = Export()
    _, exporter_id = export.read(Path(__file__))
    report, report_id = export.json_input(BATCH / 'report.json', REPORT_SHA)
    analysis, analysis_id = export.json_input(BATCH / 'analysis.json', ANALYSIS_SHA)
    require(report['success'] and analysis['status'] == 'passed', 'ended_batch_not_qualified')
    require(tuple(report['cases']) == tuple(report['completed_cases']) ==
            tuple(row['case'] for row in analysis['cases']) == CASES, 'case_schedule_changed')
    require(len({row['application_pid'] for row in analysis['cases']}) == 14, 'fresh_process_count_changed')
    require(analysis['variants']['baseline']['startup_arguments'] ==
            ['--demo', '--offline', '--legacy-startup-route-refresh'] and
            analysis['variants']['candidate']['startup_arguments'] == ['--demo', '--offline'],
            'route_only_treatment_changed')
    rows = {row['case']: row for row in analysis['cases']}
    export.copy(Path(__file__), 'tools/export_route_refresh.py', 'Verbatim fixed-allowlist stored exporter source; not a new experiment.', 'exporter-source')
    export.copy(BATCH / 'analysis.json', 'analysis/frozen-analyzer.json',
                'Verbatim privacy-reviewed frozen analyzer output; derived analysis, not a raw trace.', 'derived-frozen-analysis', ANALYSIS_SHA)
    derived_ids = {}
    for source_name, (target, expected) in DERIVED_NAMES.items():
        derived_ids[source_name] = export.copy(DERIVED / source_name, target,
            'Verbatim final frozen stored route-only analysis or its source; not a new run.',
            'analysis-source' if source_name.endswith('.py') else 'derived-stored-analysis', expected)
    frozen, frozen_id = export.json_input(TOOLS / 'frozen-tools-r2.json', FROZEN_TOOLS_SHA)
    export.copy(TOOLS / 'frozen-tools-r2.json', 'tools/frozen-tools-r2.json',
                'Verbatim historical measurement revision 2 provenance; fixture checks remain separate from device results.', 'measurement-provenance', FROZEN_TOOLS_SHA)
    tool_ids = {}
    for name in TOOL_NAMES:
        tool_ids[name] = export.copy(TOOLS / name, 'tools/' + name, 'Verbatim privacy-reviewed public measurement revision 2 source; not executed by export.',
                    'measurement-source', frozen['files'][name]['sha256'])
    audit_id = export.copy(TOOLS / 'PHYSICAL_AUDIT.md', 'analysis/physical-audit.md',
                'Verbatim final independent stored-evidence audit from the remote measurement revision 2 directory.',
                'independent-stored-evidence-audit', AUDIT_SHA)
    fixture_id = export.copy(BATCH / 'inputs/fixture-manifest.json', 'evidence/fixture-manifest.json',
                'Verbatim anonymous fixture identities only; no database or fixture payload.', 'fixture-identities', FIXTURE_SHA)
    for case in CASES:
        for name in RAW_NAMES:
            export.copy(BATCH / 'outputs/out' / case / name, 'cases/' + case + '/' + name,
                        'Verbatim individually privacy-reviewed stored original; complete bytes and records without filtering.',
                        'physical-device-stored-raw', rows[case]['file_sha256'][name])

    report_fields = ('schema_version', 'run_id', 'scope', 'started_at_utc', 'finished_at_utc',
        'success', 'cancelled', 'cases', 'completed_cases', 'primary_rounds', 'same_boot',
        'input_roster_unchanged', 'install_unchanged', 'lighting_restored', 'native_restored',
        'temporary_stage_removed', 'transport_closed', 'lease_released', 'preservation_scope',
        'preservation_path_count', 'post_business_observation')
    export.json('evidence/report.projection.json', {'derivation': 'Fixed allowlist projection, not original report bytes.',
        'source': report_id, 'fields': select(report, report_fields)}, [report_id],
        'Fixed field projection; command lines, private ownership bindings and native Home identities excluded.')
    release, release_id = export.json_input(BATCH / 'evidence/lease-released.json',
        '372375d976bfeef21f768ef2ee9d005f52487e648cb498b9752551da837da693')
    release_fields = ('ok', 'operation', 'state', 'project', 'run_id', 'released_at_utc',
        'lease_directory_absent_after_release', 'native_restored_and_connections_closed',
        'only_owned_owner_json_removed_then_rmdir', 'owner_and_token_checked', 'private_token_retained')
    export.json('evidence/lease-release.projection.json', {'derivation': 'Fixed field projection; private ownership values and token digest excluded.',
        'source': release_id, 'fields': select(release, release_fields)}, [release_id],
        'Completed-event release projection, not original receipt bytes or current lease state.')
    closure, closure_id = export.json_input(BATCH / 'evidence/transport-closure.json',
        'b41842cf22de3206ca05d7ef007d4c1921e451db5f75927ce1993c7b108436ad')
    export.json('evidence/transport-closure.projection.json', {'derivation': 'Fixed field projection; private ownership values excluded.',
        'source': closure_id, 'fields': select(closure, ('schema_version', 'run_id', 'connections_closed',
        'device_polled', 'lease_accessed', 'signals_sent', 'ssh_multiplexing', 'unreadable_host_process_count')),
        'active_owned_transport_count': len(closure['active_owned_transports'])}, [closure_id],
        'Fixed field projection of the completed host transport-closure event.')

    for case in ('baseline-06', 'candidate-06'):
        for label, raw_name, field in (('early', 'content-frame.raw', 'framebuffer_capture'),
                                       ('scheduled', 'settled-frame.raw', 'settled_framebuffer_capture')):
            observation = rows[case][field]
            raw, raw_id = export.read(BATCH / 'outputs/out' / case / raw_name, observation['file_sha256'])
            header, separator, pixels = raw.partition(b'\n')
            geometry = json.loads(header)
            require(separator and geometry == observation['geometry'] and
                    geometry['width'] == 1860 and geometry['height'] == 2480 and
                    geometry['bits_per_pixel'] == 8 and geometry['line_length'] == 1872 and
                    len(pixels) == 4612800 and sha(pixels) == observation['visible_raw_sha256'],
                    'frozen_visible_pixels_changed')
            stream = BytesIO()
            Image.frombytes('L', (1860, 2480), pixels).save(stream, format='PNG')
            png = stream.getvalue()
            target = 'images/' + case + '-' + label + '-memory.png'
            transform = 'Lossless PNG encoding of all packed visible Gray8 memory bytes after the JSON header; no crop, resize, rotation, inversion, masking, tone correction or panel compensation.'
            export.write(target, png, [raw_id], transform, 'derived-framebuffer-memory-image')
            export.json(target[:-4] + '.json', {
                'derivation': 'Lossless full-visible-memory projection, not a panel photograph or optical completion observation.',
                'source': raw_id, 'source_header_bytes': len(header), 'source_header_sha256': sha(header),
                'source_separator_bytes': 1, 'source_visible_pixel_bytes': len(pixels),
                'source_visible_pixel_sha256': sha(pixels), 'geometry': geometry,
                'png': {'path': target, 'bytes': len(png), 'sha256': sha(png), 'mode': 'L', 'width': 1860, 'height': 2480},
                'transformation': transform, 'complete_visible_frame_preserved': True, 'padding_included': False,
                'native_epdc_mode': observation['native_epdc_mode'], 'native_epdc_mode_provenance': observation['mode_provenance'],
                'framebuffer_visual_inversion_applied': False, 'epdc_inversion_applied': False,
                'application_software_inversion_reapplied': False, 'physical_panel_update_confirmed': False,
                'capture_window': observation['capture_window'], 'observation_timing': observation.get('observation_timing'),
                'application_liveness': rows[case]['capture_liveness_status']}, [raw_id, analysis_id],
                'Derived image-provenance sidecar; raw framebuffer bytes remain private.')
        logical_id = export.copy(BATCH / 'outputs/out' / case / 'final.png', 'images/' + case + '-final-logical.png',
            'Verbatim original logical Slint PNG before framebuffer compensation; no encoding or pixel changes.',
            'original-logical-image', rows[case]['file_sha256']['final.png'])
        export.json('images/' + case + '-final-logical.json', {'derivation': 'Derived provenance for unchanged original logical PNG.',
            'source': logical_id, 'logical_snapshot': rows[case]['final_capture'], 'physical_framebuffer_capture': False,
            'physical_panel_update_confirmed': False, 'pixel_masking_applied': False, 'encoding_changed': False},
            [logical_id, analysis_id], 'Derived provenance sidecar; original image bytes unchanged.')

    summary = {
        'schema_version': 1, 'decision': 'not_adopted', 'evidence_class': 'stored-physical-device-offline-demo',
        'measurement_status': analysis['status'],
        'decision_scope': 'The route-only experiment is retained as evidence but not adopted into the retained runtime. Both variants keep overlapped preparation.',
        'source_report': report_id, 'source_analysis': analysis_id, 'critical_path_analysis': derived_ids['route-analysis.json'],
        'variants': analysis['variants'], 'clock': analysis['clock'],
        'request_origin_clarification': 'The helper samples CLOCK_MONOTONIC before writing/closing request.json and execvp of lipc-set-prop com.lab126.appmgrd start for native Scriptlet dispatch; this is not direct UI-process exec.',
        'case_schedule': list(CASES), 'fresh_application_process_count': 14,
        'warmup_cases': analysis['warmup_cases'], 'primary_cases': analysis['primary_cases'],
        'measured_pairs': analysis['measured_pairs'], 'paired_rounds': analysis['paired_rounds'],
        'summary': analysis['summary'], 'median_reductions': analysis['median_reductions'],
        'faster_pairs': {metric: sum(pair['metrics'][metric]['reduction_ms'] > 0 for pair in analysis['paired_rounds'])
                         for metric in ('content_send_ms', 'software_ready_ms')},
        'cases': [select(rows[case], ('case', 'variant', 'warmup', 'included_in_primary_statistics', 'metrics',
            'request', 'post_business_gate', 'resource_observation_scope', 'memory', 'recovery',
            'first_startup_frame', 'first_business_frame', 'final_capture')) for case in CASES],
        'restoration': analysis['restoration'], 'input_roster_unchanged': report['input_roster_unchanged'],
        'temporary_stage_removed': report['temporary_stage_removed'], 'release': select(release, release_fields),
        'framebuffer_comparisons': analysis['framebuffer_comparisons'],
        'raw_framebuffer_capture_case_count': 2, 'raw_framebuffer_capture_file_count': 4,
        'final_logical_case_count': 14, 'final_logical_business_pixels_match': analysis['final_logical_business_pixels_match'],
        'capture_windows': {case: {label: rows[case][field]['capture_window'] for label, field in
            (('early', 'framebuffer_capture'), ('scheduled', 'settled_framebuffer_capture'))}
            for case in ('baseline-06', 'candidate-06')},
        'fixture': frozen['fixture'], 'fixture_manifest': fixture_id,
        'historical_host_fixture_checks': frozen['verification'],
        'historical_checks_scope': 'Previously completed tool-only host fixtures, with pre-review/final-source scope retained exactly in the frozen revision 2 manifest; separate from physical results and never rerun here.',
        'limitations': analysis['limitations'],
        'structural_finding': 'Candidate removes accepted noninitial pre-Ready forced-full GC16 replacements. Candidate 02/03/06 still render a no-SEND attempt; neither those attempts nor partial updates are additional full submissions.',
        'unimplemented_design': 'Bounded coalescing or backend-loop changes remain unimplemented hypotheses; no performance gain is asserted.',
        'independent_item1_scope': 'Separate source and treatment from item1; no serial preparation variant, pooled samples, or repeated assisted attempt.',
        'export_scope': 'Fixed stored-file reads and derived writes on test-env only; no tests, builds, application, emulator, runtime probe, account, device transport or lease operation.',
    }
    export.json('summary.json', summary, [report_id, analysis_id, derived_ids['route-analysis.json'],
        release_id, frozen_id, fixture_id, tool_ids['startup_ab.py'], tool_ids['startup-clock.c']],
        'Derived not-adopted route-only summary retaining all 14 cases and exact distributions.')
    b = rows['baseline-06']
    c = rows['candidate-06']
    windows = lambda row: (row['framebuffer_capture']['capture_window']['request_after_first_business_send_ms'],
        row['settled_framebuffer_capture']['capture_window']['request_after_first_business_send_ms'],
        row['settled_framebuffer_capture']['capture_window']['bracket_wall_ms'])
    bw, cw = windows(b), windows(c)
    readme = f'''# Route-refresh physical startup experiment, 2026-11-17

**Decision: NOT ADOPTED.** The route-only candidate removes redundant accepted
pre-Ready GC16 complete replacements, but does not produce a stable overall
startup gain. This independent experiment remains archived; its source is not
the retained runtime. No future coalescing or backend-loop design was implemented
or measured by this batch.

Both variants use source `f6208d6f7dd3d8e64a09ef0ceb2ceb6b0c45d0f5`, ELF
`3962bc49bb8cd33d21651845fe5de229e1b8458e5d0ba9026485c8bef7224525` and ZIP
`ff41fdf74505020f50ab9928b1a7168ed773b82713b3330ce6b71a0b915b194c`.
Both retain default overlapped preparation. Baseline uses
`--demo --offline --legacy-startup-route-refresh`; candidate uses
`--demo --offline`. There is no serial-preparation variant in this experiment.
[summary.json](summary.json) retains exact source-input fingerprint, compiler,
worker, launcher, package, build and fixture identities. This is separate from
item1; the batches are not pooled or compared as paired treatment effects.

## Complete measured result

All 14 launches remain: two predefined warmups and six balanced AB/BA pairs.
Only those warmups are omitted from primary distributions. Every slow sample
and all original performance/lifecycle windows remain in [cases/](cases/).

| Primary endpoint, ms | Baseline median | Candidate median | Faster pairs |
| --- | ---: | ---: | ---: |
| First Shelf SEND accepted | 6509.286116 | 6521.791231 | 3/6 |
| Software Ready | 5712.525615 | 5884.013962 | 3/6 |

Both marginal medians are slower for the candidate. The medians of paired B-C
differences are +39.451 ms for SEND and +26.821 ms for Ready; those are different
statistics and do not establish a stable gain. Full per-pair results are in the
summary, [frozen analyzer](analysis/frozen-analyzer.json) and
[critical-path report](analysis/critical-path.md).

The request helper samples device `CLOCK_MONOTONIC`, writes and closes the
request record, then calls `execvp()` for
`lipc-set-prop com.lab126.appmgrd start` with the native Scriptlet URI. Endpoints
remain the `startup/ui_ready` envelope stamp and first accepted Shelf SEND whose
Slint render begins after Ready. The interval includes request-record writing,
LIPC/appmgr dispatch and launcher work before application entry. This is a
warm-start, single-boot experiment with fresh application processes, retained
OS/filesystem caches and profiling. It does not measure physical-tap latency,
cold boot, complete covers, physical input readiness or optical panel completion.

## Why fewer complete submissions did not give an overall gain

No candidate launch, including warmup, has an accepted non-initial pre-Ready
forced-full GC16 flashing replacement. Baseline warmup, 02 and 06 each have one.
The initial GC16, sequence 2 full-screen-region DU and small activity AUTO
updates remain. A full-screen region is not the same as a complete replacement.

Candidate 02/03/06 still contain an unchanged-scene render attempt with zero
SEND ioctls. They wait for the preceding marker and compute RGB fingerprints;
they do not submit another partial or full update. Route-to-Views work therefore
remains about 400-411 ms in those samples, versus about 822-823 ms for the two
primary baseline replacements. Candidate 03 crosses this boundary where its
baseline does not, regressing Ready by 371.433 ms and SEND by 59.787 ms. The
earlier state-to-attachment handoff and cover-attachment presentation boundaries
also remain. These observed paths explain why saved submissions are insufficient
for adoption; no unrecorded scheduling/cache/power cause is asserted.

The complete [timeline](analysis/timeline.csv), [frame spans](analysis/frame-spans.csv)
and [independent physical audit](analysis/physical-audit.md) preserve actual
attempts, stages and bounds. Overlapping spans must not be added into elapsed
startup time. Bounded dispatch/coalescing at an existing barrier is only an
unimplemented design hypothesis, with no promised benefit.

## Recovery, resource and pixel scope

All 14 recoveries record normal exit and restored native control. Selected
installation identities, input roster and boot match; lighting was restored,
the temporary stage removed and owned transports closed. Exact owned lease
release is recorded at `2026-11-17T11:03:28.437714+00:00`. These are completed-event
receipts, not current lease claims. Selected-file preservation is not a whole-tree
census, and native restoration does not establish physical touch acceptance.

The resource gate targets first business SEND +1500 ms. Actual procfs reads
follow the gate and any scheduled framebuffer capture. Sampled VmRSS medians
are 55964 versus 55914 KiB, with mixed paired directions; this 50 KiB difference
does not establish a memory gain. VmHWM is the kernel high-water field when
sampled, not a continuous startup-only or whole-lifetime peak measurement.

| Case | Early memory | Scheduled memory | Original logical PNG |
| --- | --- | --- | --- |
| baseline-06 | [early](images/baseline-06-early-memory.png) | [scheduled](images/baseline-06-scheduled-memory.png) | [logical](images/baseline-06-final-logical.png) |
| candidate-06 | [early](images/candidate-06-early-memory.png) | [scheduled](images/candidate-06-scheduled-memory.png) | [logical](images/candidate-06-final-logical.png) |

The four Gray8 PNGs losslessly encode all 4,612,800 visible memory bytes at
1860 x 2480. Device stride is 1872; packed raw captures exclude padding. No
mask, crop, resize, rotation, inversion or tone correction is applied. Sidecars
bind original raw/header/pixel identities, complete geometry, compensation
scope, actual capture bracket and PNG identity. Raw framebuffer files remain
private. Both whole-visible-pixel pair comparisons have zero differences; all
four payload hashes also match. This covers two cases and four raw captures.
All 14 final logical Slint snapshots have equal full RGB pixels before
framebuffer compensation; the two exported logical PNGs retain original bytes.

| Case | Early request after SEND, ms | Scheduled request after SEND, ms | Scheduled bracket, ms |
| --- | ---: | ---: | ---: |
| baseline-06 | {bw[0]:.6f} | {bw[1]:.6f} | {bw[2]:.6f} |
| candidate-06 | {cw[0]:.6f} | {cw[1]:.6f} | {cw[2]:.6f} |

Early requests are delayed post-SEND observations, not the exact first-frame
instant. Scheduled pixels are observed within their recorded software bracket,
not instantaneously at +1500 ms. Sixth-pair procfs reads follow capture
completion at approximately +2167/+2162 ms. Memory pixels are not camera
photographs or optical panel completion. There is no new touch, sleep/wake,
rotation, live-account or hardware qualification, and no assisted attempt is
included in this independent route-refresh bundle.

## Export boundaries

Export and its independent review use stored files on `test-env` only; no test,
build, application, emulator, runtime probe, input, device transport or lease
operation runs. Original individually privacy-reviewed case files keep complete
bytes. Frozen analysis is explicitly derived; `*.projection.json` files are
fixed-field projections and retain raw source byte counts and SHA-256 identities.
They must not be treated as raw originals.

The [export manifest](export-manifest.json) maps every payload to exact source
paths, bytes, SHA-256 and transformations; its digest is in
[export-manifest.sha256](export-manifest.sha256). Public measurement revision 2
source is archived under `tools/`. Its historical tool-only host fixtures and
pre-review/final-source distinctions remain exactly in the frozen manifest,
separate from physical results; they are not rerun here. Only declared source
files are included from that larger tooling freeze. The analysis manifest uses
its original filenames; the outer manifest maps renamed export destinations.

No token or private ownership values, credentials, sessions, signed URLs,
private transport bindings, native Home/library images, real account content,
database payloads, ELF/ZIP binaries, raw framebuffer files, full output archives
or research archives are exported. Fixture results, this physical offline-demo
experiment and optical/hardware acceptance remain separate evidence classes.
'''
    export.write('README.md', readme.encode(), [exporter_id, report_id, analysis_id,
        derived_ids['README.md'], audit_id, release_id, tool_ids['startup_ab.py'], tool_ids['startup-clock.c']],
        'New English documentation of this not-adopted stored route-only experiment.', 'export-documentation')
    manifest = {'schema_version': 1, 'measurement_revision': 2, 'export_revision': 1,
        'experiment': 'route-refresh', 'decision': 'not_adopted', 'exporter': exporter_id,
        'scope': 'Fixed-allowlist stored-only export; no raw or runtime changes and no new tests, builds, application, device or lease operations.',
        'source_roots': [str(BATCH), str(DERIVED), str(TOOLS), str(INPUTS)],
        'payload_file_count': len(export.files), 'files': export.files,
        'manifest_self_binding': 'This JSON excludes itself and its SHA-256 sidecar; export-manifest.sha256 binds its exact bytes.'}
    content = (json.dumps(manifest, indent=2, sort_keys=True, allow_nan=False) + '\n').encode()
    (OUTPUT / 'export-manifest.json').write_bytes(content)
    (OUTPUT / 'export-manifest.sha256').write_text(sha(content) + '  export-manifest.json\n')
    print(json.dumps({'output': str(OUTPUT), 'payload_file_count': len(export.files),
        'total_file_count': len(export.files) + 2, 'manifest_bytes': len(content), 'manifest_sha256': sha(content)}, sort_keys=True))


if __name__ == '__main__':
    main()
