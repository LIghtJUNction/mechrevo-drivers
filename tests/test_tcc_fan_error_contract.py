#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Actual GPL TCC 3.0.10 API/N-API/TS code; only hardware and daemon inputs mocked.

Optional addon/service gates require explicit local TCC_NODE_ADDON_API,
TCC_NODE_INCLUDE, TCC_TYPESCRIPT_MODULE and TCC_NODE. Never installs dependencies.
Frozen original negative controls must fail. Production daemon is never started.
"""
import hashlib,json,os,shutil,subprocess,tempfile,unittest
from pathlib import Path
from tcc_test_log import write_command_log
ROOT=Path(__file__).resolve().parents[1]
FIXTURE=ROOT/'tests/fixtures/tcc_3_0_10'
PATCH=ROOT/'packaging/arch/tuxedo-control-center-mechrevo/0001-fan-io-error-contract.patch'
PATCH2=PATCH.with_name('0002-fan-lifecycle.patch')
PATCH3=PATCH.with_name('0003-fan-readback-diagnostics.patch')
PATCH4=PATCH.with_name('0004-installed-resource-paths.patch')
PATCH5=PATCH.with_name('0005-settings-read-failclosed.patch')
PATCH6=PATCH.with_name('0006-temperature-exception-invalidates-feedback.patch')
PATCH7=PATCH.with_name('0007-cpu-control-ownership.patch')

class TccFanContract(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.manifest=json.loads((FIXTURE/'provenance.json').read_text())
        for name,record in cls.manifest['files'].items():
            data=(FIXTURE/'upstream'/name).read_bytes()
            assert len(data)==record['size'] and hashlib.sha256(data).hexdigest()==record['sha256']
        assert hashlib.sha256(PATCH.read_bytes()).hexdigest()==cls.manifest['patch_sha256']
        assert hashlib.sha256(PATCH2.read_bytes()).hexdigest()==cls.manifest['patch2_sha256']
        assert hashlib.sha256(PATCH3.read_bytes()).hexdigest()==cls.manifest['patch3_sha256']
        for name,digest in cls.manifest['harness_sha256'].items():
            assert hashlib.sha256((FIXTURE/name).read_bytes()).hexdigest()==digest
        cls.tmp=None if os.environ.get('TCC_KEEP_TEST_TMPDIR') else tempfile.TemporaryDirectory(prefix='tcc-contract-',dir=os.environ.get('MECHREVO_TEST_TMPDIR'))
        cls.base=Path(cls.tmp.name if cls.tmp else tempfile.mkdtemp(prefix='tcc-contract-',dir=os.environ.get('MECHREVO_TEST_TMPDIR')))
        cls.command_count=0
        cls.legacy=cls.base/'legacy';cls.patched=cls.base/'patched'
        shutil.copytree(FIXTURE/'upstream',cls.legacy);shutil.copytree(FIXTURE/'upstream',cls.patched)
        subprocess.run(['patch','-p1','--fuzz=0','--batch','--forward','-i',str(PATCH)],cwd=cls.patched,check=True,stdout=subprocess.PIPE,stderr=subprocess.PIPE)
        cls.patch1=cls.base/'patch1';shutil.copytree(cls.patched,cls.patch1)
        subprocess.run(['patch','-p1','--fuzz=0','--batch','--forward','-i',str(PATCH2)],cwd=cls.patched,check=True,stdout=subprocess.PIPE,stderr=subprocess.PIPE)
        for name,digest in cls.manifest['patch12_files_sha256'].items():
            assert hashlib.sha256((cls.patched/name).read_bytes()).hexdigest()==digest
        cls.patch2=cls.base/'patch2';shutil.copytree(cls.patched,cls.patch2)
        subprocess.run(['patch','-p1','--fuzz=0','--batch','--forward','-i',str(PATCH3)],cwd=cls.patched,check=True,stdout=subprocess.PIPE,stderr=subprocess.PIPE)
        for name,digest in cls.manifest['patch123_files_sha256'].items():
            assert hashlib.sha256((cls.patched/name).read_bytes()).hexdigest()==digest
        for patch in (PATCH4,PATCH5):
            subprocess.run(['patch','-p1','--fuzz=0','--batch','--forward','-i',str(patch)],cwd=cls.patched,check=True,stdout=subprocess.PIPE,stderr=subprocess.PIPE)
        assert hashlib.sha256(PATCH5.read_bytes()).hexdigest()==cls.manifest['patch5_sha256']
        cls.patch5=cls.base/'patch5';shutil.copytree(cls.patched,cls.patch5)
        for name,digest in cls.manifest['patch5_files_sha256'].items():
            assert hashlib.sha256((cls.patched/name).read_bytes()).hexdigest()==digest
        subprocess.run(['patch','-p1','--fuzz=0','--batch','--forward','-i',str(PATCH6)],cwd=cls.patched,check=True,stdout=subprocess.PIPE,stderr=subprocess.PIPE)
        assert hashlib.sha256(PATCH6.read_bytes()).hexdigest()==cls.manifest['patch6_sha256']
        subprocess.run(['patch','-p1','--fuzz=0','--batch','--forward','-i',str(PATCH7)],cwd=cls.patched,check=True,stdout=subprocess.PIPE,stderr=subprocess.PIPE)
        assert hashlib.sha256(PATCH7.read_bytes()).hexdigest()==cls.manifest['patch7_sha256']
        for name,digest in cls.manifest['patched_files_sha256'].items():
            assert hashlib.sha256((cls.patched/name).read_bytes()).hexdigest()==digest
        cls.generated=None
    @classmethod
    def tearDownClass(cls):
        if cls.tmp:cls.tmp.cleanup()
    def run_cmd(self,args,success=True):
        r=subprocess.run([str(x) for x in args],stdout=subprocess.PIPE,stderr=subprocess.STDOUT,text=True,timeout=120)
        if os.environ.get('TCC_TEST_LOG_DIR'):
            self.__class__.command_count+=1
            write_command_log(os.environ['TCC_TEST_LOG_DIR'], self.__class__.__name__,
                              self.__class__.command_count, args, r.returncode, r.stdout)
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

    def service_tools(self):
        if not all(os.environ.get(k) for k in ('TCC_TYPESCRIPT_MODULE','TCC_NODE')):
            self.skipTest('explicit existing locked TypeScript/Node required; no automatic install')
        return os.environ['TCC_NODE'],os.environ['TCC_TYPESCRIPT_MODULE']

    def generated_classes(self):
        node,ts=self.service_tools()
        if self.__class__.generated is None:
            out=self.base/'generated-js'
            code="const fs=require('fs'),path=require('path'),ts=require(process.argv[1]);for(const name of ['DaemonWorker','FanControlBaseClass','FanControlTuxedoIO','FanControlWorker','FanControlHwmon','TuxedoControlCenterDaemon','StateSwitcherWorker']){const source=fs.readFileSync(path.join(process.argv[2],'src/service-app/classes',name+'.ts'),'utf8');const result=ts.transpileModule(source,{reportDiagnostics:true,compilerOptions:{module:ts.ModuleKind.CommonJS,target:ts.ScriptTarget.ES2022}});if(result.diagnostics.some(d=>d.category===ts.DiagnosticCategory.Error))throw Error('TypeScript transpile diagnostic: '+name);const dest=path.join(process.argv[3],'service-app/classes');fs.mkdirSync(dest,{recursive:true});fs.writeFileSync(path.join(dest,name+'.js'),result.outputText);}const source=fs.readFileSync(path.join(process.argv[2],'src/common/models/IFanData.ts'),'utf8');const result=ts.transpileModule(source,{reportDiagnostics:true,compilerOptions:{module:ts.ModuleKind.CommonJS,target:ts.ScriptTarget.ES2022}});if(result.diagnostics.some(d=>d.category===ts.DiagnosticCategory.Error))throw Error('TypeScript model diagnostic');const dest=path.join(process.argv[3],'common/models');fs.mkdirSync(dest,{recursive:true});fs.writeFileSync(path.join(dest,'IFanData.js'),result.outputText);const consumer=fs.readFileSync(path.join(process.argv[2],'src/ng-app/app/compatibility.service.ts'),'utf8');const emitted=ts.transpileModule(consumer,{compilerOptions:{module:ts.ModuleKind.CommonJS,target:ts.ScriptTarget.ES2022,experimentalDecorators:true}}).outputText;const consumerDest=path.join(process.argv[3],'ng-app/app');fs.mkdirSync(consumerDest,{recursive:true});fs.writeFileSync(path.join(consumerDest,'compatibility.service.js'),emitted);"
            self.run_cmd([node,'-e',code,ts,self.patched,out])
            self.__class__.generated=out
        return self.__class__.generated

    def negative_assertion(self,args,expected=None):
        output=self.run_cmd(args,success=False)
        self.assertIn('AssertionError',output,output)
        if expected is not None:self.assertIn(expected,output,output)

    def test_lifecycle_actual_ts_independent_js_and_patch1_negatives(self):
        node,ts=self.service_tools()
        groups=['false-throw','mapping-retry','exit-drain-reopen','exit-failed-retry','start-drain-busy-profile','start-failure-queue','exit-waits-start','queued-start-cancel','owned-capability-loss','restart-start-errors']
        for group in groups:
            for kind,source in [('ts',self.patched),('js',self.generated_classes())]:
                with self.subTest(group=group,kind=kind):
                    self.run_cmd([node,FIXTURE/'lifecycle_test.cjs',ts,source,group,kind])
            with self.subTest(group=group,kind='patch1-negative'):
                self.negative_assertion([node,FIXTURE/'lifecycle_test.cjs',ts,self.patch1,group,'ts'])

    def test_queued_start_cancellation_mutation_is_rejected(self):
        node,ts=self.service_tools();hybrid=self.base/'cancel-start-mutation';shutil.copytree(self.patched,hybrid)
        f=hybrid/'src/service-app/classes/FanControlWorker.ts';s=f.read_text();marker="if (this.closing) throw new Error('FanControlWorker: queued start cancelled by exit');";self.assertEqual(s.count(marker),2);f.write_text(s.replace(marker,'if (this.closing) return;'))
        self.negative_assertion([node,FIXTURE/'lifecycle_test.cjs',ts,hybrid,'queued-start-cancel','ts'])

    def test_cleanup_stages_actual_ts_js_and_original_mutation_negatives(self):
        node,ts=self.service_tools();rel='src/service-app/classes/FanControlTuxedoIO.ts'
        for kind,file in [('ts',self.patched/rel),('js',self.generated_classes()/'service-app/classes/FanControlTuxedoIO.js')]:
            with self.subTest(kind=kind):self.run_cmd([node,FIXTURE/'stage_errors_test.cjs',ts,file,kind])
        for name,source in [('original',self.legacy),('patch1',self.patch1)]:
            with self.subTest(kind=name):self.negative_assertion([node,FIXTURE/'stage_errors_test.cjs',ts,source/rel,'ts'])
        f=self.base/'constant-stage-mutation.ts';source=(self.patched/rel).read_text();marker="`FanControlTuxedoIO: fan cleanup incomplete: ${failureStages.join('; ')}`";self.assertEqual(source.count(marker),1);f.write_text(source.replace(marker,"'FanControlTuxedoIO: fan cleanup incomplete'"))
        self.negative_assertion([node,FIXTURE/'stage_errors_test.cjs',ts,f,'ts'])
        unsafe=self.base/'unsafe-stage-mutation.ts';marker="""            try {
                detail = String(error);
            } catch {
                detail = '[unprintable error]';
            }""";self.assertEqual(source.count(marker),1);unsafe.write_text(source.replace(marker,'            detail = String(error);'))
        for group in ['auto-formatter','disable-formatter','both-formatters']:
            with self.subTest(kind='unsafe-mutation',group=group):self.negative_assertion([node,FIXTURE/'stage_errors_test.cjs',ts,unsafe,'ts',group])

    def test_actual_callers_ts_js_and_isolated_original_callers(self):
        node,ts=self.service_tools();baseline=self.base/'original-callers-final-worker';shutil.copytree(self.patched,baseline)
        for name in ['TuxedoControlCenterDaemon','StateSwitcherWorker']:
            rel='src/service-app/classes/'+name+'.ts';shutil.copy2(self.legacy/rel,baseline/rel)
        groups=['profile-during-exit','refresh-during-apply','shutdown-during-start','shutdown-no-reopen','best-effort-workers','concurrent-starts','restart-after-cleanup','init-failure-observable','exit-failure-retry','hostile-start-error','signal-clean','signal-failed','fatal-error','trigger-state-reject']
        unchanged={'restart-after-cleanup','signal-clean'}
        for group in groups:
            for kind,source in [('ts',self.patched),('js',self.generated_classes())]:
                with self.subTest(group=group,kind=kind):self.run_cmd([node,FIXTURE/'caller_lifecycle_test.cjs',ts,source,self.patched,group,kind])
            with self.subTest(group=group,kind='original-callers'):
                args=[node,FIXTURE/'caller_lifecycle_test.cjs',ts,baseline,self.patched,group,'ts']
                if group in unchanged:self.run_cmd(args)
                else:
                    output=self.run_cmd(args,success=False)
                    expected={'exit-failure-retry':'Error: hostile primitive','hostile-start-error':'Error: hostile primitive','trigger-state-reject':'Error: state async failure'}.get(group,'AssertionError')
                    self.assertIn(expected,output,output)


    def test_readback_contract_aggregate_errors_ts_js_and_patch2_negatives(self):
        node,ts=self.service_tools()
        groups=['aggregate-hostile','onwork-hostile','invalid-native','invalid-worker','hwmon-zero','settled-hostile']
        for group in groups:
            for kind,source in [('ts',self.patched),('js',self.generated_classes())]:
                with self.subTest(group=group,kind=kind):
                    self.run_cmd([node,FIXTURE/'readback_errors_test.cjs',ts,source,group,kind])
            with self.subTest(group=group,kind='patch2-negative'):
                self.negative_assertion([node,FIXTURE/'readback_errors_test.cjs',ts,self.patch2,group,'ts'])

    def test_actual_native_failed_write_valid_readback_ts_js_original_negative(self):
        node,ts=self.service_tools()
        if not all(os.environ.get(k) for k in ('TCC_NODE_INCLUDE','TCC_NODE_ADDON_API')):
            self.skipTest('explicit existing qualified Node/N-API headers required; no automatic install')
        patched_addon=self.base/'readback-patched.node';self.compile(self.patched,patched_addon,addon=True)
        for kind,source in [('ts',self.patched),('js',self.generated_classes())]:
            with self.subTest(kind=kind):
                self.run_cmd([node,FIXTURE/'readback_errors_test.cjs',ts,source,'native-failure-readback',kind,patched_addon])
        original_addon=self.base/'readback-original.node';self.compile(self.legacy,original_addon,addon=True)
        self.negative_assertion([node,FIXTURE/'readback_errors_test.cjs',ts,self.legacy,'native-failure-readback','ts',original_addon])

    def test_temperature_exception_invalidates_actual_feedback_consumer_ts_js_and_pkg5_negative(self):
        node,ts=self.service_tools()
        for group in ['single','double','both','undefined','settled','publish-failure','normal']:
            for kind,source in [('ts',self.patched),('js',self.generated_classes())]:
                with self.subTest(group=group,kind=kind):
                    self.run_cmd([node,FIXTURE/'temperature_exception_test.cjs',ts,source,group,kind,self.patched])
            with self.subTest(group=group,kind='pkg5'):
                args=[node,FIXTURE/'temperature_exception_test.cjs',ts,self.patch5,group,'ts',self.patch5]
                if group=='normal':self.run_cmd(args)
                else:
                    expected={
                        'settled':'all round fan invalidations start after reads settle',
                        'publish-failure':'actual GPU consumer must reject aborted round feedback',
                    }.get(group,'actual consumer must not accept prior temperature after a failed round')
                    self.negative_assertion(args,expected)

if __name__=='__main__':unittest.main()
