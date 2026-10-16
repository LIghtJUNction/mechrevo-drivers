"""Exercise read-only observation, fault ordering, and the real sysfs/ioctl adapter."""
import copy
import importlib.util
import io
import json
import os
import signal
from pathlib import Path
import subprocess
import sys
import tempfile
import time
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
        self.assertEqual(self.system.gpu_calls, 8)
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
        self.system.fail_gpu = 8
        result = self.observe()
        self.assert_invalid(result)
        self.assertEqual(len(result["samples"]), 3)

    def test_silent_gpu_loss_before_sample_stops_without_waiting_for_journal(self):
        self.system.fail_gpu = 4
        result = self.observe(seconds=60)
        self.assert_invalid(result)
        self.assertEqual(self.system.sample_count, 1)
        self.assertEqual(result["faults"], [])
        self.assertEqual(self.system.now, 1)

    def test_silent_gpu_loss_during_sample_stops_before_next_ec_read(self):
        self.system.fail_gpu = 3
        result = self.observe(seconds=60)
        self.assert_invalid(result)
        self.assertEqual(self.system.sample_count, 1)
        self.assertEqual(result["faults"], [])
        self.assertEqual(self.system.now, 0)

    def test_changed_gpu_set_before_first_sample_is_rejected(self):
        original = self.system.gpu_health
        def changed():
            rows = original()
            return rows if self.system.gpu_calls == 1 else ["00000000:02:00.0"]
        self.system.gpu_health = changed
        result = self.observe()
        self.assert_invalid(result)
        self.assertEqual(self.system.sample_count, 0)

    def test_fault_during_gpu_probe_prevents_following_ec_read(self):
        original = self.system.gpu_health
        def fault():
            rows = original()
            if self.system.gpu_calls == 2:
                self.system.pending.append({"MESSAGE": "NVRM: Xid (PCI:0000:01:00): 79, lost",
                                            "__CURSOR": "probe-fault"})
            return rows
        self.system.gpu_health = fault
        result = self.observe()
        self.assert_invalid(result)
        self.assertEqual(self.system.sample_count, 0)
        self.assertEqual(len(result["faults"]), 1)

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
        parent = Path(os.environ.get("TMPDIR", ROOT / ".local-tests"))
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

    def rpm_fixture(self, cpu="3816", gpu="4939", node="hwmon7"):
        base = "/sys/class/hwmon/" + node + "/"
        for name, value in {"name": "mechrevo_ec", "fan1_label": "CPU fan",
                            "fan2_label": "GPU fan", "fan1_input": cpu,
                            "fan2_input": gpu, "temp1_input": "99000"}.items():
            self.write(base + name, value)
        return self.root / base.lstrip("/")

    def test_oem_rpm_discovery_keeps_raw_duty_and_internal_temperatures(self):
        self.rpm_fixture()
        before = {p: p.read_bytes() for p in self.root.rglob("*") if p.is_file()}
        self.system.open_sensors()
        sample = self.system.sample()
        self.assertEqual(sample["fan_rpm"], {"available": True, "source": "mechrevo_ec",
                                              "cpu_rpm": 3816, "gpu_rpm": 4939})
        self.assertEqual(sample["package_mc"], 80000)
        self.assertEqual(sample["core_max_mc"], 81000)
        self.assertEqual(sample["ec"]["fan1_raw"], 170)
        self.assertEqual({p: p.read_bytes() for p in before}, before)

    def test_absent_and_older_temperature_only_rpm_are_explicit_not_zero(self):
        expected = {"available": False, "reason": "mechrevo_ec RPM interface absent"}
        self.system.open_sensors()
        self.assertEqual(self.system.sample()["fan_rpm"], expected)
        self.write("/sys/class/hwmon/hwmon7/name", "mechrevo_ec")
        self.write("/sys/class/hwmon/hwmon7/temp1_input", "77000")
        self.assertEqual(self.system.sample()["fan_rpm"], expected)
        self.rpm_fixture(node="hwmon8")
        self.write("/sys/class/hwmon/hwmon8/name", "other_vendor")
        self.assertEqual(self.system.sample()["fan_rpm"], expected)

    def test_rpm_includes_valid_zero_and_unsigned_word_maximum_and_updates(self):
        self.rpm_fixture("0", "65535")
        self.system.open_sensors()
        first = self.system.sample()["fan_rpm"]
        self.assertEqual((first["cpu_rpm"], first["gpu_rpm"]), (0, 65535))
        self.rpm_fixture("65535", "0")
        second = self.system.sample()["fan_rpm"]
        self.assertEqual((second["cpu_rpm"], second["gpu_rpm"]), (65535, 0))

    def test_duplicate_oem_nodes_and_wrong_labels_are_errors(self):
        self.rpm_fixture()
        self.rpm_fixture(node="hwmon8")
        with self.assertRaisesRegex(THERMAL.ObservationError, "Ambiguous"):
            self.system.fan_rpm()
        self.write("/sys/class/hwmon/hwmon8/name", "other_vendor")
        for channel in (1, 2):
            with self.subTest(channel=channel):
                self.rpm_fixture()
                self.write(f"/sys/class/hwmon/hwmon7/fan{channel}_label", "other")
                with self.assertRaisesRegex(THERMAL.ObservationError, "label"):
                    self.system.fan_rpm()

    def test_advertised_missing_and_unreadable_rpm_never_become_absence(self):
        for name in ("fan1_label", "fan2_label", "fan1_input", "fan2_input"):
            with self.subTest(name=name):
                sensor = self.rpm_fixture()
                (sensor / name).unlink()
                with self.assertRaisesRegex(THERMAL.ObservationError, "Cannot read"):
                    self.system.fan_rpm()
        self.rpm_fixture()
        original = Path.read_text
        def denied(path, *args, **kwargs):
            if path.name == "fan2_input":
                raise PermissionError("denied")
            return original(path, *args, **kwargs)
        with mock.patch.object(Path, "read_text", denied):
            with self.assertRaisesRegex(THERMAL.ObservationError, "denied"):
                self.system.fan_rpm()

    def test_same_system_reopen_starts_new_rpm_availability_history(self):
        sensor = self.rpm_fixture()
        self.system.open_sensors()
        self.assertTrue(self.system.sample()["fan_rpm"]["available"])
        self.system.close()
        for path in sensor.glob("fan*"):
            path.unlink()
        self.system.open_sensors()
        self.assertEqual(self.system.sample()["fan_rpm"],
                         {"available": False, "reason": "mechrevo_ec RPM interface absent"})

    def test_previously_available_rpm_removal_invalidates_later_sample(self):
        sensor = self.rpm_fixture()
        self.system.open_sensors()
        self.assertTrue(self.system.sample()["fan_rpm"]["available"])
        (sensor / "fan1_input").unlink()
        with self.assertRaisesRegex(THERMAL.ObservationError, "Cannot read"):
            self.system.sample()
        for path in sensor.glob("fan*"):
            path.unlink()
        with self.assertRaisesRegex(THERMAL.ObservationError, "disappeared"):
            self.system.sample()
        self.write("/sys/class/hwmon/hwmon7/name", "other_vendor")
        with self.assertRaisesRegex(THERMAL.ObservationError, "disappeared"):
            self.system.sample()

    def test_actual_rpm_error_invalidates_observation_and_keeps_prior_sample(self):
        self.rpm_fixture()
        fake = FakeSystem()
        fake.open_sensors = self.system.open_sensors
        reads = 0
        def sample():
            nonlocal reads
            if reads:
                self.write("/sys/class/hwmon/hwmon7/fan2_input", "65536")
            reads += 1
            return self.system.sample()
        fake.sample = sample
        result = THERMAL.observe(2, 1, fake, fake.clock, fake.sleep)
        self.assertFalse(result["valid"])
        self.assertFalse(result["complete"])
        self.assertEqual(len(result["samples"]), 1)
        self.assertEqual(result["samples"][0]["fan_rpm"]["gpu_rpm"], 4939)
        self.assertIn("Invalid mechrevo_ec fan2 RPM", result["errors"][0])
        self.assertTrue(fake.closed)

    def test_each_malformed_or_out_of_range_rpm_is_rejected(self):
        for raw in ("-1", "+1", "65536", "1.0", "", "NaN", "1 2", "٠", "999999999999"):
            for channel in (1, 2):
                with self.subTest(raw=raw, channel=channel):
                    self.rpm_fixture()
                    self.write(f"/sys/class/hwmon/hwmon7/fan{channel}_input", raw)
                    with self.assertRaisesRegex(THERMAL.ObservationError, "Invalid"):
                        self.system.fan_rpm()

    def test_unknown_bios_rejected_before_open_or_ioctl(self):
        self.write("/sys/class/dmi/id/bios_version", "other")
        with self.assertRaises(THERMAL.ObservationError):
            self.system.open_sensors()
        self.assertIsNone(self.system.fd)
        self.assertEqual(self.requests,[])

    def qualify_msr_fixture(self):
        self.system.read_thermal_msr = True
        self.write("/proc/cpuinfo", "processor : 0\nvendor_id : GenuineIntel\n"
                   "cpu family : 6\nmodel : 151\ncpuid level : 32\nflags : msr dtherm pts pln\n")
        self.write("/dev/cpu/0/msr", "")
        path = self.system.path("/dev/cpu/0/msr")
        # This is a regular isolated fixture, never the host MSR device.
        with path.open("r+b") as stream:
            for offset, value in ((0x19c, 1 | 2 | 1024), (0x1b1, 4 | 8 | 2048), (0x1a2, 0x12345678)):
                stream.seek(offset)
                stream.write(value.to_bytes(8, "little"))

    def test_opt_in_actual_adapter_reads_fixed_msrs_read_only_and_separates_status(self):
        self.qualify_msr_fixture()
        before = self.system.path("/dev/cpu/0/msr").read_bytes()
        with mock.patch.object(THERMAL.os, "open", wraps=os.open) as opened, \
                mock.patch.object(THERMAL.os, "pread", wraps=os.pread) as read:
            self.system.open_sensors()
            result = self.system.sample()["thermal_msr"]
        self.assertEqual(opened.call_args_list[0].args,
                         (self.system.path("/dev/cpu/0/msr"), os.O_RDONLY | os.O_CLOEXEC))
        self.assertEqual([call.args[1:] for call in read.call_args_list], [(8, 0x19c), (8, 0x1b1), (8, 0x1a2)])
        core, package = result["core_status"]["status"], result["package_status"]["status"]
        self.assertTrue(core["internal_thermal_active"])
        self.assertTrue(core["power_limit_active"])
        self.assertFalse(core["external_prochot_active"])
        self.assertTrue(package["external_prochot_active"])
        self.assertTrue(package["external_prochot_log"])
        self.assertFalse(package["internal_thermal_active"])
        self.assertEqual(result["temperature_target"], {"offset":0x1a2, "raw":0x12345678})
        self.assertEqual(self.system.path("/dev/cpu/0/msr").read_bytes(), before)
        fd = self.system.msr_fd
        self.system.close()
        with self.assertRaises(OSError): os.fstat(fd)

    def test_default_adapter_never_opens_msr_and_reports_missing_optional_counters_as_null(self):
        self.system.open_sensors()
        with mock.patch.object(THERMAL.os, "pread", side_effect=AssertionError("unexpected MSR read")):
            sample = self.system.sample()
        self.assertNotIn("thermal_msr", sample)
        self.assertIsNone(self.system.msr_fd)
        self.assertEqual(sample["cpu0_throttle_details"], {name:None for name in THERMAL.EXTRA_THROTTLE_FIELDS})

    def test_cli_msr_option_is_explicit_and_failure_retains_invalid_json(self):
        for options in ([], ["--read-thermal-msr"]):
            with self.subTest(options=options), \
                    mock.patch.object(THERMAL, "observe", return_value={"valid":False, "errors":["denied"]}) as observe, \
                    redirect_stdout(io.StringIO()) as output:
                self.assertEqual(THERMAL.main(options), 1)
                self.assertEqual(json.loads(output.getvalue())["errors"], ["denied"])
                kwargs = observe.call_args.kwargs
                if options:
                    self.assertTrue(kwargs["system"].read_thermal_msr)
                else:
                    self.assertNotIn("system", kwargs)

    def test_optional_throttle_counters_preserve_values_zero_and_missing(self):
        self.write("/sys/devices/system/cpu/cpu0/thermal_throttle/core_throttle_count", "0")
        self.write("/sys/devices/system/cpu/cpu0/thermal_throttle/package_throttle_total_time_ms", "25")
        self.system.open_sensors()
        result = self.system.sample()["cpu0_throttle_details"]
        self.assertEqual(result, {"core_throttle_count":0, "package_throttle_total_time_ms":25,
                                 "package_throttle_max_time_ms":None})
        for value in ("-1", "garbage"):
            self.write("/sys/devices/system/cpu/cpu0/thermal_throttle/core_throttle_count", value)
            with self.assertRaises((ValueError, THERMAL.ObservationError)): self.system.sample()

    def test_unsupported_or_ambiguous_cpu_capabilities_fail_before_msr_open(self):
        self.qualify_msr_fixture()
        qualified = self.system.text("/proc/cpuinfo")
        cases = (qualified.replace("GenuineIntel", "OtherVendor"), qualified.replace("model : 151", "model : 183"),
                 qualified.replace("cpuid level : 32", "cpuid level : 5"), qualified.replace("pts", ""),
                 qualified.replace("pln", ""), qualified.replace("dtherm", ""), qualified.replace("msr", ""),
                 qualified + "\n\n" + qualified, "processor : 1\n", qualified + "\nmodel : 151")
        for contents in cases:
            with self.subTest(contents=contents):
                self.write("/proc/cpuinfo", contents)
                with self.assertRaises(THERMAL.ObservationError): self.system.open_sensors()
                self.assertIsNone(self.system.msr_fd)
                self.assertEqual(self.requests, [])

    def test_opt_in_missing_device_permission_denied_short_reads_and_read_errors_are_not_zero(self):
        self.qualify_msr_fixture()
        self.system.path("/dev/cpu/0/msr").unlink()
        with self.assertRaises(FileNotFoundError): self.system.open_sensors()
        with mock.patch.object(THERMAL.os, "open", side_effect=PermissionError("denied")):
            with self.assertRaises(PermissionError): self.system.open_sensors()
        self.write("/dev/cpu/0/msr", "")
        self.system.open_sensors()
        for raw in (b"", b"\0"*7):
            with mock.patch.object(THERMAL.os, "pread", return_value=raw):
                with self.assertRaisesRegex(THERMAL.ObservationError, "Short thermal MSR"):
                    self.system.sample()
        with mock.patch.object(THERMAL.os, "pread", side_effect=OSError("MSR unavailable")):
            with self.assertRaises(OSError): self.system.sample()

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

    def test_output_is_valid_incomplete_json_before_observation_starts(self):
        output = self.root / "in-progress.json"
        def inspect_progress(*args, **kwargs):
            progress = json.loads(output.read_text())
            self.assertFalse(progress["complete"])
            self.assertFalse(progress["valid"])
            return {"valid":False,"complete":False,"errors":["interrupted"],"samples":[]}
        with mock.patch.object(THERMAL,"observe",side_effect=inspect_progress):
            self.assertEqual(THERMAL.main(["--output",str(output)]),1)

    def test_checkpoint_write_failure_preserves_previous_complete_json(self):
        path = self.root / "checkpoint.json"
        output = THERMAL.CheckpointFile(path)
        before = path.read_bytes()
        with mock.patch.object(THERMAL.os,"fsync",side_effect=OSError("disk full")):
            with self.assertRaises(OSError):
                output.save({"valid":True,"complete":True,"samples":[1]})
        self.assertEqual(path.read_bytes(),before)
        self.assertFalse(json.loads(path.read_text())["valid"])
        self.assertEqual(list(self.root.glob(".mechrevo-thermal-*")),[])

    def test_checkpoint_does_not_overwrite_an_external_replacement(self):
        path = self.root / "checkpoint.json"
        output = THERMAL.CheckpointFile(path)
        replacement = self.root / "replacement.json"
        replacement.write_text("external content")
        os.replace(replacement,path)
        with self.assertRaises(THERMAL.ObservationError):
            output.save({"valid":True})
        self.assertEqual(path.read_text(),"external content")

    def test_each_sample_is_checkpointed_as_incomplete_before_final_success(self):
        system = FakeSystem()
        progress = []
        result = THERMAL.observe(2,1,system,system.clock,system.sleep,
                                 checkpoint=lambda state: progress.append(copy.deepcopy(state)))
        self.assertTrue(result["valid"])
        self.assertEqual([len(p["samples"]) for p in progress],[0,1,2,3])
        self.assertTrue(all(p["valid"] is False and p["complete"] is False for p in progress))

    def test_real_process_interruption_retains_parseable_incomplete_samples(self):
        # Real signals exercise the CLI handler, file replacement and sampler
        # together. Only FakeSystem is used; no hardware or service is touched.
        child = r'''
import importlib.util,sys,time
spec=importlib.util.spec_from_file_location('fixture',sys.argv[1])
fixture=importlib.util.module_from_spec(spec);spec.loader.exec_module(fixture)
module=fixture.THERMAL
real_observe=module.observe
system=fixture.FakeSystem()
def sleep(seconds):
 time.sleep(0.25);system.sleep(seconds)
def observe(seconds,interval,checkpoint=None):
 return real_observe(seconds,interval,system,system.clock,sleep,checkpoint)
module.observe=observe
raise SystemExit(module.main(['--seconds','60','--output',sys.argv[2]]))
'''
        for signum in (signal.SIGTERM,signal.SIGHUP,signal.SIGKILL):
            with self.subTest(signal=signum):
                output = self.root / (str(signum)+".json")
                process = subprocess.Popen([sys.executable,"-B","-c",child,__file__,str(output)],
                                           stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True)
                try:
                    deadline = time.monotonic()+5
                    while time.monotonic()<deadline:
                        try:
                            if json.loads(output.read_text()).get("samples"):
                                break
                        except (FileNotFoundError,json.JSONDecodeError):
                            pass
                        if process.poll() is not None:
                            self.fail("Fixture process ended before its first checkpoint")
                        time.sleep(0.01)
                    else:
                        self.fail("No sample checkpoint before interruption")
                    process.send_signal(signum)
                    stdout,stderr=process.communicate(timeout=5)
                    self.assertEqual(process.returncode,-signal.SIGKILL if signum==signal.SIGKILL else 1,stderr)
                    result=json.loads(output.read_text())
                    self.assertFalse(result["valid"])
                    self.assertFalse(result["complete"])
                    self.assertGreaterEqual(len(result["samples"]),1)
                    self.assertEqual(output.stat().st_mode & 0o777,0o600)
                    if signum!=signal.SIGKILL:
                        self.assertIn("signal",result["errors"][0])
                finally:
                    if process.poll() is None:
                        process.kill()
                    process.communicate(timeout=5)


if __name__ == "__main__":
    unittest.main()
