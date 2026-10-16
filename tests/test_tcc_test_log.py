"""Unique receipts through the actual TCC test command entry points; no hardware."""
# SPDX-License-Identifier: GPL-3.0-or-later
import concurrent.futures
import json
import os
from pathlib import Path
import stat
import sys
import tempfile
import threading
import unittest

from tcc_test_log import write_command_log
import test_tcc_fan_error_contract as fan
import test_tcc_settings_contract as settings


class TccTestLog(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(dir=os.environ.get('MECHREVO_TEST_TMPDIR'))
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.logs = self.root / 'logs'
        self.logs.mkdir()
        # Preserve environment without patching any standard-library function.
        prior = os.environ.get('TCC_TEST_LOG_DIR')
        self.addCleanup(self.restore_log_dir, prior)
        os.environ['TCC_TEST_LOG_DIR'] = str(self.logs)

    @staticmethod
    def restore_log_dir(prior):
        if prior is None:
            os.environ.pop('TCC_TEST_LOG_DIR', None)
        else:
            os.environ['TCC_TEST_LOG_DIR'] = prior

    def actual_entries(self):
        f = fan.TccFanContract()
        s = settings.TccSettingsContract()
        tool = self.root / 'mock-node'
        tool.write_text('#!' + sys.executable + '\nimport json,sys\nprint(json.dumps(sys.argv[1:]))\n')
        tool.chmod(0o700)
        s.node = str(tool)
        s.ts = 'mock-locked-typescript'
        s.prepared = self.root / 'mock-source'
        s.generated = self.root / 'mock-generated'
        return f, s

    def records(self):
        files = sorted(self.logs.glob('*.json'))
        return [(p, json.loads(p.read_text())) for p in files]

    def test_repeated_identical_labels_and_existing_receipts_never_overwrite(self):
        existing = self.logs / 'TccFanContract-001-fixed.json'
        existing.write_text('{"original": true}\n')
        before = existing.read_bytes(), existing.stat().st_ino
        paths = [write_command_log(self.logs, 'TccFanContract', 1,
                                  [Path('program'), 'argument'], 3, 'stdout\nstderr') for _ in range(3)]
        self.assertEqual(len(set(paths)), 3)
        self.assertEqual((existing.read_bytes(), existing.stat().st_ino), before)
        for p in paths:
            self.assertEqual(stat.S_IMODE(p.stat().st_mode), 0o600)
            self.assertEqual(json.loads(p.read_text()), {'suite': 'TccFanContract', 'sequence': 1,
                             'args': ['program', 'argument'], 'returncode': 3, 'output': 'stdout\nstderr'})

    def test_actual_both_entry_points_reset_counters_twice_in_same_directory(self):
        f, s = self.actual_entries()
        for round_number in range(2):
            fan.TccFanContract.command_count = settings.TccSettingsContract.command_count = 0
            f.run_cmd([sys.executable, '-c', f'print("fan-round-{round_number}")'])
            s.run_case('settings-round-' + str(round_number))
        records = self.records()
        self.assertEqual(len(records), 4, 'counter resets must preserve every invocation from both suites')
        self.assertEqual(sorted(d['suite'] for _, d in records), ['TccFanContract']*2 + ['TccSettingsContract']*2)
        for p, d in records:
            self.assertTrue(p.name.startswith(d['suite'] + '-001-'))
            self.assertEqual(d['sequence'], 1)
            self.assertEqual(d['returncode'], 0)
            self.assertIsInstance(d['args'], list)
            self.assertIsInstance(d['output'], str)
        self.assertEqual({d['output'].strip() for _, d in records if d['suite']=='TccFanContract'},
                         {'fan-round-0', 'fan-round-1'})
        for _, d in records:
            if d['suite']=='TccSettingsContract':
                self.assertEqual(json.loads(d['output']), d['args'][1:])

    def test_actual_failing_entry_logs_before_asserting_command_result(self):
        f, _ = self.actual_entries()
        fan.TccFanContract.command_count = 0
        with self.assertRaises(AssertionError):
            f.run_cmd([sys.executable, '-c', 'print("real-failure");raise SystemExit(7)'])
        [(p, d)] = self.records()
        self.assertEqual(d['returncode'], 7)
        self.assertEqual(d['output'], 'real-failure\n')
        self.assertIn('real-failure', d['args'][-1])

    def test_two_concurrent_writers_with_same_labels_preserve_existing_file(self):
        existing = self.logs / '001.json'
        existing.write_text('{"old": "kept"}\n')
        before = existing.read_bytes()
        gate = threading.Barrier(2)
        def writer(which):
            gate.wait(timeout=5)
            return [write_command_log(self.logs, 'TccFanContract', 1,
                    ['writer', str(which), str(i)], 0, f'{which}:{i}') for i in range(12)]
        with concurrent.futures.ThreadPoolExecutor(max_workers=2) as pool:
            jobs = [pool.submit(writer, i) for i in range(2)]
            paths = [p for job in jobs for p in job.result(timeout=10)]
        self.assertEqual(len(set(paths)), 24)
        self.assertEqual(len(self.records()), 25)
        self.assertEqual(existing.read_bytes(), before)
        self.assertEqual({json.loads(p.read_text())['output'] for p in paths},
                         {f'{i}:{j}' for i in range(2) for j in range(12)})

    def test_labels_cannot_escape_selected_directory(self):
        for label in ['', '../foreign', 'a/b', 'a\\b']:
            with self.subTest(label=label), self.assertRaises(ValueError):
                write_command_log(self.logs, label, 1, [], 0, '')
        self.assertEqual(self.records(), [])


if __name__ == '__main__':
    unittest.main()
