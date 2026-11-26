/* Recognize the packaged BiliComics cover without exporting any screen pixels. */
#include "icon_reference.h"
#include <sys/prctl.h>
#include <sys/resource.h>

static int private_capture(void) {
    const struct rlimit zero = {0, 0};
    return setrlimit(RLIMIT_CORE, &zero) == 0 && prctl(PR_SET_DUMPABLE, 0) == 0;
}

static int icon_luma(const struct frame *frame, int x, int y) {
    const unsigned char *p = frame->gray + (size_t)y * frame->width + x;
    int sum = 0;
    for (int dy = -1; dy <= 1; ++dy) {
        const unsigned char *line = p + dy * frame->width;
        sum += line[-1] + line[0] + line[1];
    }
    return sum;
}

static int icon_score(const struct frame *frame, int x, int y, int width) {
    int total = 0;
    for (int quadrant = 0; quadrant < 4; ++quadrant) {
        int dark = 0, light = 0;
        for (int i = 0; i < 32; ++i) {
            const unsigned short *point = icon_points[quadrant][i];
            int luma = icon_luma(frame, x + (point[0] * width + 300) / 600,
                                 y + (point[1] * width + 300) / 600);
            if (i < 16) dark += luma < 127 * 9;
            else light += luma > 185 * 9;
        }
        if (dark < 13 || light < 13) return 0;
        total += dark + light;
    }
    return total;
}

static struct match public_icon(const struct frame *frame) {
    struct match result = {0};
    const int top = frame->height / 16, bottom = frame->height * 85 / 100;
    int best = 0, candidates = 0;
    for (int column = 2; column < frame->width - 2; column += 2) {
        int start = top;
        for (int row = top; row < bottom; ++row) {
            if (icon_luma(frame, column, row) < 128 * 9) continue;
            const int length = row - start;
            if (length >= 40 && length <= 470) {
                if (++candidates > 30000) return (struct match){0};
                for (size_t i = 0; i < sizeof(icon_anchors) / sizeof(icon_anchors[0]); ++i) {
                    const unsigned short *anchor = icon_anchors[i];
                    const int expected = anchor[2] - anchor[1];
                    const int estimate = (length * 600 + expected / 2) / expected;
                    if (estimate < 120 || estimate > 680) continue;
                    for (int adjustment = -3; adjustment <= 3; adjustment += 3) {
                        int width = estimate + adjustment;
                        int x = column - (anchor[0] * width + 300) / 600;
                        int y = start - (anchor[1] * width + 300) / 600;
                        struct bounds box = {x, y, x + width, y + width * 4 / 3};
                        if (box.left < 2 || box.top < top || box.right >= frame->width - 2 || box.bottom >= bottom)
                            continue;
                        int score = icon_score(frame, x, y, width);
                        if (!score) continue;
                        if (result.count && (abs(box.left - result.box.left) > width / 8 ||
                                             abs(box.top - result.box.top) > width / 8))
                            return (struct match){.count = 2};
                        if (score > best) {
                            best = score;
                            result = (struct match){.count = 1, .box = box};
                        }
                    }
                }
            }
            start = row + 1;
        }
    }
    return result;
}
