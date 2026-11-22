"""Test the clock boundary using the retained projections, without a raw archive or device."""

import json
import unittest

from analyze_pre_ready import DEFAULT_BATCHES, analyze, case_times, distribution


def load_batch(index):
    return tuple(json.loads(path.read_text(encoding="utf-8")) for path in DEFAULT_BATCHES[index])


def example():
    card = {"process_entry_ns": 100, "first_loading_send_ns": 200, "shelf_ready_ns": 750,
            "send_ns": 800, "driver_ns": 999, "marker": 3, "loading_frames": 3}
    frame = {"case": "b01", "shelf_marker": 3, "loading_frames": 3,
             "state_clocks_ns": {"producer_observation": 10250, "ui_record": 10500,
                                 "third_loading_wait_start": 10400},
             "ns": {"release_to_rust": 100, "release_to_send": 800, "release_to_driver": 999,
                    "loading_send_to_ready": 550, "ready_to_send": 50, "state_observed_to_ui_record": 250,
                    "state_observed_minus_third_wait_start": -150}}
    return frame, card


class PreReadyTests(unittest.TestCase):
    def test_published_batches_and_candidate_counterexamples(self):
        prior = analyze(*load_batch(0))
        current = analyze(*load_batch(1))
        self.assertEqual(len(current["candidate"]["cases"]), 6)
        self.assertEqual(len(current["cases"]), 12)
        self.assertEqual(prior["both_arms_by_loading_frames"][4]["ms"]["producer_to_ui_record"]["available"], 4)
        self.assertAlmostEqual(current["candidate"]["ms"]["producer_to_ui_record"]["median"], 68.586, places=3)
        self.assertEqual(current["candidate_examples"], {"shortest_handoff_among_three_slowest_to_ready": "b03",
                                                        "longest_handoff": "b04"})
        self.assertIsNone(current["candidate"]["ms"]["ui_record_to_ready"]["median"])

    def test_same_relative_evidence_can_put_producer_on_either_side_of_feedback(self):
        frame, card = example()
        unknown = case_times(frame, card)
        self.assertEqual(unknown["producer_vs_feedback"], "unresolved")
        self.assertIsNone(unknown["ns"]["feedback_to_producer_signed"])
        for origin, expected, tail in ((10000, 50, 250), (10100, -50, 350)):
            with self.subTest(origin=origin):
                frame["release_monotonic_ns"] = origin
                known = case_times(frame, card)
                self.assertEqual(known["ns"]["feedback_to_producer_signed"], expected)
                self.assertEqual(known["ns"]["ui_record_to_ready"], tail)
                self.assertEqual(known["ns"]["signed_outer_sum"], tail + expected)
                self.assertEqual(known["producer_vs_feedback"], "before_feedback" if expected < 0 else "at_or_after_feedback")
                incomplete = distribution([unknown, known])["ms"]["ui_record_to_ready"]
                self.assertEqual(incomplete, {"available": 1, "min": None, "median": None, "max": None})

    def test_invalid_anchor_or_mismatched_card_is_not_a_timing(self):
        frame, card = example()
        frame["release_monotonic_ns"] = 10500
        with self.assertRaisesRegex(ValueError, "Release anchor"):
            case_times(frame, card)
        del frame["release_monotonic_ns"]
        card["shelf_ready_ns"] += 1
        with self.assertRaisesRegex(ValueError, "intervals disagree"):
            case_times(frame, card)

    def test_loading_wait_after_ready_is_not_a_pre_ready_sample(self):
        frame, card = example()
        frame["release_monotonic_ns"] = 10000
        frame["state_clocks_ns"]["third_loading_wait_start"] = 10751
        frame["ns"]["state_observed_minus_third_wait_start"] = -501
        with self.assertRaisesRegex(ValueError, "Release anchor"):
            case_times(frame, card)

    def test_missing_case_or_different_batch_is_rejected(self):
        frames, cards = load_batch(0)
        frames["cases"].pop()
        with self.assertRaisesRegex(ValueError, "Missing or repeated"):
            analyze(frames, cards)
        with self.assertRaisesRegex(ValueError, "Different run"):
            analyze(frames, load_batch(1)[1])


if __name__ == "__main__":
    unittest.main()
