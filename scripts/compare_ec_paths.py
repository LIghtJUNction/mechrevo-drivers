#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-or-later
"""Compare fixed ECRR diagnostic bytes with existing read-only WMI ioctls."""

import argparse
import math
from pathlib import Path
import signal
import time

from mechrevo_thermal import CheckpointFile, ObservationError, System


SNAPSHOT = "/sys/module/mechrevo_ec_probe/snapshot"
ADDRESSES = ("043e", "044f", "0740", "0765", "0766", "0464", "0465",
             "046b", "046c", "1804", "1809", "0e0d")
READS = ("ec_cpu_c", "ec_gpu_c", "fan1_raw", "fan2_raw")


def parse_snapshot(text):
    values = {}
    for line in text.splitlines():
        fields = line.split("=")
        if (len(fields) != 2 or fields[0] not in ADDRESSES or fields[0] in values
                or len(fields[1]) != 2
                or any(c not in "0123456789abcdef" for c in fields[1])):
            raise ObservationError("Malformed or unexpected ECRR snapshot")
        values[fields[0]] = int(fields[1], 16)
    if set(values) != set(ADDRESSES):
        raise ObservationError("Incomplete ECRR snapshot")
    return values


def sample(system):
    started = time.monotonic()
    package_before = int((system.hwmon / "temp1_input").read_text())
    before = {name: system.read_ioctl(name) for name in READS}
    direct = parse_snapshot(system.path(SNAPSHOT).read_text())
    after = {name: system.read_ioctl(name) for name in READS}
    package_after = int((system.hwmon / "temp1_input").read_text())
    if any(type(v) is not int or not 0 <= v <= 255 for v in (*before.values(), *after.values())):
        raise ObservationError("WMI reading is outside byte range")
    if not all(0 <= v <= 125000 for v in (package_before, package_after)):
        raise ObservationError("CPU package temperature is outside diagnostic range")
    return {"monotonic_s": started, "read_duration_s": time.monotonic() - started,
            "package_before_mc": package_before, "package_after_mc": package_after,
            "wmi_before": before, "ecrr": direct, "wmi_after": after,
            "ecrr_temp_seen_in_wmi": {
                name: direct[addr] in (before[name], after[name])
                for name, addr in (("ec_cpu_c", "043e"), ("ec_gpu_c", "044f"))}}


def compare(samples, interval, system=None, checkpoint=lambda result: None, sleeper=time.sleep):
    if type(samples) is not int or not 1 <= samples <= 30:
        raise ObservationError("Samples must be an integer from 1 to 30")
    if type(interval) not in (int, float) or not math.isfinite(interval) or not 0.5 <= interval <= 10:
        raise ObservationError("Interval must be finite and between 0.5 and 10 seconds")
    system = system or System()
    result = {"schema_version": 1, "kind": "ec_transport_diagnostic", "complete": False,
              "thermal_optimization_qualified": False, "gpu_health_checked": False,
              "samples": [], "errors": []}
    try:
        checkpoint(result)
        system.open_sensors()  # Exact host/BIOS gate and available-interface check.
        for i in range(samples):
            row = sample(system)
            result["samples"].append(row)
            checkpoint(result)
            if i + 1 < samples:
                sleeper(max(0, interval - row["read_duration_s"]))
        result["complete"] = True
    except (OSError, ValueError, ObservationError, KeyboardInterrupt) as exc:
        result["errors"].append(str(exc) or type(exc).__name__)
    finally:
        system.close()
        checkpoint(result)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True, help="New JSON file; refuses overwrite")
    parser.add_argument("--samples", type=int, default=10)
    parser.add_argument("--interval", type=float, default=1)
    args = parser.parse_args()
    def interrupted(signum, frame):
        raise KeyboardInterrupt("Interrupted by signal " + str(signum))
    for sig in (signal.SIGINT, signal.SIGTERM, signal.SIGHUP):
        signal.signal(sig, interrupted)
    try:
        output = CheckpointFile(args.output)
        result = compare(args.samples, args.interval, checkpoint=output.save)
    except (OSError, ObservationError) as exc:
        parser.exit(1, str(exc) + "\n")
    print(f"Saved {len(result['samples'])} samples to {args.output}; complete={result['complete']}")
    return 0 if result["complete"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
