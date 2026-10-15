"""Exercise the userspace power-cap transaction against an isolated host tree.

No service command or sysfs operation escapes the fake root.  Faults are
injected at the service/kernel I/O boundary, while controller decisions and
configuration transactions run normally.
"""

import importlib.util
import hashlib
import io
from contextlib import contextmanager, redirect_stdout, redirect_stderr
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import unittest
from unittest import mock
import uuid
import xml.etree.ElementTree as ET


ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("mechrevo_power", ROOT / "scripts/mechrevo_power.py")
POWER = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = POWER
SPEC.loader.exec_module(POWER)


STOCK_UNIT = """[Unit]
Description=Thermal Daemon Service
ConditionVirtualization=no

[Service]
Type=dbus
SuccessExitStatus=2
BusName=org.freedesktop.thermald
ExecStart=/usr/bin/thermald --systemd --dbus-enable --adaptive
Restart=on-failure

[Install]
WantedBy=multi-user.target
Alias=dbus-org.freedesktop.thermald.service
"""


class FakeHost:
    def __init__(self, testcase, root=None):
        if root is None:
            directory = ROOT / ".local-tests"
            directory.mkdir(exist_ok=True)
            self.temp = tempfile.TemporaryDirectory(prefix="power-cli-", dir=directory)
            testcase.addCleanup(self.temp.cleanup)
            self.root = Path(self.temp.name)
        else:
            self.root = Path(root)
        self.commands = []
        self.version = "2.5.13"
        self.active = True
        self.restart_failures = []
        self.apply_errors = []
        self.service_overrides = {}
        self.controller = None
        self.write("/sys/class/dmi/id/sys_vendor", "MECHREVO\n")
        self.write("/sys/class/dmi/id/product_name", "Kuangshi16Pro Series GM6PX0X\n")
        self.write("/sys/class/dmi/id/bios_version", "N.1.09MRO07\n")
        self.write("/proc/cpuinfo", "processor\t: 0\nmodel name\t: 12th Gen Intel(R) Core(TM) i9-12900HX\n")
        self.write(POWER.STOCK_UNIT, STOCK_UNIT)
        binary = self.write("/usr/bin/thermald", "isolated thermald v2.5.13 fixture\n")
        binary.chmod(0o755)
        qualified = mock.patch.object(POWER, "QUALIFIED_THERMALD_SHA256",
                                      hashlib.sha256(binary.read_bytes()).hexdigest())
        qualified.start()
        testcase.addCleanup(qualified.stop)
        self.write("/etc/thermald/thermald-features.xml", """<ThermaldFeatures>
<DbusControl>0</DbusControl><XMLThermalConfig>1</XMLThermalConfig>
<DataVaultFromFileSystem>1</DataVaultFromFileSystem>
<KobjectUeventSupport>1</KobjectUeventSupport>
</ThermaldFeatures>
""")
        self.write("/sys/bus/pci/devices/0000:00:04.0/tcc_offset_degree_celsius", "0\n")
        for zone in POWER.ZONES:
            for name, value in {
                "name": "package-0", "enabled": "1",
                "constraint_0_name": "long_term",
                "constraint_0_power_limit_uw": "140000000",
                "constraint_0_max_power_uw": "55000000",
                "constraint_0_time_window_us": "55967744",
                "constraint_1_name": "short_term",
                "constraint_1_power_limit_uw": "140000000",
                "constraint_1_time_window_us": "2440",
                "constraint_2_name": "peak_power",
                "constraint_2_power_limit_uw": "0",
            }.items():
                self.write(f"{zone}/{name}", value + "\n")
        self.env = POWER.Environment(root=self.root, runner=self.run,
                                     uid=0, owner_uid=os.geteuid())
        self.controller = POWER.PowerController(self.env)

    def path(self, absolute):
        return self.root / str(absolute).lstrip("/")

    def write(self, absolute, value):
        path = self.path(absolute)
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(value)
        return path

    def read(self, absolute):
        return self.path(absolute).read_text()

    def limit_path(self, index):
        return self.path(f"{POWER.ZONES[index]}/constraint_0_power_limit_uw")

    def limits(self):
        return [int(self.limit_path(i).read_text()) for i in range(2)]

    def protected_controls(self):
        return {str(path.relative_to(self.root)): path.read_bytes()
                for zone in POWER.ZONES for path in self.path(zone).iterdir()
                if path.name != "constraint_0_power_limit_uw"}

    def run(self, argv):
        self.commands.append(list(argv))
        if argv == ["/usr/bin/thermald", "--version"]:
            return subprocess.CompletedProcess(argv, 0, self.version + "\n", "")
        if argv[:3] == ["systemctl", "show", "thermald.service"]:
            command = "/usr/bin/thermald --systemd --dbus-enable --adaptive"
            dropins = ""
            if self.path(POWER.DROPIN).exists():
                dropins = POWER.DROPIN
                for line in self.read(POWER.DROPIN).splitlines():
                    if line.startswith("ExecStart=") and line != "ExecStart=":
                        command = line.removeprefix("ExecStart=")
            properties = {"FragmentPath": POWER.STOCK_UNIT, "DropInPaths": dropins,
                          "ActiveState": "active" if self.active else "failed",
                          "ExecStart": f"{{ path=/usr/bin/thermald ; argv[]={command} ; }}"}
            properties.update(self.service_overrides)
            output = "".join(f"{key}={value}\n" for key, value in properties.items())
            return subprocess.CompletedProcess(argv, 0, output, "")
        if argv == ["systemctl", "daemon-reload"]:
            return subprocess.CompletedProcess(argv, 0, "", "")
        if argv == ["systemctl", "restart", "thermald.service"]:
            failure = self.restart_failures.pop(0) if self.restart_failures else False
            self.active = not failure
            if failure:
                return subprocess.CompletedProcess(argv, 1, "", "injected restart failure")
            if self.path(POWER.DROPIN).exists():
                def deadlock(signum, frame):
                    raise AssertionError("ExecStartPost apply blocked by the set/restart lock")

                previous = signal.signal(signal.SIGALRM, deadlock)
                signal.setitimer(signal.ITIMER_REAL, 3)
                try:
                    self.controller.apply()
                except POWER.PowerError as error:
                    # ExecStartPost starts with '-', so thermald stays active.
                    self.apply_errors.append(str(error))
                finally:
                    signal.setitimer(signal.ITIMER_REAL, 0)
                    signal.signal(signal.SIGALRM, previous)
            return subprocess.CompletedProcess(argv, 0, "", "")
        raise AssertionError(f"Unmocked service command: {argv!r}")


