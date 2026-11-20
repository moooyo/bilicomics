#define _POSIX_C_SOURCE 200809L
#include <X11/Xlib.h>
#include <X11/Xutil.h>
#include <ctype.h>
#include <errno.h>
#include <inttypes.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <tesseract/capi.h>
#include <time.h>
#include <unistd.h>

struct frame { int width, height; unsigned char *gray; };
struct bounds { int left, top, right, bottom; };
struct match { int count; struct bounds box; };
struct result { struct match home, library, title; };

static int fail(const char *reason) {
    fprintf(stderr, "native-target: %s\n", reason);
    return 1;
}

static int channel(unsigned long pixel, unsigned long mask) {
    if (!mask) return 0;
    while (!(mask & 1UL)) { mask >>= 1; pixel >>= 1; }
    return (int)(((pixel & mask) * 255UL + mask / 2) / mask);
}

static int capture(Display *display, struct frame *frame) {
    int screen = DefaultScreen(display);
    Window root = RootWindow(display, screen);
    XWindowAttributes attr;
    if (!XGetWindowAttributes(display, root, &attr) || attr.width < 300 || attr.height < 500 ||
        attr.width > 3000 || attr.height > 3200) return 0;
    XImage *image = XGetImage(display, root, 0, 0, attr.width, attr.height, AllPlanes, ZPixmap);
    if (!image) return 0;
    int indexed = attr.visual->class != TrueColor;
    XColor colors[256];
    if (indexed) {
        if (attr.visual->class == DirectColor || image->depth > 8 || !attr.colormap) {
            XDestroyImage(image); return 0;
        }
        int entries = 1 << image->depth;
        for (int index = 0; index < entries; ++index) colors[index].pixel = (unsigned long)index;
        if (!XQueryColors(display, attr.colormap, colors, entries)) { XDestroyImage(image); return 0; }
    }
    frame->width = attr.width;
    frame->height = attr.height;
    frame->gray = malloc((size_t)attr.width * attr.height);
    if (!frame->gray) { XDestroyImage(image); return 0; }
    for (int row = 0; row < attr.height; ++row) {
        for (int column = 0; column < attr.width; ++column) {
            unsigned long pixel = XGetPixel(image, column, row);
            int red, green, blue;
            if (indexed) {
                XColor color = colors[pixel & 255UL];
                red = color.red >> 8; green = color.green >> 8; blue = color.blue >> 8;
            } else {
                red = channel(pixel, image->red_mask); green = channel(pixel, image->green_mask);
                blue = channel(pixel, image->blue_mask);
            }
            frame->gray[(size_t)row * attr.width + column] = (unsigned char)((77 * red + 150 * green + 29 * blue + 128) >> 8);
        }
    }
    XDestroyImage(image);
    return 1;
}

static void record(struct match *match, struct bounds box) {
    if (++match->count == 1) match->box = box;
}

static int normalize(const char *text, char *out, size_t capacity) {
    size_t used = 0;
    while (*text) {
        unsigned char byte = (unsigned char)*text++;
        if (byte < 128 && isspace(byte)) continue;
        if (used + 1 >= capacity) return 0;
        out[used++] = byte < 128 ? (char)tolower(byte) : (char)byte;
    }
    out[used] = '\0';
    return 1;
}

static int aligned(struct bounds previous, struct bounds next) {
    int height = previous.bottom - previous.top;
    return next.top >= previous.bottom - height / 4 && next.top <= previous.bottom + height &&
        previous.left < next.right && next.left < previous.right;
}

