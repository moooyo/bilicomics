#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <linux/fb.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <unistd.h>

#if !defined(__linux__) || !defined(__x86_64__)
#error This fixture only supports the Linux x86-64 ioctl register ABI.
#endif

int fixture_ioctl(int descriptor, unsigned long request, uintptr_t word) __asm__("ioctl");

int fixture_ioctl(int descriptor, unsigned long request, uintptr_t word) {
    struct stat opened, expected;
    if ((request != FBIOGET_VSCREENINFO && request != FBIOGET_FSCREENINFO) ||
        fstat(descriptor, &opened) || stat("/dev/fb0", &expected) || !S_ISREG(expected.st_mode) ||
        opened.st_dev != expected.st_dev || opened.st_ino != expected.st_ino) {
        return (int)syscall(SYS_ioctl, descriptor, request, word);
    }
    const char *profile = getenv("BILICOMICS_APPROVED_PROFILE");
    const char *audit = getenv("DISPLAY_FIXTURE_FB_AUDIT");
    if (!profile || (strcmp(profile, "scribe") && strcmp(profile, "pw6")) || !audit || !word) {
        errno = EINVAL;
        return -1;
    }
    const unsigned width = !strcmp(profile, "scribe") ? 1860 : 1264;
    const unsigned height = !strcmp(profile, "scribe") ? 2480 : 1680;
    const int log = open(audit, O_WRONLY | O_APPEND | O_NOFOLLOW | O_CLOEXEC);
    if (log < 0) return -1;
    const ssize_t written = write(log, request == FBIOGET_VSCREENINFO ? "V" : "F", 1);
    if (close(log) || written != 1) return -1;
    if (request == FBIOGET_VSCREENINFO) {
        struct fb_var_screeninfo value = {0};
        value.xres = value.xres_virtual = width;
        value.yres = value.yres_virtual = height;
        value.bits_per_pixel = 8;
        value.grayscale = 1;
        memcpy((void *)word, &value, sizeof value);
    } else {
        struct fb_fix_screeninfo value = {0};
        value.type = FB_TYPE_PACKED_PIXELS;
        value.visual = FB_VISUAL_MONO01;
        value.smem_len = width * height;
        value.line_length = width;
        memcpy((void *)word, &value, sizeof value);
    }
    return 0;
}
