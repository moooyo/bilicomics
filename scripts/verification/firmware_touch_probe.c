/* Test-only evdev evidence collection in the isolated ARM firmware guest. */
#include <errno.h>
#include <fcntl.h>
#include <linux/input.h>
#include <poll.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <time.h>
#include <unistd.h>

/* The ARMv7 Kindle kernel writes two 32-bit timeval words, including on musl. */
struct kernel_input_event32 {
    uint32_t seconds;
    uint32_t microseconds;
    uint16_t kind;
    uint16_t code;
    int32_t value;
};
_Static_assert(sizeof(struct kernel_input_event32) == 16, "ARM input event layout");

static double now_seconds(void) {
    struct timespec value;
    if (clock_gettime(CLOCK_MONOTONIC, &value) != 0) exit(3);
    return (double)value.tv_sec + (double)value.tv_nsec / 1000000000.0;
}

static void query_axis(int fd, unsigned int axis) {
    struct input_absinfo value;
    memset(&value, 0, sizeof(value));
    if (ioctl(fd, EVIOCGABS(axis), &value) != 0) {
        printf("{\"kind\":\"axis_error\",\"axis\":%u,\"errno\":%d}\n", axis, errno);
        return;
    }
    printf("{\"kind\":\"axis\",\"axis\":%u,\"value\":%d,\"minimum\":%d,\"maximum\":%d}\n",
           axis, value.value, value.minimum, value.maximum);
}

static void query_slots(int fd, const char *phase) {
    struct input_absinfo slot;
    memset(&slot, 0, sizeof(slot));
    if (ioctl(fd, EVIOCGABS(ABS_MT_SLOT), &slot) != 0 || slot.minimum < 0 || slot.maximum > 63) {
        printf("{\"kind\":\"slot_error\",\"errno\":%d}\n", errno);
        return;
    }
    const unsigned int axes[] = {ABS_MT_POSITION_X, ABS_MT_POSITION_Y, ABS_MT_TRACKING_ID};
    for (unsigned int index = 0; index < 3; index++) {
        int32_t values[65] = {0};
        values[0] = (int32_t)axes[index];
        const unsigned int count = (unsigned int)slot.maximum + 1;
        const unsigned int bytes = (count + 1) * sizeof(int32_t);
        if (ioctl(fd, EVIOCGMTSLOTS(bytes), values) != 0) {
            printf("{\"kind\":\"mt_slots_error\",\"axis\":%u,\"errno\":%d}\n", axes[index], errno);
            continue;
        }
        printf("{\"kind\":\"mt_slots\",\"phase\":\"%s\",\"active_slot\":%d,\"axis\":%u,\"values\":[",
               phase, slot.value, axes[index]);
        for (unsigned int item = 0; item < count; item++) printf("%s%d", item ? "," : "", values[item + 1]);
        printf("]}\n");
    }
}

int main(int argc, char **argv) {
    if (argc != 4 || strncmp(argv[1], "/dev/input/event", 16) != 0) return 2;
    const int duration = atoi(argv[2]);
    if (duration < 1 || duration > 30) return 2;
    const int query_only = strcmp(argv[3], "query") == 0;
    if (!query_only && strcmp(argv[3], "read") != 0) return 2;
    const int fd = open(argv[1], O_RDONLY | O_NONBLOCK | O_CLOEXEC);
    if (fd < 0) return 3;
    if (!query_only && ioctl(fd, EVIOCGRAB, 1) != 0) {
        printf("{\"kind\":\"grab_error\",\"errno\":%d}\n", errno);
        close(fd);
        return 4;
    }
    query_axis(fd, ABS_MT_SLOT);
    query_axis(fd, ABS_MT_POSITION_X);
    query_axis(fd, ABS_MT_POSITION_Y);
    query_slots(fd, "opened");
    printf("{\"kind\":\"ready\",\"pid\":%ld,\"query_only\":%s}\n", (long)getpid(), query_only ? "true" : "false");
    fflush(stdout);
    if (!query_only) {
        const double end = now_seconds() + duration;
        while (now_seconds() < end) {
            struct pollfd descriptor = {fd, POLLIN, 0};
            const int polled = poll(&descriptor, 1, 200);
            if (polled < 0 && errno == EINTR) continue;
            if (polled < 0) break;
            if (!(descriptor.revents & POLLIN)) continue;
            struct kernel_input_event32 events[64];
            const ssize_t size = read(fd, events, sizeof(events));
            if (size < 0 && errno == EAGAIN) continue;
            if (size < 0 || size % sizeof(events[0]) != 0) break;
            for (size_t index = 0; index < (size_t)size / sizeof(events[0]); index++) {
                printf("{\"kind\":\"event\",\"seconds\":%u,\"microseconds\":%u,\"type\":%u,\"code\":%u,\"value\":%d}\n",
                       events[index].seconds, events[index].microseconds,
                       events[index].kind, events[index].code, events[index].value);
            }
            fflush(stdout);
        }
        query_slots(fd, "before_close");
        ioctl(fd, EVIOCGRAB, 0);
    }
    close(fd);
    fflush(stdout);
    return 0;
}
