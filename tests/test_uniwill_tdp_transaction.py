"""Run actual IO/provider/PM functions under deterministic pthread schedules."""
import os
from pathlib import Path
import re
import resource
import subprocess
import tempfile
import unittest

from test_uniwill_fan_runtime import function

ROOT = Path(__file__).resolve().parents[1]


def disable_core_dumps():
    resource.setrlimit(resource.RLIMIT_CORE, (0, 0))


class UniwillTdpTransaction(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        source_root = Path(os.environ.get("UNIWILL_TDP_SOURCE_ROOT", ROOT))
        keyboard = (source_root / "src/uniwill_keyboard.h").read_text()
        io = (source_root / "src/tuxedo_io/tuxedo_io.c").read_text()
        interfaces = (source_root / "src/uniwill_interfaces.h").read_text()
        keyboard = re.sub(
            r"#if LINUX_VERSION_CODE < KERNEL_VERSION\(6, 11, 0\)\n"
            r"static int uniwill_keyboard_remove\([^\n]*\)\n#else\n"
            r"(static void uniwill_keyboard_remove\([^\n]*\))\n#endif", r"\1", keyboard)
        constants = []
        for source, name in ((keyboard, "FAN_ON_MIN_SPEED_PERCENT"),
                             (interfaces, "NB02_FAN_SPEED_MAX"),
                             (interfaces, "UW_EC_REG_CUSTOM_PROFILE")):
            match = re.search(r"(?m)^#define " + name + r"\s+.+$", source)
            if not match:
                raise AssertionError("Missing driver constant: " + name)
            constants.append(match.group())
        start = keyboard.index("static u8 direct_fan_control_current_value_fan0")
        end = keyboard.index("static u8 uniwill_touchp_toggle_seq", start)
        # This complete provider section includes its actual locks, gates,
        # mode APIs, fan APIs, worker and lifecycle start, without reimplementing them.
        provider = keyboard[start:end]
        enum = re.search(r"enum uw_perf_profiles_v1\s*\{[^}]+\};", interfaces)
        if not enum:
            raise AssertionError("Missing driver profile enum")
        keyboard_names = ("uniwill_set_custom_profile_mode", "uniwill_write_ec_ram_with_retry")
        tail_names = ("uniwill_keyboard_prepare_remove", "uniwill_keyboard_remove",
                      "uw_restore_fan_state_locked", "uniwill_keyboard_suspend",
                      "uniwill_keyboard_resume")
        io_names = ["uw_get_tdp_min", "uw_get_tdp_max"]
        for name in ("uw_get_performance_profile_count", "uw_apply_performance_profile_v1"):
            if re.search(r"(?m)^static[^;{]*\b" + name + r"\([^;{]*\)\s*\{", io):
                io_names.append(name)
        io_names += ["uw_set_tdp", "uw_set_performance_profile_v1"]
        bodies = constants + [enum.group()] + [function(keyboard, name) for name in keyboard_names]
        bodies += [provider] + [function(keyboard, name) for name in tail_names]
        bodies += [function(io, name) for name in io_names]
        cls.baseline = not re.search(r"(?m)^int uw_write_tdp\(", keyboard)
        fixture = (ROOT / "tests/fixtures/uniwill_tdp_transaction.c").read_text()
        fixture = fixture.replace("/* @DRIVER_FUNCTIONS@ */", "\n\n".join(bodies))
        fixture = fixture.replace("/* @HAS_TDP_PROVIDER@ */",
                                  "#define HAS_TDP_PROVIDER " + str(int(not cls.baseline)))
        parent = Path(os.environ.get("UNIWILL_TEST_TMPDIR", ROOT / ".local-tests"))
        parent.mkdir(parents=True, exist_ok=True)
        cls.temp = tempfile.TemporaryDirectory(prefix="tdp-transaction-", dir=parent)
        cls.addClassCleanup(cls.temp.cleanup)
        path = Path(cls.temp.name)
        source = path / "runtime.c"
        source.write_text(fixture)
        cls.binaries = {}
        for debug in (False, True):
            binary = path / ("runtime-debug" if debug else "runtime")
            result = subprocess.run(
                ["cc", "-std=c11", "-O2", "-Wall", "-Wextra", "-Werror", "-pthread",
                 *(["-DDEBUG"] if debug else []), str(source), "-o", str(binary)],
                text=True, capture_output=True,
            )
            if result.returncode:
                raise AssertionError(result.stdout + result.stderr)
            cls.binaries[debug] = binary

    def check_case(self, case):
        for debug, binary in self.binaries.items():
            with self.subTest(debug=debug):
                result = subprocess.run([str(binary), case], text=True, capture_output=True,
                                        timeout=10, preexec_fn=disable_core_dumps)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_profile_waits_until_final_tdp_commit(self):
        self.check_case("tdp_profile")

    def test_raw_mode_waits_until_final_tdp_commit(self):
        self.check_case("tdp_raw_mode")

    def test_full_fan_mode_waits_until_final_tdp_commit(self):
        self.check_case("tdp_full_fan")

    def test_suspend_waits_until_final_custom_tdp_commit(self):
        self.check_case("tdp_suspend_custom")

    def test_suspend_waits_until_final_noncustom_tdp_commit(self):
        self.check_case("tdp_suspend_noncustom")

    def test_prepare_remove_waits_until_final_custom_tdp_commit(self):
        self.check_case("tdp_prepare_custom")

    def test_prepare_remove_waits_until_final_noncustom_tdp_commit(self):
        self.check_case("tdp_prepare_noncustom")

    def test_remove_waits_until_final_custom_tdp_commit(self):
        self.check_case("tdp_remove_custom")

    def test_remove_waits_until_final_noncustom_tdp_commit(self):
        self.check_case("tdp_remove_noncustom")

    def test_two_custom_tdp_writes_are_complete_serial_commands(self):
        self.check_case("tdp_tdp_custom")

    def test_two_noncustom_tdp_writes_are_complete_serial_commands(self):
        self.check_case("tdp_tdp_noncustom")

    def test_suspended_custom_tdp_is_rejected_without_ec(self):
        self.check_case("suspended_custom")

    def test_suspended_noncustom_tdp_is_rejected_without_ec(self):
        self.check_case("suspended_noncustom")

    def test_resume_completion_reopens_tdp_for_both_device_types(self):
        self.check_case("resume_recovery")

    def test_prepare_remove_rejects_tdp_for_both_device_types_without_ec(self):
        self.check_case("prepared")

    def test_removed_device_rejects_tdp_for_both_device_types_without_ec(self):
        self.check_case("removed")

    def test_unready_device_rejects_tdp_for_both_device_types_without_ec(self):
        self.check_case("unready")

    def test_custom_mode_read_failure_keeps_errno_and_has_no_writes(self):
        self.check_case("read_failure")

    def test_custom_mode_write_failure_keeps_errno_and_skips_tdp(self):
        self.check_case("mode_failure")

    def test_final_custom_tdp_failure_keeps_errno_and_does_not_rollback_mode(self):
        self.check_case("tdp_failure_custom")

    def test_final_noncustom_tdp_failure_keeps_errno_and_skips_mode(self):
        self.check_case("tdp_failure_noncustom")

    def test_failure_releases_command_and_state_locks_for_a_later_request(self):
        self.check_case("failure_recovery")

    def test_direct_provider_rejects_invalid_index_before_ec(self):
        if self.baseline:
            self.skipTest("Old source has no direct uw_write_tdp API; actual IO path is still tested")
        self.check_case("provider_invalid_index")


if __name__ == "__main__":
    unittest.main()
