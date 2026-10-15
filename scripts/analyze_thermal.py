#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-or-later
"""Summarize saved thermal captures without opening devices or changing policy."""

import argparse
import hashlib
import json
import math
from pathlib import Path
import statistics
import sys


def number(value, name, low=0, high=float("inf"), integer=False):
    if (type(value) not in (int, float) or not math.isfinite(value)
            or not low <= value <= high or (integer and type(value) is not int)):
        raise ValueError("Invalid " + name)
    return value


def summarize(data):
    if not isinstance(data, dict) or not isinstance(data.get("samples"), list):
        raise ValueError("Expected a capture with a samples list")
    modern = (data.get("kind") == "read_only_observation"
              and type(data.get("schema_version")) is int and data["schema_version"] == 1)
    legacy = isinstance(data.get("labels"), dict) and "energy_max_uj" in data and "sample_interval_s" in data
    if not modern and not legacy:
        raise ValueError("Unrecognized capture format")
    if legacy and not modern and data["labels"].get("temp1_input") != "Package id 0":
        raise ValueError("Legacy capture does not identify the CPU package sensor")
    if modern and (not isinstance(data.get("errors"), list)
                   or not all(isinstance(e, str) for e in data["errors"])
                   or not isinstance(data.get("faults"), list)):
        raise ValueError("Observer capture has malformed error or fault records")
    rows = []
    for sample in data["samples"]:
        if modern:
            row = dict(t=sample["monotonic_s"], cpu=sample["package_mc"] / 1000,
                       ec=sample["ec"]["ec_cpu_c"], energy=sample["energy_uj"],
                       modulus=sample["energy_max_uj"], throttle=sample["cpu0_package_throttle_count"],
                       fans=[sample["ec"]["fan1_raw"], sample["ec"]["fan2_raw"]])
            # Validate the unconverted input too, rejecting bool and NaN.
            number(sample["package_mc"], "package_mc", high=125000)
        else:
            number(sample["core_before"]["temp1_input"], "package_mc", high=125000)
            row = dict(t=sample["t"], cpu=sample["core_before"]["temp1_input"] / 1000,
                       ec=sample["ec"]["cpu_temp"]["value"], energy=sample["energy_uj"],
                       modulus=data["energy_max_uj"], throttle=sample["cpu0_throttle"]["package_throttle_count"],
                       fans=[sample["ec"]["fan1_raw"]["value"], sample["ec"]["fan2_raw"]["value"]])
        for key, high in (("t", float("inf")), ("cpu", 125), ("ec", 125)):
            number(row[key], key, high=high)
        for key in ("energy", "modulus", "throttle"):
            number(row[key], key, integer=True)
        if not 0 <= row["energy"] < row["modulus"]:
            raise ValueError("Energy counter outside its range")
        for fan in row["fans"]:
            number(fan, "fan raw", high=200, integer=True)
        if rows and (row["t"] <= rows[-1]["t"] or row["modulus"] != rows[-1]["modulus"]
                     or row["throttle"] < rows[-1]["throttle"]):
            raise ValueError("Time, energy range or throttle counter changed inconsistently")
        rows.append(row)
    result = {"format": "observer_v1" if modern else "legacy",
              "capture_checks_passed": bool(modern and data.get("valid") is True
                  and data.get("complete") is True and data.get("errors") == []
                  and data.get("faults") == [] and len(rows) >= 2),
              "sample_count": len(rows), "capture_errors": data["errors"] if modern else None,
              "limitations": ["Describes one capture; does not prove cooling improvement or root cause.",
                              "Temperature fractions count samples, not time.",
                              "Fan raw values are not RPM or a calibrated percentage.",
                              "Thermal throttle counts are interrupt-handler observations of active internal thermal status, not independent episodes or duration; they do not measure external PROCHOT events.",
                              "coretemp caches temperatures for approximately one second; closely spaced before/after reads may reuse the same cached value.",
                              "Sample-interval average power cannot exclude millisecond PL2 bursts or establish their cause.",
                              "Kernel throttle time counters have filtering and timer-resolution limits; they are not an exact throttling duty cycle.",
                              "Thermal MSR log bits may predate the capture or be cleared by the kernel; absence does not exclude events between samples."]}
    if not rows:
        return result
    cpu = [r["cpu"] for r in rows]
    ec = [r["ec"] for r in rows]
    result.update(cpu_c={"min": min(cpu), "max": max(cpu), "sample_mean": statistics.mean(cpu)},
                  cpu_ge95_samples=sum(v >= 95 for v in cpu),
                  cpu_ge95_sample_fraction=sum(v >= 95 for v in cpu) / len(cpu),
                  cpu_ec_mean_abs_delta_c=statistics.mean(abs(a-b) for a,b in zip(cpu, ec)),
                  fan_raw=[{"min": min(r["fans"][i] for r in rows),
                            "max": max(r["fans"][i] for r in rows),
                            "distinct_values": len(set(r["fans"][i] for r in rows))} for i in (0, 1)])
    if len(rows) < 2:
        return result
    duration = rows[-1]["t"] - rows[0]["t"]
    energy = sum((b["energy"] - a["energy"]) % a["modulus"] for a,b in zip(rows, rows[1:]))
    result.update(sample_span_s=duration, approximate_mean_package_w=energy / duration / 1e6,
                  package_throttle_count_delta=rows[-1]["throttle"] - rows[0]["throttle"])
    result["limitations"].append("Power uses sample timestamps and assumes at most one energy wrap per interval; counter resets cannot be distinguished from wraps.")
    intervals = [b["t"]-a["t"] for a,b in zip(rows, rows[1:])]
    step = statistics.median(intervals)
    if len(rows) >= 10 and all(abs(v-step) <= step * 0.1 for v in intervals):
        shifts = []
        for shift in range(-5, 6):
            a, b = (cpu[-shift:], ec[:shift]) if shift < 0 else (cpu[:len(cpu)-shift], ec[shift:]) if shift else (cpu, ec)
            am, bm = statistics.mean(a), statistics.mean(b)
            denom = math.sqrt(sum((v-am)**2 for v in a) * sum((v-bm)**2 for v in b))
            shifts.append({"ec_shift_samples": shift, "approximate_shift_s": shift * step,
                           "pairs": len(a), "pearson_r": sum((x-am)*(y-bm) for x,y in zip(a,b)) / denom if denom else None,
                           "mean_abs_delta_c": statistics.mean(abs(x-y) for x,y in zip(a,b))})
        result["temperature_shift_scan"] = shifts
        result["limitations"].append("Positive shift pairs later EC samples with earlier CPU samples. Correlation does not establish physical sensor latency; different shifts use different endpoints.")
    else:
        result["limitations"].append("Temperature shift scan omitted: fewer than 10 samples or irregular intervals.")
    return result


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("capture", type=Path)
    args = parser.parse_args(argv)
    try:
        raw = args.capture.read_bytes()
        report = summarize(json.loads(raw))
        report["input_sha256"] = hashlib.sha256(raw).hexdigest()
        json.dump(report, sys.stdout, indent=2, ensure_ascii=False, allow_nan=False)
        sys.stdout.write("\n")
        return 0
    except (OSError, ValueError, KeyError, TypeError, OverflowError, UnicodeError) as exc:
        print("Cannot analyze capture: " + str(exc), file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
