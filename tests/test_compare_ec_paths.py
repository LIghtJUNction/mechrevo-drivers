import copy
from pathlib import Path
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "scripts"))
import compare_ec_paths as probe


class FakeSystem:
    def __init__(self, root):
        self.root = Path(root)
        self.hwmon = self.root / "hwmon"
        self.hwmon.mkdir()
        (self.hwmon / "temp1_input").write_text("94000")
        self.snapshot = self.root / "snapshot"
        self.snapshot.write_text("\n".join(f"{a}={v:02x}" for a, v in zip(
            probe.ADDRESSES, (95, 63, 16, 161, 180, 14, 70, 75, 19, 255, 255, 0))) + "\n")
        self.opened = self.closed = False
        self.values = dict(zip(probe.READS, (95, 63, 170, 170)))

    def path(self, name):
        assert name == probe.SNAPSHOT
        return self.snapshot

    def open_sensors(self):
        self.opened = True

    def close(self):
        self.closed = True

    def read_ioctl(self, name):
        return self.values[name]


class CompareTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.system = FakeSystem(self.tmp.name)

    def test_realistic_transport_difference_is_preserved(self):
        checkpoints, waits = [], []
        result = probe.compare(2, 1, self.system,
                               lambda r: checkpoints.append(copy.deepcopy(r)), waits.append)
        self.assertTrue(result["complete"])
        self.assertFalse(result["thermal_optimization_qualified"])
        self.assertFalse(result["gpu_health_checked"])
        self.assertEqual(len(result["samples"]), 2)
        row = result["samples"][0]
        self.assertEqual(row["ecrr"]["1804"], 255)
        self.assertEqual(row["wmi_before"]["fan1_raw"], 170)
        self.assertTrue(all(row["ecrr_temp_seen_in_wmi"].values()))
        self.assertFalse(checkpoints[0]["complete"])
        self.assertTrue(checkpoints[-1]["complete"])
        self.assertEqual(len(waits), 1)
        self.assertTrue(self.system.closed)

    def test_missing_duplicate_unknown_and_invalid_bytes_rejected(self):
        good = self.system.snapshot.read_text()
        cases = (good.split("\n", 1)[1], good + "043e=01\n", good + "ffff=01\n",
                 good.replace("043e=5f", "043e=100"), good.replace("043e=5f", "043e=zz"))
        for text in cases:
            with self.subTest(text=text), self.assertRaises(probe.ObservationError):
                probe.parse_snapshot(text)

    def test_snapshot_failure_preserves_partial_samples_and_closes(self):
        def invalidate(delay):
            self.system.snapshot.write_text("043e=5f\n")
        result = probe.compare(2, 1, self.system, sleeper=invalidate)
        self.assertFalse(result["complete"])
        self.assertEqual(len(result["samples"]), 1)
        self.assertIn("Incomplete", result["errors"][0])
        self.assertTrue(self.system.closed)

    def test_read_failure_and_interruption_are_recorded(self):
        for failure in (OSError("read failed"), KeyboardInterrupt("stop")):
            with self.subTest(failure=failure):
                def fail():
                    raise failure
                self.system.open_sensors = fail
                result = probe.compare(1, 1, self.system)
                self.assertFalse(result["complete"])
                self.assertEqual(result["errors"], [str(failure)])
                self.assertTrue(self.system.closed)

    def test_invalid_limits_never_open_sensors(self):
        for count, interval in ((0, 1), (31, 1), (True, 1), (1, float("nan")),
                                (1, float("inf")), (1, 0.49), (1, 10.1), (1, True)):
            with self.subTest(count=count, interval=interval), self.assertRaises(probe.ObservationError):
                probe.compare(count, interval, self.system)
        self.assertFalse(self.system.opened)

    def test_wmi_invalid_value_is_not_masked(self):
        self.system.values["ec_cpu_c"] = -5
        result = probe.compare(1, 1, self.system)
        self.assertFalse(result["complete"])
        self.assertIn("byte range", result["errors"][0])

    def test_temperature_disagreement_is_reported_not_fixed(self):
        self.system.values["ec_cpu_c"] = 63
        result = probe.compare(1, 1, self.system)
        self.assertTrue(result["complete"])
        self.assertFalse(result["samples"][0]["ecrr_temp_seen_in_wmi"]["ec_cpu_c"])

    def test_wrong_bios_rejected_before_hardware_or_gpu_calls(self):
        from mechrevo_thermal import IDENTITY, System
        dmi = Path(self.tmp.name) / "sys/class/dmi/id"
        dmi.mkdir(parents=True)
        for key, value in IDENTITY.items():
            (dmi / key).write_text("N.1.09MRO13" if key == "bios_version" else value)
        def forbidden(*args, **kwargs):
            self.fail("Hardware or external command reached before identity gate")
        system = System(root=Path(self.tmp.name), runner=forbidden, ioctl=forbidden)
        result = probe.compare(1, 1, system)
        self.assertFalse(result["complete"])
        self.assertIn("not qualified", result["errors"][0])
        self.assertIsNone(system.fd)


if __name__ == "__main__":
    unittest.main()
