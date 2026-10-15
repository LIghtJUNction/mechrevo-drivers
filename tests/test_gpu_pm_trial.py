"""Exercise actual trial ordering with fake hardware and a real filesystem adapter."""
import copy
import io
import json
import os
from pathlib import Path
import signal
import stat
import subprocess
import sys
import tempfile
import time
import unittest
from unittest import mock
from contextlib import redirect_stdout

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0,str(ROOT/"scripts"))
import gpu_pm_trial as PM
sys.path.pop(0)


class Fake:
    def __init__(self):
        self.boot = "a"*32
        self.control = "auto"
        self.runtime = "active"
        self.writes = []
        self.probes = 0
        self.now = 0
        self.pending = []
        self.fail_gpu_from = None
        self.journal_fail = False
        self.write_then_fail = False

    def identity(self): pass
    def boot_id(self): return self.boot
    def gpu_health(self):
        self.probes += 1
        if self.fail_gpu_from and self.probes >= self.fail_gpu_from:
            raise PM.ObservationError("GPU lost")
        return [PM.PCI]
    def journal(self,boot,cursor=None):
        if self.journal_fail: raise PM.ObservationError("Journal unavailable")
        if cursor is None: return [{"__CURSOR":"old","MESSAGE":"NVRM: Xid (PCI:0000:01:00): 79, old"}]
        rows,self.pending = self.pending,[]
        return rows
    def state(self): return {"control":self.control,"runtime_status":self.runtime}
    def set_control(self,value):
        self.writes.append(value)
        self.control = value
        if self.write_then_fail and value == "on": raise OSError("Failed after write")
    def clock(self): return self.now
    def sleep(self,seconds): self.now += seconds