class PowerTransactions(unittest.TestCase):
    def setUp(self):
        self.host = FakeHost(self)
        self.controller = self.host.controller

    def assert_unconfigured(self):
        for path in (POWER.XML, POWER.DROPIN, POWER.STATE):
            self.assertFalse(self.host.path(path).exists(), str(path))

    def pending(self, operation="set"):
        self.controller.set_limit(55)
        state = json.loads(self.host.read(POWER.STATE))
        transaction = str(uuid.uuid4())
        state["operation_id"] = transaction
        state["transaction"] = {"id": transaction, "operation": operation,
                                "owner": self.host.env.process_identity()}
        self.host.write(POWER.STATE, json.dumps(state))
        return state

    def orphaned_owner(self):
        identity = self.host.env.process_identity
        return mock.patch.object(self.host.env, "process_identity",
                                 side_effect=lambda pid=None: identity() if pid is None else None)

    def test_recover_refuses_a_transaction_owned_by_a_live_process(self):
        self.pending()
        original = {path: self.host.read(path) for path in (POWER.XML, POWER.DROPIN, POWER.STATE)}
        with self.assertRaises(POWER.PowerError):
            self.controller.recover()
        self.assertEqual({path: self.host.read(path) for path in original}, original)
        self.assertTrue(self.host.active)

    def test_recover_accepts_a_reused_pid_with_a_different_start_time(self):
        state = self.pending()
        original = self.host.env.process_identity
        reused = dict(state["transaction"]["owner"])
        reused["start_ticks"] += 1
        with mock.patch.object(self.host.env, "process_identity",
                               side_effect=lambda pid=None: original() if pid is None else reused):
            with mock.patch.object(self.host.env, "write_control", wraps=self.host.env.write_control) as writes:
                self.controller.recover()
        writes.assert_not_called()
        self.assert_unconfigured()
        self.assertTrue(self.host.active)

    def test_recover_accepts_a_previous_boot_owner_even_if_its_pid_now_exists(self):
        self.pending()
        with mock.patch.object(self.host.env, "boot_id", return_value=str(uuid.uuid4())):
            self.controller.recover()
        self.assert_unconfigured()
        self.assertTrue(self.host.active)

    def _assert_recover_handles_partial_clear(self, missing):
        self.pending("clear")
        self.host.path(missing).unlink()
        with self.orphaned_owner():
            with mock.patch.object(self.host.env, "write_control", wraps=self.host.env.write_control) as writes:
                self.controller.recover()
        writes.assert_not_called()
        self.assert_unconfigured()
        self.assertTrue(self.host.active)
        self.assertEqual(self.host.limits(), [55_000_000, 55_000_000])

    def test_recover_handles_clear_with_xml_already_removed(self):
        self._assert_recover_handles_partial_clear(POWER.XML)

    def test_recover_handles_clear_with_dropin_already_removed(self):
        self._assert_recover_handles_partial_clear(POWER.DROPIN)

    def test_recover_preserves_foreign_bytes_even_for_an_orphaned_transaction(self):
        self.pending("clear")
        foreign = self.host.read(POWER.XML) + "<!-- user changed this -->\n"
        self.host.write(POWER.XML, foreign)
        original_state = self.host.read(POWER.STATE)
        with self.orphaned_owner():
            with self.assertRaises(POWER.PowerError):
                self.controller.recover()
        self.assertEqual(self.host.read(POWER.XML), foreign)
        self.assertEqual(self.host.read(POWER.STATE), original_state)
        self.assertTrue(self.host.path(POWER.DROPIN).is_file())

    def test_failed_recover_retains_a_failure_receipt_and_can_be_retried(self):
        self.pending()
        self.host.restart_failures = [True, False]
        with self.orphaned_owner():
            with self.assertRaises(POWER.PowerError):
                self.controller.recover()
            self.assertTrue(self.host.path(POWER.STATE).is_file())
            receipts = [json.loads(path.read_text()) for path in self.host.path(POWER.RECEIPTS).glob("*.json")]
            self.assertTrue(any(receipt["operation"] == "recover" and not receipt["success"]
                                for receipt in receipts))
            self.controller.recover()
        self.assert_unconfigured()
        self.assertTrue(self.host.active)

    def test_recover_does_not_discard_a_completed_configuration(self):
        self.controller.set_limit(55)
        original = self.host.read(POWER.STATE)
        with self.assertRaises(POWER.PowerError):
            self.controller.recover()
        self.assertEqual(self.host.read(POWER.STATE), original)
        self.assertTrue(self.controller.status()["holding_now"])

    def test_recover_fails_closed_when_owner_identity_cannot_be_read(self):
        self.pending()
        original = self.host.read(POWER.STATE)
        with mock.patch.object(self.host.env, "process_identity", side_effect=OSError("permission denied")):
            with self.assertRaises(POWER.PowerError):
                self.controller.recover()
        self.assertEqual(self.host.read(POWER.STATE), original)
        self.assertTrue(self.host.path(POWER.XML).is_file())

    def test_recover_accepts_verified_prior_xml_after_partial_limit_update(self):
        self.controller.set_limit(55)
        old_xml = self.host.read(POWER.XML)
        atomic = self.host.env.atomic

        def die_after_new_state(path, data, mode=0o600):
            atomic(path, data, mode)
            if path == POWER.STATE and json.loads(data)["cap_uw"] == 80_000_000:
                raise SystemExit("interrupted before replacing old XML")

        with mock.patch.object(self.host.env, "atomic", side_effect=die_after_new_state):
            with self.assertRaises(SystemExit):
                self.controller.set_limit(80)
        self.assertEqual(json.loads(self.host.read(POWER.STATE))["cap_uw"], 80_000_000)
        self.assertEqual(self.host.read(POWER.XML), old_xml)
        with self.orphaned_owner():
            self.controller.recover()
        self.assert_unconfigured()
        self.assertTrue(self.host.active)

    def test_recover_accepts_verified_prior_xml_during_interrupted_rollback(self):
        self.controller.set_limit(55)
        old_xml = self.host.read(POWER.XML).encode()
        self.host.restart_failures = [True]
        atomic = self.host.env.atomic
        restored_old_xml = False

        def die_after_restoring_xml(path, data, mode=0o600):
            nonlocal restored_old_xml
            atomic(path, data, mode)
            if path == POWER.XML and data == old_xml:
                restored_old_xml = True
                raise SystemExit("interrupted during rollback")

        with mock.patch.object(self.host.env, "atomic", side_effect=die_after_restoring_xml):
            with self.assertRaises(SystemExit):
                self.controller.set_limit(80)
        self.assertTrue(restored_old_xml)
        with self.orphaned_owner():
            self.controller.recover()
        self.assert_unconfigured()
        self.assertTrue(self.host.active)

    def test_sigkill_orphan_can_be_recovered_without_upward_hardware_writes(self):
        code = r'''
import os, signal, sys, unittest
from pathlib import Path
sys.path.insert(0, str(Path.cwd() / 'tests'))
from test_mechrevo_power import FakeHost
host = FakeHost(unittest.TestCase(), root=Path(sys.argv[1]))
run = host.env.runner
def killed_at_restart(argv):
    if argv == ['systemctl', 'restart', 'thermald.service']:
        os.kill(os.getpid(), signal.SIGKILL)
    return run(argv)
host.env.runner = killed_at_restart
host.controller.set_limit(55)
'''
        child = subprocess.Popen([sys.executable, "-c", code, str(self.host.root)],
                                 cwd=ROOT, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        stdout, stderr = child.communicate(timeout=15)
        self.assertEqual(child.returncode, -signal.SIGKILL, stdout + stderr)
        state = json.loads(self.host.read(POWER.STATE))
        self.assertEqual(state["transaction"]["owner"]["pid"], child.pid)
        self.assertIsNone(self.host.env.process_identity(child.pid))
        with mock.patch.object(self.host.env, "write_control", wraps=self.host.env.write_control) as writes:
            self.controller.recover()
        writes.assert_not_called()
        self.assert_unconfigured()
        self.assertTrue(self.host.active)
        self.assertEqual(self.host.limits(), [140_000_000, 140_000_000])

    def test_set_down_limits_both_interfaces_and_preserves_other_controls(self):
        before = self.host.protected_controls()
        result = self.controller.set_limit(55)
        self.assertEqual(self.host.limits(), [55_000_000, 55_000_000])
        self.assertEqual(self.host.protected_controls(), before)
        self.assertTrue(self.host.active)
        self.assertIsInstance(result, dict)
        self.assertTrue(self.host.path(POWER.STATE).is_file())
        root = ET.fromstring(self.host.read(POWER.XML))
        devices = {node.findtext("Type"): node for node in root.iter("CoolingDevice")}
        self.assertEqual(set(devices), {"rapl_controller", "rapl_controller_mmio"})
        for device in devices.values():
            self.assertEqual(int(device.findtext("MinState")), 55_000_000)
            self.assertEqual(int(device.findtext("MaxState")), 27_500_000)
        self.assertEqual(self.host.read(POWER.STOCK_UNIT), STOCK_UNIT)

    def test_apply_never_raises_a_thermally_lowered_interface(self):
        self.controller.set_limit(55)
        self.host.limit_path(0).write_text("40000000\n")
        self.host.limit_path(1).write_text("140000000\n")
        self.controller.apply()
        self.assertEqual(self.host.limits(), [40_000_000, 55_000_000])
        self.host.limit_path(1).write_text("30000000\n")
        self.controller.apply()
        self.assertEqual(self.host.limits(), [40_000_000, 30_000_000])

    def test_clear_restores_stock_service_without_a_direct_upward_limit_write(self):
        self.controller.set_limit(55)
        self.host.limit_path(0).write_text("40000000\n")
        self.host.limit_path(1).write_text("30000000\n")
        self.controller.clear()
        self.assert_unconfigured()
        self.assertTrue(self.host.active)
        self.assertEqual(self.host.limits(), [40_000_000, 30_000_000])
        self.assertEqual(self.host.read(POWER.STOCK_UNIT), STOCK_UNIT)

    def test_clear_partial_staging_failure_restores_managed_files_and_service(self):
        self.controller.set_limit(55)
        original_files = {path: self.host.read(path) for path in (POWER.XML, POWER.DROPIN)}
        remove = self.host.env.remove
        failed = False

        def fail_dropin_once(path):
            nonlocal failed
            if path == POWER.DROPIN and not failed:
                failed = True
                raise OSError("injected failure after XML removal")
            return remove(path)

        with mock.patch.object(self.host.env, "remove", side_effect=fail_dropin_once):
            with self.assertRaises(POWER.PowerError):
                self.controller.clear()
        self.assertTrue(failed)
        self.assertEqual({path: self.host.read(path) for path in original_files}, original_files)
        state = json.loads(self.host.read(POWER.STATE))
        self.assertFalse(state.get("transaction"))
        self.assertTrue(self.host.active)
        self.assertTrue(self.controller.status()["holding_now"])

    def test_initial_restart_failure_restores_configuration_and_active_thermal_daemon(self):
        self.host.restart_failures = [True, False]
        with self.assertRaises(POWER.PowerError):
            self.controller.set_limit(55)
        self.assert_unconfigured()
        self.assertTrue(self.host.active)
        self.assertEqual(self.host.limits(), [140_000_000, 140_000_000])

    def test_second_interface_write_failure_rolls_back_config_and_keeps_thermal_daemon(self):
        before = self.host.protected_controls()
        original = self.host.env.write_control
        writes = []

        def fail_second(path, value):
            writes.append((str(path), value))
            if str(path).startswith(POWER.ZONES[1] + "/"):
                raise OSError("injected second-interface write failure")
            return original(path, value)

        with mock.patch.object(self.host.env, "write_control", side_effect=fail_second):
            with self.assertRaises(POWER.PowerError):
                self.controller.set_limit(55)
        self.assert_unconfigured()
        self.assertTrue(self.host.active)
        self.assertEqual(self.host.limits(), [55_000_000, 140_000_000])
        self.assertEqual(self.host.protected_controls(), before)
        self.assertEqual([value for _, value in writes], [55_000_000, 55_000_000])

    def test_kernel_readback_mismatch_is_failure_and_restores_thermal_service(self):
        original = self.host.env.write_control

        def ignored_write(path, value):
            original(path, value)
            self.host.path(path).write_text("133000000\n")

        with mock.patch.object(self.host.env, "write_control", side_effect=ignored_write):
            with self.assertRaises(POWER.PowerError):
                self.controller.set_limit(55)
        self.assert_unconfigured()
        self.assertTrue(self.host.active)

    def test_service_and_rollback_failure_are_reported_instead_of_success(self):
        self.host.restart_failures = [True, True]
        with self.assertRaises(POWER.PowerError):
            self.controller.set_limit(55)
        self.assertFalse(self.host.active)

    def test_managed_rollback_requires_apply_to_succeed_in_addition_to_daemon_active(self):
        self.controller.set_limit(55)
        old_xml = self.host.read(POWER.XML)
        run = self.host.env.runner

        def raise_before_every_restart(argv):
            if argv == ["systemctl", "restart", "thermald.service"]:
                for index in range(2):
                    self.host.limit_path(index).write_text("140000000\n")
            return run(argv)

        self.host.env.runner = raise_before_every_restart
        with mock.patch.object(self.host.env, "write_control", side_effect=OSError("kernel write denied")):
            with self.assertRaisesRegex(POWER.PowerError, "ROLLBACK FAILED"):
                self.controller.set_limit(60)
        self.assertTrue(self.host.active)
        self.assertEqual(self.host.read(POWER.XML), old_xml)
        self.assertFalse(self.controller.status()["holding_now"])

    def _assert_rollback_blocks_concurrent_mutation(self, operation):
        self.controller.set_limit(55)
        self.host.restart_failures = [True, False]
        run = self.host.env.runner
        restarts = 0
        rejected = []

        def concurrent_during_rollback(argv):
            nonlocal restarts
            if argv == ["systemctl", "restart", "thermald.service"]:
                restarts += 1
                if restarts == 2:
                    try:
                        if operation == "set":
                            self.controller.set_limit(65)
                        else:
                            self.controller.clear()
                    except POWER.PowerError:
                        rejected.append(True)
                    else:
                        rejected.append(False)
            return run(argv)

        self.host.env.runner = concurrent_during_rollback
        with self.assertRaises(POWER.PowerError):
            self.controller.set_limit(60)
        self.assertEqual(rejected, [True], "rollback restart must retain its transaction barrier")
        self.assertEqual(json.loads(self.host.read(POWER.STATE))["cap_uw"], 55_000_000)
        self.assertTrue(self.controller.status()["holding_now"])

    def test_rollback_blocks_concurrent_set_while_service_restarts(self):
        self._assert_rollback_blocks_concurrent_mutation("set")

    def test_rollback_blocks_concurrent_clear_while_service_restarts(self):
        self._assert_rollback_blocks_concurrent_mutation("clear")

    def test_stock_rollback_checks_the_actually_loaded_service_configuration(self):
        self.host.restart_failures = [True, False]
        run = self.host.env.runner
        restarts = 0

        def wrong_loaded_command_after_rollback(argv):
            nonlocal restarts
            result = run(argv)
            if argv == ["systemctl", "restart", "thermald.service"]:
                restarts += 1
                if restarts == 2:
                    self.host.service_overrides["ExecStart"] = "/usr/bin/thermald --no-existing-policy"
            return result

        self.host.env.runner = wrong_loaded_command_after_rollback
        with self.assertRaisesRegex(POWER.PowerError, "ROLLBACK FAILED"):
            self.controller.set_limit(55)
        self.assertTrue(self.host.active)

    def test_keyboard_interrupt_during_restart_restores_stock_configuration(self):
        run = self.host.env.runner
        interrupted = False

        def interrupt_once(argv):
            nonlocal interrupted
            if argv == ["systemctl", "restart", "thermald.service"] and not interrupted:
                interrupted = True
                raise KeyboardInterrupt()
            return run(argv)

        self.host.env.runner = interrupt_once
        with self.assertRaises((POWER.PowerError, KeyboardInterrupt)):
            self.controller.set_limit(55)
        self.assertTrue(interrupted)
        self.assert_unconfigured()
        self.assertTrue(self.host.active)

    def _interrupt_first_lock_exit(self):
        lock = self.controller.lock
        interrupted = False

        @contextmanager
        def interrupt_after_unlock():
            nonlocal interrupted
            with lock():
                yield
            if not interrupted:
                interrupted = True
                raise KeyboardInterrupt()

        return mock.patch.object(self.controller, "lock", side_effect=interrupt_after_unlock)

    def test_set_interrupt_at_unlock_restart_handoff_restores_stock_configuration(self):
        with self._interrupt_first_lock_exit():
            with self.assertRaises((POWER.PowerError, KeyboardInterrupt)):
                self.controller.set_limit(55)
        self.assert_unconfigured()
        self.assertTrue(self.host.active)

    def test_clear_interrupt_at_unlock_restart_handoff_restores_managed_configuration(self):
        self.controller.set_limit(55)
        with self._interrupt_first_lock_exit():
            with self.assertRaises((POWER.PowerError, KeyboardInterrupt)):
                self.controller.clear()
        self.assertTrue(self.controller.status()["holding_now"])
        self.assertTrue(self.host.active)
        self.assertFalse(json.loads(self.host.read(POWER.STATE)).get("transaction"))

    def test_sigterm_during_cli_restart_restores_stock_configuration(self):
        code = r'''
import json, os, signal, sys, unittest
from unittest import mock
sys.path.insert(0, str(__import__('pathlib').Path.cwd() / 'tests'))
from test_mechrevo_power import FakeHost, POWER
case = unittest.TestCase()
host = FakeHost(case)
run = host.env.runner
interrupted = False
def terminate_once(argv):
    global interrupted
    if argv == ['systemctl', 'restart', 'thermald.service'] and not interrupted:
        interrupted = True
        os.kill(os.getpid(), signal.SIGTERM)
    return run(argv)
host.env.runner = terminate_once
try:
    with mock.patch.object(POWER, 'PowerController', return_value=host.controller):
        result = POWER.main(['set', '55'])
    print(json.dumps({'exit_code': result, 'interrupted': interrupted,
                      'active': host.active,
                      'remaining': [path for path in (POWER.XML, POWER.DROPIN, POWER.STATE)
                                    if host.path(path).exists()]}))
finally:
    case.doCleanups()
'''
        result = subprocess.run([sys.executable, "-c", code], cwd=ROOT, text=True,
                                capture_output=True, timeout=15)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        evidence = json.loads(result.stdout.splitlines()[-1])
        self.assertNotEqual(evidence["exit_code"], 0)
        self.assertTrue(evidence["interrupted"])
        self.assertTrue(evidence["active"])
        self.assertEqual(evidence["remaining"], [])

    def test_final_status_failure_cannot_be_returned_as_a_successful_set(self):
        status = self.controller.status

        def external_override_before_final_status():
            self.host.limit_path(0).write_text("133000000\n")
            return status()

        with mock.patch.object(self.controller, "status", side_effect=external_override_before_final_status):
            with self.assertRaises(POWER.PowerError):
                self.controller.set_limit(55)
        self.assert_unconfigured()
        self.assertTrue(self.host.active)

    def test_final_status_cannot_be_replaced_by_a_concurrent_limit_change(self):
        status = self.controller.status
        rejected = []

        def concurrent_set_during_final_status():
            try:
                self.controller.set_limit(60)
            except POWER.PowerError:
                rejected.append(True)
            else:
                rejected.append(False)
            return status()

        with mock.patch.object(self.controller, "status", side_effect=concurrent_set_during_final_status):
            result = self.controller.set_limit(55)
        self.assertEqual(rejected, [True])
        self.assertTrue(result["holding_now"])
        self.assertEqual(result["desired_limit_uw"], 55_000_000)
        self.assertEqual(json.loads(self.host.read(POWER.STATE))["cap_uw"], 55_000_000)

    def test_failed_apply_does_not_forge_a_successful_receipt(self):
        self.controller.set_limit(55)
        self.host.limit_path(0).write_text("140000000\n")
        self.host.limit_path(1).write_text("140000000\n")
        with mock.patch.object(self.host.env, "write_control", side_effect=OSError("write denied")):
            with self.assertRaises(POWER.PowerError):
                self.controller.apply()
        state = json.loads(self.host.read(POWER.STATE))
        self.assertFalse(state["last_apply"]["success"])
        self.assertFalse(self.controller.status()["holding_now"])

    def test_status_observes_actual_power_limits_instead_of_trusting_state(self):
        self.controller.set_limit(55)
        self.assertTrue(self.controller.status()["holding_now"])
        self.host.limit_path(0).write_text("133000000\n")
        status = self.controller.status()
        self.assertFalse(status["holding_now"])
        self.assertEqual(status["observed"][POWER.ZONES[0]]["pl1_uw"], 133_000_000)

    def test_status_requires_the_thermal_daemon_to_be_active(self):
        self.controller.set_limit(55)
        self.host.active = False
        self.assertFalse(self.controller.status()["holding_now"])

    def test_status_uses_the_latest_qualification_readback(self):
        self.controller.set_limit(55)
        observe = self.controller.observe
        calls = 0

        def externally_raised_second_sample():
            nonlocal calls
            calls += 1
            if calls == 2:
                self.host.limit_path(0).write_text("133000000\n")
            return observe()

        with mock.patch.object(self.controller, "observe", side_effect=externally_raised_second_sample):
            status = self.controller.status()
        self.assertGreaterEqual(calls, 2)
        self.assertFalse(status["holding_now"])
        self.assertEqual(status["observed"][POWER.ZONES[0]]["pl1_uw"], 133_000_000)

    def test_invalid_requested_limits_do_not_write_configuration_or_hardware(self):
        for watts in (-1, 0, 54, 141, 55.5, "55", None):
            with self.subTest(watts=watts):
                with self.assertRaises(POWER.PowerError):
                    self.controller.set_limit(watts)
                self.assert_unconfigured()
                self.assertEqual(self.host.limits(), [140_000_000, 140_000_000])

    def test_mutation_requires_root(self):
        self.host.env.uid = 1000
        with self.assertRaises(POWER.PowerError):
            self.controller.set_limit(55)
        self.assert_unconfigured()
        self.assertEqual(self.host.limits(), [140_000_000, 140_000_000])

    def test_other_bios_is_rejected_before_any_mutation(self):
        self.host.write("/sys/class/dmi/id/bios_version", "N.1.10\n")
        with self.assertRaises(POWER.PowerError):
            self.controller.set_limit(55)
        self.assert_unconfigured()

    def test_other_product_is_rejected_before_any_mutation(self):
        self.host.write("/sys/class/dmi/id/product_name", "Similar GM6PX0X device\n")
        with self.assertRaises(POWER.PowerError):
            self.controller.set_limit(55)
        self.assert_unconfigured()

    def test_other_cpu_is_rejected_before_any_mutation(self):
        self.host.write("/proc/cpuinfo", "model name\t: Intel Core i9-14900HX\n")
        with self.assertRaises(POWER.PowerError):
            self.controller.set_limit(55)
        self.assert_unconfigured()

    def test_other_daemon_version_is_rejected_before_any_mutation(self):
        self.host.version = "2.5.14"
        with self.assertRaises(POWER.PowerError):
            self.controller.set_limit(55)
        self.assert_unconfigured()

    def test_unqualified_daemon_binary_is_rejected_before_any_mutation(self):
        self.host.write("/usr/bin/thermald", "same-version different binary\n")
        with self.assertRaises(POWER.PowerError):
            self.controller.set_limit(55)
        self.assert_unconfigured()

    def test_daemon_binary_change_blocks_apply_before_hardware_writes(self):
        self.controller.set_limit(55)
        state = json.loads(self.host.read(POWER.STATE))
        self.assertEqual(state["thermald_sha256"],
                         hashlib.sha256(self.host.path("/usr/bin/thermald").read_bytes()).hexdigest())
        self.host.write("/usr/bin/thermald", "unreviewed daemon update\n")
        self.host.limit_path(0).write_text("133000000\n")
        with mock.patch.object(self.host.env, "write_control", wraps=self.host.env.write_control) as writes:
            with self.assertRaises(POWER.PowerError):
                self.controller.apply()
        writes.assert_not_called()
        self.assertEqual(self.host.limits(), [133_000_000, 55_000_000])
        self.assertFalse(self.controller.status()["holding_now"])

    def test_ppcc_firmware_is_rejected_before_any_mutation(self):
        self.host.write("/sys/bus/pci/devices/0000:00:04.0/power_limits", "firmware PPCC table\n")
        with self.assertRaises(POWER.PowerError):
            self.controller.set_limit(55)
        self.assert_unconfigured()

    def test_int3400_adaptive_firmware_is_rejected_before_any_mutation(self):
        self.host.path("/sys/bus/platform/drivers/int3400 thermal").mkdir(parents=True)
        with self.assertRaises(POWER.PowerError):
            self.controller.set_limit(55)
        self.assert_unconfigured()

    def test_tcc_offset_above_qualified_range_is_rejected(self):
        self.host.write("/sys/bus/pci/devices/0000:00:04.0/tcc_offset_degree_celsius", "11\n")
        with self.assertRaises(POWER.PowerError):
            self.controller.set_limit(55)
        self.assert_unconfigured()

    def test_tcc_offset_boundary_is_allowed(self):
        self.host.write("/sys/bus/pci/devices/0000:00:04.0/tcc_offset_degree_celsius", "10\n")
        self.controller.set_limit(55)
        self.assertEqual(self.host.limits(), [55_000_000, 55_000_000])

    def test_unknown_preference_is_rejected_without_erasing_it(self):
        self.host.write(POWER.PREF, "2\n")
        with self.assertRaises(POWER.PowerError):
            self.controller.set_limit(55)
        self.assert_unconfigured()
        self.assertEqual(self.host.read(POWER.PREF), "2\n")

    def test_existing_preference_is_preserved(self):
        for preference in (0, 1):
            with self.subTest(preference=preference):
                self.host.write(POWER.PREF, f"{preference}\n")
                self.controller.set_limit(55)
                self.assertEqual(self.host.read(POWER.PREF), f"{preference}\n")
                xml = ET.fromstring(self.host.read(POWER.XML))
                self.assertEqual(xml.findtext(".//Preference"),
                                 ("ENERGY_CONSERVE", "PERFORMANCE")[preference])
                self.controller.clear()

    def test_missing_preference_uses_the_native_energy_conserve_default(self):
        self.assertFalse(self.host.path(POWER.PREF).exists())
        self.controller.set_limit(55)
        xml = ET.fromstring(self.host.read(POWER.XML))
        self.assertEqual(xml.findtext(".//Preference"), "ENERGY_CONSERVE")

    def test_foreign_stock_thermal_configuration_is_preserved(self):
        original = "<ThermalConfiguration><Platform><Name>existing cooling policy</Name></Platform></ThermalConfiguration>\n"
        path = "/etc/thermald/thermal-conf.xml"
        self.host.write(path, original)
        with self.assertRaises(POWER.PowerError):
            self.controller.set_limit(55)
        self.assertEqual(self.host.read(path), original)
        self.assert_unconfigured()

    def test_foreign_stock_exec_start_post_is_rejected(self):
        modified = STOCK_UNIT.replace("Restart=on-failure", "ExecStartPost=/usr/local/bin/power-policy\nRestart=on-failure")
        self.host.write(POWER.STOCK_UNIT, modified)
        with self.assertRaises(POWER.PowerError):
            self.controller.set_limit(55)
        self.assertEqual(self.host.read(POWER.STOCK_UNIT), modified)
        self.assert_unconfigured()

    def test_xml_thermal_config_disabled_is_rejected(self):
        path = "/etc/thermald/thermald-features.xml"
        original = self.host.read(path).replace("<XMLThermalConfig>1</XMLThermalConfig>",
                                                "<XMLThermalConfig>0</XMLThermalConfig>")
        self.host.write(path, original)
        with self.assertRaises(POWER.PowerError):
            self.controller.set_limit(55)
        self.assertEqual(self.host.read(path), original)
        self.assert_unconfigured()

    def test_disabled_powercap_interface_is_rejected(self):
        self.host.write(f"{POWER.ZONES[1]}/enabled", "0\n")
        with self.assertRaises(POWER.PowerError):
            self.controller.set_limit(55)
        self.assert_unconfigured()
        self.assertEqual(self.host.limits(), [140_000_000, 140_000_000])

    def test_constraint_index_must_actually_be_long_term(self):
        self.host.write(f"{POWER.ZONES[1]}/constraint_0_name", "short_term\n")
        with self.assertRaises(POWER.PowerError):
            self.controller.set_limit(55)
        self.assert_unconfigured()

    def test_status_without_managed_state_does_not_claim_a_user_cap(self):
        status = self.controller.status()
        self.assertFalse(status["managed"])
        self.assertFalse(status["holding_now"])
        self.assertIsNone(status["desired_limit_uw"])
        self.assertEqual(status["observed"][POWER.ZONES[0]]["pl1_uw"], 140_000_000)

    def test_foreign_xml_is_preserved(self):
        original = "<ThermalConfiguration><Platform><Name>user</Name></Platform></ThermalConfiguration>\n"
        self.host.write(POWER.XML, original)
        with self.assertRaises(POWER.PowerError):
            self.controller.set_limit(55)
        self.assertEqual(self.host.read(POWER.XML), original)
        self.assertFalse(self.host.path(POWER.STATE).exists())
        self.assertEqual(self.host.limits(), [140_000_000, 140_000_000])

    def test_foreign_service_dropin_is_preserved(self):
        original = "[Service]\nEnvironment=OTHER=1\n"
        self.host.write(POWER.DROPIN, original)
        with self.assertRaises(POWER.PowerError):
            self.controller.set_limit(55)
        self.assertEqual(self.host.read(POWER.DROPIN), original)
        self.assertFalse(self.host.path(POWER.STATE).exists())

    def test_xml_symlink_is_rejected_without_touching_its_target(self):
        target = self.host.write("/etc/user-original.xml", "preserve me\n")
        self.host.path(POWER.XML).parent.mkdir(parents=True, exist_ok=True)
        self.host.path(POWER.XML).symlink_to(target)
        with self.assertRaises(POWER.PowerError):
            self.controller.set_limit(55)
        self.assertEqual(target.read_text(), "preserve me\n")
        self.assertTrue(self.host.path(POWER.XML).is_symlink())

    def test_symlink_in_the_configuration_parent_is_rejected(self):
        directory = self.host.path(POWER.XML).parent
        # The baseline contains the features file; retain it in the target so
        # refusal cannot be explained by missing configuration data.
        target = self.host.path("/opt/thermal-config")
        target.parent.mkdir(parents=True)
        directory.rename(target)
        directory.symlink_to(target)
        with self.assertRaises(POWER.PowerError):
            self.controller.set_limit(55)
        self.assertFalse((target / Path(POWER.XML).name).exists())
        self.assertTrue(directory.is_symlink())
        self.assertFalse(self.host.path(POWER.STATE).exists())

    def test_state_symlink_is_rejected_without_touching_its_target(self):
        target = self.host.write("/etc/user-state.json", '{"user": "preserve"}\n')
        self.host.path(POWER.STATE).parent.mkdir(parents=True, exist_ok=True)
        self.host.path(POWER.STATE).symlink_to(target)
        with self.assertRaises(POWER.PowerError):
            self.controller.set_limit(55)
        self.assertEqual(target.read_text(), '{"user": "preserve"}\n')
        self.assertTrue(self.host.path(POWER.STATE).is_symlink())

    def test_malformed_state_is_preserved_and_rejected(self):
        original = '{"broken":'
        self.host.write(POWER.STATE, original)
        with self.assertRaises(POWER.PowerError):
            self.controller.set_limit(55)
        self.assertEqual(self.host.read(POWER.STATE), original)
        self.assertFalse(self.host.path(POWER.XML).exists())

    def test_managed_xml_modified_outside_the_tool_is_not_overwritten_or_removed(self):
        self.controller.set_limit(55)
        modified = self.host.read(POWER.XML) + "<!-- user adjustment -->\n"
        self.host.write(POWER.XML, modified)
        with self.assertRaises(POWER.PowerError):
            self.controller.clear()
        self.assertEqual(self.host.read(POWER.XML), modified)
        self.assertTrue(self.host.path(POWER.STATE).exists())


class PowerStateValidation(unittest.TestCase):
    """Malformed persisted data must never authorize writes or a holding claim."""

    def setUp(self):
        self.host = FakeHost(self)
        self.controller = self.host.controller
        self.controller.set_limit(55)
        self.valid = json.loads(self.host.read(POWER.STATE))
        self.host.commands.clear()

    def test_corrupt_nested_state_is_reported_and_preserved(self):
        mutations = [
            ((), []),
            (("version",), True),
            (("cap_uw",), True),
            (("initial_ceiling_uw",), True),
            (("initial_ceiling_uw",), 141_000_000),
            (("preference",), []),
            (("preference", "value"), False),
            (("preference", "value"), "0"),
            (("preference", "name"), "PERFORMANCE"),
            (("initial_limits",), []),
            (("initial_limits", POWER.ZONES[0]), {}),
            (("initial_limits", POWER.ZONES[0], "pl1_uw"), "140000000"),
            (("initial_limits", POWER.ZONES[0], "max_power_uw"), 54_000_000),
            (("initial_limits", POWER.ZONES[0], "physical_path"), "/outside/sysfs"),
            (("managed_hashes", POWER.XML), []),
            (("stock_unit_sha256",), "not-a-hash"),
            (("operation_id",), ""),
            (("backup",), ""),
            (("transaction",), None),
            (("transaction",), []),
            (("transaction",), {}),
            (("transaction",), "pending"),
            (("transaction",), {"id": "", "operation": "set", "owner": self.host.env.process_identity()}),
            (("transaction",), {"id": "pending", "operation": "unknown", "owner": self.host.env.process_identity()}),
            (("transaction",), {"id": "pending", "operation": "set", "owner": self.host.env.process_identity(),
                                "prior_cap_uw": None, "prior_managed_hashes": {POWER.XML: "foreign", POWER.DROPIN: None}}),
            (("last_apply",), []),
            (("last_apply",), {}),
            (("last_apply",), "success"),
            (("last_apply", "success"), "false"),
            (("last_apply", "success"), 1),
            (("last_apply", "operation"), "clear"),
            (("last_apply", "cap_uw"), 140_000_000),
            (("last_apply", "observed"), {}),
            (("last_apply", "observed", POWER.ZONES[0], "pl1_uw"), 140_000_000),
            (("last_apply", "writes"), ["/sys/foreign"]),
            (("last_apply", "transaction"), []),
        ]
        for keys, value in mutations:
            with self.subTest(keys=keys, value=value):
                state = json.loads(json.dumps(self.valid))
                if keys:
                    target = state
                    for key in keys[:-1]:
                        target = target[key]
                    target[keys[-1]] = value
                else:
                    state = value
                self.host.write(POWER.STATE, json.dumps(state))
                before = {path: path.read_bytes() for path in self.host.root.rglob("*") if path.is_file()}
                limits = self.host.limits()
                with mock.patch.object(self.host.env, "write_control", wraps=self.host.env.write_control) as writes:
                    status = self.controller.status()
                    self.assertFalse(status["holding_now"])
                    self.assertTrue(status["errors"])
                    self.assertIn("Invalid managed state:", status["errors"][0])
                    for operation in (lambda: self.controller.set_limit(55), self.controller.apply,
                                      self.controller.clear, self.controller.recover):
                        with self.assertRaises(POWER.PowerError):
                            operation()
                    writes.assert_not_called()
                self.assertEqual(self.host.limits(), limits)
                after = {path: path.read_bytes() for path in self.host.root.rglob("*") if path.is_file()}
                self.assertEqual(before, after)
                self.assertFalse(any(argv[:2] == ["systemctl", "restart"] for argv in self.host.commands))

    def test_invalid_pending_owner_is_reported_without_a_traceback(self):
        valid_owner = self.host.env.process_identity()
        for owner in (None, [], {}, dict(valid_owner, pid=True),
                      dict(valid_owner, start_ticks=-1), dict(valid_owner, boot_id="")):
            with self.subTest(owner=owner):
                state = dict(self.valid, transaction={"id": str(uuid.uuid4()), "operation": "set", "owner": owner})
                self.host.write(POWER.STATE, json.dumps(state))
                self.assertFalse(self.controller.status()["holding_now"])
                self.assertIn("Invalid managed state:", self.controller.status()["errors"][0])
                with self.assertRaises(POWER.PowerError):
                    self.controller.recover()
                self.assertEqual(json.loads(self.host.read(POWER.STATE)), state)

    def test_cli_status_exit_distinguishes_errors_from_healthy_unmanaged_state(self):
        for state, expected_exit in ((self.valid, 0), (dict(self.valid, last_apply={"success": "false"}), 1),
                                     (dict(self.valid, last_apply=None), 1), (None, 0)):
            with self.subTest(state=state):
                if state is None:
                    self.controller.clear()
                else:
                    self.host.write(POWER.STATE, json.dumps(state))
                stdout, stderr = io.StringIO(), io.StringIO()
                with mock.patch.object(POWER, "PowerController", return_value=self.controller), \
                        redirect_stdout(stdout), redirect_stderr(stderr):
                    result = POWER.main(["status"])
                status = json.loads(stdout.getvalue())
                self.assertEqual(result, expected_exit)
                self.assertEqual(bool(status["errors"]), expected_exit == 1)
                self.assertEqual(stderr.getvalue(), "")

    def test_valid_persisted_state_still_holds_and_applies(self):
        self.assertTrue(self.controller.status()["holding_now"])
        with mock.patch.object(self.host.env, "write_control", wraps=self.host.env.write_control) as writes:
            receipt = self.controller.apply()
        writes.assert_not_called()
        self.assertIs(receipt["success"], True)
        self.assertTrue(self.controller.status()["holding_now"])
        self.controller.clear()
        self.assertFalse(self.controller.status()["managed"])

    def test_failed_apply_receipt_and_staged_state_are_valid_but_not_holding(self):
        for receipt in (None, {"version": 1, "operation": "apply", "success": False,
                               "error": "injected failure", "writes": [], "transaction": None,
                               "time": 0}):
            with self.subTest(receipt=receipt):
                self.host.write(POWER.STATE, json.dumps(dict(self.valid, last_apply=receipt)))
                self.assertIsNotNone(self.controller.load_state())
                status = self.controller.status()
                self.assertFalse(status["holding_now"])
                self.assertIn("Latest apply has not succeeded", status["errors"])


if __name__ == "__main__":
    unittest.main()
