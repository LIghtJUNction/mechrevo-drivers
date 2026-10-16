"""Run real TCC IPC/webcam functions using explicit existing Node/TypeScript only."""
import os,pathlib,subprocess,unittest
import test_tcc_packaging as packaging
ROOT=pathlib.Path(__file__).resolve().parents[1]
FIXTURE=ROOT/'tests/fixtures/tcc_3_0_10'
class TccResourcePaths(unittest.TestCase):
    def setUp(self):
        if not all(os.environ.get(k) for k in ('TCC_NODE','TCC_TYPESCRIPT_MODULE')):
            self.skipTest('explicit existing Node/locked TypeScript required; no automatic install')
        self.fixture=packaging.TccPackaging();self.fixture.setUp();self.addCleanup(self.fixture.tearDown)
        result=self.fixture.invoke('prepare');self.assertEqual(result.returncode,0,result.stdout+result.stderr)
    def run_case(self,scenario,source=None):
        workspace=self.fixture.base/scenario;workspace.mkdir()
        result=subprocess.run([os.environ['TCC_NODE'],str(FIXTURE/'resource_paths_test.cjs'),os.environ['TCC_TYPESCRIPT_MODULE'],str(source or self.fixture.source),str(workspace),scenario],capture_output=True,text=True)
        self.assertEqual(result.returncode,0,result.stdout+result.stderr)
        self.assertIn('"passed":true',result.stdout)
    def test_production_installed_resource_paths(self):self.run_case('production')
    def test_system_electron_asar_with_ispackaged_false(self):self.run_case('asar')
    def test_development_keeps_development_paths(self):self.run_case('development')
    def test_missing_daemon_reports_failure(self):self.run_case('missing-target')
    def test_missing_camera_rejects_read_and_filter_calls(self):self.run_case('missing-camera')
    def test_actual_config_write_failure_prevents_process_launch(self):self.run_case('write-failure')
    def test_launch_failure_reports_failure(self):self.run_case('launch-failure')
    def test_old_synchronous_caller_false_success_after_launch_failure(self):
        relative=pathlib.Path('src/e-app/backendAPIs/configAPI.ts')
        (self.fixture.source/relative).write_bytes((packaging.FIXTURE/relative).read_bytes())
        self.run_case('old-sync-failure')
    def test_old_selector_fails_on_system_electron_asar(self):
        # Keep relocation fixed and restore only the original real callers: isolates selector regression.
        for name in ('configAPI.ts','webcamAPI.ts'):
            relative=pathlib.Path('src/e-app/backendAPIs')/name
            (self.fixture.source/relative).write_bytes((packaging.FIXTURE/relative).read_bytes())
        self.run_case('old-asar')
if __name__=='__main__':unittest.main()
