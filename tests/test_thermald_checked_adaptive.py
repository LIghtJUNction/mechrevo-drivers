"""Pinned actual adaptive startup/GDDV paths; no daemon or hardware execution."""
import os
from pathlib import Path
import tempfile
import unittest

from thermald_adaptive_check import build, prepare_source, run, verify_sources

ROOT = Path(__file__).resolve().parents[1]


class ThermaldCheckedAdaptive(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        parent = Path(os.environ.get('MECHREVO_TEST_TMPDIR', ROOT / '.local-tests'))
        parent.mkdir(parents=True, exist_ok=True)
        cls.tmp = tempfile.TemporaryDirectory(prefix='thermald-adaptive-', dir=parent)
        cls.addClassCleanup(cls.tmp.cleanup)
        cls.output = Path(cls.tmp.name)
        cls.checked = prepare_source(ROOT, cls.output / 'checked')
        cls.upstream = prepare_source(ROOT, cls.output / 'upstream', checked=False)
        cls.binaries = {}
        for variant, source, mutation in [
            ('checked', cls.checked, None),
            ('upstream', cls.upstream, None),
            ('no-cleanup', cls.checked, 'no-cleanup'),
            ('sentinel-no-match', cls.checked, 'sentinel-no-match'),
        ]:
            cls.binaries[variant], _ = build(ROOT, source, cls.output / variant / 'build', mutation)

    def case(self, name, variant='checked'):
        return run(self.binaries[variant], name, self.output / variant / name)

    def assert_passes(self, name, variant='checked'):
        result = self.case(name, variant)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(result.stdout, name + ': passed\n')
        self.assertEqual(result.stderr, '')

    def assert_regression(self, name, variant, assertion):
        result = self.case(name, variant)
        # Signals, compiler errors and unrelated failures are never accepted.
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertEqual(result.stderr, assertion + '\n')
        self.assertEqual(result.stdout, '')

    def test_invalid_sentinel_preserves_target_policy_and_condition_time_history(self):
        self.assert_passes('invalid')
        self.assert_regression('invalid', 'upstream',
                               'engine must preserve output on invalid sensor read')
        self.assert_regression('invalid', 'sentinel-no-match',
                               'evaluated invalid temperature must return sensor sentinel')

    def test_actual_predicate_short_circuit_is_preserved(self):
        self.assert_passes('short-circuit')
        self.assert_regression('short-circuit', 'upstream',
                               'applicable temperature predicate must propagate invalid and block later default')

    def test_both_invalid_startup_paths_block_default_aggressive_and_base_start(self):
        for case in ['startup-supported', 'startup-unsupported']:
            with self.subTest(case=case):
                self.assert_passes(case)
                for variant in ['upstream', 'sentinel-no-match']:
                    self.assert_regression(case, variant,
                                           'invalid startup must not enter aggressive/default/base startup fallback')

    def test_actual_cleanup_closes_regular_fds_and_handles_aliases_exactly_once(self):
        for case in ['startup-supported', 'startup-unsupported']:
            with self.subTest(case=case):
                self.assert_passes(case)
                self.assert_regression(case, 'no-cleanup',
                                       'invalid startup must actually close owned regular FDs')

    def test_valid_zero_and_true_no_match_retain_original_default_paths(self):
        for variant in ['upstream', 'checked']:
            with self.subTest(variant=variant):
                self.assert_passes('valid-no-match', variant)

    def test_prepared_build_source_matches_pinned_and_patched_inputs(self):
        source = os.environ.get('THERMALD_PREPARED_SOURCE')
        if source is None:
            self.skipTest('Prepared package source was not supplied')
        source = Path(source)
        for name in verify_sources(ROOT):
            with self.subTest(file=name):
                self.assertEqual((source / name).read_bytes(), (self.checked / name).read_bytes())


if __name__ == '__main__':
    unittest.main()
