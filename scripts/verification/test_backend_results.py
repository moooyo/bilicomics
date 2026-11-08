"""Check selected-test receipts when bounded diagnostics interrupt libtest output."""

import os
import sys
import unittest

from backend import completed_test_results


class BackendResultTests(unittest.TestCase):
    def test_diagnostics_do_not_hide_a_completed_touch_test(self):
        output = (
            "running 2 tests\n"
            "test touch::tests::diagnostic_case ... "
            "SLINT_KINDLE_INPUT_DIAGNOSTIC seq=1 event=press\n"
            "SLINT_KINDLE_INPUT_DIAGNOSTIC seq=2 event=release\n"
            "ok\n"
            "test touch::tests::ordinary_case ... ok\n"
            "test result: ok. 2 passed; 0 failed; 0 ignored;\n"
        )
        self.assertEqual(completed_test_results(output), {
            "touch::tests::diagnostic_case": "ok",
            "touch::tests::ordinary_case": "ok",
        })

    def test_diagnostic_ok_text_cannot_turn_a_failure_into_success(self):
        output = (
            "test touch::tests::failed_case ... diagnostic status=ok\n"
            "FAILED\n"
            "test touch::tests::incomplete_case ... diagnostic status=ok\n"
            "test result: FAILED. 0 passed; 1 failed; 0 ignored;\n"
        )
        self.assertEqual(completed_test_results(output), {
            "touch::tests::failed_case": "FAILED",
        })


if __name__ == "__main__":
    if sys.platform != "linux" or os.environ.get("BILICOMICS_REMOTE_VERIFICATION") != "1":
        raise SystemExit("Run only on test-env with BILICOMICS_REMOTE_VERIFICATION=1")
    unittest.main(verbosity=2)
