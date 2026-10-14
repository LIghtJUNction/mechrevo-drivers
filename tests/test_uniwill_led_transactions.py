#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Actual LED/parent functions and actual EC transaction API, with pthreads.

Only EC transport, mutex/workqueue services and hardware are doubles. Frozen
official LED core/PM functions are reused unmodified. UNIWILL_LEDS_HEADER and
UNIWILL_LED_KEYBOARD_HEADER can select old production inputs for sensitivity;
the transaction API comes from UNIWILL_EC_KEYBOARD_HEADER (normally this tree).
No fixture silently substitutes the new transaction implementation for it.
"""
import os
from pathlib import Path
import re
import resource
import subprocess
import sys
import tempfile
import unittest

from test_uniwill_led_runtime import initializer

ROOT = Path(__file__).resolve().parents[1]
HEADER = Path(os.environ.get('UNIWILL_LEDS_HEADER', ROOT / 'src/uniwill_leds.h'))
KEYBOARD = Path(os.environ.get('UNIWILL_LED_KEYBOARD_HEADER', ROOT / 'src/uniwill_keyboard.h'))
FOUNDATION = Path(os.environ.get('UNIWILL_EC_KEYBOARD_HEADER', ROOT / 'src/uniwill_keyboard.h'))
USB_KEYBOARD = Path(os.environ.get('UNIWILL_USB_KEYBOARD_HEADER', KEYBOARD))


def function(source, name):
    match = re.search(r'(?m)^(?:static\s+)?(?:int|void|bool|uniwill_kb_backlight_type_t)\s+'
                      + re.escape(name) + r'\([^;{]*\)\s*\{', source)
    if not match:
        raise AssertionError('Missing actual function: ' + name)
    depth, end = 1, match.end()
    while depth and end < len(source):
        depth += (source[end] == '{') - (source[end] == '}')
        end += 1
    if depth:
        raise AssertionError('Unbalanced actual function: ' + name)
    return source[match.start():end]


class UniwillLedTransactions(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        (ROOT / '.local-tests').mkdir(exist_ok=True)
        cls.temporary = tempfile.TemporaryDirectory(prefix='uniwill-led-transactions-',
                                                    dir=ROOT / '.local-tests')
        cls.addClassCleanup(cls.temporary.cleanup)
        path = Path(cls.temporary.name)
        source, keyboard, foundation = HEADER.read_text(), KEYBOARD.read_text(), FOUNDATION.read_text()
        usb = USB_KEYBOARD.read_text()
        helpers = ('uniwill_write_kbd_bl_brightness',
                   'uniwill_write_kbd_bl_brightness_white_workaround',
                   'tf_convert_rgb_range', 'uniwill_write_kbd_bl_color',
                   'uniwill_leds_set_brightness', 'uniwill_leds_set_brightness_mc')
        pieces = [function(source, name) for name in helpers]
        pieces += [initializer(source, name) for name in
                   ('uniwill_led_cdev', 'uw_mcled_cdev_subleds', 'uniwill_mcled_cdev')]
        defines = '\n'.join(re.findall(r'(?m)^#define UNIWILL_KBD_[^\n]+', source))
        interfaces = (ROOT / 'src/uniwill_interfaces.h').read_text()
        defines += '\n' + '\n'.join(re.findall(r'(?m)^#define UW_EC_[^\n]+', interfaces))
        lifecycle = [re.search(r'typedef enum \{.*?\} uniwill_kb_backlight_type_t;', source, re.S).group()]
        for name in ('uniwill_kb_backlight_type', 'uniwill_barebone_id',
                     'uniwill_kbl_brightness_ec_controlled', 'uw_leds_initialized'):
            lifecycle.append(re.search(r'(?m)^static [^\n]+\b' + name + r'\s*=.*?;', source).group())
        # Old inputs intentionally have no LED mutex; add no fake replacement.
        led_mutex = re.search(r'(?m)^static DEFINE_MUTEX\(uniwill_led_lock\);', source)
        declarations = lifecycle + ([led_mutex.group()] if led_mutex else [])
        lifecycle = [initializer(source, name) for name in
                     ('force_no_ec_led_control', 'kbl_type_fixed_color_5_levels')]
        lifecycle += [function(source, name) for name in
                      ('uniwill_leds_init', 'uniwill_leds_remove',
                       'uniwill_leds_restore_state_extern',
                       'uniwill_leds_notify_brightness_change_extern',
                       'uniwill_leds_get_backlight_type_extern')]
        api = [re.search(r'(?m)^static DEFINE_MUTEX\(uniwill_ec_transaction_lock\);', foundation).group()]
        api += [function(foundation, name) for name in
                ('uniwill_read_ec_ram', 'uniwill_write_ec_ram', 'uniwill_update_ec_ram')]
        parent = function(keyboard, 'uniwill_write_kbd_bl_enable')
        usb_setter = re.search(r'(?m)^static bool uw_usb_powershare_last_written_value;', usb).group()
        usb_setter += '\n' + function(usb, 'uw_set_usb_powershare')
        fixture = (ROOT / 'tests/fixtures/uniwill_led_transactions.c').read_text()
        replacements = {
            '/* @DRIVER_CONSTANTS@ */': defines + '\n#define HAS_LED_LOCK ' + str(int(bool(led_mutex))),
            '/* @ACTUAL_EC_API@ */': '\n\n'.join(api),
            '/* @DRIVER_DECLARATIONS@ */': '\n\n'.join(declarations),
            '/* @DRIVER_SOURCE@ */': '\n\n'.join(pieces),
            '/* @PARENT_BACKLIGHT_HELPER@ */': parent,
            '/* @ACTUAL_USB_SETTER@ */': usb_setter,
            '/* @ACTUAL_LED_CORE@ */': (ROOT / 'tests/fixtures/linux_led_core_v7_2_9.c').read_text(),
            '/* @ACTUAL_LED_PM@ */': (ROOT / 'tests/fixtures/linux_led_pm_v7_2_9.c').read_text(),
            '/* @DRIVER_LIFECYCLE@ */': '\n\n'.join(lifecycle),
            '/* @INVOKE_PARENT_BACKLIGHT@ */': 'return uniwill_write_kbd_bl_enable(enable);',
        }
        for marker, content in replacements.items():
            if fixture.count(marker) != 1:
                raise AssertionError('Fixture marker count: ' + marker)
            fixture = fixture.replace(marker, content)
        c_source = path / 'transactions.c'
        c_source.write_text(fixture)
        cls.binaries = []
        for version in ((6, 10, 0), (7, 2, 9)):
            code = (version[0] << 16) | (version[1] << 8) | version[2]
            binary = path / ('transactions-' + '-'.join(map(str, version)))
            result = subprocess.run([os.environ.get('CC', 'cc'), '-std=c11', '-O2',
                                     '-Wall', '-Wextra', '-Werror',
                                     '-pthread',
                                     '-DLINUX_VERSION_CODE=' + str(code),
                                     str(c_source), '-o', str(binary)], capture_output=True, text=True)
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
                                        text=True, timeout=20, preexec_fn=no_core)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_notification_read_errors_preserve_cache_and_emit_no_false_hardware_event(self):
        self.check_case('notify_read_errors')

    def test_notification_rgb_compensation_errors_preserve_cache_and_stop_dependencies(self):
        self.check_case('notify_compensation_errors')

    def test_notification_success_retains_white_levels_and_rgb_compensation_behavior(self):
        self.check_case('notify_success')

    def test_uninitialized_uncontrolled_and_absent_notifications_do_not_access_ec(self):
        self.check_case('notify_gate')

    def test_actual_child_suspend_late_notify_resume_retains_desired_brightness(self):
        self.check_case('notify_suspended')

    def test_suspend_flag_arriving_during_notification_read_prevents_cache_overwrite(self):
        self.check_case('notify_suspend_interleave')

    def test_status_rmw_brightness_then_parent_enable_preserves_both_all_256_bytes(self):
        self.check_case('status_brightness_first')

    def test_status_rmw_parent_enable_then_brightness_preserves_both_all_256_bytes(self):
        self.check_case('status_parent_first')

    def test_two_rgb_callbacks_complete_whole_register_sequences_without_interleaving(self):
        self.check_case('rgb_transaction')

    def test_restore_then_rgb_callback_complete_whole_sequences_without_interleaving(self):
        self.check_case('restore_transaction')

    def test_rgb_callback_then_restore_complete_whole_sequences_without_interleaving(self):
        self.check_case('callback_restore_transaction')

    def test_each_white_failure_returns_original_errno_and_releases_transaction_lock(self):
        self.check_case('white_failure_recovery')

    def test_each_rgb_failure_returns_original_errno_and_releases_transaction_lock(self):
        self.check_case('rgb_failure_recovery')

    def test_black_restore_preserves_all_request_levels_and_only_writes_hardware_zero(self):
        self.check_case('restore_black')

    def test_black_restore_each_ec_failure_stops_and_releases_transaction_lock(self):
        self.check_case('restore_black_errors')

    def test_nonblack_restore_preserves_independent_brightness_attempt_after_color_failure(self):
        self.check_case('restore_color_errors')

    def test_remove_closes_exports_then_unregister_flush_waits_for_off_callback_without_deadlock(self):
        self.check_case('remove_flush')

    def test_removed_restore_notify_and_backlight_type_queries_are_safe_and_no_ec(self):
        self.check_case('removed_exports')

    def test_actual_usb_then_rgb_callback_preserves_both_0767_bits_all_bytes(self):
        self.check_case('usb_rgb_usb_first')

    def test_actual_rgb_callback_then_usb_preserves_both_0767_bits_all_bytes(self):
        self.check_case('usb_rgb_rgb_first')


if __name__ == '__main__':
    unittest.main(verbosity=2)
