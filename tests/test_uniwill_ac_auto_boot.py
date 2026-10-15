"""Run actual AC auto-boot helpers/sysfs with the shared EC provider and pthreads.

UNIWILL_AC_AUTO_BOOT_HEADER selects historical source for a red regression run.
No physical EC is accessed; scheduling doubles are shared with provider tests.
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


class UniwillAcAutoBoot(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        parent = Path(os.environ.get('UNIWILL_TEST_TMPDIR', ROOT / '.local-tests'))
        parent.mkdir(parents=True, exist_ok=True)
        cls.temp = tempfile.TemporaryDirectory(prefix='ac-auto-boot-', dir=parent)
        cls.addClassCleanup(cls.temp.cleanup)
        header = Path(os.environ.get('UNIWILL_AC_AUTO_BOOT_HEADER', ROOT / 'src/uniwill_keyboard.h'))
        source = header.read_text()
        interfaces = (ROOT / 'src/uniwill_interfaces.h').read_text()
        types = re.findall(r'(?m)^typedef (?:int|void) \(uniwill_[^\n]+;', interfaces)
        types.append(re.search(r'(?ms)^struct uniwill_interface_t \{.*?^};', interfaces).group())
        constants = [re.search(r'(?m)^#define UW_EC_REG_AC_AUTO_BOOT_STATUS\s+.+$', interfaces).group()]
        pieces = [re.search(r'(?m)^static DEFINE_MUTEX\(uniwill_ec_transaction_lock\);', source).group(),
                  re.search(r'(?m)^static bool uw_ac_auto_boot_last_written_value;', source).group()]
        for name in ('uniwill_read_ec_ram_unlocked', 'uniwill_read_ec_ram',
                     'uniwill_write_ec_ram_unlocked', 'uniwill_write_ec_ram',
                     'uniwill_update_ec_ram', 'uw_set_ac_auto_boot', 'uw_get_ac_auto_boot',
                     'uw_ac_auto_boot_show', 'uw_ac_auto_boot_store'):
            # Extend the shared brace parser's return-type recognition only.
            adapted = source.replace('static ssize_t ' + name, 'static int ' + name)
            body = function(adapted, name)
            if name in ('uw_ac_auto_boot_show', 'uw_ac_auto_boot_store'):
                body = body.replace('static int ' + name, 'static ssize_t ' + name, 1)
            pieces.append(body)
        shared = (ROOT / 'tests/fixtures/uniwill_ec_transaction.c').read_text().split('enum operation {', 1)[0]
        shared = shared.replace('/* @INTERFACE_TYPES@ */', '\n'.join(types + constants))
        fixture = (ROOT / 'tests/fixtures/uniwill_ac_auto_boot.c').read_text()
        c_source = Path(cls.temp.name) / 'runtime.c'
        c_source.write_text(shared.replace('/* @DRIVER_FUNCTIONS@ */', fixture.split('/* @HARNESS@ */')[0]
                                         + '\n\n'.join(pieces)) + fixture.split('/* @HARNESS@ */')[1])
        cls.binary = Path(cls.temp.name) / 'runtime'
        result = subprocess.run(['cc', '-std=c11', '-O2', '-Wall', '-Wextra', '-Werror',
                                 '-Wno-unused-parameter', '-Wno-type-limits', '-pthread',
                                 str(c_source), '-o', str(cls.binary)], capture_output=True, text=True)
        if result.returncode:
            raise AssertionError(result.stdout + result.stderr)

    def check_case(self, case):
        def no_core():
            resource.setrlimit(resource.RLIMIT_CORE, (0, 0))
        result = subprocess.run([str(self.binary), case], capture_output=True, text=True,
                                timeout=10, preexec_fn=no_core)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_all_register_bytes_and_all_u8_low_bit_inputs_update_success_cache(self):
        self.check_case('set_bytes')

    def test_getter_decodes_all_bytes_and_preserves_cache(self):
        self.check_case('get_bytes')

    def test_read_errors_skip_write_and_preserve_errno_and_cache(self):
        self.check_case('read_errors')

    def test_write_errors_preserve_errno_and_cache_without_retry_or_rollback(self):
        self.check_case('write_errors')

    def test_partial_write_error_preserves_hardware_side_effect_and_old_cache(self):
        self.check_case('partial_write')

    def test_getter_error_does_not_change_output_or_cache(self):
        self.check_case('get_errors')

    def test_missing_interface_preserves_cache_without_transport(self):
        self.check_case('missing')

    def test_sysfs_validation_show_and_existing_eio_translation(self):
        self.check_case('sysfs')

    def test_ac_update_then_other_bit_update_has_no_lost_bits(self):
        self.check_case('ac_update')

    def test_other_bit_update_then_ac_update_has_no_lost_bits(self):
        self.check_case('update_ac')

    def test_ac_update_then_plain_write_has_no_stale_overwrite(self):
        self.check_case('ac_write')

    def test_plain_write_then_ac_update_uses_completed_byte(self):
        self.check_case('write_ac')


if __name__ == '__main__':
    unittest.main()
