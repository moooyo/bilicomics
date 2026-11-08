/* Read-only framebuffer evidence for a supervised physical-device run. */
#define _FILE_OFFSET_BITS 64
#define _POSIX_C_SOURCE 200809L
#include <errno.h>
#include <fcntl.h>
#include <inttypes.h>
#include <linux/fb.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <unistd.h>

static int fail(const char *message) {
    fprintf(stderr, "physical-framebuffer: %s\n", message);
    return 1;
}

int main(int argc, char **argv) {
    int capture = argc == 2 && strcmp(argv[1], "--capture") == 0;
    if (argc != 2 || (!capture && strcmp(argv[1], "--info") != 0))
        return fail("Use --info or --capture.");
    int fd = open("/dev/fb0", O_RDONLY | O_CLOEXEC);
    if (fd < 0) return fail("The framebuffer cannot be opened read-only.");
    struct fb_fix_screeninfo fixed;
    struct fb_var_screeninfo variable;
    memset(&fixed, 0, sizeof(fixed));
    memset(&variable, 0, sizeof(variable));
    if (ioctl(fd, FBIOGET_FSCREENINFO, &fixed) != 0 || ioctl(fd, FBIOGET_VSCREENINFO, &variable) != 0) {
        close(fd);
        return fail("Framebuffer GET information failed.");
    }
    uint64_t bytes_per_pixel = variable.bits_per_pixel / 8;
    uint64_t row_bytes = (uint64_t) variable.xres * bytes_per_pixel;
    uint64_t first = (uint64_t) variable.yoffset * fixed.line_length + (uint64_t) variable.xoffset * bytes_per_pixel;
    uint64_t end = first + (uint64_t) (variable.yres ? variable.yres - 1 : 0) * fixed.line_length + row_bytes;
    if (!variable.xres || !variable.yres || variable.xres > 10000 || variable.yres > 10000 ||
        (variable.bits_per_pixel != 8 && variable.bits_per_pixel != 16 && variable.bits_per_pixel != 24 && variable.bits_per_pixel != 32) ||
        (uint64_t) variable.xoffset + variable.xres > variable.xres_virtual ||
        (uint64_t) variable.yoffset + variable.yres > variable.yres_virtual ||
        (uint64_t) (variable.xoffset + (uint64_t) variable.xres) * bytes_per_pixel > fixed.line_length ||
        fixed.line_length > 65536 || !fixed.smem_len || end > fixed.smem_len ||
        row_bytes * variable.yres > 128 * 1024 * 1024 || fixed.type != FB_TYPE_PACKED_PIXELS) {
        close(fd);
        return fail("Framebuffer geometry is outside the bounded packed-pixel reader.");
    }
    uint16_t endian_probe = 1;
    printf("{\"width\":%" PRIu32 ",\"height\":%" PRIu32 ",\"virtual_width\":%" PRIu32
           ",\"virtual_height\":%" PRIu32 ",\"xoffset\":%" PRIu32 ",\"yoffset\":%" PRIu32
           ",\"bits_per_pixel\":%" PRIu32 ",\"line_length\":%" PRIu32 ",\"smem_len\":%" PRIu32
           ",\"visual\":%" PRIu32 ",\"grayscale\":%" PRIu32 ",\"rotate\":%" PRIu32
           ",\"red_offset\":%" PRIu32 ",\"red_length\":%" PRIu32 ",\"red_msb_right\":%" PRIu32
           ",\"green_offset\":%" PRIu32 ",\"green_length\":%" PRIu32 ",\"green_msb_right\":%" PRIu32
           ",\"blue_offset\":%" PRIu32 ",\"blue_length\":%" PRIu32 ",\"blue_msb_right\":%" PRIu32
           ",\"byte_order\":\"%s\",\"visible_bytes\":%" PRIu64 ",\"read_only\":true}\n",
           variable.xres, variable.yres, variable.xres_virtual, variable.yres_virtual,
           variable.xoffset, variable.yoffset, variable.bits_per_pixel, fixed.line_length, fixed.smem_len,
           fixed.visual, variable.grayscale, variable.rotate,
           variable.red.offset, variable.red.length, variable.red.msb_right,
           variable.green.offset, variable.green.length, variable.green.msb_right,
           variable.blue.offset, variable.blue.length, variable.blue.msb_right,
           *(unsigned char *) &endian_probe ? "little" : "big", row_bytes * variable.yres);
    if (fflush(stdout) != 0) { close(fd); return fail("The information output failed."); }
    if (capture) {
        unsigned char *row = malloc((size_t) row_bytes);
        if (!row) { close(fd); return fail("The bounded row buffer could not be allocated."); }
        for (uint32_t y = 0; y < variable.yres; ++y) {
            size_t complete = 0;
            while (complete < row_bytes) {
                ssize_t count = pread(fd, row + complete, (size_t) row_bytes - complete,
                                      (off_t) (first + (uint64_t) y * fixed.line_length + complete));
                if (count < 0 && errno == EINTR) continue;
                if (count <= 0) { free(row); close(fd); return fail("A bounded framebuffer row read failed."); }
                complete += (size_t) count;
            }
            if (fwrite(row, 1, (size_t) row_bytes, stdout) != row_bytes) {
                free(row); close(fd); return fail("The framebuffer evidence output failed.");
            }
        }
        free(row);
    }
    close(fd);
    return fflush(stdout) == 0 ? 0 : fail("The evidence output could not be flushed.");
}
