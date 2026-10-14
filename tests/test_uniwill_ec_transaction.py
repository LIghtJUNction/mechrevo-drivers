"""Execute real provider read/write/update with deterministic pthread schedules.

The optional header selector is for explicit source mutations. No physical EC
or kernel module is accessed. The transport callbacks are service doubles.
"""
import os
from pathlib import Path
import re
import resource
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[1]


def function(source, name):
    match = re.search(r"(?m)^(?:static\s+)?(?:int|u32|void)\s+" + re.escape(name)
                      + r"\([^;{]*\)\s*\{", source)
    if not match:
        raise AssertionError("Missing driver function: " + name)
    depth, position = 1, match.end()
    while depth and position < len(source):
        depth += (source[position] == "{") - (source[position] == "}")
        position += 1
    if depth:
        raise AssertionError("Unbalanced driver function: " + name)
    return source[match.start():position]

class UniwillECTransaction(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        parent = Path(os.environ.get('UNIWILL_TEST_TMPDIR', ROOT / '.local-tests'))
        parent.mkdir(parents=True, exist_ok=True)
        cls.temp = tempfile.TemporaryDirectory(prefix='ec-transaction-', dir=parent)
        cls.addClassCleanup(cls.temp.cleanup)
        path = Path(cls.temp.name)
        header = Path(os.environ.get('UNIWILL_EC_TRANSACTION_HEADER', ROOT / 'src/uniwill_keyboard.h'))
        source = header.read_text()
        interfaces = (ROOT / 'src/uniwill_interfaces.h').read_text()
        typedefs = re.findall(r'(?m)^typedef (?:int|void) \(uniwill_[^\n]+;', interfaces)
        structure = re.search(r'(?ms)^struct uniwill_interface_t \{.*?^};', interfaces).group()
        lock = re.search(r'(?m)^static DEFINE_MUTEX\(uniwill_ec_transaction_lock\);', source).group()
        pieces = [lock] + [function(source, name) for name in (
            'uniwill_read_ec_ram_unlocked', 'uniwill_read_ec_ram',
            'uniwill_write_ec_ram_unlocked', 'uniwill_write_ec_ram',
            'uniwill_update_ec_ram', 'uniwill_read_ec_ram_with_retry')]
        fixture = (ROOT / 'tests/fixtures/uniwill_ec_transaction.c').read_text()
        fixture = fixture.replace('/* @INTERFACE_TYPES@ */', '\n'.join(typedefs + [structure]))
        fixture = fixture.replace('/* @DRIVER_FUNCTIONS@ */', '\n\n'.join(pieces))
        c_source = path / 'runtime.c'
        c_source.write_text(fixture)
        cls.binary = path / 'runtime'
        build = subprocess.run(['cc', '-std=c11', '-O2', '-Wall', '-Wextra', '-Werror',
                                '-pthread', str(c_source), '-o', str(cls.binary)],
                               capture_output=True, text=True)
        if build.returncode:
            raise AssertionError(build.stdout + build.stderr)

    def check_case(self, case):
        def no_core():
            resource.setrlimit(resource.RLIMIT_CORE, (0, 0))
        result = subprocess.run([str(self.binary), case], capture_output=True, text=True,
                                timeout=30, preexec_fn=no_core)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_all_16777216_initial_clear_set_byte_combinations(self):
        self.check_case('all_bytes_masks')

    def test_addresses_include_both_u16_boundaries_without_truncation(self):
        self.check_case('addresses')

    def test_zero_masks_preserve_the_defined_read_write_sequence(self):
        self.check_case('unchanged')

    def test_read_errno_stops_before_any_write_and_unlocks_for_recovery(self):
        self.check_case('read_errors')

    def test_write_errno_returns_unchanged_without_retry_or_rollback(self):
        self.check_case('write_errors')

    def test_failed_transport_may_have_side_effect_without_guessed_rollback(self):
        self.check_case('partial_write_error')

    def test_absent_or_error_interface_keeps_eio_and_does_not_call_transport(self):
        self.check_case('missing_interface')

    def test_null_read_pointer_is_passed_through_for_existing_provider_validation(self):
        self.check_case('null_read')

    def test_plain_read_preserves_all_byte_results_original_errno_and_output(self):
        self.check_case('plain_read')

    def test_plain_write_preserves_all_byte_inputs_and_original_errno(self):
        self.check_case('plain_write')

    def test_update_then_plain_write_cannot_interleave_after_sample(self):
        self.check_case('update_write')

    def test_plain_write_then_update_uses_completed_write(self):
        self.check_case('write_update')

    def test_disjoint_updates_keep_both_fields_in_either_order(self):
        self.check_case('two_updates')

    def test_overlapping_updates_follow_complete_transaction_order(self):
        self.check_case('overlap_updates')

    def test_plain_read_waits_for_completed_update(self):
        self.check_case('update_read')

    def test_update_waits_for_completed_plain_read(self):
        self.check_case('read_update')

    def test_repeated_failure_and_recovery_never_recursively_lock_or_leak_lock(self):
        self.check_case('lock_release')


    def test_read_retry_rejects_zero_negative_and_int_min_without_ec_access(self):
        self.check_case('retry_invalid')

    def test_read_retry_first_success_stops_even_with_int_max_budget(self):
        self.check_case('retry_first_success')

    def test_read_retry_transient_errors_then_success_preserve_byte_and_attempt_count(self):
        self.check_case('retry_transient')

    def test_read_retry_exhaustion_returns_each_original_errno_and_keeps_output(self):
        self.check_case('retry_exhausted')

    def test_read_retry_budget_stops_before_later_success_and_returns_last_errno(self):
        self.check_case('retry_budget')

    def test_read_retry_missing_interface_keeps_eio_without_transport_calls(self):
        self.check_case('retry_missing')

    def test_read_retry_null_pointer_preserves_provider_errno_per_attempt(self):
        self.check_case('retry_null')


if __name__ == '__main__':
    unittest.main()
