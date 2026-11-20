"""Check transaction orders and reject corrupted copies of one qualified case."""

import argparse
import copy
import json
from pathlib import Path
import sys
import tempfile
import unittest

from project_frames import load, project_case, records, shelf_order


def clocks(start, end):
    return {"started_monotonic_ns": start, "finished_monotonic_ns": end, "wall_ns": end - start}


class StageOrder(unittest.TestCase):
    def setUp(self):
        names = ("preceding_update_wait", "slint_render", "rgb_fingerprint", "framebuffer_write",
                 "refresh_submit", "refresh_complete_wait")
        self.stages = {name: clocks(start, start + 5) for name, start in zip(names, range(10, 70, 10))}

    def test_wait_before_private_render(self):
        self.assertEqual(shelf_order(self.stages), "wait_then_render")

    def test_private_render_before_wait(self):
        self.stages["slint_render"] = clocks(1, 5)
        self.assertEqual(shelf_order(self.stages), "render_then_wait")
        self.stages["rgb_fingerprint"] = clocks(5, 9)
        self.assertEqual(shelf_order(self.stages), "render_then_wait")

    def test_mapped_write_cannot_begin_during_wait(self):
        self.stages["preceding_update_wait"] = clocks(35, 44)
        with self.assertRaises(ValueError):
            shelf_order(self.stages)

    def test_overlapping_private_stages_on_ui_thread_are_not_allowed(self):
        self.stages["preceding_update_wait"] = clocks(15, 23)
        with self.assertRaises(ValueError):
            shelf_order(self.stages)

    def test_submit_cannot_precede_end_of_mapped_write(self):
        self.stages["refresh_submit"] = clocks(44, 49)
        with self.assertRaises(ValueError):
            shelf_order(self.stages)


class CaseReplay(unittest.TestCase):
    batch = None

    @classmethod
    def setUpClass(cls):
        if cls.batch is None:
            raise unittest.SkipTest("Pass the closed batch and paired result to replay evidence mutations")
        report = load(cls.paired_result)
        cls.name = report["schedule"][2]
        cls.published = report["cases"][cls.name]
        cls.rgb = report["common_shelf_rgb_fingerprint"]
        cls.original = project_case(cls.batch / cls.name, cls.published, cls.rgb)

    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.case = Path(temporary.name) / self.name
        self.case.mkdir()
        for leaf in ("final.json", "lifecycle.jsonl", "click.json"):
            (self.case / leaf).write_bytes((self.batch / self.name / leaf).read_bytes())
        self.perf = records(self.batch / self.name / "performance.jsonl")
        self.shelf = self.perf[self.original["shelf_performance_line"] - 1]

    def replay(self):
        (self.case / "performance.jsonl").write_text("".join(json.dumps(row) + "\n" for row in self.perf))
        return project_case(self.case, self.published, self.rgb)

    def test_unchanged_copy(self):
        self.assertEqual(self.replay(), self.original)

    def test_wrong_pid(self):
        self.perf[0]["process_id"] += 1
        with self.assertRaisesRegex(ValueError, "different process"):
            self.replay()

    def test_wrong_previous_marker(self):
        self.shelf["data"]["preceding_update_marker"] = self.original["shelf_marker"]
        with self.assertRaisesRegex(ValueError, "receipt"):
            self.replay()

    def test_wrong_driver_marker(self):
        self.shelf["data"]["completed_update_marker"] = self.original["previous_marker"]
        with self.assertRaisesRegex(ValueError, "receipt"):
            self.replay()

    def test_wrong_shelf_pixels(self):
        self.rgb = "different"
        with self.assertRaisesRegex(ValueError, "Shelf pixels"):
            self.replay()

    def test_safe_alternate_transaction_from_isolated_copy(self):
        stages = self.shelf["data"]["stages"]
        names = ("slint_render", "rgb_fingerprint", "preceding_update_wait")
        timeline = min(stages[key]["started_monotonic_ns"] for key in names)
        cpu = min(stages[key]["started_thread_cpu_ns"] for key in names)
        for key in names:
            stage = stages[key]
            stage["started_monotonic_ns"], stage["started_thread_cpu_ns"] = timeline, cpu
            timeline += stage["wall_ns"]
            cpu += stage["thread_cpu_ns"]
            stage["finished_monotonic_ns"], stage["finished_thread_cpu_ns"] = timeline, cpu
        projected = self.replay()
        self.assertEqual(projected["order"], "render_then_wait")
        self.assertEqual(projected["ns"]["release_to_driver"], self.original["ns"]["release_to_driver"])

    def test_missing_stage_clock(self):
        self.shelf["data"]["stages"]["framebuffer_write"]["wall_ns"] += 1
        with self.assertRaisesRegex(ValueError, "clocks"):
            self.replay()

    def test_invalid_thread_cpu_clock(self):
        self.shelf["data"]["stages"]["slint_render"]["thread_cpu_ns"] += 1
        with self.assertRaisesRegex(ValueError, "CPU clocks"):
            self.replay()

    def test_do_not_mix_state_producer_and_consumer_timestamps(self):
        prepared = next(row for row in self.perf if row["stage"] == "state_prepared")
        prepared["data"]["observed"]["monotonic_ns"] = prepared["monotonic_ns"] + 1
        with self.assertRaisesRegex(ValueError, "Producer clocks differ"):
            self.replay()

    def test_wrong_clock_origin(self):
        click = load(self.case / "click.json")
        click["clock"] = "CLOCK_BOOTTIME"
        (self.case / "click.json").write_text(json.dumps(click))
        with self.assertRaisesRegex(ValueError, "release bracket"):
            self.replay()

    def test_paired_endpoint_must_match_raw_case(self):
        self.published = copy.copy(self.published)
        self.published["driver_ns"] += 1
        with self.assertRaisesRegex(ValueError, "endpoints differ"):
            self.replay()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("batch", type=Path)
    parser.add_argument("paired_result", type=Path)
    args = parser.parse_args()
    CaseReplay.batch, CaseReplay.paired_result = args.batch, args.paired_result
    unittest.main(argv=[sys.argv[0]], verbosity=2)
