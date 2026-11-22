#define _GNU_SOURCE

#include <errno.h>
#include <fcntl.h>
#include <inttypes.h>
#include <limits.h>
#include <stdbool.h>
#include <stdio.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

static bool valid_run(const char *run) {
    const char prefix[] = "bili-shelf-";
    if (strncmp(run, prefix, sizeof(prefix) - 1) != 0) return false;
    const char *digits = run + sizeof(prefix) - 1;
    return strlen(digits) == 32 && strspn(digits, "0123456789abcdef") == 32;
}

static bool record_directory(const char *path, const char *run, const char *leaf,
                             char directory[PATH_MAX]) {
    char suffix[96];
    const int suffix_size = snprintf(suffix, sizeof(suffix), "/%s/picked/%s", run, leaf);
    const size_t size = strlen(path);
    if (path[0] != '/' || size >= PATH_MAX || suffix_size <= 0 ||
        (size_t)suffix_size >= sizeof(suffix) || size < (size_t)suffix_size ||
        strcmp(path + size - (size_t)suffix_size, suffix) != 0) return false;
    const size_t length = size - strlen(leaf) - 1;
    memcpy(directory, path, length);
    directory[length] = '\0';
    return true;
}

static bool monotonic_ns(uint64_t *sample) {
    struct timespec now;
    if (clock_gettime(CLOCK_MONOTONIC, &now) != 0 || now.tv_sec < 0 ||
        now.tv_nsec < 0 || now.tv_nsec >= 1000000000L) return false;
    const uint64_t seconds = (uint64_t)now.tv_sec;
    const uint64_t nanos = (uint64_t)now.tv_nsec;
    const uint64_t nanos_per_second = UINT64_C(1000000000);
    if (seconds > (UINT64_MAX - nanos) / nanos_per_second) return false;
    *sample = seconds * nanos_per_second + nanos;
    return *sample != 0;
}

static bool write_record(int descriptor, const char *record, size_t remaining) {
    while (remaining != 0) {
        const ssize_t written = write(descriptor, record, remaining);
        if (written < 0 && errno == EINTR) continue;
        if (written <= 0) return false;
        record += written;
        remaining -= (size_t)written;
    }
    return true;
}

static bool mark(const char *directory, const char *leaf, const char *run, const char *stage) {
    uint64_t stamp;
    if (!monotonic_ns(&stamp)) return false;
    char record[256];
    const int length = snprintf(record, sizeof(record),
        "{\"schema\":\"bili-startup-clock-mark-v1\",\"run_id\":\"%s\",\"clock\":\"CLOCK_MONOTONIC\","
        "\"stage\":\"%s\",\"pid\":%ld,\"monotonic_ns\":%" PRIu64 "}\n",
        run, stage, (long)getpid(), stamp);
    if (length <= 0 || (size_t)length >= sizeof(record)) return false;
    const int parent = open(directory, O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC);
    if (parent < 0) return false;
    const int descriptor = openat(parent, leaf, O_WRONLY | O_CREAT | O_EXCL | O_NOFOLLOW | O_CLOEXEC, 0600);
    (void)close(parent);
    if (descriptor < 0) return false;
    const bool recorded = write_record(descriptor, record, (size_t)length);
    return close(descriptor) == 0 && recorded;
}

int main(int argc, char **argv) {
    char directory[PATH_MAX];
    const char *leaf = NULL;
    if (argc >= 6) {
        if (strcmp(argv[2], "shim_entry") == 0) leaf = "shim-entry.json";
        else if (strcmp(argv[2], "app_preinvoke") == 0) leaf = "app-preinvoke.json";
    }
    if (leaf == NULL || !valid_run(argv[1]) || !record_directory(argv[3], argv[1], leaf, directory) ||
        strcmp(argv[4], "--") != 0 || argv[5][0] != '/') {
        fputs("clock-marks: invalid diagnostic invocation\n", stderr);
        return 64;
    }
    if (!mark(directory, leaf, argv[1], argv[2])) {
        fputs("clock-marks: diagnostic mark unavailable\n", stderr);
    }
    execv(argv[5], argv + 5);
    const int status = errno == ENOENT ? 127 : 126;
    fputs("clock-marks: cannot execute original command\n", stderr);
    return status;
}
