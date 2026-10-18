"""Compile real Uniwill driver functions with safe userspace mocks (requires cc)."""
from pathlib import Path
import os
import re
import subprocess
import tempfile
import unittest

from test_uniwill_fan_runtime import function as provider_function
from uniwill_curve_compat import curve_ioctl_compat, no_curve_owner_hooks

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
        parent = Path(os.environ.get("UNIWILL_TEST_TMPDIR", ROOT / ".local-tests"))
        parent.mkdir(parents=True, exist_ok=True)
        cls.temp = tempfile.TemporaryDirectory(dir=parent)
        cls.addClassCleanup(cls.temp.cleanup)
        path = Path(cls.temp.name)
        source_root = Path(os.environ.get("UNIWILL_IO_SOURCE_ROOT", ROOT))
        keyboard = (source_root / "src/uniwill_keyboard.h").read_text()
        io = (source_root / "src/tuxedo_io/tuxedo_io.c").read_text()
        interfaces = (source_root / "src/uniwill_interfaces.h").read_text()
        fan_constant = re.search(r"(?m)^#define FAN_ON_MIN_SPEED_PERCENT\s+.+$", io)
        if not fan_constant:
            raise AssertionError("Missing fan minimum speed definition")
        functions = [fan_constant.group(), function(keyboard, "uw_mini_led_local_dimming_init")]
        # Baseline sensitivity runs use the same fixture against the prior
        # source. New private helpers are absent there, rather than mocked.
        for name in ("uw_get_performance_profile_count", "uw_apply_performance_profile_v1"):
            if re.search(r"(?m)^static[^;{]*\b" + name + r"\([^;{]*\)\s*\{", io):
                functions.append(function(io, name))
        functions.extend(function(io, name) for name in
                         ("uw_get_tdp_min", "uw_get_tdp_max", "uw_get_tdp", "uw_set_tdp",
                          "uw_set_performance_profile_v1", "uw_copy_tdp_value"))
        types = []
        for kind, name in (("union", "uw_ec_read_return"), ("union", "uw_ec_write_return"),
                           ("enum", "uw_perf_profiles_v1")):
            declaration = re.search(r"(?ms)^" + kind + " " + name + r" \{.*?^};", interfaces)
            if not declaration:
                raise AssertionError("Missing driver type: " + name)
            types.append(declaration.group())
        fixture = (ROOT / "tests/fixtures/uniwill_runtime.c").read_text()
        fixture = fixture.replace("/* @CURVE_IOCTL_COMPAT@ */", curve_ioctl_compat(io))
        provider = ""
        if re.search(r"(?m)^int uw_write_tdp\(", keyboard):
            provider_names = ("uw_fan_lock", "uw_fan_unlock", "uw_fan_lock_active", "uw_write_tdp")
            if re.search(r"(?m)^int uw_fan_curve_debug_write\(", keyboard):
                provider_names += ("uw_fan_curve_debug_write",)
            provider = "#define ACTUAL_TDP_PROVIDER 1\n" + no_curve_owner_hooks(keyboard) + "\n" + "\n\n".join(
                provider_function(keyboard, name) for name in provider_names)
        else:
            provider = "#define ACTUAL_TDP_PROVIDER 0\n"
        fixture = fixture.replace("/* @DRIVER_TDP_PROVIDER@ */", provider)
        fixture = fixture.replace("/* @DRIVER_FUNCTIONS@ */", "\n\n".join(functions))
        fixture = fixture.replace("/* @DRIVER_INTERFACE_TYPES@ */", "\n\n".join(types))
        prototypes = []
        for name in ("uw_set_mode", "uw_update_mode"):
            prototype = re.search(r"(?m)^int " + name + r"\([^;]+\);$", interfaces)
            if not prototype:
                raise AssertionError("Missing driver prototype: " + name)
            prototypes.append(prototype.group())
        fixture = fixture.replace("/* @DRIVER_MODE_PROTOTYPES@ */", "\n".join(prototypes))
        fixture = fixture.replace("/* @DRIVER_UNIWILL_IOCTL@ */", function(io, "uniwill_ioctl_interface"))
        fixture = fixture.replace("/* @DRIVER_IOCTL_ENTRY@ */", function(io, "fop_ioctl"))
        source = path / "runtime.c"
        source.write_text(fixture)
        cls.binaries = {}
        for debug in (False, True):
            binary = path / ("runtime-debug" if debug else "runtime")
            result = subprocess.run(
                ["cc", "-std=c11", "-Wall", "-Wextra", "-Werror",
                 "-I", str(source_root / "src/tuxedo_io"), "-I", str(source_root / "src"),
                 *(["-DDEBUG"] if debug else []),
                 str(source), "-o", str(binary)], text=True, capture_output=True,
            )
            if result.returncode:
                raise AssertionError(result.stdout + result.stderr)
            cls.binaries[debug] = binary

    def check_case(self, case, debug_only=False):
        for debug, binary in self.binaries.items():
            if debug_only and not debug:
                continue
            with self.subTest(debug=debug):
                result = subprocess.run([str(binary), case], text=True, capture_output=True)
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

    def test_all_six_ec_read_failures_return_errno_without_copy(self):
        self.check_case("ec_read_error")

    def test_all_six_successful_ec_reads_keep_u32_abi(self):
        self.check_case("ec_read_success")

    def test_fan_zero_mapping_is_preserved(self):
        self.check_case("fan_zero_mapping")

    def test_all_uniwill_and_generic_read_copy_faults_return_efault(self):
        self.check_case("all_read_copy_faults")

    def test_interface_strings_and_metadata_abi_are_preserved(self):
        self.check_case("read_metadata_success")

    def test_partial_copy_from_fault_never_reaches_any_write_helper(self):
        self.check_case("write_copy_faults")

    def test_fan_commands_preserve_every_valid_byte_and_selected_fan(self):
        self.check_case("fan_write_success")

    def test_oversized_fan_speed_never_reaches_helper(self):
        self.check_case("fan_write_out_of_range")

    def test_unsigned_fan_helper_errno_reaches_ioctl_as_signed(self):
        self.check_case("fan_write_error")

    def test_successful_mode_write_keeps_low_byte_abi(self):
        self.check_case("mode_write_success")

    def test_mode_write_ec_failure_reaches_ioctl_entry(self):
        self.check_case("mode_write_error")

    def test_mode_command_uses_serialized_helper_and_propagates_its_errno(self):
        self.check_case("mode_helper_error")

    def test_unsupported_tdp_write_does_not_change_profile(self):
        self.check_case("tdp_write_unsupported")

    def test_invalid_tdp_index_does_not_change_profile(self):
        self.check_case("tdp_write_invalid_index")

    def test_invalid_tdp_value_does_not_change_profile(self):
        self.check_case("tdp_write_invalid_value")

    def test_missing_or_disabled_tdp_max_does_not_change_profile(self):
        self.check_case("tdp_write_invalid_max")

    def test_successful_tdp_writes_preserve_all_three_addresses(self):
        self.check_case("tdp_write_success")

    def test_double_pl4_write_preserves_integer_division(self):
        self.check_case("tdp_write_double_pl4")

    def test_all_three_tdp_write_errors_reach_ioctl_entry(self):
        self.check_case("tdp_write_ec_error")

    def test_tdp_profile_read_error_prevents_all_writes(self):
        self.check_case("tdp_profile_read_error")

    def test_tdp_profile_write_error_prevents_tdp_write(self):
        self.check_case("tdp_profile_write_error")

    def test_tdp_profile_change_precedes_valid_tdp_write(self):
        self.check_case("tdp_profile_success")

    def test_tdp_provider_gate_error_prevents_tdp_write(self):
        self.check_case("tdp_provider_gate_error")

    def test_tdp_write_error_after_profile_change_is_reported(self):
        self.check_case("tdp_ec_error_after_profile")

    def test_profile_read_error_does_not_write(self):
        self.check_case("profile_read_error")

    def test_profile_write_error_reaches_ioctl_entry(self):
        self.check_case("profile_write_error")

    def test_profile_command_uses_serialized_helper_and_propagates_its_errno(self):
        self.check_case("profile_helper_error")

    def test_invalid_profile_does_not_write(self):
        self.check_case("profile_invalid")

    def test_all_three_profiles_preserve_other_mode_bits(self):
        self.check_case("profile_success")

    def test_all_profiles_pass_exact_masks_for_every_previous_mode_byte(self):
        self.check_case("profile_masks")

    def test_profile_counts_preserve_all_feature_flag_combinations(self):
        self.check_case("profile_count_matrix")

    def test_public_profile_writes_follow_all_advertised_capabilities(self):
        self.check_case("profile_capability_matrix")

    def test_two_profile_device_rejects_overboost_without_ec_access(self):
        self.check_case("profile_two_rejects_overboost")

    def test_two_profile_flag_keeps_precedence_over_three_and_leds_only(self):
        self.check_case("profile_two_precedence")

    def test_leds_only_profiles_keep_the_established_mode_bits(self):
        self.check_case("profile_leds_only")

    def test_custom_only_device_rejects_public_profiles_but_keeps_tdp_mode(self):
        self.check_case("profile_custom_only_tdp")

    def test_invalid_profiles_remain_einval_for_every_capability_combination(self):
        self.check_case("profile_invalid_capability_matrix")

    def test_disabled_mode_enable_remains_noop_without_accessing_its_argument(self):
        self.check_case("mode_enable_noop")

    def test_unknown_commands_return_enotty_without_user_copy_or_hardware_access(self):
        self.check_case("unknown_commands")

    def test_all_uniwill_commands_reject_wrong_direction_size_and_type(self):
        self.check_case("malformed_commands")

    def test_debug_partial_input_copy_fault_prevents_ec_access(self):
        self.check_case("debug_copy_from_faults", debug_only=True)

    def test_debug_read_error_prevents_output_copy(self):
        self.check_case("debug_read_error", debug_only=True)

    def test_debug_write_error_prevents_output_copy(self):
        self.check_case("debug_write_error", debug_only=True)

    def test_debug_output_copy_fault_is_reported(self):
        self.check_case("debug_copy_to_faults", debug_only=True)

    def test_debug_successful_register_command_abi_is_preserved(self):
        self.check_case("debug_success", debug_only=True)


if __name__ == "__main__":
    unittest.main()
