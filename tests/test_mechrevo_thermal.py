"""Exercise read-only observation, fault ordering, and the real sysfs/ioctl adapter."""
import copy
import importlib.util
import io
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest import mock
from contextlib import redirect_stdout, redirect_stderr

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("mechrevo_thermal", ROOT / "scripts/mechrevo_thermal.py")
THERMAL = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(THERMAL)


class FakeSystem:
    def __init__(self):
        self.now = 0.0
        self.closed = False
        self.opened = False
        self.sample_count = 0
        self.journal_calls = []
        self.pending = []
        self.gpu_calls = 0
        self.fail_gpu = None
        self.fault_at_sample = None
        self.boot = "a" * 32
        self.fail_journal = False
        self.read_error = None
        self.cap = 55_000_000
        self.power = {"managed": True, "desired_limit_uw": self.cap,
                      "holding_now": True, "pending_transaction": None, "errors": []}
        self.power_after = None
        self.power_calls = 0

    def clock(self):
        return self.now

    def sleep(self, seconds):
        self.now += seconds

    def boot_id(self):
        return self.boot

    def gpu_health(self):
        self.gpu_calls += 1
        if self.gpu_calls == self.fail_gpu:
            raise THERMAL.ObservationError("GPU unavailable")
        return ["00000000:01:00.0"]

    def journal(self, boot, cursor=None):
        self.journal_calls.append(cursor)
        if self.fail_journal:
            raise THERMAL.ObservationError("Cannot read kernel journal")
        if cursor is None:
            # An old error establishes the start cursor, not a new fault.
            return [{"__CURSOR": "old", "_BOOT_ID": boot,
                     "MESSAGE": "NVRM: Xid (PCI:0000:01:00): 79, old fault"}]
        entries, self.pending = self.pending, []
        return entries

    def power_status(self):
        self.power_calls += 1
        return copy.deepcopy(self.power_after if self.power_calls > 1 and self.power_after else self.power)

    def open_sensors(self):
        self.opened = True

    def sample(self):
        if self.read_error:
            raise self.read_error
        self.sample_count += 1
        if self.sample_count == self.fault_at_sample:
            self.pending.append({"MESSAGE": "NVRM: Xid (PCI:0000:01:00): 79, GPU has fallen off the bus.",
                                 "__CURSOR": "new", "__MONOTONIC_TIMESTAMP": "200"})
        return {"monotonic_s": self.now, "rapl": {z: {"constraint_0_power_limit_uw": self.cap}
                                                   for z in THERMAL.ZONES}}

    def close(self):
        self.closed = True


