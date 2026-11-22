"""Exercise the desktop observer against a scripted X window, not a BiliComics executable."""

import ctypes
import json
import os
import subprocess
import sys
import time

from desktop_pairs import FULL, ROOT, SHELF_TEST, SIZE, Sampler, known_layout, now, packed, reference_pixels, require, rgb, valid_x11_order, visible_rgb, wait_child, watch, window_id


def draw():
    api = ctypes.CDLL('libX11.so.6')
    pointer, word, integer, unsigned = ctypes.c_void_p, ctypes.c_ulong, ctypes.c_int, ctypes.c_uint

    def native(name, result, *arguments):
        function = getattr(api, name)
        function.argtypes, function.restype = arguments, result
        return function

    display = native('XOpenDisplay', pointer, ctypes.c_char_p)(None)
    parent = native('XDefaultRootWindow', word, pointer)(display)
    window = native('XCreateSimpleWindow', word, pointer, word, integer, integer, unsigned, unsigned, unsigned, word, word)(display, parent, 0, 0, *SIZE, 0, 0, 0)
    native('XStoreName', integer, pointer, word, ctypes.c_char_p)(display, window, b'BiliComics - Design Preview')
    atom = native('XInternAtom', word, pointer, ctypes.c_char_p, integer)(display, b'_NET_WM_PID', 0)
    pid = word(os.getpid())
    native('XChangeProperty', integer, pointer, word, word, word, integer, integer, pointer, integer)(display, window, atom, 6, 32, 0, ctypes.byref(pid), 1)
    native('XMapWindow', integer, pointer, word)(display, window)
    visual = native('XDefaultVisual', pointer, pointer, integer)(display, 0)
    context = native('XCreateGC', pointer, pointer, word, word, pointer)(display, window, 0, None)
    create_image = native('XCreateImage', pointer, pointer, pointer, unsigned, integer, integer, pointer, unsigned, unsigned, integer, integer)
    send_image = native('XPutImage', integer, pointer, word, pointer, pointer, integer, integer, integer, integer, unsigned, unsigned)
    sync = native('XSync', integer, pointer, integer)

    def show(image, box):
        left, top, width, height = box
        payload = ctypes.create_string_buffer(packed(image, box))
        native_image = create_image(display, visual, 24, 2, 0, payload, width, height, 32, width * 4)
        require(native_image, 'Cannot create mock image')
        send_image(display, window, context, native_image, 0, 0, left, top, width, height)
        sync(display, 0)

    first, activity, shelf = [rgb(ROOT / 'references' / file) for file in ('startup-0.png', 'startup-1.png', 'final-x11.png')]
    for image, box in ((first, FULL), (activity, FULL), (shelf, SHELF_TEST), (shelf, FULL)):
        before = now()
        show(image, box)
        print(json.dumps([before, now()]), flush=True)
        time.sleep(.25)


def rehearsal():
    require(visible_rgb(b'\x11\x33\x55\x00\x22\x44\x66\xff', (2, 1)) == b'\x55\x33\x11\x66\x44\x22', 'Padding affected RGB comparison')
    for depth, bits, stride, red, allowed in ((24, 32, 8, 0xff0000, True), (32, 32, 8, 0xff0000, True),
                                              (30, 32, 8, 0xff0000, False), (32, 24, 8, 0xff0000, False),
                                              (32, 32, 12, 0xff0000, False), (32, 32, 8, 0xff, False)):
        require(known_layout((2, 1, 2, depth, bits, 0, stride, red, 0xff00, 0xff), (2, 1)) == allowed, 'Layout contract changed')
    require(valid_x11_order(5, (10, 12), (20, 23), 27, (25, 30)) and not valid_x11_order(5, (10, 12), (20, 23), 31, (25, 30)), 'A cross-ready X11 reply was misclassified')
    require(window_id(None) is None, 'Mock needs its own X server')
    pixels = reference_pixels(*(rgb(ROOT / 'references' / file) for file in ('startup-0.png', 'final-x11.png')))
    sampler = Sampler()
    origin = now()
    process = subprocess.Popen([sys.executable, '-B', __file__, 'draw'], stdout=subprocess.PIPE, text=True)
    try:
        observed = watch(sampler, process, pixels, origin)
        output, _ = process.communicate(timeout=6)
        require(process.returncode == 0, 'Mock failed')
    finally:
        sampler.close()
        wait_child(process)
    writes = [json.loads(line) for line in output.splitlines()]
    require(len(writes) == 4 and observed['activity_states'] == ['initial', 'changed'], 'Missing visible mock activity')
    require(observed['last_non_shelf_sample_ns'][0] >= writes[2][1] and observed['first_exact_shelf_sample_ns'][1] >= writes[3][0], 'Prefilter incorrectly classified the partial Shelf')
    print(json.dumps({'matched_actual_window_pid': observed['pid'], 'saw_changing_activity': True, 'rejected_partial_shelf': True, 'ignored_padding': True, 'cross_ready_reply': True,
                      'full_read_count': len(observed['full_xgetimage_intervals_ns']), 'poll_target_ns': 4_000_000}), flush=True)


if __name__ == '__main__':
    draw() if sys.argv[1:] == ['draw'] else rehearsal()
