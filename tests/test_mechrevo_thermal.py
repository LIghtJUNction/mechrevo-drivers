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
