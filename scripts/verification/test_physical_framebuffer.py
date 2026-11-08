"""Pure framebuffer evidence conversion tests; no host or device transport."""

import unittest

from physical_device import (
    framebuffer_memory_pixels,
    framebuffer_panel_projection,
    sha256,
    trace_epdc_mode,
)


def grayscale_geometry(width=6, visual=1, grayscale=1):
    return {"width": width, "height": 1, "bits_per_pixel": 8,
            "visual": visual, "grayscale": grayscale}


class FramebufferConversionTests(unittest.TestCase):
    def setUp(self):
        self.raw = bytes((0, 1, 127, 128, 254, 255))

    def test_linear_grayscale_keeps_endpoint_bytes(self):
        value = framebuffer_memory_pixels(grayscale_geometry(), self.raw)
        self.assertEqual(value["pixels"], self.raw)
        self.assertEqual(value["image_mode"], "L")
        self.assertFalse(value["framebuffer_visual_inversion_applied"])
        self.assertFalse(value["native_epdc_mode_applied"])

    def test_mono01_visual_inverts_exactly_once(self):
        value = framebuffer_memory_pixels(grayscale_geometry(visual=0), self.raw)
        self.assertEqual(value["pixels"], bytes((255, 254, 128, 127, 1, 0)))
        self.assertTrue(value["framebuffer_visual_inversion_applied"])
        self.assertFalse(value["native_epdc_mode_applied"])

    def test_declared_grayscale_retains_non_mono_visual_semantics(self):
        value = framebuffer_memory_pixels(grayscale_geometry(visual=2), self.raw)
        self.assertEqual(value["pixels"], self.raw)
        self.assertFalse(value["framebuffer_visual_inversion_applied"])

    def test_unconfirmed_palette_is_rejected(self):
        with self.assertRaisesRegex(RuntimeError, "unconfirmed 8-bit palette"):
            framebuffer_memory_pixels(grayscale_geometry(visual=3, grayscale=0), self.raw)

    def test_y8_projection_keeps_visual_decoded_pixels(self):
        geometry = grayscale_geometry()
        memory = framebuffer_memory_pixels(geometry, self.raw)
        value = framebuffer_panel_projection(geometry, memory, "Y8")
        self.assertEqual(value["status"], "generated")
        self.assertEqual(value["pixels"], self.raw)
        self.assertFalse(value["epdc_inversion_applied"])
        self.assertFalse(value["physical_panel_update_confirmed"])
        self.assertFalse(value["panel_photograph"])

    def test_y8inv_projection_is_a_separate_inversion_layer(self):
        geometry = grayscale_geometry()
        memory = framebuffer_memory_pixels(geometry, self.raw)
        value = framebuffer_panel_projection(geometry, memory, "Y8INV")
        self.assertEqual(value["status"], "generated")
        self.assertEqual(value["pixels"], bytes((255, 254, 128, 127, 1, 0)))
        self.assertTrue(value["epdc_inversion_applied"])
        self.assertEqual(memory["pixels"], self.raw)

    def test_visual_and_epdc_layers_are_each_applied_once(self):
        geometry = grayscale_geometry(visual=0)
        memory = framebuffer_memory_pixels(geometry, self.raw)
        y8 = framebuffer_panel_projection(geometry, memory, "Y8")
        inverted = framebuffer_panel_projection(geometry, memory, "Y8INV")
        self.assertEqual(memory["pixels"], bytes((255, 254, 128, 127, 1, 0)))
        self.assertEqual(y8["pixels"], memory["pixels"])
        self.assertEqual(inverted["pixels"], self.raw)
        self.assertTrue(memory["framebuffer_visual_inversion_applied"])
        self.assertTrue(inverted["epdc_inversion_applied"])

    def test_unknown_modes_never_fabricate_a_projection(self):
        geometry = grayscale_geometry()
        memory = framebuffer_memory_pixels(geometry, self.raw)
        for mode in (None, "unknown", "Y8INV ", "y8", 0, {}, []):
            with self.subTest(mode=mode):
                value = framebuffer_panel_projection(geometry, memory, mode)
                self.assertEqual(value["status"], "unavailable")
                self.assertIsNone(value["pixels"])
                self.assertIsNone(value["native_epdc_mode"])
                self.assertFalse(value["epdc_inversion_applied"])
                self.assertIsNotNone(value["gap"])
        self.assertEqual(memory["pixels"], self.raw)

    def test_raw_input_and_hash_are_unchanged_by_every_view(self):
        raw = bytearray(self.raw)
        original_hash = sha256(raw)
        for visual in (0, 1, 2):
            geometry = grayscale_geometry(visual=visual)
            memory = framebuffer_memory_pixels(geometry, raw)
            for mode in ("Y8", "Y8INV", None):
                framebuffer_panel_projection(geometry, memory, mode)
                self.assertEqual(bytes(raw), self.raw)
                self.assertEqual(sha256(raw), original_hash)

    def test_incomplete_raw_pixels_are_rejected(self):
        with self.assertRaisesRegex(RuntimeError, "incomplete"):
            framebuffer_memory_pixels(grayscale_geometry(), self.raw[:-1])

    def test_legacy_packed_color_decoding_does_not_expand_projection_scope(self):
        geometry = {"width": 2, "height": 1, "bits_per_pixel": 32, "byte_order": "little",
                    "red_offset": 16, "red_length": 8, "red_msb_right": 0,
                    "green_offset": 8, "green_length": 8, "green_msb_right": 0,
                    "blue_offset": 0, "blue_length": 8, "blue_msb_right": 0}
        raw = bytes((0, 0, 255, 255, 0, 255, 0, 255))
        memory = framebuffer_memory_pixels(geometry, raw)
        self.assertEqual(memory["image_mode"], "RGB")
        self.assertEqual(memory["pixels"], bytes((255, 0, 0, 0, 255, 0)))
        for mode in ("Y8", "Y8INV"):
            with self.subTest(mode=mode):
                value = framebuffer_panel_projection(geometry, memory, mode)
                self.assertEqual(value["status"], "unavailable")
                self.assertIsNone(value["pixels"])


