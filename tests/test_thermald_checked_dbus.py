# SPDX-License-Identifier: GPL-2.0-or-later
"""Actual pinned DBus read/allocation/ownership paths without a daemon or bus."""
import os
from pathlib import Path
import tempfile
import unittest

from thermald_dbus_check import build, prepare_source, run, verify_sources

ROOT = Path(__file__).resolve().parents[1]


class ThermaldCheckedDBus(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        parent = Path(os.environ.get('MECHREVO_TEST_TMPDIR', ROOT / '.local-tests'))
        parent.mkdir(parents=True, exist_ok=True)
        cls.tmp = tempfile.TemporaryDirectory(prefix='thermald-dbus-', dir=parent)
        cls.addClassCleanup(cls.tmp.cleanup)
        cls.output = Path(cls.tmp.name)
        cls.checked = prepare_source(ROOT, cls.output / 'checked')
        cls.upstream = prepare_source(ROOT, cls.output / 'upstream', checked=False)
        cls.binaries = {}
        for variant, source, mutation in [
            ('checked', cls.checked, None),
            ('upstream', cls.upstream, None),
            ('read-leak', cls.checked, 'read-leak'),
            ('second-allocation-leak', cls.checked, 'second-allocation-leak'),
            ('response-type', cls.checked, 'response-type'),
        ]:
            cls.binaries[variant], _ = build(ROOT, source, cls.output / variant / 'build', mutation)

    def assert_passes(self, case, variant='checked'):
        result = run(self.binaries[variant], case)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(result.stdout, case + ': passed\n')
        self.assertEqual(result.stderr, '')

    def assert_regression(self, case, variant, assertion):
        result = run(self.binaries[variant], case)
        # Signals and unrelated failures do not satisfy a negative regression.
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertEqual(result.stderr, assertion + '\n')
        self.assertEqual(result.stdout, '')

    def test_invalid_reads_preserve_outputs_and_release_both_temporary_strings(self):
        for case in ['io', 'eof', 'negative']:
            with self.subTest(case=case):
                self.assert_passes(case)
        self.assert_regression('io', 'upstream',
                               'DBus information must reject invalid temperature')
        self.assert_regression('io', 'read-leak',
                               'DBus failed read must release both allocated strings')

    def test_first_and_second_allocation_failure_release_only_owned_memory(self):
        for variant in ['upstream', 'checked']:
            for case in ['allocation-first', 'allocation-second']:
                with self.subTest(variant=variant, case=case):
                    self.assert_passes(case, variant)
        self.assert_regression('allocation-second', 'second-allocation-leak',
                               'DBus allocation failure must release owned temporary strings')

    def test_valid_zero_normal_and_bounded_strings_transfer_ownership_to_caller(self):
        for variant in ['upstream', 'checked']:
            for case in ['zero', 'normal', 'long-strings']:
                with self.subTest(variant=variant, case=case):
                    self.assert_passes(case, variant)

    def test_actual_gdbus_handler_branches_preserve_response_types_and_release_strings(self):
        for variant in ['upstream', 'checked']:
            for case in ['handler-zero', 'handler-normal']:
                with self.subTest(variant=variant, case=case):
                    self.assert_passes(case, variant)
        self.assert_regression('handler-normal', 'response-type',
                               'information response must retain (ssi) wire signature')

    def test_actual_failed_handlers_return_errors_without_success_payload(self):
        self.assert_passes('handler-failures')
        self.assert_regression('handler-failures', 'upstream',
                               'failed information handler must return error without payload')

    def test_invalid_sensor_indexes_do_not_allocate_or_change_outputs(self):
        for variant in ['upstream', 'checked']:
            with self.subTest(variant=variant):
                self.assert_passes('invalid-index', variant)

    def test_prepared_package_source_matches_pinned_and_patched_inputs(self):
        source = os.environ.get('THERMALD_PREPARED_SOURCE')
        if source is None:
            self.skipTest('Prepared package source was not supplied')
        source = Path(source)
        for name in verify_sources(ROOT):
            with self.subTest(file=name):
                self.assertEqual((source / name).read_bytes(), (self.checked / name).read_bytes())


if __name__ == '__main__':
    unittest.main()
