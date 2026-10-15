"""Offline analysis must preserve failed-capture status and counter semantics."""
import copy
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import unittest
from unittest import mock
from contextlib import redirect_stderr, redirect_stdout

SPEC = importlib.util.spec_from_file_location("analyzer", Path(__file__).resolve().parents[1] / "scripts/analyze_thermal.py")
ANALYZER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(ANALYZER)


def capture(temperatures=(80, 96, 100), times=(0, 1, 3)):
    return {"schema_version": 1, "kind": "read_only_observation", "valid": True,
            "complete": True, "errors": [], "faults": [], "samples": [
                {"monotonic_s": t, "package_mc": c * 1000,
                 "ec": {"ec_cpu_c": c-2, "fan1_raw": 170-i, "fan2_raw": 160+i},
                 "energy_uj": int(t * 40e6), "energy_max_uj": 1_000_000_000,
                 "cpu0_package_throttle_count": 10+i} for i,(t,c) in enumerate(zip(times,temperatures))]}


class Analysis(unittest.TestCase):
    def test_weighted_power_and_sample_fraction_are_distinct_from_duration(self):
        data = capture()
        data["samples"][1]["energy_uj"] = 20_000_000
        original = copy.deepcopy(data)
        report = ANALYZER.summarize(data)
        self.assertTrue(report["capture_checks_passed"])
        self.assertEqual(report["approximate_mean_package_w"], 40)
        self.assertEqual(report["cpu_ge95_sample_fraction"], 2/3)
        self.assertEqual(report["package_throttle_count_delta"], 2)
        self.assertEqual(report["cpu_ec_mean_abs_delta_c"], 2)
        self.assertEqual(report["fan_raw"][0], {"min":168, "max":170, "distinct_values":3})
        self.assertEqual(data, original)
        self.assertNotIn("temperature_shift_scan", report)

    def test_energy_wrap_is_accounted_for(self):
        data = capture((80, 81), (0, 2))
        for sample,energy in zip(data["samples"], (950_000_000, 30_000_000)):
            sample["energy_uj"] = energy
        self.assertEqual(ANALYZER.summarize(data)["approximate_mean_package_w"], 40)

    def test_failed_incomplete_faulted_and_empty_captures_never_become_qualified(self):
        for change in ({"valid":False}, {"complete":False}, {"errors":["GPU failed"]},
                       {"faults":[{"message":"Xid 79"}]}, {"samples":[]}, {"valid":1}):
            with self.subTest(change=change):
                data = capture(); data.update(change)
                report = ANALYZER.summarize(data)
                self.assertFalse(report["capture_checks_passed"])
                json.dumps(report, allow_nan=False)

    def test_legacy_data_is_supported_but_never_marked_qualified(self):
        modern = capture()
        data = {"labels":{"temp1_input":"Package id 0"}, "energy_max_uj":1_000_000_000,
                "sample_interval_s":1, "samples":[
                    {"t":s["monotonic_s"], "core_before":{"temp1_input":s["package_mc"]},
                     "ec":{"cpu_temp":{"value":s["ec"]["ec_cpu_c"]},
                           "fan1_raw":{"value":s["ec"]["fan1_raw"]},
                           "fan2_raw":{"value":s["ec"]["fan2_raw"]}},
                     "energy_uj":s["energy_uj"], "cpu0_throttle":{"package_throttle_count":s["cpu0_package_throttle_count"]}}
                    for s in modern["samples"]]}
        report = ANALYZER.summarize(data)
        self.assertEqual(report["format"], "legacy")
        self.assertFalse(report["capture_checks_passed"])
        self.assertEqual(report["approximate_mean_package_w"], 40)
        data["labels"]["temp1_input"] = "Another sensor"
        with self.assertRaises(ValueError): ANALYZER.summarize(data)

    def test_invalid_counters_times_and_values_are_rejected(self):
        changes = (("monotonic_s",0), ("monotonic_s",float('nan')), ("energy_max_uj",0),
                   ("energy_max_uj",500_000_000), ("energy_uj",1_000_000_000),
                   ("energy_uj",True), ("package_mc",True), ("package_mc",float('inf')),
                   ("cpu0_package_throttle_count",9))
        for key,value in changes:
            with self.subTest(key=key,value=value):
                data = capture(); data["samples"][1][key] = value
                with self.assertRaises(ValueError): ANALYZER.summarize(data)

    def test_shift_direction_constant_series_and_irregular_samples(self):
        cpu = [80,90,75,95,83,87,92,79,94,82,85,91,76,97,84,89]
        data = capture(cpu, list(range(len(cpu))))
        for i,s in enumerate(data["samples"]): s["ec"]["ec_cpu_c"] = cpu[max(i-1,0)]
        scan = ANALYZER.summarize(data)["temperature_shift_scan"]
        best = max(scan, key=lambda r:r["pearson_r"])
        self.assertEqual(best["ec_shift_samples"],1)
        self.assertAlmostEqual(best["pearson_r"],1)
        for s in data["samples"]: s["ec"]["ec_cpu_c"] = 80
        self.assertTrue(all(r["pearson_r"] is None for r in ANALYZER.summarize(data)["temperature_shift_scan"]))
        data["samples"][-1]["monotonic_s"] += 3
        self.assertNotIn("temperature_shift_scan", ANALYZER.summarize(data))

    def test_cli_failure_is_nonzero_and_success_is_strict_json(self):
        malformed = capture(); malformed["errors"] = [float('nan')]
        for content,code in (("broken",1), ('{"samples":[]}',1), (json.dumps(malformed),1), (json.dumps(capture()),0)):
            out,err = io.StringIO(),io.StringIO()
            with mock.patch.object(Path,"read_bytes",return_value=content.encode()), redirect_stdout(out), redirect_stderr(err):
                self.assertEqual(ANALYZER.main(["capture.json"]),code)
            if code: self.assertEqual(out.getvalue(), "")
            else:
                self.assertEqual(json.loads(out.getvalue())["sample_count"],3)
                self.assertEqual(json.loads(out.getvalue())["input_sha256"],hashlib.sha256(content.encode()).hexdigest())

    def test_empty_single_sample_and_out_of_range_ec_values(self):
        data = capture(); data["samples"] = []
        report = ANALYZER.summarize(data)
        self.assertEqual(report["sample_count"],0)
        self.assertNotIn("cpu_c",report)
        data = capture((90,), (0,))
        report = ANALYZER.summarize(data)
        self.assertFalse(report["capture_checks_passed"])
        self.assertNotIn("approximate_mean_package_w",report)
        for key,value in (("ec_cpu_c",126), ("fan1_raw",201), ("fan2_raw",True)):
            with self.subTest(key=key):
                broken = capture(); broken["samples"][0]["ec"][key] = value
                with self.assertRaises(ValueError): ANALYZER.summarize(broken)


if __name__ == "__main__":
    unittest.main()
