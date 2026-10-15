# SPDX-License-Identifier: GPL-2.0-or-later
"""Real PL2 trial entry points against isolated sysfs and service fixtures."""
import copy
import json
import os
from pathlib import Path
import signal
import sys
import tempfile
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts"))
sys.path.insert(0, str(ROOT / "tests"))
import peak_limit_trial as PEAK
import test_mechrevo_power as FIXTURE


class Sensors:
    def __init__(self, host):
        self.host = host
        self.callback = lambda: None
        self.gpu = ["00000000:01:00.0"]
        self.entries = []
        self.closed = False

    def boot_id(self):
        return self.host.env.boot_id().replace("-", "")

    def journal(self, boot, cursor=None):
        if cursor is None:
            return [{"__CURSOR": "start", "MESSAGE": "baseline", "_BOOT_ID": boot}]
        entries, self.entries = self.entries, []
        return entries

    def gpu_health(self):
        return self.gpu

    def open_sensors(self):
        pass

    def sample(self):
        self.callback()
        return {"package_mc": 65000, "ec": {"fan1_raw": 90}, "rapl": {
            z.rsplit("/", 1)[-1]: {
                "constraint_0_power_limit_uw": int(self.host.read(z + "/constraint_0_power_limit_uw")),
                "constraint_1_power_limit_uw": int(self.host.read(z + PEAK.LIMIT))}
            for z in FIXTURE.POWER.ZONES}}

    def close(self):
        self.closed = True


