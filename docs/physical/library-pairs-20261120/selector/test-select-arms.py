"""Host-only synthetic directory-switch fixtures; no Kindle transport or lease calls."""

import fcntl
import hashlib
import os
from pathlib import Path
import shutil
import socket
import stat
import subprocess
import sys
import tempfile
import unittest


WORK = Path(os.environ['BILI_PAIR_WORK'])
RUN = 'lib-p-abc00000'
LEAVES = ('launch.sh', 'app/bilicomics', 'app/kindle-jpeg-worker')


class SelectTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        history_name = os.environ.get('BILI_PAIR_HISTORY')
        if not history_name and os.environ.get('BILI_PAIR_SELECTOR_ONLY') != '1':
            raise RuntimeError('Missing click history outside the standalone selector fixture')
        if history_name:
            history = Path(history_name)
            if (not history.is_absolute() or not history.is_dir() or history.resolve(strict=True) != history
                    or history == WORK or WORK in history.parents or history in WORK.parents):
                raise RuntimeError('Keep selector fixtures separate from historical inputs')
        if (sys.platform != 'linux' or os.geteuid() != 0 or socket.gethostname() != os.environ['BILI_PAIR_TEST_HOST']
                or not WORK.is_absolute() or WORK.resolve(strict=True) != WORK or not WORK.is_dir()
                or WORK != Path.cwd() or Path(__file__).resolve().parent != WORK or WORK.stat().st_uid != 0
                or stat.S_IMODE(WORK.stat().st_mode) != 0o700):
            raise RuntimeError('Run synthetic tests only in the isolated test-env workspace')
        cls.temp = tempfile.TemporaryDirectory(dir=WORK, prefix='selector-fixture-')
        cls.base = Path(cls.temp.name)
        cls.media, cls.private, cls.locks = (cls.base / p for p in ('media', 'var', 'locks'))
        cls.bin = cls.base / 'tools'
        cls.bin.mkdir()
        (cls.bin / 'lipc-get-prop').write_text('''#!/bin/sh
set -eu
test "$#" = 2 && test "$1" = com.lab126.appmgrd && test "$2" = activeApp || exit 1
if test -n "${TEST_RACE_MARK:-}"; then
    if test -f "$TEST_RACE_MARK"; then mkdir "$TEST_RACE_TARGET"; echo contents >"$TEST_RACE_TARGET/survives"; else : >"$TEST_RACE_MARK"; fi
fi
printf '%s\\n' "${TEST_FRONT:-com.lab126.KPPMainApp}"
''')
        (cls.bin / 'lipc-get-prop').chmod(0o755)
        subprocess.run(['cc','-std=c11','-Wall','-Wextra','-Werror','-O2',f'-DPAIR_MEDIA="{cls.media}"',
                        f'-DPAIR_TMP="{cls.private}"',str(WORK/'pair-fs.c'),'-o',str(cls.bin/'pair-fs')],check=True)
        cls.template=(WORK/'select-arms.in.sh').read_text()
        subprocess.run(['sh','-n'],input=cls.template.encode(),check=True)

    @classmethod
    def tearDownClass(cls):
        cls.temp.cleanup()

    def setUp(self):
        self.addCleanup(self.clean_roots)
        for root in (self.media,self.private,self.locks): root.mkdir()
        self.stage=self.media / ('bilicomics-'+RUN)
        self.runtime=self.private / ('bilicomics-'+RUN)
        self.home=self.media / 'bilicomics'
        for root in (self.stage,self.runtime):
            root.mkdir(mode=0o700)
            (root/'owner').write_text(RUN+'\n')
        self.sentinel=self.media/'unrelated-user-file'
        self.sentinel.write_text('leave intact\n')
        (self.stage/'tools').mkdir()
        shutil.copyfile(self.bin/'pair-fs',self.stage/'tools/pair-fs')
        (self.stage/'tools/pair-fs').chmod(0o755)
        self.selector=self.stage/'tools/select-arms.sh'
        content=self.template.replace('@RUN@',RUN).replace('/mnt/us',str(self.media)).replace('/var/tmp',str(self.private)).replace('/tmp/bilicomics-launcher.lock',str(self.locks/'production'))
        self.expect={}
        for arm,root in [('A',self.home),('B',self.stage/'park-B')]:
            (root/'app').mkdir(parents=True)
            (root/'.native-library-owner').write_text(RUN+'\n')
            (root/'.bench-arm').write_text(arm+'\n')
            digests=[]
            for name,key in zip(LEAVES,('L','APP','W')):
                blob=(arm+':'+name+'\n').encode()
                (root/name).write_bytes(blob)
                (root/name).chmod(0o755)
                digest=hashlib.sha256(blob).hexdigest()
                digests.append(digest)
                content=content.replace(f'@{arm}_{key}@',digest)
            self.expect[arm]=' '.join([arm,*digests])+'\n'
        self.selector.write_text(content)
        self.selector.chmod(0o755)

    def clean_roots(self):
        if self.sentinel.exists(): self.assertEqual(self.sentinel.read_text(),'leave intact\n')
        for d in (self.media,self.private,self.locks):
            if d.is_dir(): shutil.rmtree(d)

    def call(self,arm,mode,ok=True,extra=None):
        env=dict(PATH=str(self.bin)+':'+os.defpath, LC_ALL='C')
        env.update(extra or {})
        p=subprocess.run(['sh',str(self.selector),arm,mode],capture_output=True,env=env,text=True,timeout=10)
        if ok:
            self.assertEqual(p.returncode,0,p.stderr)
            self.assertEqual(p.stdout,self.expect[arm]); self.assertEqual(p.stderr,'')
        else:
            self.assertNotEqual(p.returncode,0); self.assertEqual(p.stdout,'')
        return p

    def test_roundtrip_probe_and_verify(self):
        self.call('A','probe'); self.call('A','select'); self.call('B','select'); self.call('B','verify')
        self.call('B','select'); self.call('A','restore'); self.call('A','verify')
        self.assertFalse((self.stage/'park-A').exists()); self.assertTrue((self.stage/'park-B').is_dir())
        self.assertFalse(list(self.stage.glob('.pair-capability.*')))

    def test_same_payload_warmups_and_six_alternating_pairs(self):
        parked = self.stage / 'park-B'
        for name in LEAVES:
            shutil.copyfile(self.home / name, parked / name)
        selector = self.selector.read_text()
        for original, common in zip(self.expect['B'].split()[1:], self.expect['A'].split()[1:], strict=True):
            selector = selector.replace(original, common)
        self.selector.write_text(selector)
        self.expect['B'] = 'B' + self.expect['A'][1:]
        self.call('A', 'probe')
        for arm in 'AB' + 'ABBA' * 3:
            self.call(arm, 'select')
            self.call(arm, 'verify')
            self.assertEqual((self.home / '.bench-arm').read_text(), arm + '\n')
        self.call('A', 'restore')
        self.call('A', 'verify')

    def test_both_parked_requires_explicit_a_recovery(self):
        self.home.rename(self.stage/'park-A')
        self.call('A','select',False); self.call('B','select',False); self.call('B','restore',False)
        self.call('A','restore'); self.call('A','verify')

    def test_interruption_after_move_recovers_a(self):
        self.call('B','select'); self.home.rename(self.stage/'park-B')
        self.call('A','restore'); self.call('A','verify')

    def test_extra_park_and_unknown_target_never_replaced(self):
        unknown=self.stage/'park-A'; unknown.mkdir(); (unknown/'survives').touch()
        self.call('B','select',False); self.assertTrue((unknown/'survives').is_file())
        unknown.rename(self.stage/'park-C'); self.call('B','select',False)
        self.assertEqual((self.home/'.bench-arm').read_text(),'A\n')

    def test_destination_created_after_hashes_is_not_replaced_or_nested(self):
        unknown=self.stage/'park-A'
        self.call('B','select',False,{'TEST_RACE_MARK':str(self.private/'race-marker'),
                                    'TEST_RACE_TARGET':str(unknown)})
        self.assertEqual([x.name for x in unknown.iterdir()],['survives'])
        self.assertEqual((unknown/'survives').read_text(),'contents\n')
        self.assertEqual((self.home/'.bench-arm').read_text(),'A\n')

    def test_unknown_canonical_never_replaced(self):
        self.home.rename(self.stage/'park-A'); self.home.mkdir()
        (self.home/'not-bili').write_text('user data')
        self.call('A','restore',False); self.assertEqual((self.home/'not-bili').read_text(),'user data')

    def test_wrong_owner_or_inactive_hash(self):
        other=self.stage/'park-B'
        (other/'.native-library-owner').write_text(RUN+'\n\n'); self.call('A','select',False)
        (other/'.native-library-owner').write_text(RUN+'\n'); (other/LEAVES[1]).write_text('wrong')
        self.call('A','select',False); self.assertEqual((self.home/'.bench-arm').read_text(),'A\n')

    def test_link_inside_bundle_and_escaped_bundle(self):
        other=self.stage/'park-B'; exe=other/LEAVES[0]; saved=self.media/'saved'; exe.rename(saved); exe.symlink_to(saved)
        self.call('B','select',False); self.assertTrue(exe.is_symlink())
        exe.unlink(); saved.rename(exe); away=self.media/'external'; other.rename(away); other.symlink_to(away)
        self.call('B','select',False); self.assertTrue(other.is_symlink())

    def test_null_link_at_target_is_not_absence(self):
        stale=self.stage/'park-A'; stale.symlink_to(self.media/'missing')
        self.call('B','select',False); self.assertTrue(stale.is_symlink())

    def test_unexpected_hidden_and_hardlink_fail(self):
        extra=self.home/'app/.unknown'; extra.write_text('unknown')
        self.call('B','select',False); extra.unlink()
        os.link(self.home/LEAVES[1], self.media/'extra-link')
        self.call('B','select',False)

    def test_kernel_lock_and_read_only_verification(self):
        with (self.runtime/'owner').open() as owner:
            fcntl.flock(owner,fcntl.LOCK_EX|fcntl.LOCK_NB)
            self.call('B','select',False); self.call('A','restore',False)
            self.call('A','verify')
        self.call('B','select'); self.call('A','restore')

    def test_native_app_or_existing_lock_refuses(self):
        self.call('B','select',False,{'TEST_FRONT':'not-native'})
        (self.locks/'production').symlink_to(self.media/'missing'); self.call('B','select',False)

    def test_forged_locked_cli_does_not_move(self):
        p=subprocess.run(['sh',str(self.selector),'locked','B','select'],capture_output=True,
                         env=dict(PATH=str(self.bin)+':'+os.defpath, LC_ALL='C'))
        self.assertNotEqual(p.returncode,0); self.assertEqual((self.home/'.bench-arm').read_text(),'A\n')

    def test_both_parked_with_bad_hash_is_not_recovery(self):
        self.home.rename(self.stage/'park-A'); (self.stage/'park-B'/LEAVES[0]).write_text('not trusted')
        self.call('A','restore',False); self.assertFalse(self.home.exists())

    def test_missing_tool_read_only_restore_proof(self):
        (self.stage/'tools/pair-fs').unlink(); self.call('A','verify'); self.call('B','select',False)


if __name__=='__main__':
    unittest.main(verbosity=2)
