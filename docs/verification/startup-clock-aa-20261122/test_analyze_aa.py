import copy
import unittest

from analyze_aa import CONTRACT, SCHEDULE, analyze, unique


def sample(latencies=(100, 101, 100, 110, 200, 201, 300, 280, 400, 370)):
    rows = []
    for number, ((pair, arm), latency) in enumerate(zip(SCHEDULE, latencies)):
        origin = 1000 * number
        clock = {"release_before": 0, "release_after": 2, "launch_entry": 20, "pre_app_invocation": 40,
                 "rust_main_entry": 50, "trace_started": 55, "state_produced": 75, "state_consumed": None,
                 "shelf_rendered": latency, "shelf_submitted": latency + 7, "driver_completed": latency + 25}
        rows.append({"pair": pair, "arm": arm, "clock_offset_ns": {key: None if stamp is None else origin + stamp for key, stamp in clock.items()},
                     "evidence": {key: True for key in CONTRACT | {"accepted_send", "same_marker_driver"}}})
    return {"schema": "bili-shelf-clock-aa-projection-v1", "clock": "CLOCK_MONOTONIC", "cases": rows}


class ClosedSeriesTests(unittest.TestCase):
    def test_positional_direction_brackets_and_order_groups(self):
        result = analyze(sample())
        self.assertTrue(result["complete_series"])
        self.assertEqual([pair["second_minus_first_ns"]["card_to_renderer"] for pair in result["measured_pairs"]],
                         [[8, 12], [-1, 3], [-22, -18], [-32, -28]])
        primary = result["groups"]["all"]["card_to_renderer"]
        self.assertEqual(primary["median_second_minus_first_ns"], [-11.5, -7.5])
        self.assertEqual([primary[key] for key in ("pairs", "second_earlier", "second_later", "indeterminate")], [4, 2, 1, 1])
        self.assertEqual(result["groups"]["AB"]["card_to_renderer"]["median_second_minus_first_ns"], [-7, -3])
        self.assertEqual(result["groups"]["BA"]["card_to_renderer"]["median_second_minus_first_ns"], [-16.5, -12.5])

    def test_warmups_excluded_and_pair_median_not_marginal(self):
        source = sample((900, 890, 300, 101, 301, 201, 302, 401, 80, 300))
        first = analyze(source)
        primary = first["groups"]["all"]["card_to_renderer"]
        self.assertEqual(primary["median_second_minus_first_ns"], [-2.5, 1.5])
        self.assertNotEqual(primary["median_second_minus_first_ns"][1], primary["median_second_ns"][1] - primary["median_first_ns"][1])
        source["cases"][0]["clock_offset_ns"]["shelf_rendered"] -= 100
        self.assertEqual(first, analyze(source))

    def test_adjacent_cadence_keeps_cross_pair_pauses(self):
        source = sample()
        for row in source["cases"][4:]:
            for key, value in row["clock_offset_ns"].items():
                if value is not None:
                    row["clock_offset_ns"][key] += 5000
        result = analyze(source)
        within, between = (result["release_cadence"][key] for key in ("within_pair", "between_pairs"))
        self.assertEqual(within, {"count": 4, "median_ns": [998, 1002], "min_lower_ns": 998, "max_upper_ns": 1002})
        self.assertEqual(between["count"], 3)
        self.assertEqual(between["max_upper_ns"], 6002)
        self.assertEqual(result["adjacent_measured_releases"][1]["release_to_release_ns"], [5998, 6002])

    def test_secondary_gaps_preserve_primary_denominator(self):
        source = sample()
        row = source["cases"][2]
        row["clock_offset_ns"].update(shelf_submitted=None, driver_completed=None, state_produced=None)
        row["evidence"].update(accepted_send=False, same_marker_driver=False)
        groups = analyze(source)["groups"]["all"]
        self.assertEqual(groups["card_to_renderer"]["pairs"], 4)
        self.assertEqual(groups["card_to_driver"]["pairs"], 3)
        self.assertEqual(groups["header_to_state_producer"]["pairs"], 3)
        self.assertIsNone(groups["state_producer_to_ui_observation"]["median_second_minus_first_ns"])

    def test_partial_prefix_retains_unpaired_launch_without_imputation(self):
        for count in range(11):
            source = sample()
            source["cases"] = source["cases"][:count]
            result = analyze(source)
            measured = max(count - 2, 0)
            self.assertEqual(result["complete_measured_pairs"], measured // 2)
            self.assertEqual(result["unpaired_measured_visits"], measured % 2)
            self.assertEqual(len(result["measured_visits"]), measured)
            self.assertEqual(result["warmups_excluded"], min(count, 2))

    def test_invalid_receipts_or_overlapping_marks_are_rejected(self):
        source = sample()
        for container, key, replacement in (("clock_offset_ns", "pre_app_invocation", 0), ("clock_offset_ns", "release_before", 0),
                ("clock_offset_ns", "rust_main_entry", True), ("clock_offset_ns", "state_consumed", 1500),
                ("evidence", "lease_released", False), ("evidence", "same_run", False), ("evidence", "accepted_send", False),
                ("evidence", "same_marker_driver", False), ("evidence", "same_launcher_and_elf", False)):
            with self.subTest(key=key):
                changed = copy.deepcopy(source)
                changed["cases"][2][container][key] = replacement
                with self.assertRaises(ValueError):
                    analyze(changed)

    def test_substituted_visit_and_unknown_private_fields_are_rejected(self):
        source = sample()
        source["cases"].pop(3)
        with self.assertRaises(ValueError):
            analyze(source)
        source = sample()
        source["cases"][0]["unexpected"] = "private"
        with self.assertRaises(ValueError):
            analyze(source)
        with self.assertRaises(ValueError):
            unique([("pair", 1), ("pair", 1)])


if __name__ == "__main__":
    unittest.main()
