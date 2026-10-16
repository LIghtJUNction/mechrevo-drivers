"""Exercise read-only observation, fault ordering, and the real sysfs/ioctl adapter."""
import copy
from types import SimpleNamespace
import importlib.util
import io
import json
import os
import signal
import stat
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

    def test_managed_status_requires_an_explicit_boolean_and_complete_cap(self):
        missing = object()
        cases = [("managed", value) for value in (missing, None, 0, 1, "false", [], {})]
        cases += [("desired_limit_uw", value) for value in
                  (missing, None, True, "55000000", 55_000_000.0, 0, -1, 34_999_999, 2_147_483_648)]
        cases += [("holding_now", value) for value in (missing, None, False, 1, "true")]
        cases += [("pending_transaction", missing)]
        for key, value in cases:
            with self.subTest(key=key, value=value):
                self.system = FakeSystem()
                if value is missing:
                    del self.system.power[key]
                else:
                    self.system.power[key] = value
                self.assert_invalid(self.observe())
                self.assertFalse(self.system.opened)
                self.assertEqual(self.system.sample_count, 0)

    def test_managed_cap_range_endpoints_remain_valid(self):
        for cap in (35_000_000, 2_147_483_647):
            with self.subTest(cap=cap):
                self.system = FakeSystem()
                self.system.cap = cap
                self.system.power["desired_limit_uw"] = cap
                result = self.observe()
                self.assertTrue(result["valid"])
                self.assertEqual(result["errors"], [])
                self.assertEqual(len(result["samples"]), 3)

    def test_unmanaged_status_without_optional_policy_fields_remains_valid(self):
        self.system.power = {"managed": False, "errors": []}
        result = self.observe()
        self.assertTrue(result["valid"])
        self.assertTrue(result["complete"])
        self.assertEqual(result["errors"], [])
        self.assertEqual(len(result["samples"]), 3)

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
        self.msr_backing_identity = None
        self.msr_type = (0o20600, os.makedev(202, 0))
        real_stat, real_fstat = os.stat, os.fstat
        def character(info):
            mode, device = self.msr_type
            return SimpleNamespace(st_mode=mode, st_rdev=device, st_dev=info.st_dev, st_ino=info.st_ino)
        def node_stat(name, *args, **kwargs):
            info = real_stat(name, *args, **kwargs)
            if (name == "msr" and kwargs.get("dir_fd") is not None
                    and not stat.S_ISLNK(info.st_mode) and self.msr_backing_identity is not None):
                return character(info)
            return info
        def descriptor_stat(fd):
            info = real_fstat(fd)
            if self.msr_backing_identity == (info.st_dev, info.st_ino):
                return character(info)
            return info
        self.stat_patch = mock.patch.object(THERMAL.os, "stat", side_effect=node_stat)
        self.stat_patch.start(); self.addCleanup(self.stat_patch.stop)
        self.fstat_patch = mock.patch.object(THERMAL.os, "fstat", side_effect=descriptor_stat)
        self.fstat_patch.start(); self.addCleanup(self.fstat_patch.stop)
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

    def assert_timed_adapter(self, with_msr, with_rpm=False):
        if with_rpm:
            self.rpm_fixture()
        if with_msr:
            self.qualify_msr_fixture()
        self.system.open_sensors()
        now, events = [100.0], []
        original_read = Path.read_text
        original_ioctl, original_pread = self.system.ioctl, THERMAL.os.pread
        def timed(kind, operation, *args, **kwargs):
            before = now[0]
            try:
                return operation(*args, **kwargs)
            finally:
                now[0] += 0.05 if kind == "ioctl" else 0.02
                events.append((kind, before, now[0]))
        def text(path, *args, **kwargs):
            return timed("text", original_read, path, *args, **kwargs)
        with mock.patch.object(THERMAL.time, "monotonic", side_effect=lambda: now[0]), \
                mock.patch.object(Path, "read_text", text), \
                mock.patch.object(self.system, "ioctl", side_effect=lambda *a: timed("ioctl", original_ioctl, *a)), \
                mock.patch.object(THERMAL.os, "pread", side_effect=lambda *a: timed("msr", original_pread, *a)):
            sample = self.system.sample()
        expected = [("package_before", 1), ("core_max", 2), ("throttle_count", 1),
                    ("energy", 2), ("ec", 4), ("rapl", 8), ("fan_rpm", 6 if with_rpm else 1),
                    ("package_after", 1), ("throttle_details", 3)]
        if with_msr:
            # Release and boot qualification reads occur before and after the
            # three fixed preads, all inside the same measured MSR window.
            expected.append(("thermal_msr", 7))
        self.assertEqual(list(sample["read_windows"]), [name for name, _ in expected])
        previous, position = sample["monotonic_s"], 0
        for name, count in expected:
            window = sample["read_windows"][name]
            begin, end = window["start_monotonic_s"], window["end_monotonic_s"]
            self.assertGreaterEqual(begin, previous)
            self.assertGreater(end, begin)
            group = events[position:position+count]
            self.assertEqual(len(group), count)
            self.assertEqual(begin, group[0][1])
            self.assertEqual(end, group[-1][2])
            self.assertTrue(all(begin <= a <= b <= end for _, a, b in group))
            previous, position = end, position+count
        self.assertEqual(position, len(events))
        self.assertEqual(previous, sample["monotonic_s"] + sample["read_duration_s"])
        self.assertEqual(sample["fan_rpm"]["available"], with_rpm)
        if with_rpm:
            self.assertEqual(sample["fan_rpm"]["cpu_rpm"], 3816)
            self.assertEqual(sample["fan_rpm"]["gpu_rpm"], 4939)
        self.assertEqual(self.requests, [THERMAL.READ_REQUESTS[name] for name in THERMAL.READ_REQUESTS])
        self.assertEqual(sample["package_mc"], 80000)
        self.assertEqual(sample["energy_uj"], 1000000)
        return sample

    def test_read_windows_enclose_actual_reads_in_original_order_without_optional_msr(self):
        sample = self.assert_timed_adapter(False)
        self.assertNotIn("thermal_msr", sample)
        self.assertNotIn("thermal_msr", sample["read_windows"])

    def test_read_windows_enclose_all_requested_msr_reads_after_other_channels(self):
        sample = self.assert_timed_adapter(True)
        self.assertEqual(len(sample["thermal_msr"]), 3)

    def test_available_rpm_window_encloses_discovery_and_both_labels_and_inputs(self):
        # Two discovered hwmon names (coretemp and mechrevo_ec), then four
        # label/input reads. Discovery itself belongs inside this same window.
        self.assert_timed_adapter(False, with_rpm=True)

    def test_invalid_or_decreasing_read_clock_refuses_sample_before_later_io(self):
        self.system.open_sensors()
        for stamps in ([float("nan")], [float("inf")], [10**400], [-1], [True], ["1"],
                       [100, 99], [100, 100, 99], [100, 100, float("nan")]):
            with self.subTest(stamps=stamps), \
                    mock.patch.object(THERMAL.time, "monotonic", side_effect=stamps):
                with self.assertRaisesRegex(THERMAL.ObservationError, "monotonic read timestamp"):
                    self.system.sample()
        self.assertEqual(self.requests, [THERMAL.READ_REQUESTS["available"]])

    def test_read_failure_does_not_publish_partial_window_sample_and_preserves_prior(self):
        self.system.open_sensors()
        fake = FakeSystem()
        original = self.system.text
        calls = [0]
        def sample():
            calls[0] += 1
            if calls[0] == 2:
                def fail_energy(name):
                    if name.endswith("/energy_uj"):
                        raise OSError("injected energy read failure")
                    return original(name)
                self.system.text = fail_energy
            return self.system.sample()
        fake.cap = 35000000
        fake.power["desired_limit_uw"] = fake.cap
        fake.sample = sample
        result = THERMAL.observe(2, 1, fake, fake.clock, fake.sleep)
        self.assertFalse(result["valid"])
        self.assertFalse(result["complete"])
        self.assertEqual(len(result["samples"]), 1)
        self.assertEqual(len(result["samples"][0]["read_windows"]), 9)
        self.assertIn("injected energy read failure", result["errors"][0])

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
        self.write("/proc/sys/kernel/osrelease", "7.2.9-arch1-1")
        self.write("/proc/sys/kernel/random/boot_id", "11111111-2222-3333-4444-555555555555")
        self.write("/sys/module/msr/parameters/allow_writes", "off")
        self.write("/sys/kernel/notes", "synthetic built-in kernel notes")
        self.write("/usr/lib/modules/7.2.9-arch1-1/modules.builtin", "kernel/arch/x86/kernel/msr.ko")
        self.write("/dev/cpu/0/msr", "")
        path = self.system.path("/dev/cpu/0/msr")
        info = path.stat()
        self.msr_backing_identity = (info.st_dev, info.st_ino)
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
        device_open = next(call for call in opened.call_args_list if call.args[0] == "msr")
        self.assertEqual(device_open.args, ("msr", os.O_RDONLY | os.O_CLOEXEC | os.O_NOFOLLOW))
        self.assertIsNotNone(device_open.kwargs["dir_fd"])
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
        info = self.system.path("/dev/cpu/0/msr").stat()
        self.msr_backing_identity = (info.st_dev, info.st_ino)
        self.system.open_sensors()
        for raw in (b"", b"\0"*7):
            if self.system.msr_fd is None:
                self.system.open_sensors()
            with mock.patch.object(THERMAL.os, "pread", return_value=raw):
                with self.assertRaisesRegex(THERMAL.ObservationError, "Short thermal MSR"):
                    self.system.sample()
        self.system.open_sensors()
        with mock.patch.object(THERMAL.os, "pread", side_effect=OSError("MSR unavailable")):
            with self.assertRaises(OSError): self.system.sample()

    def assert_msr_fds_closed(self, descriptors):
        self.assertIsNone(self.system.msr_fd)
        self.assertIsNone(self.system.fd)
        self.assertEqual(self.system.msr_descriptors, [])
        for fd in descriptors:
            with self.assertRaises(OSError):
                os.fstat(fd)

    def test_regular_wrong_major_and_wrong_cpu_msr_nodes_are_refused_before_read(self):
        self.qualify_msr_fixture()
        for identity in ((0o100600, os.makedev(202, 0)),
                         (0o20600, os.makedev(203, 0)), (0o20600, os.makedev(202, 1))):
            with self.subTest(identity=identity), \
                    mock.patch.object(THERMAL.os, "pread") as read:
                self.msr_type = identity
                with self.assertRaisesRegex(THERMAL.ObservationError, "character device 202:0"):
                    self.system.open_sensors()
                read.assert_not_called()
                self.assertEqual(self.system.msr_descriptors, [])

    def test_msr_leaf_and_parent_symlinks_refused_without_reading_targets(self):
        self.qualify_msr_fixture()
        node = self.system.path("/dev/cpu/0/msr")
        saved = node.with_name("saved-msr")
        node.rename(saved); node.symlink_to(saved.name)
        with mock.patch.object(THERMAL.os, "pread") as read:
            with self.assertRaisesRegex(THERMAL.ObservationError, "character device"):
                self.system.open_sensors()
            read.assert_not_called()
        node.unlink(); saved.rename(node)
        parent = node.parent
        saved_parent = parent.with_name("saved-cpu0")
        parent.rename(saved_parent); parent.symlink_to(saved_parent.name)
        with mock.patch.object(THERMAL.os, "pread") as read:
            with self.assertRaises(OSError):
                self.system.open_sensors()
            read.assert_not_called()
        self.assertEqual(self.system.msr_descriptors, [])

    def test_missing_support_or_loaded_module_refused_before_device_open(self):
        self.qualify_msr_fixture()
        mutations = (("/sys/module/msr/initstate", "live"),
                     ("/usr/lib/modules/7.2.9-arch1-1/modules.builtin", "other.ko"),
                     ("/sys/kernel/notes", ""), ("/proc/sys/kernel/random/boot_id", "bad"),
                     ("/proc/sys/kernel/osrelease", "../bad"))
        for name, content in mutations:
            path = self.system.path(name)
            before = path.read_bytes() if path.exists() else None
            self.write(name, content)
            with self.subTest(name=name), mock.patch.object(THERMAL.os, "open") as opened:
                with self.assertRaises(THERMAL.ObservationError):
                    self.system.open_sensors()
                opened.assert_not_called()
            if before is None: path.unlink()
            else: path.write_bytes(before)
        module = self.system.path("/sys/module/msr")
        module.rename(module.with_name("missing-msr"))
        with mock.patch.object(THERMAL.os, "open") as opened:
            with self.assertRaises(FileNotFoundError): self.system.open_sensors()
            opened.assert_not_called()

    def test_symlinked_support_manifest_module_and_dangling_initstate_refused_before_open(self):
        self.qualify_msr_fixture()
        for logical in ("/usr/lib/modules/7.2.9-arch1-1/modules.builtin", "/sys/module/msr"):
            path = self.system.path(logical); saved = path.with_name(path.name+"-saved")
            path.rename(saved); path.symlink_to(saved.name)
            with self.subTest(logical=logical), mock.patch.object(THERMAL.os, "open") as opened:
                with self.assertRaises(THERMAL.ObservationError): self.system.open_sensors()
                opened.assert_not_called()
            path.unlink(); saved.rename(path)
        self.system.path("/sys/module/msr/initstate").symlink_to("missing-initstate")
        with mock.patch.object(THERMAL.os, "open") as opened:
            with self.assertRaisesRegex(THERMAL.ObservationError, "Loaded-only"):
                self.system.open_sensors()
            opened.assert_not_called()

    def test_default_path_does_not_probe_msr_support_or_validate_nodes(self):
        with mock.patch.object(self.system, "msr_support_identity", side_effect=AssertionError("MSR support probe")), \
                mock.patch.object(self.system, "open_thermal_msr", side_effect=AssertionError("MSR open")):
            self.system.open_sensors()
            self.system.sample()

    def test_changed_opened_msr_identity_is_rejected_and_parent_fds_closed(self):
        self.qualify_msr_fixture()
        original, opened, actual_open = THERMAL.os.fstat, [], THERMAL.os.open
        def changed(fd):
            value = original(fd)
            if stat.S_ISCHR(value.st_mode):
                return SimpleNamespace(st_mode=value.st_mode, st_dev=value.st_dev,
                                       st_ino=value.st_ino+1, st_rdev=value.st_rdev)
            return value
        def opening(*args, **kwargs):
            fd = actual_open(*args, **kwargs); opened.append(fd); return fd
        with mock.patch.object(THERMAL.os, "fstat", side_effect=changed), \
                mock.patch.object(THERMAL.os, "open", side_effect=opening), \
                mock.patch.object(THERMAL.os, "pread") as read:
            with self.assertRaisesRegex(THERMAL.ObservationError, "device identity changed"):
                self.system.open_sensors()
            read.assert_not_called()
        self.assert_msr_fds_closed(opened)

    def test_msr_open_permission_failure_closes_all_previously_opened_directories(self):
        self.qualify_msr_fixture()
        original, opened = THERMAL.os.open, []
        def opening(name, *args, **kwargs):
            if name == "msr": raise PermissionError("injected MSR open permission")
            fd = original(name, *args, **kwargs); opened.append(fd); return fd
        with mock.patch.object(THERMAL.os, "open", side_effect=opening):
            with self.assertRaisesRegex(PermissionError, "MSR open permission"):
                self.system.open_sensors()
        self.assertEqual(len(opened), 3)
        self.assert_msr_fds_closed(opened)

    def test_support_boot_and_path_changes_during_read_discard_values_close_all_fds(self):
        self.qualify_msr_fixture()
        original_read = os.pread
        cases = ("boot", "notes", "manifest", "loaded", "modulegone", "parent", "device")
        for mode in cases:
            with self.subTest(mode=mode):
                self.system.open_sensors()
                descriptors = self.system.msr_descriptors + [self.system.fd]
                path = saved = before = None
                def mutate(fd, count, offset):
                    nonlocal path, saved, before
                    raw = original_read(fd, count, offset)
                    if offset == 0x1a2:
                        if mode in ("boot", "notes", "manifest", "loaded"):
                            logical, text = {"boot":("/proc/sys/kernel/random/boot_id", "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"),
                                "notes":("/sys/kernel/notes", "changed notes"),
                                "manifest":("/usr/lib/modules/7.2.9-arch1-1/modules.builtin", "kernel/arch/x86/kernel/msr.ko\nextra.ko"),
                                "loaded":("/sys/module/msr/initstate", "live")}[mode]
                            path = self.system.path(logical)
                            before = path.read_bytes() if path.exists() else None
                            self.write(logical, text)
                        else:
                            path = self.system.path({"modulegone":"/sys/module/msr", "parent":"/dev/cpu/0", "device":"/dev/cpu/0/msr"}[mode])
                            saved = path.with_name(path.name+"-old")
                            path.rename(saved)
                            if mode == "parent": path.mkdir()
                            elif mode == "device": path.write_bytes(saved.read_bytes())
                    return raw
                with mock.patch.object(THERMAL.os, "pread", side_effect=mutate):
                    with self.assertRaises((THERMAL.ObservationError, FileNotFoundError)):
                        self.system.thermal_msr()
                self.assert_msr_fds_closed(descriptors)
                if saved is not None:
                    if path.exists():
                        if path.is_dir(): path.rmdir()
                        else: path.unlink()
                    saved.rename(path)
                elif before is None: path.unlink()
                else: path.write_bytes(before)

    def test_support_change_between_samples_refused_before_any_new_pread(self):
        self.qualify_msr_fixture(); self.system.open_sensors()
        self.system.thermal_msr()
        self.write("/sys/module/msr/initstate", "live")
        descriptors = self.system.msr_descriptors + [self.system.fd]
        with mock.patch.object(THERMAL.os, "pread") as read:
            with self.assertRaisesRegex(THERMAL.ObservationError, "Loaded-only"):
                self.system.thermal_msr()
            read.assert_not_called()
        self.assert_msr_fds_closed(descriptors)

    def test_msr_close_failure_attempts_every_fd_retains_original_read_error_in_invalid_capture(self):
        self.qualify_msr_fixture(); self.system.open_sensors()
        descriptors = self.system.msr_descriptors + [self.system.fd]
        target, original_close, closes = self.system.msr_fd, os.close, []
        def close(fd):
            closes.append(fd); original_close(fd)
            if fd == target: raise OSError("injected actual close report")
        fake = FakeSystem(); fake.cap = 35000000; fake.power["desired_limit_uw"] = fake.cap
        fake.sample, fake.close = self.system.sample, self.system.close
        with mock.patch.object(THERMAL.os, "close", side_effect=close), \
                mock.patch.object(THERMAL.os, "pread", return_value=b"bad"):
            result = THERMAL.observe(1, 1, fake, fake.clock, fake.sleep)
        self.assertFalse(result["complete"]); self.assertFalse(result["valid"])
        self.assertIn("actual close report", result["errors"][0])
        self.assertIn("Short thermal MSR", result["errors"][0])
        self.assertIn("elapsed_s", result)
        self.assertCountEqual(closes, descriptors)
        self.assert_msr_fds_closed(descriptors)

    def test_final_close_failure_preserves_prior_samples_and_returns_invalid_capture(self):
        self.qualify_msr_fixture(); self.system.open_sensors()
        descriptors = self.system.msr_descriptors + [self.system.fd]
        target, original_close, closes = self.system.msr_fd, os.close, []
        def close(fd):
            closes.append(fd); original_close(fd)
            if fd == target: raise OSError("injected final close report")
        fake = FakeSystem(); fake.cap = 35000000; fake.power["desired_limit_uw"] = fake.cap
        fake.sample, fake.close = self.system.sample, self.system.close
        with mock.patch.object(THERMAL.os, "close", side_effect=close):
            result = THERMAL.observe(1, 1, fake, fake.clock, fake.sleep)
        self.assertFalse(result["complete"]); self.assertFalse(result["valid"])
        self.assertEqual(len(result["samples"]), 2)
        self.assertIn("final close report", result["errors"][0])
        self.assertEqual(result["elapsed_s"], 1)
        self.assertCountEqual(closes, descriptors)
        self.assert_msr_fds_closed(descriptors)

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
