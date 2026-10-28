"""Exercise actual model detection, TDP bounds and mode selection without hardware."""
import os
from pathlib import Path
import re
import resource
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


def function(source, name):
    match = re.search(r'(?m)^(?:static )?[^;{}\n]*\b' + re.escape(name)
                      + r'\([^;{}]*\)\s*\{', source)
    if not match:
        raise AssertionError('Missing production function: ' + name)
    depth, end = 1, match.end()
    while depth:
        depth += (source[end] == '{') - (source[end] == '}')
        end += 1
    return source[match.start():end]


class MechrevoModels(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        parent = Path(os.environ.get('UNIWILL_TEST_TMPDIR', ROOT / '.local-tests'))
        cls.temp = tempfile.TemporaryDirectory(prefix='mechrevo-models-', dir=parent)
        cls.addClassCleanup(cls.temp.cleanup)
        keyboard = Path(os.environ.get('MECHREVO_MODEL_KEYBOARD',
                                      ROOT / 'src/uniwill_keyboard.h')).read_text()
        ioctl = Path(os.environ.get('MECHREVO_MODEL_IO',
                                   ROOT / 'src/tuxedo_io/tuxedo_io.c')).read_text()
        interfaces = (ROOT / 'src/uniwill_interfaces.h').read_text()
        types = '\n'.join(re.search(r'(?ms)^' + kind + ' ' + name + r' \{.*?^};',
                                    interfaces).group() for kind, name in (
                                        ('struct', 'uniwill_device_features_t'),
                                        ('enum', 'uw_perf_profiles_v1')))
        constants = '\n'.join(re.findall(r'(?m)^#define UW_MODEL_.*$', interfaces))
        tables = ioctl[ioctl.index('static int tdp_min_ph4tux'):ioctl.index('static void uw_id_tdp')]
        pieces = [function(keyboard, 'uniwill_get_device_features')]
        pieces += [function(ioctl, name) for name in (
            'uw_id_tdp', 'uw_get_tdp_min', 'uw_get_tdp_max', 'uw_get_tdp',
            'uw_set_tdp', 'uw_get_performance_profile_count',
            'uw_apply_performance_profile_v1', 'uw_set_performance_profile_v1')]
        fixture = (ROOT / 'tests/fixtures/uniwill_mechrevo_models.c').read_text()
        unit = Path(cls.temp.name) / 'runtime.c'
        unit.write_text(fixture.replace('/* @TYPES@ */', types + '\n' + constants)
                        .replace('/* @TABLES@ */', tables)
                        .replace('/* @PRODUCTION@ */', '\n\n'.join(pieces)))
        cls.binaries = []
        for version in ((4, 17, 0), (4, 18, 0), (7, 2, 11)):
            binary = Path(cls.temp.name) / ('runtime-' + '-'.join(map(str, version)))
            code = (version[0] << 16) | (version[1] << 8) | version[2]
            result = subprocess.run([
                'cc', '-std=c11', '-O2', '-Wall', '-Wextra', '-Werror',
                '-Wno-unused-variable', '-Wno-unused-const-variable',
                '-DLINUX_VERSION_CODE=' + str(code), str(unit), '-o', str(binary)
            ], capture_output=True, text=True, timeout=60)
            if result.returncode:
                raise AssertionError(str(version) + '\n' + result.stdout + result.stderr)
            cls.binaries.append(binary)

    def check_case(self, case):
        def no_core():
            resource.setrlimit(resource.RLIMIT_CORE, (0, 0))
        for binary in self.binaries:
            with self.subTest(version=binary.name):
                result = subprocess.run([str(binary), case], capture_output=True, text=True,
                                        timeout=10, preexec_fn=no_core)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_gx4hrxl_modes_preserve_other_bits_and_do_not_enable_tdp(self):
        self.check_case('gx4hrxl')

    def test_jiaolong_bounds_double_pl4_and_provider_errors(self):
        self.check_case('jiaolong')

    def test_missing_foreign_and_similar_identities_never_inherit_support(self):
        self.check_case('identity')

    def test_existing_tuxedo_profiles_and_local_model_are_preserved(self):
        self.check_case('existing')

    def test_feature_read_failure_retries_without_hardware_writes(self):
        self.check_case('retry')


if __name__ == '__main__':
    unittest.main()
