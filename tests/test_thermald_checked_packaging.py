"""Actual recipe functions with private filesystem inputs; never run the daemon."""
import os,shutil,subprocess,tempfile,unittest
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
RECIPE=ROOT/'packaging/arch/thermald-mechrevo'
class ThermaldCheckedPackaging(unittest.TestCase):
 def setUp(self):
  parent=Path(os.environ.get('MECHREVO_TEST_TMPDIR',ROOT/'.local-tests'));parent.mkdir(parents=True,exist_ok=True)
  self.tmp=tempfile.TemporaryDirectory(prefix='thermald-package-',dir=parent);self.addCleanup(self.tmp.cleanup)
  self.d=Path(self.tmp.name);(self.d/'src').mkdir();(self.d/'pkg').mkdir()
 def recipe(self,body,extra=None):
  env={**os.environ,'TMPDIR':str(self.d),'srcdir':str(self.d/'src'),'pkgdir':str(self.d/'pkg'),'startdir':str(RECIPE)}
  env.pop('THERMALD_SOURCE_URL',None);env.pop('THERMALD_CONTRACT_ROOT',None)
  if extra:env.update(extra)
  return subprocess.run(['bash','-ec','source "$1"; '+body,'bash',str(RECIPE/'PKGBUILD')],text=True,capture_output=True,env=env,timeout=15)
 def test_metadata_and_remote_override(self):
  r=self.recipe('printf "%s\\n" "$pkgname" "${provides[0]}" "${conflicts[0]}" "${arch[*]}"');self.assertEqual(r.returncode,0,r.stderr)
  self.assertEqual(r.stdout.splitlines(),['thermald-mechrevo','thermald=2:2.5.13','thermald','x86_64 x86_64_v3'])
  r=self.recipe(':',{'THERMALD_SOURCE_URL':'https://invalid.example/untrusted.git'});self.assertNotEqual(r.returncode,0);self.assertIn('Only official upstream',r.stderr)
 def test_wrong_upstream_commit_is_rejected_before_patch(self):
  d=self.d/'src/thermal_daemon';d.mkdir();subprocess.run(['git','init','-q',str(d)],check=True)
  r=self.recipe('prepare');self.assertNotEqual(r.returncode,0);self.assertIn('Wrong upstream commit',r.stderr)
 def test_missing_test_or_prepared_source_is_rejected(self):
  r=self.recipe('check',{'THERMALD_CONTRACT_ROOT':str(self.d/'missing')});self.assertNotEqual(r.returncode,0)
  for name in ['0001-checked-temperature.patch','0002-preference-state.patch','0003-periodic-restart-cache.patch']:shutil.copy2(RECIPE/name,self.d/'src'/name)
  r=self.recipe('check');self.assertNotEqual(r.returncode,0);self.assertFalse((self.d/'src/.contract-tmp').exists())
 def test_changed_patch_is_rejected(self):
  for name in ['0001-checked-temperature.patch','0002-preference-state.patch','0003-periodic-restart-cache.patch']:shutil.copy2(RECIPE/name,self.d/'src'/name)
  (self.d/'src/0002-preference-state.patch').write_text('altered input')
  r=self.recipe('check');self.assertNotEqual(r.returncode,0);self.assertFalse((self.d/'src/.contract-tmp').exists())
 def complete_check_inputs(self):
  root=self.d/'contract';(root/'tests').mkdir(parents=True)
  names=['test_thermald_checked_temperature.py','test_thermald_checked_adaptive.py','test_thermald_checked_virtual.py','test_thermald_checked_dbus.py','test_thermald_checked_packaging.py']
  for name in names:(root/'tests'/name).write_text('# Test-only presence fixture; python boundary is intercepted.\n')
  recipe=root/'packaging/arch/thermald-mechrevo';recipe.mkdir(parents=True)
  for name in ['0001-checked-temperature.patch','0002-preference-state.patch','0003-periodic-restart-cache.patch']:
   shutil.copy2(RECIPE/name,self.d/'src'/name);shutil.copy2(RECIPE/name,recipe/name)
  (self.d/'src/thermal_daemon/src').mkdir(parents=True)
  return root
 def test_check_invokes_all_contracts_with_actual_prepared_source(self):
  root=self.complete_check_inputs()
  r=self.recipe('python() { printf "%s\\n" "$THERMALD_PREPARED_SOURCE" "$@"; }; check',{'THERMALD_CONTRACT_ROOT':str(root)})
  self.assertEqual(r.returncode,0,r.stdout+r.stderr)
  args=r.stdout.splitlines();self.assertEqual(args[0],str(self.d/'src/thermal_daemon/src'))
  self.assertEqual(args[args.index('-p')+1],'test_thermald_checked*.py')
  self.assertEqual(args[args.index('-s')+1],str(root/'tests'))
 def test_missing_each_contract_is_rejected_before_python(self):
  root=self.complete_check_inputs()
  for name in ['temperature','adaptive','virtual','dbus','packaging']:
   with self.subTest(contract=name):
    p=root/'tests'/('test_thermald_checked_'+name+'.py');data=p.read_bytes();p.unlink()
    r=self.recipe('python() { echo unexpected-python-launch; }; check',{'THERMALD_CONTRACT_ROOT':str(root)})
    self.assertNotEqual(r.returncode,0);self.assertNotIn('unexpected-python-launch',r.stdout)
    p.write_bytes(data)
 def test_package_requires_built_inputs_and_installs_without_execution(self):
  r=self.recipe('package');self.assertNotEqual(r.returncode,0)
  b=self.d/'src/build';b.mkdir();binary=b/'thermald';binary.write_text('#!/bin/sh\nexit 99\n');binary.chmod(0o755)
  (b/'Makefile').write_text('install:\n\tmkdir -p $(DESTDIR)/usr/bin\n\tcp thermald $(DESTDIR)/usr/bin/thermald\n')
  source=self.d/'src/thermal_daemon';source.mkdir();(source/'COPYING').write_text('test-only-license')
  r=self.recipe('package');self.assertEqual(r.returncode,0,r.stdout+r.stderr)
  self.assertEqual((self.d/'pkg/usr/bin/thermald').read_bytes(),binary.read_bytes())
  self.assertEqual((self.d/'pkg/usr/share/licenses/thermald-mechrevo/COPYING').read_text(),'test-only-license')
if __name__=='__main__':unittest.main()
