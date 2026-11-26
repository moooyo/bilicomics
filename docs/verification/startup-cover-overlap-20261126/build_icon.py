"""Generate the private native-card detector from the repository's public PNG."""

import argparse
from pathlib import Path
import random

from PIL import Image, ImageFilter

HERE = Path(__file__).resolve().parent


def reference(artwork):
    with Image.open(artwork) as source:
        image = source.convert('L')
    if image.size != (600, 800):
        raise ValueError('Expected the BiliComics 600 by 800 Library cover')
    high = image.filter(ImageFilter.MaxFilter(7))
    low = image.filter(ImageFilter.MinFilter(7))
    rng = random.Random(1601)
    groups = []
    for left, top, right, bottom in ((65, 240, 310, 410), (310, 225, 554, 445),
                                     (41, 410, 305, 618), (305, 420, 551, 618)):
        points = []
        for dark in (True, False):
            available = [(x, y) for y in range(top, bottom, 3) for x in range(left, right, 3)
                         if (high.getpixel((x, y)) < 95 if dark else low.getpixel((x, y)) > 215)]
            rng.shuffle(available)
            chosen = []
            for x, y in available:
                if all((px - x)**2 + (py - y)**2 >= 11**2 for px, py in chosen):
                    chosen.append((x, y))
                    if len(chosen) == 16:
                        break
            if len(chosen) != 16:
                raise ValueError('The cover no longer has the reference contrast')
            points.extend(chosen)
        groups.append(points)
    anchors = []
    for x in range(160, 306, 6):
        start = None
        for y in range(170, 590):
            dark = image.getpixel((x, y)) < 128
            if dark and start is None:
                start = y
            elif not dark and start is not None:
                if 140 <= y - start <= 260:
                    anchors.append((x, start, y))
                start = None
    if not 8 <= len(anchors) <= 35:
        raise ValueError('The public logo anchors changed')

    def row(points):
        return ','.join('{' + ','.join(map(str, point)) + '}' for point in points)

    checks = ',\n'.join('{' + row(points) + '}' for points in groups)
    return 'static const unsigned short icon_points[4][32][2] = {\n' + checks + '\n};\n' + \
           'static const unsigned short icon_anchors[][3] = {\n' + row(anchors) + '\n};\n'


def once(source, old, new):
    if source.count(old) != 1:
        raise ValueError('The pinned native recognizer boundary changed')
    return source.replace(old, new, 1)


def between(source, first, last, new):
    return once(source, source[source.index(first):source.index(last, source.index(first))], new)


def generate(original, artwork, directory):
    source = original.read_text(encoding='ascii')
    source = between(source, 'static int aligned(', 'static int recognize(', '#include "icon_match.c"\n\n')
    source = between(source, '        } else if (!navigation', '    } while (TessResultIteratorNext', '        }\n')
    source = once(source, '    int prefix = 0;\n    struct bounds previous = {0};\n', '')
    source = once(source, 'if (!valid) { prefix = 0; continue; }', 'if (!valid) continue;')
    source = once(source, 'int top = navigation ? footer : 0;', 'int top = footer;')
    source = once(source, 'if (navigation) TessBaseAPISetRectangle', 'TessBaseAPISetRectangle')
    source = once(source, '    TessResultIteratorDelete(iterator);',
                  '    TessResultIteratorDelete(iterator);\n    if (!navigation) result->title = public_icon(frame);')
    source = once(source, 'attr.width < 300 || attr.height < 500 ||\n        attr.width > 3000 || attr.height > 3200',
                  'attr.width != 1860 || attr.height != 2480')
    source = once(source, 'int main(int argc, char **argv) {',
                  'int main(int argc, char **argv) {\n    if (!private_capture()) return fail("private capture unavailable");')
    source = once(source, '    free(current.gray);', '''    if (stable && open_card) {
        struct match again = public_icon(&current);
        stable = again.count == 1 && again.box.left == target->box.left && again.box.top == target->box.top &&
            again.box.right == target->box.right && again.box.bottom == target->box.bottom;
    }
    free(current.gray);''')
    # Differentiate a confirmed rejection before any button-down from a
    # transport failure or a receipt failure after a possible dispatch.
    source = once(source, 'const struct result *result, const char *mode) {',
                  'const struct result *result, const char *mode, int *attempted) {')
    source = once(source, '    int down = XTestFakeButtonEvent',
                  '    *attempted = 1;\n    int down = XTestFakeButtonEvent')
    source = once(source, '''        ok = ok && !(mask & (Button1Mask|Button2Mask|Button3Mask|Button4Mask|Button5Mask)) &&
            activate(display, &frame, &result, argv[1]);''', '''        int attempted = 0;
        ok = ok && !(mask & (Button1Mask|Button2Mask|Button3Mask|Button4Mask|Button5Mask)) &&
            activate(display, &frame, &result, argv[1], &attempted);
        if (!ok && !attempted && !strcmp(argv[1], "open-card")) {
            puts("{\\"schema\\":\\"bili-card-input-v1\\",\\"dispatched\\":false}");
            ok = 1;
        }''')
    if 'bilicomicsforkindle' in source:
        raise ValueError('The artwork detector must not scan book text')
    (directory / 'native-target.c').write_text(source, encoding='ascii')
    (directory / 'icon_reference.h').write_text(reference(artwork), encoding='ascii')
    (directory / 'icon_match.c').write_bytes((HERE / 'icon_match.c').read_bytes())


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('original', 'artwork', 'directory'):
        parser.add_argument(name, type=Path)
    options = parser.parse_args()
    generate(options.original, options.artwork, options.directory)
