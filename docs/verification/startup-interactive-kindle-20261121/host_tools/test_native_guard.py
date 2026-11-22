"""Exercise the unprivileged control logic with a synthetic process tree and a signal sink."""

import os
from pathlib import Path
import subprocess
import tempfile
import unittest


RUN = 'bili-shelf-0123456789abcdef0123456789abcdef'
BOOT = '01234567-89ab-cdef-0123-456789abcdef'


class GuardTest(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.proc, media, private, tools = (self.root / name for name in ('proc', 'media', 'private', 'bin'))
        self.tmp = self.root / 'tmp'
        self.stage = media / RUN
        self.lock = private / 'bilicomics-launcher.lock'
        for directory in (self.proc, self.stage, private, tools):
            directory.mkdir(parents=True)
        self.tmp.symlink_to(private)
        self.bin = tools
        self.env = dict(os.environ, PATH=str(tools) + ':' + os.defpath)
        self.signal = self.root / 'signals'
        self.tool('lipc-get-prop', 'case "$*" in "com.lab126.appmgrd activeApp") echo com.lab126.KPPMainApp;; "-s com.lab126.powerd state") echo active;; *) exit 2;; esac')
        self.tool('status', 'test "$*" = statusbar && echo statusbar start/running')
        self.tool('pidof', 'test "$*" = awesome && echo 101')
        self.tool('sleep', 'exit 0')
        self.tool('signal-sink', f'test "$*" = "-TERM 702" && printf "%s\\n" "$*" >> "{self.signal}"')
        firmware = self.root / 'firmware'
        firmware.write_text('Kindle 5.19.5 (fixture)\n')
        fb = self.root / 'fb0'
        fb.mkdir()
        (fb / 'bits_per_pixel').write_text('8\n')
        (fb / 'virtual_size').write_text('1872,4960\n')
        boot_dir = self.proc / 'sys/kernel/random'
        boot_dir.mkdir(parents=True)
        (boot_dir / 'boot_id').write_text(BOOT + '\n')
        wm = self.proc / '101'
        wm.mkdir()
        (wm / 'status').write_text('State:\tS (sleeping)\n')
        (wm / 'exe').symlink_to('/bin/true')
        (self.stage / 'run-id').write_text(RUN + '\n')
        self.source = Path(__file__).with_name('native_guard.sh').read_text(encoding='ascii')
        self.assertEqual(self.source.count('kill -TERM "$worker"'), 1)
        for before, after in (
            ('stage=/mnt/us/$run', f'stage={media}/$run'), ('tmp=/tmp\n', f'tmp={self.tmp}\n'),
            ('alias=/var/tmp\n', f'alias={private}\n'), ('proc=/proc\n', f'proc={self.proc}\n'),
            ('/etc/prettyversion.txt', str(firmware)), ('/sys/class/graphics/fb0/', str(fb) + '/'),
            ('[ -c /dev/fb0 ]', '[ -c /dev/null ]'), ('kill -TERM "$worker"', 'signal-sink -TERM "$worker"'),
            ('remaining=24;', 'remaining=2;'),
        ):
            self.assertIn(before, self.source)
            self.source = self.source.replace(before, after)
        uid = str(os.getuid())
        for before, after in ((" = 0 ]", f" = {uid} ]"), (" = 0:1 ]", f" = {uid}:1 ]"),
                              ('in 0:700:*|0:2700:*)', f'in {uid}:700:*|{uid}:2700:*)')):
            self.source = self.source.replace(before, after)

    def tool(self, name, body):
        path = self.bin / name
        path.write_text('#!/bin/sh\nset -eu\n' + body + '\n')
        path.chmod(0o700)

    def guard(self, operation, *, success, output=''):
        result = subprocess.run(['/bin/sh', '-s', '--', RUN, operation], input=self.source, text=True,
                                stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=5, env=self.env)
        self.assertEqual(result.returncode == 0, success, result.stderr)
        self.assertEqual(result.stdout, BOOT + '\n' if success and operation == 'native' else output)

    def claim(self):
        (self.stage / 'picked').mkdir()
        (self.stage / 'picked/entry').write_text(f'{RUN} 701 150\n')

    def process(self, pid, parent, group, session, birth, args, *, binary='/bin/sh'):
        path = self.proc / str(pid)
        path.mkdir(exist_ok=True)
        (path / 'stat').write_text(f'{pid} (task (with space)) S {parent} {group} {session} ' + '0 ' * 15 + f'{birth}\n')
        (path / 'cmdline').write_bytes(b'\0'.join(os.fsencode(value) for value in args) + b'\0')
        if not (path / 'exe').is_symlink():
            (path / 'exe').symlink_to(Path(binary).resolve())

    def running(self):
        self.claim()
        self.process(701, 501, 500, 500, 150, ['sh', self.stage / 'launch.sh', '--library'])
        self.process(702, 701, 702, 702, 165, ['sh', self.stage / 'launch.sh', '--detached'])
        self.lock.mkdir(mode=0o700)
        nonce = 'a' * 32
        for name, value in (('pid', '702'), ('generation', '1:' + nonce), ('nonce', nonce)):
            (self.lock / name).write_text(value + '\n')

    def test_native_health_and_peer_lock(self):
        self.guard('native', success=True)
        (self.tmp / 'aidoku-launcher.lock').mkdir()
        self.guard('native', success=False)

    def test_no_pick_and_retired_pick(self):
        self.guard('wait', success=False)
        self.guard('stop', success=False)
        self.assertFalse(self.signal.exists())
        self.claim()
        self.guard('wait', success=True)
        self.guard('stop', success=True)
        self.process(701, 1, 1, 1, 200, ['foreign'], binary='/bin/true')
        self.guard('wait', success=True)

    def test_live_child_or_stage_binary_blocks_return(self):
        self.claim()
        self.process(708, 1, 708, 708, 203, ['sh', self.stage / 'launch.sh', '--detached'])
        self.guard('wait', success=False)
        self.assertFalse(self.signal.exists())
        (self.proc / '708/cmdline').write_bytes(b'foreign\0')
        exe = self.proc / '708/exe'
        exe.unlink()
        exe.symlink_to(self.stage / 'app/bilicomics')
        self.guard('wait', success=False)

    def test_stop_checks_identity_and_sends_only_one_term(self):
        self.running()
        self.guard('wait', success=False)
        self.assertFalse(self.signal.exists())
        self.guard('stop', success=False)
        self.assertEqual(self.signal.read_text(), '-TERM 702\n')
        self.signal.unlink()
        for leaf, content in ((self.lock / 'generation', b'2:replaced\n'),
                              (self.proc / '702/cmdline', b'sh\0/mnt/us/bilicomics/launch.sh\0--detached\0'),
                              (self.stage / 'picked/entry', f'{RUN} 701 152\n'.encode())):
            original = leaf.read_bytes()
            leaf.write_bytes(content)
            self.guard('stop', success=False)
            self.assertFalse(self.signal.exists())
            leaf.write_bytes(original)
        self.process(702, 700, 702, 702, 165, ['sh', self.stage / 'launch.sh', '--detached'])
        self.guard('stop', success=False)
        self.assertFalse(self.signal.exists())

    def observe_app_then_retire(self, *, mismatch=False):
        self.running()
        (self.stage / 'app').mkdir()
        binary = self.stage / 'app/bilicomics'
        binary.write_bytes(b'isolated executable identity only')
        args = [binary, '--kindle', '--device', 'scribe', '--data-dir', self.stage / 'data', '--demo', '--offline',
                '--shelf-render-trace', self.stage / 'data' / ('another-run.jsonl' if mismatch else RUN + '.jsonl'), '--exit-after-ms', '2500']
        self.process(703, 702, 702, 702, 180, ['sh', self.stage / 'launch.sh', '--detached'])
        self.process(704, 703, 702, 702, 181, args, binary=binary)
        (self.lock / 'application-pid').write_text('704\n')
        (self.proc / '704/environ').write_bytes(b'\0'.join(os.fsencode(value) for value in (
            'BILICOMICS_HOME=' + str(self.stage), 'BILICOMICS_LIFECYCLE_DIR=' + str(self.tmp / self.lock.name),
            'BILICOMICS_LAUNCH_GENERATION=1', 'BILICOMICS_LAUNCH_TOKEN=' + 'a' * 32)) + b'\0')
        retiring = ' '.join(f'"{self.proc / str(pid)}"' for pid in (701, 702, 703, 704))
        self.tool('sleep', f'for item in {retiring} "{self.lock}"; do rm -f "$item"/*; rmdir "$item"; done')
        self.guard('wait', success=True, output='' if mismatch else 'app_pid=704\n')
        self.assertFalse(self.signal.exists())

    def test_wait_witnesses_isolated_app_pid_outside_its_trace(self):
        self.observe_app_then_retire()

    def test_wait_does_not_bind_different_trace_path(self):
        self.observe_app_then_retire(mismatch=True)


if __name__ == '__main__':
    unittest.main()