class TraceEpdcModeTests(unittest.TestCase):
    def test_latest_explicit_snapshot_wins(self):
        value = trace_epdc_mode([
            {"event": "ready", "native_epdc_mode": "Y8"},
            {"event": "resume", "native_epdc_mode": "Y8INV"},
        ])
        self.assertEqual(value["mode"], "Y8INV")
        self.assertEqual(value["trace_event"], "resume")
        self.assertEqual(value["trace_record_index"], 1)

    def test_ui_state_without_snapshot_does_not_hide_verified_mode(self):
        value = trace_epdc_mode([
            {"event": "reader-ready", "native_epdc_mode": "Y8INV"},
            {"event": "ui-state"},
        ])
        self.assertEqual(value["mode"], "Y8INV")
        self.assertEqual(value["trace_record_index"], 0)

    def test_explicit_null_blocks_older_known_mode(self):
        value = trace_epdc_mode([
            {"event": "ready", "native_epdc_mode": "Y8"},
            {"event": "resume", "native_epdc_mode": None},
        ])
        self.assertIsNone(value["mode"])
        self.assertEqual(value["trace_event"], "resume")
        self.assertIsNotNone(value["gap"])

    def test_missing_snapshots_remain_unknown(self):
        value = trace_epdc_mode([{"event": "ready"}, {"event": "ui-state"}])
        self.assertIsNone(value["mode"])
        self.assertIsNone(value["source"])
        self.assertIsNotNone(value["gap"])

    def test_unrecognized_snapshot_does_not_fall_back_to_earlier_mode(self):
        for mode in ("unknown", "Y8INV ", {}, 1):
            with self.subTest(mode=mode):
                value = trace_epdc_mode([
                    {"event": "ready", "native_epdc_mode": "Y8INV"},
                    {"event": "resume", "native_epdc_mode": mode},
                ])
                self.assertIsNone(value["mode"])
                self.assertEqual(value["trace_record_index"], 1)


if __name__ == "__main__":
    unittest.main()
