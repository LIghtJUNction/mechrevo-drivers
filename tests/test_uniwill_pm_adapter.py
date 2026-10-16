"""Actual probe, adapter, fan, suspend/resume and remove with pthread schedules.

UNIWILL_PM_BASELINE selects a frozen header without altering driver functions.
The fixture records mock EC/resource access; no physical hardware is accessed.
"""
import os
from pathlib import Path
import re
import resource
import subprocess
import tempfile
import unittest

from test_uniwill_fan_runtime import function
from test_uniwill_probe_runtime import versioned_function

ROOT = Path(__file__).resolve().parents[1]


class UniwillPMAdapter(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        parent = Path(os.environ.get('UNIWILL_TEST_TMPDIR', ROOT / '.local-tests'))
        parent.mkdir(parents=True, exist_ok=True)
        cls.temp = tempfile.TemporaryDirectory(prefix='pm-adapter-', dir=parent)
        cls.addClassCleanup(cls.temp.cleanup)
        header = Path(os.environ.get('UNIWILL_PM_BASELINE', ROOT / 'src/uniwill_keyboard.h'))
        keyboard = header.read_text()
        keyboard = re.sub(
            r'#if LINUX_VERSION_CODE < KERNEL_VERSION\(6, 11, 0\)\n'
            r'static int uniwill_keyboard_remove\([^\n]*\)\n#else\n'
            r'(static void uniwill_keyboard_remove\([^\n]*\))\n#endif', r'\1', keyboard)
        interfaces = (ROOT / 'src/uniwill_interfaces.h').read_text()
        constants = []
        for source, name in ((keyboard, 'FAN_ON_MIN_SPEED_PERCENT'),
                             (interfaces, 'NB02_FAN_SPEED_MAX')):
            constants.append(re.search(r'(?m)^#define ' + name + r'\s+.+$', source).group())
        start = keyboard.index('static u8 direct_fan_control_current_value_fan0')
        end = keyboard.index('static u8 uniwill_touchp_toggle_seq', start)
        declarations = re.search(r'static u8 uniwill_touchp_toggle_seq\[\] = \{[^}]+\};', keyboard).group()
        declarations += '\n' + re.search(r'(?m)^static bool uniwill_i8042_filter_installed;', keyboard).group()
        declarations += '\n' + re.search(r'(?m)^static u8 uniwill_touchp_toggle_seq_pos;', keyboard).group()
        pieces = constants + [function(keyboard, 'uniwill_write_ec_ram_with_retry'),
                              keyboard[start:end],
                              function(keyboard, 'uniwill_write_kbd_bl_enable'),
                              function(keyboard, 'uniwill_set_custom_profile_mode')]
        if 'static void uniwill_restore_adapter_state(void)\n{' in keyboard:
            pieces += [function(keyboard, 'uniwill_restore_adapter_state')]
        pieces += [function(keyboard, 'uniwill_event_callb'), declarations,
                   versioned_function(keyboard, 'uniwill_i8042_filter')]
        if 'static int uniwill_keyboard_abort_probe(' in keyboard:
            pieces += [function(keyboard, 'uniwill_keyboard_abort_probe')]
        pieces += [function(keyboard, name) for name in
                   ('uniwill_keyboard_prepare_remove', 'uniwill_keyboard_probe',
                    'uniwill_keyboard_remove', 'uw_restore_fan_state_locked',
                    'uniwill_keyboard_suspend', 'uniwill_keyboard_resume')]
        fixture = (ROOT / 'tests/fixtures/uniwill_pm_adapter.c').read_text()
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

    def test_profile_clear_read_and_landed_or_unlanded_write_errors_restore_prior_state(self):
        self.check_case('profile_abort_matrix')

    def test_profile_clear_error_keeps_original_errno_when_rollback_also_fails(self):
        self.check_case('profile_abort_cleanup')

    def test_profile_success_and_feature_disabled_paths_keep_existing_pm_contract(self):
        self.check_case('profile_success_and_qualification')

    def test_profile_rollback_keeps_adapter_out_until_suspend_abort_completes(self):
        self.check_case('profile_abort_concurrency')

    def test_adapter_then_suspend_keeps_final_custom_bit_cleared(self):
        self.check_case('adapter_suspend')

    def test_suspend_then_adapter_cannot_write_or_restore_resources(self):
        self.check_case('suspend_adapter')

    def test_late_adapter_in_auto_and_manual_sleep_is_silent(self):
        self.check_case('late_adapter')

    def test_successful_suspend_rejects_all_exported_fan_and_mode_writes(self):
        self.check_case('suspended_commands')

    def test_suspend_and_resume_keep_original_successful_ec_order(self):
        self.check_case('pm_order')

    def test_backlight_suspend_failure_restores_original_custom_bit_and_fans(self):
        self.check_case('suspend_backlight_failure')

    def test_suspend_cleanup_failure_keeps_original_errno_and_allows_recovery(self):
        self.check_case('suspend_cleanup_failure')

    def test_resume_profile_and_backlight_failures_preserve_manual_retry(self):
        self.check_case('resume_failures')

    def test_failed_resume_allows_auto_and_adapter_without_another_resume(self):
        self.check_case('resume_explicit_recovery')

    def test_adapter_and_remove_are_serialized_in_both_directions(self):
        self.check_case('adapter_remove')

    def test_real_probe_failure_removal_and_reprobe_gate_adapter_resources(self):
        self.check_case('lifecycle')

    def test_failed_suspend_leaves_adapter_and_manual_recovery_available(self):
        self.check_case('failed_suspend')

    def test_adapter_and_resume_complete_restoration_in_both_serial_orders(self):
        self.check_case('adapter_resume')

    def test_early_remove_gate_stops_worker_before_platform_resource_removal(self):
        self.check_case('prepare_remove_worker')

    def test_pm_gate_preserves_other_input_callback_behavior(self):
        self.check_case('other_callbacks')


if __name__ == '__main__':
    unittest.main()
