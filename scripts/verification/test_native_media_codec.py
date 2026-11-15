#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Exercise the production JPEG C FFI using host-only V4L2 and memory mocks.

Run only on the designated Linux worker through ssh test-env. The C executable
includes the current production source with all device operations replaced by
in-process mocks. It cannot forward an open/ioctl to /dev/video or /dev/kmsg.
These tests establish host transaction contracts, not hardware qualification.

The lifecycle cases extend AidokuForKindle's 2026-11-15 shutdown-contract-mock.c
and guarded-codec-contracts.json. See vendor/aidoku-native-media/README.md.
"""

import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[2]


@unittest.skipUnless(sys.platform == "linux", "Run verification through ssh test-env")
class NativeMediaCodecTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.temporary = tempfile.TemporaryDirectory(prefix="bilicomics-codec-mock-")
        cls.addClassCleanup(cls.temporary.cleanup)
        cls.binary = Path(cls.temporary.name) / "codec-mock"
        build = subprocess.run([
            "gcc", "-std=c11", "-O2", "-Wall", "-Wextra", "-Werror",
            str(ROOT / "scripts/verification/native_media_codec_mock.c"),
            "-o", str(cls.binary),
        ], capture_output=True, text=True, timeout=60)
        if build.returncode:
            raise RuntimeError(f"Host codec mock compilation failed:\n{build.stdout}\n{build.stderr}")

    def run_case(self, name):
        result = subprocess.run([str(self.binary), name], capture_output=True,
                                text=True, timeout=5)
        self.assertEqual(result.returncode, 0,
                         f"{name}:\nstdout={result.stdout}\nstderr={result.stderr}")
        report = json.loads(result.stdout)
        self.assertEqual(report["case"], name)
        self.assertIs(report["physical_device_opened"], False)
        self.assertIs(report["native_ioctl_executed"], False)
        self.assertEqual(report["allocations"], report["frees"])
        return report

    def test_query_checks_the_mocked_driver_identity(self):
        for name in ("query", "query_bad_card"):
            with self.subTest(case=name):
                report = self.run_case(name)
                self.assertEqual(report["video_open_attempts"], 1)
                self.assertEqual(report["allocations"], 0)

    def test_qualified_grey_transactions_keep_the_memory_contract(self):
        for name, shape in (("gray", (1920, 2560, 1860, 2480)),
                            ("gray_wide", (1920, 1280, 1860, 1240))):
            with self.subTest(case=name):
                report = self.run_case(name)
                self.assertEqual(report["status"], 0)
                self.assertEqual(report["video_open_attempts"], 1)
                self.assertEqual(report["stop_calls"], 2)
                self.assertEqual(report["capture_planes"], 1)
                self.assertEqual(tuple(report[key] for key in (
                    "source_width", "source_height", "target_width", "target_height")), shape)

    def test_unsupported_input_and_geometry_stop_before_video_open(self):
        for name in (
            "reject_unchanged", "reject_upscale_width", "reject_upscale_height",
            "reject_source_pixels", "reject_png", "reject_422", "reject_444", "reject_420", "reject_odd_color",
            "reject_odd_color_target", "reject_progressive", "fail_allocate_source",
            "reject_faulting_tall", "reject_unqualified_source", "reject_unqualified_target",
            "reject_reference_target", "reject_gray_odd_target",
        ):
            with self.subTest(case=name):
                report = self.run_case(name)
                self.assertEqual(report["video_open_attempts"], 0)
                self.assertEqual(report["mock_ioctls"], 0)

    def test_errors_at_each_transaction_stage_clean_up_after_the_required_fences(self):
        for name in (
            "fail_open", "fail_querycap", "fail_subscribe", "fail_output_format",
            "fail_output_readback", "fail_output_request", "fail_output_query",
            "fail_output_qbuf", "fail_output_streamon", "fail_source_event",
            "fail_source_timeout", "fail_header_readback", "fail_capture_format",
            "fail_capture_readback", "invalid_capture_planes", "fail_allocate_capture_first",
            "fail_capture_request", "fail_capture_query",
            "changed_planes", "fail_capture_qbuf", "fail_capture_queued_readback",
            "fail_capture_streamon", "fail_capture_dequeue", "fail_capture_timeout",
            "fail_output_dequeue", "fail_output_timeout", "capture_error_flag",
            "oversized_payload", "short_payload", "fail_close",
        ):
            with self.subTest(case=name):
                report = self.run_case(name)
                self.assertNotEqual(report["status"], 0)
                self.assertEqual(report["video_open_attempts"], 1,
                                 "The error injection must reach a qualified mock transaction")
                self.assertEqual(tuple(report[key] for key in (
                    "source_width", "source_height", "target_width", "target_height")),
                    (1920, 2560, 1860, 2480))

    def test_streamoff_retries_keep_context_and_userptr_storage_alive(self):
        for name, calls in (("stop_eintr", 3), ("stop_io_once", 3), ("stop_io_repeated", 5)):
            with self.subTest(case=name):
                report = self.run_case(name)
                self.assertEqual(report["stop_calls"], calls)
                self.assertEqual(report["video_open_attempts"], 1)
                self.assertEqual(tuple(report[key] for key in (
                    "source_width", "source_height", "target_width", "target_height")),
                    (1920, 2560, 1860, 2480))


if __name__ == "__main__":
    unittest.main()
