"""Run actual binding functions and fan-start failure path with kernel API mocks.

UNIWILL_BINDING_BASELINE selects old outer binding functions while retaining the
new real fan-start/probe failure path, testing the newly reachable regression.
The probe is truncated only after its initial fan-start block; unrelated LED,
battery and sysfs setup is outside these binding tests.
"""
import os
from pathlib import Path
import re
import resource
import subprocess
import tempfile
import unittest

from test_uniwill_fan_runtime import function

ROOT = Path(__file__).resolve().parents[1]


def extract(source, name):
    # The common initializer returns a pointer, unlike the fan parser's functions.
    source = source.replace('struct platform_device *tuxedo_keyboard_init_driver(',
                            'int tuxedo_keyboard_init_driver(')
    result = function(source, name)
    if name == 'tuxedo_keyboard_init_driver':
        result = result.replace('int tuxedo_keyboard_init_driver(',
                                'struct platform_device *tuxedo_keyboard_init_driver(', 1)
    return result


class UniwillBindingRuntime(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        parent = Path(os.environ.get('UNIWILL_TEST_TMPDIR', tempfile.gettempdir()))
        parent.mkdir(exist_ok=True)
        cls.temp = tempfile.TemporaryDirectory(prefix='binding-runtime-', dir=parent)
        cls.addClassCleanup(cls.temp.cleanup)
        path = Path(cls.temp.name)
        selected = Path(os.environ.get('UNIWILL_BINDING_BASELINE', ROOT))
        common = (selected / 'src/tuxedo_keyboard.c').read_text().replace('__exit ', '')
        keyboard = (selected / 'src/uniwill_keyboard.h').read_text()
        fixed = (ROOT / 'src/uniwill_keyboard.h').read_text()
        wmi = (selected / 'src/uniwill_wmi.c').read_text()
        wmi = re.sub(r'#if LINUX_VERSION_CODE < KERNEL_VERSION\(5, 3, 0\)\n'
                     r'static int uniwill_wmi_probe\([^\n]+\)\n#else\n'
                     r'(static int uniwill_wmi_probe\([^\n]+\))\n#endif', r'\1', wmi)
        probe = function(fixed, 'uniwill_keyboard_probe')
        probe = probe[:probe.index('\n\tif (uw_feats->uniwill_profile_v1)')] + '\n\treturn 0;\n}'
        pieces = [function(fixed, name) for name in
                  ('uw_fan_quiesce_locked', 'uw_fan_unlock', 'uw_fan_control_start')]
        if 'static void uniwill_restore_adapter_state(' in keyboard:
            pieces += [function(keyboard, name) for name in
                       ('uw_fan_lock', 'uniwill_restore_adapter_state')]
        pieces += [probe]
        pieces += [extract(common, name) for name in
                   ('tuxedo_input_init', 'tuxedo_keyboard_init_driver',
                    'tuxedo_input_exit', 'tuxedo_keyboard_remove_driver')]
        pieces += [function(keyboard, name) for name in
                   ('uniwill_add_interface', 'uniwill_remove_interface', 'uniwill_event_callb')]
        pieces += [function(wmi, 'uniwill_wmi_probe')]
        fixture = (ROOT / 'tests/fixtures/uniwill_binding_runtime.c').read_text()
        fixture = fixture.replace('/* @ADAPTER_DECL@ */',
                                  'static void uniwill_restore_adapter_state(void);'
                                  if 'static void uniwill_restore_adapter_state(' in keyboard else '')
        source = path / 'runtime.c'
        source.write_text(fixture.replace('/* @DRIVER_FUNCTIONS@ */', '\n\n'.join(pieces)))
        cls.binary = path / 'runtime'
        result = subprocess.run(['cc', '-std=c11', '-O2', '-Wall', '-Wextra', '-Werror',
                                 str(source), '-o', str(cls.binary)], capture_output=True, text=True)
        if result.returncode:
            raise AssertionError(result.stdout + result.stderr)

    def check_case(self, name):
        def no_core():
            resource.setrlimit(resource.RLIMIT_CORE, (0, 0))
        result = subprocess.run([str(self.binary), name], capture_output=True, text=True,
                                timeout=10, preexec_fn=no_core)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_ec_failure_reaches_wmi_probe_and_rolls_back_binding(self):
        self.check_case('probe_failure')

    def test_unbind_after_failed_probe_and_repeated_remove_are_safe(self):
        self.check_case('failed_remove')

    def test_failed_probe_can_retry_without_stale_fan_or_binding_state(self):
        self.check_case('retry')

    def test_same_driver_initialization_reuses_the_existing_device(self):
        self.check_case('same_driver')

    def test_other_driver_cannot_reuse_or_remove_the_current_device(self):
        self.check_case('other_driver')

    def test_wrong_interface_removal_preserves_the_active_transport(self):
        self.check_case('wrong_interface')

    def test_duplicate_transport_registration_is_rejected(self):
        self.check_case('duplicate')

    def test_input_keymap_failure_clears_freed_global_and_can_unbind(self):
        self.check_case('keymap_failure')

    def test_input_registration_failure_clears_freed_global_and_can_unbind(self):
        self.check_case('input_failure')

    def test_input_allocation_failure_can_unbind_without_freeing_a_null_device(self):
        self.check_case('allocation_failure')

    def test_mode_event_without_an_input_device_does_nothing(self):
        self.check_case('mode_no_input')

    def test_callback_is_published_after_success_and_cleared_before_teardown(self):
        self.check_case('event_lifecycle')

    def test_missing_wmi_guid_does_not_start_binding(self):
        self.check_case('missing_guid')


if __name__ == '__main__':
    unittest.main()
