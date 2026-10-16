"""Read-only archive verification against real temporary files and actual mutation boundaries."""
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import stat
import subprocess
import sys
import tempfile
import unittest

SCRIPT = Path(__file__).resolve().parents[1] / 'scripts/verify_archive.py'
spec = importlib.util.spec_from_file_location('archive_verifier', SCRIPT)
archive = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = archive
spec.loader.exec_module(archive)


class VerifyArchive(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(dir=os.environ.get('MECHREVO_TEST_TMPDIR'))
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.payload = self.root / 'data.bin'
        self.payload.write_bytes(b'archive payload\x00\xff\n')
        self.payload.chmod(0o640)
        self.manifest = self.root / 'manifest.json'

    def record(self):
        s = self.payload.stat()
        return {'path': 'data.bin', 'size': s.st_size,
                'sha256': hashlib.sha256(self.payload.read_bytes()).hexdigest(),
                'mode': stat.S_IMODE(s.st_mode), 'uid': s.st_uid, 'gid': s.st_gid}

    def write_manifest(self, records=None, key='files', raw=None):
        self.manifest.write_text(raw if raw is not None else json.dumps({key: records or [self.record()]}))
        return hashlib.sha256(self.manifest.read_bytes()).hexdigest()

    def invoke(self, key='files', pin=None, extra=()):
        if pin is None:
            pin = hashlib.sha256(self.manifest.read_bytes()).hexdigest()
        p = subprocess.run([sys.executable, '-B', str(SCRIPT), '--manifest', str(self.manifest),
                            '--records-key', key, '--expected-sha256', pin, *extra], capture_output=True, text=True)
        d = json.loads(p.stdout)
        self.assertEqual(p.returncode, d['exit_code'], p.stdout + p.stderr)
        return d

    def assert_failed(self, result):
        self.assertEqual(result['outcome'], 'failed')
        self.assertTrue(result['errors'])

    def test_real_files_and_payloads_schema_success_and_coverage(self):
        for key, permissions in [('files', 0o640), ('payloads', '0o640'), ('files', '0640'), ('payloads', '640')]:
            with self.subTest(key=key, mode=permissions):
                record = self.record(); record['mode'] = permissions
                if key=='files': record['mtime_ns'] = self.payload.stat().st_mtime_ns
                record['source'] = '/ignored/source/metadata'
                self.write_manifest([record],key)
                d = self.invoke(key)
                self.assertEqual((d['outcome'],d['verified'],d['bytes_verified']), ('passed',1,self.payload.stat().st_size))
                self.assertEqual(d['expected_sha256'],hashlib.sha256(self.manifest.read_bytes()).hexdigest())
                self.assertEqual(d['observed_sha256'],d['expected_sha256']);self.assertEqual(d['records_key'],key)
                self.assertEqual(d['mtime_requested'], int(key=='files'))
                self.assertEqual(d['mtime_verified'], int(key=='files'))
                self.assertFalse(d['unlisted_contents_verified']);self.assertFalse(d['child_command_or_tracking_verified'])
        (self.root/'unlisted.txt').write_text('not listed, not claimed')
        self.assertEqual(self.invoke('payloads')['outcome'], 'passed')

    def test_external_manifest_pin_and_explicit_records_key_required(self):
        self.write_manifest(key='payloads')
        self.assert_failed(self.invoke('files'))
        self.assert_failed(self.invoke('payloads','0'*64))
        p = subprocess.run([sys.executable,'-B',str(SCRIPT),'--manifest',str(self.manifest),'--records-key','payloads'],capture_output=True,text=True)
        self.assertEqual(p.returncode,1);self.assertEqual(json.loads(p.stdout)['outcome'],'failed')

    def test_sha_size_mode_uid_gid_mtime_mismatch_fail(self):
        cases={'sha256':'0'*64,'size':0,'mode':0o600,'uid':self.payload.stat().st_uid+1,
               'gid':self.payload.stat().st_gid+1,'mtime_ns':self.payload.stat().st_mtime_ns+1}
        for field,value in cases.items():
            with self.subTest(field=field):
                r=self.record();r[field]=value;self.write_manifest([r]);self.assert_failed(self.invoke())

    def test_invalid_record_types_ranges_and_paths_fail(self):
        cases=[('size',True),('size',-1),('size',2**63),('size',1.0),('size','17'),
               ('uid',False),('uid',-1),('gid',2**32-1),('mtime_ns',None),('mtime_ns',True),
               ('mode',True),('mode',0o100640),('mode','888'),('mode','-1'),
               ('sha256',True),('sha256','x'*64),('path','/absolute'),('path','../outside'),
               ('path','a/../data.bin'),('path','./data.bin'),('path','a//b'),('path','a\\b'),('path','')]
        for field,value in cases:
            with self.subTest(field=field,value=value):
                r=self.record();r[field]=value;self.write_manifest([r]);self.assert_failed(self.invoke())
        for value in [None,[],True,17]:
            self.write_manifest([value]);self.assert_failed(self.invoke())

    def test_duplicate_keys_paths_and_nonfinite_json_rejected(self):
        r=self.record();self.write_manifest([r,r]);self.assert_failed(self.invoke())
        for raw in ['{"files":[],"files":[]}', '{"files":['+json.dumps(r)[:-1]+',"size":17}]}', '{"files":['+json.dumps(r)+'],"extra":NaN}']:
            self.write_manifest(raw=raw);self.assert_failed(self.invoke())
        self.write_manifest(raw='{"files":[]}');self.assert_failed(self.invoke())

    def test_leaf_parent_manifest_symlinks_and_hardlinks_rejected(self):
        r=self.record();self.write_manifest([r]);original=self.root/'original';self.payload.rename(original)
        self.payload.symlink_to(original);self.assert_failed(self.invoke());self.payload.unlink()
        os.link(original,self.payload);self.assert_failed(self.invoke());self.payload.unlink();original.rename(self.payload)
        directory=self.root/'real';directory.mkdir();(directory/'copy').write_bytes(self.payload.read_bytes());(directory/'copy').chmod(0o640)
        (self.root/'linked').symlink_to(directory,target_is_directory=True);r['path']='linked/copy';self.write_manifest([r]);self.assert_failed(self.invoke())
        r['path']='real';self.write_manifest([r]);self.assert_failed(self.invoke())
        self.write_manifest(); saved=self.root/'saved-manifest';self.manifest.rename(saved);self.manifest.symlink_to(saved);self.assert_failed(self.invoke());self.manifest.unlink()
        os.link(saved,self.manifest);self.assert_failed(self.invoke())

    def test_budget_limits_prevent_reads_and_type_confusion(self):
        self.write_manifest();self.assert_failed(self.invoke(extra=['--max-bytes','1']))
        self.assert_failed(self.invoke(extra=['--max-bytes','-1']))
        d=archive.verify(self.manifest,'files',hashlib.sha256(self.manifest.read_bytes()).hexdigest(),True)
        self.assert_failed(d.as_dict())
        self.manifest.write_bytes(b' '*(archive.MANIFEST_LIMIT+1));self.assert_failed(self.invoke())
        self.write_manifest(raw=json.dumps({'files':[{}]*(archive.RECORD_LIMIT+1)}));self.assert_failed(self.invoke())

    def test_real_growth_replacement_and_rewrite_during_read_fail(self):
        for mutation in ['grow','replace','rewrite','parent-replace']:
            with self.subTest(mutation=mutation):
                directory=self.root/mutation;directory.mkdir();p=directory/'payload';p.write_bytes(b'a'*100_000)
                fired=False
                def reader(fd,count):
                    nonlocal fired
                    chunk=os.read(fd,count)
                    if not fired:
                        fired=True
                        if mutation=='grow':
                            with p.open('ab') as out:out.write(b'growth')
                        elif mutation=='replace':
                            new=directory/'new';new.write_bytes(b'a'*100_000);os.replace(new,p)
                        elif mutation=='rewrite':
                            p.write_bytes(b'b'*100_000);s=p.stat();os.utime(p,ns=(s.st_atime_ns,s.st_mtime_ns+1))
                        else:
                            directory.rename(self.root/'old-parent');directory.mkdir();p.write_bytes(b'a'*100_000)
                    return chunk
                with self.assertRaisesRegex((ValueError,OSError),'grew|changed'):
                    archive.read_checked(p,100_000,100_000,reader=reader)
                self.assertTrue(fired)

    def test_optional_output_is_exclusive_and_payloads_readonly(self):
        pin=self.write_manifest(); before=self.payload.stat(); raw=self.payload.read_bytes();manifest=self.manifest.read_bytes()
        output=self.root/'result.json';d=self.invoke(pin=pin,extra=['--output',str(output)])
        self.assertEqual(json.loads(output.read_text()),d);self.assertEqual(stat.S_IMODE(output.stat().st_mode),0o600)
        for protected in [self.manifest,self.payload]:
            prior_bytes=protected.read_bytes();self.assert_failed(self.invoke(extra=['--output',str(protected)]));self.assertEqual(protected.read_bytes(),prior_bytes)
        prior=output.read_bytes();self.assert_failed(self.invoke(extra=['--output',str(output)]));self.assertEqual(output.read_bytes(),prior)
        (self.root/'out-link').symlink_to(output);self.assert_failed(self.invoke(extra=['--output',str(self.root/'out-link')]))
        self.assertEqual(self.payload.read_bytes(),raw);self.assertEqual(self.manifest.read_bytes(),manifest)
        after=self.payload.stat();self.assertEqual((before.st_ino,before.st_size,before.st_mtime_ns,before.st_ctime_ns),
                                                 (after.st_ino,after.st_size,after.st_mtime_ns,after.st_ctime_ns))


if __name__=='__main__':
    unittest.main()
