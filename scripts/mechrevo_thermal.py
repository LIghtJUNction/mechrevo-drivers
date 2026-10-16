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
import signal
import stat
import subprocess
import sys
import tempfile
import time


IDENTITY = {"sys_vendor": "MECHREVO", "product_name": "Kuangshi16Pro Series GM6PX0X",
            "bios_version": "N.1.09MRO07"}
ZONES = ("intel-rapl:0", "intel-rapl-mmio:0")
# Architectural thermal status bits: Intel SDM Vol. 3B, thermal management.
# TEMPERATURE_TARGET stays raw: do not infer model-specific TCC offsets.
THERMAL_MSRS = {"core_status": 0x19c, "package_status": 0x1b1,
                "temperature_target": 0x1a2}
THERMAL_STATUS_BITS = {"internal_thermal_active": 0, "internal_thermal_log": 1,
                       "external_prochot_active": 2, "external_prochot_log": 3,
                       "power_limit_active": 10, "power_limit_log": 11}
EXTRA_THROTTLE_FIELDS = ("core_throttle_count", "package_throttle_total_time_ms",
                         "package_throttle_max_time_ms")
# All five requests are _IOR. Pointer size is part of this driver's ABI.
READ_REQUESTS = {"available": 0x80000000 | (array.array("L").itemsize << 16) | (0xec << 8) | 6}
READ_REQUESTS.update({name: 0x80000000 | (array.array("L").itemsize << 16) | (0xef << 8) | number
                      for name, number in (("ec_cpu_c", 0x12), ("ec_gpu_c", 0x13),
                                           ("fan1_raw", 0x10), ("fan2_raw", 0x11))})


class ObservationError(RuntimeError):
    pass


class CheckpointFile:
    """Reserve a new output, then replace only our own file with complete JSON."""

    def __init__(self, path):
        self.path = Path(path).absolute()
        fd = os.open(self.path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_CLOEXEC, 0o600)
        try:
            self.identity = self.file_identity(os.fstat(fd))
        finally:
            os.close(fd)
        self.save({"schema_version": 1, "kind": "read_only_observation",
                   "complete": False, "valid": False, "errors": [], "faults": [], "samples": []})

    @staticmethod
    def file_identity(info):
        return info.st_dev, info.st_ino

    def check_destination(self):
        info = self.path.lstat()
        if not stat.S_ISREG(info.st_mode) or self.file_identity(info) != self.identity:
            raise ObservationError("Output file was replaced externally; refusing overwrite")

    def save(self, result):
        self.check_destination()
        fd, temporary = tempfile.mkstemp(prefix=".mechrevo-thermal-", dir=self.path.parent)
        try:
            with os.fdopen(fd, "w", encoding="utf-8") as stream:
                identity = self.file_identity(os.fstat(stream.fileno()))
                json.dump(result, stream, indent=2, ensure_ascii=False, allow_nan=False)
                stream.write("\n")
                stream.flush()
                os.fsync(stream.fileno())
            self.check_destination()
            # A handled signal between rename and the identity update would
            # make our own new checkpoint look like an external replacement.
            # Defer interruption only for this small publication operation.
            previous_mask = signal.pthread_sigmask(signal.SIG_BLOCK,
                                                  {signal.SIGINT, signal.SIGTERM, signal.SIGHUP})
            try:
                os.replace(temporary, self.path)
                self.identity = identity
            finally:
                signal.pthread_sigmask(signal.SIG_SETMASK, previous_mask)
        finally:
            if os.path.exists(temporary):
                os.unlink(temporary)


def fault_message(message):
    """Conservatively invalidate any new Xid or uncorrectable PCIe event."""
    return isinstance(message, str) and bool(re.search(
        r"NVRM:.*\bXid\s*\([^)]*\):|GPU has fallen off the bus|"
        r"PCIe Bus Error:.*Uncorrectable|AER:.*(?:Uncorrectable|device recovery failed)",
        message, re.IGNORECASE))