static int recognize(TessBaseAPI *ocr, const struct frame *frame, struct result *result, int navigation) {
    int footer = frame->height * 85 / 100;
    int top = navigation ? footer : 0;
    if (navigation) TessBaseAPISetRectangle(ocr, 0, top, frame->width, frame->height - top);
    if (TessBaseAPIRecognize(ocr, NULL)) return 0;
    TessResultIterator *iterator = TessBaseAPIGetIterator(ocr);
    if (!iterator) return 1;
    int prefix = 0;
    struct bounds previous = {0};
    do {
        struct bounds box;
        char normalized[96];
        char *line = TessResultIteratorGetUTF8Text(iterator, RIL_TEXTLINE);
        int valid = line && normalize(line, normalized, sizeof(normalized)) &&
            TessResultIteratorConfidence(iterator, RIL_TEXTLINE) >= 55 &&
            TessPageIteratorBoundingBox(TessResultIteratorGetPageIterator(iterator), RIL_TEXTLINE,
                                        &box.left, &box.top, &box.right, &box.bottom) &&
            box.left >= 0 && box.top >= top && box.right > box.left && box.bottom > box.top &&
            box.right <= frame->width && box.bottom <= frame->height;
        if (line) TessDeleteText(line);
        if (!valid) { prefix = 0; continue; }
        if (box.top >= footer) {
            if (!strcmp(normalized, "home") || !strcmp(normalized, "\u4e3b\u9875") || !strcmp(normalized, "\u9996\u9875"))
                record(&result->home, box);
            if (!strcmp(normalized, "library") || !strcmp(normalized, "\u56fe\u4e66\u9986") ||
                !strcmp(normalized, "\u6211\u7684\u56fe\u4e66\u9986") || !strcmp(normalized, "\u4e66\u5e93"))
                record(&result->library, box);
        } else if (!navigation && box.top >= frame->height / 16 && box.bottom <= footer) {
            if (!strcmp(normalized, "bilicomicsforkindle")) record(&result->title, box);
            if (((prefix == 1 && !strcmp(normalized, "forkindle")) || (prefix == 2 && !strcmp(normalized, "kindle"))) &&
                aligned(previous, box)) {
                struct bounds combined = {previous.left < box.left ? previous.left : box.left, previous.top,
                    previous.right > box.right ? previous.right : box.right, box.bottom};
                record(&result->title, combined);
            }
            prefix = !strcmp(normalized, "bilicomics") ? 1 : !strcmp(normalized, "bilicomicsfor") ? 2 : 0;
            previous = box;
        }
    } while (TessResultIteratorNext(iterator, RIL_TEXTLINE));
    TessResultIteratorDelete(iterator);
    return 1;
}

static int underline(const struct frame *frame, struct bounds box) {
    int longest = 0;
    int limit = box.bottom + (box.bottom - box.top);
    if (limit > frame->height) limit = frame->height;
    for (int row = box.bottom + 1; row < limit; ++row) {
        int current = 0;
        for (int column = box.left; column < box.right; ++column) {
            if (frame->gray[(size_t)row * frame->width + column] < 100) ++current; else current = 0;
            if (current > longest) longest = current;
        }
    }
    return 100 * longest / (box.right - box.left);
}

static void print_match(const struct frame *frame, const struct match *match, int nav) {
    printf("{\"count\":%d", match->count);
    if (match->count == 1) {
        struct bounds box = match->box;
        printf(",\"box\":[%d,%d,%d,%d]", box.left, box.top, box.right, box.bottom);
        if (nav) printf(",\"underline_pct\":%d", underline(frame, box));
    }
    printf("}");
}

#ifdef BILI_NATIVE_INPUT
extern int XTestQueryExtension(Display *, int *, int *, int *, int *);
extern int XTestFakeMotionEvent(Display *, int, int, int, unsigned long);
extern int XTestFakeButtonEvent(Display *, unsigned int, Bool, unsigned long);

static int pointer_at(Display *display, int column, int row, unsigned expected) {
    Window root, child;
    int actual_x, actual_y, local_x, local_y;
    unsigned mask;
    return XQueryPointer(display, DefaultRootWindow(display), &root, &child, &actual_x, &actual_y, &local_x, &local_y, &mask) &&
        actual_x == column && actual_y == row &&
        (mask & (Button1Mask|Button2Mask|Button3Mask|Button4Mask|Button5Mask)) == expected;
}

