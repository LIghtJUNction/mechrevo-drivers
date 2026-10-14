"""Exercise the production profile function and raw-mode ioctl case in C."""
import os
from pathlib import Path
import re
import resource
import subprocess
import tempfile
import unittest

from test_uniwill_fan_runtime import function

ROOT = Path(__file__).resolve().parents[1]


class UniwillFanIoRouting(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        parent = Path(os.environ.get("UNIWILL_TEST_TMPDIR", tempfile.gettempdir()))
        cls.temp = tempfile.TemporaryDirectory(prefix="fan-io-routing-", dir=parent)
        cls.addClassCleanup(cls.temp.cleanup)
        path = Path(cls.temp.name)
        source = (ROOT / "src/tuxedo_io/tuxedo_io.c").read_text()
        interfaces = (ROOT / "src/uniwill_interfaces.h").read_text()
        enum = re.search(r"enum uw_perf_profiles_v1\s*\{[^}]+\};", interfaces).group()
        mode_case = re.search(r"\t\tcase W_UW_MODE:\n.*?(?=\t\tcase W_UW_MODE_ENABLE:)",
                              source, re.S).group()
        fixture = (ROOT / "tests/fixtures/uniwill_fan_io_runtime.c").read_text()
        fixture = fixture.replace("/* @PROFILE_ENUM@ */", enum)
        fixture = fixture.replace("/* @PROFILE_FUNCTION@ */",
                                  function(source, "uw_set_performance_profile_v1"))
        fixture = fixture.replace("/* @MODE_CASE@ */", mode_case)
        program = path / "runtime.c"
        program.write_text(fixture)
        cls.binary = path / "runtime"
        result = subprocess.run(["cc", "-std=c11", "-O2", "-Wall", "-Wextra", "-Werror",
                                 str(program), "-o", str(cls.binary)],
                                text=True, capture_output=True)
        if result.returncode:
            raise AssertionError(result.stdout + result.stderr)

    def check_case(self, case):
        def no_core():
            resource.setrlimit(resource.RLIMIT_CORE, (0, 0))
        result = subprocess.run([str(self.binary), case], text=True, capture_output=True,
                                timeout=10, preexec_fn=no_core)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_all_profiles_route_to_one_masked_update(self):
        self.check_case("profiles")

    def test_invalid_profiles_never_touch_ec(self):
        self.check_case("invalid")

    def test_profile_preserves_helper_errno(self):
        self.check_case("errors")

    def test_mode_ioctl_routes_each_byte_through_serialized_helper(self):
        self.check_case("mode")


if __name__ == "__main__":
    unittest.main()
