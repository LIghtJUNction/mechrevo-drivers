"""Exercise console protocol v1 through the qualified production controller.

All service and actuator I/O stays in FakeHost.  These tests cover the wire
boundary and real transaction decisions; they do not establish hardware effect.
"""

from contextlib import contextmanager, redirect_stderr, redirect_stdout
from copy import deepcopy
import io
import json
import os
import subprocess
import unittest
from unittest import mock
import uuid

from test_mechrevo_power import FakeHost, POWER


POLICY = {
    "cap_uw": 55_000_000,
    "target_celsius": None,
    "polling_seconds": None,
    "rapl_debounce_seconds": None,
}
FAST_POLICY = dict(POLICY, target_celsius=90, polling_seconds=1,
                   rapl_debounce_seconds=0)
REPLY_KEYS = {"schema_version", "request_id", "outcome", "status", "receipt", "error"}
STATUS_KEYS = {
    "management", "revision", "qualified", "allowed_actions", "current_policy",
    "operational_ceiling_uw", "minimum_limit_uw", "thermal_spec_reference_uw",
    "observed", "thermald_active", "holding_now", "blocking_errors",
    "diagnostic_errors",
}


def request(operation, request_id="protocol-test-1"):
    return {"schema_version": 1, "request_id": request_id, "operation": operation}


def execution(action, revision="a" * 64):
    return request({"kind": "execute", "plan": {"revision": revision, "action": action}})


def set_action(policy=None):
    return {"kind": "set", "policy": deepcopy(POLICY if policy is None else policy)}


