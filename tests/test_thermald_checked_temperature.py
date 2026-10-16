"""Pinned GPL upstream methods plus maintained patches, compiled verbatim.

Only sysfs, engine preference and trip callback are doubles. Assertions concern
sample validity, no partial zone callback and protected field ordering, not RPM,
actual RAPL power or installed daemon behavior. No binary daemon is executed.
"""
import hashlib,json,os,resource,shutil,subprocess,tempfile,unittest
from pathlib import Path
from test_thermald_ceiling_runtime import COMMON,SYSFS,actual_functions
ROOT=Path(__file__).resolve().parents[1]
FIX=ROOT/'tests/fixtures/thermald_checked'
RECIPE=ROOT/'packaging/arch/thermald-mechrevo'

class ThermaldCheckedTemperature(unittest.TestCase):
 @classmethod
 def setUpClass(cls):
  provenance=json.loads((FIX/'provenance.json').read_text())
  for name,want in provenance['sha256'].items():
   if hashlib.sha256((FIX/name).read_bytes()).hexdigest()!=want:raise AssertionError('Frozen GPL source changed: '+name)
  parent=Path(os.environ.get('MECHREVO_TEST_TMPDIR',ROOT/'.local-tests'));parent.mkdir(parents=True,exist_ok=True)
  cls.tmp=tempfile.TemporaryDirectory(prefix='thermald-checked-',dir=parent);cls.addClassCleanup(cls.tmp.cleanup);cls.binaries={}
  for variant in ['upstream','checked','final']:
   d=Path(cls.tmp.name)/variant;(d/'src').mkdir(parents=True)
   subprocess.run(['git','init','-q',str(d)],check=True)
   for name in provenance['sha256']:shutil.copy2(FIX/name,d/'src'/name)
   if variant!='upstream':
    for patch in ['0001-checked-temperature.patch']+(['0002-preference-state.patch','0003-periodic-restart-cache.patch'] if variant=='final' else []):
     if patch=='0003-periodic-restart-cache.patch':
      cls.prior_lifecycle=Path(cls.tmp.name)/'prior-lifecycle';shutil.copytree(d/'src',cls.prior_lifecycle)
     r=subprocess.run(['git','apply','--include=src/thd_sensor.h','--include=src/thd_sensor.cpp','--include=src/thd_zone.cpp','--include=src/thd_sensor_virtual.h','--include=src/thd_sensor_virtual.cpp',str(RECIPE/patch)],cwd=d,text=True,capture_output=True)
     if r.returncode:raise AssertionError(r.stdout+r.stderr)
   common=COMMON+'\n#define thd_log_error(...) fake_log(__VA_ARGS__)\n'
   (d/'src/thd_common.h').write_text(common)
   fake=SYSFS.replace('    int failure = -EIO;','    int failure = -EIO;\n    int read_error = 0;\n    bool empty_read = false;').replace('        auto found = state().numbers.find(key);','        if (state().empty_read) return 0;\n        if (state().read_error) return state().read_error;\n        auto found = state().numbers.find(key);').replace('    fake_state &state() {','    void update_path(std::string p) { path=std::move(p); }\n    fake_state &state() {')
   (d/'src/thd_sys_fs.h').write_text(fake);(d/'src/thd_util.h').write_text('#pragma once\ninline bool starts_with(const std::string&a,const std::string&b){return a.rfind(b,0)==0;}\n')
   sensor=(d/'src/thd_sensor.cpp').read_text();zone=(d/'src/thd_zone.cpp').read_text()
   funcs=[actual_functions(sensor,n).decode() for n in ['cthd_sensor::cthd_sensor','cthd_sensor::read_temperature']]
   if variant!='upstream':funcs.append(actual_functions(sensor,'cthd_sensor::read_temperature_checked').decode())
   funcs += [actual_functions(zone,n).decode() for n in ['cthd_zone::read_zone_temp','cthd_zone::update_zone_preference']]
   (d/'runtime.cpp').write_text((FIX/'runtime.cpp').read_text().replace('/* @ACTUAL_FUNCTIONS@ */','\n'.join(funcs)))
   binary=d/'runtime';cmd=['c++','-std=c++17','-O2','-Wall','-Wextra','-Werror','-Wno-unused-parameter','-I'+str(d/'src')]
   if variant!='upstream':cmd+=['-DCHECKED_API']
   r=subprocess.run(cmd+[str(d/'runtime.cpp'),'-o',str(binary)],text=True,capture_output=True,env={**os.environ,"TMPDIR":str(d)})
   if r.returncode:raise AssertionError(r.stdout+r.stderr)
   cls.binaries[variant]=binary
 def run_case(self,case,variant='final'):
  return subprocess.run([str(self.binaries[variant]),case],capture_output=True,text=True,timeout=10,preexec_fn=lambda:resource.setrlimit(resource.RLIMIT_CORE,(0,0)))
 def test_prepared_build_inputs_match_pinned_source_and_patch_outputs(self):
  source=os.environ.get('THERMALD_PREPARED_SOURCE')
  if source is None:return # Standalone fixture tests do not claim build-source verification.
  source=Path(source)
  for name in json.loads((FIX/'provenance.json').read_text())['sha256']:
   self.assertEqual((source/name).read_bytes(),(self.binaries['final'].parent/'src'/name).read_bytes(),name)
 def test_failed_reads_no_longer_emit_cooling_callbacks_or_mutate_zone(self):
  for case in ['io','eof','negative','second','pref-invalid']:
   with self.subTest(case=case):
    old=self.run_case(case,'upstream');self.assertNotEqual(old.returncode,0);self.assertIn('zone.calls.empty()&&zone.zone_temp==12345',old.stderr)
    r=self.run_case(case);self.assertEqual(r.returncode,0,r.stderr)
 def test_checked_api_preserves_output_on_failure_and_keeps_real_zero_and_scale(self):
  r=self.run_case('checked-api');self.assertEqual(r.returncode,0,r.stderr)
 def test_successful_preference_replays_both_rounds_with_original_zone_state(self):
  for case in ['preference','preference-zero']:
   with self.subTest(case=case):
    self.assertEqual(self.run_case(case,'upstream').returncode,0)
    old=self.run_case(case,'checked');self.assertNotEqual(old.returncode,0);self.assertIn('zone.calls[i]==std::make_tuple(7+i,0u,t)',old.stderr)
    r=self.run_case(case);self.assertEqual(r.returncode,0,r.stderr)
 def test_zero_and_normal_temperature_preserve_existing_independent_zone_behavior(self):
  for case in ['zero','normal','correlated-zero']:
   with self.subTest(case=case):
    for variant in ['upstream','final']:
     r=self.run_case(case,variant);self.assertEqual(r.returncode,0,r.stderr)
 def test_real_pthread_restart_invalidates_old_cold_cache_before_first_sample(self):
  helper=ROOT/'tests/thermald_lifecycle_check.py'
  for cold in [0,75000]:
   out=Path(self.tmp.name)/('lifecycle-'+str(cold))
   r=subprocess.run([os.sys.executable,'-B',str(helper),'--workspace',str(ROOT),'--source-dir',str(self.binaries['final'].parent/'src'),'--out',str(out),'--prior-cold',str(cold),'--expect-restart-invalidation'],text=True,capture_output=True,timeout=30)
   self.assertEqual(r.returncode,0,r.stdout+r.stderr)
   receipt=json.loads((out/'receipt.json').read_text());self.assertEqual(receipt['cases'][0]['exit_code'],0)
  r=subprocess.run([os.sys.executable,'-B',str(helper),'--workspace',str(ROOT),'--source-dir',str(self.prior_lifecycle),'--out',str(Path(self.tmp.name)/'lifecycle-old'),'--prior-cold','75000','--expect-restart-invalidation'],text=True,capture_output=True,timeout=30)
  self.assertNotEqual(r.returncode,0);self.assertIn('restart accepted stale cold cache before first worker sample',r.stderr)
 def test_actual_periodic_callback_does_not_treat_invalid_cache_as_cold(self):
  out=Path(self.tmp.name)/'periodic'
  source=self.binaries['final'].parent/'src'
  r=subprocess.run([os.sys.executable,'-B',str(ROOT/'tests/thermald_periodic_check.py'),'--workspace',str(ROOT),'--source-dir',str(source),'--out',str(out)],text=True,capture_output=True,timeout=30)
  self.assertEqual(r.returncode,0,r.stdout+r.stderr)
  receipt=json.loads((out/'receipt.json').read_text());self.assertEqual(receipt['cases'][0]['exit_code'],0);self.assertNotEqual(receipt['cases'][1]['exit_code'],0)
if __name__=='__main__':unittest.main()
