"""Exercise real WMI functions with bounded, hardware-free userspace mocks."""
from pathlib import Path
import re
import os
import resource
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


def disable_core_dumps():
    resource.setrlimit(resource.RLIMIT_CORE, (0, 0))


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


class UniwillWmiRuntime(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        parent = Path(os.environ.get("UNIWILL_TEST_TMPDIR", ROOT / ".local-tests"))
        parent.mkdir(parents=True, exist_ok=True)
        cls.temp = tempfile.TemporaryDirectory(dir=parent)
        cls.addClassCleanup(cls.temp.cleanup)
        path = Path(cls.temp.name)
        wmi = Path(os.environ.get("UNIWILL_WMI_SOURCE", ROOT / "src/uniwill_wmi.c")).read_text()
        interfaces = (ROOT / "src/uniwill_interfaces.h").read_text()
        declarations = []
        for name, source in (
             *((name, wmi) for name in ("UNIWILL_EC_REG_LDAT", "UNIWILL_EC_REG_HDAT", "UNIWILL_EC_REG_FLAGS", "UNIWILL_EC_REG_CMDL", "UNIWILL_EC_REG_CMDH", "UNIWILL_EC_BIT_RFLG", "UNIWILL_EC_BIT_WFLG", "UNIWILL_EC_BIT_BFLG", "UNIWILL_EC_BIT_DRDY", "UW_EC_BUSY_WAIT_CYCLES", "UW_EC_BUSY_WAIT_DELAY")),
            ("UW_WMI_EC_INPUT_SIZE", wmi), ("UW_WMI_EC_RESPONSE_WORDS", wmi),
            ("UNIWILL_WMI_MGMT_GUID_BC", interfaces),
            ("UNIWILL_WMI_FUNCTION_READ", interfaces),
            ("UNIWILL_WMI_FUNCTION_WRITE", interfaces),
        ):
            match = re.search(r"(?m)^#define\s+" + re.escape(name) + r"\s+.+$", source)
            if not match:
                raise AssertionError("Missing driver constant: " + name)
            declarations.append(match.group())
        for name in ("uw_ec_read_return", "uw_ec_write_return"):
            match = re.search(r"(?ms)^union " + name + r" \{.*?^};", interfaces)
            if not match:
                raise AssertionError("Missing driver type: " + name)
            declarations.append(match.group())
        functions = [function(wmi, name) for name in (
            "uw_wmi_ec_evaluate", "uw_ec_read_addr_wmi", "uw_ec_write_addr_wmi",
            "uw_ec_read_addr_direct", "uw_ec_write_addr_direct",
            "uw_wmi_read_ec_ram", "uw_wmi_write_ec_ram",
        )]
        fixture = (ROOT / "tests/fixtures/uniwill_wmi_runtime.c").read_text()
        fixture = fixture.replace("/* @DRIVER_DECLARATIONS@ */", "\n\n".join(declarations))
        fixture = fixture.replace("/* @DRIVER_FUNCTIONS@ */", "\n\n".join(functions))
        source = path / "runtime.c"
        source.write_text(fixture)
        cls.binary = path / "runtime"
        result = subprocess.run(
            ["cc", "-std=c11", "-Wall", "-Wextra", "-Werror", "-g",
             "-fsanitize=address,undefined", "-fno-omit-frame-pointer", "-fno-pie", "-no-pie",
             str(source), "-o", str(cls.binary)], text=True, capture_output=True,
        )
        if result.returncode:
            raise AssertionError(result.stdout + result.stderr)

    def check_case(self, case):
        result = subprocess.run([str(self.binary), case], text=True, capture_output=True, timeout=10, preexec_fn=disable_core_dumps)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_allocation_failure_never_calls_wmi(self):
        self.check_case("allocation_failure")

    def test_null_destination_never_allocates_or_calls_wmi(self):
        self.check_case("null_destination")

    def test_eight_byte_request_preserves_argument_function_and_reserved_bytes(self):
        self.check_case("request_abi")

    def test_acpi_failure_with_object_does_not_copy_and_frees_response(self):
        self.check_case("acpi_failure_with_object")

    def test_acpi_failure_without_object_preserves_output(self):
        self.check_case("acpi_failure_without_object")

    def test_missing_acpi_object_is_an_error(self):
        self.check_case("missing_object")

    def test_non_buffer_acpi_object_is_an_error(self):
        self.check_case("wrong_type")

    def test_null_buffer_pointer_is_an_error(self):
        self.check_case("null_payload")

    def test_all_short_response_lengths_are_errors(self):
        self.check_case("short_payload")

    def test_oversized_response_does_not_overwrite_output_or_canary(self):
        self.check_case("oversized_payload")

    def test_every_valid_length_from_four_through_forty_preserves_bytes_and_bounds(self):
        self.check_case("valid_lengths")

    def test_read_and_write_helpers_preserve_outputs_on_all_response_errors(self):
        self.check_case("helper_errors")

    def test_read_and_write_helpers_reject_error_sentinel_without_changing_output(self):
        self.check_case("sentinel")

    def test_successful_helpers_preserve_read_write_argument_and_result_abi(self):
        self.check_case("helper_success")

    def test_wmi_ram_read_failure_preserves_callers_byte(self):
        self.check_case("ram_read_error")

    def test_direct_ram_read_failure_preserves_callers_byte(self):
        self.check_case("direct_ram_read_error")

    def test_ram_read_success_returns_only_low_data_byte(self):
        self.check_case("ram_read_success")

    def test_ram_write_returns_wmi_and_direct_errors(self):
        self.check_case("ram_write_error")

    def test_direct_success_preserves_exact_handshake_order_and_payload(self):
        self.check_case("direct_success")

    def test_direct_each_access_failure_and_landed_write_preserve_errno_and_output(self):
        self.check_case("direct_each_error")

    def test_direct_primary_errno_survives_independently_logged_cleanup_failure(self):
        self.check_case("direct_cleanup_errors")

    def test_direct_final_allowed_ready_poll_succeeds_and_timeout_is_bounded(self):
        self.check_case("direct_last_poll_and_timeout")

    def test_direct_each_poll_error_is_not_misreported_as_timeout(self):
        self.check_case("direct_poll_errors")

    def test_direct_caller_output_stays_unchanged_until_cleanup_success(self):
        self.check_case("direct_caller_guard")


if __name__ == "__main__":
    unittest.main()
