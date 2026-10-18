"""Actual TCC CPU worker, ownership store and DBus adapter; no production daemon or hardware I/O."""
# SPDX-License-Identifier: GPL-3.0-or-later
import hashlib
import json
import os
import pathlib
import shutil
import subprocess
import unittest

import test_tcc_packaging as packaging
from tcc_test_log import write_command_log

FIXTURE = packaging.FIXTURE.parent


class TccCpuOwnership(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        if not all(os.environ.get(k) for k in ('TCC_NODE', 'TCC_TYPESCRIPT_MODULE')):
            raise unittest.SkipTest('explicit existing Node/locked TypeScript required; no automatic install')
        cls.fixture = packaging.TccPackaging()
        cls.fixture.setUp()
        cls.addClassCleanup(cls.fixture.tearDown)
        result = cls.fixture.invoke('prepare')
        assert result.returncode == 0, result.stdout + result.stderr
        cls.prepared = cls.fixture.source
        cls.legacy = cls.fixture.base / 'pkg6'
        shutil.copytree(cls.prepared, cls.legacy)
        result = subprocess.run(['patch', '-R', '-p1', '--fuzz=0', '--batch', '-i',
                                 str(packaging.RECIPE / packaging.PATCHES[6])],
                                cwd=cls.legacy, text=True, capture_output=True)
        assert result.returncode == 0, result.stdout + result.stderr
        cls.count = 0

    def case(self, group, *, old=False, success=True):
        self.__class__.count += 1
        scratch = self.fixture.base / ('cpu-case-' + str(self.count))
        scratch.mkdir(mode=0o700)
        args = [os.environ['TCC_NODE'], str(FIXTURE / 'cpu_ownership_test.cjs'),
                os.environ['TCC_TYPESCRIPT_MODULE'], str(self.prepared), str(scratch), group,
                str(self.legacy if old else self.prepared), os.environ.get('TCC_DBUS_MODULE', '')]
        result = subprocess.run(args, capture_output=True, text=True, timeout=45)
        if os.environ.get('TCC_TEST_LOG_DIR'):
            write_command_log(os.environ['TCC_TEST_LOG_DIR'], self.__class__.__name__,
                              self.count, args, result.returncode, result.stdout + result.stderr)
        if success:
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertIn('"passed":true', result.stdout)
        else:
            self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertIn('AssertionError', result.stderr)
        return result

    def test_roundtrip_persistence_noop_and_no_other_settings_mutation(self): self.case('roundtrip')
    def test_actual_worker_start_work_and_profile_change_are_fenced(self): self.case('boundaries')
    def test_actual_worker_exit_cannot_reclaim_external_cpu_policy(self): self.case('exit')
    def test_actual_private_write_methods_share_the_same_gate(self): self.case('direct')
    def test_release_acknowledges_after_synchronous_writes_and_blocks_queued_work(self): self.case('fence')
    def test_request_is_bound_to_profile_settings_instance_and_file_revision(self): self.case('stale')
    def test_restart_preserves_owner_and_invalidates_old_requests(self): self.case('restart')
    def test_noncanonical_corrupt_oversized_and_exhausted_records_fail_closed(self): self.case('schema')
    def test_file_directory_identity_permissions_and_links_are_checked(self): self.case('files')
    def test_write_rename_sync_readback_cleanup_and_external_change_failures(self): self.case('failures')
    def test_cpu_replay_success_failure_and_disabled_setting_are_separate_from_persistence(self): self.case('replay')
    def test_empty_cpu_inventory_does_not_report_a_verified_replay(self): self.case('empty-cpu')
    def test_actual_daemon_startup_wires_the_persisted_owner_before_cpu_start(self): self.case('startup')
    def test_real_dbus_service_interface_calls_only_the_cpu_authority(self): self.case('dbus')

    def test_old_worker_is_caught_on_start_work_exit_and_private_methods(self):
        for group in ('boundaries', 'exit', 'direct'):
            with self.subTest(group=group):
                result = self.case(group, old=True, success=False)
                self.assertIn('external owner must block every CPU write path', result.stderr)

    def test_qualified_originals_current_candidate_and_previous_stage_have_exact_bytes(self):
        manifest = json.loads((FIXTURE / 'provenance.json').read_text())
        for name, record in manifest['files'].items():
            raw = (packaging.FIXTURE / name).read_bytes()
            self.assertEqual(len(raw), record['size'], name)
            self.assertEqual(hashlib.sha256(raw).hexdigest(), record['sha256'], name)
        for root, key in ((self.prepared, 'patched_files_sha256'), (self.legacy, 'patch6_files_sha256')):
            for name, digest in manifest[key].items():
                self.assertEqual(hashlib.sha256((root / name).read_bytes()).hexdigest(), digest, name)
        self.assertEqual(hashlib.sha256((packaging.RECIPE / packaging.PATCHES[6]).read_bytes()).hexdigest(), manifest['patch7_sha256'])
        self.assertEqual(hashlib.sha256((FIXTURE / 'cpu_ownership_test.cjs').read_bytes()).hexdigest(), manifest['harness_sha256']['cpu_ownership_test.cjs'])

    def test_reviewed_bus_policy_denies_unprivileged_named_and_unique_owner_calls(self):
        if not os.environ.get('TCC_DBUS_MODULE') or not shutil.which('dbus-daemon'):
            self.skipTest('explicit existing dbus-next module and dbus-daemon required; no automatic install')
        self.case('policy')

    def test_old_bus_policy_is_caught_by_actual_unprivileged_rpc(self):
        if not os.environ.get('TCC_DBUS_MODULE') or not shutil.which('dbus-daemon'):
            self.skipTest('explicit existing dbus-next module and dbus-daemon required; no automatic install')
        result = self.case('policy-old', old=True, success=False)
        self.assertIn('unprivileged CPU setter must be denied', result.stderr)


if __name__ == '__main__': unittest.main()
