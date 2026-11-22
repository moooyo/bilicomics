"""One untimed desktop suspend/resume attempt across the research post-submit turn."""

import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import time


def require(condition, message):
    if not condition:
        raise ValueError(message)


def records(path, pid):
    if not path.exists():
        return []
    data = path.read_bytes()
    require(len(data) <= 16 * 1024 * 1024, 'Unexpected trace size')
    rows = [json.loads(line) for line in data.splitlines(keepends=True) if line.endswith(b'\n')]
    require(all(row['process_id'] == pid for row in rows), 'Trace contains a different process')
    return rows


def events(rows, name):
    return [row for row in rows if (row.get('kind'), row.get('stage')) == ('startup', name)]


def wait_for(process, performance, name):
    deadline = time.monotonic() + 10
    while True:
        matches = events(records(performance, process.pid), name)
        if matches:
            require(len(matches) == 1, f'Repeated {name}')
            return matches[0]
        require(process.poll() is None and time.monotonic() < deadline, f'No {name} in the running process')
        time.sleep(0.0005)


def run(output, result):
    binary = Path('/work/research')
    require(Path.cwd() == Path('/work') and binary.is_file() and not binary.is_symlink(), 'Expected a private research ELF')
    require({line.split(':', 1)[0].strip() for line in Path('/proc/net/dev').read_text().splitlines()[2:]} == {'lo'},
            'This qualification requires loopback-only networking')
    require(hasattr(os, 'pidfd_open') and hasattr(signal, 'pidfd_send_signal'), 'A stable child signal handle is required')
    os.close(os.pidfd_open(os.getpid()))
    performance, life = output / 'performance.jsonl', output / 'lifecycle.jsonl'
    environment = dict(os.environ, BILICOMICS_DESKTOP_PRESENT_PROBE='wake', SLINT_BACKEND='winit-software',
                       WINIT_UNIX_BACKEND='x11', SLINT_SCALE_FACTOR='2')
    command = [binary, '--screen', 'shelf', '--offline', '--device', 'scribe', '--data-dir', output / 'empty-profile',
               '--performance-trace', performance, '--lifecycle-trace', life, '--exit-after-ms', '200']
    with (output / 'application.log').open('xb') as log:
        process = subprocess.Popen(command, env=environment, stdout=log, stderr=subprocess.STDOUT)
        handle = os.pidfd_open(process.pid)
        result['pid'] = process.pid

        def send(sig):
            require(process.poll() is None and Path(f'/proc/{process.pid}/exe').samefile(binary), 'Child identity changed')
            before = time.monotonic_ns()
            signal.pidfd_send_signal(handle, sig)
            result['signals'][sig.name] = [before, time.monotonic_ns()]

        try:
            full = wait_for(process, performance, 'desktop_software_full_submit')
            require(full['data']['mode'] == 'wake' and full['data']['ui_thread'] and
                    (full['data']['width'], full['data']['height']) == (1860, 2480), 'Not a full Scribe software submit')
            send(signal.SIGUSR1)
            turn = wait_for(process, performance, 'desktop_post_submit_turn')
            result['turn'] = turn['data']
            send(signal.SIGUSR2)
            result['exit_code'] = process.wait(15)
        finally:
            try:
                if process.poll() is None:
                    result['required_cleanup'] = True
                    if 'SIGUSR1' in result['signals'] and 'SIGUSR2' not in result['signals']:
                        send(signal.SIGUSR2)
                    send(signal.SIGTERM)
                    try:
                        process.wait(3)
                    except subprocess.TimeoutExpired:
                        send(signal.SIGKILL)
                        process.wait(3)
            finally:
                os.close(handle)

    perf, lifecycle = records(performance, process.pid), records(life, process.pid)
    context, = events(perf, 'state-prepare-context')
    require(context['data']['offline'] is True and context['data']['demo'] is False, 'Not an ordinary offline startup')
    marks = {}
    for label in ('desktop_software_full_submit', 'desktop_post_submit_turn', 'workers_started', 'font_worker_queued',
                  'state_worker_queued', 'ui_worker_gate', 'ui_ready'):
        hit, = events(perf, label)
        require(hit['thread_id'] == process.pid, f'{label} came from a different thread')
        marks[label] = hit['monotonic_ns']
    for label in ('suspend', 'resume', 'ready'):
        hit, = [row for row in lifecycle if row['event'] == label]
        marks[label] = hit['monotonic_ns']
        if label != 'ready':
            require(hit['checkpoint_saved'] is True and hit['booting'] is True and hit['suspended'] is (label == 'suspend'), f'Wrong {label} state')
        else:
            result['ready_route'] = hit['route']
            require(hit['route'] == 'shelf', 'The ordinary guest startup did not reach Shelf')
    result['marks_ns'] = marks
    gate, = events(perf, 'ui_worker_gate')
    result['gate_source'] = gate['data']['source']
    check = result['turn']
    require(check['mode'] == 'wake' and check['delay_ms'] == 16 and check['suspended'] is True and check['will_drive'] is False
            and check['workers_started_before'] is False and all(check[key] is True for key in ('booting', 'ui_thread', 'window_alive', 'driver_available'))
            and all(check[key] is False for key in ('cancelled', 'quitting')), 'The real turn did not isolate the suspended guard')
    require(marks['desktop_software_full_submit'] <= result['signals']['SIGUSR1'][0] <= marks['suspend']
            <= marks['desktop_post_submit_turn'] <= result['signals']['SIGUSR2'][0] <= marks['resume']
            <= marks['workers_started'] <= marks['font_worker_queued'] <= marks['state_worker_queued'] <= marks['ui_worker_gate']
            <= marks['ui_ready'] <= marks['ready'] and result['gate_source'] in ('runtime_wakeup', 'maintenance_timer')
            and gate['data']['workers_admitted_by_ui'] is True and result['exit_code'] == 0
            and not result['required_cleanup'] and not any(row['event'] == 'quit' for row in lifecycle), 'Unexpected recovery or gate order')


if __name__ == '__main__':
    os.umask(0o077)
    output = Path('/work/suspend-case')
    output.mkdir()
    result = {'scope': 'One untimed isolated desktop guest process. Not a Kindle test or a timing sample.',
              'passed': False, 'required_cleanup': False, 'signals': {}}
    try:
        run(output, result)
        result['passed'] = True
    except Exception as error:
        result['error'] = str(error)
    with (output / 'receipt.json').open('x') as stream:
        json.dump(result, stream, indent=2)
        stream.write('\n')
    print(json.dumps(result))
    sys.exit(0 if result['passed'] else 1)
