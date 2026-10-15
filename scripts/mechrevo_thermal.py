#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-or-later
"""Read-only GM6PX0X thermal observation with GPU/kernel fault gates."""

import argparse
import array
import fcntl
import json
import math
import os
from pathlib import Path
import re
import subprocess
import sys
import time


IDENTITY = {"sys_vendor": "MECHREVO", "product_name": "Kuangshi16Pro Series GM6PX0X",
            "bios_version": "N.1.09MRO07"}
ZONES = ("intel-rapl:0", "intel-rapl-mmio:0")
# All five requests are _IOR. Pointer size is part of this driver's ABI.
READ_REQUESTS = {"available": 0x80000000 | (array.array("L").itemsize << 16) | (0xec << 8) | 6}
READ_REQUESTS.update({name: 0x80000000 | (array.array("L").itemsize << 16) | (0xef << 8) | number
                      for name, number in (("ec_cpu_c", 0x12), ("ec_gpu_c", 0x13),
                                           ("fan1_raw", 0x10), ("fan2_raw", 0x11))})


class ObservationError(RuntimeError):
    pass


def fault_message(message):
    """Conservatively invalidate any new Xid or uncorrectable PCIe event."""
    return isinstance(message, str) and bool(re.search(
        r"NVRM:.*\bXid\s*\([^)]*\):|GPU has fallen off the bus|"
        r"PCIe Bus Error:.*Uncorrectable|AER:.*(?:Uncorrectable|device recovery failed)",
        message, re.IGNORECASE))


class System:
    def __init__(self, root=Path("/"), runner=subprocess.run, ioctl=fcntl.ioctl):
        self.root, self.runner, self.ioctl = Path(root), runner, ioctl
        self.fd = None
        self.hwmon = None

    def path(self, name):
        return self.root / name.lstrip("/")

    def text(self, name):
        return self.path(name).read_text().strip()

    def command(self, *args):
        result = self.runner(list(args), capture_output=True, text=True, timeout=8)
        if result.returncode:
            raise ObservationError(f"{args[0]} failed: {result.stderr.strip() or result.stdout.strip()}")
        return result.stdout

    def boot_id(self):
        return self.text("/proc/sys/kernel/random/boot_id").replace("-", "")

    def gpu_health(self):
        rows = self.command("nvidia-smi", "--query-gpu=pci.bus_id", "--format=csv,noheader").splitlines()
        if not rows or any(not re.fullmatch(r"[0-9a-fA-F]{4,8}:[0-9a-fA-F]{2}:[0-9a-fA-F]{2}\.[0-7]",
                                          row.strip()) for row in rows):
            raise ObservationError("GPU health probe returned no valid device")
        return [row.strip() for row in rows]

    def journal(self, boot, cursor=None):
        args = ["journalctl", "--boot=" + boot, "--dmesg", "--output=json", "--no-pager", "--quiet"]
        args += ["--after-cursor=" + cursor] if cursor else ["--lines=1"]
        entries = []
        for line in self.command(*args).splitlines():
            try:
                entry = json.loads(line)
            except (ValueError, TypeError) as exc:
                raise ObservationError("Cannot parse kernel journal") from exc
            if (not isinstance(entry, dict) or not isinstance(entry.get("__CURSOR"), str)
                    or not entry["__CURSOR"] or entry.get("_BOOT_ID") != boot
                    or not isinstance(entry.get("MESSAGE"), str)):
                raise ObservationError("Kernel journal omitted cursor, boot identity or message")
            entries.append(entry)
        if cursor is None and not entries:
            raise ObservationError("Cannot establish kernel journal cursor")
        return entries

    def power_status(self):
        value = json.loads(self.command("/usr/bin/mechrevo-power", "status"))
        if not isinstance(value, dict) or value.get("errors") != []:
            raise ObservationError("Power status is not healthy")
        return value

    def open_sensors(self):
        if {k: self.text("/sys/class/dmi/id/" + k) for k in IDENTITY} != IDENTITY:
            raise ObservationError("This host/BIOS is not qualified for these EC readings")
        candidates = [p for p in self.path("/sys/class/hwmon").glob("hwmon*")
                      if (p / "name").read_text().strip() == "coretemp"
                      and (p / "temp1_label").read_text().strip() == "Package id 0"]
        if len(candidates) != 1:
            raise ObservationError("Cannot identify one CPU package temperature sensor")
        self.hwmon = candidates[0]
        self.fd = os.open(self.path("/dev/tuxedo_io"), os.O_RDONLY | os.O_CLOEXEC)
        if self.read_ioctl("available") != 1:
            raise ObservationError("Uniwill interface is unavailable")

    def read_ioctl(self, name):
        value = array.array("i", [0])
        self.ioctl(self.fd, READ_REQUESTS[name], value, True)
        return value[0]

    def sample(self):
        start = time.monotonic()
        package = int((self.hwmon / "temp1_input").read_text())
        result = {"monotonic_s": start, "package_mc": package,
                  "core_max_mc": max(int(p.read_text()) for p in self.hwmon.glob("temp*_input")
                                     if p.name != "temp1_input"),
                  "cpu0_package_throttle_count": int(self.text(
                      "/sys/devices/system/cpu/cpu0/thermal_throttle/package_throttle_count")),
                  "energy_uj": int(self.text("/sys/class/powercap/intel-rapl:0/energy_uj")),
                  "energy_max_uj": int(self.text("/sys/class/powercap/intel-rapl:0/max_energy_range_uj")),
                  "ec": {name: self.read_ioctl(name) for name in READ_REQUESTS if name != "available"},
                  "rapl": {zone: {name: int(self.text("/sys/class/powercap/" + zone + "/" + name))
                                  for name in ("constraint_0_power_limit_uw", "constraint_1_power_limit_uw",
                                               "constraint_0_time_window_us", "constraint_1_time_window_us")}
                           for zone in ZONES}}
        result["package_after_mc"] = int((self.hwmon / "temp1_input").read_text())
        result["read_duration_s"] = time.monotonic() - start
        if not all(0 <= result[k] <= 125000 for k in ("package_mc", "core_max_mc", "package_after_mc")):
            raise ObservationError("CPU temperature is outside the diagnostic range")
        if (not all(0 <= v <= 125 for k, v in result["ec"].items() if k.startswith("ec_"))
                or not all(0 <= result["ec"][k] <= 200 for k in ("fan1_raw", "fan2_raw"))):
            raise ObservationError("EC reading is outside the diagnostic range")
        return result

    def close(self):
        if self.fd is not None:
            os.close(self.fd)
            self.fd = None


