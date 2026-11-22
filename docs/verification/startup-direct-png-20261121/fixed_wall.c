#define _GNU_SOURCE
#include <dlfcn.h>
#include <errno.h>
#include <time.h>

typedef int (*ClockReader)(clockid_t, struct timespec *);
static ClockReader underlying_clock;

__attribute__((constructor)) static void bind_clock(void) {
    underlying_clock = (ClockReader)dlsym(RTLD_NEXT, "clock_gettime");
}

int clock_gettime(clockid_t clock, struct timespec *stamp) {
    if (clock == CLOCK_REALTIME) {
        stamp->tv_sec = 1795233600;
        stamp->tv_nsec = 0;
        return 0;
    }
    if (!underlying_clock) {
        errno = ENOSYS;
        return -1;
    }
    return underlying_clock(clock, stamp);
}
