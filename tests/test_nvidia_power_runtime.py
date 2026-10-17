"""Run the real NVIDIA power driver functions with userspace EC/DMI mocks."""

from pathlib import Path
import os
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


class NvidiaPowerRuntime(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        parent = Path(os.environ.get("MECHREVO_TEST_TMPDIR", ROOT / ".local-tests"))
        parent.mkdir(parents=True, exist_ok=True)
        cls.temp = tempfile.TemporaryDirectory(dir=parent)
        cls.addClassCleanup(cls.temp.cleanup)
        path = Path(cls.temp.name)
        driver = (ROOT / "src/tuxedo_nb02_nvidia_power_ctrl/tuxedo_nb02_nvidia_power_ctrl.c").read_text()
        interfaces = (ROOT / "src/uniwill_interfaces.h").read_text()
        constants = re.findall(r"(?m)^#define UW_EC_REG_CTGP_DB_\w+\s+[^\n]+", interfaces)
        if len(constants) != 7:
            raise AssertionError("Missing power register definitions")
        functions = [function(driver, name) for name in
                     ("uw_preserve_firmware_power_config", "init_db_and_ctgp", "ctgp_offset_store")]
        fixture = (ROOT / "tests/fixtures/nvidia_power_runtime.c").read_text()
        fixture = fixture.replace("/* @DRIVER_CONSTANTS@ */", "\n".join(constants))
        fixture = fixture.replace("/* @DRIVER_FUNCTIONS@ */", "\n\n".join(functions))
        source = path / "runtime.c"
        source.write_text(fixture)
        cls.binary = path / "runtime"
        result = subprocess.run(
            ["cc", "-std=c11", "-Wall", "-Wextra", "-Werror",
             str(source), "-o", str(cls.binary)], text=True, capture_output=True,
        )
        if result.returncode:
            raise AssertionError(result.stdout + result.stderr)

    def check_case(self, case):
        result = subprocess.run([str(self.binary), case], text=True, capture_output=True)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_verified_dmi_requires_exact_vendor_and_board(self):
        self.check_case("dmi_boundaries")

    def test_verified_model_load_preserves_all_firmware_power_settings(self):
        self.check_case("preserve_config")

    def test_other_models_keep_all_four_legacy_writes_in_order(self):
        self.check_case("legacy_config")

    def test_each_legacy_write_error_stops_initialization(self):
        self.check_case("legacy_write_errors")

    def test_same_ctgp_value_reads_without_writing_for_every_model(self):
        self.check_case("ctgp_unchanged")

    def test_changed_ctgp_value_writes_the_parsed_byte_for_every_model(self):
        self.check_case("ctgp_changed")

    def test_ctgp_read_failure_prevents_write(self):
        self.check_case("ctgp_read_error")

    def test_ctgp_write_failure_is_returned(self):
        self.check_case("ctgp_write_error")

    def test_ctgp_parse_failure_prevents_ec_access(self):
        self.check_case("ctgp_parse_error")


if __name__ == "__main__":
    unittest.main()
