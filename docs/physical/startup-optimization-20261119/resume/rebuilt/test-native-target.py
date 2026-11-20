"""Exercise the actual ARM recognizer on a synthetic, isolated Xvfb display on test-env."""

import ctypes
import ctypes.util
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

from PIL import Image, ImageDraw, ImageFont


ROOT = Path("/work")
LIBS = ROOT / "runtime/usr/lib/arm-linux-gnueabihf"
MODELS = ROOT / "runtime/usr/share/tesseract-ocr/5/tessdata"


class FixtureDisplay:
    def __init__(self):
        xlib = ctypes.CDLL(ctypes.util.find_library("X11"))
        pointer, integer, unsigned, resource = ctypes.c_void_p, ctypes.c_int, ctypes.c_uint, ctypes.c_ulong
        definitions = {
            "XOpenDisplay": (pointer, [ctypes.c_char_p]), "XDefaultRootWindow": (resource, [pointer]),
            "XDefaultDepth": (integer, [pointer, integer]), "XWhitePixel": (resource, [pointer, integer]),
            "XBlackPixel": (resource, [pointer, integer]), "XCreateGC": (pointer, [pointer, resource, resource, pointer]),
            "XCreatePixmap": (resource, [pointer, resource, unsigned, unsigned, unsigned]),
            "XCreateBitmapFromData": (resource, [pointer, resource, ctypes.c_char_p, unsigned, unsigned]),
            "XSetForeground": (integer, [pointer, pointer, resource]), "XSetBackground": (integer, [pointer, pointer, resource]),
            "XCopyPlane": (integer, [pointer, resource, resource, pointer, integer, integer, unsigned, unsigned,
                                     integer, integer, resource]),
            "XSetWindowBackgroundPixmap": (integer, [pointer, resource, resource]),
            "XClearWindow": (integer, [pointer, resource]), "XSync": (integer, [pointer, integer]),
            "XFreeGC": (integer, [pointer, pointer]), "XFreePixmap": (integer, [pointer, resource]),
            "XCloseDisplay": (integer, [pointer]),
        }
        for name, (returns, arguments) in definitions.items():
            function = getattr(xlib, name)
            function.restype, function.argtypes = returns, arguments
        self.xlib = xlib
        self.display = xlib.XOpenDisplay(None)
        if not self.display:
            raise RuntimeError("Use a fresh 900x1200 Xvfb screen")
        self.root = xlib.XDefaultRootWindow(self.display)

    def paint(self, image):
        width, height = image.size
        xlib, display = self.xlib, self.display
        bitmap = xlib.XCreateBitmapFromData(display, self.root, image.convert("1").tobytes("raw", "1;R"), width, height)
        colored = xlib.XCreatePixmap(display, self.root, width, height, xlib.XDefaultDepth(display, 0))
        gc = xlib.XCreateGC(display, colored, 0, None)
        xlib.XSetForeground(display, gc, xlib.XWhitePixel(display, 0))
        xlib.XSetBackground(display, gc, xlib.XBlackPixel(display, 0))
        xlib.XCopyPlane(display, bitmap, colored, gc, 0, 0, width, height, 0, 0, 1)
        xlib.XSetWindowBackgroundPixmap(display, self.root, colored)
        xlib.XClearWindow(display, self.root)
        xlib.XSync(display, 0)
        xlib.XFreeGC(display, gc)
        xlib.XFreePixmap(display, bitmap)
        xlib.XFreePixmap(display, colored)

    def close(self):
        self.xlib.XCloseDisplay(self.display)


class NativeTargetTests(unittest.TestCase):
    def setUp(self):
        if not os.environ.get("DISPLAY") or Path.cwd() != ROOT or os.environ.get("BILI_CLICK_FIXTURE") != "1":
            self.fail("Use only the explicitly selected host fixture")
        self.display = FixtureDisplay()
        self.addCleanup(self.display.close)
        self.font = ImageFont.truetype("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf", 33)

    def capture(self, labels, mode="card", program="native-target", allowed=True):
        image = Image.new("L", (900, 1200), 255)
        drawing = ImageDraw.Draw(image)
        drawing.text((68, 1095), "Home", 0, font=self.font)
        drawing.text((560, 1095), "Library", 0, font=self.font)
        drawing.rectangle((558, 1137, 684, 1140), fill=0)
        for offset, label in enumerate(labels):
            drawing.text((100, 160 + offset * 140), label, 0, font=self.font)
        self.display.paint(image)
        with tempfile.TemporaryDirectory() as temporary:
            process = subprocess.run(
                ["qemu-arm", "-cpu", "cortex-a9", LIBS / "ld-linux-armhf.so.3", "--library-path", LIBS,
                 ROOT / program, mode, MODELS], cwd=temporary,
                env=dict(os.environ, OMP_THREAD_LIMIT="1"), capture_output=True, timeout=100,
            )
        self.assertNotIn(b"Do not export", process.stdout + process.stderr)
        if not allowed:
            self.assertNotEqual(process.returncode, 0)
            self.assertEqual(process.stdout, b"")
            return
        self.assertEqual(process.returncode, 0, process.stderr)
        record = json.loads(process.stdout)
        if program == "card-input":
            return record
        self.assertEqual(record["screen"], [900, 1200])
        self.assertEqual(record["home"]["count"], 1)
        self.assertEqual(record["library"]["count"], 1)
        self.assertTrue(record["buttons_clear"])
        return record

    def test_exact_card(self):
        record = self.capture(["Do not export", "BiliComics for Kindle"])
        self.assertEqual(record["title"]["count"], 1)
        self.assertLess(record["home"]["underline_pct"], 15)
        self.assertGreater(record["library"]["underline_pct"], 75)

    def test_ambiguous_and_longer_titles(self):
        record = self.capture(["BiliComics for Kindle", "BiliComics for Kindle", "BiliComics for Kindle Plus"])
        self.assertEqual(record["title"], {"count": 2})

    def test_navigation_only_does_not_read_card_labels(self):
        record = self.capture(["BiliComics for Kindle"], "navigation")
        self.assertEqual(record["title"], {"count": 0})

    def test_timed_release_for_exact_card_only(self):
        record = self.capture(["BiliComics for Kindle"], "open-card", "card-input")
        self.assertTrue(record["buttons_clear_after"] and record["down_confirmed"])
        self.assertGreaterEqual(record["up_before_ns"] - record["down_after_ns"], 50_000_000)
        self.assertLessEqual(record["up_before_ns"], record["up_after_ns"])
        self.capture(["BiliComics for Kindle", "BiliComics for Kindle"], "open-card", "card-input", False)
        self.capture(["BiliComics for Kindle Plus"], "open-card", "card-input", False)

    def test_public_tab_and_pointer_recovery(self):
        record = self.capture([], "nav-home", "card-input")
        self.assertEqual(record["target"], "nav-home")
        self.capture([], "nav-library", "card-input", False)
        self.capture([], "nav-home", "native-target", False)
        command = ["qemu-arm", "-cpu", "cortex-a9", LIBS / "ld-linux-armhf.so.3", "--library-path", LIBS,
                   ROOT / "card-input", "restore-pointer", "17", "23"]
        self.assertEqual(json.loads(subprocess.check_output(command)), {"restored": True})


if __name__ == "__main__":
    unittest.main(verbosity=2)
