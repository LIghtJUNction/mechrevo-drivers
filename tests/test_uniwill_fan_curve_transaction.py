"""Compile the real fan-table engine with transport-only fault injection.

No register transaction is reimplemented here. The production header controls
validation, snapshot, commit, verification, rollback and retry. This suite never
opens hardware; fake EC memory is private to each bounded C subprocess.
"""
import hashlib
import json
import os
from pathlib import Path
import resource
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
HEADER = Path(os.environ.get("UNIWILL_FAN_CURVE_HEADER", ROOT / "src/uniwill_fan_curve_transaction.h"))


class UniwillFanCurveTransaction(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        parent = Path(os.environ.get("MECHREVO_TEST_TMPDIR", ROOT / ".local-tests"))
        parent.mkdir(parents=True, exist_ok=True)
        cls.scratch = tempfile.TemporaryDirectory(prefix="fan-curve-engine-", dir=parent)
        cls.addClassCleanup(cls.scratch.cleanup)
        cls.results = []
        template = (ROOT / "tests/fixtures/uniwill_fan_curve_transaction.c").read_text()
        marker = "/* @ACTUAL_ENGINE_HEADER@ */"
        if template.count(marker) != 1:
            raise AssertionError("Missing unique production-header include marker")
        if not HEADER.is_file():
            raise AssertionError("Missing production fan-curve engine: " + str(HEADER))
        source = Path(cls.scratch.name) / "engine.c"
        source.write_text(template.replace(marker, '#include ' + json.dumps(str(HEADER.resolve()))))
        cls.binary = Path(cls.scratch.name) / "engine"
        command = [os.environ.get("CC", "cc"), "-std=c11", "-O2", "-Wall", "-Wextra", "-Werror",
                   str(source), "-o", str(cls.binary)]
        result = subprocess.run(command, capture_output=True, text=True, timeout=60)
        cls.evidence = {"schema_version": 1, "scope": "actual production engine with simulated EC transport; no hardware acceptance",
                        "header": str(HEADER.resolve()), "header_sha256": hashlib.sha256(HEADER.read_bytes()).hexdigest(),
                        "fixture_sha256": hashlib.sha256(template.encode()).hexdigest(),
                        "compile_argv": command, "compile_returncode": result.returncode,
                        "compile_stdout": result.stdout, "compile_stderr": result.stderr}
        if result.returncode:
            raise AssertionError(result.stdout + result.stderr)

    @classmethod
    def tearDownClass(cls):
        report = os.environ.get("UNIWILL_FAN_CURVE_REPORT")
        if report:
            payload = dict(cls.evidence, results=cls.results)
            with Path(report).open("x") as output:
                json.dump(payload, output, indent=2)
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

    def test_every_snapshot_read_failure_prevents_all_writes(self):
        self.check_case("snapshot_failures")

    def test_successful_94_position_order_preserves_reserved_and_restores_raw(self):
        self.check_case("successful_order")

    def test_every_commit_store_failure_with_and_without_landing_restores_exact_raw(self):
        self.check_case("commit_write_failures")

    def test_each_commit_read_fault_leaves_only_verified_apply_or_exact_rollback(self):
        self.check_case("commit_read_failures")

    def test_each_restore_store_failure_retains_snapshot_disabled_and_explicit_retry(self):
        self.check_case("restore_failures")

    def test_each_restore_read_failure_or_mismatch_retains_snapshot_for_retry(self):
        self.check_case("restore_read_failures")

    def test_enable_side_effects_cannot_hide_table_or_owned_bit_change(self):
        self.check_case("enable_side_effects")

    def test_read_only_snapshot_compare_and_pure_lossless_encoding(self):
        self.check_case("read_only_and_encoding")

    def test_landed_enable_and_failed_rollback_entry_attempts_emergency_atomic_disable(self):
        self.check_case("landed_enable_failed_restore_entry")

    def test_whole_input_validation_is_zero_io_and_full_mode_is_zero_write(self):
        self.check_case("invalid_inputs")

    def test_all_control_bytes_preserve_unowned_bits_on_commit_and_restore(self):
        self.check_case("rmw_all_bytes")

    def test_reserved_byte_external_change_cannot_be_hidden_or_overwritten(self):
        self.check_case("reserved_change")


if __name__ == "__main__":
    unittest.main()
