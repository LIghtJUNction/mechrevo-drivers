"""Actual charging/USB driver functions with EC/sysfs mocks and pthread schedules.

UNIWILL_CHARGING_BASELINE selects the old header without rewriting its functions.
This suite never accesses real hardware. Cache-window tests mock the EC API;
a second binary combines the actual charging and shared-provider function bodies.
"""
import os
from pathlib import Path
import re
import resource
import subprocess
import tempfile
import unittest

from test_uniwill_ec_transaction import function

ROOT = Path(__file__).resolve().parents[1]


def versioned_function(source, name):
    """Preserve the original conditional platform-remove signature."""
    match = re.search(r'(?m)^#if LINUX_VERSION_CODE < KERNEL_VERSION\([^\n]+\)\n'
                      r'static int ' + re.escape(name) + r'\(', source)
    if not match:
        raise AssertionError('Missing versioned function: ' + name)
    opening = source.index('\n{', match.end()) + 1
    depth, position = 1, opening + 1
    while depth and position < len(source):
        depth += (source[position] == '{') - (source[position] == '}')
        position += 1
    if depth:
        raise AssertionError('Unbalanced versioned function: ' + name)
    return source[match.start():position]


class UniwillChargingRuntime(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        parent = Path(os.environ.get('UNIWILL_TEST_TMPDIR', ROOT / '.local-tests'))
        parent.mkdir(parents=True, exist_ok=True)
        cls.temp = tempfile.TemporaryDirectory(prefix='charging-runtime-', dir=parent)
        cls.addClassCleanup(cls.temp.cleanup)
        header = Path(os.environ.get('UNIWILL_CHARGING_BASELINE', ROOT / 'src/uniwill_keyboard.h'))
        keyboard = header.read_text()
        names = ('uw_set_charging_priority_locked', 'uw_set_charging_priority',
                 'uw_get_charging_priority', 'uw_charging_priority_write_state',
                 'uw_charging_priority_init', 'uw_set_charging_profile_locked',
                 'uw_set_charging_profile', 'uw_get_charging_profile',
                 'uw_charging_profile_write_state', 'uw_charging_profile_init',
                 'uw_charging_uninit', 'uw_charging_profile_store',
                 'uw_charging_prio_store', 'uw_set_usb_powershare')
        pieces = []
        for name in names:
            if re.search(r'(?m)^static (?:int|void|ssize_t) ' + name + r'\([^;{]*\)\s*\{', keyboard):
                # The generic existing parser supports the other return types.
                if name.endswith('_store'):
                    adapted = keyboard.replace('static ssize_t ' + name,
                                               'static int ' + name)
                    body = function(adapted, name).replace('static int ' + name,
                                                          'static ssize_t ' + name, 1)
                else:
                    body = function(keyboard, name)
                pieces.append(body)
        declarations = re.findall(r'(?m)^static (?:bool|u8) uw_(?:charging_(?:prio|profile)_[a-z_]+|usb_powershare_last_written_value)[^\n]*;', keyboard)
        pieces.insert(0, '\n'.join(declarations))
        for name in ('charging_profile_options', 'charging_prio_options'):
            pieces.insert(1, re.search(r'static struct char_to_u8_t ' + name + r'\[\] = \{.*?\n\};', keyboard, re.S).group())
        pieces.append(versioned_function(keyboard, 'uniwill_keyboard_remove'))
        fixture = (ROOT / 'tests/fixtures/uniwill_charging_runtime.c').read_text()
        fixture = fixture.replace('/* @HAS_VALID@ */', '#define HAS_VALID 1' if 'uw_charging_prio_state_valid;' in keyboard else '#define HAS_VALID 0')
        fixture = fixture.replace('/* @HAS_UPDATE@ */', '#define HAS_UPDATE 1' if 'uw_set_charging_priority_locked(' in keyboard else '#define HAS_UPDATE 0')
        actual_ec = getattr(cls, 'actual_ec', False)
        fixture = fixture.replace('/* @ACTUAL_EC@ */', '#define ACTUAL_EC ' + str(int(actual_ec)))
        provider = (ROOT / 'src/uniwill_keyboard.h').read_text()
        ec_pieces = [function(provider, name) for name in
                     ('uniwill_read_ec_ram_unlocked', 'uniwill_read_ec_ram',
                      'uniwill_write_ec_ram_unlocked', 'uniwill_write_ec_ram',
                      'uniwill_update_ec_ram')] if actual_ec else []
        fixture = fixture.replace('/* @EC_FUNCTIONS@ */', '\n\n'.join(ec_pieces))
        source = Path(cls.temp.name) / 'runtime.c'
        source.write_text(fixture.replace('/* @DRIVER_FUNCTIONS@ */', '\n\n'.join(pieces)))
        cls.binary = Path(cls.temp.name) / 'runtime'
        result = subprocess.run(['cc', '-std=c11', '-O2', '-Wall', '-Wextra', '-Werror',
                                 '-pthread', str(source), '-o', str(cls.binary)],
                                capture_output=True, text=True)
        if result.returncode:
            raise AssertionError(result.stdout + result.stderr)

    def check_case(self, case):
        def no_core():
            resource.setrlimit(resource.RLIMIT_CORE, (0, 0))
        result = subprocess.run([str(self.binary), case], capture_output=True, text=True,
                                timeout=10, preexec_fn=no_core)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_priority_all_bytes_preserve_unrelated_bits_and_cache(self):
        self.check_case('priority_bytes')

    def test_profile_all_bytes_preserve_unrelated_bits_and_cache(self):
        self.check_case('profile_bytes')

    def test_successful_getters_decode_all_register_bytes(self):
        self.check_case('get_bytes')

    def test_failed_getters_leave_output_unchanged(self):
        self.check_case('get_errors')

    def test_setter_errors_preserve_cache_and_original_errno(self):
        self.check_case('set_errors')

    def test_priority_parallel_setters_keep_hardware_and_saved_value_together(self):
        self.check_case('priority_setters')

    def test_profile_parallel_setters_keep_hardware_and_saved_value_together(self):
        self.check_case('profile_setters')

    def test_priority_restore_then_sysfs_keeps_new_request(self):
        self.check_case('priority_restore_first')

    def test_profile_restore_then_sysfs_keeps_new_request(self):
        self.check_case('profile_restore_first')

    def test_priority_sysfs_then_restore_does_not_capture_old_cache(self):
        self.check_case('priority_set_first')

    def test_profile_sysfs_then_restore_does_not_capture_old_cache(self):
        self.check_case('profile_set_first')

    def test_failed_init_does_not_restore_unknown_priority(self):
        self.check_case('priority_init_failure')

    def test_failed_init_does_not_restore_unknown_profile(self):
        self.check_case('profile_init_failure')

    def test_failed_init_allows_explicit_sysfs_recovery(self):
        self.check_case('init_recovery')

    def test_failed_setter_does_not_validate_unknown_initial_cache(self):
        self.check_case('invalid_set_failure')

    def test_sysfs_store_between_publication_and_loaded_assignment_is_preserved(self):
        self.check_case('publication_window')

    def test_priority_publication_cannot_overwrite_new_sysfs_value(self):
        self.check_case('priority_publication')

    def test_profile_publication_cannot_overwrite_new_sysfs_value(self):
        self.check_case('profile_publication')

    def test_registration_failure_and_rebind_clear_saved_lifecycle(self):
        self.check_case('registration_rebind')

    def test_remove_drains_active_sysfs_without_holding_charging(self):
        self.check_case('remove_drain')

    def test_restore_error_is_reported_and_saved_value_can_retry(self):
        self.check_case('restore_retry')

    def test_priority_allocation_failure_returns_enomem_without_ec(self):
        self.check_case('priority_alloc')

    def test_profile_allocation_failure_returns_enomem_without_ec(self):
        self.check_case('profile_alloc')

    def test_sysfs_preserves_whitespace_invalid_and_transport_error_behavior(self):
        self.check_case('parse')

    def test_sysfs_copy_respects_size_without_reading_trailing_bytes(self):
        self.check_case('bounded')

    def test_usb_all_bytes_and_failures_preserve_unrelated_bits(self):
        self.check_case('usb_bytes')

    def test_usb_uses_shared_transaction_when_rgb_bit_is_updated(self):
        self.check_case('usb_rgb')


class UniwillChargingECIntegration(unittest.TestCase):
    actual_ec = True
    setUpClass = classmethod(UniwillChargingRuntime.setUpClass.__func__)
    check_case = UniwillChargingRuntime.check_case

    def test_real_provider_priority_preserves_all_byte_combinations(self):
        self.check_case('priority_bytes')

    def test_real_provider_profile_preserves_all_byte_combinations(self):
        self.check_case('profile_bytes')

    def test_real_provider_usb_preserves_all_byte_combinations_and_errors(self):
        self.check_case('usb_bytes')

    def test_real_provider_serializes_usb_with_other_bit_update(self):
        self.check_case('usb_rgb')

    def test_real_provider_getter_failure_keeps_output(self):
        self.check_case('get_errors')

    def test_real_provider_setter_errors_keep_saved_state(self):
        self.check_case('set_errors')


if __name__ == '__main__':
    unittest.main()
