"""Run three isolated poll-split observations with the established reversible native-card operator."""

from functools import cache
import importlib.util
from pathlib import Path
import sys
import zipfile


HERE = Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location('private_sampling_basis', HERE.parent / 'startup-display-sampling-20261122/display_runner.py')
SAMPLING = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(SAMPLING)
THREE, ROUTE, BASE, FROZEN, UTILS, CONTROL = SAMPLING.THREE, SAMPLING.ROUTE, SAMPLING.BASE, SAMPLING.FROZEN, SAMPLING.UTILS, SAMPLING.CONTROL
require = CONTROL.require
START, WAKE, HELPER = SAMPLING.START, SAMPLING.WAKE, SAMPLING.HELPER
ENV_RUN = b'BILICOMICS_DISPLAY_POLL_RUN'
SCHEMA = 'bili-display-poll-v1'
MARK = 'poll-visit.json'
ADMITTED = 'poll-inputs-verified.json'


def binding(case):
    return {'schema': SCHEMA, 'treatment': 'frozen-stock-display-poll', 'ordinal': case['ordinal'], 'run': case['run']}


def input_receipt(case):
    return {'schema': 'bili-display-poll-inputs-v1', 'run_id': case['run'], 'ordinal': case['ordinal'],
            'source': 'frozen-installed-control', 'comparison': 'complete-three-archive-byte-comparison'}


def case_files(stock, run):
    require(ENV_RUN not in stock, 'Stock launcher contains an unexpected poll selector')
    files = SAMPLING.case_files(stock, run)
    launcher = UTILS.once(files['launch.sh'], SAMPLING.ENV_RUN, ENV_RUN)
    switch = UTILS.once(files['switch.sh'], SAMPLING.ENV_RUN, ENV_RUN)
    for previous, updated in ((b'"data/$run.display-cost.json"|', b'"data/$run.display-poll.json"|'),
                              (b'! present "$stage/data/$run.display-cost.json" &&', b'! present "$stage/data/$run.display-poll.json" &&')):
        switch = UTILS.once(switch, previous, updated)
    require(SAMPLING.ENV_RUN not in launcher + switch and b'.display-cost.json' not in switch, 'A previous treatment leaked into the poll route')
    return {**files, 'launch.sh': launcher, 'switch.sh': switch}


def qualified_inputs(args):
    stock = UTILS.sources(args)
    require(len(stock) == 80185, 'Not the frozen shipping launcher')
    host = BASE.BOUNDS.read_json(CONTROL.plain(args.host_qualification, 64 << 10))
    require(host['schema'] == 'bili-display-poll-host-product-qualification-v1' and host['physical_device_accessed'] is False
            and all(host[key] is True for key in ('passed', 'complete_product_elf', 'unchanged_permit', 'only_hardware_inputs_synthetic', 'collector_validated'))
            and set(host['profiles']) == {'scribe', 'pw6'} and all(row[key] is True for row in host['profiles'].values()
                for key in ('passed', 'poll_split_counts_sum', 'poll_split_elapsed_sum')), 'Missing independent poll-split product-host qualification')
    source = CONTROL.plain(args.qualified_application, 150 << 20)
    stage = CONTROL.plain(args.application, 150 << 20)
    ticket = CONTROL.plain(args.arm_qualification, 16384)
    require(source.name == 'bilicomics' and source.parent.name == 'artifacts' and ticket == source.parent.parent / 'qualification/arm.json',
            'Expected the poll reference and its adjacent qualification')
    proof = BASE.BOUNDS.read_json(ticket)
    require(proof['schema'] == 'bili-display-poll-arm-qualification-v1' and proof['poll_branch_instrumentation'] is True and proof['passed'] is True
            and proof['host_integration_passed'] is True and proof['overlay_only'] is True and proof['physical_device_accessed'] is False
            and proof['device_lease_accessed'] is False and proof['cargo_bin'] == 'bilicomics' and proof['cross_target'] == 'armv7-unknown-linux-musleabihf'
            and proof['static_elf32_arm_hard_float'] is True and type(proof['application_bytes']) is int and proof['application_bytes'] == source.stat().st_size
            and type(proof['version_exit_code']) is int and proof['version_exit_code'] == 0 and proof['cli_version'] == 'bilicomics 0.1.25',
            'Missing poll-specific ARM qualification')
    profiles = proof['profiles']
    require(type(profiles) is list and len(profiles) == 2 and {row['profile'] for row in profiles} == {'scribe', 'pw6'}
            and all(type(row[key]) is int and row[key] == 1 for row in profiles for key in ('native_exit_code', 'helper_exit_code'))
            and all(row[key] is True for row in profiles for key in ('missing_owned_snapshot_rejected', 'helper_missing_permission_rejected'))
            and all(row[key] is False for row in profiles for key in ('helper_cost_sidecar_created', 'shelf_rendered', 'native_framebuffer_available', 'native_input_available')),
            'Both guarded ARM profiles must refuse unauthorized output')
    worker = CONTROL.plain(args.worker, 4 << 20)
    require(proof['worker']['unchanged_sources'] is True and proof['worker']['identical_to_install_zip'] is True
            and proof['worker']['bytes'] == worker.stat().st_size == 426672, 'Unexpected JPEG worker proof')
    with zipfile.ZipFile(FROZEN.STOCK_ZIP) as archive:
        member = 'bilicomics/app/kindle-jpeg-worker'
        require(archive.namelist().count(member) == 1 and archive.getinfo(member).file_size == worker.stat().st_size
                and archive.read(member) == worker.read_bytes(), 'Different installed JPEG worker')
    with stage.open('rb') as stream:
        require(not stage.samefile(source) and FROZEN.matched_stream(stream, source, stage.stat().st_size), 'Poll executable differs from the qualified build')
    return stock


@cache
def collector():
    return BASE.load_module(HERE / 'collect_display.py')


THREE.SCHEMA, THREE.MARK, THREE.ADMITTED = SCHEMA, MARK, ADMITTED
THREE.binding, THREE.input_receipt, THREE.case_files = binding, input_receipt, case_files
THREE.inputs, THREE.collector = qualified_inputs, collector


if __name__ == '__main__':
    try:
        THREE.main()
    except (CONTROL.RecoveryRequired, CONTROL.SampleUnavailable, CONTROL.OperatorError) as error:
        print(error, file=sys.stderr)
        sys.exit(3 if isinstance(error, CONTROL.RecoveryRequired) else 4 if isinstance(error, CONTROL.SampleUnavailable) else 2)
    except (Exception, KeyboardInterrupt):
        sys.exit('Poll observations stopped; check the original private plan and visit. Diagnostic details withheld.')
