"""Deterministic pthread schedules run the driver's actual fan and PM functions."""
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


class UniwillFanConcurrency(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        parent = Path(os.environ.get("UNIWILL_TEST_TMPDIR", ROOT / ".local-tests"))
        parent.mkdir(parents=True, exist_ok=True)
        cls.temp = tempfile.TemporaryDirectory(prefix="fan-concurrency-", dir=parent)
        cls.addClassCleanup(cls.temp.cleanup)
        path = Path(cls.temp.name)
        keyboard = Path(os.environ.get("UNIWILL_FAN_HEADER", ROOT / "src/uniwill_keyboard.h")).read_text()
        keyboard = re.sub(
            r"#if LINUX_VERSION_CODE < KERNEL_VERSION\(6, 11, 0\)\n"
            r"static int uniwill_keyboard_remove\([^\n]*\)\n#else\n"
            r"(static void uniwill_keyboard_remove\([^\n]*\))\n#endif", r"\1", keyboard)
        interfaces = (ROOT / "src/uniwill_interfaces.h").read_text()
        constants = []
        for source, name in ((keyboard, "FAN_ON_MIN_SPEED_PERCENT"),
                             (interfaces, "NB02_FAN_SPEED_MAX"),
                             (interfaces, "UW_EC_REG_CUSTOM_PROFILE")):
            constants.append(re.search(r"(?m)^#define " + name + r"\s+.+$", source).group())
        start = keyboard.index("static u8 direct_fan_control_current_value_fan0")
        end = keyboard.index("static u8 uniwill_touchp_toggle_seq", start)
        fan = keyboard[start:end]
        # These macros annotate real kernel exports and work declarations only.
        names = ("uniwill_write_ec_ram_with_retry", "uniwill_keyboard_remove",
                 "uw_restore_fan_state_locked", "uw_restore_fan_state",
                 "uniwill_keyboard_suspend", "uniwill_keyboard_resume")
        tail = []
        for name in names:
            if re.search(r"(?m)^(?:static\s+)?(?:int|u32|void)\s+" + name + r"\(", keyboard):
                tail.append(function(keyboard, name))
        fixture = (ROOT / "tests/fixtures/uniwill_fan_concurrency.c").read_text()
        compat = ""
        cls.baseline = "int uw_set_mode(" not in keyboard
        if cls.baseline:
            io_path = os.environ.get("UNIWILL_FAN_IO_BASELINE")
            if not io_path:
                raise AssertionError("Baseline mode regression needs UNIWILL_FAN_IO_BASELINE")
            profile = function(Path(io_path).read_text(), "uw_set_performance_profile_v1")
            enum = re.search(r"enum uw_perf_profiles_v1\s*\{[^}]+\};", interfaces).group()
            compat = enum + "\n" + profile + """
/* Only adapt the new call signature; run the frozen production profile RMW. */
static int uw_update_mode(u8 clear, u8 set) {
    assert(clear == 0xb0 && set == 0xa0);
    return uw_set_performance_profile_v1(PROFILE_POWERSAVE);
}
/* These new APIs are not exercised by baseline sensitivity cases. */
static int uw_set_mode(u8 value) { (void)value; return -ENOSYS; }
static int uw_fan_control_start(void) { return -ENOSYS; }
"""
        fixture = fixture.replace("/* @BASELINE_COMPAT@ */", compat)
        fixture = fixture.replace("/* @LEGACY_PM_DECL@ */",
                                  "static bool uniwill_pm_ready, uniwill_pm_suspended;"
                                  if "static bool uniwill_pm_ready;" not in fan else "")
        custom_helper = ([function(keyboard, "uniwill_set_custom_profile_mode")]
                         if any("uniwill_set_custom_profile_mode(" in body for body in tail) else [])
        fixture = fixture.replace("/* @DRIVER_FUNCTIONS@ */", "\n\n".join(
            constants + custom_helper + [tail.pop(0), fan] + tail))
        source = path / "runtime.c"
        source.write_text(fixture)
        cls.binary = path / "runtime"
        result = subprocess.run(
            ["cc", "-std=c11", "-O2", "-Wall", "-Wextra", "-Werror", "-pthread",
             str(source), "-o", str(cls.binary)], text=True, capture_output=True,
        )
        if result.returncode:
            raise AssertionError(result.stdout + result.stderr)

    def check_case(self, case):
        result = subprocess.run([str(self.binary), case], text=True, capture_output=True,
                                timeout=10, preexec_fn=disable_core_dumps)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_universal_suspend_serializes_and_blocks_new_manual_request(self):
        self.check_case("universal_suspend_manual")

    def test_universal_resume_serializes_with_explicit_auto(self):
        self.check_case("universal_resume_auto")

    def test_universal_resume_drains_before_remove_and_rejects_later_calls(self):
        self.check_case("universal_resume_remove")

    def test_manual_after_auto_does_not_get_lost(self):
        self.check_case("auto_manual")

    def test_worker_cannot_overwrite_newer_manual_target(self):
        self.check_case("worker_manual")

    def test_auto_waits_for_worker_without_deadlocking_or_requeue(self):
        self.check_case("worker_auto")

    def test_initialization_cannot_reenable_tables_after_auto(self):
        self.check_case("init_auto")

    def test_parallel_initialization_only_programs_one_table(self):
        self.check_case("init_init")

    def test_auto_cannot_be_overwritten_by_older_full_mode_request(self):
        self.check_case("full_auto")

    def test_manual_waiting_for_suspend_is_rejected_until_resume_completes(self):
        self.check_case("suspend_manual")

    def test_resume_cannot_repopulate_cache_after_newer_auto(self):
        self.check_case("resume_auto")

    def test_remove_drains_worker_before_disabling_manual_mode(self):
        self.check_case("worker_remove")

    def test_remove_prevents_concurrent_and_later_manual_requeue(self):
        self.check_case("remove_manual")

    def test_mode_updates_preserve_bits_from_other_serialized_update(self):
        self.check_case("mode_update")

    def test_reprobe_start_clears_stale_cache_and_keeps_exports_closed_until_complete(self):
        if self.baseline:
            self.skipTest("New lifecycle start helper has no baseline equivalent")
        self.check_case("reprobe")


if __name__ == "__main__":
    unittest.main()
