"""Verify BiliComics icon recognition and guarded XTest under ARM QEMU/Xvfb."""

import json
import os
from pathlib import Path
import runpy
import subprocess
import tempfile
import unittest

from PIL import Image, ImageDraw, ImageFont


ROOT = Path('/work')
LIBS = ROOT / 'runtime/usr/lib/arm-linux-gnueabihf'
MODELS = ROOT / 'runtime/usr/share/tesseract-ocr/5/tessdata'


class IconFixture(unittest.TestCase):
    def setUp(self):
        self.assertEqual(os.environ.get('BILI_ICON_XVFB'), '1')
        self.assertEqual(Path.cwd(), ROOT)
        self.assertTrue(os.environ.get('DISPLAY'))
        self.display = runpy.run_path(str(ROOT / 'test-native-target.py'))['FixtureDisplay']()
        self.addCleanup(self.display.close)
        with Image.open(ROOT / 'cover.png') as cover:
            self.cover = cover.convert('L')
        self.font = ImageFont.truetype('/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf', 56)

    def paint(self, covers, library=True, wrong_artwork=False):
        image = Image.new('L', (1860, 2480), 255)
        draw = ImageDraw.Draw(image)
        for x, word in ((330, 'Home'), (1270, 'Library')):
            draw.text((x, 2350), word, 0, font=self.font)
            if (word == 'Library') == library:
                left, _, right, bottom = draw.textbbox((x, 2350), word, font=self.font)
                draw.rectangle((left, bottom + 8, right, bottom + 12), fill=0)
        draw.text((112, 220), 'Private placeholder', 0, font=self.font)
        for col, row, width in covers:
            tile = self.cover.copy()
            if wrong_artwork:
                ImageDraw.Draw(tile).rectangle((310, 245, 595, 650), fill=255)
            image.paste(tile.resize((width, width * 4 // 3), Image.Resampling.LANCZOS), (col, row))
        self.display.paint(image)

    def invoke(self, mode, accepted=True):
        name = 'native-target' if mode in ('card', 'navigation') else 'card-input'
        with tempfile.TemporaryDirectory() as work:
            result = subprocess.run(['qemu-arm', '-cpu', 'cortex-a9', LIBS / 'ld-linux-armhf.so.3', '--inhibit-cache',
                                     '--library-path', LIBS, ROOT / name, mode, MODELS], cwd=work,
                                    env={**os.environ, 'OMP_THREAD_LIMIT': '1'}, capture_output=True, timeout=65)
            self.assertEqual(list(Path(work).iterdir()), [])
        self.assertNotIn(b'Private placeholder', result.stdout + result.stderr)
        self.assertEqual(result.returncode, 0, result.stderr)
        record = json.loads(result.stdout)
        if not accepted:
            self.assertEqual(record, {'schema': 'bili-card-input-v1', 'dispatched': False})
        return record

    def test_scaled_cover_uses_native_release_clock(self):
        for tile in ((779, 443, 316), (150, 610, 278), (1075, 720, 440)):
            self.paint([tile])
            target = self.invoke('card')['title']
            self.assertEqual(target['count'], 1, (tile, target))
            left, top, right, bottom = target['box']
            self.assertLessEqual(abs(left - tile[0]), 14)
            self.assertLessEqual(abs(top - tile[1]), 14)
            self.assertLessEqual(abs(right - tile[0] - tile[2]), 14)
            self.assertLessEqual(abs(bottom - tile[1] - tile[2] * 4 // 3), 18)
        receipt = self.invoke('open-card')
        self.assertEqual((receipt['target'], receipt['clock']), ('open-card', 'CLOCK_MONOTONIC'))
        self.assertTrue(receipt['ok'] and receipt['buttons_clear_after'])
        self.assertLessEqual(receipt['down_after_ns'] + 50_000_000, receipt['up_before_ns'])
        self.assertLessEqual(receipt['up_before_ns'], receipt['up_after_ns'])

    def test_duplicate_and_partial_artwork_do_not_click(self):
        self.paint([(330, 300, 290), (1050, 954, 360)])
        self.assertEqual(self.invoke('card')['title']['count'], 2)
        self.invoke('open-card', accepted=False)
        self.paint([(330, 300, 290)], wrong_artwork=True)
        self.assertEqual(self.invoke('card')['title']['count'], 0)
        self.invoke('open-card', accepted=False)

    def test_home_recent_and_empty_view_do_not_click(self):
        self.paint([(650, 400, 363)], library=False)
        self.assertEqual(self.invoke('card')['title']['count'], 1)
        self.invoke('open-card', accepted=False)
        self.paint([])
        self.assertEqual(self.invoke('card')['title']['count'], 0)
        self.invoke('open-card', accepted=False)


if __name__ == '__main__':
    unittest.main(verbosity=2)
