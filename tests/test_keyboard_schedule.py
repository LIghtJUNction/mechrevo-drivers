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
        with mock.patch.object(Path, 'write_text', side_effect=PermissionError('denied')):
            self.schedule.tick()
        self.assertIn('denied', self.schedule.last_error)
        with mock.patch.object(Path, 'write_text', return_value=0):
            self.schedule.tick()
        self.assertIn('回读不匹配', self.schedule.last_error)

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
