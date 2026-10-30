"""Hardware-free regression checks; all sensor values below are synthetic."""

import json
from pathlib import Path
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts" / "gm6px0x_fan2_ecrr"))
import collect as fan2


def attempt(index=1, hi0=0x13, b=0x4b, d=0x4b, hi1=None, order="BD"):
    hi1 = hi0 if hi1 is None else hi1
    return (f"attempt={index} order={order} hi0={hi0:02x} b={b:02x} "
            f"d={d:02x} hi1={hi1:02x} stable={int(hi0 == hi1)}\n")


class Fan2ComparisonTests(unittest.TestCase):
    def test_exact_dmi_identity_gate(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            for field, value in fan2.IDENTITY.items():
                (root / field).write_text(value)
            fan2.check_identity(root)
            (root / "board_name").write_text("other")
            with self.assertRaisesRegex(fan2.SampleError, "Unsupported board_name"):
                fan2.check_identity(root)

    def test_stable_pair_decodes_same_high_byte_for_both(self):
        row = fan2.sample_row(1, attempt(hi0=0x13, b=0x4b, d=0x4d))
        self.assertEqual(row["count_046b"], 0x134b)
        self.assertEqual(row["count_046d"], 0x134d)
        self.assertTrue(row["stable"])
        self.assertEqual(row["rollovers"], 0)

    def test_rollover_discarded_then_retried(self):
        trace = (attempt(hi0=0x12, hi1=0x13) +
                 attempt(index=2, hi0=0x13, b=0x4a, d=0x4b, order="DB"))
        row = fan2.sample_row(1, trace)
        self.assertEqual(len(row["attempts"]), 2)
        self.assertEqual(row["rollovers"], 1)
        self.assertEqual(row["count_046b"], 0x134a)
        self.assertEqual(row["count_046d"], 0x134b)

    def test_three_rollovers_yield_no_invented_rpm(self):
        trace = "".join(attempt(i, hi0=i, hi1=i+1) for i in range(1, 4))
        row = fan2.sample_row(1, trace)
        self.assertFalse(row["stable"])
        self.assertIsNone(row["count_046b"])
        self.assertIsNone(row["count_046d"])
        self.assertEqual(fan2.summarize([row])["stable_rows"], 0)

    def test_bad_firmware_or_transport_values_fail_closed(self):
        good = attempt()
        bad = [
            good.replace("b=4b", "b=100"), good.replace("b=4b", "b=gg"),
            good.replace("stable=1", "stable=0"),
            good + good, good.replace("attempt=1", "attempt=2"),
            attempt(hi0=0x12, hi1=0x13) + attempt(index=3),
            good + attempt(index=2), "", "anything\n",
        ]
        for raw in bad:
            with self.subTest(raw=raw), self.assertRaises(fan2.SampleError):
                fan2.parse_sample(raw)

    def test_ff_is_not_automatically_invalidated(self):
        row = fan2.sample_row(1, attempt(b=0x00, d=0xff))
        self.assertEqual(row["count_046d"], 0x13ff)
        self.assertEqual(row["count_046b"], 0x1300)
        self.assertFalse(fan2.summarize([row])["independent_physical_rpm_validation"])

    def test_tracking_needs_variation_not_only_equal_constant_values(self):
        flat = [fan2.sample_row(i, attempt()) for i in range(1, 31)]
        self.assertEqual(fan2.summarize(flat)["finding"],
                         "insufficient_observed_variation")
        changing = [fan2.sample_row(i, attempt(b=40 + 3*i, d=40 + 3*i))
                    for i in range(1, 31)]
        outcome = fan2.summarize(changing)
        self.assertEqual(outcome["finding"],
                         "closely_tracking_in_observed_range")
        self.assertEqual(outcome["exact_pair_matches"], 30)
        self.assertFalse(outcome["driver_change_justified"])

    def test_constant_d_with_changing_b_is_not_a_compatible_result(self):
        rows = [fan2.sample_row(i, attempt(b=40 + 3*i, d=0xff, order="DB"))
                for i in range(1, 31)]
        outcome = fan2.summarize(rows)
        self.assertEqual(outcome["finding"], "different_in_observed_range")
        self.assertEqual(outcome["low_d_distinct_values"], 1)
        self.assertIsNone(outcome["pearson_correlation"])

    def test_constant_low_d_is_detected_even_when_high_changes(self):
        rows = [fan2.sample_row(i, attempt(hi0=0x13 + i // 10,
                                           b=40 + 3*i, d=0xff))
                for i in range(1, 31)]
        self.assertEqual(fan2.summarize(rows)["low_d_distinct_values"], 1)
        self.assertEqual(fan2.summarize(rows)["finding"],
                         "different_in_observed_range")

    def test_collect_limits_and_checkpoint_on_read_error(self):
        for count, interval in ((0, 1), (61, 1), (True, 1), (1, 0),
                                (1, float("nan")), (1, float("inf")), (1, True)):
            with self.subTest(count=count, interval=interval):
                with self.assertRaises(fan2.SampleError):
                    fan2.collect(count, interval, lambda: self.fail("reader reached"),
                                 lambda row: self.fail("writer reached"))

        writes = []
        calls = iter([attempt(), OSError("ACPI read failed")])

        def read():
            item = next(calls)
            if isinstance(item, OSError):
                raise item
            return item

        summary = fan2.collect(3, 1, read, writes.append,
                               sleep=lambda delay: None)
        self.assertEqual([w["type"] for w in writes],
                         ["sample", "error", "summary"])
        self.assertEqual(summary["stable_rows"], 1)
        self.assertEqual(summary["read_errors"], 1)
        self.assertFalse(summary["complete"])

    def test_analyze_saved_jsonl_without_accessing_hardware(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "sample.jsonl"
            with path.open("x") as capture:
                capture.write(json.dumps({"type": "header", "schema": 1}) + "\n")
                capture.write(json.dumps(fan2.sample_row(1, attempt())) + "\n")
            self.assertEqual(fan2.analyze(path)["captured_rows"], 1)
            path.write_text('{"type":"header","schema":999}\n')
            with self.assertRaises(fan2.SampleError):
                fan2.analyze(path)

    def test_kernel_module_has_fixed_ecrr_only_and_private_sysfs(self):
        code = (ROOT / "scripts/gm6px0x_fan2_ecrr/gm6px0x_fan2_ecrr.c").read_text()
        self.assertIn('acpi_evaluate_integer(inou, "ECRR"', code)
        self.assertNotIn('acpi_evaluate_object(', code)
        self.assertNotIn('regmap_write(', code)
        self.assertNotIn('"ECRW"', code)
        self.assertIn('__ATTR(sample, 0400, sample_show, NULL)', code)
        for fixed in ("0x046b", "0x046c", "0x046d", "0x0740", "0x078e"):
            self.assertIn(fixed, code)
        self.assertIn("DMI_BOARD_NAME", code)
        self.assertNotIn("module_param", code)


if __name__ == "__main__":
    unittest.main()
