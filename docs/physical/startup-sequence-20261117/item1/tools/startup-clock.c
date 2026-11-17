#define _GNU_SOURCE

#include <errno.h>
#include <fcntl.h>
#include <inttypes.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/stat.h>
#include <time.h>
#include <unistd.h>

enum {
    EXIT_USAGE = 64,
    EXIT_CLOCK = 70,
    EXIT_CREATE = 73,
    EXIT_RECORD = 74,
    EXIT_EXEC = 126,
    EXIT_EXEC_MISSING = 127
};

static int sample_ns(clockid_t clock_id, uint64_t *value) {
    struct timespec stamp;
    int status;
    do {
        status = clock_gettime(clock_id, &stamp);
    } while (status < 0 && errno == EINTR);
    if (status < 0) {
        return -1;
    }
    if (stamp.tv_sec < 0 || stamp.tv_nsec < 0 || stamp.tv_nsec >= 1000000000L) {
        errno = ERANGE;
        return -1;
    }
    const uint64_t seconds = (uint64_t)stamp.tv_sec;
    const uint64_t nanos = (uint64_t)stamp.tv_nsec;
    if (seconds > (UINT64_MAX - nanos) / UINT64_C(1000000000)) {
        errno = ERANGE;
        return -1;
    }
    *value = seconds * UINT64_C(1000000000) + nanos;
    return 0;
}

static int parse_deadline(const char *text, uint64_t *value) {
    uint64_t parsed = 0;
    size_t digits = 0;
    for (const unsigned char *cursor = (const unsigned char *)text; *cursor != '\0'; ++cursor) {
        if (digits >= 20 || *cursor < '0' || *cursor > '9') {
            return -1;
        }
        const uint64_t digit = (uint64_t)(*cursor - '0');
        if (parsed > (UINT64_MAX - digit) / UINT64_C(10)) {
            return -1;
        }
        parsed = parsed * UINT64_C(10) + digit;
        ++digits;
    }
    if (digits == 0) {
        return -1;
    }
    *value = parsed;
    return 0;
}

static int wait_until(uint64_t deadline_ns) {
    uint64_t now_ns;
    if (sample_ns(CLOCK_MONOTONIC, &now_ns) < 0) {
        return -1;
    }
    if (deadline_ns <= now_ns) {
        return 0;
    }
    if (deadline_ns - now_ns > UINT64_C(5000000000)) {
        errno = EINVAL;
        return -1;
    }
    const uint64_t seconds = deadline_ns / UINT64_C(1000000000);
    const time_t deadline_seconds = (time_t)seconds;
    if (deadline_seconds < 0 || (uint64_t)deadline_seconds != seconds) {
        errno = ERANGE;
        return -1;
    }
    const struct timespec deadline = {
        .tv_sec = deadline_seconds,
        .tv_nsec = (long)(deadline_ns % UINT64_C(1000000000))
    };
    int status;
    do {
        /* Restart with the same absolute deadline after an interruption. */
        status = clock_nanosleep(CLOCK_MONOTONIC, TIMER_ABSTIME, &deadline, NULL);
    } while (status == EINTR);
    if (status != 0) {
        errno = status;
        return -1;
    }
    return 0;
}

static int write_all(int fd, const char *data, size_t length) {
    size_t written = 0;
    while (written < length) {
        const ssize_t count = write(fd, data + written, length - written);
        if (count < 0 && errno == EINTR) {
            continue;
        }
        if (count <= 0) {
            if (count == 0) {
                errno = EIO;
            }
            return -1;
        }
        written += (size_t)count;
    }
    return 0;
}

static int report_failure(const char *operation, int code) {
    const int saved_errno = errno;
    fprintf(stderr, "startup-clock: %s: %s\n", operation, strerror(saved_errno));
    return code;
}

