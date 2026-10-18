"""Run the actual lease provider and legacy/PM callers with a private EC transport.

This is offline state-machine acceptance, not loaded-module or fan hardware
acceptance. No lease/transaction behavior is replaced by a compatibility stub.
"""
import hashlib
import json
import os
from pathlib import Path
import re
import resource
import subprocess
import tempfile
import unittest

from test_uniwill_fan_runtime import function

ROOT = Path(__file__).resolve().parents[1]
HEADER = Path(os.environ.get("UNIWILL_FAN_PROVIDER_HEADER", ROOT / "src/uniwill_keyboard.h"))


class UniwillFanCurveProvider(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        parent = Path(os.environ.get("MECHREVO_TEST_TMPDIR", ROOT / ".local-tests"))
        parent.mkdir(parents=True, exist_ok=True)
        cls.scratch = tempfile.TemporaryDirectory(prefix="fan-curve-provider-", dir=parent)
        cls.addClassCleanup(cls.scratch.cleanup)
        cls.results = []
        keyboard = HEADER.read_text()
        keyboard = re.sub(
            r"#if LINUX_VERSION_CODE < KERNEL_VERSION\(6, 11, 0\)\n"
            r"static int uniwill_keyboard_remove\([^\n]*\)\n#else\n"
            r"(static void uniwill_keyboard_remove\([^\n]*\))\n#endif", r"\1", keyboard)
        interfaces = (ROOT / "src/uniwill_interfaces.h").read_text()
        constants = []
        for source, name in ((keyboard, "FAN_ON_MIN_SPEED_PERCENT"),
                             (interfaces, "NB02_FAN_SPEED_MAX"),
                             (interfaces, "UW_EC_REG_CUSTOM_PROFILE")):
            match = re.search(r"(?m)^#define " + name + r"\s+.+$", source)
            if not match:
                raise AssertionError("Missing production constant: " + name)
            constants.append(match.group())
        start = keyboard.index("static u8 direct_fan_control_current_value_fan0")
        end = keyboard.index("static u8 uniwill_touchp_toggle_seq", start)
        fan = keyboard[start:end]
        if "UW_CURVE_PROVIDER_BEGIN" not in fan or "uw_fan_curve_acquire" not in fan:
            raise AssertionError("Actual lease provider missing from extraction")
        names = ("uniwill_set_custom_profile_mode", "uniwill_write_ec_ram_with_retry")
        tail = ("uw_restore_fan_state_locked", "uniwill_keyboard_suspend",
                "uniwill_keyboard_resume", "uniwill_keyboard_remove", "uniwill_keyboard_abort_probe")
        production = "\n\n".join(constants + [function(keyboard, n) for n in names]
                                    + [fan] + [function(keyboard, n) for n in tail])
        template = (ROOT / "tests/fixtures/uniwill_fan_curve_provider.c").read_text()
        includes = "\n".join('#include ' + json.dumps(str((ROOT / p).resolve()))
                               for p in ("src/uniwill_fan_curve_ioctl.h", "src/uniwill_fan_curve_transaction.h"))
        source = Path(cls.scratch.name) / "provider.c"
        source.write_text(template.replace("/* @ACTUAL_HEADERS@ */", includes)
                         .replace("/* @ACTUAL_DRIVER@ */", production))
        cls.binary = Path(cls.scratch.name) / "provider"
        command = [os.environ.get("CC", "cc"), "-std=c11", "-O2", "-Wall", "-Wextra", "-Werror", "-pthread",
                   str(source), "-o", str(cls.binary)]
        result = subprocess.run(command, capture_output=True, text=True, timeout=60)
        cls.evidence = {"schema_version": 1,
                        "scope": "actual provider, engine, legacy setters and PM/remove; mocked transport and kernel runtime; no hardware acceptance",
                        "keyboard_sha256": hashlib.sha256(HEADER.read_bytes()).hexdigest(),
                        "engine_sha256": hashlib.sha256((ROOT / "src/uniwill_fan_curve_transaction.h").read_bytes()).hexdigest(),
                        "uapi_sha256": hashlib.sha256((ROOT / "src/uniwill_fan_curve_ioctl.h").read_bytes()).hexdigest(),
                        "fixture_sha256": hashlib.sha256(template.encode()).hexdigest(),
                        "extracted_source_sha256": hashlib.sha256(production.encode()).hexdigest(),
                        "compile_argv": command, "compile_returncode": result.returncode,
                        "compile_stdout": result.stdout, "compile_stderr": result.stderr}
        if result.returncode:
            raise AssertionError(result.stdout + result.stderr)

    @classmethod
    def tearDownClass(cls):
        report = os.environ.get("UNIWILL_FAN_PROVIDER_REPORT")
        if report:
            with Path(report).open("x") as output:
                json.dump(dict(cls.evidence, results=cls.results), output, indent=2)
                output.write("\n")

    def check_case(self, case):
        def no_core():
            resource.setrlimit(resource.RLIMIT_CORE, (0, 0))
        result = subprocess.run([str(self.binary), case], capture_output=True, text=True,
                                timeout=20, preexec_fn=no_core)
        entry = {"case": case, "returncode": result.returncode,
                 "stdout": result.stdout, "stderr": result.stderr}
        if result.returncode == 0:
            entry.update(json.loads(result.stdout))
        self.results.append(entry)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_acquire_is_read_only_and_refuses_unqualified_full_uncertain_and_worker_states(self):
        self.check_case("acquire_contract")

    def test_every_acquire_snapshot_read_failure_is_zero_write_and_unowned(self):
        self.check_case("acquire_read_failures")

    def test_active_lease_gates_actual_legacy_setters_and_worker_without_io_or_requeue(self):
        self.check_case("active_legacy")

    def test_orphaned_failed_close_keeps_anchor_and_module_until_new_owner_explicit_restore(self):
        self.check_case("orphan_recovery")

    def test_owner_and_epoch_generation_lease_tokens_reject_forgery_and_stale_requests(self):
        self.check_case("identity")

    def test_metadata_and_header_validation_cannot_authorize_hardware_writes(self):
        self.check_case("abi_validation")

    def test_multiple_apply_transactions_restore_original_raw_and_prelease_manual_pair(self):
        self.check_case("raw_restore")

    def test_changed_expected_raw_is_not_overwritten_and_no_apply_close_is_read_only(self):
        self.check_case("expected_mismatch")

    def test_failed_unapplied_transaction_release_clears_receipt_errors_without_writes(self):
        self.check_case("failed_unapplied_release")

    def test_rebind_cannot_write_old_recovery_anchor_after_capability_or_identity_loss(self):
        self.check_case("rebind_unqualified")

    def test_suspend_revoke_restore_and_late_close_cannot_rewrite_suspended_or_new_lease(self):
        self.check_case("pm_lifecycle")

    def test_failed_pm_revoke_preserves_active_owner_and_recovery_for_explicit_retry(self):
        self.check_case("pm_recovery")

    def test_get_reports_transport_failure_without_rewriting_transaction_receipt(self):
        self.check_case("metadata_transport_failure")

    def test_each_manual_pair_restore_failure_keeps_anchor_and_disabled_curve_until_retry(self):
        self.check_case("manual_target_restore_failure")

    def test_failed_remove_keeps_orphan_anchor_then_rebind_restores_and_revokes_old_fd(self):
        self.check_case("failed_remove_recovery")

    def test_abort_probe_keeps_original_error_even_when_cleanup_succeeds_or_fails(self):
        self.check_case("abort_preserves_errno")

    def test_remove_revokes_before_resource_teardown_and_late_close_is_zero_io(self):
        self.check_case("remove_lifecycle")


if __name__ == "__main__":
    unittest.main()