class Trial(unittest.TestCase):
    def setUp(self):
        self.system = Fake()
        self.receipts = []
    def save(self,result): self.receipts.append(copy.deepcopy(result))
    def run_trial(self,seconds=2,save=None,sleeper=None):
        return PM.trial(seconds,self.system,save or self.save,self.system.clock,sleeper or self.system.sleep)

    def test_success_changes_only_render_control_and_restores(self):
        result = self.run_trial()
        self.assertTrue(result["valid"])
        self.assertTrue(result["restored"])
        self.assertEqual(self.system.writes,["on","auto"])
        self.assertEqual(self.system.control,"auto")
        self.assertEqual(len(result["samples"]),3)
        self.assertFalse(self.receipts[0]["write_attempted"])
        self.assertTrue(self.receipts[1]["write_attempted"])
        self.assertEqual(result,self.receipts[-1])
        # No continuous NVML polling during the passive hold interval.
        self.assertEqual(self.system.probes,5)

    def test_unavailable_gpu_prevents_all_writes_and_receipt_creation(self):
        self.system.fail_gpu_from = 1
        result = self.run_trial()
        self.assertFalse(result["valid"])
        self.assertEqual(self.system.writes,[])
        self.assertEqual(self.receipts,[])

    def test_invalid_duration_rejected_without_probing(self):
        for duration in (0,301,True,float('nan'),float('inf')):
            with self.subTest(duration=duration):
                self.system = Fake()
                self.assertFalse(self.run_trial(duration)["valid"])
                self.assertEqual(self.system.probes,0)
                self.assertEqual(self.system.writes,[])

    def test_receipt_failure_before_control_write_is_fail_closed(self):
        for fail_at in (1,2):
            calls = []
            def fail_save(result):
                calls.append(1)
                if len(calls) >= fail_at: raise OSError("Disk full")
            self.system = Fake()
            result = self.run_trial(save=fail_save)
            self.assertFalse(result["valid"])
            self.assertEqual(self.system.writes,[])

    def test_failed_write_with_side_effect_still_restores(self):
        self.system.write_then_fail = True
        result = self.run_trial()
        self.assertFalse(result["valid"])
        self.assertTrue(result["restored"])
        self.assertEqual(self.system.writes,["on","auto"])

    def test_interruption_and_progress_write_failure_restore_healthy_gpu(self):
        def interrupt(seconds): raise KeyboardInterrupt("signal")
        result = self.run_trial(sleeper=interrupt)
        self.assertFalse(result["valid"])
        self.assertTrue(result["restored"])
        self.system = Fake()
        def fail_after_apply(result):
            if result["samples"]: raise OSError("Disk full")
        result = self.run_trial(save=fail_after_apply)
        self.assertFalse(result["valid"])
        self.assertTrue(result["restored"])
        self.assertEqual(self.system.writes,["on","auto"])

    def test_lost_gpu_prevents_restore_power_switch(self):
        self.system.fail_gpu_from = 3
        result = self.run_trial()
        self.assertFalse(result["valid"])
        self.assertFalse(result["restored"])
        self.assertEqual(self.system.writes,["on"])
        self.assertIn("Cleanup verification failed",result["errors"][-1])

    def test_fault_or_gpu_loss_during_restore_invalidates_success(self):
        self.system.fail_gpu_from = 5
        result = self.run_trial()
        self.assertFalse(result["valid"])
        self.assertTrue(result["restored"])
        self.assertEqual(self.system.writes,["on","auto"])
        self.system = Fake()
        original = self.system.set_control
        def fault_on_restore(value):
            original(value)
            if value == "auto": self.system.pending = [{"__CURSOR":"restore-fault","MESSAGE":"NVRM: Xid (PCI:0000:01:00): 79, lost"}]
        self.system.set_control = fault_on_restore
        result = self.run_trial()
        self.assertFalse(result["valid"])
        self.assertTrue(result["restored"])
        self.assertEqual(len(result["faults"]),1)

    def test_new_fault_boot_change_and_external_control_are_not_overwritten(self):
        def fault(seconds):
            self.system.sleep(seconds)
            self.system.pending = [{"__CURSOR":"fault","MESSAGE":"PCIe Bus Error: severity=Uncorrectable (Non-Fatal)"}]
        def reboot(seconds): self.system.boot = "b"*32
        def external(seconds): self.system.control = "auto"
        for change in (fault,reboot,external):
            with self.subTest(change=change.__name__):
                self.system = Fake()
                result = self.run_trial(sleeper=change)
                self.assertFalse(result["valid"])
                self.assertFalse(result["restored"])
                self.assertEqual(self.system.writes,["on"])

    def test_already_on_suspended_or_unreadable_journal_prevents_apply(self):
        for attr,value in (("control","on"),("runtime","suspended"),("journal_fail",True)):
            self.system = Fake(); setattr(self.system,attr,value)
            self.assertFalse(self.run_trial()["valid"])
            self.assertEqual(self.system.writes,[])

    def test_cli_default_is_read_only_and_failed_preflight_creates_no_lock(self):
        for unavailable in (False,True):
            self.system = Fake()
            if unavailable: self.system.fail_gpu_from = 1
            stream = io.StringIO()
            with mock.patch.object(PM,"Hardware",return_value=self.system), mock.patch.object(PM.os,"open") as opened, redirect_stdout(stream):
                self.assertEqual(PM.main([]),1 if unavailable else 0)
                opened.assert_not_called()
            self.assertEqual(json.loads(stream.getvalue())["applied"],False)
            self.assertEqual(self.system.writes,[])

    def test_busy_lock_prevents_trial(self):
        with mock.patch.object(PM,"Hardware",return_value=self.system), mock.patch.object(PM,"BACKUPS",ROOT), mock.patch.object(PM.os,"geteuid",return_value=0), mock.patch.object(PM.os,"open",return_value=99), mock.patch.object(PM.os,"fstat",return_value=mock.Mock(st_uid=0,st_mode=stat.S_IFREG|0o600)), mock.patch.object(PM.os,"close"), mock.patch.object(PM.fcntl,"flock",side_effect=BlockingIOError("busy")), mock.patch.object(PM,"trial") as trial, redirect_stdout(io.StringIO()):
            self.assertEqual(PM.main(["--apply","--output",str(ROOT/"tests/new.json")]),1)
            trial.assert_not_called()
        self.assertEqual(self.system.writes,[])

    def test_real_signals_restore_or_leave_explicit_unfinished_receipt(self):
        # The CLI and receipt implementation are real; only device access and
        # privileged lock location/ownership are replaced inside the child.
        child = r'''
import importlib.util,os,sys
from pathlib import Path
spec=importlib.util.spec_from_file_location('fixture',sys.argv[1])
fixture=importlib.util.module_from_spec(spec);spec.loader.exec_module(fixture)
module=fixture.PM
base=Path(sys.argv[2]);fake=fixture.Fake()
real_set=fake.set_control
def set_control(value):
 real_set(value)
 with (base/'writes.txt').open('a') as f:f.write(value+'\n')
fake.set_control=set_control
module.Hardware=lambda:fake
module.BACKUPS=base
module.os.geteuid=lambda:0
real_open=module.os.open;real_fstat=module.os.fstat
def opened(path,*args,**kwargs):
 if str(path)=='/run/lock/mechrevo-gpu-pm-trial.lock':path=base/'lock'
 return real_open(path,*args,**kwargs)
def fstat(fd):
 fields=list(real_fstat(fd));fields[4]=0
 return os.stat_result(fields)
module.os.open=opened;module.os.fstat=fstat
raise SystemExit(module.main(['--apply','--seconds','60','--output',str(base/'receipt.json')]))
'''
        for signum in (signal.SIGTERM,signal.SIGHUP,signal.SIGKILL):
            with self.subTest(signal=signum), tempfile.TemporaryDirectory(dir=ROOT/"tests",prefix=".gpu-pm-signal-") as directory:
                base = Path(directory)
                process = subprocess.Popen([sys.executable,"-B","-c",child,__file__,directory],stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True)
                try:
                    deadline = time.monotonic()+5
                    while time.monotonic()<deadline:
                        try:
                            if json.loads((base/"receipt.json").read_text()).get("samples"): break
                        except (FileNotFoundError,json.JSONDecodeError): pass
                        if process.poll() is not None: self.fail(process.communicate()[1])
                        time.sleep(0.01)
                    else: self.fail("No checkpoint before signal")
                    process.send_signal(signum)
                    stdout,stderr = process.communicate(timeout=5)
                    self.assertEqual(process.returncode,-signal.SIGKILL if signum==signal.SIGKILL else 1,stderr)
                    result = json.loads((base/"receipt.json").read_text())
                    self.assertFalse(result["valid"])
                    self.assertTrue(result["write_attempted"])
                    self.assertEqual(result["restored"],signum!=signal.SIGKILL,stdout+stderr)
                    self.assertEqual((base/"writes.txt").read_text(),"on\n" if signum==signal.SIGKILL else "on\nauto\n")
                    self.assertEqual((base/"receipt.json").stat().st_mode & 0o777,0o600)
                finally:
                    if process.poll() is None: process.kill()
                    process.communicate(timeout=5)

    def test_signal_between_checkpoint_replace_and_identity_update_allows_final_save(self):
        with tempfile.TemporaryDirectory(dir=ROOT/"tests",prefix=".gpu-pm-checkpoint-") as directory:
            path = Path(directory)/"receipt.json"
            output = PM.CheckpointFile(path)
            original = os.replace
            def replace_then_signal(source,destination):
                original(source,destination)
                os.kill(os.getpid(),signal.SIGTERM)
            def interrupted(signum,frame): raise KeyboardInterrupt("signal")
            previous = signal.signal(signal.SIGTERM,interrupted)
            try:
                with mock.patch.object(os,"replace",side_effect=replace_then_signal):
                    with self.assertRaises(KeyboardInterrupt): output.save({"restored":False})
                output.save({"restored":True})
                self.assertEqual(json.loads(path.read_text()),{"restored":True})
            finally:
                signal.signal(signal.SIGTERM,previous)

    def test_checkpoint_preserves_existing_signal_mask_and_failed_replace_is_retryable(self):
        with tempfile.TemporaryDirectory(dir=ROOT/"tests",prefix=".gpu-pm-mask-") as directory:
            path = Path(directory)/"receipt.json"
            output = PM.CheckpointFile(path)
            previous = signal.pthread_sigmask(signal.SIG_BLOCK,{signal.SIGTERM})
            try:
                with mock.patch.object(os,"replace",side_effect=OSError("rename failed")):
                    with self.assertRaises(OSError): output.save({"restored":False})
                self.assertIn(signal.SIGTERM,signal.pthread_sigmask(signal.SIG_BLOCK,set()))
                output.save({"restored":True})
                self.assertIn(signal.SIGTERM,signal.pthread_sigmask(signal.SIG_BLOCK,set()))
                self.assertEqual(json.loads(path.read_text()),{"restored":True})
            finally:
                signal.pthread_sigmask(signal.SIG_SETMASK,previous)


