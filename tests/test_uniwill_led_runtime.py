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
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
HEADER = Path(os.environ.get('UNIWILL_LEDS_HEADER', ROOT / 'src/uniwill_leds.h'))


def function(source, name):
    match = re.search(r'(?m)^(?:static\s+)?(?:int|void)\s+' + re.escape(name)
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
    match = re.search(r'(?m)^static struct [^\n]+\b' + re.escape(name)
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
        pieces = [function(source, name) for name in helpers]
        pieces += [initializer(source, name) for name in
                   ('uniwill_led_cdev', 'uw_mcled_cdev_subleds', 'uniwill_mcled_cdev')]
        defines = '\n'.join(re.findall(r'(?m)^#define UNIWILL_KBD_[^\n]+', source))
        interfaces = (ROOT / 'src/uniwill_interfaces.h').read_text()
        defines += '\n' + '\n'.join(re.findall(r'(?m)^#define UW_EC_REG_KBD_BL_[^\n]+', interfaces))
        fixture = (ROOT / 'tests/fixtures/uniwill_led_runtime.c').read_text()
        fixture = fixture.replace('/* @DRIVER_CONSTANTS@ */', defines)
        fixture = fixture.replace('/* @DRIVER_SOURCE@ */', '\n\n'.join(pieces))
        core = (ROOT / 'tests/fixtures/linux_led_core_v7_2_9.c').read_text()
        fixture = fixture.replace('/* @ACTUAL_LED_CORE@ */', core)
        c_source = path / 'runtime.c'
        c_source.write_text(fixture)
        cls.binaries = []
        # Exercise the existing multicolor max_intensity version branch too.
        for version in ((6, 10, 0), (7, 2, 9)):
            code = (version[0] << 16) | (version[1] << 8) | version[2]
            binary = path / ('runtime-' + '-'.join(map(str, version)))
            result = subprocess.run([os.environ.get('CC', 'cc'), '-std=c11', '-O2',
                                     '-Wall', '-Wextra', '-Werror',
                                     '-DLINUX_VERSION_CODE=' + str(code),
                                     str(c_source), '-o', str(binary)],
                                    capture_output=True, text=True)
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

    def test_deferred_off_then_on_keeps_core_order_and_sleepable_ec_access(self):
        self.check_case('off_on')


if __name__ == '__main__':
    unittest.main(verbosity=2)