class System:
    def __init__(self, root=Path("/"), runner=subprocess.run, ioctl=fcntl.ioctl,
                 read_thermal_msr=False):
        self.root, self.runner, self.ioctl = Path(root), runner, ioctl
        self.fd = None
        self.hwmon = None
        self.rpm_interface_seen = False
        self.read_thermal_msr = read_thermal_msr
        self.msr_fd = None

    def qualify_thermal_msr(self):
        # Linux exposes these CPUID capabilities as /proc/cpuinfo flags:
        # dtherm=CPUID.06H:EAX[0], pln=[4], pts=[6]. Fail closed on ambiguity.
        records = []
        for block in self.text("/proc/cpuinfo").split("\n\n"):
            record = {}
            for line in block.splitlines():
                key, separator, value = line.partition(":")
                if separator:
                    if key.strip() in record:
                        raise ObservationError("Ambiguous CPU capability record")
                    record[key.strip()] = value.strip()
            if record.get("processor") == "0":
                records.append(record)
        if len(records) != 1:
            raise ObservationError("Cannot identify CPU0 thermal MSR capabilities")
        record = records[0]
        if (record.get("vendor_id") != "GenuineIntel" or record.get("cpu family") != "6"
                or record.get("model") != "151"
                or int(record.get("cpuid level", "0")) < 6
                or not {"msr", "dtherm", "pts", "pln"} <= set(record.get("flags", "").split())):
            raise ObservationError("CPU0 is not qualified for thermal MSR readings")

    def thermal_msr(self):
        result = {}
        for name, offset in THERMAL_MSRS.items():
            raw = os.pread(self.msr_fd, 8, offset)
            if len(raw) != 8:
                raise ObservationError(f"Short thermal MSR read at {offset:#x}: {len(raw)} bytes")
            value = int.from_bytes(raw, "little")
            entry = {"offset": offset, "raw": value}
            if name != "temperature_target":
                entry["status"] = {key: bool(value & (1 << bit))
                                   for key, bit in THERMAL_STATUS_BITS.items()}
            result[name] = entry
        return result

    def throttle_details(self):
        result = {}
        for name in EXTRA_THROTTLE_FIELDS:
            try:
                value = int(self.text("/sys/devices/system/cpu/cpu0/thermal_throttle/" + name))
            except FileNotFoundError:
                result[name] = None
                continue
            if value < 0:
                raise ObservationError("Negative thermal throttle counter: " + name)
            result[name] = value
        return result

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
        # Availability history belongs to this capture, not the System lifetime.
        self.rpm_interface_seen = False
        if {k: self.text("/sys/class/dmi/id/" + k) for k in IDENTITY} != IDENTITY:
            raise ObservationError("This host/BIOS is not qualified for these EC readings")
        if self.read_thermal_msr:
            self.qualify_thermal_msr()
            self.msr_fd = os.open(self.path("/dev/cpu/0/msr"), os.O_RDONLY | os.O_CLOEXEC)
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

    def fan_rpm(self):
        """Read the qualified OEM hwmon interface, never infer RPM from duty."""
        candidates = [p for p in self.path("/sys/class/hwmon").glob("hwmon*")
                      if (p / "name").read_text().strip() == "mechrevo_ec"]
        if len(candidates) > 1:
            raise ObservationError("Ambiguous mechrevo_ec RPM sensors")
        absent = {"available": False, "reason": "mechrevo_ec RPM interface absent"}
        if not candidates:
            if self.rpm_interface_seen:
                raise ObservationError("Previously available mechrevo_ec RPM interface disappeared")
            return absent
        sensor = candidates[0]
        # Older temperature-only releases are supported. A partial advertised
        # fan interface is a failure, rather than evidence that RPM is absent.
        if not list(sensor.glob("fan*")):
            if self.rpm_interface_seen:
                raise ObservationError("Previously available mechrevo_ec RPM interface disappeared")
            return absent
        result = {"available": True, "source": "mechrevo_ec"}
        for channel, label, key in ((1, "CPU fan", "cpu_rpm"),
                                    (2, "GPU fan", "gpu_rpm")):
            try:
                actual_label = (sensor / f"fan{channel}_label").read_text().strip()
                raw = (sensor / f"fan{channel}_input").read_text().strip()
            except OSError as exc:
                raise ObservationError(f"Cannot read mechrevo_ec fan{channel} RPM: {exc}") from exc
            if actual_label != label:
                raise ObservationError(f"Unexpected mechrevo_ec fan{channel} label")
            if not re.fullmatch(r"[0-9]+", raw) or len(raw) > 5 or int(raw) > 65535:
                raise ObservationError(f"Invalid mechrevo_ec fan{channel} RPM value")
            result[key] = int(raw)
        self.rpm_interface_seen = True
        return result

    def sample(self):
        previous = None
        def stamp():
            nonlocal previous
            value = time.monotonic()
            try:
                finite = type(value) in (int, float) and math.isfinite(value)
            except OverflowError:
                finite = False
            if (not finite or value < 0 or (previous is not None and value < previous)):
                raise ObservationError("Invalid or decreasing monotonic read timestamp")
            previous = value
            return value

        start = stamp()
        windows = {}
        def read_group(name, reader):
            before = stamp()
            value = reader()
            after = stamp()
            windows[name] = {"start_monotonic_s": before, "end_monotonic_s": after}
            return value

        # Keep the original read order and number of hardware accesses. Bounds
        # describe userspace read calls, not atomic hardware acquisition times.
        result = {"monotonic_s": start, "read_windows": windows}
        result["package_mc"] = read_group("package_before", lambda: int((self.hwmon / "temp1_input").read_text()))
        result["core_max_mc"] = read_group("core_max", lambda: max(
            int(p.read_text()) for p in self.hwmon.glob("temp*_input") if p.name != "temp1_input"))
        result["cpu0_package_throttle_count"] = read_group("throttle_count", lambda: int(self.text(
            "/sys/devices/system/cpu/cpu0/thermal_throttle/package_throttle_count")))
        energy, modulus = read_group("energy", lambda: (
            int(self.text("/sys/class/powercap/intel-rapl:0/energy_uj")),
            int(self.text("/sys/class/powercap/intel-rapl:0/max_energy_range_uj"))))
        result["energy_uj"], result["energy_max_uj"] = energy, modulus
        result["ec"] = read_group("ec", lambda: {
            name: self.read_ioctl(name) for name in READ_REQUESTS if name != "available"})
        result["rapl"] = read_group("rapl", lambda: {
            zone: {name: int(self.text("/sys/class/powercap/" + zone + "/" + name))
                   for name in ("constraint_0_power_limit_uw", "constraint_1_power_limit_uw",
                                "constraint_0_time_window_us", "constraint_1_time_window_us")}
            for zone in ZONES})
        result["fan_rpm"] = read_group("fan_rpm", self.fan_rpm)
        result["package_after_mc"] = read_group("package_after", lambda: int(
            (self.hwmon / "temp1_input").read_text()))
        result["cpu0_throttle_details"] = read_group("throttle_details", self.throttle_details)
        if self.read_thermal_msr:
            result["thermal_msr"] = read_group("thermal_msr", self.thermal_msr)
        result["read_duration_s"] = stamp() - start
        if not all(0 <= result[k] <= 125000 for k in ("package_mc", "core_max_mc", "package_after_mc")):
            raise ObservationError("CPU temperature is outside the diagnostic range")
        if (not all(0 <= v <= 125 for k, v in result["ec"].items() if k.startswith("ec_"))
                or not all(0 <= result["ec"][k] <= 200 for k in ("fan1_raw", "fan2_raw"))):
            raise ObservationError("EC reading is outside the diagnostic range")
        return result

    def close(self):
        if self.msr_fd is not None:
            os.close(self.msr_fd)
            self.msr_fd = None
        if self.fd is not None:
            os.close(self.fd)
            self.fd = None


