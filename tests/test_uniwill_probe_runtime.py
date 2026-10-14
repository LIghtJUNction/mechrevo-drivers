"""Compile the full real probe, filter and remove with mocked kernel services.

Kernel-version macros exercise both API signature changes; this is not an old
kernel module build or a hardware test. Ownership is set by the real probe.
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
        pieces = [declarations,
                  versioned_function(keyboard, 'uniwill_i8042_filter'),
                  function(keyboard, 'uniwill_keyboard_prepare_remove'),
                  function(keyboard, 'uniwill_keyboard_probe'),
                  versioned_function(keyboard, 'uniwill_keyboard_remove')]
        fixture = (ROOT / 'tests/fixtures/uniwill_probe_runtime.c').read_text()
        source = Path(cls.temp.name) / 'runtime.c'
        source.write_text(fixture.replace('/* @DRIVER_FUNCTIONS@ */', '\n\n'.join(pieces)))
        cls.binaries = []
        for version in ((6, 10, 0), (6, 13, 0), (6, 14, 0), (7, 2, 9)):
            code = (version[0] << 16) | (version[1] << 8) | version[2]
            binary = Path(cls.temp.name) / ('runtime-' + '-'.join(map(str, version)))
            result = subprocess.run(['cc', '-std=c11', '-O2', '-Wall', '-Wextra', '-Werror',
                                     '-DLINUX_VERSION_CODE=' + str(code), str(source), '-o', str(binary)],
                                    capture_output=True, text=True)
            if result.returncode:
                raise AssertionError(str(version) + '\n' + result.stdout + result.stderr)
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


if __name__ == '__main__':
    unittest.main()