class ThermalObservation(unittest.TestCase):
    def setUp(self):
        self.system = FakeSystem()

    def observe(self, seconds=2, interval=1):
        return THERMAL.observe(seconds, interval, self.system, self.system.clock, self.system.sleep)

    def assert_invalid(self, result):
        self.assertFalse(result["valid"])
        self.assertFalse(result["complete"])
        self.assertTrue(result["errors"])
        self.assertTrue(self.system.closed)

    def test_healthy_capture_ignores_old_fault_and_collects_both_endpoints(self):
        result = self.observe()
        self.assertTrue(result["valid"])
        self.assertTrue(result["complete"])
        self.assertEqual(len(result["samples"]), 3)
        self.assertEqual(result["faults"], [])
        self.assertEqual(self.system.gpu_calls, 2)
        self.assertTrue(self.system.closed)

    def test_unavailable_gpu_prevents_sensor_open_or_any_samples(self):
        self.system.fail_gpu = 1
        result = self.observe()
        self.assert_invalid(result)
        self.assertFalse(self.system.opened)
        self.assertEqual(result["samples"], [])

    def test_fault_during_sensor_read_stops_immediately_and_retains_sample(self):
        self.system.fault_at_sample = 2
        result = self.observe(seconds=60)
        self.assert_invalid(result)
        self.assertEqual(len(result["samples"]), 2)
        self.assertEqual(self.system.now, 1)
        self.assertEqual(len(result["faults"]), 1)

    def test_fault_before_first_sensor_read_prevents_sampling(self):
        self.system.pending = [{"MESSAGE": "PCIe Bus Error: severity=Uncorrectable (Non-Fatal)",
                                "__CURSOR": "fatal"}]
        result = self.observe()
        self.assert_invalid(result)
        self.assertEqual(self.system.sample_count, 0)

    def test_silent_gpu_loss_in_final_probe_invalidates_complete_samples(self):
        self.system.fail_gpu = 2
        result = self.observe()
        self.assert_invalid(result)
        self.assertEqual(len(result["samples"]), 3)

    def test_journal_failure_is_not_treated_as_zero_errors(self):
        self.system.fail_journal = True
        self.assert_invalid(self.observe())
        self.assertEqual(self.system.gpu_calls, 0)

    def test_changed_boot_stops_capture(self):
        original = self.system.sample
        def reboot():
            sample = original()
            self.system.boot = "b" * 32
            return sample
        self.system.sample = reboot
        result = self.observe()
        self.assert_invalid(result)
        self.assertEqual(len(result["samples"]), 1)

    def test_io_failure_and_interrupt_retain_prior_samples_and_close(self):
        for error in (OSError("EC timeout"), KeyboardInterrupt()):
            with self.subTest(error=type(error).__name__):
                self.system = FakeSystem()
                original = self.system.sample
                def fail_second():
                    if self.system.sample_count:
                        raise error
                    return original()
                self.system.sample = fail_second
                result = self.observe()
                self.assert_invalid(result)
                self.assertEqual(len(result["samples"]), 1)

    def test_bounds_reject_nonfinite_zero_negative_and_excessive_samples_before_probing(self):
        for seconds, interval in ((0,1), (-1,1), (3601,1), (60,0), (60,0.1), (60,11),
                                  (float('nan'),1), (1,float('inf')), (True,1), (3600,0.2)):
            with self.subTest(seconds=seconds, interval=interval):
                self.system = FakeSystem()
                self.assert_invalid(self.observe(seconds, interval))
                self.assertEqual(self.system.journal_calls, [])

    def test_invalid_parameters_still_produce_strict_json(self):
        for value in (float('nan'), float('inf'), -float('inf')):
            result = self.observe(value, 1)
            self.assert_invalid(result)
            json.dumps(result, allow_nan=False)

    def test_does_not_busy_loop_to_recover_a_slow_read(self):
        original = self.system.sample
        def slow():
            self.system.now += 1.5
            return original()
        self.system.sample = slow
        result = self.observe(seconds=4)
        self.assertTrue(result["valid"])
        self.assertEqual(len(result["samples"]), 2)

    def test_pending_transaction_and_unconfirmed_cap_prevent_sampling(self):
        for mutation in ({"pending_transaction": "pending"}, {"holding_now": False},
                         {"desired_limit_uw": True}, {"desired_limit_uw": "55000000"}):
            self.system = FakeSystem()
            self.system.power.update(mutation)
            self.assert_invalid(self.observe())
            self.assertFalse(self.system.opened)

    def test_transient_pl1_exceedance_invalidates_even_if_end_status_would_be_healthy(self):
        self.system.cap = 56_000_000
        self.assert_invalid(self.observe())
        self.assertEqual(self.system.sample_count, 1)

    def test_external_cap_change_invalidates_comparison(self):
        self.system.power_after = dict(self.system.power, desired_limit_uw=35_000_000)
        self.assert_invalid(self.observe())

    def test_fault_patterns_exclude_corrected_pcie_and_unrelated_logs(self):
        for message in ("NVRM: Xid (PCI:0000:01:00): 31, app fault", "GPU has fallen off the bus.",
                        "pcieport: AER: Uncorrectable (Non-Fatal) error", "pcieport: AER: device recovery failed"):
            self.assertTrue(THERMAL.fault_message(message), message)
        for message in ("PCIe Bus Error: severity=Corrected", "CPU0: Thermal monitoring enabled",
                        "[UFW BLOCK]", "NVRM: loading NVIDIA UNIX", None, [1]):
            self.assertFalse(THERMAL.fault_message(message), message)