def observe(seconds, interval, system=None, clock=time.monotonic, sleeper=time.sleep, checkpoint=None):
    system = system or System()
    checkpoint = checkpoint or (lambda result: None)
    def parameter(value):
        return value if type(value) in (int, float) and math.isfinite(value) else repr(value)
    result = {"schema_version": 1, "complete": False, "valid": False,
              "kind": "read_only_observation", "seconds": parameter(seconds), "interval_s": parameter(interval),
              "errors": [], "faults": [], "samples": []}
    start = clock()
    try:
        checkpoint(result)
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
        if type(before.get("managed")) is not bool:
            raise ObservationError("Power status omitted a boolean managed state")
        if before.get("pending_transaction") is not None:
            raise ObservationError("Power configuration has a pending transaction")
        cap = before.get("desired_limit_uw") if before.get("managed") else None
        # Match the power tool's persisted-state range. A missing/null cap is
        # not an unmanaged capture when the tool explicitly reports ownership.
        if before["managed"] and (type(cap) is not int or not 35_000_000 <= cap <= 2_147_483_647
                                  or "pending_transaction" not in before
                                  or before.get("holding_now") is not True):
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

        def check_gpu():
            # The journal can lag behind a failed NVML probe. Do not continue
            # EC sampling until the end of a long capture in that case.
            if system.gpu_health() != result["gpu_before"]:
                raise ObservationError("GPU device set changed during observation")

        start = clock()
        while True:
            check_kernel()
            check_gpu()
            check_kernel()  # Include faults raised while nvidia-smi was running.
            sample = system.sample()
            result["samples"].append(sample)
            if cap is not None and any(z["constraint_0_power_limit_uw"] > cap
                                       for z in sample["rapl"].values()):
                raise ObservationError("PL1 exceeded the managed ceiling during observation")
            check_gpu()
            check_kernel()  # Catch a fault during the EC/thermal read sequence.
            checkpoint(result)
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
    parser.add_argument("--read-thermal-msr", action="store_true",
                        help="Also read qualified CPU0 thermal MSRs; requires an existing readable MSR device")
    args = parser.parse_args(argv)
    try:
        output = CheckpointFile(args.output) if args.output else None
    except (OSError, ObservationError, ValueError) as exc:
        print(str(exc), file=sys.stderr)
        return 2
    previous_handlers = {}

    def interrupted(signum, frame):
        raise KeyboardInterrupt("Interrupted by signal " + str(signum))

    try:
        for signum in (signal.SIGTERM, signal.SIGHUP):
            previous_handlers[signum] = signal.signal(signum, interrupted)
        options = {"system": System(read_thermal_msr=True)} if args.read_thermal_msr else {}
        result = observe(args.seconds, args.interval,
                         checkpoint=output.save if output else None, **options)
        if output:
            output.save(result)
        else:
            json.dump(result, sys.stdout, indent=2, ensure_ascii=False, allow_nan=False)
            sys.stdout.write("\n")
        return 0 if result["valid"] else 1
    except (OSError, ObservationError, ValueError) as exc:
        print("Cannot save observation: " + str(exc), file=sys.stderr)
        return 2
    finally:
        for signum, handler in previous_handlers.items():
            signal.signal(signum, handler)


if __name__ == "__main__":
    raise SystemExit(main())
