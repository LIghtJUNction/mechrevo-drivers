"""Run actual file/lease ioctl routing with provider boundary spies, never hardware."""
import hashlib
import json
import os
from pathlib import Path
import resource
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class UniwillFanCurveIoctl(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        parent = Path(os.environ.get("MECHREVO_TEST_TMPDIR", ROOT / ".local-tests"))
        parent.mkdir(parents=True, exist_ok=True)
        cls.scratch = tempfile.TemporaryDirectory(prefix="fan-curve-ioctl-", dir=parent)
        cls.addClassCleanup(cls.scratch.cleanup)
        driver = Path(os.environ.get("UNIWILL_CURVE_IOCTL_SOURCE", ROOT / "src/tuxedo_io/tuxedo_io.c"))
        text = driver.read_text()
        start = text.index("struct uw_curve_file_context {")
        end = text.index("static long clevo_ioctl_interface(", start)
        actual = text[start:end]
        template = (ROOT / "tests/fixtures/uniwill_fan_curve_ioctl.c").read_text()
        cls.evidence = {"schema_version": 1, "scope": "actual lifecycle/ioctl plus provider boundary spies; no hardware acceptance",
                        "driver_sha256": hashlib.sha256(driver.read_bytes()).hexdigest(),
                        "uapi_sha256": hashlib.sha256((ROOT / "src/uniwill_fan_curve_ioctl.h").read_bytes()).hexdigest(),
                        "fixture_sha256": hashlib.sha256(template.encode()).hexdigest()}
        source = Path(cls.scratch.name) / "ioctl.c"
        source.write_text(template.replace("/* @UAPI_HEADER@ */", '#include ' + json.dumps(str(ROOT / "src/uniwill_fan_curve_ioctl.h")))
                         .replace("/* @ACTUAL_FILE_LIFECYCLE_AND_IOCTL@ */", actual))
        cls.binary = Path(cls.scratch.name) / "ioctl"
        command = ["cc", "-std=c11", "-O2", "-Wall", "-Wextra", "-Werror", "-pthread", str(source), "-o", str(cls.binary)]
        result = subprocess.run(command, text=True, capture_output=True, timeout=60)
        cls.evidence.update(compile_argv=command, compile_returncode=result.returncode,
                            compile_stdout=result.stdout, compile_stderr=result.stderr)
        cls.results = []
        if result.returncode:
            raise AssertionError(result.stdout + result.stderr)

    @classmethod
    def tearDownClass(cls):
        if report := os.environ.get("UNIWILL_CURVE_IOCTL_REPORT"):
            with Path(report).open("x") as output:
                json.dump(dict(cls.evidence, results=cls.results), output, indent=2)
                output.write("\n")

    def check_case(self, case):
        def no_core():
            resource.setrlimit(resource.RLIMIT_CORE, (0, 0))
        result = subprocess.run([str(self.binary), case], text=True, capture_output=True,
                                timeout=10, preexec_fn=no_core)
        self.results.append({"case": case, "returncode": result.returncode,
                             "stdout": result.stdout, "stderr": result.stderr})
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_each_open_has_unique_owner_and_only_its_lease_is_closed(self):
        self.check_case("unique_files")

    def test_only_exact_commands_and_privileged_mutations_reach_provider(self):
        self.check_case("permission_and_shape")

    def test_copy_failures_and_provider_errors_preserve_cleanup_ownership(self):
        self.check_case("failed_copies_and_receipts")

    def test_query_marks_a_lost_context_token_revoked(self):
        self.check_case("revoked_receipt")

    def test_failed_acquire_copyout_is_cleaned_before_a_same_file_reacquire(self):
        self.check_case("same_file_failed_ack")

    def test_same_file_acquire_ack_is_serialized_with_release_and_reacquire(self):
        self.check_case("same_file_serializes")


if __name__ == "__main__":
    unittest.main()
