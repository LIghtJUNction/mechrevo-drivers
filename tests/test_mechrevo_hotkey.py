"""Regression tests for the physical performance-mode cycle."""
import importlib.util
from pathlib import Path
import struct
import unittest
from unittest import mock


ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / "scripts" / "mechrevo_control.py"


def load_control_module():
    spec = importlib.util.spec_from_file_location("mechrevo_control_under_test", SOURCE)
    if spec is None or spec.loader is None:
        raise AssertionError(f"Unable to load {SOURCE}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


control = load_control_module()


class PerformanceModeCycleTests(unittest.TestCase):
    def call_with_profile(self, profile_id):
        with mock.patch.object(control, "get_current_mode_info", return_value=("", profile_id)), \
                mock.patch.object(control, "set_mode") as set_mode:
            result = control.cycle_mode(notify=False)
        return result, set_mode

    def test_gaming_advances_to_gaming_quiet(self):
        result, set_mode = self.call_with_profile("mechrevo_gaming")
        self.assertEqual(result, "gaming-quiet")
        set_mode.assert_called_once_with("gaming-quiet", notify=False)

    def test_gaming_quiet_advances_to_turbo(self):
        result, set_mode = self.call_with_profile("mechrevo_gaming_quiet")
        self.assertEqual(result, "turbo")
        set_mode.assert_called_once_with("turbo", notify=False)

    def test_turbo_wraps_back_to_gaming(self):
        result, set_mode = self.call_with_profile("mechrevo_turbo")
        self.assertEqual(result, "gaming")
        set_mode.assert_called_once_with("gaming", notify=False)

    def test_unknown_profile_falls_back_to_gaming(self):
        result, set_mode = self.call_with_profile("some-user-profile")
        self.assertEqual(result, "gaming")
        set_mode.assert_called_once_with("gaming", notify=False)


class ControlFailureTests(unittest.TestCase):
    def test_dbus_transport_failure_is_not_replaced_by_default_mode(self):
        with mock.patch.object(control, "call_tccd_dbus", side_effect=OSError("bus down")), \
                self.assertRaises(OSError):
            control.get_current_mode_info()

    def test_malformed_dbus_profile_is_rejected(self):
        with mock.patch.object(control, "call_tccd_dbus", return_value="{}"), \
                self.assertRaisesRegex(RuntimeError, "no active profile"):
            control.get_current_mode_info()

    def test_mode_ioctl_failure_is_raised_and_fd_is_closed(self):
        with mock.patch.object(control.os, "open", return_value=7), \
                mock.patch.object(control.os, "close") as close, \
                mock.patch.object(control.fcntl, "ioctl", side_effect=OSError("ioctl failed")), \
                self.assertRaises(OSError):
            control.set_hardware_mode_led("gaming")
        close.assert_called_once_with(7)

    def test_mode_readback_mismatch_is_not_reported_as_success(self):
        def ioctl(_fd, command, argument, *_):
            if command == 0x8008EF14:
                argument[:4] = struct.pack("i", 0x10)
            return 0

        with mock.patch.object(control.os, "open", return_value=7), \
                mock.patch.object(control.os, "close") as close, \
                mock.patch.object(control.fcntl, "ioctl", side_effect=ioctl), \
                self.assertRaisesRegex(RuntimeError, "readback mismatch"):
            control.set_hardware_mode_led("gaming")
        close.assert_called_once_with(7)

    def test_notification_failure_is_visible(self):
        with mock.patch.object(control.shutil, "which", return_value=None), \
                self.assertRaisesRegex(RuntimeError, "notify-send"):
            control.send_notification("title", "message")


if __name__ == "__main__":
    unittest.main()