def observe(seconds, interval, system=None, clock=time.monotonic, sleeper=time.sleep):
    system = system or System()
    def parameter(value):
        return value if type(value) in (int, float) and math.isfinite(value) else repr(value)
    result = {"schema_version": 1, "complete": False, "valid": False,
              "kind": "read_only_observation", "seconds": parameter(seconds), "interval_s": parameter(interval),
              "errors": [], "faults": [], "samples": []}
    start = clock()
    try:
        if (not all(isinstance(v, (int, float)) and not isinstance(v, bool) and math.isfinite(v)
                    for v in (seconds, interval)) or not 1 <= seconds <= 3600
                or not 0.2 <= interval <= 10 or seconds / interval > 3600):
            raise ObservationError("Use 1–3600 seconds, 0.2–10 second intervals and at most 3601 samples")
        boot = system.boot_id()
        result["boot_id"] = boot
        # Establish the cursor before probing hardware, so faults during preflight
        # are included. Old entries establish position; only later entries gate.
        cursor = system.journal(boot)[-1]["__CURSOR"]
        result["kernel_cursor_start"] = cursor
        result["gpu_before"] = system.gpu_health()
        result["power_before"] = system.power_status()
        before = result["power_before"]
        if before.get("pending_transaction") is not None:
            raise ObservationError("Power configuration has a pending transaction")
        cap = before.get("desired_limit_uw") if before.get("managed") else None
        if cap is not None and (type(cap) is not int or cap <= 0 or before.get("holding_now") is not True):
            raise ObservationError("Managed power ceiling is not confirmed")
        system.open_sensors()

        def check_kernel():
            nonlocal cursor
            if system.boot_id() != boot:
                raise ObservationError("Boot changed during observation")
            entries = system.journal(boot, cursor)
            if entries:
                cursor = entries[-1]["__CURSOR"]
            result["kernel_cursor_last"] = cursor
            faults = [entry for entry in entries if fault_message(entry["MESSAGE"])]
            result["faults"].extend({"message": e["MESSAGE"],
                                     "monotonic_us": e.get("__MONOTONIC_TIMESTAMP")}
                                    for e in faults)
            if faults:
                raise ObservationError("New GPU/PCIe kernel fault; observation invalidated")

        start = clock()
        while True:
            check_kernel()
            sample = system.sample()
            result["samples"].append(sample)
            if cap is not None and any(z["constraint_0_power_limit_uw"] > cap
                                       for z in sample["rapl"].values()):
                raise ObservationError("PL1 exceeded the managed ceiling during observation")
            check_kernel()  # Catch a fault during the EC/thermal read sequence.
            elapsed = clock() - start
            if elapsed >= seconds:
                break
            # Do not busy-loop to catch up when a read takes longer than interval.
            sleeper(min(interval, seconds - elapsed))
        result["gpu_after"] = system.gpu_health()
        if result["gpu_after"] != result["gpu_before"]:
            raise ObservationError("GPU device set changed during observation")
        result["power_after"] = system.power_status()
        after = result["power_after"]
        if (any(after.get(k) != before.get(k) for k in ("managed", "desired_limit_uw", "pending_transaction"))
                or (cap is not None and after.get("holding_now") is not True)):
            raise ObservationError("Power configuration changed during observation")
        check_kernel()
        result["complete"] = result["valid"] = True
    except (ObservationError, OSError, ValueError, subprocess.SubprocessError, KeyboardInterrupt) as exc:
        result["errors"].append(str(exc) or type(exc).__name__)
    finally:
        system.close()
        result["elapsed_s"] = clock() - start
    return result


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--seconds", type=float, default=60)
    parser.add_argument("--interval", type=float, default=1)
    parser.add_argument("--output", type=Path, help="Create a new JSON file; never overwrite an existing file")
    args = parser.parse_args(argv)
    try:
        # Reserve the destination before any hardware probing. Exclusive create
        # refuses existing paths, including symlinks; no automatic directories.
        stream = args.output.open("x", encoding="utf-8") if args.output else None
    except OSError as exc:
        print(str(exc), file=sys.stderr)
        return 2
    try:
        result = observe(args.seconds, args.interval)
        json.dump(result, stream or sys.stdout, indent=2, ensure_ascii=False)
        (stream or sys.stdout).write("\n")
        return 0 if result["valid"] else 1
    finally:
        if stream:
            stream.close()


if __name__ == "__main__":
    raise SystemExit(main())
