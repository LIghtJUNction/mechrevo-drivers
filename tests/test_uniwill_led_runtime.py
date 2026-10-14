#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Run actual LED callbacks/initializers through frozen official LED core code.

UNIWILL_LEDS_HEADER selects old source for sensitivity; no driver function is
rewritten for the old void callbacks. Only the test invocation boundary adapts
int/void callback types. No physical LED, timer, workqueue or EC is accessed.
"""
import os
from pathlib import Path
import re
import resource
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
HEADER = Path(os.environ.get('UNIWILL_LEDS_HEADER', ROOT / 'src/uniwill_leds.h'))


def function(source, name):
    match = re.search(r'(?m)^(?:static\s+)?(?:int|void|bool)\s+' + re.escape(name)
                      + r'\([^;{]*\)\s*\{', source)
    if not match:
        raise AssertionError('Missing actual function: ' + name)
    depth, end = 1, match.end()
    while depth and end < len(source):
        depth += (source[end] == '{') - (source[end] == '}')
        end += 1
    if depth:
        raise AssertionError('Unbalanced function: ' + name)
    return source[match.start():end]


def initializer(source, name):
    match = re.search(r'(?m)^static (?:const )?struct [^\n]+\b' + re.escape(name)
                      + r'(?:\[[^\]]*\])?\s*=\s*\{', source)
    if not match:
        raise AssertionError('Missing actual initializer: ' + name)
    depth, end = 1, match.end()
    while depth and end < len(source):
        depth += (source[end] == '{') - (source[end] == '}')
        end += 1
    if depth or source[end] != ';':
        raise AssertionError('Unbalanced initializer: ' + name)
    return source[match.start():end + 1]


class UniwillLedRuntime(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.temporary = tempfile.TemporaryDirectory(prefix='uniwill-led-runtime-')
        cls.addClassCleanup(cls.temporary.cleanup)
        path = Path(cls.temporary.name)
        source = HEADER.read_text()
        helpers = ('uniwill_write_kbd_bl_brightness',
                   'uniwill_write_kbd_bl_brightness_white_workaround',
                   'tf_convert_rgb_range', 'uniwill_write_kbd_bl_color',
                   'uniwill_leds_set_brightness', 'uniwill_leds_set_brightness_mc')
        pieces = [re.search(r'(?m)^static DEFINE_MUTEX\(uniwill_led_lock\);', source).group()
                  if 'static DEFINE_MUTEX(uniwill_led_lock);' in source else
                  'static DEFINE_MUTEX(uniwill_led_lock);']
        pieces += [function(source, name) for name in helpers]
        pieces += [initializer(source, name) for name in
                   ('uniwill_led_cdev', 'uw_mcled_cdev_subleds', 'uniwill_mcled_cdev')]
        defines = '\n'.join(re.findall(r'(?m)^#define UNIWILL_KBD_[^\n]+', source))
        interfaces = (ROOT / 'src/uniwill_interfaces.h').read_text()
        defines += '\n' + '\n'.join(re.findall(r'(?m)^#define UW_EC_[^\n]+', interfaces))
        fixture = (ROOT / 'tests/fixtures/uniwill_led_runtime.c').read_text()
        fixture = fixture.replace('/* @DRIVER_CONSTANTS@ */', defines)
        fixture = fixture.replace('/* @DRIVER_SOURCE@ */', '\n\n'.join(pieces))
        core = (ROOT / 'tests/fixtures/linux_led_core_v7_2_9.c').read_text()
        fixture = fixture.replace('/* @ACTUAL_LED_CORE@ */', core)
        lifecycle = [re.search(r'typedef enum \{.*?\} uniwill_kb_backlight_type_t;', source, re.S).group()]
        for name in ('uniwill_kb_backlight_type', 'uniwill_barebone_id',
                     'uniwill_kbl_brightness_ec_controlled', 'uw_leds_initialized'):
            lifecycle.append(re.search(r'(?m)^static [^\n]+\b' + name + r'\s*=.*?;', source).group())
        lifecycle += [initializer(source, name) for name in
                      ('force_no_ec_led_control', 'kbl_type_fixed_color_5_levels')]
        lifecycle += [function(source, name) for name in ('uniwill_leds_init', 'uniwill_leds_remove', 'uniwill_leds_restore_state_extern', 'uniwill_leds_notify_brightness_change_extern')]
        keyboard = Path(os.environ.get('UNIWILL_LEDS_KEYBOARD', ROOT / 'src/uniwill_keyboard.h')).read_text()
        foundation = (ROOT / 'src/uniwill_keyboard.h').read_text()
        wrappers = [re.search(r'(?m)^static DEFINE_MUTEX\(uniwill_ec_transaction_lock\);', foundation).group()]
        wrappers += [function(foundation, name) for name in
                     ('uniwill_read_ec_ram', 'uniwill_write_ec_ram', 'uniwill_update_ec_ram')]
        fixture = fixture.replace('/* @ACTUAL_EC_WRAPPERS@ */', '\n\n'.join(wrappers))
        parent_helper = function(keyboard, 'uniwill_write_kbd_bl_enable')
        fixture = fixture.replace('/* @PARENT_BACKLIGHT_HELPER@ */', parent_helper)
        parent_call = ('return uniwill_write_kbd_bl_enable(enable);'
                       if parent_helper.startswith('static int ') else
                       'uniwill_write_kbd_bl_enable(enable); return 0;')
        fixture = fixture.replace('/* @INVOKE_PARENT_BACKLIGHT@ */', parent_call)
        fixture = fixture.replace('/* @DRIVER_LIFECYCLE@ */', '\n\n'.join(lifecycle))
        pm = (ROOT / 'tests/fixtures/linux_led_pm_v7_2_9.c').read_text()
        fixture = fixture.replace('/* @ACTUAL_LED_PM@ */', pm)
        c_source = path / 'runtime.c'
        c_source.write_text(fixture)
        cls.binaries = []
        # Exercise the existing multicolor max_intensity version branch too.
        for version in ((6, 10, 0), (7, 2, 9)):
            code = (version[0] << 16) | (version[1] << 8) | version[2]
            binary = path / ('runtime-' + '-'.join(map(str, version)))
            result = subprocess.run([os.environ.get('CC', 'cc'), '-std=c11', '-O2',
                                     '-Wall', '-Wextra', '-Werror',
                                     *(['-Wno-error=maybe-uninitialized'] if parent_helper.startswith('static void ') else []),
                                     '-DLINUX_VERSION_CODE=' + str(code),
                                     '-pthread', str(c_source), '-o', str(binary)],
                                    capture_output=True, text=True)
            if result.stdout or result.stderr:
                sys.stderr.write(str(version) + '\n' + result.stdout + result.stderr)
            if result.returncode:
                raise AssertionError(str(version) + '\n' + result.stdout + result.stderr)
            cls.binaries.append((version, binary))

    def check_case(self, name):
        def no_core():
            resource.setrlimit(resource.RLIMIT_CORE, (0, 0))
        for version, binary in self.binaries:
            with self.subTest(version=version):
                result = subprocess.run([str(binary), name], capture_output=True,
                                        text=True, timeout=10, preexec_fn=no_core)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_white_atomic_nosleep_dispatch_defers_all_ec_access_to_work(self):
        self.check_case('white_dispatch')

    def test_rgb_atomic_nosleep_dispatch_defers_all_ec_access_to_work(self):
        self.check_case('rgb_dispatch')

    def test_white_real_software_blink_timer_defers_each_ec_transaction(self):
        self.check_case('white_timer')

    def test_rgb_real_software_blink_timer_defers_each_ec_transaction(self):
        self.check_case('rgb_timer')

    def test_white_success_preserves_status_bits_and_immediate_workaround_order(self):
        self.check_case('white_success')

    def test_white_already_on_and_off_do_not_nudge_immediate_brightness(self):
        self.check_case('white_no_nudge')

    def test_white_each_ec_failure_returns_errno_and_stops_dependent_operations(self):
        self.check_case('white_failures')

    def test_rgb_success_preserves_mode_and_status_bits_and_exact_write_order(self):
        self.check_case('rgb_success')

    def test_each_rgb_channel_keeps_existing_all_256_value_conversion(self):
        self.check_case('rgb_conversion')

    def test_rgb_black_keeps_special_zero_hardware_brightness_and_requested_cache(self):
        self.check_case('rgb_black')

    def test_rgb_each_ec_failure_returns_errno_without_claiming_partial_write_rollback(self):
        self.check_case('rgb_failures')

    def test_rgb_black_each_ec_failure_returns_errno_without_color_write(self):
        self.check_case('black_failures')

    def test_real_workqueue_dispatch_reports_original_transport_errno(self):
        self.check_case('work_error')

    def test_real_sync_dispatch_returns_errno_with_core_request_cache_unchanged(self):
        self.check_case('sync_error')

    def test_white_old_ec_work_does_not_overwrite_newer_core_request_cache(self):
        self.check_case('white_cache_interleave')

    def test_rgb_old_ec_work_does_not_overwrite_newer_core_request_cache(self):
        self.check_case('rgb_cache_interleave')

    def test_core_pm_flag_is_present_in_both_actual_static_initializers(self):
        self.check_case('pm_initial_flags')

    def test_each_actual_init_registration_retains_core_pm_flag_and_hardware_notification(self):
        self.check_case('init_flags')

    def test_real_child_suspend_flushes_queued_rmw_before_parent_disables_backlight(self):
        self.check_case('pm_queued_rmw')

    def test_suspend_requests_only_update_cache_then_child_resume_applies_latest(self):
        self.check_case('pm_latest_request')

    def test_parent_suspend_failure_still_resumes_already_suspended_led_child(self):
        self.check_case('pm_suspend_abort')

    def test_parent_resume_failure_still_allows_child_core_resume_without_claiming_enabled(self):
        self.check_case('pm_resume_error')

    def test_core_pm_off_and_restore_ec_errors_are_logged_not_returned_as_child_pm_errors(self):
        self.check_case('pm_ec_errors')

    def test_successful_init_preserves_all_256_feature_bytes(self):
        self.check_case('init_feature_bytes')

    def test_feature_reads_and_write_failures_stop_before_led_registration(self):
        self.check_case('init_feature_failures')

    def test_barebone_transport_errors_retain_original_errno_after_three_attempts(self):
        self.check_case('init_barebone_errors')

    def test_zero_barebone_id_is_error_after_exactly_three_attempts(self):
        self.check_case('init_zero_barebone')

    def test_legal_remove_then_init_resets_flags_type_notification_and_white_level_limit(self):
        self.check_case('init_rebind')

    def test_init_registration_error_retains_errno_without_publishing_a_child(self):
        self.check_case('init_registration_error')

    def test_successful_init_restore_keeps_existing_white_and_rgb_ec_order(self):
        self.check_case('restore_success')

    def test_late_restore_after_actual_driver_remove_does_not_access_ec(self):
        self.check_case('restore_after_remove')

    def test_successfully_initialized_restore_preserves_existing_ec_error_semantics(self):
        self.check_case('restore_errors')

    def test_deferred_off_then_on_keeps_core_order_and_sleepable_ec_access(self):
        self.check_case('off_on')


if __name__ == '__main__':
    unittest.main(verbosity=2)
