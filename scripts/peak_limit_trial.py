#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-or-later
"""Qualified, bounded CPU PL2 trial with durable, conditional recovery.

Only short-term power limits are written. A GPU/kernel fault inhibits upward
restoration: retain the receipt and recover after health is restored. SIGKILL
cannot run finally; --recover checks the original owner's kernel identity.
"""

import argparse
from contextlib import contextmanager
import json
import math
import os
from pathlib import Path
import signal
import stat
import sys
import time

import mechrevo_power as power
import mechrevo_thermal as thermal


SIGNALS = {signal.SIGINT, signal.SIGTERM, signal.SIGHUP}
LIMIT = "/constraint_1_power_limit_uw"


@contextmanager
def defer_signals():
    previous = signal.pthread_sigmask(signal.SIG_BLOCK, SIGNALS)
    try:
        yield
    finally:
        signal.pthread_sigmask(signal.SIG_SETMASK, previous)


class PeakTrial:
    def __init__(self, controller=None, system=None, clock=time.monotonic, sleeper=time.sleep):
        self.controller = controller or power.PowerController()
        self.env = self.controller.env
        self.system = system or thermal.System()
        self.clock, self.sleeper = clock, sleeper

    def frozen(self):
        state = self.controller.load_state()
        if not state or state.get("transaction"):
            raise power.PowerError("A completed managed PL1 ceiling is required")
        self.controller.qualify(state)
        status = self.controller.status()
        if not status["holding_now"]:
            raise power.PowerError("Managed PL1 ceiling is not holding")
        return {"state_sha256": power.digest(self.env.regular(power.STATE)),
                "host": state["host"], "cap_uw": state["cap_uw"],
                "physical_paths": {z: str(self.env.path(z).resolve()) for z in power.ZONES}}

    def check_frozen(self, result):
        if self.env.boot_id() != result["owner"]["boot_id"]:
            raise power.PowerError("Boot changed; refusing stale hardware recovery")
        if self.frozen() != result["frozen"]:
            raise power.PowerError("Power configuration or physical interface changed")
        for zone in power.ZONES:
            if self.env.read_control(zone + "/constraint_1_name") != "short_term":
                raise power.PowerError("Constraint 1 is not short_term: " + zone)

    def health(self, result):
        if result.get("recovery_inhibited") or result["faults"]:
            raise power.PowerError("Earlier health failure; explicit recovery is required")
        try:
            self._health(result)
        except Exception:
            # A transient NVML/journal failure must not be erased by a later
            # healthy probe in finally. Explicit recovery starts a fresh gate.
            result["recovery_inhibited"] = True
            raise

    def _health(self, result):
        self.check_frozen(result)
        boot = result["owner"]["boot_id"].replace("-", "")
        if self.system.boot_id() != boot:
            raise power.PowerError("Observation boot identity changed")
        entries = self.system.journal(boot, result["kernel_cursor_last"])
        if entries:
            result["kernel_cursor_last"] = entries[-1]["__CURSOR"]
        faults = [e for e in entries if thermal.fault_message(e["MESSAGE"])]
        result["faults"].extend(faults)
        if faults:
            raise power.PowerError("New GPU/PCIe fault; stop trial and defer upward restoration")
        if self.system.gpu_health() != result["gpu_before"]:
            raise power.PowerError("GPU health/device set changed; restoration deferred")
        # Include kernel faults raised while probing NVML, before any write.
        entries = self.system.journal(boot, result["kernel_cursor_last"])
        if entries:
            result["kernel_cursor_last"] = entries[-1]["__CURSOR"]
        faults = [e for e in entries if thermal.fault_message(e["MESSAGE"])]
        result["faults"].extend(faults)
        if faults:
            raise power.PowerError("New GPU/PCIe fault; restoration deferred")

    def restore(self, result, save):
        # Block repeat handled signals while attempting bounded recovery. This
        # does not make hardware writes atomic or protect against SIGKILL.
        with defer_signals():
            def checkpoint():
                try:
                    save(result)
                except Exception as exc:
                    # The durable pre-write snapshot already contains the
                    # originals and attempted flags. A broken output must not
                    # prevent best-effort conditional hardware recovery.
                    result["errors"].append("Recovery checkpoint: " + str(exc))
            try:
                self.health(result)
            except Exception as exc:
                result["errors"].append("Recovery gate: " + str(exc))
                result["restored"] = not any(v["attempted"] for v in result["zones"].values())
                checkpoint()
                return
            for zone, record in result["zones"].items():
                if not record["attempted"]:
                    continue
                try:
                    current = int(self.env.read_control(zone + LIMIT))
                    if current != record["original_uw"]:
                        if current != result["target_uw"]:
                            raise power.PowerError("PL2 changed externally; refusing overwrite: " + zone)
                        # Recheck the qualified configuration and health before
                        # each upward write; the remaining sysfs race is unavoidable.
                        self.health(result)
                        record["restore_attempted"] = True
                        checkpoint()
                        self.env.write_control(zone + LIMIT, record["original_uw"])
                    if int(self.env.read_control(zone + LIMIT)) != record["original_uw"]:
                        raise power.PowerError("PL2 restoration readback failed: " + zone)
                    record["restored"] = True
                except Exception as exc:
                    result["errors"].append(str(exc))
                checkpoint()
            # Independently re-read every attempted interface; an earlier zone
            # may have changed while the second was being restored.
            for zone, record in result["zones"].items():
                if record["attempted"]:
                    try:
                        record["restored"] = int(self.env.read_control(zone + LIMIT)) == record["original_uw"]
                    except Exception as exc:
                        record["restored"] = False
                        result["errors"].append(str(exc))
            result["restored"] = all(not r["attempted"] or r["restored"] for r in result["zones"].values())
            try:
                self.health(result)
            except Exception as exc:
                result["errors"].append("Post-recovery gate: " + str(exc))
            checkpoint()

    def run(self, watts, seconds, save):
        result = {"schema_version": 1, "kind": "cpu_pl2_trial", "complete": False,
                  "valid": False, "restored": False, "errors": [], "faults": [], "samples": [],
                  "zones": {}, "target_uw": watts * 1_000_000 if type(watts) is int else None}
        self.controller.require_root()
        with self.controller.lock():
            try:
                if (type(watts) is not int or not 35 <= watts <= 55 or type(seconds) not in (int, float)
                        or not math.isfinite(seconds) or not 1 <= seconds <= 300):
                    raise power.PowerError("Use explicit 35–55 peak watts and 1–300 seconds")
                result["owner"] = self.env.process_identity()
                result["frozen"] = self.frozen()
                if result["target_uw"] < result["frozen"]["cap_uw"]:
                    raise power.PowerError("PL2 target must be at least the managed PL1 ceiling")
                boot = result["owner"]["boot_id"].replace("-", "")
                result["kernel_cursor_last"] = self.system.journal(boot)[-1]["__CURSOR"]
                result["kernel_cursor_start"] = result["kernel_cursor_last"]
                result["gpu_before"] = self.system.gpu_health()
                self.check_frozen(result)
                for zone in power.ZONES:
                    original = int(self.env.read_control(zone + LIMIT))
                    if not result["target_uw"] < original <= 2_147_483_647:
                        raise power.PowerError("Trial must lower both original PL2 limits")
                    result["zones"][zone] = {"original_uw": original, "attempted": False,
                                             "restore_attempted": False, "restored": False}
                self.system.open_sensors()
                self.health(result)
                save(result)  # Full recovery snapshot before the first write.
                for zone, record in result["zones"].items():
                    self.health(result)
                    if int(self.env.read_control(zone + LIMIT)) != record["original_uw"]:
                        raise power.PowerError("Original PL2 changed before trial write")
                    # Publish intent first, including the write-failure case.
                    with defer_signals():
                        record["attempted"] = True
                        save(result)
                        self.env.write_control(zone + LIMIT, result["target_uw"])
                    if int(self.env.read_control(zone + LIMIT)) != result["target_uw"]:
                        raise power.PowerError("PL2 trial write/readback failed: " + zone)
                start = self.clock()
                while True:
                    self.health(result)
                    sample = self.system.sample()
                    result["samples"].append(sample)
                    for zone in power.ZONES:
                        observed = sample["rapl"][zone.rsplit("/", 1)[-1]]
                        if not 0 < observed["constraint_0_power_limit_uw"] <= result["frozen"]["cap_uw"]:
                            raise power.PowerError("Sample PL1 exceeded the managed ceiling")
                        if observed["constraint_1_power_limit_uw"] != result["target_uw"]:
                            raise power.PowerError("Sample PL2 does not match the trial target")
                    if any(int(self.env.read_control(z + LIMIT)) != result["target_uw"] for z in power.ZONES):
                        raise power.PowerError("PL2 trial limit changed during observation")
                    self.health(result)
                    save(result)
                    elapsed = self.clock() - start
                    if elapsed >= seconds:
                        break
                    self.sleeper(min(1, seconds - elapsed))
                result["complete"] = True
            except (Exception, KeyboardInterrupt) as exc:
                result["errors"].append(str(exc) or type(exc).__name__)
            finally:
                # Sensor teardown must not bypass conditional PL2 recovery or
                # hide a prior observation failure. Each stage remains best effort.
                try:
                    self.system.close()
                except (Exception, KeyboardInterrupt) as exc:
                    result["errors"].append("Sensor close: " + (str(exc) or type(exc).__name__))
                try:
                    if len(result["zones"]) == len(power.ZONES):
                        self.restore(result, save)
                    else:
                        result["restored"] = True  # No hardware write has occurred.
                except (Exception, KeyboardInterrupt) as exc:
                    # A failed recovery cannot establish aggregate restoration,
                    # even if it updated some zone records before raising.
                    result["restored"] = False
                    result["errors"].append("Recovery: " + (str(exc) or type(exc).__name__))
                result["valid"] = result["complete"] and result["restored"] and not result["errors"]
                try:
                    save(result)
                except (Exception, KeyboardInterrupt) as exc:
                    # Only the returned result/CLI can report this last failure;
                    # do not claim that these errors were durably checkpointed.
                    result["valid"] = False
                    result["errors"].append("Final checkpoint: " + (str(exc) or type(exc).__name__))
        return result

    def read_recovery(self, path):
        path = Path(path).absolute()
        with open(path, "rb", opener=lambda name, flags: os.open(name, flags | os.O_NOFOLLOW)) as stream:
            info = os.fstat(stream.fileno())
            if not stat.S_ISREG(info.st_mode) or info.st_uid != self.env.owner_uid or info.st_mode & 0o022:
                raise power.PowerError("Untrusted recovery receipt")
            value = json.load(stream)
        if (type(value["schema_version"]) is not int or value["schema_version"] != 1 or value["kind"] != "cpu_pl2_trial"
                or type(value["target_uw"]) is not int or not 35_000_000 <= value["target_uw"] <= 55_000_000
                or set(value["zones"]) != set(power.ZONES) or not isinstance(value["errors"], list)
                or not isinstance(value["faults"], list) or not isinstance(value["gpu_before"], list)
                or not value["gpu_before"] or not isinstance(value["kernel_cursor_last"], str)
                or not value["kernel_cursor_last"]):
            raise power.PowerError("Invalid recovery schema")
        owner = value["owner"]
        if (set(owner) != {"pid", "start_ticks", "boot_id"} or type(owner["pid"]) is not int
                or owner["pid"] <= 0 or type(owner["start_ticks"]) is not int or owner["start_ticks"] < 0
                or not isinstance(owner["boot_id"], str) or not owner["boot_id"]):
            raise power.PowerError("Invalid recovery owner")
        frozen = value["frozen"]
        if (not power.PowerController.valid_hash(frozen["state_sha256"])
                or frozen["host"] != power.IDENTITY or type(frozen["cap_uw"]) is not int
                or not 35_000_000 <= frozen["cap_uw"] <= value["target_uw"]
                or set(frozen["physical_paths"]) != set(power.ZONES)):
            raise power.PowerError("Invalid recovery qualification")
        for record in value["zones"].values():
            if (type(record["original_uw"]) is not int
                    or not value["target_uw"] < record["original_uw"] <= 2_147_483_647
                    or any(type(record[k]) is not bool for k in ("attempted", "restore_attempted", "restored"))):
                raise power.PowerError("Invalid recovery PL2 snapshot")
        return value

    def recover(self, path, save):
        self.controller.require_root()
        with self.controller.lock():
            result = self.read_recovery(path)
            if result["owner"]["boot_id"] != self.env.boot_id():
                raise power.PowerError("Receipt is from another boot; refusing stale recovery")
            if self.env.process_identity(result["owner"]["pid"]) == result["owner"]:
                raise power.PowerError("Trial owner is still running")
            result["recovery_source"] = str(Path(path).absolute())
            result["recovery_owner"] = self.env.process_identity()
            result["errors"] = []
            result["faults"] = []
            result["recovery_inhibited"] = False
            result["valid"] = False
            # Historical faults remain in the source receipt. Establish a fresh
            # cursor only after dead-owner/boot/config verification; NVML must
            # now be healthy before recovering the original limits.
            self.check_frozen(result)
            result["kernel_cursor_last"] = self.system.journal(
                result["owner"]["boot_id"].replace("-", ""))[-1]["__CURSOR"]
            save(result)
            self.restore(result, save)
            result["valid"] = result["restored"] and not result["errors"]
            save(result)
            return result


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--peak-watts", type=int)
    parser.add_argument("--seconds", type=float)
    parser.add_argument("--recover", type=Path)
    parser.add_argument("--read-thermal-msr", action="store_true", help="Include read-only CPU thermal MSR samples")
    parser.add_argument("--output", type=Path, required=True, help="New receipt; never replaces the source")
    args = parser.parse_args(argv)
    if (args.recover and (args.peak_watts is not None or args.seconds is not None)) or (
            not args.recover and (args.peak_watts is None or args.seconds is None)):
        parser.error("Use --peak-watts and --seconds, or --recover, plus --output")
    handlers = {}

    def interrupted(signum, frame):
        raise KeyboardInterrupt("Interrupted by signal " + str(signum))

    try:
        trial = PeakTrial(system=thermal.System(read_thermal_msr=True)) if args.read_thermal_msr else PeakTrial()
        trial.controller.require_root()  # Fail before reserving an output.
        output = thermal.CheckpointFile(args.output)
        for signum in SIGNALS:
            handlers[signum] = signal.signal(signum, interrupted)
        result = trial.recover(args.recover, output.save) if args.recover else trial.run(
            args.peak_watts, args.seconds, output.save)
        print(json.dumps({k: result[k] for k in ("valid", "restored", "errors")}))
        return 0 if result["valid"] else 1
    except (Exception, KeyboardInterrupt) as exc:
        print(str(exc) or type(exc).__name__, file=sys.stderr)
        return 2
    finally:
        for signum, handler in handlers.items():
            signal.signal(signum, handler)


if __name__ == "__main__":
    raise SystemExit(main())
