"""Portable actual TCC settings read/migration tests; never run daemon main/native I/O."""
# SPDX-License-Identifier: GPL-3.0-or-later
import hashlib,json,os,pathlib,shutil,subprocess,unittest
import test_tcc_packaging as packaging
from tcc_test_log import write_command_log
ROOT=pathlib.Path(__file__).resolve().parents[1]
FIXTURE=ROOT/'tests/fixtures/tcc_3_0_10'
GROUPS=['valid','stringfalse','schema','ioerrors','malformed','reload','missingfile','missingfields','writeerrors','ports','compatibility','rollback','profiles','mainorder']
NEGATIVE={'stringfalse','schema','ioerrors','malformed','reload','missingfields','writeerrors','rollback'}
class TccSettingsContract(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        if not all(os.environ.get(k) for k in ('TCC_NODE','TCC_TYPESCRIPT_MODULE')):
            raise unittest.SkipTest('explicit existing Node/locked TypeScript required; no automatic install')
        cls.node=os.environ['TCC_NODE'];cls.ts=os.environ['TCC_TYPESCRIPT_MODULE'];cls.command_count=0
        assert pathlib.Path(cls.node).is_file() and pathlib.Path(cls.ts).exists()
        cls.fixture=packaging.TccPackaging();cls.fixture.setUp();cls.addClassCleanup(cls.fixture.tearDown)
        result=cls.fixture.invoke('prepare');assert result.returncode==0,result.stdout+result.stderr
        cls.prepared=cls.fixture.source;cls.pkg4=cls.fixture.base/'pkg4';shutil.copytree(cls.prepared,cls.pkg4)
        for patch in reversed(packaging.PATCHES[4:]):
            result=subprocess.run(['patch','-R','-p1','--fuzz=0','--batch','-i',str(packaging.RECIPE/patch)],cwd=cls.pkg4,capture_output=True,text=True)
            assert result.returncode==0,result.stdout+result.stderr
        cls.generated=cls.fixture.base/'settings-generated-js';cls.generated.mkdir()
    def run_case(self,group,source=None,kind='ts',success=True):
        argv=[self.node,str(FIXTURE/'settings_contract_test.cjs'),self.ts,str(source or self.prepared),group,kind,str(self.generated)]
        result=subprocess.run(argv,capture_output=True,text=True,timeout=30)
        if os.environ.get('TCC_TEST_LOG_DIR'):
            self.__class__.command_count+=1
            write_command_log(os.environ['TCC_TEST_LOG_DIR'], self.__class__.__name__,
                              self.__class__.command_count, argv, result.returncode, result.stdout+result.stderr)
        if success:self.assertEqual(result.returncode,0,result.stdout+result.stderr)
        else:self.assertNotEqual(result.returncode,0,result.stdout+result.stderr);self.assertIn('AssertionError',result.stderr)
        return result.stdout
    def test_actual_settings_ts_and_independent_generated_js(self):
        self.run_case('emit')
        for group in GROUPS:
            for kind in ['ts','js']:
                with self.subTest(group=group,kind=kind):self.run_case(group,kind=kind)
    def test_pkg4_independent_negatives_and_compatible_paths(self):
        # Reverse patches6 and5 from real current recipe prepare, preserving patches1..4.
        for group in GROUPS:
            with self.subTest(group=group):self.run_case(group,source=self.pkg4,success=group not in NEGATIVE)
    def test_prepared_candidate_and_pkg4_reverse_have_exact_qualified_bytes(self):
        manifest=json.loads((FIXTURE/'provenance.json').read_text())
        for name,digest in manifest['patched_files_sha256'].items():
            self.assertEqual(hashlib.sha256((self.prepared/name).read_bytes()).hexdigest(),digest,name)
        for name,digest in manifest['patch4_settings_files_sha256'].items():
            self.assertEqual(hashlib.sha256((self.pkg4/name).read_bytes()).hexdigest(),digest,name)
        self.assertEqual(hashlib.sha256((packaging.RECIPE/packaging.PATCHES[4]).read_bytes()).hexdigest(),manifest['patch5_sha256'])
    def test_original_fixture_and_harness_provenance(self):
        manifest=json.loads((FIXTURE/'provenance.json').read_text())
        for name,record in manifest['files'].items():
            data=(FIXTURE/'upstream'/name).read_bytes();self.assertEqual(len(data),record['size']);self.assertEqual(hashlib.sha256(data).hexdigest(),record['sha256'])
        data=(FIXTURE/'settings_contract_test.cjs').read_bytes();self.assertEqual(hashlib.sha256(data).hexdigest(),manifest['harness_sha256']['settings_contract_test.cjs'])
        self.assertNotIn(b'.local-tests',data);self.assertNotIn(b'/home/',data)
if __name__=='__main__':unittest.main()
