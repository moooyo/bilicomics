"""Host-only checks for the app-only comparison and its timing boundaries."""

import importlib.util
from pathlib import Path
import tempfile
import unittest
import zipfile

from project_archived import FRAME_STAGES, observed_times


def module(filename):
    spec = importlib.util.spec_from_file_location(filename, Path(__file__).with_name(filename + ".py"))
    result = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(result)
    return result


prepare, runner = module("prepare-pair"), module("run-pair")


class Archive(unittest.TestCase):
    def setUp(self):
        area = tempfile.TemporaryDirectory()
        self.addCleanup(area.cleanup)
        self.root = Path(area.name)
        self.elf = b"\x7fELF\x01\x01\x01" + bytes(11) + b"(\0" + bytes(36)
        self.new = self.elf + b"new"
        self.members = dict.fromkeys(prepare.SELECTED, b"control")
        self.members.update({f"notices/test-{index}": b"notice" for index in range(49 - len(self.members))})
        self.members[prepare.MAIN_BINARY] = self.elf
        self.original = self.archive("control", self.members)
        self.built = self.root / "built"
        self.built.write_bytes(self.new)

    def archive(self, name, members):
        target = self.root / name
        with zipfile.ZipFile(target, "w") as archive:
            for path, data in members.items():
                archive.writestr(path, data)
        return target

    def compare(self, edits):
        changed = self.archive("new.zip", self.members | edits)
        return prepare.compare_archives(self.original, changed, self.built)

    def test_exact_binary_only(self):
        output = self.compare({prepare.MAIN_BINARY: self.new})
        self.assertEqual(output[prepare.MAIN_BINARY], self.new)

    def test_resource_change_rejected(self):
        with self.assertRaisesRegex(ValueError, "Non-application"):
            self.compare({prepare.MAIN_BINARY: self.new, "documents/bilicomics.sh": b"other"})

    def test_unchanged_or_unbound_binary_rejected(self):
        for data in (self.elf, self.new + b"other"):
            with self.subTest(data=data), self.assertRaisesRegex(ValueError, "unchanged or not the built"):
                self.compare({prepare.MAIN_BINARY: data})

    def test_wrong_roster_rejected(self):
        with self.assertRaisesRegex(ValueError, "roster"):
            self.compare({prepare.MAIN_BINARY: self.new, "extra": b"x"})


class Protocol(unittest.TestCase):
    def test_only_real_matching_arms(self):
        old = {"protocol": "rebuilt-library-click-2", "source": prepare.COMMITS, "dry_only": False}
        new = {"protocol": "shelf-render-click-3", "source": {"A": prepare.COMMITS["B"],
               "B": {"source_base_commit": prepare.SOURCE_BASE, "source_variant": prepare.SHELF_VARIANT}}, "dry_only": False}
        for admitted, other in ((old, new), (new, old)):
            self.assertEqual(runner.protocol(admitted), admitted["protocol"])
            for edits in ({"dry_only": True}, {"source": other["source"]}, {"protocol": "unknown"}):
                with self.subTest(edits=edits), self.assertRaises(ValueError):
                    runner.protocol(admitted | edits)


def clocks(start):
    return {"started_monotonic_ns": start, "finished_monotonic_ns": start + 1, "wall_ns": 1}


def frame(sequence, marker, stage_names, start):
    stages = {name: clocks(start + 2 * index) for index, name in enumerate(stage_names)}
    data = {"rendered": True, "refresh_accepted": True, "stages": stages, "refresh_ioctl_returned": True,
            "refresh_ioctl_attempts": 1, "refresh_errno": None, "initial_frame": sequence == 1, "render_sequence": sequence,
            "complete_frame_replacement": True, "framebuffer_bits_per_pixel": 8,
            "refresh_region": {"x": 0, "y": 0, "width": 1860, "height": 2480}, "accepted_update_marker": marker,
            "completed_update_marker": marker, "preceding_update_marker": marker - 1, "driver_completion_observed": True,
            "completion_errno": None, "panel_completion_observed": False}
    return {"source_line_1based": sequence + 1, "fields": {
        "kind": "native", "stage": "frame", "monotonic_ns": start + 2 * len(stage_names), "data": data}}


class StageOrder(unittest.TestCase):
    def setUp(self):
        self.final = {"backend": "kindle", "profile": "scribe", "demo": True, "offline": True, "route": "shelf",
                      "startup": {"error": False, "booting": False}, "suspended": False}
        self.life = [{"event": "startup-stage", "stage": "process_enter", "monotonic_ns": 5}, {"event": "ready", "route": "shelf"}]
        self.perf = [frame(1, 1, FRAME_STAGES, 10), {"source_line_1based": 5, "fields": {
            "kind": "startup", "stage": "ui_ready", "monotonic_ns": 30}}]
        self.private = ("slint_render", "rgb_fingerprint", "preceding_update_wait", "framebuffer_write",
                        "refresh_submit", "refresh_complete_wait")

    def measure(self, shelf, private):
        return observed_times(self.final, self.life, self.perf + [shelf], 1, private_shelf_render=private)

    def test_transaction_order_must_match_arm(self):
        for private, sequence in ((False, FRAME_STAGES), (True, self.private)):
            with self.subTest(private=private):
                shelf = frame(2, 2, sequence, 40)
                self.assertEqual(self.measure(shelf, private)[0], 2)
                with self.assertRaisesRegex(ValueError, "stage order"):
                    self.measure(shelf, not private)

    def test_wrong_superseded_marker_rejected(self):
        shelf = frame(2, 2, self.private, 40)
        shelf["fields"]["data"]["preceding_update_marker"] = 9
        with self.assertRaisesRegex(ValueError, "preceding update"):
            self.measure(shelf, True)

    def test_write_before_wait_or_wrong_completion_rejected(self):
        shelf = frame(2, 2, self.private, 40)
        shelf["fields"]["data"]["stages"]["framebuffer_write"] = clocks(43)
        with self.assertRaisesRegex(ValueError, "stage order"):
            self.measure(shelf, True)
        shelf = frame(2, 2, self.private, 40)
        shelf["fields"]["data"]["completed_update_marker"] = 1
        with self.assertRaisesRegex(ValueError, "completion"):
            self.measure(shelf, True)


if __name__ == "__main__":
    unittest.main()
