"""Compile real Uniwill driver functions with safe userspace mocks (requires cc)."""
from pathlib import Path
import re
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


def function(source, name):
    match = re.search(r"(?m)^static[^;{]*\b" + re.escape(name) + r"\([^;{]*\)\s*\{", source)
    if not match:
        raise AssertionError("Missing driver function: " + name)
    depth = 1
    position = match.end()
    while depth and position < len(source):
        if source[position] == "{":
            depth += 1
        elif source[position] == "}":
            depth -= 1
        position += 1
    if depth:
        raise AssertionError("Unbalanced function: " + name)
    return source[match.start():position]


class UniwillRuntime(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        parent = ROOT / ".local-tests"
        parent.mkdir(exist_ok=True)
        cls.temp = tempfile.TemporaryDirectory(dir=parent)
        cls.addClassCleanup(cls.temp.cleanup)
        path = Path(cls.temp.name)
        keyboard = (ROOT / "src/uniwill_keyboard.h").read_text()
        io = (ROOT / "src/tuxedo_io/tuxedo_io.c").read_text()
        functions = [function(keyboard, "uw_mini_led_local_dimming_init")]
        functions.extend(function(io, name) for name in
                         ("uw_get_tdp_min", "uw_get_tdp_max", "uw_get_tdp", "uw_copy_tdp_value"))
        cases = re.search(r"(?ms)^\s*case R_UW_TDP0:.*?(?=^\s*case R_UW_PROFS_AVAILABLE:)", io)
        if not cases:
            raise AssertionError("Missing TDP ioctl dispatch")
        fixture = (ROOT / "tests/fixtures/uniwill_runtime.c").read_text()
        fixture = fixture.replace("/* @DRIVER_FUNCTIONS@ */", "\n\n".join(functions))
        fixture = fixture.replace("/* @DRIVER_TDP_CASES@ */", cases.group())
        fixture = fixture.replace("/* @DRIVER_IOCTL_ENTRY@ */", function(io, "fop_ioctl"))
        source = path / "runtime.c"
        source.write_text(fixture)
        cls.binary = path / "runtime"
        result = subprocess.run(
            ["cc", "-std=c11", "-Wall", "-Wextra", "-Werror", str(source), "-o", str(cls.binary)],
            text=True, capture_output=True,
        )
        if result.returncode:
            raise AssertionError(result.stdout + result.stderr)

    def check_case(self, case):
        result = subprocess.run([str(self.binary), case], text=True, capture_output=True)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_miniled_unsupported_does_not_write(self):
        self.check_case("miniled_unsupported")

    def test_miniled_supported_default(self):
        self.check_case("miniled_supported")

    def test_miniled_registration_failure_does_not_write(self):
        self.check_case("miniled_registration_failure")

    def test_tdp_unsupported_errno_reaches_ioctl_entry_without_copy(self):
        self.check_case("tdp_unsupported")

    def test_tdp_ioctl_entry_preserves_all_nine_read_commands(self):
        self.check_case("tdp_supported")

    def test_tdp_ec_error_reaches_ioctl_entry(self):
        self.check_case("tdp_ec_error")

    def test_tdp_invalid_index_does_not_read_ec(self):
        self.check_case("tdp_invalid_index")

    def test_tdp_disabled_limit_does_not_read_ec(self):
        self.check_case("tdp_disabled_limit")

    def test_tdp_invalid_userspace_buffer_fault_reaches_ioctl_entry(self):
        self.check_case("tdp_copy_fault")

    def test_tdp_double_pl4_is_preserved(self):
        self.check_case("tdp_double_pl4")


if __name__ == "__main__":
    unittest.main()
