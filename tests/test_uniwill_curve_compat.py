"""Protect legacy source extraction from silently discarding driver code."""
import unittest

from uniwill_curve_compat import curve_ioctl_compat, no_curve_owner_hooks, without_curve_provider


class UniwillCurveCompatibility(unittest.TestCase):
    def test_unmodified_baseline_is_returned_exactly(self):
        source = "static int legacy(void) { return 42; }\n"
        self.assertEqual(without_curve_provider(source), source)
        self.assertEqual(no_curve_owner_hooks(source), "")

    def test_only_the_marked_provider_is_removed(self):
        before = "static int legacy_before(void) { return 42; }\n"
        after = "\nstatic int legacy_after(void) { return 43; }\n"
        for opener in ("/* UW_CURVE_PROVIDER_BEGIN */", "/* UW_CURVE_PROVIDER_BEGIN\n * Description. */"):
            with self.subTest(opener=opener):
                source = before + opener + "\nprovider\n/* UW_CURVE_PROVIDER_END */" + after
                result = without_curve_provider(source)
                self.assertTrue(result.startswith(before))
                self.assertTrue(result.endswith(after))
                self.assertNotIn("\nprovider\n", result)

    def test_missing_or_repeated_markers_fail_closed(self):
        begin, end = "/* UW_CURVE_PROVIDER_BEGIN */", "/* UW_CURVE_PROVIDER_END */"
        for source in (begin, end, begin + begin + end, begin + end + end):
            with self.subTest(source=source), self.assertRaises(AssertionError):
                without_curve_provider(source)

    def test_reversed_markers_fail_closed(self):
        with self.assertRaises(AssertionError):
            without_curve_provider("/* UW_CURVE_PROVIDER_END */\n/* UW_CURVE_PROVIDER_BEGIN */")

    def test_compatibility_defines_only_hooks_needed_by_the_selected_source(self):
        source = "uw_curve_blocks_legacy_locked(); uw_curve_revoke_locked();"
        hooks = no_curve_owner_hooks(source)
        self.assertIn("bool uw_curve_blocks_legacy_locked(void) { return false; }", hooks)
        self.assertIn("int uw_curve_revoke_locked(void) { return 0; }", hooks)
        self.assertNotIn("uw_curve_new_lifecycle_locked", hooks)

    def test_lifecycle_compatibility_is_an_explicit_no_owner_noop(self):
        hooks = no_curve_owner_hooks("uw_curve_new_lifecycle_locked(); uw_curve_note_legacy_write_locked();")
        self.assertIn("void uw_curve_new_lifecycle_locked(void)", hooks)
        self.assertIn("void uw_curve_note_legacy_write_locked(void)", hooks)
        self.assertNotIn("return", hooks)

    def test_old_ioctl_sources_need_no_new_abi_adapter(self):
        self.assertEqual(curve_ioctl_compat("static long fop_ioctl(void) { return 0; }"), "")


if __name__ == "__main__":
    unittest.main()
