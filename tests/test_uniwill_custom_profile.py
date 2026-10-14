#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Run actual custom-profile helper, probe and adapter callback with EC doubles.

UNIWILL_KEYBOARD_HEADER selects an older header for regression sensitivity.
The old void helper is adapted only at the test invocation boundary; the driver
functions themselves are extracted unchanged. No physical EC is accessed.
"""
import os
from pathlib import Path
import re
import resource
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
HEADER = Path(os.environ.get('UNIWILL_KEYBOARD_HEADER', ROOT / 'src/uniwill_keyboard.h'))


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


class CustomProfileTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.temporary = tempfile.TemporaryDirectory(prefix='uniwill-custom-profile-')
        cls.addClassCleanup(cls.temporary.cleanup)
        path = Path(cls.temporary.name)
        source = HEADER.read_text()
        helper = function(source, 'uniwill_set_custom_profile_mode')
        invocation = ('return uniwill_set_custom_profile_mode(zero);'
                      if helper.startswith('static int ') else
                      'uniwill_set_custom_profile_mode(zero); return 0;')
        pieces = [helper, function(source, 'uniwill_event_callb')]
        pieces += [function(source, name) for name in
                   ('uw_fan_quiesce_locked', 'uw_fan_unlock', 'uw_fan_control_start')]
        if 'static int uniwill_keyboard_abort_probe(' in source:
            pieces.append(function(source, 'uniwill_keyboard_abort_probe'))
        pieces.append(function(source, 'uniwill_keyboard_probe'))
        has_adapter_gate = 'static void uniwill_restore_adapter_state(' in source
        if has_adapter_gate:
            pieces.append(function(source, 'uw_fan_lock'))
            pieces.append(function(source, 'uniwill_restore_adapter_state'))
        fixture = (ROOT / 'tests/fixtures/uniwill_custom_profile.c').read_text()
        fixture = fixture.replace('/* @ADAPTER_DECL@ */',
                                  'static void uniwill_restore_adapter_state(void);'
                                  if has_adapter_gate else '')
        fixture = fixture.replace('/* @HAS_ADAPTER_GATE@ */', str(int(has_adapter_gate)))
        fixture = fixture.replace('/* @ACTUAL_FUNCTIONS@ */', '\n\n'.join(pieces))
        fixture = fixture.replace('/* @INVOKE_HELPER@ */', invocation)
        c_source = path / 'runtime.c'
        c_source.write_text(fixture)
        cls.binary = path / 'runtime'
        result = subprocess.run([os.environ.get('CC', 'cc'), '-std=c11', '-O0',
                                 '-Wall', '-Wextra', '-Werror', str(c_source),
                                 '-o', str(cls.binary)], capture_output=True, text=True)
        if result.returncode:
            raise AssertionError(result.stdout + result.stderr)

    def check_case(self, name):
        def no_core():
            resource.setrlimit(resource.RLIMIT_CORE, (0, 0))
        result = subprocess.run([str(self.binary), name], capture_output=True,
                                text=True, timeout=10, preexec_fn=no_core)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_unsupported_boot_helper_does_not_access_ec_or_sleep(self):
        self.check_case('unsupported_boot')

    def test_unsupported_adapter_helper_does_not_access_ec_or_sleep(self):
        self.check_case('unsupported_adapter')

    def test_boot_preserves_all_other_bits_and_exact_reset_delay_set_sequence(self):
        self.check_case('boot_bytes')

    def test_adapter_preserves_all_other_bits_without_reset_or_delay(self):
        self.check_case('adapter_bytes')

    def test_boot_read_failure_never_writes_uninitialized_data(self):
        self.check_case('boot_read_failure')

    def test_adapter_read_failure_never_writes_uninitialized_data(self):
        self.check_case('adapter_read_failure')

    def test_boot_reset_failure_stops_before_delay_and_set(self):
        self.check_case('boot_reset_failure')

    def test_boot_set_failure_returns_original_errno_without_extra_write(self):
        self.check_case('boot_set_failure')

    def test_adapter_set_failure_returns_original_errno(self):
        self.check_case('adapter_set_failure')

    def test_unsupported_probe_keeps_other_initialization_without_custom_writes(self):
        self.check_case('probe_unsupported')

    def test_probe_success_keeps_existing_ec_order_and_initialization(self):
        self.check_case('probe_success')

    def test_probe_each_custom_failure_is_logged_and_independent_initialization_continues(self):
        self.check_case('probe_failures')

    def test_probe_profile_v1_curve_copy_still_precedes_custom_profile(self):
        self.check_case('probe_curve')

    def test_unsupported_adapter_still_restores_backlight_and_charging(self):
        self.check_case('event_unsupported')

    def test_adapter_success_restores_custom_profile_then_backlight_and_charging(self):
        self.check_case('event_success')

    def test_adapter_each_custom_failure_is_logged_and_independent_restoration_continues(self):
        self.check_case('event_failures')


if __name__ == '__main__':
    unittest.main(verbosity=2)