class ThermalSystemAdapter(unittest.TestCase):
    def setUp(self):
        parent = ROOT / ".local-tests"
        parent.mkdir(exist_ok=True)
        self.temp = tempfile.TemporaryDirectory(prefix="thermal-observer-", dir=parent)
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.requests = []
        def ioctl(fd, request, buffer, mutate):
            self.requests.append(request)
            buffer[0] = {"available": 1, "ec_cpu_c": 77, "ec_gpu_c": 55,
                         "fan1_raw": 170, "fan2_raw": 165}[next(k for k,v in THERMAL.READ_REQUESTS.items()
                                                             if v == request)]
        self.system = THERMAL.System(self.root, ioctl=ioctl)
        self.addCleanup(self.system.close)
        for k,v in THERMAL.IDENTITY.items():
            self.write("/sys/class/dmi/id/"+k,v)
        hwmon = "/sys/class/hwmon/hwmon42/"
        for name,value in {"name":"coretemp", "temp1_label":"Package id 0", "temp1_input":"80000",
                           "temp2_input":"79000", "temp6_input":"81000"}.items():
            self.write(hwmon+name,value)
        self.write("/dev/tuxedo_io", "")
        self.write("/sys/devices/system/cpu/cpu0/thermal_throttle/package_throttle_count", "1234")
        self.write("/sys/class/powercap/intel-rapl:0/energy_uj", "1000000")
        self.write("/sys/class/powercap/intel-rapl:0/max_energy_range_uj", "262143328850")
        for zone in THERMAL.ZONES:
            for name,value in {"constraint_0_power_limit_uw":"35000000", "constraint_1_power_limit_uw":"140000000",
                               "constraint_0_time_window_us":"999424", "constraint_1_time_window_us":"2440"}.items():
                self.write("/sys/class/powercap/"+zone+"/"+name,value)

    def write(self,name,value):
        path = self.root / name.lstrip("/")
        path.parent.mkdir(parents=True,exist_ok=True)
        path.write_text(value)

    def test_actual_adapter_selects_by_identity_and_only_issues_read_requests(self):
        before = {p:p.read_bytes() for p in self.root.rglob("*") if p.is_file()}
        self.system.open_sensors()
        sample = self.system.sample()
        self.assertEqual(sample["package_mc"],80000)
        self.assertEqual(sample["core_max_mc"],81000)
        self.assertEqual(sample["ec"]["fan1_raw"],170)
        self.assertEqual(sample["cpu0_package_throttle_count"],1234)
        self.assertEqual(sample["rapl"]["intel-rapl:0"]["constraint_1_power_limit_uw"],140000000)
        self.assertEqual(self.requests,[0x8008ec06,0x8008ef12,0x8008ef13,0x8008ef10,0x8008ef11])
        self.assertTrue(all(r >> 30 == 2 for r in self.requests))
        self.assertEqual({p:p.read_bytes() for p in before},before)

    def test_unknown_bios_rejected_before_open_or_ioctl(self):
        self.write("/sys/class/dmi/id/bios_version", "other")
        with self.assertRaises(THERMAL.ObservationError):
            self.system.open_sensors()
        self.assertIsNone(self.system.fd)
        self.assertEqual(self.requests,[])

    def test_bad_temperature_is_not_reported_as_valid(self):
        self.system.open_sensors()
        self.write("/sys/class/hwmon/hwmon42/temp1_input", "255000")
        with self.assertRaises(THERMAL.ObservationError):
            self.system.sample()

    def test_gpu_health_rejects_empty_or_malformed_success_and_timeout(self):
        for output in ("", "No devices were found\n", "00000000:01:00.0\nUnknown Error\n"):
            self.system.runner = mock.Mock(return_value=subprocess.CompletedProcess([],0,output,""))
            with self.assertRaises(THERMAL.ObservationError):
                self.system.gpu_health()
        self.system.runner = mock.Mock(side_effect=subprocess.TimeoutExpired("nvidia-smi",8))
        with self.assertRaises(subprocess.TimeoutExpired):
            self.system.gpu_health()

    def test_journal_rejects_missing_boot_cursor_or_nontext_message(self):
        for entry in ({}, {"__CURSOR":"x","_BOOT_ID":"different","MESSAGE":"ok"},
                      {"__CURSOR":"x","_BOOT_ID":"boot","MESSAGE":[1,2]}):
            self.system.runner = mock.Mock(return_value=subprocess.CompletedProcess([],0,json.dumps(entry),""))
            with self.assertRaises(THERMAL.ObservationError):
                self.system.journal("boot")

    def test_journal_incremental_empty_is_valid_but_initial_empty_is_not(self):
        self.system.runner = mock.Mock(return_value=subprocess.CompletedProcess([],0,"",""))
        self.assertEqual(self.system.journal("boot","cursor"),[])
        with self.assertRaises(THERMAL.ObservationError):
            self.system.journal("boot")

    def test_cli_refuses_symlink_destination_without_touching_target(self):
        target = self.root / "target"
        target.write_text("original")
        output = self.root / "link"
        output.symlink_to(target)
        with mock.patch.object(THERMAL,"observe") as observe, redirect_stderr(io.StringIO()):
            self.assertEqual(THERMAL.main(["--output",str(output)]),2)
            observe.assert_not_called()
        self.assertEqual(target.read_text(),"original")

    def test_cli_refuses_existing_destination_without_probing_and_returns_failure_json(self):
        output = self.root / "capture.json"
        output.write_text("existing")
        with mock.patch.object(THERMAL,"observe") as observe, redirect_stderr(io.StringIO()):
            self.assertEqual(THERMAL.main(["--output",str(output)]),2)
            observe.assert_not_called()
        self.assertEqual(output.read_text(),"existing")
        invalid = {"valid":False,"complete":False,"errors":["GPU unavailable"]}
        stream = io.StringIO()
        with mock.patch.object(THERMAL,"observe",return_value=invalid), redirect_stdout(stream):
            self.assertEqual(THERMAL.main([]),1)
        self.assertEqual(json.loads(stream.getvalue()),invalid)


if __name__ == "__main__":
    unittest.main()
