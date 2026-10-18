"""Actual CPU helper wire cases for the Rust decoder; isolated FakeHost only.

Run with python3 -B. No pytest, authorization, hardware or real service command.
The existing FakeHost owns and cleans every policy/config/actuator fixture.
"""
from contextlib import contextmanager, redirect_stderr, redirect_stdout
import io
import json
import os
from pathlib import Path
import sys
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tests"))
from test_power_console_protocol import (  # noqa: E402
    FAST_POLICY, POWER, PowerConsoleTransactions, request, set_action,
)


@contextmanager
def isolated_host():
    case = PowerConsoleTransactions()
    try:
        case.setUp()
        yield case
    finally:
        case.doCleanups()


def generate_cases():
    # The verifier provides TMPDIR. Local direct runs stay in the known test
    # output root; never use /sys or installed controller paths for fixture I/O.
    os.environ.setdefault("MECHREVO_TEST_TMPDIR", os.environ.get(
        "TMPDIR", str(ROOT / ".local-tests" / "rust-target" / "cpu-power-integration")))
    cases = []

    def emit(name, case, operation, expected, previous=None, malformed=False):
        value = request(operation, "actual-wire-" + name)
        raw = json.dumps(value, ensure_ascii=False)
        if malformed:
            raw = raw[:-1] + ',"schema_version":1}'
        stdout, stderr = io.StringIO(), io.StringIO()
        with mock.patch.object(POWER, "PowerController", return_value=case.controller), \
                redirect_stdout(stdout), redirect_stderr(stderr):
            exit_status = POWER.console_main(["--request", raw])
        terminal = json.loads(stdout.getvalue())
        assert terminal["outcome"] == expected, (name, terminal)
        cases.append({"case": name, "request": value, "previous_status": previous,
                      "stdout": stdout.getvalue(), "stderr": stderr.getvalue(),
                      "exit_status": exit_status, "expected_outcome": expected})

    def execute(name, case, action, expected="applied_verified"):
        before = case.controller.console_probe()
        emit(name, case, {"kind": "execute", "plan": {
            "revision": before["revision"], "action": action}}, expected, before)

    with isolated_host() as case:
        emit("native-probe", case, {"kind": "probe"}, "observed")
        execute("set-native-options", case, set_action())
        emit("managed-probe", case, {"kind": "probe"}, "observed")
        execute("set-fast-options", case, set_action(dict(FAST_POLICY, cap_uw=45_000_000)))
        case.host.limit_path(0).write_text("30000000\n")
        case.host.limit_path(1).write_text("140000000\n")
        execute("downward-only-reapply", case, {"kind": "reapply"})
        execute("clear", case, {"kind": "clear"})
        emit("native-below-minimum-probe", case, {"kind": "probe"}, "observed")

    with isolated_host() as case:
        case.set_policy(FAST_POLICY)
        case.pending(previous_boot=True)
        emit("recovery-pending-probe", case, {"kind": "probe"}, "observed")
        execute("recover", case, {"kind": "recover"})

    with isolated_host() as case:
        case.set_policy(FAST_POLICY)
        case.pending(previous_boot=True)
        case.host.limit_path(0).write_text("30000000\n")
        case.host.limit_path(1).write_text("45000000\n")
        execute("recover-below-minimum", case, {"kind": "recover"})

    for name, no_receipt in [("failed-restored", False), ("failed-restored-null-receipt", True)]:
        with isolated_host() as case:
            case.set_policy(FAST_POLICY)
            case.host.restart_failures = [True, False]
            if no_receipt:
                with mock.patch.object(case.controller, "failure_receipt", return_value=(
                        None, PermissionError("injected failure receipt write denial"))):
                    execute(name, case, set_action(dict(FAST_POLICY, cap_uw=45_000_000)),
                            "failed_restored_verified")
            else:
                execute(name, case, set_action(dict(FAST_POLICY, cap_uw=45_000_000)),
                        "failed_restored_verified")

    with isolated_host() as case:
        case.set_policy(FAST_POLICY)
        before = case.controller.console_probe()
        case.host.write(POWER.STATE, case.host.read(POWER.STATE) + "\n")
        emit("stale-revision", case, {"kind": "execute", "plan": {
            "revision": before["revision"], "action": {"kind": "clear"}}},
            "rejected_before_write", before)

    with isolated_host() as case:
        case.host.write("/sys/class/dmi/id/bios_version", "unqualified firmware\n")
        emit("partial-unqualified-probe", case, {"kind": "probe"}, "observed")

    with isolated_host() as case:
        emit("malformed-request-rejection", case, {"kind": "probe"},
             "rejected_before_write", malformed=True)
    assert len(cases) == 15
    return cases


if __name__ == "__main__":
    print(json.dumps({"schema_version": 1, "cases": generate_cases()},
                     sort_keys=True, ensure_ascii=False, allow_nan=False))
