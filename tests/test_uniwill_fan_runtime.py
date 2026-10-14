"""Run the actual fan helpers with mocked EC memory and a manually run worker."""
from pathlib import Path
import re
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


def function(source, name):
    match = re.search(r"(?m)^(?:static\s+)?(?:int|u32|void)\s+" + re.escape(name)
                      + r"\([^;{]*\)\s*\{", source)
    if not match:
        raise AssertionError("Missing driver function: " + name)
    depth, position = 1, match.end()
    while depth and position < len(source):
        depth += (source[position] == "{") - (source[position] == "}")
        position += 1
    if depth:
        raise AssertionError("Unbalanced driver function: " + name)
    return source[match.start():position]


class UniwillFanRuntime(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        parent = ROOT / ".local-tests"
        parent.mkdir(exist_ok=True)
        cls.temp = tempfile.TemporaryDirectory(prefix="fan-runtime-", dir=parent)
        cls.addClassCleanup(cls.temp.cleanup)
        path = Path(cls.temp.name)
        keyboard = (ROOT / "src/uniwill_keyboard.h").read_text()
        interfaces = (ROOT / "src/uniwill_interfaces.h").read_text()
        constants = []
        for source, name in ((keyboard, "FAN_ON_MIN_SPEED_PERCENT"),
                             (interfaces, "NB02_FAN_SPEED_MAX")):
            match = re.search(r"(?m)^#define " + name + r"\s+.+$", source)
            if not match:
                raise AssertionError("Missing driver constant: " + name)
            constants.append(match.group())
        names = ("uniwill_write_ec_ram_with_retry", "set_full_fan_mode", "uw_init_fan",
                 "restart_direct_fan_control_work_handler", "direct_fan_control",
                 "uw_set_fan", "uw_set_fan_auto", "uw_restore_fan_state", "uniwill_keyboard_suspend",
                 "uniwill_keyboard_resume")
        fixture = (ROOT / "tests/fixtures/uniwill_fan_runtime.c").read_text()
        fixture = fixture.replace("/* @DRIVER_FUNCTIONS@ */", "\n\n".join(
            constants + [function(keyboard, name) for name in names]))
        source = path / "runtime.c"
        source.write_text(fixture)
        cls.binary = path / "runtime"
        result = subprocess.run(
            ["cc", "-std=c11", "-O2", "-Wall", "-Wextra", "-Werror",
             str(source), "-o", str(cls.binary)], text=True, capture_output=True,
        )
        if result.returncode:
            raise AssertionError(result.stdout + result.stderr)

    def check_case(self, case):
        result = subprocess.run([str(self.binary), case], text=True, capture_output=True)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_retry_requires_positive_attempt_count(self):
        self.check_case("retry_invalid")

    def test_retry_returns_each_transport_error_and_retries_only_three_times(self):
        self.check_case("retry_errors")

    def test_retry_rejects_exhausted_readback_mismatch(self):
        self.check_case("retry_mismatch")

    def test_retry_recovers_from_transient_write_read_and_mismatch_failures(self):
        self.check_case("retry_recovers")

    def test_all_exported_paths_preserve_transport_errno(self):
        self.check_case("errno_propagation")

    def test_full_fan_mode_is_idempotent_and_preserves_other_bits(self):
        self.check_case("mode_success")

    def test_full_fan_mode_does_not_write_after_read_error(self):
        self.check_case("mode_errors")

    def test_init_preserves_every_table_value_and_write_order(self):
        self.check_case("init_success")

    def test_init_each_ec_operation_failure_stops_and_keeps_cache_invalid(self):
        self.check_case("init_failures")

    def test_init_preserves_already_enabled_bits_and_unsupported_noop(self):
        self.check_case("init_boundaries")

    def test_all_universal_speeds_preserve_mapping_and_both_addresses(self):
        self.check_case("universal_speeds")

    def test_invalid_fan_index_and_universal_speed_never_access_ec(self):
        self.check_case("fan_invalid")

    def test_universal_full_fan_mode_still_leaves_manual_request_unchanged(self):
        self.check_case("fan_full_mode_noop")

    def test_universal_read_init_and_two_write_failures_reach_exported_helper(self):
        self.check_case("fan_errors")

    def test_all_legacy_speeds_preserve_byte_range_and_success_cache(self):
        self.check_case("legacy_speeds")

    def test_sync_write_failure_keeps_previous_speed_cache(self):
        self.check_case("direct_errors")

    def test_legacy_first_request_queues_without_claiming_a_hardware_write(self):
        self.check_case("worker_queue")

    def test_worker_preserves_twenty_writes_and_fifty_minute_schedule(self):
        self.check_case("worker_success")

    def test_worker_each_ec_failure_logs_stops_and_keeps_manual_recovery_state(self):
        self.check_case("worker_failures")

    def test_worker_periodic_success_clears_failure_restart_state(self):
        self.check_case("worker_periodic_recovery")

    def test_auto_success_preserves_other_bits_and_clears_cache(self):
        self.check_case("auto_success")

    def test_auto_each_ec_failure_is_reported_and_does_not_claim_cache_reset(self):
        self.check_case("auto_failures")

    def test_legacy_auto_failure_stops_worker_but_keeps_speed_cache(self):
        self.check_case("legacy_auto_errors")

    def test_failed_auto_of_queued_request_is_recovered_by_next_manual_request(self):
        self.check_case("legacy_auto_queue_recovery")

    def test_suspend_resume_preserves_both_nonzero_fan_targets(self):
        self.check_case("pm_success")

    def test_resume_second_fan_failure_cannot_make_queued_worker_write_zero(self):
        self.check_case("pm_resume_second_fan_error")

    def test_suspend_fan_errors_keep_saved_targets_and_do_not_disable_backlight(self):
        self.check_case("pm_suspend_fan_errors")

    def test_repeated_resume_failures_preserve_targets_and_return_errno(self):
        self.check_case("pm_resume_retry_errors")

    def test_repeated_suspend_does_not_overwrite_pending_resume_targets(self):
        self.check_case("pm_repeated_suspend")

    def test_pm_custom_profile_read_and_write_errors_are_reported_safely(self):
        self.check_case("pm_profile_errors")

    def test_failed_suspend_rolls_back_without_a_resume_callback(self):
        self.check_case("pm_suspend_abort_recovery")

    def test_explicit_manual_updates_and_auto_cancels_pending_pm_targets(self):
        self.check_case("pm_explicit_commands")


if __name__ == "__main__":
    unittest.main()