int main(int argc, char **argv) {
    const int wait_requested = argc > 1 && strcmp(argv[1], "--wait-until") == 0;
    const int output_index = wait_requested ? 3 : 1;
    const int command_index = output_index + 2;
    uint64_t deadline_ns = 0;
    if (argc <= command_index || argv[output_index][0] == '\0'
            || strcmp(argv[output_index + 1], "--") != 0 || argv[command_index][0] == '\0'
            || (wait_requested && parse_deadline(argv[2], &deadline_ns) < 0)) {
        fprintf(stderr, "Usage: startup-clock [--wait-until NS] OUTPUT -- EXEC [ARGS...]\n");
        return EXIT_USAGE;
    }
    /* Reject invalid budgets before creating evidence or executing anything. */
    if (wait_requested && wait_until(deadline_ns) < 0) {
        return report_failure("wait for monotonic deadline", EXIT_CLOCK);
    }

    umask(077);
    int fd;
    do {
        fd = open(argv[output_index], O_WRONLY | O_CREAT | O_EXCL | O_NOFOLLOW | O_CLOEXEC, 0600);
    } while (fd < 0 && errno == EINTR);
    if (fd < 0) {
        return report_failure("create request record", EXIT_CREATE);
    }

    struct stat identity;
    int status;
    do {
        status = fstat(fd, &identity);
    } while (status < 0 && errno == EINTR);
    if (status < 0) {
        const int saved_errno = errno;
        (void)close(fd);
        errno = saved_errno;
        return report_failure("inspect request record", EXIT_RECORD);
    }
    if (!S_ISREG(identity.st_mode) || identity.st_nlink != 1 || identity.st_size != 0
            || identity.st_uid != geteuid() || (identity.st_mode & 0777) != 0600) {
        (void)close(fd);
        errno = EINVAL;
        return report_failure("reject unsafe request record", EXIT_RECORD);
    }

    uint64_t monotonic_ns;
    if (sample_ns(CLOCK_MONOTONIC, &monotonic_ns) < 0) {
        const int saved_errno = errno;
        (void)close(fd);
        errno = saved_errno;
        return report_failure("sample monotonic clock", EXIT_CLOCK);
    }

    uint64_t boottime_ns = 0;
    int have_boottime = 0;
#ifdef CLOCK_BOOTTIME
    have_boottime = sample_ns(CLOCK_BOOTTIME, &boottime_ns) == 0;
#endif
    char boot_value[32];
    int length;
    if (have_boottime) {
        length = snprintf(boot_value, sizeof(boot_value), "%" PRIu64, boottime_ns);
    } else {
        length = snprintf(boot_value, sizeof(boot_value), "null");
    }
    if (length < 0 || (size_t)length >= sizeof(boot_value)) {
        (void)close(fd);
        errno = EOVERFLOW;
        return report_failure("format boottime clock", EXIT_RECORD);
    }

    char record[192];
    length = snprintf(record, sizeof(record),
        "{\"monotonic_ns\":%" PRIu64 ",\"boottime_ns\":%s,\"pid\":%ld}\n",
        monotonic_ns, boot_value, (long)getpid());
    if (length < 0 || (size_t)length >= sizeof(record)) {
        (void)close(fd);
        errno = EOVERFLOW;
        return report_failure("format request record", EXIT_RECORD);
    }
    if (write_all(fd, record, (size_t)length) < 0) {
        const int saved_errno = errno;
        (void)close(fd);
        errno = saved_errno;
        return report_failure("write request record", EXIT_RECORD);
    }
    /* A failed close leaves this new evidence file in place; never unlink it. */
    if (close(fd) < 0) {
        return report_failure("close request record", EXIT_RECORD);
    }

    /* Preserve argv exactly and replace this process without a shell. */
    execvp(argv[command_index], argv + command_index);
    const int exec_errno = errno;
    errno = exec_errno;
    return report_failure("execute requested program",
        exec_errno == ENOENT ? EXIT_EXEC_MISSING : EXIT_EXEC);
}
