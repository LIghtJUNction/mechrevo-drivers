#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Actual GPL TCC 3.0.10 API/N-API/TS code; only hardware and daemon inputs mocked.

Optional addon/service gates require explicit local TCC_NODE_ADDON_API,
TCC_NODE_INCLUDE, TCC_TYPESCRIPT_MODULE and TCC_NODE. Never installs dependencies.
Frozen original negative controls must fail. Production daemon is never started.
"""
import hashlib,json,os,shutil,subprocess,tempfile,unittest
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
FIXTURE=ROOT/'tests/fixtures/tcc_3_0_10'
PATCH=ROOT/'packaging/arch/tuxedo-control-center-mechrevo/0001-fan-io-error-contract.patch'

class TccFanContract(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.manifest=json.loads((FIXTURE/'provenance.json').read_text())
        for name,record in cls.manifest['files'].items():
            data=(FIXTURE/'upstream'/name).read_bytes()
            assert len(data)==record['size'] and hashlib.sha256(data).hexdigest()==record['sha256']
        assert hashlib.sha256(PATCH.read_bytes()).hexdigest()==cls.manifest['patch_sha256']
        cls.tmp=tempfile.TemporaryDirectory(prefix='tcc-contract-',dir=os.environ.get('MECHREVO_TEST_TMPDIR'))
        cls.base=Path(cls.tmp.name);cls.legacy=cls.base/'legacy';cls.patched=cls.base/'patched'
        shutil.copytree(FIXTURE/'upstream',cls.legacy);shutil.copytree(FIXTURE/'upstream',cls.patched)
        subprocess.run(['patch','-p1','--fuzz=0','--batch','--forward','-i',str(PATCH)],cwd=cls.patched,check=True,stdout=subprocess.PIPE,stderr=subprocess.PIPE)
    @classmethod
    def tearDownClass(cls):cls.tmp.cleanup()
    def run_cmd(self,args,success=True):
        r=subprocess.run([str(x) for x in args],stdout=subprocess.PIPE,stderr=subprocess.STDOUT,text=True,timeout=120)
        if success:self.assertEqual(r.returncode,0,r.stdout)
        else:self.assertNotEqual(r.returncode,0,r.stdout)
        return r.stdout
    def compile(self,source,out,addon=False):
        args=['g++','-std=c++17','-fexceptions','-I',source/'src/native-lib',FIXTURE/'mock_io.cpp']
        if addon:
            args+=['-shared','-fPIC','-DNAPI_CPP_EXCEPTIONS','-I',os.environ['TCC_NODE_INCLUDE'],'-I',os.environ['TCC_NODE_ADDON_API'],source/'src/native-lib/tuxedo_io_napi.cc','-ludev']
        else:args += [FIXTURE/'api_main.cpp']
        args+=['-Wl,--wrap=open','-Wl,--wrap=ioctl','-Wl,--wrap=close','-o',out]
        self.run_cmd(args)
    def test_actual_uniwill_api_and_original_negative_control(self):
        for name,source in [('legacy',self.legacy),('patched',self.patched)]:
            binary=self.base/('api-'+name);self.compile(source,binary);self.run_cmd([binary],success=name=='patched')
    def test_actual_native_addon_and_original_negative_control(self):
        if not all(os.environ.get(k) for k in ('TCC_NODE_INCLUDE','TCC_NODE_ADDON_API','TCC_NODE')):self.skipTest('explicit existing Node/N-API headers required; no automatic install')
        for name,source in [('legacy',self.legacy),('patched',self.patched)]:
            addon=self.base/(name+'.node');self.compile(source,addon,addon=True);self.run_cmd([os.environ['TCC_NODE'],FIXTURE/'native_test.cjs',addon],success=name=='patched')
    def test_actual_generated_service_cache_retry_and_original_negative_control(self):
        if not all(os.environ.get(k) for k in ('TCC_TYPESCRIPT_MODULE','TCC_NODE')):self.skipTest('explicit existing locked TypeScript/Node required; no automatic install')
        for name,source in [('legacy',self.legacy),('patched',self.patched)]:
            self.run_cmd([os.environ['TCC_NODE'],FIXTURE/'service_test.cjs',os.environ['TCC_TYPESCRIPT_MODULE'],source],success=name=='patched')

    def test_old_worker_rejected_with_patched_io_boundary(self):
        if not all(os.environ.get(k) for k in ('TCC_TYPESCRIPT_MODULE','TCC_NODE')):self.skipTest('explicit existing locked TypeScript/Node required; no automatic install')
        hybrid=self.base/'old-worker-with-patched-io';shutil.copytree(self.patched,hybrid)
        name='src/service-app/classes/FanControlWorker.ts';shutil.copy2(self.legacy/name,hybrid/name)
        self.run_cmd([os.environ['TCC_NODE'],FIXTURE/'service_test.cjs',os.environ['TCC_TYPESCRIPT_MODULE'],hybrid],success=False)

if __name__=='__main__':unittest.main()
