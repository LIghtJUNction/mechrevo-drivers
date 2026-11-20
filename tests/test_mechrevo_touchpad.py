# SPDX-License-Identifier: GPL-2.0-or-later
"""Exercise real filesystem discovery, writes, readback, rollback and locking."""

from concurrent.futures import ThreadPoolExecutor
from contextlib import ExitStack, redirect_stdout
import importlib.util
import io
import json
import os
from pathlib import Path
import tempfile
import threading
import time
import unittest
from unittest import mock


SPEC = importlib.util.spec_from_file_location(
    "mechrevo_touchpad", Path(__file__).resolve().parents[1] / "scripts/mechrevo_touchpad.py",
)
touchpad = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(touchpad)


class TouchpadTests(unittest.TestCase):
    def setUp(self):
        self.stack = ExitStack()
        self.addCleanup(self.stack.close)
        self.root = Path(self.stack.enter_context(tempfile.TemporaryDirectory()))
        self.sysfs = self.root / "sys"
        self.state = self.root / "var/lib/mechrevo-touchpad"
        self.lock = self.root / "run/mechrevo-touchpad.lock"
        self.sysfs.mkdir()
        self.state.parent.mkdir(parents=True)
        self.lock.parent.mkdir(parents=True)
        for name, value in (
            ("SYS_ROOT", self.sysfs), ("STATE_DIR", self.state), ("LOCK_PATH", self.lock),
            ("_TRUST_ROOT", self.root), ("_OWNER_UID", os.geteuid()),
        ):
            self.stack.enter_context(mock.patch.object(touchpad, name, value))
        self.stack.enter_context(mock.patch.object(touchpad.os, "geteuid", return_value=0))
        self.stack.enter_context(mock.patch.dict(os.environ, {}, clear=True))
        self.records = []
        self.command = self.stack.enter_context(mock.patch.object(touchpad, "_command", side_effect=self.run_command))

    def run_command(self, command):
        if command != ["/usr/bin/udevadm", "info", "--export-db"]:
            raise AssertionError(command)
        return "\n\n".join(self.records)

    def device(self, number=0, enabled=True, tags=None, seat="seat0"):
        path = self.sysfs / f"devices/platform/pad{number}/input/input{number}"
        (path / f"event{number}").mkdir(parents=True)
        (path / "name").write_text(f"Touchpad {number}\n")
        (path / "inhibited").write_text("0\n" if enabled else "1\n")
        props = {"SUBSYSTEM": "input", "ID_INPUT": "1", "ID_INPUT_TOUCHPAD": "1", "ID_SEAT": seat}
        props.update(tags or {})
        self.records.append(
            f"P: /{(path / f'event{number}').relative_to(self.sysfs)}\n"
            + "\n".join(f"E: {key}={value}" for key, value in props.items())
        )
        return path

    def saved(self, enabled, seat="seat0"):
        self.state.mkdir(exist_ok=True)
        path = self.state / touchpad._state_name(seat)
        path.write_text(json.dumps({"version": 1, "seat": seat, "enabled": enabled}) + "\n")
        return path

    def assert_enabled(self, path, enabled):
        self.assertEqual((path / "inhibited").read_text().strip(), "0" if enabled else "1")

    def test_status_discovers_and_deduplicates_using_one_udev_snapshot(self):
        self.device()
        self.records.append(self.records[0])
        result = touchpad.control("status")
        self.assertTrue(result["enabled"])
        self.assertFalse(result["mixed"])
        self.assertEqual([item["name"] for item in result["devices"]], ["Touchpad 0"])
        self.command.assert_called_once_with(["/usr/bin/udevadm", "info", "--export-db"])
        self.assertFalse(self.state.exists())

    def test_filters_mouse_keyboard_touchscreen_and_other_seat(self):
        chosen = self.device()
        mouse = self.device(1, tags={"ID_INPUT_TOUCHPAD": "0", "ID_INPUT_MOUSE": "1"})
        keyboard = self.device(2, tags={"ID_INPUT_KEYBOARD": "1"})
        touchscreen = self.device(3, tags={"ID_INPUT_TOUCHSCREEN": "1"})
        other_seat = self.device(4, seat="seat1")
        result = touchpad.control("off")
        self.assertEqual(len(result["devices"]), 1)
        self.assert_enabled(chosen, False)
        for path in (mouse, keyboard, touchscreen, other_seat):
            self.assert_enabled(path, True)

    def test_default_seat_when_udev_id_seat_is_absent(self):
        self.device()
        self.records[0] = self.records[0].replace("\nE: ID_SEAT=seat0", "")
        self.assertEqual(touchpad.control("status")["seat"], "seat0")

    def test_devices_are_not_selected_by_their_display_name_or_bus(self):
        path = self.device()
        replacement = self.sysfs / "devices/pci0000:00/usb1/1-2/input/input3"
        replacement.parent.mkdir(parents=True)
        path.rename(replacement)
        (replacement / "event0").rename(replacement / "event3")
        (replacement / "name").write_text("USB precision input\n")
        self.records[0] = self.records[0].replace(
            "devices/platform/pad0/input/input0/event0", "devices/pci0000:00/usb1/1-2/input/input3/event3",
        )
        self.assertEqual(touchpad.control("off")["devices"][0]["name"], "USB precision input")
        self.assert_enabled(replacement, False)

    def test_no_device_is_an_error_not_disabled(self):
        with self.assertRaisesRegex(touchpad.TouchpadError, "No verified touchpad"):
            touchpad.control("status")

    def test_missing_inhibit_rejects_the_whole_operation_before_writing(self):
        first = self.device()
        second = self.device(1)
        (second / "inhibited").unlink()
        with self.assertRaisesRegex(touchpad.TouchpadError, "no kernel inhibit"):
            touchpad.control("off")
        self.assert_enabled(first, True)

    def test_invalid_inhibit_value_is_not_treated_as_disabled(self):
        path = self.device()
        (path / "inhibited").write_text("unknown\n")
        with self.assertRaisesRegex(touchpad.TouchpadError, "Invalid kernel inhibit"):
            touchpad.control("status")

    def test_off_on_and_idempotence(self):
        path = self.device()
        self.assertFalse(touchpad.control("off")["enabled"])
        self.assertFalse(json.loads((self.state / "touchpad_state").read_text())["enabled"])
        with mock.patch.object(touchpad, "write_enabled", wraps=touchpad.write_enabled) as write:
            touchpad.control("off")
            write.assert_not_called()
        self.assertTrue(touchpad.control("on")["enabled"])
        self.assert_enabled(path, True)

    def test_mixed_toggle_enables_all(self):
        a, b = self.device(enabled=False), self.device(1, enabled=True)
        self.assertTrue(touchpad.control("status")["mixed"])
        result = touchpad.control("toggle")
        self.assertTrue(result["enabled"])
        self.assertFalse(result["mixed"])
        self.assert_enabled(a, True)
        self.assert_enabled(b, True)

    def test_write_failure_after_a_mutation_rolls_back_all_attempted_devices(self):
        a, b = self.device(), self.device(1)
        real_write = touchpad.write_enabled

        def failure(device, enabled):
            real_write(device, enabled)
            if device["name"] == "Touchpad 1" and not enabled:
                raise OSError("device reported a write failure")

        with mock.patch.object(touchpad, "write_enabled", side_effect=failure):
            with self.assertRaisesRegex(touchpad.TouchpadError, "write failure") as error:
                touchpad.control("off")
        self.assertEqual(error.exception.rollback_errors, [])
        self.assert_enabled(a, True)
        self.assert_enabled(b, True)
        self.assertFalse((self.state / "touchpad_state").exists())

    def test_readback_failure_rolls_back_the_device_that_was_written(self):
        path = self.device()
        real_read = touchpad.read_enabled
        failed = False

        def read(device):
            nonlocal failed
            value = real_read(device)
            if not value and not failed:
                failed = True
                raise OSError("readback disappeared")
            return value

        with mock.patch.object(touchpad, "read_enabled", side_effect=read):
            with self.assertRaisesRegex(touchpad.TouchpadError, "readback disappeared"):
                touchpad.control("off")
        self.assert_enabled(path, True)

    def test_readback_mismatch_is_failure_and_rolls_back(self):
        path = self.device()
        with mock.patch.object(touchpad.os, "write", return_value=2):
            with self.assertRaisesRegex(touchpad.TouchpadError, "readback disagrees"):
                touchpad.control("off")
        self.assert_enabled(path, True)

    def test_final_snapshot_mismatch_restores_other_devices_without_saving(self):
        a, b = self.device(), self.device(1)
        state = self.saved(True)
        old_state = state.read_text()
        real_snapshot = touchpad.snapshot
        calls = 0

        def changed_snapshot(devices, seat):
            nonlocal calls
            calls += 1
            if calls == 2:
                (a / "inhibited").write_text("0\n")
            return real_snapshot(devices, seat)

        with mock.patch.object(touchpad, "snapshot", side_effect=changed_snapshot):
            with self.assertRaisesRegex(touchpad.TouchpadError, "final readback"):
                touchpad.control("off")
        self.assert_enabled(a, True)
        self.assert_enabled(b, True)
        self.assertEqual(state.read_text(), old_state)

    def test_persistence_failure_after_replace_restores_hardware_and_previous_state(self):
        path = self.device()
        previous = self.saved(True).read_text()
        real_save = touchpad._save_raw
        writes = 0

        def save(seat, raw):
            nonlocal writes
            writes += 1
            real_save(seat, raw)
            if writes == 1:
                raise OSError("directory fsync failed")

        with mock.patch.object(touchpad, "_save_raw", side_effect=save):
            with self.assertRaisesRegex(touchpad.TouchpadError, "fsync failed") as error:
                touchpad.control("off")
        self.assertEqual(error.exception.rollback_errors, [])
        self.assert_enabled(path, True)
        self.assertEqual((self.state / "touchpad_state").read_text(), previous)
        self.assertEqual(list(self.state.glob(".*")), [])

    def test_persistence_failure_with_no_previous_file_removes_new_state(self):
        path = self.device()
        real_save = touchpad._save_raw

        def save(seat, raw):
            real_save(seat, raw)
            if raw is not None:
                raise OSError("storage error")

        with mock.patch.object(touchpad, "_save_raw", side_effect=save):
            with self.assertRaises(touchpad.TouchpadError):
                touchpad.control("off")
        self.assert_enabled(path, True)
        self.assertFalse((self.state / "touchpad_state").exists())

    def test_rollback_failure_is_reported_not_hidden(self):
        self.device()
        with mock.patch.object(touchpad, "write_enabled", side_effect=OSError("removed")):
            with self.assertRaises(touchpad.TouchpadError) as error:
                touchpad.control("off")
        self.assertEqual(len(error.exception.rollback_errors), 1)
        self.assertIn("Touchpad 0", error.exception.rollback_errors[0])

    def test_saved_invalid_or_legacy_values_do_not_change_input(self):
        path = self.device()
        state = self.saved(True)
        for raw in ("0", "1", "[]", "broken", '{"version":true,"seat":"seat0","enabled":true}',
                    '{"version":1,"seat":"seat1","enabled":true}',
                    '{"version":1,"seat":"seat0","enabled":1}'):
            with self.subTest(raw=raw):
                state.write_text(raw)
                with self.assertRaises(touchpad.TouchpadError):
                    touchpad.control("off")
                self.assert_enabled(path, True)

    def test_restore_missing_is_a_noop_without_device_discovery(self):
        result = touchpad.control("restore")
        self.assertIsNone(result["enabled"])
        self.assertEqual(result["devices"], [])
        self.command.assert_not_called()
        self.assertFalse(self.state.exists())

    def test_restore_saved_state_does_not_rewrite_it(self):
        path = self.device()
        self.saved(False)
        with mock.patch.object(touchpad, "_save_raw") as save:
            self.assertFalse(touchpad.control("restore")["enabled"])
            save.assert_not_called()
        self.assert_enabled(path, False)

    def test_lifecycle_requires_direct_root_and_rejects_even_empty_pkexec_uid(self):
        path = self.device()
        self.saved(False)
        for action in ("restore-all", "release-all"):
            with self.subTest(action=action):
                with mock.patch.object(touchpad.os, "geteuid", return_value=1000):
                    with self.assertRaisesRegex(touchpad.TouchpadError, "require.*root"):
                        touchpad.control(action)
                for uid in ("1000", "0", ""):
                    with mock.patch.dict(os.environ, {"PKEXEC_UID": uid}):
                        with self.assertRaisesRegex(touchpad.TouchpadError, "direct root"):
                            touchpad.control(action)
        self.command.assert_not_called()
        self.assert_enabled(path, True)

    def test_lifecycle_with_no_saved_state_does_not_discover_or_change_devices(self):
        path = self.device(enabled=False)
        for action in ("restore-all", "release-all"):
            self.assertEqual(touchpad.control(action), {"results": [], "errors": []})
        self.command.assert_not_called()
        self.assert_enabled(path, False)

    def test_restore_all_restores_both_seats_under_one_lock(self):
        a, b = self.device(enabled=True), self.device(1, enabled=False, seat="seat1")
        state_a, state_b = self.saved(False), self.saved(True, "seat1")
        old_a, old_b = state_a.read_text(), state_b.read_text()
        with mock.patch.object(touchpad, "operation_lock", wraps=touchpad.operation_lock) as lock:
            result = touchpad.control("restore-all")
        lock.assert_called_once_with()
        self.assertEqual(result["errors"], [])
        self.assertEqual({r["seat"]: r["enabled"] for r in result["results"]}, {"seat0": False, "seat1": True})
        self.assert_enabled(a, False)
        self.assert_enabled(b, True)
        self.assertEqual((state_a.read_text(), state_b.read_text()), (old_a, old_b))

    def test_lifecycle_partial_failure_retains_missing_seat_state_and_exits_nonzero(self):
        path = self.device()
        self.saved(False)
        absent = self.saved(False, "seat1")
        old_absent = absent.read_text()
        output = io.StringIO()
        with redirect_stdout(output):
            code = touchpad.main(["restore-all"])
        result = json.loads(output.getvalue())
        self.assertEqual(code, 1)
        self.assertFalse(result["ok"])
        self.assertEqual([item["seat"] for item in result["results"]], ["seat0"])
        self.assertEqual([item["seat"] for item in result["errors"]], ["seat1"])
        self.assertIn("No verified touchpad", result["errors"][0]["error"])
        self.assertEqual(absent.read_text(), old_absent)
        self.assert_enabled(path, False)

    def test_release_all_only_enables_seats_saved_disabled_by_this_tool(self):
        external = self.device(enabled=False)
        owned = self.device(1, enabled=False, seat="seat1")
        untracked = self.device(2, enabled=False, seat="seat2")
        untouched_state = self.saved(True)
        owned_state = self.saved(False, "seat1")
        old_untouched = untouched_state.read_text()
        result = touchpad.control("release-all")
        self.assertEqual(result["errors"], [])
        self.assert_enabled(external, False)
        self.assert_enabled(owned, True)
        self.assert_enabled(untracked, False)
        self.assertEqual(untouched_state.read_text(), old_untouched)
        self.assertTrue(json.loads(owned_state.read_text())["enabled"])
        self.assertEqual(result["results"][0], {
            "seat": "seat0", "skipped": True, "reason": "No saved disabled state",
        })
        self.assertEqual(result["results"][1]["seat"], "seat1")
        self.command.assert_called_once_with(["/usr/bin/udevadm", "info", "--export-db"])

    def test_lifecycle_rejects_untrusted_or_invalid_state_and_continues_other_seats(self):
        a, b = self.device(), self.device(1, seat="seat1")
        original = self.saved(False)
        state_b = self.saved(False, "seat1")
        original.unlink()
        original.symlink_to(state_b)
        (self.state / "touchpad_state.bad-seat").write_text("{}")
        result = touchpad.control("restore-all")
        self.assertEqual([item["seat"] for item in result["results"]], ["seat1"])
        self.assertEqual(len(result["errors"]), 2)
        self.assert_enabled(a, True)
        self.assert_enabled(b, False)

    def test_two_concurrent_toggles_are_serialized_and_restore_initial_state(self):
        path = self.device()
        real_write = touchpad.write_enabled
        entered, release = threading.Event(), threading.Event()

        def slow_write(device, enabled):
            if not enabled:
                entered.set()
                if not release.wait(3):
                    raise AssertionError("second operation did not start")
            real_write(device, enabled)

        with mock.patch.object(touchpad, "write_enabled", side_effect=slow_write), ThreadPoolExecutor(2) as pool:
            first = pool.submit(touchpad.control, "toggle")
            self.assertTrue(entered.wait(2))
            second = pool.submit(touchpad.control, "toggle")
            time.sleep(0.05)
            release.set()
            self.assertFalse(first.result(3)["enabled"])
            self.assertTrue(second.result(3)["enabled"])
        self.assert_enabled(path, True)

    def test_nonroot_mutation_rejected_but_status_available(self):
        self.device()
        self.logind({"c1": "User=1000\nActive=yes\nRemote=no\nSeat=seat0\n"})
        with mock.patch.object(touchpad.os, "geteuid", return_value=1000), \
                mock.patch.object(touchpad.os, "getuid", return_value=1000):
            with self.assertRaisesRegex(touchpad.TouchpadError, "require.*root"):
                touchpad.control("off")
            self.assertTrue(touchpad.control("status")["enabled"])

    def test_nonroot_status_uses_its_logind_seat(self):
        self.device()
        self.device(1, enabled=False, seat="seat1")
        self.logind({"c1": "User=1000\nActive=yes\nRemote=no\nSeat=seat1\n"})
        with mock.patch.object(touchpad.os, "geteuid", return_value=1000), \
                mock.patch.object(touchpad.os, "getuid", return_value=1000):
            result = touchpad.control("status")
        self.assertEqual(result["seat"], "seat1")
        self.assertFalse(result["enabled"])

    def test_sysfs_symlink_or_replaced_device_is_rejected(self):
        path = self.device()
        discovered = touchpad.discover("seat0")[0]
        attribute = path / "inhibited"
        moved = path / "old"
        attribute.rename(moved)
        attribute.write_text("0\n")
        with self.assertRaisesRegex(touchpad.TouchpadError, "interface changed"):
            touchpad.write_enabled(discovered, False)
        attribute.unlink()
        attribute.symlink_to(moved)
        with self.assertRaises(OSError):
            touchpad.control("off")

    def test_symlink_in_input_directory_is_rejected(self):
        path = self.device()
        real = path.with_name("real")
        path.rename(real)
        path.symlink_to(real, target_is_directory=True)
        with self.assertRaises(OSError):
            touchpad.control("off")

    def test_state_symlink_hardlink_and_world_writable_files_are_rejected(self):
        path = self.device()
        state = self.saved(True)
        other = self.state / "other"
        state.rename(other)
        state.symlink_to(other)
        with self.assertRaises(OSError):
            touchpad.control("off")
        state.unlink()
        os.link(other, state)
        with self.assertRaisesRegex(touchpad.TouchpadError, "Hard-linked"):
            touchpad.control("off")
        state.unlink()
        other.rename(state)
        state.chmod(0o666)
        with self.assertRaisesRegex(touchpad.TouchpadError, "Untrusted"):
            touchpad.control("off")
        self.assert_enabled(path, True)

    def test_untrusted_state_directory_and_lock_link_are_rejected(self):
        path = self.device()
        self.saved(True)
        self.state.chmod(0o777)
        with self.assertRaisesRegex(touchpad.TouchpadError, "Untrusted"):
            touchpad.control("off")
        self.state.chmod(0o755)
        self.lock.unlink()
        self.lock.symlink_to(self.state / "touchpad_state")
        with self.assertRaises(OSError):
            touchpad.control("off")
        self.assert_enabled(path, True)

    def test_udev_parent_traversal_is_rejected(self):
        self.device()
        self.records[0] = self.records[0].replace("/devices/platform/", "/devices/../platform/")
        with self.assertRaisesRegex(touchpad.TouchpadError, "invalid udev"):
            touchpad.control("off")

    def test_directory_failures_close_every_opened_descriptor(self):
        unsafe = self.root / "unsafe"
        unsafe.mkdir(mode=0o777)
        unsafe.chmod(0o777)
        linked = self.root / "linked"
        linked.symlink_to(unsafe, target_is_directory=True)
        paths = (self.root / "missing/child", unsafe, linked)
        before = len(os.listdir("/proc/self/fd"))
        for _ in range(20):
            for path in paths:
                with self.assertRaises((OSError, touchpad.TouchpadError)):
                    with touchpad._directory(path):
                        self.fail("An untrusted directory was opened")
        self.assertEqual(len(os.listdir("/proc/self/fd")), before)

    def test_json_cli_reports_failure_and_does_not_accept_extra_arguments(self):
        output = io.StringIO()
        with redirect_stdout(output):
            code = touchpad.main(["off", "/tmp/arbitrary"])
        self.assertEqual(code, 1)
        self.assertFalse(json.loads(output.getvalue())["ok"])

    def logind(self, records):
        def command(argv):
            if argv[0] == "/usr/bin/udevadm":
                return self.run_command(argv)
            if argv[1] == "show-user":
                self.assertEqual(argv[2], "1000")
                return " ".join(records)
            self.assertEqual(argv[1], "show-session")
            return records[argv[2]]
        self.command.side_effect = command

    def test_pkexec_uses_active_local_seat_and_separate_state(self):
        a, b = self.device(), self.device(1, seat="seat1")
        self.logind({"c1": "User=1000\nActive=yes\nRemote=no\nSeat=seat1\n"})
        with mock.patch.dict(os.environ, {"PKEXEC_UID": "1000", "XDG_SEAT": "seat0"}):
            result = touchpad.control("off")
        self.assertEqual(result["seat"], "seat1")
        self.assert_enabled(a, True)
        self.assert_enabled(b, False)
        self.assertTrue((self.state / "touchpad_state.seat1").exists())
        self.assertFalse((self.state / "touchpad_state").exists())

    def test_pkexec_rejects_remote_inactive_wrong_user_missing_and_ambiguous_seats(self):
        path = self.device()
        sessions = [
            {},
            {"c1": "User=1000\nActive=yes\nRemote=yes\nSeat=seat0\n"},
            {"c1": "User=1000\nActive=no\nRemote=no\nSeat=seat0\n"},
            {"c1": "User=1001\nActive=yes\nRemote=no\nSeat=seat0\n"},
            {"c1": "User=1000\nActive=yes\nRemote=no\nSeat=\n"},
            {"c1": "User=1000\nActive=yes\nRemote=no\nSeat=seat0\n",
             "c2": "User=1000\nActive=yes\nRemote=no\nSeat=seat1\n"},
        ]
        for records in sessions:
            with self.subTest(records=records), mock.patch.dict(os.environ, {"PKEXEC_UID": "1000"}):
                self.logind(records)
                with self.assertRaises(touchpad.TouchpadError):
                    touchpad.control("off")
                self.assert_enabled(path, True)

    def test_seatless_manager_does_not_mask_the_real_active_local_session(self):
        path = self.device()
        self.logind({
            "3": "User=1000\nActive=yes\nRemote=no\nSeat=\n",
            "1": "User=1000\nActive=yes\nRemote=no\nSeat=seat0\n",
        })
        with mock.patch.dict(os.environ, {"PKEXEC_UID": "1000"}):
            result = touchpad.control("off")
        self.assertEqual(result["seat"], "seat0")
        self.assert_enabled(path, False)

    def test_seatless_only_session_has_no_authorized_input_seat(self):
        path = self.device()
        self.logind({"3": "User=1000\nActive=yes\nRemote=no\nSeat=\n"})
        with mock.patch.dict(os.environ, {"PKEXEC_UID": "1000"}):
            with self.assertRaisesRegex(touchpad.TouchpadError, "exactly one active local seat"):
                touchpad.control("off")
        self.assert_enabled(path, True)

    def test_nonempty_invalid_seat_is_rejected_even_with_another_valid_session(self):
        path = self.device()
        self.logind({
            "3": "User=1000\nActive=yes\nRemote=no\nSeat=../seat0\n",
            "1": "User=1000\nActive=yes\nRemote=no\nSeat=seat0\n",
        })
        with mock.patch.dict(os.environ, {"PKEXEC_UID": "1000"}):
            with self.assertRaisesRegex(touchpad.TouchpadError, "invalid local seat"):
                touchpad.control("off")
        self.assert_enabled(path, True)

    def test_pkexec_malformed_uid_fails_before_spawning(self):
        with mock.patch.dict(os.environ, {"PKEXEC_UID": "1000 --help"}):
            with self.assertRaisesRegex(touchpad.TouchpadError, "Invalid PKEXEC_UID"):
                touchpad.control("off")
        self.command.assert_not_called()


if __name__ == "__main__":
    unittest.main()