class PowerConsoleParsing(unittest.TestCase):
    """Malformed or ambiguous requests must fail before controller creation."""

    def parse(self, value):
        return POWER.parse_console_request(json.dumps(value))

    def assert_rejected(self, raw):
        with self.assertRaises((ValueError, POWER.PowerError)):
            POWER.parse_console_request(raw)

    def test_complete_probe_and_set_requests_preserve_exact_explicit_values(self):
        probe = request({"kind": "probe"})
        self.assertEqual(self.parse(probe), probe)
        value = execution(set_action(FAST_POLICY))
        self.assertEqual(self.parse(value), value)
        self.assertEqual(self.parse(execution(set_action())), execution(set_action()))

    def test_duplicate_keys_are_rejected_at_root_and_inside_policy(self):
        self.assert_rejected('{"schema_version":1,"schema_version":1,"request_id":"p",'
                             '"operation":{"kind":"probe"}}')
        raw = json.dumps(execution(set_action())).replace(
            '"cap_uw": 55000000', '"cap_uw": 55000000, "cap_uw": 35000000')
        self.assert_rejected(raw)

    def test_unknown_and_missing_fields_are_rejected_at_every_object_boundary(self):
        original = execution(set_action())
        paths = [(), ("operation",), ("operation", "plan"),
                 ("operation", "plan", "action"),
                 ("operation", "plan", "action", "policy")]
        for path in paths:
            for missing in (False, True):
                with self.subTest(path=path, missing=missing):
                    value = deepcopy(original)
                    target = value
                    for key in path:
                        target = target[key]
                    if missing:
                        target.pop(next(iter(target)))
                    else:
                        target["unexpected"] = None
                    self.assert_rejected(json.dumps(value))
            if path:
                for wrong_type in (None, [], False, "object"):
                    value = deepcopy(original)
                    parent = value
                    for key in path[:-1]:
                        parent = parent[key]
                    parent[path[-1]] = wrong_type
                    with self.subTest(path=path, wrong_type=wrong_type):
                        self.assert_rejected(json.dumps(value))
        self.assert_rejected(json.dumps(request({"kind": "probe", "plan": {}})))
        self.assert_rejected(json.dumps(execution({"kind": "clear", "policy": POLICY})))
        self.assert_rejected(json.dumps(execution({"kind": "toggle"})))

    def test_versions_request_ids_and_revision_are_strictly_typed_and_bounded(self):
        for version in (None, True, False, 1.0, "1", 0, 2):
            with self.subTest(version=version):
                self.assert_rejected(json.dumps(dict(request({"kind": "probe"}),
                                                     schema_version=version)))
        for nonce in (None, True, 1, "", "a" * 129, "with_space", "../x", "中文", "line\n"):
            with self.subTest(request_id=nonce):
                self.assert_rejected(json.dumps(request({"kind": "probe"}, nonce)))
        self.assertEqual(self.parse(request({"kind": "probe"}, "A0-" * 42 + "B7")),
                         request({"kind": "probe"}, "A0-" * 42 + "B7"))
        for revision in (None, True, 1, "a" * 63, "a" * 65, "A" * 64, "g" * 64):
            with self.subTest(revision=revision):
                self.assert_rejected(json.dumps(execution(set_action(), revision)))

    def test_non_integral_policy_values_and_implicit_dependencies_are_rejected(self):
        cases = []
        for cap in (None, True, False, "55000000", 55_000_000.0, 0, -1, 55_000_001):
            cases.append(dict(POLICY, cap_uw=cap))
        for field, values in (
                ("target_celsius", (True, 90.0, "90", 85, 95)),
                ("polling_seconds", (True, 1.0, "1", 0, 2)),
                ("rapl_debounce_seconds", (True, False, 0.0, "0", -1, 1))):
            cases.extend(dict(FAST_POLICY, **{field: value}) for value in values)
        cases += [dict(POLICY, polling_seconds=1),
                  dict(POLICY, rapl_debounce_seconds=0),
                  dict(POLICY, target_celsius=90, rapl_debounce_seconds=0)]
        for policy in cases:
            with self.subTest(policy=policy):
                self.assert_rejected(json.dumps(execution(set_action(policy))))

    def test_json_document_size_and_non_objects_are_rejected(self):
        for raw in ("", "null", "[]", "true", "1", "{} {}", "{" + " " * 32_768 + "}"):
            with self.subTest(raw=raw[:40]):
                self.assert_rejected(raw)
        # Count UTF-8 bytes rather than Python characters, even for invalid input.
        self.assert_rejected(json.dumps(request({"kind": "probe"}), ensure_ascii=False)
                             + " " * 32_768)

    def test_console_main_rejects_invalid_argv_before_constructing_controller(self):
        valid = json.dumps(request({"kind": "probe"}))
        cases = [[], ["--request"], ["--request", "{"],
                 ["--request", valid, "extra"], ["--root", "/tmp", "--request", valid],
                 ["--request", valid, "--request", valid]]
        for argv in cases:
            with self.subTest(argv=argv), \
                    mock.patch.object(POWER, "PowerController") as controller, \
                    redirect_stdout(io.StringIO()), redirect_stderr(io.StringIO()):
                try:
                    exit_code = POWER.console_main(argv)
                except SystemExit as exc:
                    exit_code = exc.code
                self.assertNotEqual(exit_code, 0)
                controller.assert_not_called()


