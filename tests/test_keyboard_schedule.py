"""Exercise the installed Python CLI/daemon entry with fake LED and lid files."""
from datetime import datetime
import io
from pathlib import Path
import tempfile
import unittest
from contextlib import ExitStack, redirect_stderr
from unittest import mock

from test_mechrevo_hotkey import load_control_module

c = load_control_module()


class ScheduleTests(unittest.TestCase):
    def setUp(self):
        self.stack = ExitStack()
        self.addCleanup(self.stack.close)
        self.root = Path(self.stack.enter_context(tempfile.TemporaryDirectory()))
        self.led = self.root / 'led'
        self.led.mkdir()
        (self.led / 'brightness').write_text('3\n')
        (self.led / 'max_brightness').write_text('4\n')
        self.lid = self.root / 'state'
        self.lid.write_text('state: open\n')
        self.stack.enter_context(mock.patch.object(c, 'RGB_LEDS_DIR', self.led))
        self.stack.enter_context(mock.patch.object(c, '_dmi_text', side_effect={
            'sys_vendor': 'MECHREVO', 'board_name': 'GM6BG0Q',
            'product_name': 'Jiaolong16K Series GM6BG0Q'}.get))
        self.stack.enter_context(mock.patch.object(Path, 'glob', return_value=[self.lid]))
        self.clock = self.stack.enter_context(mock.patch.object(c, 'datetime'))
        self.clock.now.return_value.astimezone.return_value = datetime(2026, 11, 25, 10)
        self.stack.enter_context(redirect_stderr(io.StringIO()))
        self.schedule = c.KeyboardSchedule()
        self.addCleanup(lambda: self.schedule.owner and self.schedule.owner.close())

    def tick(self, hhmm, closed=False):
        hour, minute = map(int, hhmm.split(':'))
        self.clock.now.return_value.astimezone.return_value = datetime(2026, 11, 25, hour, minute)
        self.lid.write_text('state: closed\n' if closed else 'state: open\n')
        self.schedule.tick()
        return (self.led / 'brightness').read_text().strip()

    def test_boundaries_midnight_and_open_lid(self):
        for time, expected in [('06:59', '1'), ('07:00', '0'), ('18:59', '0'),
                               ('19:00', '1'), ('23:59', '1'), ('00:00', '1')]:
            self.assertEqual(self.tick(time), expected, time)

    def test_lid_across_both_boundaries_and_reopen(self):
        for time, closed, expected in [('23:00', False, '1'), ('06:59', True, '0'),
            ('07:00', True, '0'), ('08:00', False, '0'), ('18:59', True, '0'),
            ('19:00', True, '0'), ('20:00', False, '1')]:
            self.assertEqual(self.tick(time, closed), expected)

    def test_wall_clock_jumps_are_recomputed(self):
        for time, expected in [('22:00', '1'), ('12:00', '0'), ('22:00', '1'), ('07:00', '0')]:
            self.assertEqual(self.tick(time), expected)

    def test_missing_lid_interfaces(self):
        with mock.patch.object(Path, 'glob', return_value=[]):
            self.schedule.tick()
        self.assertIn('合盖状态', self.schedule.last_error)
        self.assertEqual((self.led / 'brightness').read_text().strip(), '3')

    def test_cross_midnight_day_interval(self):
        self.schedule.day_start, self.schedule.night_start = 1200, 360
        for time, expected in [('19:59', '1'), ('20:00', '0'), ('00:00', '0'),
                                ('05:59', '0'), ('06:00', '1')]:
            self.assertEqual(self.tick(time), expected)

    def test_restart_recomputes_instead_of_restoring(self):
        self.assertEqual(self.tick('22:00'), '1')
        self.schedule.owner.close()
        self.schedule = c.KeyboardSchedule(night_brightness=2)
        self.assertEqual(self.tick('10:00'), '0')
        self.assertEqual(self.tick('21:00', True), '0')
        self.assertEqual(self.tick('21:00'), '2')

    def test_disabled_unknown_and_unsupported_range_do_not_write(self):
        self.schedule.enabled = False
        self.assertEqual(self.tick('10:00'), '3')
        self.schedule.enabled = True
        with mock.patch.object(c, '_dmi_text', return_value='unknown'):
            self.assertEqual(self.tick('10:00'), '3')
        (self.led / 'max_brightness').write_text('255')
        self.assertEqual(self.tick('10:00'), '3')

    def test_missing_or_bad_lid_never_lights(self):
        for text in ['state: unknown', '']:
            self.lid.write_text(text)
            (self.led / 'brightness').write_text('0')
            self.schedule.tick()
            self.assertEqual((self.led / 'brightness').read_text(), '0')
        self.lid.unlink()
        self.schedule.tick()
        self.assertIsNotNone(self.schedule.last_error)

    def test_close_during_lighting_check(self):
        with mock.patch.object(self.schedule, 'lid_closed', side_effect=[False, True]):
            self.assertEqual(self.tick('22:00'), '0')

    def test_single_owner(self):
        self.assertEqual(self.tick('22:00'), '1')
        other = c.KeyboardSchedule()
        other.tick()
        self.assertIn('另一个', other.last_error)
        self.assertIsNone(other.owner)

    def test_write_failure_and_readback_mismatch_are_visible(self):
        original_open = Path.open
        def broken_open(path, *args, **kwargs):
            endpoint = original_open(path, *args, **kwargs)
            if path != self.led / 'brightness':
                return endpoint
            proxy = mock.MagicMock(wraps=endpoint)
            proxy.__enter__.return_value = proxy
            proxy.__exit__.side_effect = lambda *a: endpoint.close()
            proxy.write.side_effect = PermissionError('denied')
            return proxy
        with mock.patch.object(Path, 'open', autospec=True, side_effect=broken_open):
            self.schedule.tick()
        self.assertIn('denied', self.schedule.last_error)
        self.assertIsNone(self.schedule.owner)
        with mock.patch.object(c.KeyboardSchedule, 'read_brightness', side_effect=[3, 3]):
            self.schedule.tick()
        self.assertIn('回读不匹配', self.schedule.last_error)
        self.assertIsNone(self.schedule.owner)

    def test_cli_default_custom_disabled_and_invalid(self):
        for options, expected in [([], (True, 420, 1140, 1)),
            (['--day-start', '20:00', '--night-start', '06:00', '--night-brightness', '3'],
             (True, 1200, 360, 3)), (['--no-keyboard-schedule'], (False, 420, 1140, 1))]:
            with mock.patch('sys.argv', ['mechrevo-control', 'daemon'] + options), \
                 mock.patch.object(c, 'run_daemon') as run:
                self.assertEqual(c.main(), 0)
                s = run.call_args.kwargs['keyboard_schedule']
                self.assertEqual((s.enabled, s.day_start, s.night_start, s.night_brightness), expected)
        with mock.patch('sys.argv', ['mechrevo-control', 'daemon', '--day-start', '19:00']), \
             mock.patch.object(c, 'run_daemon') as run:
            self.assertEqual(c.main(), 1)
            run.assert_not_called()
        for value in ['7:00', '24:00', '12:60', '-1:00']:
            with self.assertRaises(c.argparse.ArgumentTypeError):
                c.clock_minute(value)

    def drive_daemon(self, schedule, ec_only, steps, helper_value=3):
        """Run main/daemon on a real input pipe with deterministic select steps.

        Stopping parks the instance (including its real ownership socket), so
        another daemon can be started in either order without threads/sleeps.
        """
        import os, select, struct
        class Park(BaseException):
            pass
        reader, writer = os.pipe()
        steps = iter(steps)
        # An already parked owner resumes at select, not at daemon startup.
        skip_startup_tick = schedule.owner is not None
        original_tick = schedule.tick
        def resumed_tick():
            nonlocal skip_startup_tick
            if skip_startup_tick:
                skip_startup_tick = False
            else:
                original_tick()
        def ready(readers, *args):
            try:
                step = next(steps)
            except StopIteration:
                raise Park()
            code = step() if callable(step) else step
            if code is None:
                return [], [], []
            os.write(writer, struct.pack('qqHHi', 0, 0, 1, code, 1))
            return readers, [], []
        def helper(argv):
            self.assertEqual(argv[0], 'omarchy-brightness-keyboard')
            if helper_value is not None:
                (self.led / 'brightness').write_text(f'{helper_value}\n')
            return ''
        try:
            with mock.patch('sys.argv', ['mechrevo-control', 'daemon'] + (['--ec-only'] if ec_only else [])), \
                 mock.patch.object(Path, 'exists', return_value=True), \
                 mock.patch.object(c, 'open', side_effect=lambda *a, **k: os.fdopen(os.dup(reader), 'rb', buffering=0), create=True), \
                 mock.patch.object(c, 'KeyboardSchedule', return_value=schedule), \
                 mock.patch.object(schedule, 'tick', side_effect=resumed_tick), \
                 mock.patch.object(c, 'read_hardware_mode', return_value=0xa0), \
                 mock.patch.object(c, 'read_uefi_power_mode', return_value=1), \
                 mock.patch.object(c, 'sync_oem_profiles'), \
                 mock.patch.object(c, 'sync_mode_led_from_profile', return_value='mechrevo_gaming'), \
                 mock.patch.object(c, 'get_current_mode_info', return_value=('', 'mechrevo_gaming')), \
                 mock.patch.object(c.shutil, 'which', return_value='/fake/helper'), \
                 mock.patch.object(c, 'run_cmd', side_effect=helper) as command, \
                 mock.patch.object(select, 'select', side_effect=ready), \
                 self.assertRaises(Park):
                c.main()
            return command.call_args_list
        finally:
            os.close(reader)
            os.close(writer)

    def set_clock(self, hhmm, day=25):
        hour, minute = map(int, hhmm.split(':'))
        self.clock.now.return_value.astimezone.return_value = datetime(2026, 11, day, hour, minute)

    def assert_level(self, expected):
        self.assertEqual((self.led / 'brightness').read_text().strip(), str(expected))

    def test_fn_up_down_cycle_confirmed_through_cli_event_path(self):
        for code, action, value in [(228, 'cycle', 0), (229, 'down', 0), (230, 'up', 4)]:
            with self.subTest(code=code):
                self.schedule.release()
                self.set_clock('22:00')
                calls = self.drive_daemon(self.schedule, False, [code, None, None], value)
                self.assertEqual(calls, [mock.call(['omarchy-brightness-keyboard', action])])
                self.assertEqual(self.schedule.override, value)
                self.assert_level(value)

    def test_fn_override_lid_and_day_boundaries(self):
        self.set_clock('23:59')
        self.drive_daemon(self.schedule, False, [230, None], 3)
        self.assertEqual(self.schedule.override, 3)
        self.set_clock('00:00', 26)
        self.drive_daemon(self.schedule, False, [None])
        self.assert_level(3)  # same night across midnight
        self.set_clock('07:00', 26)
        self.drive_daemon(self.schedule, False, [230, None], 4)
        self.assert_level(0)
        self.assertIsNone(self.schedule.override)
        self.set_clock('19:00', 26)
        self.drive_daemon(self.schedule, False, [None])
        self.assert_level(1)
        self.drive_daemon(self.schedule, False, [230, None], 3)
        self.lid.write_text('state: closed')
        self.drive_daemon(self.schedule, False, [230, None], 4)
        self.assert_level(0)
        self.assertIsNone(self.schedule.override)
        self.lid.write_text('state: open')
        self.drive_daemon(self.schedule, False, [None])
        self.assert_level(1)

    def test_fn_override_restart_and_unobserved_day_clear(self):
        self.set_clock('22:00')
        self.drive_daemon(self.schedule, False, [230, None], 3)
        self.set_clock('22:00', 26)  # jump past an entire day
        self.drive_daemon(self.schedule, False, [None])
        self.assert_level(1)
        self.drive_daemon(self.schedule, False, [230, None], 3)
        self.schedule.release()
        self.schedule = c.KeyboardSchedule()
        self.drive_daemon(self.schedule, False, [None])
        self.assert_level(1)

    def test_fn_event_without_changed_readback_does_not_override(self):
        self.set_clock('22:00')
        self.drive_daemon(self.schedule, False, [230, None], None)
        self.assertIsNone(self.schedule.override)
        self.assert_level(1)
        self.schedule.fn_pending = (1, 0.0)  # helper confirmation window expires
        self.drive_daemon(self.schedule, False, [None])
        self.assertIsNone(self.schedule.fn_pending)
        self.assertIsNone(self.schedule.override)

    def test_disabled_fn_path_remains_manual_in_day_and_closed_lid(self):
        self.schedule.enabled = False
        self.lid.write_text('state: closed')
        self.set_clock('10:00')
        calls = self.drive_daemon(self.schedule, False, [230, None], 4)
        self.assertEqual(calls, [mock.call(['omarchy-brightness-keyboard', 'up'])])
        self.assert_level(4)
        self.assertIsNone(self.schedule.owner)
        self.assertIsNone(self.schedule.fn_pending)

    def test_both_daemon_start_orders_and_fn_owner_observation(self):
        for first_ec in (True, False):
            with self.subTest(first_ec=first_ec):
                self.schedule.release()
                peer = c.KeyboardSchedule()
                self.addCleanup(peer.release)
                self.set_clock('22:00')
                self.drive_daemon(self.schedule, first_ec, [None])
                self.drive_daemon(peer, not first_ec, [None])
                self.assertIsNotNone(self.schedule.owner)
                self.assertIsNone(peer.owner)
                if first_ec:
                    # EC owner receives the same physical key, before the user
                    # helper finishes. It must not run the helper itself.
                    calls = self.drive_daemon(self.schedule, True, [230])
                    self.assertEqual(calls, [])
                    self.drive_daemon(peer, False, [230, None], 3)
                    self.drive_daemon(self.schedule, True, [None])
                else:
                    self.drive_daemon(self.schedule, False, [230, None], 3)
                    self.drive_daemon(peer, True, [230, None])
                self.assert_level(3)
                self.assertEqual(self.schedule.override, 3)
                peer.release()

    def test_owner_observes_fn_after_user_helper_already_finished(self):
        self.set_clock('22:00')
        self.drive_daemon(self.schedule, True, [None])
        peer = c.KeyboardSchedule()
        self.addCleanup(peer.release)
        self.drive_daemon(peer, False, [230, None], 4)
        self.drive_daemon(self.schedule, True, [230, None])
        self.assert_level(4)
        self.assertEqual(self.schedule.override, 4)

    def test_readonly_first_at_target_never_claims_ownership_both_orders(self):
        original_open = Path.open
        def denied_open(path, mode='r', *args, **kwargs):
            if path == self.led / 'brightness' and mode == 'r+':
                raise PermissionError('read-only endpoint')
            return original_open(path, mode, *args, **kwargs)
        for first_ec in (True, False):
            for initial in (0, 3):  # already at daytime target vs write needed
                with self.subTest(first_ec=first_ec, initial=initial):
                    self.schedule.release()
                    (self.led / 'brightness').write_text(str(initial))
                    self.set_clock('10:00')
                    with mock.patch.object(Path, 'open', autospec=True, side_effect=denied_open):
                        self.drive_daemon(self.schedule, first_ec, [None])
                    self.assertIsNone(self.schedule.owner)
                    peer = c.KeyboardSchedule()
                    self.addCleanup(peer.release)
                    self.drive_daemon(peer, not first_ec, [None])
                    self.assertIsNotNone(peer.owner)
                    self.assert_level(0)
                    peer.release()

    def test_first_owner_loses_access_then_peer_takes_over_both_orders(self):
        original_open = Path.open
        for first_ec in (True, False):
            for failure in ('open', 'read', 'invalid', 'write', 'flush', 'readback'):
                with self.subTest(first_ec=first_ec, failure=failure):
                    self.schedule.release()
                    self.set_clock('22:00')
                    self.drive_daemon(self.schedule, first_ec, [None])
                    self.assertIsNotNone(self.schedule.owner)
                    peer = c.KeyboardSchedule()
                    self.addCleanup(peer.release)
                    self.drive_daemon(peer, not first_ec, [None])
                    self.assertIsNone(peer.owner)
                    # Match current target for open/read failures, require a
                    # write for write/readback failures.
                    if failure in ('write', 'flush', 'readback'):
                        self.set_clock('10:00')
                    def failed_open(path, mode='r', *args, **kwargs):
                        if path != self.led / 'brightness' or mode != 'r+':
                            return original_open(path, mode, *args, **kwargs)
                        if failure == 'open':
                            raise PermissionError('lost access')
                        endpoint = original_open(path, mode, *args, **kwargs)
                        proxy = mock.MagicMock(wraps=endpoint)
                        proxy.__enter__.return_value = proxy
                        proxy.__exit__.side_effect = lambda *a: endpoint.close()
                        if failure == 'read':
                            proxy.read.side_effect = OSError('read failed')
                        elif failure == 'invalid':
                            proxy.read.return_value = '255'
                        elif failure == 'write':
                            proxy.write.side_effect = OSError('write failed')
                        elif failure == 'flush':
                            proxy.flush.side_effect = OSError('flush failed')
                        else:
                            proxy.read.side_effect = ['1', '1']
                        return proxy
                    with mock.patch.object(Path, 'open', autospec=True, side_effect=failed_open):
                        self.drive_daemon(self.schedule, first_ec, [None])
                    self.assertIsNone(self.schedule.owner)
                    self.assertIsNone(self.schedule.override)
                    self.assertIsNone(self.schedule.fn_pending)
                    self.assertIsNotNone(self.schedule.last_error)
                    self.drive_daemon(peer, not first_ec, [None])
                    self.assertIsNotNone(peer.owner)
                    self.assert_level(1 if failure in ('open', 'read', 'invalid') else 0)
                    peer.release()

    def test_at_target_checks_same_endpoint_write_access_without_writes(self):
        self.set_clock('22:00')
        (self.led / 'brightness').write_text('1\n')
        original_open = Path.open
        writes = []
        def observed_open(path, mode='r', *args, **kwargs):
            endpoint = original_open(path, mode, *args, **kwargs)
            if path != self.led / 'brightness' or mode != 'r+':
                return endpoint
            proxy = mock.MagicMock(wraps=endpoint)
            proxy.__enter__.return_value = proxy
            proxy.__exit__.side_effect = lambda *a: endpoint.close()
            proxy.write.side_effect = lambda value: writes.append(value)
            return proxy
        with mock.patch.object(Path, 'open', autospec=True, side_effect=observed_open) as opened:
            self.drive_daemon(self.schedule, True, [None, None])
        self.assertIsNotNone(self.schedule.owner)
        self.assertEqual(writes, [])
        self.assertEqual(sum(call.args[:2] == (self.led / 'brightness', 'r+')
                             for call in opened.call_args_list), 3)

    def test_real_cli_daemon_ticks_on_start_and_input(self):
        class Done(BaseException):
            pass
        self.clock.now.return_value.astimezone.return_value = datetime(2026, 11, 25, 10)
        # A real pipe carries an ignored input event; the next loop must still
        # evaluate the policy. This enters main -> run_daemon -> tick -> sysfs.
        import os, select, struct
        reader, writer = os.pipe()
        self.addCleanup(os.close, reader)
        self.addCleanup(os.close, writer)
        os.write(writer, struct.pack('qqHHi', 0, 0, 1, 10, 1))
        def ready(*args):
            if (self.led / 'brightness').read_text().strip() == '0':
                self.lid.write_text('state: closed')
            if ready.calls:
                raise Done()
            ready.calls += 1
            return [args[0][0]], [], []
        ready.calls = 0
        with mock.patch('sys.argv', ['mechrevo-control', 'daemon', '--ec-only']), \
             mock.patch.object(Path, 'exists', return_value=True), \
             mock.patch.object(c, 'open', side_effect=lambda *a, **k: os.fdopen(os.dup(reader), 'rb', buffering=0), create=True), \
             mock.patch.object(c, 'read_hardware_mode', return_value=0xa0), \
             mock.patch.object(c, 'read_uefi_power_mode', return_value=1), \
             mock.patch.object(select, 'select', side_effect=ready), \
             mock.patch.object(c, 'KeyboardSchedule', return_value=self.schedule), \
             self.assertRaises(Done):
            c.main()
        self.assertEqual((self.led / 'brightness').read_text().strip(), '0')


if __name__ == '__main__':
    unittest.main()
