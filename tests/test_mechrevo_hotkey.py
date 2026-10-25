"""Regression tests for the physical performance-mode cycle."""
import importlib.util
import fcntl
import io
import os
from pathlib import Path
import select
import struct
import unittest
from contextlib import ExitStack, redirect_stdout
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

    def test_gaming_quiet_uses_the_gaming_hardware_mode_request(self):
        def ioctl(_fd, command, argument, *_):
            if command == 0x8008EF14:
                argument[:4] = struct.pack("i", 0)
            return 0

        with mock.patch.object(control.os, "open", return_value=7), \
                mock.patch.object(control.os, "close"), \
                mock.patch.object(control.fcntl, "ioctl", side_effect=ioctl):
            control.set_hardware_mode_led("gaming-quiet")

    def test_notification_failure_is_visible(self):
        with mock.patch.object(control.shutil, "which", return_value=None), \
                self.assertRaisesRegex(RuntimeError, "notify-send"):
            control.send_notification("title", "message")




class ConfirmedProfileTests(unittest.TestCase):
    def setUp(self):
        self.stack = ExitStack()
        self.addCleanup(self.stack.close)
        self.stack.enter_context(mock.patch.object(control, "sync_oem_profiles"))
        self.bus = self.stack.enter_context(mock.patch.object(control, "call_tccd_dbus",
                                                              return_value='{"type":"b","data":[true]}'))
        self.active = self.stack.enter_context(mock.patch.object(control, "get_current_mode_info",
                                                                 return_value=("Gaming Quiet", "mechrevo_gaming_quiet")))
        self.led = self.stack.enter_context(mock.patch.object(control, "set_hardware_mode_led"))
        self.notify = self.stack.enter_context(mock.patch.object(control, "send_notification"))
        self.sleep = self.stack.enter_context(mock.patch.object(control.time, "sleep"))
        self.output = self.stack.enter_context(redirect_stdout(io.StringIO()))

    def test_quiet_mode_confirms_profile_before_lamp_or_success_notification(self):
        events = []
        profiles = iter([("Gaming", "mechrevo_gaming"), ("Quiet", "mechrevo_gaming_quiet")])
        def active():
            value = next(profiles)
            events.append(value[1])
            return value
        self.active.side_effect = active
        self.led.side_effect = lambda mode: events.append("led:" + mode)
        control.set_mode("gaming-quiet", notify=True)
        self.bus.assert_called_once_with("SetTempProfileById", "s", "mechrevo_gaming_quiet")
        self.assertEqual(events, ["mechrevo_gaming", "mechrevo_gaming_quiet", "led:gaming-quiet"])
        self.notify.assert_called_once()
        self.assertIn("[✓]", self.output.getvalue())

    def test_rejected_or_malformed_reply_never_writes_lamp_or_announces_success(self):
        for response in ('{"type":"b","data":[false]}', '{', '{}',
                         '{"type":"b","data":[1]}', '{"type":"s","data":[true]}'):
            with self.subTest(response=response):
                self.bus.return_value = response
                with self.assertRaises(RuntimeError):
                    control.set_mode("gaming-quiet", notify=True)
        self.active.assert_not_called()
        self.led.assert_not_called()
        self.notify.assert_not_called()
        self.assertNotIn("[✓]", self.output.getvalue())

    def test_profile_timeout_never_writes_lamp_or_announces_success(self):
        self.active.return_value = ("Gaming", "mechrevo_gaming")
        with mock.patch.object(control.time, "monotonic", side_effect=[10.0, 15.0]), \
                self.assertRaisesRegex(RuntimeError, "Timed out"):
            control.set_mode("gaming-quiet", notify=True)
        self.led.assert_not_called()
        self.notify.assert_not_called()
        self.assertNotIn("[✓]", self.output.getvalue())

    def test_profile_read_failure_is_propagated_before_lamp_or_success_notification(self):
        self.active.side_effect = OSError("bus disconnected")
        with self.assertRaisesRegex(OSError, "bus disconnected"):
            control.set_mode("gaming-quiet", notify=True)
        self.led.assert_not_called()
        self.notify.assert_not_called()
        self.assertNotIn("[✓]", self.output.getvalue())

    def test_switch_transport_failure_never_writes_lamp_or_announces_success(self):
        self.bus.side_effect = OSError("switch request disconnected")
        with self.assertRaisesRegex(OSError, "switch request disconnected"):
            control.set_mode("gaming-quiet", notify=True)
        self.active.assert_not_called()
        self.led.assert_not_called()
        self.notify.assert_not_called()
        self.assertNotIn("[✓]", self.output.getvalue())


class HotkeyStreamTests(unittest.TestCase):
    def run_pipe(self, payload, *, close_writer=False):
        reader, writer = os.pipe()
        self.addCleanup(os.close, reader)
        if payload:
            os.write(writer, payload)
        if close_writer:
            os.close(writer)
        else:
            self.addCleanup(os.close, writer)
        real_select = select.select
        def ready(readers, writers, errors, timeout):
            result = real_select(readers, writers, errors, 0)
            if not result[0]:
                raise AssertionError("The F6 event was buffered; no second input should be needed")
            return result
        def stream(_path, mode, **kwargs):
            return os.fdopen(os.dup(reader), mode, **kwargs)
        stack = ExitStack()
        self.addCleanup(stack.close)
        stack.enter_context(mock.patch.object(Path, "exists", return_value=True))
        stack.enter_context(mock.patch.object(control, "open", side_effect=stream, create=True))
        stack.enter_context(mock.patch.object(control, "sync_oem_profiles"))
        stack.enter_context(mock.patch.object(control, "sync_mode_led_from_profile", return_value="mechrevo_gaming"))
        stack.enter_context(mock.patch.object(select, "select", side_effect=ready))
        stack.enter_context(redirect_stdout(io.StringIO()))
        return stack

    def test_one_complete_mode_chord_is_handled_without_a_later_input(self):
        class ModeHandled(Exception):
            pass
        chord = [(125, 1), (56, 1), (64, 1), (64, 0), (56, 0), (125, 0)]
        payload = b"".join(struct.pack("qqHHi", 0, 0, 1, code, value) for code, value in chord)
        self.run_pipe(payload)
        with mock.patch.object(control, "cycle_mode", side_effect=ModeHandled) as cycle, \
                self.assertRaises(ModeHandled):
            control.run_daemon()
        cycle.assert_called_once_with(notify=True)

    def test_eof_is_a_visible_device_failure(self):
        self.run_pipe(b"", close_writer=True)
        with self.assertRaisesRegex(RuntimeError, "disconnected"):
            control.run_daemon()

    def test_short_input_event_is_a_visible_failure(self):
        self.run_pipe(b"short", close_writer=True)
        with self.assertRaisesRegex(RuntimeError, "Incomplete hotkey input event"):
            control.run_daemon()


if __name__ == "__main__":
    unittest.main()