class PeakLimitTests(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory(prefix="peak-trial-")
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)
        self.host = FIXTURE.FakeHost(self, self.root)
        self.host.controller.set_limit(55)
        self.addCleanup(mock.patch.stopall)
        mock.patch.object(PEAK, "power", FIXTURE.POWER).start()
        self.system = Sensors(self.host)
        self.tick = 0

        def sleep(seconds):
            self.tick += seconds

        self.trial = PEAK.PeakTrial(self.host.controller, self.system,
                                   clock=lambda: self.tick, sleeper=sleep)
        self.saved = []
        self.writes = []
        self.original_write = self.host.env.write_control

        def write(path, value):
            self.writes.append((path, value))
            self.original_write(path, value)

        self.host.env.write_control = write
        self.before = self.controls()

    def controls(self):
        return {str(p.relative_to(self.root)): p.read_bytes()
                for p in self.root.glob("sys/class/powercap/*/*")}

    def save(self, value):
        self.saved.append(copy.deepcopy(value))

    def run_trial(self):
        return self.trial.run(55, 2, self.save)

    def receipt(self, value):
        path = self.root / "receipt.json"
        path.write_text(json.dumps(value))
        path.chmod(0o600)
        return path

    def dead_owner(self, value):
        value = copy.deepcopy(value)
        value["owner"]["start_ticks"] += 1  # Same PID, distinct process lifetime.
        return value

    def test_success_saves_intent_before_each_write_and_restores_only_pl2(self):
        result = self.run_trial()
        self.assertTrue(result["valid"])
        self.assertTrue(result["restored"])
        self.assertEqual(self.before, self.controls())
        self.assertEqual(len(self.writes), 4)
        self.assertTrue(all(p.endswith(PEAK.LIMIT) for p, _ in self.writes))
        self.assertTrue(self.saved[0]["zones"])
        first = FIXTURE.POWER.ZONES[0]
        self.assertFalse(self.saved[0]["zones"][first]["attempted"])
        self.assertTrue(any(v["zones"][first]["attempted"] for v in self.saved))
        self.assertTrue(self.system.closed)

    def test_first_write_failure_restores_and_does_not_touch_second(self):
        def failure(path, value):
            raise OSError("injected write failure")
        self.host.env.write_control = failure
        result = self.run_trial()
        self.assertFalse(result["valid"])
        self.assertTrue(result["restored"])
        self.assertEqual(self.before, self.controls())

    def test_second_write_failure_restores_first(self):
        second = FIXTURE.POWER.ZONES[1]
        def failure(path, value):
            if path == second + PEAK.LIMIT and value == 55_000_000:
                raise OSError("second interface failure")
            self.original_write(path, value)
        self.host.env.write_control = failure
        result = self.run_trial()
        self.assertFalse(result["valid"])
        self.assertTrue(result["restored"])
        self.assertEqual(self.before, self.controls())

    def test_restoration_write_failure_retains_pending_zone_and_original(self):
        second = FIXTURE.POWER.ZONES[1]
        def failure(path, value):
            if path == second + PEAK.LIMIT and value == 140_000_000:
                raise OSError("restore write failed")
            self.original_write(path, value)
        self.host.env.write_control = failure
        result = self.run_trial()
        self.assertFalse(result["valid"])
        self.assertFalse(result["restored"])
        self.assertEqual(int(self.host.read(second + PEAK.LIMIT)), 55_000_000)
        self.assertEqual(result["zones"][second]["original_uw"], 140_000_000)
        self.assertTrue(result["zones"][second]["restore_attempted"])

    def test_checkpoint_failure_after_write_still_recovers_hardware(self):
        def save(value):
            if any(int(self.host.read(z + PEAK.LIMIT)) == 55_000_000 for z in FIXTURE.POWER.ZONES):
                raise OSError("output became unwritable")
            self.save(value)
        result = self.trial.run(55, 2, save)
        self.assertFalse(result["valid"])
        self.assertTrue(result["restored"])
        self.assertEqual(self.before, self.controls())

    def test_readback_mismatch_is_failure_and_preserves_external_value(self):
        first = FIXTURE.POWER.ZONES[0]
        def clamp(path, value):
            self.original_write(path, 60_000_000 if value == 55_000_000 else value)
        self.host.env.write_control = clamp
        result = self.run_trial()
        self.assertFalse(result["valid"])
        self.assertFalse(result["restored"])
        self.assertEqual(int(self.host.read(first + PEAK.LIMIT)), 60_000_000)

    def test_external_update_is_not_overwritten_but_other_zone_recovers(self):
        first, second = FIXTURE.POWER.ZONES
        self.system.callback = lambda: self.original_write(first + PEAK.LIMIT, 45_000_000)
        result = self.run_trial()
        self.assertFalse(result["restored"])
        self.assertFalse(result["valid"])
        self.assertEqual(int(self.host.read(first + PEAK.LIMIT)), 45_000_000)
        self.assertEqual(int(self.host.read(second + PEAK.LIMIT)), 140_000_000)

    def test_keyboard_interrupt_restores(self):
        def stop():
            raise KeyboardInterrupt("signal fixture")
        self.system.callback = stop
        result = self.run_trial()
        self.assertFalse(result["valid"])
        self.assertTrue(result["restored"])
        self.assertEqual(self.before, self.controls())

    def test_gpu_fault_defers_upward_restoration_then_explicit_recovery_works(self):
        self.system.callback = lambda: setattr(self.system, "gpu", [])
        result = self.run_trial()
        self.assertFalse(result["restored"])
        self.assertEqual(len(self.writes), 2)
        source = self.receipt(self.dead_owner(result))
        original = source.read_bytes()
        self.system.gpu = ["00000000:01:00.0"]
        restored = self.trial.recover(source, self.save)
        self.assertTrue(restored["valid"])
        self.assertTrue(restored["restored"])
        self.assertEqual(source.read_bytes(), original)
        self.assertEqual(self.before, self.controls())

    def test_new_kernel_fault_stops_and_defers_upward_restoration(self):
        def fault():
            self.system.entries = [{"__CURSOR": "fault", "MESSAGE": "NVRM: Xid (PCI:0000:01:00): 79"}]
        self.system.callback = fault
        result = self.run_trial()
        self.assertFalse(result["valid"])
        self.assertTrue(result["faults"])
        # Health may recover on the next probe, but a fault seen during this
        # trial must still inhibit any upward write until explicit recovery.
        self.assertFalse(result["restored"])
        self.assertEqual(len(self.writes), 2)

    def test_one_transient_nvml_failure_stays_inhibited_until_explicit_recovery(self):
        original = self.system.gpu_health
        def fail_once():
            self.system.gpu_health = original
            raise OSError("NVML transient failure")
        self.system.callback = lambda: setattr(self.system, "gpu_health", fail_once)
        result = self.run_trial()
        self.assertFalse(result["restored"])
        self.assertTrue(result["recovery_inhibited"])
        self.assertEqual(len(self.writes), 2)

    def test_sample_with_pl1_overshoot_cannot_report_valid_trial(self):
        original = self.system.sample
        def bad_sample():
            sample = original()
            sample["rapl"]["intel-rapl:0"]["constraint_0_power_limit_uw"] = 60_000_000
            return sample
        self.system.sample = bad_sample
        result = self.run_trial()
        self.assertFalse(result["valid"])
        self.assertTrue(result["restored"])

    def test_configuration_change_defers_recovery(self):
        def change():
            path = self.host.path(FIXTURE.POWER.STATE)
            path.write_bytes(path.read_bytes() + b" ")
        self.system.callback = change
        result = self.run_trial()
        self.assertFalse(result["restored"])
        self.assertEqual(len(self.writes), 2)

    def test_wrong_constraint_name_rejects_before_any_write(self):
        self.host.write(FIXTURE.POWER.ZONES[1] + "/constraint_1_name", "long_term")
        result = self.run_trial()
        self.assertFalse(result["valid"])
        self.assertEqual(self.writes, [])

    def test_limits_duration_and_no_raise_boundary(self):
        for watts, seconds in ((34, 2), (56, 2), (True, 2), (55, float("nan")), (55, 301), (45, 2)):
            with self.subTest(watts=watts, seconds=seconds):
                result = self.trial.run(watts, seconds, self.save)
                self.assertFalse(result["valid"])
        self.host.write(FIXTURE.POWER.ZONES[1] + PEAK.LIMIT, "55000000")
        self.assertFalse(self.run_trial()["valid"])
        self.assertEqual(self.writes, [])

    def test_live_owner_and_different_boot_cannot_recover(self):
        result = self.run_trial()
        with self.assertRaisesRegex(FIXTURE.POWER.PowerError, "still running"):
            self.trial.recover(self.receipt(result), self.save)
        result["owner"]["boot_id"] = "another-boot"
        with self.assertRaisesRegex(FIXTURE.POWER.PowerError, "another boot"):
            self.trial.recover(self.receipt(result), self.save)

    def test_recover_is_idempotent_and_preserves_source(self):
        source = self.receipt(self.dead_owner(self.run_trial()))
        previous = source.read_bytes()
        self.writes.clear()
        self.assertTrue(self.trial.recover(source, self.save)["restored"])
        self.assertEqual(self.writes, [])
        self.assertEqual(source.read_bytes(), previous)

    def test_corrupt_recovery_cannot_write_or_modify_source(self):
        base = self.dead_owner(self.run_trial())
        self.writes.clear()
        mutations = [lambda x: x.update(target_uw=True), lambda x: x.update(zones={}),
                     lambda x: x["owner"].update(pid=True),
                     lambda x: x["zones"][FIXTURE.POWER.ZONES[0]].update(original_uw=999999999999),
                     lambda x: x["zones"][FIXTURE.POWER.ZONES[0]].update(attempted="false"),
                     lambda x: x["frozen"].update(state_sha256="bad")]
        for mutate in mutations:
            value = copy.deepcopy(base)
            mutate(value)
            source = self.receipt(value)
            original = source.read_bytes()
            with self.assertRaises((FIXTURE.POWER.PowerError, ValueError, KeyError, TypeError)):
                self.trial.recover(source, self.save)
            self.assertEqual(source.read_bytes(), original)
        self.assertEqual(self.writes, [])

    def test_global_lock_competition_prevents_trial(self):
        with self.host.controller.lock():
            with self.assertRaisesRegex(FIXTURE.POWER.PowerError, "operation is running"):
                self.run_trial()
        self.assertEqual(self.writes, [])

    def test_cli_requires_explicit_duration_and_peak_and_has_no_root_override(self):
        for args in (["--output", "x"], ["--peak-watts", "55", "--output", "x"],
                     ["--root", "/tmp", "--peak-watts", "55", "--seconds", "1", "--output", "x"]):
            with self.assertRaises(SystemExit), mock.patch("sys.stderr"):
                PEAK.main(args)

    def test_cli_nonroot_fails_before_creating_output(self):
        self.host.env.uid = 1000
        output = self.root / "never-created.json"
        with mock.patch.object(PEAK, "PeakTrial", return_value=self.trial), mock.patch("sys.stderr"):
            self.assertEqual(PEAK.main(["--peak-watts", "55", "--seconds", "2", "--output", str(output)]), 2)
        self.assertFalse(output.exists())

    def test_cli_optional_msr_reader_is_passed_to_trial(self):
        output = self.root / "msr-cli.json"
        with mock.patch.object(PEAK.thermal, "System", return_value=self.system) as reader, \
                mock.patch.object(PEAK, "PeakTrial", return_value=self.trial) as factory, \
                mock.patch("sys.stdout"):
            code = PEAK.main(["--peak-watts", "55", "--seconds", "2", "--read-thermal-msr", "--output", str(output)])
        self.assertEqual(code, 0)
        reader.assert_called_once_with(read_thermal_msr=True)
        factory.assert_called_once_with(system=self.system)

    def test_cli_sigterm_handler_runs_recovery_and_returns_failure(self):
        self.system.callback = lambda: os.kill(os.getpid(), signal.SIGTERM)
        output = self.root / "signal.json"
        with mock.patch.object(PEAK, "PeakTrial", return_value=self.trial), mock.patch("sys.stdout"):
            self.assertEqual(PEAK.main(["--peak-watts", "55", "--seconds", "2", "--output", str(output)]), 1)
        result = json.loads(output.read_text())
        self.assertTrue(result["restored"])
        self.assertFalse(result["valid"])
        self.assertEqual(self.before, self.controls())

    def test_sigkill_orphan_has_durable_attempts_and_recovers_via_dead_owner(self):
        source = self.root / "killed.json"
        # A fork preserves the isolated Environment and never accesses real
        # sysfs. Kill during sample, after both durable write intentions.
        pid = os.fork()
        if pid == 0:
            try:
                output = PEAK.thermal.CheckpointFile(source)
                self.system.callback = lambda: os.kill(os.getpid(), signal.SIGKILL)
                self.trial.run(55, 2, output.save)
            finally:
                os._exit(99)
        _, status = os.waitpid(pid, 0)
        self.assertTrue(os.WIFSIGNALED(status))
        self.assertEqual(os.WTERMSIG(status), signal.SIGKILL)
        receipt = json.loads(source.read_text())
        self.assertTrue(all(r["attempted"] for r in receipt["zones"].values()))
        self.assertEqual(receipt["owner"]["pid"], pid)
        self.assertTrue(all(int(self.host.read(z + PEAK.LIMIT)) == 55_000_000 for z in FIXTURE.POWER.ZONES))
        original = source.read_bytes()
        result = self.trial.recover(source, self.save)
        self.assertTrue(result["restored"])
        self.assertEqual(source.read_bytes(), original)
        self.assertEqual(self.before, self.controls())


if __name__ == "__main__":
    unittest.main()