class Adapter(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(dir=ROOT/"tests",prefix=".gpu-pm-fixture-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        for k,v in PM.IDENTITY.items(): self.write("/sys/class/dmi/id/"+k,v)
        for k,v in {"vendor":"0x10de","device":"0x28a0","power/control":"auto","power/runtime_status":"active"}.items(): self.write(PM.DEVICE+"/"+k,v)
        (self.root/PM.DEVICE.lstrip("/")/"driver").symlink_to(self.root/"nvidia",target_is_directory=True)
        self.hardware = PM.Hardware(self.root)
    def write(self,name,value):
        path = self.root/name.lstrip("/"); path.parent.mkdir(parents=True,exist_ok=True); path.write_text(value)
    def test_adapter_changes_only_control_and_rejects_unknown_gpu(self):
        before = {p:p.read_bytes() for p in self.root.rglob("*") if p.is_file() and not p.is_symlink()}
        self.assertEqual(self.hardware.state(),{"control":"auto","runtime_status":"active"})
        self.hardware.set_control("on")
        changed = [p for p,data in before.items() if p.read_bytes()!=data]
        self.assertEqual(changed,[self.root/PM.DEVICE.lstrip("/")/"power/control"])
        with self.assertRaises(PM.ObservationError): self.hardware.set_control("unsupported")
        self.write(PM.DEVICE+"/device","0x9999")
        with self.assertRaises(PM.ObservationError): self.hardware.set_control("auto")
        self.assertEqual(changed[0].read_text(),"on\n")


if __name__ == "__main__": unittest.main()