static int same_region(const struct frame *original, const struct frame *current, struct bounds box) {
    if (original->width != current->width || original->height != current->height) return 0;
    box.left = box.left >= 8 ? box.left - 8 : 0;
    box.right = box.right + 8 < original->width ? box.right + 8 : original->width;
    box.top = box.top >= 6 ? box.top - 6 : 0;
    box.bottom = box.bottom + 16 < original->height ? box.bottom + 16 : original->height;
    for (int row = box.top; row < box.bottom; ++row) {
        size_t offset = (size_t)row * original->width + box.left;
        if (memcmp(original->gray + offset, current->gray + offset, (size_t)(box.right - box.left))) return 0;
    }
    return 1;
}

static uint64_t now_ns(void) {
    struct timespec value;
    if (clock_gettime(CLOCK_MONOTONIC, &value) != 0) return 0;
    return (uint64_t)value.tv_sec * UINT64_C(1000000000) + (uint64_t)value.tv_nsec;
}

static int activate(Display *display, const struct frame *original, const struct result *result, const char *mode) {
    if (result->home.count != 1 || result->library.count != 1 ||
        result->home.box.right >= result->library.box.left) return 0;
    int home_line = underline(original, result->home.box);
    int library_line = underline(original, result->library.box);
    int home_active = home_line >= 60 && library_line <= 20;
    int library_active = library_line >= 60 && home_line <= 20;
    int open_card = !strcmp(mode, "open-card");
    const struct match *target = open_card ? &result->title : !strcmp(mode, "nav-home") ? &result->home : &result->library;
    if (target->count != 1 || (!strcmp(mode, "nav-library") ? !home_active : !library_active)) return 0;
    int event_base, error_base, major, minor;
    if (!XTestQueryExtension(display, &event_base, &error_base, &major, &minor)) return 0;
    int column = (target->box.left + target->box.right) / 2, row = (target->box.top + target->box.bottom) / 2;
    if (!XTestFakeMotionEvent(display, DefaultScreen(display), column, row, CurrentTime)) return 0;
    XSync(display, False);
    struct frame current = {0};
    int stable = pointer_at(display, column, row, 0) && capture(display, &current) &&
        same_region(original, &current, result->home.box) && same_region(original, &current, result->library.box) &&
        same_region(original, &current, target->box);
    free(current.gray);
    if (!stable) return 0;
    sigset_t blocked, previous;
    sigemptyset(&blocked);
    sigaddset(&blocked, SIGTERM); sigaddset(&blocked, SIGHUP); sigaddset(&blocked, SIGINT); sigaddset(&blocked, SIGALRM);
    if (sigprocmask(SIG_BLOCK, &blocked, &previous)) return 0;
    uint64_t down_before = now_ns();
    int down = XTestFakeButtonEvent(display, 1, True, CurrentTime);
    XSync(display, False);
    uint64_t down_after = now_ns();
    int confirmed = down && pointer_at(display, column, row, Button1Mask);
    struct timespec hold = {0, 50000000};
    if (confirmed) while (nanosleep(&hold, &hold) != 0 && errno == EINTR) {}
    uint64_t up_before = now_ns();
    int up = XTestFakeButtonEvent(display, 1, False, CurrentTime);
    XSync(display, False);
    uint64_t up_after = now_ns();
    int clear = pointer_at(display, column, row, 0);
    sigprocmask(SIG_SETMASK, &previous, NULL);
    if (!confirmed || !up || !clear || !down_before || down_before > down_after || down_after + UINT64_C(50000000) > up_before || up_before > up_after) return 0;
    printf("{\"ok\":true,\"mode\":\"XTest-button-1\",\"clock\":\"CLOCK_MONOTONIC\",\"hold_requested_ms\":50,"
           "\"down_confirmed\":true,\"up_sent\":true,\"buttons_clear_after\":true,\"target\":\"%s\","
           "\"down_before_ns\":%" PRIu64 ",\"down_after_ns\":%" PRIu64 ",\"up_before_ns\":%" PRIu64
           ",\"up_after_ns\":%" PRIu64 "}\n", mode, down_before, down_after, up_before, up_after);
    return 1;
}
#endif

