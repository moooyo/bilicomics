"""Run the previously qualified three-visit controller with a startup-only display-cost treatment."""

from functools import cache
import importlib.util
from pathlib import Path
import sys
import zipfile


HERE = Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location('private_display_three_visit_controller', HERE.parent / 'startup-prepared-handoff-20261122/handoff_runner.py')
THREE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(THREE)
CONTROL, ROUTE, UTILS, FROZEN, BASE = THREE.CONTROL, THREE.ROUTE, THREE.UTILS, THREE.FROZEN, THREE.BASE
require = CONTROL.require
ENV_RUN = b'BILICOMICS_DISPLAY_COST_RUN'
START = b'if ! capture_native_epdc_mode || [ "$NATIVE_CAPTURE_STATUS" != known ]; then\n'
WAKE = b"    capture_native_epdc_mode || NATIVE_EPDC_MODE=''\n"
HELPER = b'native_read_command "$APP_DIR/bilicomics" --capture-native-display --kindle --device "$PROFILE"; then\n'
SCHEMA = 'bili-display-sampling-v1'
MARK = 'display-visit.json'
ADMITTED = 'display-inputs-verified.json'


def binding(case):
    return {'schema': SCHEMA, 'treatment': 'frozen-stock-display-sampling', 'ordinal': case['ordinal'], 'run': case['run']}


def input_receipt(case):
    return {'schema': 'bili-display-sampling-inputs-v1', 'run_id': case['run'], 'ordinal': case['ordinal'],
            'source': 'frozen-installed-control', 'comparison': 'complete-three-archive-byte-comparison'}


def case_files(stock, run):
    files = ROUTE.case_files(stock, run, False)
    require(stock.count(START) == stock.count(WAKE) == stock.count(HELPER) == 1 and ENV_RUN not in stock,
            'Unknown startup, wake or native helper call site')
    temporary = ENV_RUN + b'=' + run.encode('ascii') + b' '
    launcher = UTILS.once(files['launch.sh'], START, START.replace(b'! ', b'! ' + temporary))
    source = files['switch.sh']
    start, end = b'launcher_verified() {\n', b'\n}\n\nclean_stage() {'
    require(source.count(start) == source.count(end) == 1, 'Unknown shipping launcher verifier')
    selector = UTILS.exact_script(b'launch.sh', b'BILI_DISPLAY_FROZEN_LAUNCHER', launcher)
    source = source[:source.index(start) + len(start)] + selector + source[source.index(end):]
    source = UTILS.once(source, b'"data/$run.jsonl"|data/logs/launcher.log|', b'"data/$run.display-cost.json"|"data/$run.jsonl"|data/logs/launcher.log|')
    source = UTILS.once(source, b'! present "$stage/data/$run.jsonl" &&', b'! present "$stage/data/$run.display-cost.json" && ! present "$stage/data/$run.jsonl" &&')
    return {**files, 'launch.sh': launcher, 'switch.sh': source}


def stock_process_guard():
    return CONTROL.plain(BASE.STUDY / 'host_tools/native_guard.sh', 128 << 10).read_bytes()


def qualified_inputs(args):
    stock = UTILS.sources(args)
    require(len(stock) == 80185, 'Wrong frozen launcher size')
    host = BASE.BOUNDS.read_json(CONTROL.plain(args.host_qualification, 64 << 10))
    require(host['schema'] == 'bili-display-host-product-qualification-v1' and host['passed'] is True and host['complete_product_elf'] is True
            and host['unchanged_permit'] is True and host['only_hardware_inputs_synthetic'] is True and host['physical_device_accessed'] is False
            and host['collector_validated'] is True and set(host['profiles']) == {'scribe', 'pw6'}
            and all(row['passed'] is True for row in host['profiles'].values()), 'Missing both successful, independently checked product-host profiles')
    staged = CONTROL.plain(args.application, 150 << 20)
    source = CONTROL.plain(args.qualified_application, 150 << 20)
    ticket = CONTROL.plain(args.arm_qualification, 16384)
    require(source.name == 'bilicomics' and source.parent.name == 'artifacts' and ticket == source.parent.parent / 'qualification/arm.json',
            'Not the retained source application and its adjacent qualification')
    proof = BASE.BOUNDS.read_json(ticket)
    require(proof['schema'] == 'bili-display-helper-arm-qualification-v1' and proof['passed'] is True and proof['host_integration_passed'] is True
            and proof['overlay_only'] is True and proof['physical_device_accessed'] is False and proof['device_lease_accessed'] is False
            and proof['cargo_bin'] == 'bilicomics' and proof['cross_target'] == 'armv7-unknown-linux-musleabihf' and proof['static_elf32_arm_hard_float'] is True
            and type(proof['application_bytes']) is int and proof['application_bytes'] == source.stat().st_size
            and type(proof['version_exit_code']) is int and proof['version_exit_code'] == 0 and proof['cli_version'] == 'bilicomics 0.1.25',
            'Missing display helper build or host integration qualification')
    profiles = proof['profiles']
    require(type(profiles) is list and len(profiles) == 2 and {entry['profile'] for entry in profiles} == {'scribe', 'pw6'}
            and all(type(entry['native_exit_code']) is int and entry['native_exit_code'] == 1 and type(entry['helper_exit_code']) is int
                    and entry['helper_exit_code'] == 1 and entry['missing_owned_snapshot_rejected'] is True and entry['helper_missing_permission_rejected'] is True
                    and entry['helper_cost_sidecar_created'] is False and entry['shelf_rendered'] is False and entry['native_framebuffer_available'] is False
                    and entry['native_input_available'] is False for entry in profiles), 'Unqualified ARM profile or unauthorized sidecar write')
    worker = CONTROL.plain(args.worker, 4 << 20)
    require(proof['worker']['unchanged_sources'] is True and proof['worker']['identical_to_install_zip'] is True
            and proof['worker']['bytes'] == worker.stat().st_size == 426672, 'Unqualified JPEG worker')
    with zipfile.ZipFile(FROZEN.STOCK_ZIP) as archive:
        name = 'bilicomics/app/kindle-jpeg-worker'
        require(archive.namelist().count(name) == 1 and archive.getinfo(name).file_size == worker.stat().st_size
                and archive.read(name) == worker.read_bytes(), 'Worker differs from installed bytes')
    with staged.open('rb') as candidate:
        require(not staged.samefile(source) and FROZEN.matched_stream(candidate, source, staged.stat().st_size), 'Application differs from independent qualified build')
    return stock


@cache
def collector():
    return BASE.load_module(HERE / 'collect_display.py')


THREE.SCHEMA, THREE.MARK, THREE.ADMITTED = SCHEMA, MARK, ADMITTED
THREE.INPUTS = (*THREE.INPUTS, 'host_qualification')
THREE.binding, THREE.input_receipt, THREE.case_files = binding, input_receipt, case_files
THREE.inputs, THREE.handoff_guard, THREE.collector = qualified_inputs, stock_process_guard, collector


if __name__ == '__main__':
    try:
        THREE.main()
    except (CONTROL.RecoveryRequired, CONTROL.SampleUnavailable, CONTROL.OperatorError) as error:
        print(error, file=sys.stderr)
        sys.exit(3 if isinstance(error, CONTROL.RecoveryRequired) else 4 if isinstance(error, CONTROL.SampleUnavailable) else 2)
    except (Exception, KeyboardInterrupt):
        sys.exit('Display sampling stopped; check the original private plan and visit. Raw diagnostics withheld.')