class PowerConsoleTransactions(unittest.TestCase):
    def setUp(self):
        self.host = FakeHost(self)
        self.controller = self.host.controller
        target = mock.patch.object(self.host.env, "read_temperature_target", return_value={
            "platform_info_raw": 1 << 30,
            "temperature_target_raw": (100 << 16) | (15 << 24),
            "tjmax_celsius": 100, "tcc_offset_celsius": 15, "tcc_activation_celsius": 85,
        })
        target.start()
        self.addCleanup(target.stop)

    def wire(self, value):
        validated = POWER.parse_console_request(json.dumps(value))
        result = POWER.console_handle(self.controller, validated)
        self.assertEqual(set(result), REPLY_KEYS)
        self.assertEqual(result["schema_version"], 1)
        self.assertEqual(result["request_id"], value["request_id"])
        if result["status"] is not None:
            self.assertEqual(set(result["status"]), STATUS_KEYS)
        if result["error"] is not None:
            self.assertEqual(set(result["error"]), {"code", "message", "details"})
            self.assertIsInstance(result["error"]["code"], str)
            self.assertIsInstance(result["error"]["message"], str)
            self.assertTrue(all(isinstance(detail, str) for detail in result["error"]["details"]))
        return result

    def probe(self):
        return self.wire(request({"kind": "probe"}))["status"]

    def execute(self, action, revision=None):
        if revision is None:
            revision = self.probe()["revision"]
        self.assertRegex(revision, r"^[0-9a-f]{64}$")
        return self.wire(execution(action, revision))

    def set_policy(self, policy=None):
        result = self.execute(set_action(policy))
        self.assertEqual(result["outcome"], "applied_verified", result)
        self.assertIsNone(result["error"])
        self.assertIs(result["receipt"]["success"], True)
        return result

    def files(self):
        return {str(path.relative_to(self.host.root)): path.read_bytes()
                for path in self.host.root.rglob("*") if path.is_file()}

    @contextmanager
    def no_writes(self):
        before = self.files()
        with mock.patch.object(self.host.env, "atomic", side_effect=AssertionError("atomic write")) as atomic, \
                mock.patch.object(self.host.env, "remove", side_effect=AssertionError("file removal")) as remove, \
                mock.patch.object(self.host.env, "write_control", side_effect=AssertionError("actuator write")) as writes, \
                mock.patch.object(self.controller, "backup", side_effect=AssertionError("backup write")) as backup:
            yield
            # Production deliberately catches I/O failures. Check attempted calls
            # too, so a swallowed AssertionError cannot make a rejection pass.
            atomic.assert_not_called()
            remove.assert_not_called()
            writes.assert_not_called()
            backup.assert_not_called()
        self.assertEqual(self.files(), before)

    def assert_blocked(self, result):
        self.assertEqual(result["outcome"], "rejected_before_write", result)
        self.assertIsNotNone(result["error"])

    def pending(self, previous_boot=False):
        state = self.controller.load_state()
        owner = self.host.env.process_identity()
        if previous_boot:
            owner = dict(owner, boot_id="previous-boot")
        transaction = str(uuid.uuid4())
        state.update(operation_id=transaction,
                     transaction={"id": transaction, "operation": "set", "owner": owner})
        self.host.write(POWER.STATE, json.dumps(state))

    def test_native_probe_qualifies_and_separates_operational_bound_from_thermal_reference(self):
        status = self.probe()
        with self.no_writes():
            again = self.probe()
        self.assertEqual(status, again)
        self.assertEqual(status["management"], "native")
        self.assertTrue(status["qualified"])
        self.assertIn("set", status["allowed_actions"])
        self.assertNotIn("recover", status["allowed_actions"])
        self.assertNotIn("reapply", status["allowed_actions"])
        self.assertIsNone(status["current_policy"])
        self.assertEqual(status["operational_ceiling_uw"], 140_000_000)
        self.assertEqual(status["minimum_limit_uw"], 35_000_000)
        self.assertEqual(status["thermal_spec_reference_uw"], 55_000_000)
        self.assertFalse(status["holding_now"])
        self.assertEqual({item["interface"] for item in status["observed"]}, {"msr", "mmio"})

    def test_native_unqualified_identity_and_daemon_never_offer_writes(self):
        self.probe()  # Create only the permitted coordination lock before snapshot.
        changes = [("/sys/class/dmi/id/product_name", "foreign product\n"),
                   ("/sys/class/dmi/id/bios_version", "foreign BIOS\n"),
                   ("/proc/cpuinfo", "model name\t: foreign CPU\n"),
                   ("/usr/bin/thermald", "foreign binary\n")]
        for path, foreign in changes:
            original = self.host.path(path).read_bytes()
            with self.subTest(path=path):
                self.host.write(path, foreign)
                with self.no_writes():
                    status = self.probe()
                self.assertFalse(status["qualified"])
                self.assertFalse(status["holding_now"])
                self.assertEqual(status["allowed_actions"], [])
                self.assertIsNone(status["operational_ceiling_uw"])
                self.assertTrue(status["blocking_errors"])
                self.host.path(path).write_bytes(original)

    def test_state_permission_failure_is_unknown_not_native_and_preserves_bytes(self):
        self.set_policy()
        regular = self.host.env.regular

        def denied(path, **kwargs):
            if path == POWER.STATE:
                raise PermissionError("state read denied")
            return regular(path, **kwargs)

        with mock.patch.object(self.host.env, "regular", side_effect=denied), self.no_writes():
            status = self.probe()
        self.assertEqual(status["management"], "unknown")
        self.assertFalse(status["qualified"])
        self.assertFalse(status["holding_now"])
        self.assertEqual(status["allowed_actions"], [])
        self.assertIsNone(status["current_policy"])
        self.assertIn("state read denied", " ".join(status["blocking_errors"]))

    def test_optional_diagnostic_permission_failure_does_not_change_revision_or_qualification(self):
        self.set_policy(FAST_POLICY)
        before = self.probe()
        with mock.patch.object(self.host.env, "read_temperature_target",
                               side_effect=PermissionError("MSR diagnostic denied")), \
                mock.patch.object(self.host.env, "read_package_temperature",
                                  side_effect=PermissionError("temperature diagnostic denied")), \
                self.no_writes():
            after = self.probe()
        self.assertEqual(after["revision"], before["revision"])
        self.assertTrue(after["qualified"])
        self.assertTrue(after["holding_now"])
        self.assertEqual(after["blocking_errors"], [])
        self.assertTrue(after["diagnostic_errors"])
        self.assertIn("reapply", after["allowed_actions"])

    def test_hardware_activation_warning_is_diagnostic_at_and_below_the_software_target(self):
        self.set_policy(FAST_POLICY)
        reference = {"platform_info_raw": 1 << 30,
                     "temperature_target_raw": 100 << 16,
                     "tjmax_celsius": 100, "tcc_offset_celsius": 5,
                     "tcc_activation_celsius": 95}
        with mock.patch.object(self.host.env, "read_temperature_target", return_value=reference):
            baseline = self.probe()
        self.assertEqual(baseline["diagnostic_errors"], [])
        for activation, warning_count in ((85, 1), (90, 1), (95, 0)):
            value = dict(reference, tcc_offset_celsius=100 - activation,
                         temperature_target_raw=(100 << 16) | ((100 - activation) << 24),
                         tcc_activation_celsius=activation)
            with self.subTest(activation=activation), \
                    mock.patch.object(self.host.env, "read_temperature_target", return_value=value), \
                    self.no_writes():
                status = self.probe()
            self.assertEqual(status["revision"], baseline["revision"])
            self.assertEqual(status["current_policy"], FAST_POLICY)
            self.assertTrue(status["qualified"])
            self.assertTrue(status["holding_now"])
            self.assertEqual(status["allowed_actions"], baseline["allowed_actions"])
            self.assertEqual(status["blocking_errors"], [])
            self.assertEqual(len(status["diagnostic_errors"]), warning_count)
        with mock.patch.object(self.host.env, "read_temperature_target",
                               side_effect=PermissionError("reference unavailable")):
            unavailable = self.probe()
        self.assertEqual(len(unavailable["diagnostic_errors"]), 1)
        self.assertIn("reference unavailable", unavailable["diagnostic_errors"][0])
        self.assertEqual(unavailable["revision"], baseline["revision"])
        self.assertTrue(unavailable["qualified"])
        self.assertTrue(unavailable["holding_now"])

    def test_complete_set_applies_real_controller_and_preserves_explicit_native_options(self):
        before = self.host.protected_controls()
        result = self.set_policy()
        self.assertEqual(result["status"]["management"], "managed")
        self.assertTrue(result["status"]["holding_now"])
        self.assertEqual(result["status"]["current_policy"], POLICY)
        self.assertEqual(self.host.limits(), [55_000_000, 55_000_000])
        self.assertEqual(self.host.protected_controls(), before)
        self.assertTrue(self.host.active)

    def test_fast_policy_and_later_cap_edit_keep_90_one_and_explicit_zero(self):
        self.set_policy(FAST_POLICY)
        policy = dict(FAST_POLICY, cap_uw=45_000_000)
        result = self.set_policy(policy)
        self.assertEqual(result["status"]["current_policy"], policy)
        state = self.controller.load_state()
        self.assertEqual((state["target_celsius"], state["polling_seconds"],
                          state["rapl_debounce_seconds"]), (90, 1, 0))
        self.assertEqual(self.host.limits(), [45_000_000, 45_000_000])

    def test_stale_persistent_state_revision_rejects_all_managed_actions_before_backup(self):
        self.set_policy()
        for action in (set_action(dict(POLICY, cap_uw=45_000_000)),
                       {"kind": "clear"}, {"kind": "reapply"}):
            revision = self.probe()["revision"]
            self.host.write(POWER.STATE, self.host.read(POWER.STATE) + "\n")
            with self.subTest(action=action), self.no_writes():
                self.assert_blocked(self.execute(action, revision))

    def test_stale_preference_boot_and_native_initial_limit_reject_before_staging(self):
        for change in ("preference", "boot", "initial_limit"):
            revision = self.probe()["revision"]
            with self.subTest(change=change):
                if change == "preference":
                    self.host.write(POWER.PREF, "1\n")
                    with self.no_writes():
                        self.assert_blocked(self.execute(set_action(), revision))
                elif change == "boot":
                    with mock.patch.object(self.host.env, "boot_id", return_value="another-boot"), \
                            self.no_writes():
                        self.assert_blocked(self.execute(set_action(), revision))
                else:
                    self.host.limit_path(0).write_text("130000000\n")
                    with self.no_writes():
                        self.assert_blocked(self.execute(set_action(), revision))

    def test_probe_and_execute_share_the_existing_nonblocking_controller_lock(self):
        revision = self.probe()["revision"]
        with self.controller.lock(), self.no_writes():
            probe = self.wire(request({"kind": "probe"}))
            self.assertIsNotNone(probe["error"])
            if probe["status"] is not None:
                self.assertFalse(probe["status"]["qualified"])
                self.assertEqual(probe["status"]["allowed_actions"], [])
            self.assert_blocked(self.execute(set_action(), revision))

    def test_managed_live_pl1_changes_do_not_stale_revision_or_raise_lower_limits(self):
        self.set_policy()
        revision = self.probe()["revision"]
        self.host.limit_path(0).write_text("30000000\n")
        self.host.limit_path(1).write_text("45000000\n")
        self.assertEqual(self.probe()["revision"], revision)
        with mock.patch.object(self.host.env, "write_control", wraps=self.host.env.write_control) as writes:
            result = self.execute({"kind": "reapply"}, revision)
        self.assertEqual(result["outcome"], "applied_verified", result)
        writes.assert_not_called()
        self.assertEqual(self.host.limits(), [30_000_000, 45_000_000])

    def test_initial_cas_policy_controls_reapply_and_restoration_when_pre_entry_state_differs(self):
        # An independently sampled outer observation can see A, while the
        # operation's actual initial lock validates B. The proof must use B.
        policy_a = dict(POLICY, cap_uw=45_000_000)
        policy_b = deepcopy(FAST_POLICY)
        for kind in ("reapply", "set"):
            with self.subTest(kind=kind):
                saved_b = self.set_policy(policy_b)["status"]
                files_b = {path: self.host.env.regular(path, missing=True)
                           for path in (POWER.STATE, POWER.XML, POWER.DROPIN, POWER.PREF)}
                observed_a = self.set_policy(policy_a)["status"]
                self.assertNotEqual(observed_a["revision"], saved_b["revision"])
                method_name = "apply" if kind == "reapply" else "set_limit"
                operation = getattr(self.controller, method_name)
                entry_observations = []

                def enter_with_b(*args, **kwargs):
                    # Simulate another owner restoring B immediately before the
                    # real operation enters its own initial lock. Keep the
                    # controller, CAS, restart, apply and rollback production code.
                    entry_observations.append(self.controller.console_probe())
                    for path, data in files_b.items():
                        target = self.host.path(path)
                        if data is None:
                            if os.path.lexists(target):
                                target.unlink()
                        else:
                            target.write_bytes(data)
                    return operation(*args, **kwargs)

                if kind == "set":
                    self.host.restart_failures = [True, False]
                    action = set_action(dict(policy_b, cap_uw=35_000_000))
                else:
                    action = {"kind": "reapply"}
                with mock.patch.object(self.controller, method_name, side_effect=enter_with_b) as entry, \
                        mock.patch.object(self.host.env, "write_control", wraps=self.host.env.write_control) as writes:
                    result = self.execute(action, saved_b["revision"])
                entry.assert_called_once()
                self.assertEqual(len(entry_observations), 1)
                self.assertEqual(entry_observations[0]["current_policy"], policy_a)
                self.assertEqual(entry_observations[0]["revision"], observed_a["revision"])
                self.assertEqual(result["status"]["current_policy"], policy_b)
                self.assertTrue(result["status"]["qualified"])
                self.assertTrue(result["status"]["holding_now"])
                self.assertEqual(result["status"]["current_policy"]["rapl_debounce_seconds"], 0)
                writes.assert_not_called()
                if kind == "reapply":
                    self.assertEqual(result["outcome"], "applied_verified", result)
                    self.assertIsNone(result["error"])
                    self.assertIs(result["receipt"]["success"], True)
                else:
                    self.assertEqual(result["outcome"], "failed_restored_verified", result)
                    self.assertIn("injected restart failure", result["error"]["message"])
                    self.assertTrue(any(detail.startswith("primary_error:")
                                        for detail in result["error"]["details"]))

    def test_reapply_lowers_only_the_interface_above_cap(self):
        self.set_policy()
        self.host.limit_path(0).write_text("30000000\n")
        self.host.limit_path(1).write_text("140000000\n")
        status = self.probe()
        self.assertFalse(status["holding_now"])
        self.assertTrue(status["qualified"])
        self.assertIn("reapply", status["allowed_actions"])
        before = self.host.protected_controls()
        with mock.patch.object(self.host.env, "write_control", wraps=self.host.env.write_control) as writes:
            result = self.execute({"kind": "reapply"}, status["revision"])
        self.assertEqual(result["outcome"], "applied_verified", result)
        self.assertEqual(writes.call_args_list,
                         [mock.call(POWER.ZONES[1] + "/constraint_0_power_limit_uw", 55_000_000)])
        self.assertEqual(self.host.limits(), [30_000_000, 55_000_000])
        self.assertEqual(self.host.protected_controls(), before)

    def test_clear_managed_restores_native_service_without_claiming_a_fixed_watt_value(self):
        self.set_policy(FAST_POLICY)
        runner = self.host.env.runner

        def stock_limits(argv):
            result = runner(argv)
            if argv == ["systemctl", "restart", "thermald.service"] \
                    and not self.host.path(POWER.DROPIN).exists():
                self.host.limit_path(0).write_text("73000000\n")
                self.host.limit_path(1).write_text("81000000\n")
            return result

        self.host.env.runner = stock_limits
        with mock.patch.object(self.host.env, "write_control", wraps=self.host.env.write_control) as writes:
            result = self.execute({"kind": "clear"})
        self.assertEqual(result["outcome"], "applied_verified", result)
        writes.assert_not_called()
        self.assertEqual(result["status"]["management"], "native")
        self.assertIsNone(result["status"]["current_policy"])
        self.assertFalse(result["status"]["holding_now"])
        self.assertEqual(self.host.limits(), [73_000_000, 81_000_000])
        self.assertEqual({item["pl1_uw"] for item in result["status"]["observed"]},
                         {73_000_000, 81_000_000})
        self.assertFalse(any(self.host.path(path).exists() for path in
                             (POWER.STATE, POWER.XML, POWER.DROPIN)))

    def test_clear_already_native_is_rejected_without_forging_a_transaction(self):
        status = self.probe()
        self.assertNotIn("clear", status["allowed_actions"])
        self.host.commands.clear()
        with self.no_writes():
            result = self.execute({"kind": "clear"}, status["revision"])
        self.assert_blocked(result)
        self.assertEqual(result["status"]["management"], "native")
        self.assertTrue(all(argv[:2] not in (["systemctl", "restart"],
                                           ["systemctl", "daemon-reload"])
                            for argv in self.host.commands))

    def test_live_pending_owner_cannot_be_recovered_or_replaced(self):
        self.set_policy()
        revision = self.probe()["revision"]
        self.pending()
        with self.no_writes():
            status = self.probe()
            self.assertEqual(status["management"], "recovery_pending")
            self.assertEqual(status["allowed_actions"], [])
            for action in ({"kind": "recover"}, {"kind": "clear"}, {"kind": "reapply"}, set_action()):
                with self.subTest(action=action):
                    self.assert_blocked(self.execute(action, revision))

    def test_dead_pending_owner_exposes_only_recover_and_restores_stock_without_actuator_writes(self):
        self.set_policy(FAST_POLICY)
        self.pending(previous_boot=True)
        status = self.probe()
        self.assertEqual(status["management"], "recovery_pending")
        self.assertEqual(status["allowed_actions"], ["recover"])
        with mock.patch.object(self.host.env, "write_control", wraps=self.host.env.write_control) as writes:
            result = self.execute({"kind": "recover"}, status["revision"])
        self.assertEqual(result["outcome"], "applied_verified", result)
        self.assertEqual(result["status"]["management"], "native")
        self.assertIsNone(result["status"]["current_policy"])
        writes.assert_not_called()
        self.assertFalse(self.host.path(POWER.STATE).exists())

    def test_dead_pending_foreign_bytes_and_orphan_configuration_never_offer_recovery(self):
        self.set_policy()
        self.pending(previous_boot=True)
        self.host.write(POWER.XML, self.host.read(POWER.XML) + "<!-- foreign -->\n")
        with self.no_writes():
            foreign = self.probe()
        self.assertEqual(foreign["allowed_actions"], [])
        self.assertTrue(foreign["blocking_errors"])
        self.host.path(POWER.STATE).unlink()
        with self.no_writes():
            orphan = self.probe()
        self.assertEqual(orphan["management"], "unknown")
        self.assertEqual(orphan["allowed_actions"], [])
        self.assertTrue(orphan["blocking_errors"])

    def test_stale_recovery_revision_does_not_remove_any_pending_state(self):
        self.set_policy()
        self.pending(previous_boot=True)
        revision = self.probe()["revision"]
        self.host.write(POWER.STATE, self.host.read(POWER.STATE) + "\n")
        with self.no_writes():
            self.assert_blocked(self.execute({"kind": "recover"}, revision))

    def test_failed_set_reports_verified_restoration_and_preserves_primary_error(self):
        self.host.restart_failures = [True, False]
        result = self.execute(set_action())
        self.assertEqual(result["outcome"], "failed_restored_verified", result)
        self.assertEqual(result["status"]["management"], "native")
        self.assertIsNotNone(result["error"])
        self.assertIn("injected restart failure", result["error"]["message"])
        self.assertTrue(any(detail.startswith("primary_error:")
                            for detail in result["error"]["details"]))
        self.assertTrue(self.host.active)
        self.assertFalse(self.host.path(POWER.STATE).exists())

    def test_primary_rollback_and_receipt_failures_remain_independent_and_state_unknown(self):
        runner = self.host.env.runner
        restarts = 0

        def fail_restart(argv):
            nonlocal restarts
            if argv == ["systemctl", "restart", "thermald.service"]:
                self.host.commands.append(list(argv))
                restarts += 1
                self.host.active = False
                message = "primary restart failure" if restarts == 1 else "rollback restart failure"
                return subprocess.CompletedProcess(argv, 1, "", message)
            return runner(argv)

        receipt = self.controller.receipt

        def fail_failure_receipt(operation, success, **details):
            if not success:
                raise OSError("receipt write denied")
            return receipt(operation, success, **details)

        self.host.env.runner = fail_restart
        with mock.patch.object(self.controller, "receipt", side_effect=fail_failure_receipt):
            result = self.execute(set_action())
        self.assertEqual(result["outcome"], "failed_state_unknown", result)
        details = result["error"]["details"]
        for prefix, message in (("primary_error:", "primary restart failure"),
                                ("rollback_error:", "rollback restart failure"),
                                ("receipt_error:", "receipt write denied")):
            self.assertTrue(any(detail.startswith(prefix) and message in detail
                                for detail in details), (prefix, details))
        self.assertIn("primary restart failure", result["error"]["message"])
        self.assertNotEqual(result["outcome"], "applied_verified")

    def test_receipt_write_failure_does_not_hide_primary_or_forge_failed_restoration(self):
        self.host.restart_failures = [True, False]
        receipt = self.controller.receipt

        def fail_failure_receipt(operation, success, **details):
            if not success:
                raise OSError("failure receipt denied")
            return receipt(operation, success, **details)

        with mock.patch.object(self.controller, "receipt", side_effect=fail_failure_receipt):
            result = self.execute(set_action())
        self.assertEqual(result["outcome"], "failed_restored_verified", result)
        self.assertEqual(result["status"]["management"], "native")
        details = result["error"]["details"]
        self.assertTrue(any(detail.startswith("primary_error:") and "injected restart failure" in detail
                            for detail in details))
        self.assertTrue(any(detail.startswith("receipt_error:") and "failure receipt denied" in detail
                            for detail in details))
        self.assertFalse(any(detail.startswith("rollback_error:") for detail in details))
        self.assertTrue(self.host.active)
        self.assertFalse(self.host.path(POWER.STATE).exists())

    def test_post_read_failure_does_not_turn_completed_write_into_verified_success(self):
        regular = self.host.env.regular
        receipt = self.controller.receipt
        completed = False

        def capture_completion(operation, success, **details):
            nonlocal completed
            value = receipt(operation, success, **details)
            if operation == "set" and success:
                completed = True
            return value

        def fail_post_read(path, **kwargs):
            if completed and path == POWER.STATE:
                raise PermissionError("post-read state denied")
            return regular(path, **kwargs)

        with mock.patch.object(self.controller, "receipt", side_effect=capture_completion), \
                mock.patch.object(self.host.env, "regular", side_effect=fail_post_read):
            result = self.execute(set_action())
        self.assertTrue(completed)
        self.assertEqual(result["outcome"], "failed_state_unknown", result)
        self.assertTrue(any(detail.startswith("post_read_error:") and "post-read state denied" in detail
                            for detail in result["error"]["details"]))
        self.assertEqual(self.host.limits(), [55_000_000, 55_000_000])

    def test_console_main_outputs_one_correlated_json_reply_for_real_probe(self):
        stdout, stderr = io.StringIO(), io.StringIO()
        value = request({"kind": "probe"}, "main-probe-1")
        with mock.patch.object(POWER, "PowerController", return_value=self.controller) as factory, \
                redirect_stdout(stdout), redirect_stderr(stderr):
            code = POWER.console_main(["--request", json.dumps(value)])
        self.assertEqual(code, 0)
        factory.assert_called_once_with()
        result = json.loads(stdout.getvalue())
        self.assertEqual(set(result), REPLY_KEYS)
        self.assertEqual(result["request_id"], "main-probe-1")
        self.assertEqual(result["outcome"], "observed")
        self.assertTrue(result["status"]["qualified"])
        self.assertEqual(stderr.getvalue(), "")


if __name__ == "__main__":
    unittest.main()