int main(int argc, char **argv) {
#ifdef BILI_NATIVE_INPUT
    if (argc == 4 && !strcmp(argv[1], "restore-pointer")) {
        char *end_x, *end_y;
        long column = strtol(argv[2], &end_x, 10), row = strtol(argv[3], &end_y, 10);
        if (!argv[2][0] || !argv[3][0] || *end_x || *end_y || column < 0 || column > 3000 || row < 0 || row > 3200)
            return fail("invalid saved pointer position");
        alarm(5);
        Display *display = XOpenDisplay(NULL);
        if (!display) return fail("cannot restore pointer without display");
        int screen = DefaultScreen(display);
        int moved = column < DisplayWidth(display, screen) && row < DisplayHeight(display, screen) &&
            XWarpPointer(display, None, RootWindow(display, screen), 0, 0, 0, 0, (int)column, (int)row);
        XSync(display, False);
        int ok = moved && pointer_at(display, (int)column, (int)row, 0);
        XCloseDisplay(display);
        if (ok) { puts("{\"restored\":true}"); return 0; }
        return fail("pointer restoration not confirmed");
    }
#endif
    if (argc != 3 || argv[2][0] != '/') return fail("expected a mode and absolute OCR data directory");
    int input = 0;
#ifdef BILI_NATIVE_INPUT
    input = !strcmp(argv[1], "open-card") || !strcmp(argv[1], "nav-home") || !strcmp(argv[1], "nav-library");
#endif
    if (!input && strcmp(argv[1], "navigation") && strcmp(argv[1], "card")) return fail("unknown operation");
    alarm(40);
    Display *display = XOpenDisplay(NULL);
    if (!display) return fail("cannot open the native X display");
    struct frame frame = {0};
    if (!capture(display, &frame)) { XCloseDisplay(display); return fail("unsupported native X image"); }
    Window returned_root, child;
    int pointer_x, pointer_y, local_x, local_y;
    unsigned int mask;
    int pointer = XQueryPointer(display, DefaultRootWindow(display), &returned_root, &child,
                                &pointer_x, &pointer_y, &local_x, &local_y, &mask);
    if (!pointer) { free(frame.gray); XCloseDisplay(display); return fail("could not read pointer state"); }
    struct result result = {0};
    TessBaseAPI *ocr = TessBaseAPICreate();
    int ok = ocr && TessBaseAPIInit3(ocr, argv[2], "eng+chi_sim") == 0;
    if (ok) {
        TessBaseAPISetVariable(ocr, "debug_file", "/dev/null");
        TessBaseAPISetPageSegMode(ocr, PSM_SPARSE_TEXT);
        TessBaseAPISetImage(ocr, frame.gray, frame.width, frame.height, 1, frame.width);
        TessBaseAPISetSourceResolution(ocr, 250);
        ok = recognize(ocr, &frame, &result, strcmp(argv[1], "card") && strcmp(argv[1], "open-card"));
    }
    if (ocr) TessBaseAPIDelete(ocr);
    if (input) {
#ifdef BILI_NATIVE_INPUT
        ok = ok && !(mask & (Button1Mask|Button2Mask|Button3Mask|Button4Mask|Button5Mask)) &&
            activate(display, &frame, &result, argv[1]);
#endif
    } else if (ok) {
        printf("{\"screen\":[%d,%d],\"pointer\":[%d,%d],\"buttons_clear\":%s,\"home\":",
               frame.width, frame.height, pointer_x, pointer_y,
               mask & (Button1Mask|Button2Mask|Button3Mask|Button4Mask|Button5Mask) ? "false" : "true");
        print_match(&frame, &result.home, 1); printf(",\"library\":"); print_match(&frame, &result.library, 1);
        printf(",\"title\":"); print_match(&frame, &result.title, 0); puts("}");
    }
    free(frame.gray);
    XCloseDisplay(display);
    return ok ? 0 : fail("OCR could not inspect the requested public controls");
}
