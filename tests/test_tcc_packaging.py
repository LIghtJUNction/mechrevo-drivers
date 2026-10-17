import hashlib,json,os,pathlib,shutil,stat,subprocess,tempfile,unittest
ROOT=pathlib.Path(__file__).resolve().parents[1]
RECIPE=ROOT/'packaging/arch/tuxedo-control-center-mechrevo'
FIXTURE=ROOT/'tests/fixtures/tcc_3_0_10/upstream'
PATCHES=['0001-fan-io-error-contract.patch','0002-fan-lifecycle.patch','0003-fan-readback-diagnostics.patch','0004-installed-resource-paths.patch','0005-settings-read-failclosed.patch','0006-temperature-exception-invalidates-feedback.patch']
class TccPackaging(unittest.TestCase):
    def setUp(self):
        self.temp=tempfile.TemporaryDirectory(dir=os.environ.get('MECHREVO_TEST_TMPDIR'));self.base=pathlib.Path(self.temp.name);self.src=self.base/'src';self.src.mkdir();self.source=self.src/'tuxedo-control-center-3.0.10';shutil.copytree(FIXTURE,self.source);self.pkg=self.base/'pkg';self.pkg.mkdir()
        for name in PATCHES:shutil.copy2(RECIPE/name,self.src/name)
        dist=self.source/'src/dist-data';dist.mkdir(parents=True)
        contents={'tccd.service':'[Service]\nExecStart=/old/tccd --start\nExecStop=/old/tccd --stop\n','tccd-sleep.service':'[Service]\nExecStart=/old/tccd --sleep\n','tuxedo-control-center.desktop':'[Desktop Entry]\nExec=/old/tcc\nIcon=old\n','tuxedo-control-center-tray.desktop':'[Desktop Entry]\nExec=/old/tray\n'}
        for name,text in contents.items():(dist/name).write_text(text)
        for name in ('tuxedo-control-center_256.png','tuxedo-control-center_256.svg','com.tuxedocomputers.tccd.policy','com.tuxedocomputers.tomte.policy','com.tuxedocomputers.tcc.metainfo.xml','com.tuxedocomputers.tccd.conf'):(dist/name).write_bytes(('fixture:'+name).encode())
        udev=self.source/'src/udev';udev.mkdir();(udev/'99-webcam.rules').write_text('/opt/tuxedo-control-center/resources/dist/webcam\n')
    def tearDown(self):self.temp.cleanup()
    def invoke(self,function,env=None):
        script='source "$1"; srcdir="$2"; pkgdir="$3"; startdir="$4"; '+function
        e=(os.environ if env is None else env).copy();e['LC_ALL']='C'
        return subprocess.run(['/usr/bin/bash','-c',script,'bash',str(RECIPE/'PKGBUILD'),str(self.src),str(self.pkg),str(RECIPE)],env=e,text=True,capture_output=True)
    def test_metadata_and_each_local_source_checksum_are_exact(self):
        script=r'source "$1"; printf "%s\0" "$pkgname" "$pkgver" "$pkgrel" "${source[@]}" "${sha256sums[@]}"'
        r=subprocess.check_output(['/usr/bin/bash','-c',script,'bash',str(RECIPE/'PKGBUILD')]);fields=r.decode().rstrip('\0').split('\0');self.assertEqual(fields[:3],['tuxedo-control-center-mechrevo','3.0.10','6']);sources=fields[3:11];sums=fields[11:];self.assertEqual(len(sums),8);self.assertEqual(sources[1:],PATCHES+['electron-builder-dir.json']);self.assertEqual(sums[0],'d192391c559789c8f62af0e614e37e1222c9aa9753c61a30e3149b35d1969532')
        for name,digest in zip(sources[1:],sums[1:]):self.assertEqual(hashlib.sha256((RECIPE/name).read_bytes()).hexdigest(),digest,name)
    def test_actual_prepare_applies_six_patches_to_original_fixture(self):
        result=self.invoke('prepare');self.assertEqual(result.returncode,0,result.stdout+result.stderr);manifest=json.loads((FIXTURE.parent/'provenance.json').read_text())
        for name,digest in manifest['patched_files_sha256'].items():self.assertEqual(hashlib.sha256((self.source/name).read_bytes()).hexdigest(),digest,name)
        dist=self.source/'src/dist-data';self.assertIn('ExecStart=/usr/lib/tuxedo-control-center/data/service/tccd --start',(dist/'tccd.service').read_text());self.assertIn('Exec=tuxedo-control-center %U',(dist/'tuxedo-control-center.desktop').read_text());self.assertIn('Icon=tuxedo-control-center',(dist/'tuxedo-control-center.desktop').read_text());self.assertIn('Exec=tuxedo-control-center --tray',(dist/'tuxedo-control-center-tray.desktop').read_text());self.assertEqual((self.source/'src/udev/99-webcam.rules').read_text(),'/usr/lib/webcam\n')
    def test_corrupted_third_patch_is_rejected(self):
        (self.src/PATCHES[2]).write_text('invalid patch body\n');result=self.invoke('prepare');self.assertNotEqual(result.returncode,0,result.stdout+result.stderr)
    def test_check_rejects_third_patch_different_from_reviewed_root(self):
        contract=self.base/'contract';(contract/'tests').mkdir(parents=True);(contract/'tests/test_tcc_fan_error_contract.py').write_text('raise AssertionError("must not run")\n');public=contract/'packaging/arch/tuxedo-control-center-mechrevo';public.mkdir(parents=True)
        for name in PATCHES:shutil.copy2(RECIPE/name,public/name)
        (public/PATCHES[2]).write_text('unreviewed drift\n');env=os.environ.copy();env['TCC_CONTRACT_ROOT']=str(contract);result=self.invoke('check',env);self.assertNotEqual(result.returncode,0);self.assertNotIn('must not run',result.stderr)
    def test_corrupted_resource_patch_is_rejected(self):
        (self.src/PATCHES[3]).write_text('invalid resource patch body\n');result=self.invoke('prepare');self.assertNotEqual(result.returncode,0,result.stdout+result.stderr)
    def test_corrupted_settings_patch_is_rejected(self):
        (self.src/PATCHES[4]).write_text('invalid settings patch body\n');result=self.invoke('prepare');self.assertNotEqual(result.returncode,0,result.stdout+result.stderr)
    def test_check_rejects_settings_patch_different_from_reviewed_root(self):
        contract=self.base/'contract';(contract/'tests').mkdir(parents=True);(contract/'tests/test_tcc_fan_error_contract.py').write_text('raise AssertionError("must not run")\n');public=contract/'packaging/arch/tuxedo-control-center-mechrevo';public.mkdir(parents=True)
        for name in PATCHES:shutil.copy2(RECIPE/name,public/name)
        (public/PATCHES[4]).write_text('unreviewed settings drift\n');env=os.environ.copy();env['TCC_CONTRACT_ROOT']=str(contract);result=self.invoke('check',env);self.assertNotEqual(result.returncode,0);self.assertNotIn('must not run',result.stderr);self.assertIn(PATCHES[4],result.stdout+result.stderr);self.assertIn('differ',result.stdout+result.stderr)
    def test_check_rejects_missing_typescript_before_running_tests(self):
        contract=self.base/'contract';(contract/'tests').mkdir(parents=True);(contract/'tests/test_tcc_fan_error_contract.py').write_text('raise AssertionError("must not run")\n');public=contract/'packaging/arch/tuxedo-control-center-mechrevo';public.mkdir(parents=True)
        for name in PATCHES:shutil.copy2(RECIPE/name,public/name)
        env=os.environ.copy();env['TCC_CONTRACT_ROOT']=str(contract);result=self.invoke('check',env);self.assertNotEqual(result.returncode,0);self.assertNotIn('must not run',result.stderr)
    def test_corrupted_sixth_patch_is_rejected(self):
        (self.src/PATCHES[5]).write_text('invalid temperature patch body\n');result=self.invoke('prepare');self.assertNotEqual(result.returncode,0,result.stdout+result.stderr)
    def test_check_rejects_sixth_patch_different_from_reviewed_root(self):
        contract=self.base/'contract';(contract/'tests').mkdir(parents=True);(contract/'tests/test_tcc_fan_error_contract.py').write_text('raise AssertionError("must not run")\n');public=contract/'packaging/arch/tuxedo-control-center-mechrevo';public.mkdir(parents=True)
        for name in PATCHES:shutil.copy2(RECIPE/name,public/name)
        (public/PATCHES[5]).write_text('unreviewed temperature drift\n');env=os.environ.copy();env['TCC_CONTRACT_ROOT']=str(contract);result=self.invoke('check',env);self.assertNotEqual(result.returncode,0);self.assertNotIn('must not run',result.stderr);self.assertIn(PATCHES[5],result.stdout+result.stderr)
    def test_build_refuses_unqualified_node_before_dependency_install(self):
        tools=self.base/'tools';tools.mkdir();node=tools/'node';node.write_text('#!/bin/sh\nprintf "v99.0.0\\n"\n');node.chmod(0o755);npm=tools/'npm';marker=self.base/'npm-called';npm.write_text('#!/bin/sh\ntouch "'+str(marker)+'"\nprintf "11.19.1\\n"\n');npm.chmod(0o755);env=os.environ.copy();env['PATH']=str(tools)+':/usr/bin:/bin';result=self.invoke('build',env);self.assertNotEqual(result.returncode,0);self.assertIn('Use reviewed private Node24.22.0/npm11.19.1 PATH',result.stderr);self.assertFalse(marker.exists())
    def test_package_function_preserves_outputs_and_has_no_activation_hook(self):
        prepared=self.invoke('prepare');self.assertEqual(prepared.returncode,0,prepared.stderr);resource=self.source/'dist/packages/linux-unpacked/resources';resource.mkdir(parents=True);(resource/'app.asar').write_bytes(b'opaque-built-asar-fixture');daemon=resource/'dist/tuxedo-control-center/data/service/tccd';daemon.parent.mkdir(parents=True);daemon.write_bytes(b'opaque-built-daemon-fixture');daemon.chmod(0o755);other=resource/'app.asar.unpacked/native.node';other.parent.mkdir();other.write_bytes(b'opaque-built-native-fixture')
        tools=self.base/'tools';tools.mkdir();stub=tools/'systemctl';marker=self.base/'service-call';stub.write_text('#!/bin/sh\ntouch "'+str(marker)+'"\nexit 99\n');stub.chmod(0o755);env=os.environ.copy();env['PATH']=str(tools)+':/usr/bin:/bin';result=self.invoke('package',env);self.assertEqual(result.returncode,0,result.stdout+result.stderr)
        target=self.pkg/'usr/lib/tuxedo-control-center';self.assertEqual((target/'app.asar').read_bytes(),b'opaque-built-asar-fixture');self.assertEqual((target/'data/service/tccd').read_bytes(),b'opaque-built-daemon-fixture');self.assertEqual(stat.S_IMODE((target/'data/service/tccd').stat().st_mode),0o755);self.assertEqual((target/'app.asar.unpacked/native.node').read_bytes(),b'opaque-built-native-fixture');self.assertEqual((self.pkg/'usr/bin/tuxedo-control-center').read_text(),'#!/bin/sh\nexec /usr/bin/electron42 /usr/lib/tuxedo-control-center/app.asar "$@"\n');self.assertFalse(marker.exists());self.assertFalse((self.pkg/'.INSTALL').exists());self.assertFalse((self.pkg/'usr/share/libalpm/hooks').exists())
if __name__=='__main__':unittest.main()
