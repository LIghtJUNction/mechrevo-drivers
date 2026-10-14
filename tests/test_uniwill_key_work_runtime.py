"""Actual teardown/filter/worker functions under deterministic pthread schedules."""
import os
from pathlib import Path
import re
import resource
import subprocess
import tempfile
import unittest

from test_uniwill_fan_runtime import function

ROOT = Path(__file__).resolve().parents[1]


class UniwillKeyWorkRuntime(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        parent = Path(os.environ.get('UNIWILL_TEST_TMPDIR', tempfile.gettempdir()))
        parent.mkdir(exist_ok=True)
        cls.temp = tempfile.TemporaryDirectory(prefix='keywork-runtime-', dir=parent)
        cls.addClassCleanup(cls.temp.cleanup)
        path = Path(cls.temp.name)
        selected = Path(os.environ.get('UNIWILL_KEY_WORK_BASELINE', ROOT))
        keyboard = (selected / 'src/uniwill_keyboard.h').read_text()
        common = (selected / 'src/tuxedo_keyboard.c').read_text().replace('__exit ', '')
        keyboard = re.sub(r'#if LINUX_VERSION_CODE < KERNEL_VERSION\(6, 11, 0\)\n'
                          r'static int uniwill_keyboard_remove\([^\n]+\)\n#else\n'
                          r'(static void uniwill_keyboard_remove\([^\n]+\))\n#endif', r'\1', keyboard)
        keyboard = re.sub(r'#if LINUX_VERSION_CODE < KERNEL_VERSION\(6, 14, 0\)\n'
                          r'static bool uniwill_i8042_filter\([^#]+#else\n'
                          r'(static bool uniwill_i8042_filter\([^#]+)\n#endif', r'\1', keyboard)
        array = re.search(r'static u8 uniwill_touchp_toggle_seq\[\] = \{[^}]+\};', keyboard).group()
        pieces = [array]
        sequence_state = re.search(r'(?m)^static u8 uniwill_touchp_toggle_seq_pos;', keyboard)
        if sequence_state:
            pieces.append(sequence_state.group())
        cls.has_prepare_remove = 'static void uniwill_keyboard_prepare_remove(' in keyboard
        if cls.has_prepare_remove:
            pieces.append(function(keyboard, 'uniwill_keyboard_prepare_remove'))
        else:
            # Old source has no hook; its common remove never calls this placeholder.
            pieces.append('static __attribute__((unused)) void uniwill_keyboard_prepare_remove(void) {}')
        for name in ('key_event_work', 'uniwill_i8042_filter', 'uniwill_keyboard_remove'):
            # The existing parser supports int/u32/void; bool filter uses the same body grammar.
            source = keyboard.replace('static bool uniwill_i8042_filter(',
                                      'static int uniwill_i8042_filter(')
            body = function(source, name)
            if name == 'uniwill_i8042_filter':
                body = body.replace('static int uniwill_i8042_filter(',
                                    'static bool uniwill_i8042_filter(', 1)
            pieces.append(body)
        pieces += [function(common, name) for name in
                   ('tuxedo_input_exit', 'tuxedo_keyboard_remove_driver')]
        fixture = (ROOT / 'tests/fixtures/uniwill_key_work_runtime.c').read_text()
        initializer = re.search(r'struct tuxedo_keyboard_driver uniwill_keyboard_driver\s*=\s*\{([^}]+)\};', keyboard).group(1)
        hook = re.search(r'\.prepare_remove\s*=\s*([^,\n]+)', initializer)
        fixture = fixture.replace('/* @PREPARE_REMOVE_HOOK@ */', hook.group(1).strip() if hook else 'NULL')
        source = path / 'runtime.c'
        source.write_text(fixture.replace('/* @DRIVER_FUNCTIONS@ */', '\n\n'.join(pieces)))
        cls.binary = path / 'runtime'
        result = subprocess.run(['cc', '-std=c11', '-O2', '-Wall', '-Wextra', '-Werror',
                                 '-pthread', str(source), '-o', str(cls.binary)], capture_output=True, text=True)
        if result.returncode:
            raise AssertionError(result.stdout + result.stderr)

    def check_case(self, case):
        def no_core():
            resource.setrlimit(resource.RLIMIT_CORE, (0, 0))
        result = subprocess.run([str(self.binary), case], capture_output=True, text=True,
                                timeout=10, preexec_fn=no_core)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_queued_key_work_is_cancelled_before_input_free(self):
        self.check_case('queued')

    def test_running_key_work_drains_while_input_is_still_alive(self):
        self.check_case('running')

    def test_events_after_filter_removal_cannot_enqueue_new_work(self):
        self.check_case('late_event')

    def test_failed_input_initialization_can_drain_queued_work(self):
        self.check_case('no_input_queued')

    def test_failed_input_initialization_makes_running_key_work_safe(self):
        self.check_case('no_input_running')

    def test_absent_filter_does_not_skip_work_cancellation(self):
        self.check_case('filter_absent')

    def test_repeated_driver_removal_does_not_double_free_or_redrain(self):
        self.check_case('repeated')

    def test_direct_platform_remove_cancels_queued_work_without_freeing_input(self):
        self.check_case('direct_queued')

    def test_direct_platform_remove_drains_running_work_without_freeing_input(self):
        self.check_case('direct_running')

    def test_parallel_prepare_remove_serializes_filter_ownership_and_cancellation(self):
        if not self.has_prepare_remove:
            self.skipTest('Old source has no prepare_remove helper')
        self.check_case('parallel_prepare')

    def test_removing_a_different_driver_preserves_the_active_key_worker(self):
        self.check_case('wrong_driver')


if __name__ == '__main__':
    unittest.main()
