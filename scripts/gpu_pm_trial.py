#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-or-later
"""Experimental, reversible runtime-PM isolation for the qualified GM6PX0X."""
import argparse
import fcntl
import json
import math
import os
from pathlib import Path
import signal
import stat
import sys
import time

from mechrevo_thermal import CheckpointFile, IDENTITY, ObservationError, System, fault_message

PCI = "0000:01:00.0"
DEVICE = "/sys/bus/pci/devices/" + PCI
BACKUPS = Path("/home/lightjunction/Backups")


class Hardware(System):
    def identity(self):
        if {k:self.text("/sys/class/dmi/id/"+k) for k in IDENTITY} != IDENTITY:
            raise ObservationError("Unqualified host or BIOS")
        if (self.text(DEVICE+"/vendor"),self.text(DEVICE+"/device")) != ("0x10de","0x28a0"):
            raise ObservationError("Unexpected GPU identity")
        if self.path(DEVICE+"/driver").resolve().name != "nvidia":
            raise ObservationError("GPU is not bound to nvidia")

    def state(self):
        self.identity()
        return {k:self.text(DEVICE+"/power/"+k) for k in ("control","runtime_status")}

    def set_control(self,value):
        if value not in ("auto","on"):
            raise ObservationError("Unsupported runtime PM control value")
        self.identity()
        self.path(DEVICE+"/power/control").write_text(value+"\n")


def check_gpu(system):
    if system.gpu_health() not in ([PCI], ["00000000:01:00.0"]):
        raise ObservationError("Unexpected accessible GPU set")


def preflight(system):
    system.identity()
    boot = system.boot_id()
    cursor = system.journal(boot)[-1]["__CURSOR"]
    check_gpu(system)
    before = system.state()
    if before != {"control":"auto","runtime_status":"active"}:
        raise ObservationError("Trial requires a healthy active GPU with control=auto")
    return boot,cursor,before


def trial(seconds, system, save, clock=time.monotonic, sleeper=time.sleep):
    result = {"kind":"experimental_gpu_runtime_pm_trial", "valid":False,
              "write_attempted":False, "restored":False, "errors":[], "faults":[], "samples":[]}
    attempted = False
    boot = cursor = None
    def check_logs():
        nonlocal cursor
        if system.boot_id() != boot:
            raise ObservationError("Boot changed; refusing further power operations")
        entries = system.journal(boot,cursor)
        if entries: cursor = entries[-1]["__CURSOR"]
        faults = [e["MESSAGE"] for e in entries if fault_message(e["MESSAGE"])]
        result["faults"].extend(faults)
        if result["faults"]:
            raise ObservationError("New GPU/PCIe fault; refusing further power operations")

    try:
        if type(seconds) not in (int,float) or not math.isfinite(seconds) or not 1 <= seconds <= 300:
            raise ObservationError("Use 1–300 seconds")
        boot,cursor,before = preflight(system)
        result.update(boot_id=boot,pci=PCI,before=before,seconds=seconds,
                      restore_command="echo auto > "+DEVICE+"/power/control")
        save(result)  # Recovery receipt must be durable before the only control write.
        check_logs()
        if system.state() != before:
            raise ObservationError("GPU state changed before trial")
        check_gpu(system)
        check_logs()
        result["write_attempted"] = True
        save(result)  # A crash from here leaves an explicitly uncertain transaction.
        attempted = True
        system.set_control("on")
        start = clock()
        while True:
            check_logs()
            state = system.state()
            if state != {"control":"on","runtime_status":"active"}:
                raise ObservationError("GPU power state changed externally or pinning failed")
            result["samples"].append(dict(state,monotonic_s=clock()))
            save(result)
            elapsed = clock()-start
            if elapsed >= seconds: break
            sleeper(min(1,seconds-elapsed))
        check_gpu(system)
        check_logs()
        result["valid"] = True
    except (Exception, KeyboardInterrupt) as exc:
        result["errors"].append(str(exc) or type(exc).__name__)
    finally:
        if attempted:
            try:
                # Never try to wake/reset a lost device as part of cleanup.
                check_logs()
                check_gpu(system)
                check_logs()
                current = system.state()
                if current == {"control":"on","runtime_status":"active"}:
                    system.set_control("auto")
                    if system.state()["control"] != "auto":
                        raise ObservationError("Restoration readback failed")
                    result["restored"] = True
                    check_logs()
                    check_gpu(system)
                    check_logs()
                else:
                    raise ObservationError("Power control changed; refusing to overwrite external state")
            except (Exception, KeyboardInterrupt) as exc:
                result["errors"].append("Cleanup verification failed: "+(str(exc) or type(exc).__name__))
                result["valid"] = False
        try:
            # No file is created when preflight failed.
            if boot is not None: save(result)
        except (Exception, KeyboardInterrupt) as exc:
            result["errors"].append("Final receipt failed: "+(str(exc) or type(exc).__name__))
            result["valid"] = False
    return result


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--apply",action="store_true",help="Run the temporary trial; default only checks readiness")
    parser.add_argument("--seconds",type=float,default=60)
    parser.add_argument("--output",type=Path,help="New recovery receipt below ~/Backups; required with --apply")
    args = parser.parse_args(argv)
    system = Hardware()
    handlers = {}
    def interrupted(signum,frame): raise KeyboardInterrupt("Interrupted by signal "+str(signum))
    try:
        # A lost GPU is rejected before lock/receipt creation or any control write.
        boot,cursor,before = preflight(system)
        if not args.apply:
            print(json.dumps({"ready":True,"applied":False,"boot_id":boot,"before":before}))
            return 0
        if os.geteuid() != 0:
            raise ObservationError("Apply requires root")
        if args.output is None or not args.output.parent.resolve(strict=True).is_relative_to(BACKUPS.resolve(strict=True)):
            raise ObservationError("Use a new --output file in an existing directory below ~/Backups")
        fd = os.open("/run/lock/mechrevo-gpu-pm-trial.lock",os.O_CREAT|os.O_RDWR|os.O_CLOEXEC|os.O_NOFOLLOW,0o600)
        try:
            info = os.fstat(fd)
            if info.st_uid != 0 or not stat.S_ISREG(info.st_mode) or info.st_mode & 0o022:
                raise ObservationError("Unsafe trial lock")
            fcntl.flock(fd,fcntl.LOCK_EX|fcntl.LOCK_NB)
            for sig in (signal.SIGTERM,signal.SIGHUP): handlers[sig] = signal.signal(sig,interrupted)
            output = None
            def save(result):
                nonlocal output
                if output is None: output = CheckpointFile(args.output)
                output.save(result)
            result = trial(args.seconds,system,save)
            print(json.dumps(result,indent=2,allow_nan=False))
            return 0 if result["valid"] and result["restored"] else 1
        finally:
            for sig,handler in handlers.items(): signal.signal(sig,handler)
            os.close(fd)
    except (Exception, KeyboardInterrupt) as exc:
        print(json.dumps({"ready":False,"applied":False,"error":str(exc) or type(exc).__name__}))
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
