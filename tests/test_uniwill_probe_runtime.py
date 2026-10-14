"""Compile the full real probe, filter and remove with mocked kernel services.

Kernel-version macros exercise both API signature changes; this is not an old
kernel module build or a hardware test. Ownership is set by the real probe.
"""
import os
from pathlib import Path
import re
import resource
import subprocess
import sys
import tempfile
import unittest

from test_uniwill_fan_runtime import function

ROOT = Path(__file__).resolve().parents[1]


def versioned_function(source, name):
    """Keep the driver's original conditional signature for C preprocessing."""
    match = re.search(r'(?m)^#if LINUX_VERSION_CODE < KERNEL_VERSION\([^\n]+\)\n'
                      r'static (?:bool|int) ' + re.escape(name) + r'\(', source)
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


class UniwillProbeRuntime(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        parent = Path(os.environ.get('UNIWILL_TEST_TMPDIR', ROOT / '.local-tests'))
        parent.mkdir(parents=True, exist_ok=True)
        cls.temp = tempfile.TemporaryDirectory(prefix='probe-runtime-', dir=parent)
        cls.addClassCleanup(cls.temp.cleanup)
        selected = Path(os.environ.get('UNIWILL_PROBE_BASELINE', ROOT))
        keyboard = (selected / 'src/uniwill_keyboard.h').read_text()
        declarations = re.search(r'static u8 uniwill_touchp_toggle_seq\[\] = \{[^}]+\};', keyboard).group()
        # Old source keeps sequence state inside the real filter itself.
        state = re.search(r'(?m)^static u8 uniwill_touchp_toggle_seq_pos;', keyboard)
        if state:
            declarations += '\n' + state.group()
        declarations += '\n' + re.search(r'(?m)^static bool uniwill_i8042_filter_installed;', keyboard).group()
        backlight = function(keyboard, 'uniwill_write_kbd_bl_enable')
        invocation = ('return uniwill_write_kbd_bl_enable(enable);'
                      if backlight.startswith('static int ') else
                      'uniwill_write_kbd_bl_enable(enable); return 0;')
        pieces = [declarations, backlight,
                  function(keyboard, 'uw_fan_quiesce_locked'),
                  function(keyboard, 'uw_fan_lock'),
                  function(keyboard, 'uw_fan_unlock'),
                  function(keyboard, 'uw_fan_control_start')]
        if 'static int uniwill_keyboard_abort_probe(' in keyboard:
            pieces.append(function(keyboard, 'uniwill_keyboard_abort_probe'))
        pieces += [
                  versioned_function(keyboard, 'uniwill_i8042_filter'),
                  function(keyboard, 'uniwill_keyboard_prepare_remove'),
                  function(keyboard, 'uniwill_keyboard_probe'),
                  versioned_function(keyboard, 'uniwill_keyboard_remove')]
        fixture = (ROOT / 'tests/fixtures/uniwill_probe_runtime.c').read_text()
        source = Path(cls.temp.name) / 'runtime.c'
        fixture = fixture.replace('/* @DRIVER_FUNCTIONS@ */', '\n\n'.join(pieces))
        fixture = fixture.replace('/* @INVOKE_BACKLIGHT@ */', invocation)
        source.write_text(fixture)
        cls.binaries = []
        for version in ((6, 10, 0), (6, 13, 0), (6, 14, 0), (7, 2, 9)):
            code = (version[0] << 16) | (version[1] << 8) | version[2]
            binary = Path(cls.temp.name) / ('runtime-' + '-'.join(map(str, version)))
            flags = ['cc', '-std=c11', '-O2', '-Wall', '-Wextra', '-Werror']
            if selected.resolve() != ROOT.resolve():
                # Known old read-failure bugs trigger this compiler warning.
                # Keep it visible and permit runtime regression assertions;
                # current production source still builds with strict -Werror.
                flags.append('-Wno-error=maybe-uninitialized')
            result = subprocess.run(flags + [
                                     '-DLINUX_VERSION_CODE=' + str(code), str(source), '-o', str(binary)],
                                    capture_output=True, text=True)
            if result.returncode:
                raise AssertionError(str(version) + '\n' + result.stdout + result.stderr)
            if result.stdout or result.stderr:
                sys.stderr.write(str(version) + '\n' + result.stdout + result.stderr)
            cls.binaries.append((version, binary))

    def check_case(self, case):
        def no_core():
            resource.setrlimit(resource.RLIMIT_CORE, (0, 0))
        for version, binary in self.binaries:
            with self.subTest(version=version):
                result = subprocess.run([str(binary), case], capture_output=True, text=True,
                                        timeout=10, preexec_fn=no_core)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_successful_real_probe_owns_and_removes_filter_exactly_once(self):
        self.check_case('success')

    def test_failed_install_preserves_foreign_filter_and_drains_own_work(self):
        self.check_case('foreign')

    def test_optional_install_errno_does_not_fail_probe_or_claim_ownership(self):
        self.check_case('install_errors')

    def test_early_fan_start_errno_stops_probe_before_filter_publication(self):
        self.check_case('early_errors')

    def test_success_failure_success_reprobe_updates_actual_filter_ownership(self):
        self.check_case('reprobe')

    def test_normal_filter_sequence_and_auxiliary_bytes_keep_existing_behavior(self):
        self.check_case('sequence')

    def test_every_partial_prefix_is_reset_before_next_probe_publishes_filter(self):
        self.check_case('prefixes')

    def test_previous_lifecycle_tail_cannot_queue_work_in_new_lifecycle(self):
        self.check_case('stale_tail')

    def test_backlight_rmw_preserves_other_bits_and_enable_argument_mask(self):
        self.check_case('backlight_bytes')

    def test_backlight_read_failure_returns_original_errno_and_never_writes(self):
        self.check_case('backlight_read_errors')

    def test_backlight_write_failure_returns_original_errno(self):
        self.check_case('backlight_write_errors')

    def test_profile_curve_success_preserves_exact_ec_sequence(self):
        self.check_case('curve_success')

    def test_each_curve_read_and_write_failure_stops_dependent_initialization(self):
        self.check_case('curve_errors')

    def test_manual_enable_failure_closes_exports_without_guessing_rollback(self):
        self.check_case('manual_enable_errors')

    def test_second_fan_reset_failure_disables_confirmed_manual_mode(self):
        self.check_case('fan_detection_errors')

    def test_failed_probe_cleanup_keeps_original_errno_and_reports_cleanup_error(self):
        self.check_case('cleanup_errors')

    def test_critical_probe_failures_can_retry_with_no_live_exports_or_resources(self):
        self.check_case('critical_reprobe')

    def test_optional_initial_backlight_read_failure_keeps_unknown_sentinel(self):
        self.check_case('initial_backlight_errors')

    def test_optional_backlight_enable_failures_are_logged_and_probe_continues(self):
        self.check_case('enable_backlight_errors')

    def test_backlight_enable_is_restored_for_all_initial_and_current_bytes(self):
        self.check_case('backlight_restore_bytes')

    def test_remove_backlight_failures_do_not_stop_resource_teardown(self):
        self.check_case('restore_errors')

    def test_remove_manual_disable_errno_is_logged_without_skipping_unlock(self):
        self.check_case('remove_manual_errors')

    def test_new_probe_clears_saved_backlight_state_even_on_early_failure(self):
        self.check_case('backlight_reprobe')


if __name__ == '__main__':
    unittest.main()
