"""GPL pinned methods, real bounded cache/graph threads, no daemon or hardware."""
import hashlib,json,os,shutil,subprocess,tempfile,unittest
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
class ThermaldCheckedVirtual(unittest.TestCase):
 @classmethod
 def setUpClass(cls):
  parent=Path(os.environ.get('MECHREVO_TEST_TMPDIR',ROOT/'.local-tests'));parent.mkdir(parents=True,exist_ok=True)
  cls.tmp=tempfile.TemporaryDirectory(prefix='thermald-virtual-',dir=parent);cls.addClassCleanup(cls.tmp.cleanup)
  cls.tree=Path(cls.tmp.name)/'tree';cls.src=cls.tree/'src';cls.src.mkdir(parents=True)
  subprocess.run(['git','init','-q',str(cls.tree)],check=True)
  cls.inputs={}
  for dirname,names in [('thermald_checked',['thd_sensor.h','thd_sensor.cpp','thd_sensor_virtual.h','thd_sensor_virtual.cpp']),('thermald_virtual',['thd_engine.cpp','thd_dbus_interface.cpp','thd_util.cpp'])]:
   fixture=ROOT/'tests/fixtures'/dirname;provenance=json.loads((fixture/'provenance.json').read_text())
   if provenance['commit']!='5fdc5bbca7f8c3f5bb2ad6540e8211a159a88a3a':raise AssertionError('Wrong official source pin')
   for name in names:
    data=(fixture/name).read_bytes();want=provenance['sha256'][name]
    if hashlib.sha256(data).hexdigest()!=want:raise AssertionError('Frozen GPL source changed: '+name)
    shutil.copy2(fixture/name,cls.src/name);cls.inputs[name]=want
  for name in ['0001-checked-temperature.patch','0002-preference-state.patch','0003-periodic-restart-cache.patch']:
   cmd=['git','apply']+['--include=src/'+n for n in cls.inputs]+[str(ROOT/'packaging/arch/thermald-mechrevo'/name)]
   r=subprocess.run(cmd,cwd=cls.tree,text=True,capture_output=True,timeout=15)
   if r.returncode:raise AssertionError(r.stdout+r.stderr)
  if os.environ.get('THERMALD_PREPARED_SOURCE'):
   actual=Path(os.environ['THERMALD_PREPARED_SOURCE'])
   for name in cls.inputs:
    if (actual/name).read_bytes()!=(cls.src/name).read_bytes():raise AssertionError('Prepared source mismatch: '+name)
 def run_variant(self,name):
  out=Path(self.tmp.name)/name
  r=subprocess.run([os.sys.executable,'-B',str(ROOT/'tests/thermald_virtual_check.py'),'--workspace',str(ROOT),'--source-dir',str(self.src),'--out',str(out),'--mutation',name],text=True,capture_output=True,timeout=75)
  self.assertTrue((out/'receipt.json').is_file(),r.stdout+r.stderr)
  receipt=json.loads((out/'receipt.json').read_text());self.assertEqual(r.returncode,receipt['exit_code'],r.stdout+r.stderr)
  return receipt
 def test_actual_dbus_graph_filter_cache_and_bounded_threads(self):
  r=self.run_variant('none');self.assertEqual(r['exit_code'],0,r['stderr']);self.assertIn('concurrent-cache+parameter-writer: passed',r['stdout'])
 def test_cycle_guard_removal_is_detected(self):
  r=self.run_variant('cycle-guard');self.assertEqual(r['exit_code'],1);self.assertIn('actual DBus self update accepted',r['stderr'])
 def test_successful_parameter_update_must_invalidate_cache(self):
  r=self.run_variant('cache-invalidation');self.assertEqual(r['exit_code'],1);self.assertIn('successful update did not invalidate cache/append source',r['stderr'])
 def test_partial_failure_must_not_commit_filter_history(self):
  r=self.run_variant('filter-transaction');self.assertEqual(r['exit_code'],1);self.assertIn('failed aggregate partially changed filter history',r['stderr'])
 def test_linear_algorithm_must_not_read_ignored_secondary_source(self):
  r=self.run_variant('linear-inputs');self.assertEqual(r['exit_code'],1);self.assertIn('linear path consumed originally ignored second dependency',r['stderr'])
if __name__=='__main__':unittest.main()
