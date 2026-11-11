"""Exercise physical-run wrappers with owned Linux processes and fake services.

Run only on the designated remote worker. These tests use no device transport,
framebuffer, account, native firmware service, or injected physical input. The
small ELF fixture exists because the wrappers deliberately verify /proc/exe.
"""

from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import secrets
import shlex
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import unittest


REPOSITORY = Path(__file__).resolve().parents[2]
HELPERS = REPOSITORY / "scripts/verification/physical"


ELF_FIXTURE = r'''
#define _GNU_SOURCE
#define _POSIX_C_SOURCE 200809L
#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

#ifndef RENAME_NOREPLACE
#define RENAME_NOREPLACE 1
#endif

static volatile sig_atomic_t stopping;
static volatile sig_atomic_t suspend_pending;
static volatile sig_atomic_t resume_pending;
static unsigned int portrait_rotation;
static char portrait_orientation[6] = "FIXED";
static void stop(int unused) { (void)unused; stopping = 1; }
static void power_signal(int received) {
    if (received == SIGUSR1) suspend_pending = 1;
    if (received == SIGUSR2) resume_pending = 1;
}
static void record(const char *role, int argc, char **argv) {
    const char *path = getenv("TEST_EXECUTIONS");
    FILE *stream = path ? fopen(path, "a") : NULL;
    if (!stream) _exit(80);
    fprintf(stream, "%s:%ld\n", role, (long)getpid());
    if (fclose(stream)) _exit(81);
    path = getenv("TEST_ARGUMENT_DIRECTORY");
    char name[4096];
    if (!path || snprintf(name, sizeof(name), "%s/%s-%ld", path, role, (long)getpid()) >= (int)sizeof(name)) _exit(82);
    stream = fopen(name, "w");
    if (!stream) _exit(83);
    for (int index = 0; index < argc; ++index) fprintf(stream, "%s\n", argv[index]);
    if (fclose(stream)) _exit(84);
}
static int short_record(const unsigned char *bytes, size_t count) {
    for (;;) {
        ssize_t written = write(STDOUT_FILENO, bytes, count);
        if (written == (ssize_t)count) return 0;
        if (written < 0 && errno == EINTR) continue;
        if (written >= 0 || errno != EAGAIN) return 88;
        struct pollfd output = {STDOUT_FILENO, POLLOUT, 0};
        if (poll(&output, 1, 2000) <= 0) return 88;
    }
}
static int drain_source(unsigned char *pending, size_t *count) {
    unsigned char block[256];
    for (;;) {
        ssize_t size = read(STDIN_FILENO, block, sizeof(block));
        if (size < 0 && errno == EINTR) continue;
        if (size < 0 && errno == EAGAIN) return 1;
        if (size < 0) return -85;
        if (!size) return *count ? -86 : 0;
        for (ssize_t index = 0; index < size; ++index) {
            unsigned char byte = block[index];
            if (*count >= 255 && byte != '\n') return -87;
            pending[(*count)++] = byte;
            if (byte == '\n') {
                int result = short_record(pending, *count);
                if (result) return -result;
                *count = 0;
            }
        }
    }
}
static int relay(void) {
    unsigned char native[256], control[256];
    size_t native_count = 0, control_count = 0;
    for (int index = 0; index < 3; ++index) {
        int descriptor = index == 2 ? 3 : index;
        int flags = fcntl(descriptor, F_GETFL);
        if (flags < 0 || fcntl(descriptor, F_SETFL, flags | O_NONBLOCK) < 0) return 85;
    }
    for (;;) {
        int drained = drain_source(native, &native_count);
        if (drained <= 0) return drained ? -drained : 0;
        if (control_count && control[control_count - 1] == '\n' && !native_count) {
            int result = short_record(control, control_count);
            if (result) return result;
            control_count = 0;
        }
        struct pollfd streams[2] = {{STDIN_FILENO, POLLIN, 0}, {3, POLLIN, 0}};
        nfds_t stream_count = (native_count || control_count) ? 1 : 2;
        int ready = poll(streams, stream_count, 2000);
        if (ready < 0 && errno == EINTR) continue;
        if (ready <= 0) {
            if (native_count || control_count) return 86;
            continue;
        }
        if (streams[0].revents & (POLLIN | POLLHUP)) continue;
        if (stream_count == 2 && (streams[1].revents & POLLIN)) {
            // Read only one bounded control record; the next iteration drains
            // all already readable native events before forwarding the notice.
            while (control_count < sizeof(control)) {
                ssize_t size = read(3, control + control_count, 1);
                if (size < 0 && errno == EINTR) continue;
                if (size < 0 && errno == EAGAIN) break;
                if (size != 1) return 86;
                if (control[control_count++] == '\n') break;
            }
            if (control_count == sizeof(control) && control[control_count - 1] != '\n') return 87;
        }
    }
}
static int events(void) {
    const char *path = getenv("TEST_EVENT_INPUT");
    int input = path ? open(path, O_RDWR) : -1;
    if (input < 0) return 89;
    static const char initial[] = "wakeupFromSuspend 0\n";
    if (write(STDOUT_FILENO, initial, sizeof(initial) - 1) != (ssize_t)(sizeof(initial) - 1)) return 90;
    unsigned char block[256];
    for (;;) {
        ssize_t size = read(input, block, sizeof(block));
        if (size < 0 && errno == EINTR) continue;
        if (size < 0) return 91;
        if (!size) return 0;
        if (write(STDOUT_FILENO, block, (size_t)size) != size) return 92;
    }
}
static int read_record(const char *directory, const char *leaf, char *record, size_t capacity) {
    char name[4096];
    if (snprintf(name, sizeof(name), "%s/%s", directory, leaf) >= (int)sizeof(name)) return -1;
    FILE *stream = fopen(name, "r");
    if (!stream) return -1;
    if (!fgets(record, (int)capacity, stream)) { fclose(stream); return -1; }
    if (fgetc(stream) != EOF) { fclose(stream); return -1; }
    if (fclose(stream)) return -1;
    size_t size = strlen(record);
    if (size && record[size - 1] == '\n') record[--size] = 0;
    return size ? 0 : -1;
}
static int canonical_stream(FILE *stream, char *record, size_t capacity) {
    size_t size = fread(record, 1, capacity, stream);
    int failed = ferror(stream) || size < 2 || size >= capacity;
    if (fclose(stream)) failed = 1;
    if (failed || record[size - 1] != '\n') return -1;
    for (size_t index = 0; index + 1 < size; ++index) {
        if ((unsigned char)record[index] < 33 || (unsigned char)record[index] > 126) return -1;
    }
    record[size - 1] = 0;
    return 0;
}
static int canonical_line(const char *path, char *record, size_t capacity) {
    FILE *stream = fopen(path, "r");
    return stream ? canonical_stream(stream, record, capacity) : -1;
}
static int canonical_marker(const char *directory, const char *leaf, char *record, size_t capacity) {
    char name[4096];
    if (snprintf(name, sizeof(name), "%s/%s", directory, leaf) >= (int)sizeof(name)) return -1;
    return canonical_line(name, record, capacity);
}
static int current_portrait_rotation(unsigned int *rotation) {
    const char *path = getenv("TEST_PORTRAIT_ROTATION");
    char record_value[8];
    if (!path || canonical_line(path, record_value, sizeof(record_value))
            || strlen(record_value) != 1 || record_value[0] < '0' || record_value[0] > '3') return -1;
    *rotation = (unsigned int)(record_value[0] - '0');
    return 0;
}
static int resolve_portrait_calibration(const char *directory, const char *generation,
                                       const char *nonce, unsigned long sequence) {
    char marker[256], prefix[128], direction[6], extra;
    if (sequence) {
        char expected[160];
        if (canonical_marker(directory, "resume-native-portrait-orientation", marker, sizeof(marker))
                || snprintf(expected, sizeof(expected), "%s:%s:%lu:CALIBRATED", generation, nonce, sequence) >= (int)sizeof(expected)
                || strcmp(marker, expected)) return -1;
    }
    if (snprintf(prefix, sizeof(prefix), "%s:%s:", generation, nonce) >= (int)sizeof(prefix)
            || canonical_marker(directory, "native-portrait-orientation", marker, sizeof(marker))
            || strncmp(marker, prefix, strlen(prefix))) return -1;
    const char *value = marker + strlen(prefix);
    unsigned int baseline;
    if (sscanf(value, "%5[^:]:%u%c", direction, &baseline, &extra) != 2 || baseline > 3
            || (strcmp(direction, "U") && strcmp(direction, "D") && strcmp(direction, "FIXED"))) return -1;
    char canonical[16];
    if (snprintf(canonical, sizeof(canonical), "%s:%u", direction, baseline) >= (int)sizeof(canonical)
            || strcmp(value, canonical) || current_portrait_rotation(&portrait_rotation)) return -1;
    unsigned int delta = (portrait_rotation + 4 - baseline) % 4;
    if ((!sequence && delta) || (delta != 0 && delta != 2) || (!strcmp(direction, "FIXED") && delta)) return -1;
    if (delta == 2) strcpy(portrait_orientation, !strcmp(direction, "U") ? "D" : "U");
    else strcpy(portrait_orientation, direction);
    return 0;
}
static int portrait_metadata(char *metadata, size_t capacity) {
    const char *path = getenv("TEST_PORTRAIT_METADATA");
    if (!path || canonical_line(path, metadata, capacity)) return -1;
    size_t length = strlen(metadata);
    if (!((length == 6 && !strncmp(metadata, "AUTO:", 5))
            || (length == 7 && !strncmp(metadata, "FIXED:", 6)))
            || metadata[length - 2] != ':'
            || metadata[length - 1] < '0' || metadata[length - 1] > '3') return -1;
    return 0;
}
static int portrait_helper(int argc, char **argv) {
    const char *approved = getenv("BILICOMICS_APPROVED_PROFILE");
    char metadata[16];
    if (argc != 5 || strcmp(argv[1], "--probe-native-portrait-framebuffer")
            || strcmp(argv[2], "--kindle") || strcmp(argv[3], "--device")
            || !approved || strcmp(argv[4], approved)
            || portrait_metadata(metadata, sizeof(metadata))) return 79;
    record("portrait-probe", argc, argv);
    return printf("%s\n", metadata) < 0 ? 79 : 0;
}
static int capture_fixture_value(const char *command, char *value, size_t capacity) {
    // These fixed commands invoke only the isolated fake LIPC services. The
    // production native-reader group still bounds their complete lifetime.
    FILE *stream = popen(command, "r");
    if (!stream) return -1;
    size_t size = fread(value, 1, capacity, stream);
    int failed = ferror(stream) || !size || size >= capacity;
    int status = pclose(stream);
    if (failed || status == -1 || !WIFEXITED(status) || WEXITSTATUS(status)) return -1;
    if (value[size - 1] == '\n') --size;
    if (!size) return -1;
    for (size_t index = 0; index < size; ++index) {
        if ((unsigned char)value[index] < 32 || (unsigned char)value[index] > 126) return -1;
    }
    value[size] = 0;
    return 0;
}
static int capture_fixture_power(void) {
    char value[129];
    if (capture_fixture_value("lipc-get-prop -s com.lab126.powerd state", value, sizeof(value))) return -1;
    return !strcmp(value, "active");
}
static int capture_fixture_direction(const char *profile, char *direction) {
    char value[129];
    if (capture_fixture_value("lipc-get-prop com.lab126.winmgr accelerometer", value, sizeof(value))) return -1;
    if (!strcmp(value, "U") || (!strcmp(profile, "scribe") && !strcmp(value, "L"))) *direction = 'U';
    else if (!strcmp(value, "D") || (!strcmp(profile, "scribe") && !strcmp(value, "R"))) *direction = 'D';
    else return -1;
    return 0;
}
static int capture_fixture_metadata(char *metadata, size_t capacity) {
    if (portrait_metadata(metadata, capacity)) return -1;
    const char *path = getenv("TEST_NATIVE_CAPTURE_SAMPLES");
    FILE *stream = path ? fopen(path, "a") : NULL;
    if (!stream) return -1;
    int failed = fprintf(stream, "%ld:%s\n", (long)getpid(), metadata) < 0;
    if (fclose(stream)) failed = 1;
    return failed ? -1 : 0;
}
static int native_capture(int argc, char **argv) {
    const char *approved = getenv("BILICOMICS_APPROVED_PROFILE");
    const char *directory = getenv("BILICOMICS_LIFECYCLE_DIR");
    const char *generation = getenv("BILICOMICS_LAUNCH_GENERATION");
    const char *nonce = getenv("BILICOMICS_LAUNCH_TOKEN");
    char marker[256], binding[256];
    if (argc != 5 || strcmp(argv[1], "--capture-native-display")
            || strcmp(argv[2], "--kindle") || strcmp(argv[3], "--device")
            || !approved || strcmp(argv[4], approved) || !directory || directory[0] != '/'
            || !generation || !nonce || strlen(nonce) != 32
            || snprintf(binding, sizeof(binding), "%s:%s", generation, nonce) >= (int)sizeof(binding)
            || canonical_marker(directory, "generation", marker, sizeof(marker)) || strcmp(marker, binding)
            || canonical_marker(directory, "nonce", marker, sizeof(marker)) || strcmp(marker, nonce)) return 79;
    // The real wrapper proves the protected ownership chain before this ELF
    // entry. This fixture checks the forwarded context before simulated I/O.
    record("native-capture", argc, argv);
    int active = capture_fixture_power();
    if (active < 0) return 79;
    if (!active) return printf("not-active\n") < 0 ? 79 : 0;
    char mode[129], first[16], second[16], direction = 0, repeated_direction = 0;
    if (capture_fixture_value("lipc-get-prop -s com.lab126.winmgr epdcMode", mode, sizeof(mode))
            || (strcmp(mode, "Y8") && strcmp(mode, "Y8INV"))
            || capture_fixture_metadata(first, sizeof(first))) return 79;
    int automatic = !strncmp(first, "AUTO:", 5);
    if (automatic && capture_fixture_direction(approved, &direction)) return 79;
    if (capture_fixture_metadata(second, sizeof(second)) || strcmp(first, second)) return 79;
    if (automatic && (capture_fixture_direction(approved, &repeated_direction)
            || repeated_direction != direction)) return 79;
    active = capture_fixture_power();
    if (active < 0) return 79;
    if (!active) return printf("not-active\n") < 0 ? 79 : 0;
    char orientation[6] = "FIXED";
    if (automatic) { orientation[0] = direction; orientation[1] = 0; }
    return printf("ready:%s:%s:%c\n", mode, orientation, first[strlen(first) - 1]) < 0 ? 79 : 0;
}
static int positive_sequence(const char *text, unsigned long long *value) {
    size_t length = strlen(text);
    if (!length || length > 20 || text[0] == '0') return -1;
    for (size_t index = 0; index < length; ++index) {
        if (text[index] < '0' || text[index] > '9') return -1;
    }
    errno = 0;
    char *end;
    *value = strtoull(text, &end, 10);
    return errno || *end || !*value ? -1 : 0;
}
struct publisher_process {
    long pid, parent, group, session;
    unsigned long long birth;
};
static int publisher_process(long pid, struct publisher_process *pin) {
    char path[64], line[4096];
    if (pid <= 1 || snprintf(path, sizeof(path), "/proc/%ld/stat", pid) >= (int)sizeof(path)) return -1;
    FILE *stream = fopen(path, "r");
    if (!stream) return -1;
    int failed = !fgets(line, sizeof(line), stream);
    if (fclose(stream)) failed = 1;
    if (failed) return -1;
    char *tail = strrchr(line, ')');
    if (!tail || tail[1] != ' ') return -1;
    char *context = NULL;
    pin->pid = pid;
    for (int index = 0; index <= 19; ++index) {
        char *field = strtok_r(index ? NULL : tail + 2, " ", &context);
        if (!field || (!index && (*field == 'Z' || *field == 'X'))) return -1;
        if (index >= 1 && index <= 3) {
            unsigned long long value;
            if (positive_sequence(field, &value) || value > 2147483647ULL) return -1;
            if (index == 1) pin->parent = (long)value;
            if (index == 2) pin->group = (long)value;
            if (index == 3) pin->session = (long)value;
        }
        if (index == 19 && positive_sequence(field, &pin->birth)) return -1;
    }
    return 0;
}
static int same_publisher_process(const struct publisher_process *before) {
    struct publisher_process after;
    return !publisher_process(before->pid, &after) && before->parent == after.parent
        && before->group == after.group && before->session == after.session && before->birth == after.birth;
}
static int publisher_marker(int directory, const char *leaf, char *record_value, size_t capacity) {
    int descriptor = openat(directory, leaf, O_RDONLY | O_CLOEXEC | O_NOFOLLOW | O_NONBLOCK);
    if (descriptor < 0) return -1;
    struct stat information;
    if (fstat(descriptor, &information) || !S_ISREG(information.st_mode) || information.st_uid != geteuid()
            || (information.st_mode & 0077) || information.st_nlink != 1) { close(descriptor); return -1; }
    FILE *stream = fdopen(descriptor, "r");
    if (!stream) { close(descriptor); return -1; }
    return canonical_stream(stream, record_value, capacity);
}
static int publisher_absent(int directory, const char *leaf) {
    struct stat information;
    return fstatat(directory, leaf, &information, AT_SYMLINK_NOFOLLOW) == -1 && errno == ENOENT;
}
static int publisher_object_matches(int directory, const char *leaf, const struct stat *before) {
    struct stat current;
    return !fstatat(directory, leaf, &current, AT_SYMLINK_NOFOLLOW) && S_ISREG(current.st_mode)
        && current.st_uid == geteuid() && !(current.st_mode & 0077) && current.st_nlink == 1
        && current.st_dev == before->st_dev && current.st_ino == before->st_ino;
}
static int publisher_directory_matches(const char *path, const struct stat *before) {
    struct stat current;
    return !lstat(path, &current) && S_ISDIR(current.st_mode) && current.st_uid == geteuid()
        && (current.st_mode & 0777) == 0700 && current.st_dev == before->st_dev && current.st_ino == before->st_ino;
}
static int publisher_binding(int directory, const char *generation, const char *nonce,
                             const char *permission, unsigned long long resume_sequence, unsigned long long owning_master) {
    char value[256], expected[256];
    unsigned long long master;
    if (publisher_marker(directory, "nonce", value, sizeof(value)) || strcmp(value, nonce)
            || snprintf(expected, sizeof(expected), "%s:%s", generation, nonce) >= (int)sizeof(expected)
            || publisher_marker(directory, "generation", value, sizeof(value)) || strcmp(value, expected)
            || publisher_marker(directory, "native-read-go", value, sizeof(value)) || strcmp(value, permission)
            || publisher_marker(directory, "pid", value, sizeof(value)) || positive_sequence(value, &master)
            || master != owning_master) return -1;
    if (!resume_sequence) return publisher_absent(directory, "application-pid")
            && publisher_absent(directory, "lifecycle-request") ? 0 : -1;
    if (resume_sequence <= 1 || publisher_marker(directory, "lifecycle-ready", value, sizeof(value))
            || strcmp(value, expected)) return -1;
    if (snprintf(expected, sizeof(expected), "%s:%s:%llu:suspend", generation, nonce,
                 resume_sequence - 1) >= (int)sizeof(expected)
            || publisher_marker(directory, "lifecycle-request", value, sizeof(value)) || strcmp(value, expected)) return -1;
    char acknowledgement[256];
    if (publisher_marker(directory, "lifecycle-ack", acknowledgement, sizeof(acknowledgement))) return -1;
    if (strcmp(acknowledgement, expected)) {
        if (snprintf(value, sizeof(value), "%s:ok", expected) >= (int)sizeof(value)) return -1;
        if (strcmp(acknowledgement, value)) {
            if (snprintf(value, sizeof(value), "%s:checkpoint-error", expected) >= (int)sizeof(value)
                    || strcmp(acknowledgement, value)) return -1;
        }
    }
    unsigned long long application;
    struct publisher_process app_pin, runner_pin;
    if (publisher_marker(directory, "application-pid", value, sizeof(value)) || positive_sequence(value, &application)
            || application > 2147483647ULL || publisher_process((long)application, &app_pin)
            || publisher_process(app_pin.parent, &runner_pin)
            || runner_pin.parent != (long)master || !same_publisher_process(&app_pin)
            || !same_publisher_process(&runner_pin)) return -1;
    return 0;
}
static int portrait_publisher(int argc, char **argv) {
    const char *approved = getenv("BILICOMICS_APPROVED_PROFILE");
    const char *path = getenv("BILICOMICS_LIFECYCLE_DIR");
    const char *generation = getenv("BILICOMICS_LAUNCH_GENERATION");
    const char *nonce = getenv("BILICOMICS_LAUNCH_TOKEN");
    unsigned long long generation_value, resume_sequence = 0;
    if (argc != 6 || strcmp(argv[1], "--publish-native-portrait-orientation")
            || strcmp(argv[3], "--kindle") || strcmp(argv[4], "--device") || !approved || strcmp(argv[5], approved)
            || !path || path[0] != '/' || !generation || positive_sequence(generation, &generation_value)
            || !nonce || strlen(nonce) != 32) return 78;
    for (int index = 0; index < 32; ++index) {
        if (!((nonce[index] >= '0' && nonce[index] <= '9') || (nonce[index] >= 'a' && nonce[index] <= 'f'))) return 78;
    }
    const char *payload = argv[2];
    size_t length = strlen(payload);
    if (!strncmp(payload, "CALIBRATED:", 11)) {
        if (positive_sequence(payload + 11, &resume_sequence)) return 78;
    } else if (!((length == 3 && (payload[0] == 'U' || payload[0] == 'D') && payload[1] == ':')
                 || (length == 7 && !strncmp(payload, "FIXED:", 6)))
            || payload[length - 1] < '0' || payload[length - 1] > '3') return 78;
    int directory = open(path, O_RDONLY | O_CLOEXEC | O_DIRECTORY | O_NOFOLLOW);
    if (directory < 0) return 78;
    int status = 78, pending_descriptor = -1;
    struct stat directory_pin, pending_pin;
    struct publisher_process master_pin, reader_pin, chain[12];
    size_t chain_count = 0;
    char value[256], permission[128], permission_copy[128];
    unsigned long long master, reader, read_sequence;
    if (fstat(directory, &directory_pin) || !S_ISDIR(directory_pin.st_mode)
            || directory_pin.st_uid != geteuid() || (directory_pin.st_mode & 0777) != 0700
            || publisher_marker(directory, "pid", value, sizeof(value)) || positive_sequence(value, &master)
            || master > 2147483647ULL || publisher_process((long)master, &master_pin)
            || publisher_marker(directory, "native-read-go", permission, sizeof(permission))) goto finished;
    strcpy(permission_copy, permission);
    char *first = strchr(permission_copy, ':');
    char *second = first ? strchr(first + 1, ':') : NULL;
    if (!first || !second || strchr(second + 1, ':') || strcmp(second + 1, nonce)) goto finished;
    *first = 0;
    *second = 0;
    if (positive_sequence(permission_copy, &reader) || reader > 2147483647ULL
            || positive_sequence(first + 1, &read_sequence) || publisher_process((long)reader, &reader_pin)
            || reader_pin.parent != (long)master || reader_pin.group != (long)reader || reader_pin.session != (long)reader
            || getpgid(0) != (pid_t)reader || getsid(0) != (pid_t)reader) goto finished;
    long member = (long)getpid();
    while (chain_count < sizeof(chain) / sizeof(chain[0])) {
        if (publisher_process(member, &chain[chain_count]) || chain[chain_count].group != (long)reader
                || chain[chain_count].session != (long)reader) goto finished;
        ++chain_count;
        if (member == (long)reader) break;
        member = chain[chain_count - 1].parent;
    }
    if (!chain_count || chain[chain_count - 1].pid != (long)reader
            || publisher_binding(directory, generation, nonce, permission, resume_sequence, master)) goto finished;
    const char *leaf = resume_sequence ? "resume-native-portrait-orientation" : "native-portrait-orientation";
    const char *pending = resume_sequence ? "resume-native-portrait-orientation.pending" : "native-portrait-orientation.pending";
    if (!publisher_absent(directory, leaf)) goto finished;
    pending_descriptor = openat(directory, pending, O_WRONLY | O_CLOEXEC | O_CREAT | O_EXCL | O_NOFOLLOW, 0600);
    if (pending_descriptor < 0) goto finished;
    if (fstat(pending_descriptor, &pending_pin) || !S_ISREG(pending_pin.st_mode)
            || pending_pin.st_uid != geteuid() || (pending_pin.st_mode & 0077) || pending_pin.st_nlink != 1) goto finished;
    int bytes = resume_sequence ? snprintf(value, sizeof(value), "%s:%s:%llu:CALIBRATED\n", generation, nonce, resume_sequence)
            : snprintf(value, sizeof(value), "%s:%s:%s\n", generation, nonce, payload);
    if (bytes <= 0 || bytes >= (int)sizeof(value) || write(pending_descriptor, value, (size_t)bytes) != bytes
            || fsync(pending_descriptor) || publisher_binding(directory, generation, nonce, permission, resume_sequence, master)
            || !same_publisher_process(&master_pin) || !same_publisher_process(&reader_pin)) goto finished;
    for (size_t index = 0; index < chain_count; ++index) {
        if (!same_publisher_process(&chain[index])) goto finished;
    }
    if (!publisher_object_matches(directory, pending, &pending_pin)
            || !publisher_directory_matches(path, &directory_pin)
            || syscall(SYS_renameat2, directory, pending, directory, leaf, RENAME_NOREPLACE)
            || fsync(directory) || publisher_binding(directory, generation, nonce, permission, resume_sequence, master)
            || !publisher_object_matches(directory, leaf, &pending_pin)
            || !publisher_directory_matches(path, &directory_pin)
            || !same_publisher_process(&master_pin)) goto finished;
    char committed[256];
    if (publisher_marker(directory, leaf, committed, sizeof(committed)) || strlen(committed) + 1 != (size_t)bytes
            || strncmp(committed, value, strlen(committed))) goto finished;
    record("portrait-publish", argc, argv);
    status = printf("PUBLISHED\n") < 0 ? 78 : 0;
finished:
    // Preserve pending evidence; only the owning launcher may remove it after
    // the native reader group has stopped. Never unlink a named pending leaf here.
    if (pending_descriptor >= 0) close(pending_descriptor);
    close(directory);
    return status;
}
static int publish_record(const char *directory, const char *leaf, const char *record) {
    char destination[4096], pending[4096];
    if (snprintf(destination, sizeof(destination), "%s/%s", directory, leaf) >= (int)sizeof(destination)
            || snprintf(pending, sizeof(pending), "%s/%s.pending", directory, leaf) >= (int)sizeof(pending)) return -1;
    int descriptor = open(pending, O_WRONLY | O_CREAT | O_EXCL, 0600);
    if (descriptor < 0) return -1;
    FILE *stream = fdopen(descriptor, "w");
    if (!stream) { close(descriptor); return -1; }
    int failed = fprintf(stream, "%s\n", record) < 0;
    if (fclose(stream)) failed = 1;
    if (failed || rename(pending, destination)) return -1;
    return 0;
}
static int wm_is_stopped(void) {
    const char *configured = getenv("TEST_WM_PIDS");
    if (!configured || strlen(configured) >= 1024) return 0;
    char pids[1024];
    strcpy(pids, configured);
    char *context = NULL;
    char *pid = strtok_r(pids, " ", &context);
    int count = 0;
    while (pid) {
        char status[256], line[4096];
        if (snprintf(status, sizeof(status), "/proc/%s/stat", pid) >= (int)sizeof(status)) return 0;
        FILE *stream = fopen(status, "r");
        if (!stream) return 0;
        if (!fgets(line, sizeof(line), stream)) { fclose(stream); return 0; }
        if (fclose(stream)) return 0;
        const char *tail = strrchr(line, ')');
        if (!tail || tail[1] != ' ' || (tail[2] != 'T' && tail[2] != 't')) return 0;
        ++count;
        pid = strtok_r(NULL, " ", &context);
    }
    return count > 0;
}
static int append_state(const char *trace, const char *event, unsigned long sequence,
                        const char *source, const char *mode, unsigned int grayscale) {
    FILE *stream = fopen(trace, "a");
    if (!stream) return -1;
    fprintf(stream, "{\"event\":\"%s\",\"process_id\":%ld,\"sequence\":%lu,"
            "\"output_source\":\"%s\",\"native_epdc_mode\":\"%s\",\"grayscale\":%u,"
            "\"native_portrait_orientation\":\"%s\",\"framebuffer_rotation\":%u,"
            "\"native_window_manager_stopped\":%s}\n", event, (long)getpid(), sequence,
            source, mode, grayscale, portrait_orientation, portrait_rotation, wm_is_stopped() ? "true" : "false");
    return fclose(stream) ? -1 : 0;
}
static int probe_framebuffer_fixture(const char *trace, unsigned long sequence, const char **mode) {
    // This controlled input represents a fresh read of variable framebuffer
    // information. It never opens a device or reuses the startup polarity.
    const char *path = getenv("TEST_FRAMEBUFFER_STATE");
    char value[8];
    FILE *stream = path ? fopen(path, "r") : NULL;
    if (!stream) return -1;
    if (!fgets(value, sizeof(value), stream)) { fclose(stream); return -1; }
    if (fgetc(stream) != EOF) { fclose(stream); return -1; }
    if (fclose(stream)) return -1;
    unsigned int grayscale;
    if (!strcmp(value, "1\n")) { grayscale = 1; *mode = "Y8"; }
    else if (!strcmp(value, "2\n")) { grayscale = 2; *mode = "Y8INV"; }
    else return -1;
    if (!wm_is_stopped()) return -1;
    return append_state(trace, "fixture-framebuffer-probe", sequence, "framebuffer", *mode, grayscale);
}
static int acknowledge_power(const char *directory, const char *generation, const char *nonce,
                             const char *trace, const char *capability, const char *expected_action) {
    char request[256], prefix[128], action[16], extra;
    unsigned long sequence;
    if (snprintf(prefix, sizeof(prefix), "%s:%s:", generation, nonce) >= (int)sizeof(prefix)
            || read_record(directory, "lifecycle-request", request, sizeof(request))
            || strncmp(request, prefix, strlen(prefix))
            || sscanf(request + strlen(prefix), "%lu:%15s%c", &sequence, action, &extra) != 2
            || !sequence || strcmp(action, expected_action)) return -1;
    const char *source = "none", *mode = "unknown";
    unsigned int grayscale = 0;
    if (!strcmp(action, "resume")) {
        if (resolve_portrait_calibration(directory, generation, nonce, sequence)) return -1;
        char marker[256], header[128];
        if (snprintf(header, sizeof(header), "%s:%s:%lu:", generation, nonce, sequence) >= (int)sizeof(header)
                || read_record(directory, "resume-native-epdc-mode", marker, sizeof(marker))
                || strncmp(marker, header, strlen(header))) return -1;
        mode = marker + strlen(header);
        if (!strcmp(mode, "FRAMEBUFFER")) {
            if (strcmp(capability, "framebuffer8") || probe_framebuffer_fixture(trace, sequence, &mode)) return -1;
            source = "framebuffer";
            grayscale = !strcmp(mode, "Y8INV") ? 2 : 1;
        } else {
            if (strcmp(mode, "Y8") && strcmp(mode, "Y8INV")) return -1;
            source = "launcher";
        }
        if (append_state(trace, action, sequence, source, mode, grayscale)) return -1;
    } else if (append_state(trace, action, sequence, source, mode, grayscale)) return -1;
    char acknowledgement[300];
    if (snprintf(acknowledgement, sizeof(acknowledgement), "%s:ok", request) >= (int)sizeof(acknowledgement)) return -1;
    return publish_record(directory, "lifecycle-ack", acknowledgement);
}
int main(int argc, char **argv) {
    if (strstr(argv[0], "lipc-wait-event")) {
        record("event", argc, argv);
        return events();
    }
    if (argc == 2 && !strcmp(argv[1], "--relay-power-events")) {
        record("relay", argc, argv);
        return relay();
    }
    if (argc > 1 && !strcmp(argv[1], "--probe-native-portrait-framebuffer")) return portrait_helper(argc, argv);
    if (argc > 1 && !strcmp(argv[1], "--capture-native-display")) return native_capture(argc, argv);
    if (argc > 1 && !strcmp(argv[1], "--publish-native-portrait-orientation")) return portrait_publisher(argc, argv);
    record("application", argc, argv);
    const char *trace = NULL;
    for (int index = 1; index + 1 < argc; ++index) {
        if (!strcmp(argv[index], "--lifecycle-trace")) trace = argv[index + 1];
    }
    const char *directory = getenv("BILICOMICS_LIFECYCLE_DIR");
    const char *generation = getenv("BILICOMICS_LAUNCH_GENERATION");
    const char *nonce = getenv("BILICOMICS_LAUNCH_TOKEN");
    const char *capability = getenv("TEST_OUTPUT_CAPABILITY");
    if (!trace || !directory || !generation || !nonce || !capability
            || (strcmp(capability, "framebuffer8") && strcmp(capability, "legacy"))) return 93;
    if (resolve_portrait_calibration(directory, generation, nonce, 0)) return 94;
    signal(SIGTERM, stop);
    signal(SIGHUP, stop);
    signal(SIGINT, stop);
    signal(SIGUSR1, power_signal);
    signal(SIGUSR2, power_signal);
    char record_value[128];
    if (!strcmp(capability, "framebuffer8")) {
        const char *startup_mode;
        if (probe_framebuffer_fixture(trace, 0, &startup_mode)) return 94;
    }
    if (snprintf(record_value, sizeof(record_value), "%s:%s:%ld:%s", generation, nonce,
                 (long)getpid(), capability) >= (int)sizeof(record_value)
            || publish_record(directory, "native-output-capability", record_value)) return 95;
    if (snprintf(record_value, sizeof(record_value), "%s:%s", generation, nonce) >= (int)sizeof(record_value)
            || publish_record(directory, "lifecycle-ready", record_value)
            || append_state(trace, "ready", 0, capability, "unknown", 0)) return 96;
    const struct timespec delay = {0, 10000000};
    while (!stopping) {
        if (suspend_pending) {
            suspend_pending = 0;
            if (acknowledge_power(directory, generation, nonce, trace, capability, "suspend")) return 97;
        }
        if (resume_pending) {
            resume_pending = 0;
            if (acknowledge_power(directory, generation, nonce, trace, capability, "resume")) return 98;
        }
        nanosleep(&delay, NULL);
    }
    return 0;
}
'''


def identity(pid):
    """Return a real process birth identity without trusting a PID file alone."""
    try:
        fields = Path(f"/proc/{int(pid)}/stat").read_text().rsplit(") ", 1)[1].split()
        return {"pid": int(pid), "birth": fields[19], "parent": int(fields[1]),
                "group": int(fields[2]), "session": int(fields[3]), "state": fields[0],
                "exe": str(Path(f"/proc/{int(pid)}/exe").resolve(strict=True))}
    except (OSError, ValueError, IndexError):
        return None


def same_live_process(record):
    current = identity(record["pid"])
    return current and current["birth"] == record["birth"] and current["exe"] == record["exe"] \
        and current["state"] not in ("Z", "X")


class PhysicalWrapperRecordTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        if sys.platform != "linux" or os.environ.get("BILICOMICS_REMOTE_VERIFICATION") != "1":
            raise RuntimeError("Run physical harness fixtures only through ssh test-env")

    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="bilicomics-physical-record-test-")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        source = (HELPERS / "physical-wrapper.sh").read_text()
        definitions = source[source.index("physical_wrapper_cr="):source.index("read_record()")]
        start = source.index("read_exact_record() {")
        definitions += source[start:source.index("\n}\n", start) + 3]
        self.record_script = self.root / "read-record.sh"
        self.record_script.write_text('#!/bin/sh\nset -eu\n' + definitions + 'read_exact_record "$@"\n')
        self.record = self.root / "record"
        self.inode_script = self.root / "lock-inode.sh"
        definition = next(line for line in source.splitlines() if line.startswith("lock_inode()"))
        self.inode_script.write_text('#!/bin/sh\nset -eu\n' + definition + '\nlock=$1\nlock_inode\n')

    def read_record(self, budget=256):
        return subprocess.run(["sh", str(self.record_script), str(self.record), str(budget)],
                              stdin=subprocess.DEVNULL, capture_output=True, timeout=5)

    def test_exact_record_preserves_canonical_bytes_at_both_limits(self):
        for budget in (256, 512):
            for content in (b"\n", b"123:456:abcdef\n", b"value with spaces\tand\\backslash\n",
                            b"x" * (budget - 1) + b"\n"):
                with self.subTest(budget=budget, content_length=len(content)):
                    self.record.write_bytes(content)
                    result = self.read_record(budget)
                    self.assertEqual(result.returncode, 0, result.stderr)
                    self.assertEqual(result.stdout, content)

    def test_exact_record_rejects_noncanonical_bytes_and_overflow(self):
        malformed = (b"", b"unterminated", b"record\r\n", b"record\r", b"record\n\n",
                     b"record\nextra\n", b"record\nextra", b"rec\x00ord\n", b"\x00\n",
                     b"record\n\x00", b"record\x00\n")
        for budget in (256, 512):
            for content in (*malformed, b"x" * budget + b"\n", b"x" * (budget + 1),
                            b"record\n" + b"x" * (budget * 4)):
                with self.subTest(budget=budget, content=content[:40], content_length=len(content)):
                    self.record.write_bytes(content)
                    result = self.read_record(budget)
                    self.assertNotEqual(result.returncode, 0)
                    self.assertEqual(result.stdout, b"", "A rejected record published partial data")
        self.record.write_bytes(b"valid\n")
        for budget in (0, 255, 257, 513, -1, "invalid"):
            with self.subTest(budget=budget):
                self.assertNotEqual(self.read_record(budget).returncode, 0)

    def test_exact_record_rejects_missing_link_directory_and_fifo(self):
        self.assertNotEqual(self.read_record().returncode, 0)
        target = self.root / "target"
        target.write_bytes(b"valid\n")
        self.record.symlink_to(target)
        self.assertNotEqual(self.read_record().returncode, 0)
        self.record.unlink()
        self.record.mkdir()
        self.assertNotEqual(self.read_record().returncode, 0)
        self.record.rmdir()
        os.mkfifo(self.record, 0o600)
        self.assertNotEqual(self.read_record().returncode, 0)

    def test_lock_inode_preserves_link_identity_and_detects_replacement(self):
        directory = self.root / "lock"
        directory.mkdir()
        original_inode = directory.lstat().st_ino
        link = self.root / "link"
        link.symlink_to(directory, target_is_directory=True)
        dangling = self.root / "dangling"
        dangling.symlink_to(self.root / "absent", target_is_directory=True)
        for path in (directory, link, dangling):
            with self.subTest(path=path.name):
                result = subprocess.run(["sh", str(self.inode_script), str(path)],
                                        stdin=subprocess.DEVNULL, capture_output=True, timeout=5)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(result.stdout, f"{path.lstat().st_ino}\n".encode())
        directory.rename(self.root / "original-lock")
        directory.mkdir()
        result = subprocess.run(["sh", str(self.inode_script), str(directory)],
                                stdin=subprocess.DEVNULL, capture_output=True, timeout=5)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout, f"{directory.lstat().st_ino}\n".encode())
        self.assertNotEqual(int(result.stdout), original_inode)


class PhysicalWrapperTraceTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        if sys.platform != "linux" or os.environ.get("BILICOMICS_REMOTE_VERIFICATION") != "1":
            raise RuntimeError("Run physical harness fixtures only through ssh test-env")

    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="bilicomics-physical-trace-test-")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.logs = self.root / "data/logs"
        self.logs.mkdir(parents=True)
        self.trace = self.logs / "physical-wrapper-timing.tsv"
        self.pid = self.root / "caller-pid"
        source = (HELPERS / "physical-wrapper.sh").read_text()
        start = source.index("physical_wrapper_trace() {")
        definition = source[start:source.index("\n}\n", start) + 3]
        self.script = self.root / "trace.sh"
        self.script.write_text('#!/bin/sh\nset -eu\numask 077\n' + definition +
            'run_root=$TEST_TRACE_ROOT\nlogs=$run_root/data/logs\n'
            'printf "%s\\n" "$$" >"$TEST_TRACE_PID"\n'
            'before=$(ulimit -f)\nindex=0\n'
            'while [ "$index" -lt "$TEST_TRACE_COUNT" ]; do\n'
            '    physical_wrapper_trace "$1"\n    index=$((index + 1))\ndone\n'
            '[ "$(ulimit -f)" = "$before" ] || exit 74\nexit 73\n')

    def run_trace(self, stage="probe_entry", enabled="1", count=1, root=None):
        environment = dict(os.environ, TEST_TRACE_ROOT=str(root or self.root),
                           TEST_TRACE_PID=str(self.pid), TEST_TRACE_COUNT=str(count),
                           BILICOMICS_PHYSICAL_TOKEN="a" * 64)
        if enabled is None:
            environment.pop("BILICOMICS_LAUNCH_TRACE", None)
        else:
            environment["BILICOMICS_LAUNCH_TRACE"] = enabled
        result = subprocess.run(["sh", str(self.script), stage], env=environment,
                                stdin=subprocess.DEVNULL, capture_output=True, timeout=5)
        self.assertEqual(result.returncode, 73, "Tracing changed the caller status or file-size limit")
        self.assertEqual(result.stdout, b"", "Tracing wrote to the native protocol output")
        return result

    def test_trace_is_opt_in_and_rejects_nonfixed_stages(self):
        for enabled in (None, "0", "true"):
            with self.subTest(enabled=enabled):
                self.run_trace(enabled=enabled)
                self.assertFalse(self.trace.exists())
        for stage in ("", "unapproved", "probe_entry\nprivate-value"):
            with self.subTest(stage=stage):
                self.run_trace(stage=stage)
                self.assertFalse(self.trace.exists())

    def test_trace_records_only_fixed_stage_pid_and_uptime(self):
        stages = ("probe_entry", "capture_entry", "publish_entry", "elf_receipt_loaded", "observer_loaded",
                  "issuer_bound", "ancestry_scanned", "ancestry_rechecked", "final_checks_entry")
        expected_pids = []
        for stage in stages:
            result = self.run_trace(stage)
            self.assertEqual(result.stderr, b"")
            expected_pids.append(self.pid.read_text().strip())
        records = self.trace.read_text().splitlines()
        self.assertEqual(len(records), len(stages))
        for line, stage, pid in zip(records, stages, expected_pids):
            fields = line.split("\t")
            self.assertEqual(len(fields), 3)
            self.assertRegex(fields[0], r"^[0-9]+\.[0-9]+$")
            self.assertLessEqual(len(fields[0]), 32)
            self.assertEqual(fields[1:], [stage, f"pid={pid}"])
        self.assertEqual(self.trace.stat().st_mode & 0o777, 0o600)
        self.assertEqual(self.trace.stat().st_nlink, 1)

    def test_trace_rejects_preexisting_special_paths_without_writes(self):
        outside = self.root / "outside"
        outside.write_bytes(b"outside remains unchanged\n")
        for kind in ("symlink", "dangling", "fifo", "directory"):
            with self.subTest(kind=kind):
                if kind == "symlink":
                    self.trace.symlink_to(outside)
                elif kind == "dangling":
                    self.trace.symlink_to(self.root / "absent")
                elif kind == "fifo":
                    os.mkfifo(self.trace, 0o600)
                else:
                    self.trace.mkdir()
                self.run_trace()
                self.assertEqual(outside.read_bytes(), b"outside remains unchanged\n")
                self.assertFalse((self.root / "absent").exists())
                if kind == "directory":
                    self.assertEqual(list(self.trace.iterdir()), [])
                    self.trace.rmdir()
                else:
                    self.trace.unlink()
        alias = self.root / "run-alias"
        alias.symlink_to(self.root, target_is_directory=True)
        self.run_trace(root=alias)
        self.assertFalse(self.trace.exists())

    def test_trace_size_limit_and_write_failure_preserve_the_caller(self):
        self.run_trace(count=800)
        content = self.trace.read_bytes()
        self.assertGreater(len(content), 0)
        self.assertLessEqual(len(content), 16 * 1024)
        self.run_trace()
        self.assertEqual(self.trace.read_bytes(), content)
        oversized = b"x" * (32 * 1024)
        self.trace.write_bytes(oversized)
        self.run_trace()
        self.assertEqual(self.trace.read_bytes(), oversized, "Tracing changed a pre-existing oversized file")


class PhysicalWatchdogLockCleanupTests(unittest.TestCase):
    COVER_LEAVES = ("sleep-cover-request", "sleep-cover-request.pending",
                    "sleep-cover-ready", "sleep-cover-ready.pending")

    def setUp(self):
        if sys.platform != "linux" or os.environ.get("BILICOMICS_REMOTE_VERIFICATION") != "1":
            raise RuntimeError("Run physical watchdog fixtures only through ssh test-env")
        temporary = tempfile.TemporaryDirectory(prefix="bilicomics-watchdog-lock-")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.root.chmod(0o700)
        self.logs = self.root / "logs"
        self.logs.mkdir(mode=0o700)
        self.nonce = secrets.token_hex(16)
        self.launcher_pid = str(os.getpid())
        source = (HELPERS / "physical-watchdog.sh").read_text()
        self.read_one = next(line for line in source.splitlines() if line.startswith("read_one() {"))
        begin = source.index("\nremove_owned_lock() {")
        self.cleanup_source = source[begin:source.index("\ncleanup() {", begin)]
        self.prepare_lock("launcher.lock")

    def prepare_lock(self, name):
        self.lock = self.root / name
        self.lock.mkdir(mode=0o700)
        self.expected_inode = self.lock.stat().st_ino
        self.contents = {"pid": self.launcher_pid + "\n", "nonce": self.nonce + "\n",
                         "generation": f"1:{self.nonce}\n"}
        self.contents.update({leaf: f"1:{self.nonce}:7:suspend\n" for leaf in self.COVER_LEAVES})
        for leaf, content in self.contents.items():
            path = self.lock / leaf
            path.write_text(content)
            path.chmod(0o600)

    def run_cleanup(self):
        # Execute the actual watchdog function with its recorded directory
        # identity, without starting a launcher or replacing native services.
        script = "set -eu\numask 077\n" + self.read_one + "\n" + self.cleanup_source + "\n" + \
            'lock=$1\nlogs=$2\nlauncher_pid=$3\nowned_lock_nonce=$4\n' + \
            'owned_lock_generation=1\nowned_lock_inode=$5\nrecovery_complete=1\n' + \
            'remove_owned_lock\nprintf "%s\\n" "$recovery_complete"\n'
        result = subprocess.run(["sh", "-c", script, "watchdog-lock-fixture", str(self.lock), str(self.logs),
                                 self.launcher_pid, self.nonce, str(self.expected_inode)],
                                capture_output=True, text=True, timeout=5)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn(result.stdout.strip(), ("0", "1"))
        return result.stdout.strip()

    def assert_owned_records_unchanged(self):
        for leaf, content in self.contents.items():
            self.assertEqual((self.lock / leaf).read_text(), content,
                             "Rejected cleanup changed an owned lock record")

    def test_sleep_cover_records_are_removed_with_the_owned_lock(self):
        self.assertEqual(self.run_cleanup(), "1")
        self.assertFalse(self.lock.exists(), "Recognized cover records prevented exact lock cleanup")

    def test_unknown_lock_entry_preserves_cover_records_and_the_lock(self):
        unexpected = self.lock / "foreign-cover-record"
        unexpected.write_text("unrecognized state must remain\n")
        self.assertEqual(self.run_cleanup(), "0")
        self.assertTrue(self.lock.is_dir())
        self.assert_owned_records_unchanged()
        self.assertEqual(unexpected.read_text(), "unrecognized state must remain\n")
        self.assertIn("An unexpected lock entry requires operator review.",
                      (self.logs / "recovery.log").read_text())

    def test_cover_records_do_not_bypass_the_lock_identity(self):
        (self.lock / "generation").write_text(f"2:{self.nonce}\n")
        self.contents["generation"] = f"2:{self.nonce}\n"
        self.assertEqual(self.run_cleanup(), "0")
        self.assertTrue(self.lock.is_dir())
        self.assert_owned_records_unchanged()
        self.assertIn("Launcher lock identity is not proven; leaving it untouched.",
                      (self.logs / "recovery.log").read_text())

    def test_sleep_cover_records_still_require_regular_nonsymlink_leaves(self):
        outside = self.root / "outside"
        outside.write_text("external state must remain\n")
        for index, leaf in enumerate(self.COVER_LEAVES):
            for kind in ("symlink", "fifo", "directory"):
                with self.subTest(leaf=leaf, kind=kind):
                    self.prepare_lock(f"hostile-{index}-{kind}")
                    bad = self.lock / leaf
                    bad.unlink()
                    if kind == "symlink":
                        bad.symlink_to(outside)
                    elif kind == "fifo":
                        os.mkfifo(bad, 0o600)
                    else:
                        bad.mkdir(mode=0o700)
                    self.assertEqual(self.run_cleanup(), "0")
                    self.assertTrue(self.lock.is_dir())
                    self.assertEqual((self.lock / "pid").read_text(), self.contents["pid"])
                    self.assertEqual(outside.read_text(), "external state must remain\n")
                    self.assertTrue(bad.is_symlink() if kind == "symlink" else bad.is_fifo() if kind == "fifo" else bad.is_dir(),
                                    "Rejected cleanup removed a hostile cover record")


class PhysicalHarnessTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        if sys.platform != "linux" or os.environ.get("BILICOMICS_REMOTE_VERIFICATION") != "1":
            raise RuntimeError("Run physical harness fixtures only through ssh test-env")
        cls.build_root = Path(tempfile.mkdtemp(prefix="bilicomics-physical-elf-fixture-"))
        cls.build_root.chmod(0o700)
        (cls.build_root / "fixture.c").write_text(ELF_FIXTURE)
        with (cls.build_root / "compiler.log").open("w") as output:
            result = subprocess.run(["cc", "-std=c11", "-O2", "-Wall", "-Wextra", "-Werror",
                                     str(cls.build_root / "fixture.c"), "-o", str(cls.build_root / "fixture")],
                                    stdout=output, stderr=subprocess.STDOUT, timeout=30)
        if result.returncode:
            raise RuntimeError(f"The controlled ELF fixture did not compile; inspect {cls.build_root}/compiler.log")

    @classmethod
    def tearDownClass(cls):
        shutil.rmtree(cls.build_root)

    def setUp(self):
        self.root = Path(tempfile.mkdtemp(prefix="bilicomics-physical-harness-test-"))
        self.root.chmod(0o700)
        self.run_root = self.root / "bilicomics-physical-fixture"
        self.logs = self.run_root / "data/logs"
        self.logs.mkdir(parents=True)
        (self.run_root / "app").mkdir()
        (self.run_root / "tools").mkdir()
        self.bin = self.root / "bin"
        self.bin.mkdir()
        self.arguments = self.root / "arguments"
        self.arguments.mkdir()
        self.lock = self.root / "launcher.lock"
        self.receipt_directory = self.root / "bilicomics-physical-fixture-elf"
        self.canonical_temporary_base = self.root / "canonical-temp"
        self.hash_trace = self.root / "elf-hashes"
        self.token = secrets.token_hex(32)
        self.watched = []
        self.log_sinks = []
        self.observer = None
        self.watchdog = None
        self.watchdog_log = None
        self.native_baseline = []
        self.completed = False
        self.addCleanup(self.cleanup_fixture)
        fixture_digest = hashlib.sha256((self.build_root / "fixture").read_bytes()).hexdigest()
        self.fixture_binary = self.run_root / "app" / ("bilicomics-" + fixture_digest)
        shutil.copyfile(self.build_root / "fixture", self.fixture_binary)
        self.fixture_binary.chmod(0o755)
        event = self.bin / "fixture-lipc-event"
        shutil.copyfile(self.build_root / "fixture", event)
        event.chmod(0o755)
        self.event_command = event
        os.mkfifo(self.root / "event-input", 0o600)
        (self.logs / "run-token").write_text(self.token + "\n")
        (self.logs / "binary-name").write_text(self.fixture_binary.name + "\n")
        (self.logs / "screen").write_text("reader\n")
        (self.logs / "exit-after-ms").write_text("60000\n")
        (self.run_root / "data/device-profile").write_text("scribe\n")
        (self.run_root / "data/device-allowlist").write_text("scribe 5.19.5\n")
        (self.root / "firmware").write_text("Kindle 5.19.5 (isolated physical harness fixture)\n")
        (self.root / "statusbar-state").write_text("start/running, process 123\n")
        (self.root / "framebuffer-state").write_text("1\n")
        (self.root / "portrait-metadata").write_text("FIXED:3\n")
        (self.root / "portrait-rotation").write_text("3\n")
        (self.root / "portrait-direction").write_text("U\n")
        self.environment = dict(os.environ,
            PATH=str(self.bin) + ":/usr/bin:/bin", BILICOMICS_HOME=str(self.run_root),
            BILICOMICS_PHYSICAL_TOKEN=self.token, TEST_STATE=str(self.root / "statusbar-state"),
            TEST_EXECUTIONS=str(self.root / "executions"), TEST_ARGUMENT_DIRECTORY=str(self.arguments),
            TEST_EVENT_INPUT=str(self.root / "event-input"), TEST_SERVICE_TRACE=str(self.root / "services"),
            TEST_OUTPUT_CAPABILITY="framebuffer8", TEST_FRAMEBUFFER_STATE=str(self.root / "framebuffer-state"),
            TEST_PORTRAIT_METADATA=str(self.root / "portrait-metadata"),
            TEST_PORTRAIT_ROTATION=str(self.root / "portrait-rotation"),
            TEST_PORTRAIT_DIRECTION=str(self.root / "portrait-direction"),
            TEST_NATIVE_QUERY_TRACE=str(self.root / "native-queries"),
            TEST_NATIVE_CAPTURE_SAMPLES=str(self.root / "native-capture-samples"),
            TEST_FORBID_NATIVE_MODE_QUERY=str(self.root / "forbid-native-mode-query"),
            TEST_ELF_HASH_TRACE=str(self.hash_trace), TEST_KNOWN_TMP=str(self.root),
            TEST_TMP_ALIAS=str(self.root / "temporary-base-alias"))
        for stopped in (False, True):
            child = subprocess.Popen(["sleep", "3600"], start_new_session=True)
            record = identity(child.pid)
            self.assertTrue(record is not None, "A controlled native baseline process has no birth identity")
            self.native_baseline.append((child, record, stopped))
            if stopped:
                os.kill(child.pid, signal.SIGSTOP)
        self.environment["TEST_WM_PIDS"] = " ".join(str(child.pid) for child, _, _ in self.native_baseline)
        self.write_script(self.bin / "status", 'printf "statusbar %s\\n" "$(cat "$TEST_STATE")"')
        self.write_script(self.bin / "start", 'printf "start/running, process 123\\n" >"$TEST_STATE"\nprintf "start:%s\\n" "$*" >>"$TEST_SERVICE_TRACE"')
        self.write_script(self.bin / "stop", 'printf "stop/waiting\\n" >"$TEST_STATE"\nprintf "stop:%s\\n" "$*" >>"$TEST_SERVICE_TRACE"')
        self.write_script(self.bin / "pidof", '[ "$*" = awesome ]\nprintf "%s\\n" "$TEST_WM_PIDS"')
        self.write_script(self.bin / "sha256sum", 'printf "%s\\n" "$*" >>"$TEST_ELF_HASH_TRACE"\nexec /usr/bin/sha256sum "$@"')
        self.write_script(self.bin / "readlink", 'if [ "$#" -eq 2 ] && [ "$1" = -f ] '
                          '&& [ "$2" = "$TEST_KNOWN_TMP" ] && [ -f "$TEST_TMP_ALIAS" ]; then\n'
                          '    exec /bin/cat "$TEST_TMP_ALIAS"\nfi\nexec /usr/bin/readlink "$@"')
        self.write_script(self.bin / "xrefresh", 'printf "refresh\\n" >>"$TEST_SERVICE_TRACE"')
        self.write_script(self.bin / "lipc-get-prop",
            'case "$*" in\n'
            '  "-s com.lab126.winmgr epdcMode")\n'
            '    printf "epdcMode\\n" >>"$TEST_NATIVE_QUERY_TRACE"\n'
            '    [ ! -e "$TEST_FORBID_NATIVE_MODE_QUERY" ] || exit 96\n'
            '    printf "Y8INV\\n" ;;\n'
            '  "com.lab126.winmgr accelerometer")\n'
            '    printf "accelerometer\\n" >>"$TEST_NATIVE_QUERY_TRACE"\n'
            '    cat "$TEST_PORTRAIT_DIRECTION" ;;\n'
            '  "-s com.lab126.powerd state")\n'
            '    printf "power-state\\n" >>"$TEST_NATIVE_QUERY_TRACE"\n'
            '    printf "active\\n" ;;\n'
            '  *) exit 96 ;;\n'
            'esac')
        self.write_script(self.bin / "lipc-set-prop", 'printf "unexpected-global-write\\n" >>"$TEST_SERVICE_TRACE"\nexit 95')
        self.write_script(self.bin / "get-dynconf-value", 'printf "unexpected-persisted-read\\n" >>"$TEST_SERVICE_TRACE"\nexit 95')
        (self.bin / "lipc-wait-event").symlink_to(event)
        usleep = self.bin / "usleep"
        usleep.write_text("#!/usr/bin/python3\nimport sys, time\ntime.sleep(int(sys.argv[1]) / 1_000_000)\n")
        usleep.chmod(0o755)
        for source, destination in ((HELPERS / "physical-wrapper.sh", self.run_root / "app/bilicomics"),
                                    (HELPERS / "physical-event-wrapper.sh", self.run_root / "tools/lipc-wait-event"),
                                    (HELPERS / "physical-watchdog.sh", self.run_root / "physical-watchdog.sh"),
                                    (REPOSITORY / "packaging/kindle/launch.sh", self.run_root / "launch.sh")):
            # Expand original source paths once; generated fixture paths may
            # themselves contain a prefix used by another source replacement.
            text = source.read_text().replace("/mnt/us/bilicomics-physical-", "@PHYSICAL_RUN_PREFIX@")
            text = text.replace("/tmp/bilicomics-launcher.lock", "@PHYSICAL_LOCK_PATH@")
            text = text.replace("/tmp/bilicomics-physical-", "@PHYSICAL_RECEIPT_PREFIX@")
            text = text.replace("readlink -f /tmp", "readlink -f @PHYSICAL_TEMP_BASE@")
            text = text.replace('case "$elf_resolved_temp" in /tmp|/var/tmp)',
                                'case "$elf_resolved_temp" in @PHYSICAL_ALLOWED_TEMP_BASES@)')
            text = text.replace("@PHYSICAL_RUN_PREFIX@", str(self.root) + "/bilicomics-physical-")
            text = text.replace("@PHYSICAL_LOCK_PATH@", str(self.lock))
            text = text.replace("@PHYSICAL_RECEIPT_PREFIX@", str(self.root) + "/receipt-")
            text = text.replace("@PHYSICAL_TEMP_BASE@", str(self.root))
            text = text.replace("@PHYSICAL_ALLOWED_TEMP_BASES@", "/tmp|/var/tmp|" + str(self.root)
                                + "|" + str(self.canonical_temporary_base))
            text = text.replace("/etc/prettyversion.txt", str(self.root / "firmware")).replace("/dev/fb0", "/dev/null")
            destination.write_text(text)
            destination.chmod(0o755)

    @staticmethod
    def write_script(path, body):
        path.write_text("#!/bin/sh\nset -eu\n" + body + "\n")
        path.chmod(0o755)

    def wait_for(self, description, predicate, timeout=20):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            self.remember_observer_if_owned()
            try:
                value = predicate()
            except (OSError, ValueError, IndexError, json.JSONDecodeError):
                value = None
            if value:
                return value
            if self.watchdog and self.watchdog.poll() is not None:
                self.fail(f"The fixture watchdog exited before {description}; retained evidence: {self.root}")
            time.sleep(0.03)
        self.fail(f"Timed out waiting for {description}; retained evidence: {self.root}")

    def remember_observer_if_owned(self):
        if self.observer is not None or not self.watchdog:
            return
        try:
            launcher = identity((self.lock / "pid").read_text().strip())
            generation, nonce, pid, birth = (self.lock / "observer-owned").read_text().strip().split(":")
            anchor = identity(pid)
            if (launcher and launcher["parent"] == self.watchdog.pid and anchor
                    and anchor["parent"] == launcher["pid"] and anchor["birth"] == birth
                    and anchor["group"] == anchor["session"] == anchor["pid"]
                    and (self.lock / "generation").read_text().strip() == f"{generation}:{nonce}"
                    and (self.lock / "nonce").read_text().strip() == nonce):
                self.observer = anchor
                self.watched.append(anchor)
        except (OSError, ValueError, IndexError):
            pass

    def read_identity(self, name):
        fields = (self.logs / name).read_text().strip().split("|")
        self.assertTrue(len(fields) == 5 and secrets.compare_digest(fields[4], self.token),
                        "A controlled identity record is malformed or has the wrong ownership token")
        record = identity(fields[0])
        self.assertTrue(record is not None and record["birth"] == fields[1] and record["exe"] == fields[2],
                        "A wrapper record does not match the live executable and birth identity")
        self.watched.append(record)
        return record

    def wrapper_records_ready(self):
        for name in ("power-observer.identity", "power-source-runner.identity", "power-event.identity",
                     "power-relay.identity", "native.identity"):
            fields = (self.logs / name).read_text().strip().split("|")
            if len(fields) != 5 or not secrets.compare_digest(fields[4], self.token):
                return False
            current = identity(fields[0])
            if not current or current["birth"] != fields[1] or current["exe"] != fields[2]:
                return False
        return True

    def start_fixture(self):
        self.watchdog_log = (self.root / "watchdog-output.log").open("w")
        self.watchdog = subprocess.Popen(["sh", str(self.run_root / "physical-watchdog.sh"), str(self.run_root), self.token, "30", "90"],
            env=self.environment, stdout=self.watchdog_log, stderr=subprocess.STDOUT, start_new_session=True)
        self.watchdog_identity = identity(self.watchdog.pid)
        self.wait_for("watchdog startup", lambda: (self.logs / "watchdog.ready").exists())
        self.wait_for("application readiness", lambda: (self.logs / "watchdog.application-ready").exists(), timeout=30)
        launcher_fields = (self.logs / "launcher.identity").read_text().strip().split("|")
        self.launcher = identity(launcher_fields[0])
        self.assertTrue(self.launcher and self.launcher["birth"] == launcher_fields[1]
                        and self.launcher["parent"] == self.watchdog.pid, "The launcher is not the watchdog's recorded direct child")
        self.watched.append(self.launcher)
        self.log_sinks = []
        for name in ("launcher", "application"):
            generation, nonce, pid, birth = (self.lock / f"log-{name}-owned").read_text().strip().split(":")
            sink = identity(pid)
            self.assertTrue(sink and sink["parent"] == self.launcher["pid"] and sink["birth"] == birth
                            and sink["group"] == sink["session"] == sink["pid"]
                            and (self.lock / "generation").read_text().strip() == f"{generation}:{nonce}"
                            and (self.lock / "nonce").read_text().strip() == nonce,
                            "A bounded log sink has no current launcher-owned process identity")
            self.assertTrue((self.lock / f"log-{name}-pipe").is_fifo(), "A bounded log input is not a private FIFO")
            self.log_sinks.append(sink)
            self.watched.append(sink)
        self.wait_for("observer and relay executable identities", self.wrapper_records_ready)
        self.application = self.read_identity("native.identity")
        self.relay = self.read_identity("power-relay.identity")
        self.source = self.read_identity("power-event.identity")
        self.source_runner = self.read_identity("power-source-runner.identity")
        self.observer = self.read_identity("power-observer.identity")
        self.assertTrue(self.observer["parent"] == self.launcher["pid"] and
                        self.observer["group"] == self.observer["session"] == self.observer["pid"],
                        "The observer is not the launcher's private session anchor")
        self.assertTrue(all(record["group"] == record["session"] == self.observer["pid"] for record in
                            (self.source, self.relay)), "Observer children escaped their private session")
        self.assertTrue(self.relay["parent"] == self.observer["pid"], "The relay is not the anchor's direct child")
        self.assertTrue(self.source["parent"] == self.source_runner["pid"] and
                        self.source_runner["parent"] == self.observer["pid"],
                        "The event source did not preserve the anchor-to-source-runner parent chain")
        self.assertTrue(self.source["exe"] == str(self.event_command), "The event wrapper did not exec the fixed source ELF")
        capability = (self.lock / "native-output-capability").read_text().strip()
        expected_capability = ((self.lock / "generation").read_text().strip()
                               + f":{self.application['pid']}:" + self.environment["TEST_OUTPUT_CAPABILITY"])
        self.assertTrue(capability == expected_capability,
                        "The startup output capability is not bound to the current launch and application")
        self.assertFalse((self.lock / "native-output-capability.pending").exists(),
                         "Startup left an unpublished output capability")
        return self

    def assert_wrapper_trace(self, pid, entry):
        records = [line.split("\t") for line in (self.logs / "physical-wrapper-timing.tsv").read_text().splitlines()]
        self.assertTrue(all(len(record) == 3 for record in records))
        self.assertNotIn(f"pid={self.application['pid']}", [record[2] for record in records])
        stages = [record[1] for record in records if record[2] == f"pid={pid}"]
        self.assertEqual(stages, [entry, "elf_receipt_loaded", "observer_loaded", "issuer_bound",
                                  "ancestry_scanned", "ancestry_rechecked", "final_checks_entry"])

    def trace_values(self, event):
        lines = (self.logs / "lifecycle.jsonl").read_bytes().splitlines(keepends=True)
        values = [json.loads(line) for line in lines if line.endswith(b"\n")]
        return [value for value in values if value.get("event") == event
                and value.get("process_id") == self.application["pid"]]

    def emit_events(self, *events):
        descriptor = os.open(self.root / "event-input", os.O_WRONLY | os.O_NONBLOCK)
        try:
            for event in events:
                record = (event + " 0\n").encode("ascii")
                self.assertEqual(os.write(descriptor, record), len(record),
                                 "A controlled source event was not written as one complete record")
        finally:
            os.close(descriptor)

    def suspend_fixture(self):
        self.emit_events("goingToScreenSaver")

        def suspended_and_released():
            active_native = identity(self.native_baseline[0][0].pid)
            return (self.trace_values("suspend") and active_native
                    and active_native["state"] not in ("T", "t")
                    and "start/running" in (self.root / "statusbar-state").read_text())

        self.wait_for("same-process suspend acknowledgement and native release", suspended_and_released)
        self.assertTrue(same_live_process(self.application), "Sleep replaced the controlled application")

    def resume_fixture(self):
        self.emit_events("outOfScreenSaver", "exitingScreenSaver")

        def resumed_and_acknowledged():
            values = self.trace_values("resume")
            acknowledgement = (self.lock / "lifecycle-ack").read_text().strip()
            return values[-1] if values and acknowledgement.endswith(f":{values[-1]['sequence']}:resume:ok") else None

        value = self.wait_for("same-process resume acknowledgement", resumed_and_acknowledged)
        self.assertTrue(same_live_process(self.application), "Resume replaced the controlled application")
        return value

    def finish_fixture(self, *, kill_master=False, expected_recovery=True, expected_lock_removed=True):
        if kill_master:
            # Give the independent watchdog a sampling interval after all live
            # identities have been established, then kill only that same birth.
            time.sleep(1.2)
            self.assertTrue(same_live_process(self.launcher), "The launcher changed before the crash injection")
            os.kill(self.launcher["pid"], signal.SIGKILL)
        else:
            (self.logs / "stop.request").write_text(self.token + "\n")
        deadline = time.monotonic() + 40
        while not (self.logs / "watchdog.done").exists() and time.monotonic() < deadline:
            time.sleep(0.05)
        self.assertTrue((self.logs / "watchdog.done").exists(), f"The independent cleanup did not finish; retained evidence: {self.root}")
        done = (self.logs / "watchdog.done").read_text().strip().split("|")
        self.assertTrue(len(done) == 4 and secrets.compare_digest(done[0], self.token), "The completion record has the wrong owner")
        self.assertEqual(done[2], "1" if expected_recovery else "0",
                         f"The watchdog recovery result differs from the expected ownership outcome: {self.root}")
        try:
            self.watchdog.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self.fail(f"The completed watchdog did not exit; retained evidence: {self.root}")
        if expected_recovery:
            self.assertEqual(self.watchdog.returncode, 0, "The watchdog returned a failure after its completion record")
        else:
            self.assertNotEqual(self.watchdog.returncode, 0, "Incomplete recovery was reported as successful exit")
        if expected_lock_removed:
            for leaf in ("native-output-capability", "native-output-capability.pending",
                         "native-portrait-orientation", "native-portrait-orientation.pending",
                         "resume-native-portrait-orientation", "resume-native-portrait-orientation.pending"):
                self.assertFalse((self.lock / leaf).exists(), "Cleanup retained an owned presentation leaf")
            self.assertFalse(self.lock.exists(), "The owned launcher lock was not removed")
        else:
            self.assertTrue(self.lock.exists(), "An unsafe lock entry was removed without ownership proof")
        for record in (self.application, self.source, self.source_runner, self.relay, self.observer, self.launcher):
            self.assertFalse(same_live_process(record), "An exact owned process remains live after cleanup")
        for record in self.log_sinks:
            self.assertFalse(same_live_process(record), "A bounded log sink remains live after cleanup")
        self.assertIn("start/running", (self.root / "statusbar-state").read_text())
        for child, record, stopped in self.native_baseline:
            current = identity(child.pid)
            self.assertTrue(current and current["birth"] == record["birth"], "A controlled native baseline process was replaced")
            self.assertEqual(current["state"] in ("T", "t"), stopped, "Cleanup changed the native stopped-state baseline")
        services = (self.root / "services").read_text()
        self.assertNotIn("unexpected-global-write", services)
        self.assertNotIn("unexpected-persisted-read", services)
        self.completed = True

    def test_relay_entry_preserves_native_identity_and_arguments(self):
        self.assertFalse((self.logs / "performance-trace-enabled").exists())
        self.start_fixture()
        before_pid = (self.logs / "native.pid").read_bytes()
        before_identity = (self.logs / "native.identity").read_bytes()
        self.assertTrue(self.application["pid"] != self.relay["pid"], "The relay replaced the native application identity")
        relay_args = (self.arguments / f"relay-{self.relay['pid']}").read_text().splitlines()
        self.assertEqual(relay_args, [str(self.fixture_binary), "--relay-power-events"])
        application_args = (self.arguments / f"application-{self.application['pid']}").read_text().splitlines()
        self.assertTrue(all(flag in application_args for flag in ("--kindle", "--demo", "--offline")),
                        "The actual application lost its isolated native arguments")
        self.assertNotIn("--performance-trace", application_args,
                         "A missing opt-in marker enabled performance tracing on the actual UI")
        self.assertTrue((self.logs / "native.pid").read_bytes() == before_pid and
                        (self.logs / "native.identity").read_bytes() == before_identity,
                        "Observer startup overwrote the native application record")
        self.finish_fixture()

    def test_performance_trace_opt_in_only_changes_the_actual_ui_arguments(self):
        (self.logs / "performance-trace-enabled").write_text("1\n")
        self.start_fixture()
        application_args = (self.arguments / f"application-{self.application['pid']}").read_text().splitlines()
        self.assertEqual(application_args.count("--performance-trace"), 1)
        option_index = application_args.index("--performance-trace")
        self.assertEqual(application_args[option_index:option_index + 2],
                         ["--performance-trace", str(self.logs / "performance.jsonl")])
        self.assertTrue(all(flag in application_args for flag in ("--kindle", "--demo", "--offline")),
                        "Performance tracing changed the isolated UI arguments")
        entries = (self.root / "executions").read_text().splitlines()
        expected_helpers = {
            "relay": [str(self.fixture_binary), "--relay-power-events"],
            "native-capture": [str(self.fixture_binary), "--capture-native-display",
                               "--kindle", "--device", "scribe"],
            "portrait-publish": [str(self.fixture_binary), "--publish-native-portrait-orientation",
                                 "FIXED:3", "--kindle", "--device", "scribe"],
        }
        for role, expected in expected_helpers.items():
            with self.subTest(role=role):
                pids = [int(entry.split(":", 1)[1]) for entry in entries if entry.startswith(role + ":")]
                self.assertEqual(len(pids), 1, "Startup did not execute exactly one typed helper")
                arguments = (self.arguments / f"{role}-{pids[0]}").read_text().splitlines()
                self.assertEqual(arguments, expected,
                                 "UI performance tracing altered a native helper's exact arguments")
        self.finish_fixture()

    def test_native_capture_preserves_ui_identity_and_exact_internal_arguments(self):
        self.start_fixture()
        entries = (self.root / "executions").read_text().splitlines()
        helpers = [int(entry.split(":", 1)[1]) for entry in entries if entry.startswith("native-capture:")]
        applications = [entry for entry in entries if entry.startswith("application:")]
        self.assertEqual(len(helpers), 1, "Startup did not perform exactly one native capture invocation")
        self.assertFalse(any(entry.startswith("portrait-probe:") for entry in entries),
                         "Startup still launched the old per-observation helper")
        self.assertEqual((self.root / "native-capture-samples").read_text().splitlines(),
                         [f"{helpers[0]}:FIXED:3"] * 2,
                         "The single capture did not compare two read-only framebuffer observations")
        self.assertEqual((self.root / "native-queries").read_text().splitlines(),
                         ["power-state", "epdcMode", "power-state"])
        self.assertEqual(applications, [f"application:{self.application['pid']}"],
                         "A capture helper was launched as a demonstration UI")
        for pid in helpers:
            arguments = (self.arguments / f"native-capture-{pid}").read_text().splitlines()
            self.assertEqual(arguments, [str(self.fixture_binary), "--capture-native-display",
                                         "--kindle", "--device", "scribe"])
            self.assertNotEqual(pid, self.application["pid"], "A helper replaced the application process")
            self.assert_wrapper_trace(pid, "capture_entry")
        self.assertEqual((self.logs / "native.pid").read_text().strip(), str(self.application["pid"]))
        self.finish_fixture()

    def test_legacy_portrait_probe_remains_a_typed_fixture_primitive(self):
        environment = dict(self.environment, BILICOMICS_APPROVED_PROFILE="scribe")
        for value in ("AUTO:0\n", "AUTO:3\n", "FIXED:1\n", "FIXED:2\n"):
            with self.subTest(value=value):
                (self.root / "portrait-metadata").write_text(value)
                result = subprocess.run([str(self.fixture_binary), "--probe-native-portrait-framebuffer",
                                         "--kindle", "--device", "scribe"],
                    env=environment, stdin=subprocess.DEVNULL, capture_output=True, timeout=5)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(result.stdout, value.encode())
        before = (self.root / "executions").read_bytes()
        for value in ("AUTO:4\n", "AUTO:01\n", "FIXED:3", "FIXED:3\nextra\n", "AUTO:3\x00\n"):
            with self.subTest(value=value):
                (self.root / "portrait-metadata").write_text(value)
                result = subprocess.run([str(self.fixture_binary), "--probe-native-portrait-framebuffer",
                                         "--kindle", "--device", "scribe"],
                    env=environment, stdin=subprocess.DEVNULL, capture_output=True, timeout=5)
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(result.stdout, b"")
                self.assertEqual((self.root / "executions").read_bytes(), before)
        self.assertFalse((self.root / "native-capture-samples").exists())
        self.assertFalse((self.logs / "native.identity").exists())
        self.completed = True

    def test_busybox_checksum_dispatch_uses_known_image_and_preserves_bound(self):
        source = (HELPERS / "physical-watchdog.sh").read_text()
        start = source.index("    elf_checksum_command=$(command -v sha256sum")
        end = source.index('    [ "$(cat "$command_output"', start)
        dispatch = source[start:end]
        self.assertEqual(dispatch.count("/bin/busybox|/usr/bin/busybox"), 1)
        known_image = str(self.fixture_binary)
        dispatch = dispatch.replace("/bin/busybox|/usr/bin/busybox", shlex.quote(known_image))
        recorded = self.root / "checksum-dispatch-arguments"
        script = self.root / "checksum-dispatch.sh"
        # Exercise the source selection and bound, without executing a SHA applet.
        self.write_script(script,
            'sha256sum() { return 97; }\n'
            'proc_exe() { printf "%s\\n" "$TEST_SELF_IMAGE"; }\n'
            'bounded_command() { printf "%s\\n" "$@" >"$TEST_DISPATCH_ARGUMENTS"; }\n'
            'dispatch() {\n    elf_binary_path=$TEST_SELECTED_ELF\n' + dispatch + '}\ndispatch')
        for self_image, expected_status in ((known_image, 0), (str(self.event_command), 1)):
            recorded.unlink(missing_ok=True)
            environment = dict(self.environment, TEST_SELF_IMAGE=self_image,
                               TEST_SELECTED_ELF=known_image, TEST_DISPATCH_ARGUMENTS=str(recorded))
            result = subprocess.run(["sh", str(script)], env=environment, capture_output=True,
                                    text=True, timeout=5)
            self.assertEqual(result.returncode, expected_status, result.stderr)
            if expected_status == 0:
                self.assertEqual(recorded.read_text().splitlines(),
                                 ["10", known_image, "sha256sum", known_image])
            else:
                self.assertFalse(recorded.exists(), "An unrecognized image reached bounded execution")
        self.completed = True

    def test_verified_elf_receipt_is_reused_only_for_native_helpers(self):
        self.start_fixture()
        receipt = self.receipt_directory / "verified-elf.receipt"
        fields = receipt.read_text().rstrip("\n").split("|")
        self.assertEqual(fields[:4], ["v1", "fixture", self.token, self.fixture_binary.name])
        self.assertEqual(fields[4:6], [str(self.watchdog.pid), self.watchdog_identity["birth"]])
        self.assertEqual(receipt.stat().st_mode & 0o777, 0o600)
        self.assertEqual(receipt.stat().st_nlink, 1)
        self.assertEqual(self.hash_trace.read_text().splitlines(), [str(self.fixture_binary)] * 3,
                         "Native helpers repeated the full ELF hash inside the production read budget")
        before_identity = (self.logs / "native.identity").read_bytes()
        self.suspend_fixture()
        self.resume_fixture()
        self.assertEqual(self.hash_trace.read_text().splitlines(), [str(self.fixture_binary)] * 3,
                         "The sequence-bound publisher repeated the full ELF hash")
        self.assertEqual((self.logs / "native.identity").read_bytes(), before_identity,
                         "Receipt reuse entered or replaced the UI identity")
        self.finish_fixture()
        self.assertFalse(self.receipt_directory.exists(), "The stopped run retained its owned receipt")

    def test_verified_elf_receipt_resolves_only_the_known_temporary_base_alias(self):
        self.canonical_temporary_base.mkdir(mode=0o700)
        (self.root / "temporary-base-alias").write_text(str(self.canonical_temporary_base) + "\n")
        self.receipt_directory = self.canonical_temporary_base / "bilicomics-physical-fixture-elf"
        self.start_fixture()
        receipt = self.receipt_directory / "verified-elf.receipt"
        self.assertTrue(receipt.is_file(), "The temporary-base alias did not reach the canonical receipt")
        self.assertFalse((self.root / "bilicomics-physical-fixture-elf").exists(),
                         "The watchdog used a different receipt base than the helpers")
        self.assertEqual(receipt.stat().st_mode & 0o777, 0o600)
        self.assertEqual(self.receipt_directory.stat().st_mode & 0o777, 0o700)
        self.finish_fixture()
        self.assertFalse(self.receipt_directory.exists(), "Cleanup lost the canonical receipt inode")

    def assert_verified_elf_identity_change_is_rejected(self, capability):
        self.environment["TEST_OUTPUT_CAPABILITY"] = capability
        self.start_fixture()
        self.suspend_fixture()
        before = (self.root / "executions").read_text().splitlines()
        linked = self.root / "extra-elf-link"
        os.link(self.fixture_binary, linked)
        self.fixture_binary.chmod(0o750)
        self.assertEqual(hashlib.sha256(self.fixture_binary.read_bytes()).hexdigest(),
                         self.fixture_binary.name.removeprefix("bilicomics-"),
                         "The identity-change fixture must preserve the selected ELF bytes")
        self.emit_events("outOfScreenSaver", "exitingScreenSaver")
        self.wait_for("rejection of changed verified ELF identity", lambda: not same_live_process(self.application))
        self.assertEqual(self.trace_values("resume"), [])
        self.assertEqual([entry for entry in (self.root / "executions").read_text().splitlines()
                          if entry.startswith(("native-capture:", "portrait-publish:"))],
                         [entry for entry in before if entry.startswith(("native-capture:", "portrait-publish:"))],
                         "A changed file identity reused the validated native helper receipt")
        linked.unlink()
        self.fixture_binary.chmod(0o755)
        self.finish_fixture()

    def test_verified_elf_receipt_rejects_changed_metadata_and_link_identity(self):
        self.assert_verified_elf_identity_change_is_rejected("framebuffer8")

    def test_capture_receipt_rejects_changed_metadata_and_link_identity(self):
        self.assert_verified_elf_identity_change_is_rejected("legacy")

    def test_verified_elf_receipt_rejects_and_preserves_a_foreign_run(self):
        self.start_fixture()
        self.suspend_fixture()
        receipt = self.receipt_directory / "verified-elf.receipt"
        fields = receipt.read_text().rstrip("\n").split("|")
        fields[2] = "0" * 64
        foreign = "|".join(fields) + "\n"
        receipt.write_text(foreign)
        self.emit_events("outOfScreenSaver", "exitingScreenSaver")
        self.wait_for("rejection of a foreign verified ELF receipt", lambda: not same_live_process(self.application))
        self.assertEqual(self.trace_values("resume"), [])
        self.finish_fixture(expected_recovery=False)
        self.assertEqual(receipt.read_text(), foreign, "Recovery removed or rewrote a foreign receipt")

    def test_native_helpers_reject_unrelated_parent_and_invalid_arguments(self):
        self.start_fixture()
        before = (self.root / "executions").read_bytes()
        original_identity = (self.logs / "native.identity").read_bytes()
        header = (self.lock / "generation").read_text().strip().split(":")
        environment = dict(self.environment, BILICOMICS_APPROVED_PROFILE="scribe",
                           BILICOMICS_LIFECYCLE_DIR=str(self.lock), BILICOMICS_LAUNCH_GENERATION=header[0],
                           BILICOMICS_LAUNCH_TOKEN=header[1])
        for helper in ("--probe-native-portrait-framebuffer", "--capture-native-display"):
            for arguments in ([helper, "--kindle", "--device", "scribe"],
                              [helper, "--kindle", "--device", "pw5"],
                              [helper, "--kindle", "--device", "scribe", "--demo"],
                              ["--kindle", helper, "--device", "scribe"],
                              [helper, "--kindle", "--device", "scribe", "--relay-power-events"]):
                with self.subTest(arguments=arguments):
                    result = subprocess.run(["sh", str(self.run_root / "app/bilicomics"), *arguments],
                        env=environment, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=5)
                    self.assertNotEqual(result.returncode, 0, "An unowned or malformed helper invocation was accepted")
                    self.assertEqual((self.root / "executions").read_bytes(), before,
                                     "Rejected helper invocation executed an ELF entry point")
                    self.assertEqual((self.logs / "native.identity").read_bytes(), original_identity,
                                     "Rejected helper invocation overwrote the application identity")
        self.finish_fixture()

    def test_native_helpers_reject_a_mismatched_digest_artifact(self):
        self.start_fixture()
        before = (self.root / "executions").read_bytes()
        header = (self.lock / "generation").read_text().strip().split(":")
        environment = dict(self.environment, BILICOMICS_APPROVED_PROFILE="scribe",
                           BILICOMICS_LIFECYCLE_DIR=str(self.lock), BILICOMICS_LAUNCH_GENERATION=header[0],
                           BILICOMICS_LAUNCH_TOKEN=header[1])
        binary_marker = self.logs / "binary-name"
        original = binary_marker.read_bytes()
        mismatched = self.run_root / "app" / ("bilicomics-" + "0" * 64)
        shutil.copyfile(self.fixture_binary, mismatched)
        mismatched.chmod(0o755)
        binary_marker.write_text(mismatched.name + "\n")
        try:
            for helper in ("--probe-native-portrait-framebuffer", "--capture-native-display"):
                with self.subTest(helper=helper):
                    result = subprocess.run(["sh", str(self.run_root / "app/bilicomics"),
                                             helper, "--kindle", "--device", "scribe"],
                        env=environment, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=5)
                    self.assertNotEqual(result.returncode, 0, "A helper accepted an artifact whose digest differs from its binding")
                    self.assertEqual((self.root / "executions").read_bytes(), before,
                                     "A digest mismatch still executed the helper")
        finally:
            binary_marker.write_bytes(original)
        self.finish_fixture()

    def test_portrait_publisher_preserves_ui_identity_and_exact_internal_arguments(self):
        self.start_fixture()
        original_identity = (self.logs / "native.identity").read_bytes()
        publishers = [int(entry.split(":", 1)[1]) for entry in (self.root / "executions").read_text().splitlines()
                      if entry.startswith("portrait-publish:")]
        self.assertEqual(len(publishers), 1, "Startup did not publish exactly one calibrated orientation record")
        arguments = (self.arguments / f"portrait-publish-{publishers[0]}").read_text().splitlines()
        self.assertEqual(arguments, [str(self.fixture_binary), "--publish-native-portrait-orientation", "FIXED:3",
                                     "--kindle", "--device", "scribe"])
        self.assertNotEqual(publishers[0], self.application["pid"])
        self.suspend_fixture()
        resumed = self.resume_fixture()
        publishers = [int(entry.split(":", 1)[1]) for entry in (self.root / "executions").read_text().splitlines()
                      if entry.startswith("portrait-publish:")]
        self.assertEqual(len(publishers), 2, "Resume did not publish exactly one sequence-bound permission")
        for pid in publishers:
            self.assert_wrapper_trace(pid, "publish_entry")
        arguments = (self.arguments / f"portrait-publish-{publishers[-1]}").read_text().splitlines()
        self.assertEqual(arguments, [str(self.fixture_binary), "--publish-native-portrait-orientation",
                                     f"CALIBRATED:{resumed['sequence']}", "--kindle", "--device", "scribe"])
        self.assertEqual((self.logs / "native.identity").read_bytes(), original_identity,
                         "A publisher overwrote the live UI identity")
        self.assertEqual([entry for entry in (self.root / "executions").read_text().splitlines()
                          if entry.startswith("application:")], [f"application:{self.application['pid']}"],
                         "A publisher was launched as a demonstration UI")
        self.finish_fixture()

    def test_portrait_publisher_rejects_malformed_payloads_and_foreign_context(self):
        self.start_fixture()
        before = (self.root / "executions").read_bytes()
        original_identity = (self.logs / "native.identity").read_bytes()
        header = (self.lock / "generation").read_text().strip().split(":")
        environment = dict(self.environment, BILICOMICS_LIFECYCLE_DIR=str(self.lock),
                           BILICOMICS_LAUNCH_GENERATION=header[0], BILICOMICS_LAUNCH_TOKEN=header[1])
        for payload in ("U:4", "D:01", "fixed:3", "CALIBRATED:0", "CALIBRATED:01",
                        "CALIBRATED:18446744073709551616", "CALIBRATED:2:extra", "U:3\n"):
            with self.subTest(payload=payload):
                result = subprocess.run(["sh", str(self.run_root / "app/bilicomics"),
                                         "--publish-native-portrait-orientation", payload, "--kindle", "--device", "scribe"],
                    env=environment, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=5)
                self.assertNotEqual(result.returncode, 0, "Malformed publisher payload was accepted")
        for arguments in (["--publish-native-portrait-orientation", "FIXED:3", "--kindle", "--device", "scribe", "--demo"],
                          ["--publish-native-portrait-orientation", "FIXED:3", "--kindle", "--device", "pw5"]):
            result = subprocess.run(["sh", str(self.run_root / "app/bilicomics"), *arguments],
                env=environment, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=5)
            self.assertNotEqual(result.returncode, 0, "Malformed publisher arguments were accepted")
        foreign = dict(environment, BILICOMICS_LAUNCH_TOKEN="0" * 32)
        result = subprocess.run(["sh", str(self.run_root / "app/bilicomics"),
                                 "--publish-native-portrait-orientation", "CALIBRATED:2", "--kindle", "--device", "scribe"],
            env=foreign, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=5)
        self.assertNotEqual(result.returncode, 0, "Foreign publisher context was accepted")
        self.assertEqual((self.root / "executions").read_bytes(), before,
                         "Rejected publisher invocation executed an ELF entry point")
        self.assertEqual((self.logs / "native.identity").read_bytes(), original_identity,
                         "Rejected publisher invocation overwrote the application identity")
        self.finish_fixture()

    def test_portrait_publisher_accepts_the_previous_checkpoint_error_acknowledgement(self):
        self.start_fixture()
        self.suspend_fixture()
        request = (self.lock / "lifecycle-request").read_text().strip()
        acknowledgement = self.lock / "lifecycle-ack"
        acknowledgement.write_text(request + ":checkpoint-error\n")
        resumed = self.resume_fixture()
        self.assertGreater(resumed["sequence"], 1)
        self.finish_fixture()

    def test_portrait_publisher_accepts_the_previous_bare_acknowledgement(self):
        self.start_fixture()
        self.suspend_fixture()
        request = (self.lock / "lifecycle-request").read_text().strip()
        (self.lock / "lifecycle-ack").write_text(request + "\n")
        resumed = self.resume_fixture()
        self.assertGreater(resumed["sequence"], 1)
        self.finish_fixture()

    def test_portrait_publisher_refuses_a_pending_fifo_without_blocking(self):
        self.start_fixture()
        self.suspend_fixture()
        os.mkfifo(self.lock / "resume-native-portrait-orientation.pending", 0o600)
        before = time.monotonic()
        self.emit_events("outOfScreenSaver", "exitingScreenSaver")
        self.wait_for("bounded rejection of a pending FIFO", lambda: not same_live_process(self.application))
        self.assertLess(time.monotonic() - before, 10, "A publisher blocked on an existing pending FIFO")
        self.assertEqual(self.trace_values("resume"), [])
        self.finish_fixture()

    def test_portrait_publisher_never_follows_a_final_directory_symlink(self):
        self.start_fixture()
        self.suspend_fixture()
        destination = self.root / "outside-publisher-directory"
        destination.mkdir()
        original = destination / "unchanged.txt"
        original.write_text("fixture destination remains unchanged\n")
        (self.lock / "resume-native-portrait-orientation").symlink_to(destination, target_is_directory=True)
        self.emit_events("outOfScreenSaver", "exitingScreenSaver")
        self.wait_for("rejection of a final directory symlink", lambda: not same_live_process(self.application))
        self.assertEqual(self.trace_values("resume"), [])
        self.assertEqual(sorted(path.name for path in destination.iterdir()), ["unchanged.txt"],
                         "A publisher moved its record into the directory symlink destination")
        self.assertEqual(original.read_text(), "fixture destination remains unchanged\n")
        self.finish_fixture()

    def test_portrait_publisher_preserves_a_final_directory_and_reports_unsafe_cleanup(self):
        self.start_fixture()
        self.suspend_fixture()
        destination = self.lock / "resume-native-portrait-orientation"
        destination.mkdir()
        original = destination / "unchanged.txt"
        original.write_text("fixture directory remains unchanged\n")
        self.emit_events("outOfScreenSaver", "exitingScreenSaver")
        self.wait_for("rejection of an existing final directory", lambda: not same_live_process(self.application))
        self.assertEqual(self.trace_values("resume"), [])
        self.finish_fixture(expected_recovery=False, expected_lock_removed=False)
        self.assertEqual(original.read_text(), "fixture directory remains unchanged\n",
                         "Unsafe cleanup modified the pre-existing directory destination")

    def test_event_wrapper_rejects_an_unrelated_parent(self):
        self.start_fixture()
        before = (self.root / "executions").read_text().splitlines()
        original_identity = (self.logs / "power-event.identity").read_bytes()
        result = subprocess.run(["sh", str(self.run_root / "tools/lipc-wait-event"), "-m"],
            env=self.environment, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=5)
        self.assertNotEqual(result.returncode, 0, "An unrelated parent was accepted as the proven source runner")
        self.assertTrue((self.root / "executions").read_text().splitlines() == before,
                        "Rejected event-wrapper invocation still executed the native source")
        self.assertTrue((self.logs / "power-event.identity").read_bytes() == original_identity,
                        "Rejected event-wrapper invocation overwrote the valid source identity")
        self.finish_fixture()

    def test_normal_exit_restores_services_and_reaps_observer_group(self):
        self.start_fixture()
        self.finish_fixture()

    def test_killed_master_restores_services_and_reaps_observer_group(self):
        self.start_fixture()
        # Preserve a second owned regular leaf as if atomic capability
        # publication had been interrupted immediately before the crash.
        pending = self.lock / "native-output-capability.pending"
        pending.write_bytes((self.lock / "native-output-capability").read_bytes())
        pending.chmod(0o600)
        pending = self.lock / "native-portrait-orientation.pending"
        pending.write_bytes((self.lock / "native-portrait-orientation").read_bytes())
        pending.chmod(0o600)
        header = (self.lock / "generation").read_text().strip()
        for leaf in ("resume-native-portrait-orientation", "resume-native-portrait-orientation.pending"):
            marker = self.lock / leaf
            marker.write_text(f"{header}:2:CALIBRATED\n")
            marker.chmod(0o600)
        self.finish_fixture(kill_master=True)

    def test_existing_foreign_calibration_lock_is_preserved(self):
        self.lock.mkdir(mode=0o700)
        foreign_nonce = "f" * 32
        expected = {
            "pid": "12345\n", "nonce": foreign_nonce + "\n", "generation": f"9:{foreign_nonce}\n",
            "native-portrait-orientation": f"9:{foreign_nonce}:D:1\n",
            "resume-native-portrait-orientation": f"9:{foreign_nonce}:8:CALIBRATED\n",
        }
        for leaf, content in expected.items():
            marker = self.lock / leaf
            marker.write_text(content)
            marker.chmod(0o600)
        result = subprocess.run(["sh", str(self.run_root / "physical-watchdog.sh"),
                                 str(self.run_root), self.token, "30", "90"],
            env=self.environment, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=5)
        self.assertEqual(result.returncode, 3, "The watchdog did not refuse an existing foreign launcher lock")
        for leaf, content in expected.items():
            self.assertEqual((self.lock / leaf).read_text(), content, "Foreign calibration state was modified")
        self.assertFalse((self.root / "services").exists(), "Foreign lock refusal changed native services")
        self.completed = True

    def test_framebuffer_resume_uses_fresh_polarity_without_live_winmgr_query(self):
        self.start_fixture()
        probes = self.trace_values("fixture-framebuffer-probe")
        self.assertEqual(len(probes), 1)
        self.assertEqual(probes[0]["sequence"], 0)
        self.assertEqual(probes[0]["grayscale"], 1)
        self.assertEqual(probes[0]["native_epdc_mode"], "Y8")
        self.suspend_fixture()
        before_capability = (self.lock / "native-output-capability").read_bytes()
        before_queries = (self.root / "native-queries").read_text().splitlines()
        # A changed controlled variable-info value requires a fresh resume
        # observation. A live winmgr query is explicitly rejected after sleep.
        (self.root / "framebuffer-state").write_text("2\n")
        (self.root / "forbid-native-mode-query").write_text("1\n")
        resumed = self.resume_fixture()
        marker = (self.lock / "resume-native-epdc-mode").read_text().strip()
        self.assertTrue(marker.endswith(f":{resumed['sequence']}:FRAMEBUFFER"),
                        "A trusted framebuffer capability did not request a fresh variable-info probe")
        self.assertEqual(resumed["output_source"], "framebuffer")
        self.assertEqual(resumed["grayscale"], 2)
        self.assertEqual(resumed["native_epdc_mode"], "Y8INV")
        self.assertTrue(resumed["native_window_manager_stopped"],
                        "The fresh probe was requested before the native window manager was quiescent")
        probes = self.trace_values("fixture-framebuffer-probe")
        self.assertEqual(len(probes), 2)
        self.assertEqual(probes[-1]["sequence"], resumed["sequence"])
        self.assertEqual(probes[-1]["grayscale"], 2)
        after_queries = (self.root / "native-queries").read_text().splitlines()
        self.assertEqual(after_queries.count("epdcMode"), before_queries.count("epdcMode"),
                         "The explicit framebuffer wake path queried the live native display mode")
        self.assertTrue((self.lock / "native-output-capability").read_bytes() == before_capability,
                        "The resume path rewrote or reused the capability as a polarity cache")
        self.finish_fixture()

    def test_framebuffer_resume_calibrates_half_turns_without_new_ipc_or_observation_helpers(self):
        (self.root / "portrait-metadata").write_text("AUTO:3\n")
        self.start_fixture()
        calibration = (self.lock / "native-portrait-orientation").read_bytes()
        queries = (self.root / "native-queries").read_bytes()
        self.assertEqual(queries.decode().splitlines().count("accelerometer"), 2)
        helpers = [entry for entry in (self.root / "executions").read_text().splitlines()
                   if entry.startswith(("native-capture:", "portrait-probe:"))]
        self.assertEqual(len(helpers), 1)
        self.assertTrue(helpers[0].startswith("native-capture:"))
        samples = (self.root / "native-capture-samples").read_bytes()
        self.assertEqual(len(samples.splitlines()), 2)
        for rotation, expected in ((1, "D"), (3, "U")):
            self.suspend_fixture()
            (self.root / "portrait-rotation").write_text(f"{rotation}\n")
            (self.root / "forbid-native-mode-query").write_text("1\n")
            resumed = self.resume_fixture()
            self.assertEqual(resumed["native_portrait_orientation"], expected)
            self.assertEqual(resumed["framebuffer_rotation"], rotation)
            header = (self.lock / "generation").read_text().strip()
            self.assertEqual((self.lock / "resume-native-portrait-orientation").read_bytes(),
                             f"{header}:{resumed['sequence']}:CALIBRATED\n".encode())
            self.assertEqual((self.lock / "native-portrait-orientation").read_bytes(), calibration,
                             "Resume replaced the trusted startup calibration")
            self.assertEqual((self.root / "native-queries").read_bytes(), queries,
                             "The framebuffer wake path added a live native query")
            self.assertEqual([entry for entry in (self.root / "executions").read_text().splitlines()
                              if entry.startswith(("native-capture:", "portrait-probe:"))], helpers,
                             "The framebuffer wake path spawned a portrait observation helper")
            self.assertEqual((self.root / "native-capture-samples").read_bytes(), samples,
                             "The framebuffer wake path resampled through a native capture helper")
        self.finish_fixture()

    def test_resume_rejects_a_calibration_request_from_another_sequence(self):
        self.start_fixture()
        self.suspend_fixture()
        self.assertTrue(same_live_process(self.application))
        os.kill(self.application["pid"], signal.SIGSTOP)
        try:
            self.emit_events("outOfScreenSaver", "exitingScreenSaver")
            marker = self.lock / "resume-native-portrait-orientation"
            self.wait_for("the current protected resume calibration request", lambda: marker.exists())
            fields = marker.read_text().strip().split(":")
            self.assertEqual(fields[3], "CALIBRATED")
            sequence = int(fields[2])
            pending = marker.with_name(marker.name + ".pending")
            pending.write_text(f"{fields[0]}:{fields[1]}:{sequence + 1}:CALIBRATED\n")
            pending.replace(marker)
        finally:
            if same_live_process(self.application):
                os.kill(self.application["pid"], signal.SIGCONT)
        self.wait_for("rejection of the mismatched calibration", lambda: not same_live_process(self.application))
        self.assertEqual(self.trace_values("resume"), [], "A stale calibration sequence was acknowledged")
        self.finish_fixture()

    def test_resume_rejects_a_quarter_turn_before_acknowledgement(self):
        (self.root / "portrait-metadata").write_text("AUTO:3\n")
        self.start_fixture()
        self.suspend_fixture()
        (self.root / "portrait-rotation").write_text("0\n")
        self.emit_events("outOfScreenSaver", "exitingScreenSaver")
        self.wait_for("rejection of an unapproved portrait quarter turn", lambda: not same_live_process(self.application))
        self.assertEqual(self.trace_values("resume"), [], "A quarter turn was silently mapped into a portrait orientation")
        self.finish_fixture()

    def test_fixed_portrait_calibration_rejects_a_changed_kernel_rotation(self):
        self.start_fixture()
        self.suspend_fixture()
        (self.root / "portrait-rotation").write_text("1\n")
        self.emit_events("outOfScreenSaver", "exitingScreenSaver")
        self.wait_for("rejection of a changed fixed portrait rotation", lambda: not same_live_process(self.application))
        self.assertEqual(self.trace_values("resume"), [], "A fixed device silently acquired accelerometer rotation support")
        self.finish_fixture()

    def test_legacy_resume_captures_once_without_reading_ui_controls(self):
        self.environment["TEST_OUTPUT_CAPABILITY"] = "legacy"
        self.start_fixture()
        original_identity = (self.logs / "native.identity").read_bytes()
        original_pid = (self.logs / "native.pid").read_bytes()
        self.suspend_fixture()
        before_queries = (self.root / "native-queries").read_text().splitlines()
        # A helper must not enter the UI branch, even when its controls no
        # longer exist. The current application retains its same live session.
        (self.logs / "screen").unlink()
        (self.logs / "exit-after-ms").unlink()
        resumed = self.resume_fixture()
        marker = (self.lock / "resume-native-epdc-mode").read_text().strip()
        self.assertTrue(marker.endswith(f":{resumed['sequence']}:Y8INV"),
                        "A legacy capability received a framebuffer probe request")
        self.assertEqual(resumed["output_source"], "launcher")
        self.assertEqual(resumed["native_epdc_mode"], "Y8INV")
        self.assertEqual(self.trace_values("fixture-framebuffer-probe"), [])
        after_queries = (self.root / "native-queries").read_text().splitlines()
        self.assertEqual(after_queries.count("epdcMode"), before_queries.count("epdcMode") + 1,
                         "The legacy resume path did not capture its native mode exactly once")
        entries = (self.root / "executions").read_text().splitlines()
        helpers = [int(entry.split(":", 1)[1]) for entry in entries if entry.startswith("native-capture:")]
        self.assertEqual(len(helpers), 2, "Startup and resume did not each use one capture invocation")
        self.assertFalse(any(entry.startswith("portrait-probe:") for entry in entries))
        self.assertEqual((self.root / "native-capture-samples").read_text().splitlines(),
                         [f"{pid}:FIXED:3" for pid in helpers for _ in range(2)])
        for pid in helpers:
            self.assertEqual((self.arguments / f"native-capture-{pid}").read_text().splitlines(),
                             [str(self.fixture_binary), "--capture-native-display", "--kindle", "--device", "scribe"])
            self.assert_wrapper_trace(pid, "capture_entry")
        self.assertEqual([entry for entry in entries if entry.startswith("application:")],
                         [f"application:{self.application['pid']}"])
        self.assertEqual((self.logs / "native.identity").read_bytes(), original_identity)
        self.assertEqual((self.logs / "native.pid").read_bytes(), original_pid)
        self.finish_fixture()

    def cleanup_fixture(self):
        # A failed assertion must not leave the test's independently detached
        # observer group behind. Every fallback signal checks its same birth.
        self.remember_observer_if_owned()
        if self.watchdog and self.watchdog.poll() is None:
            if getattr(self, "watchdog_identity", None) and same_live_process(self.watchdog_identity):
                self.watchdog.send_signal(signal.SIGTERM)
            try:
                self.watchdog.wait(timeout=40)
            except subprocess.TimeoutExpired:
                pass
        if self.observer and same_live_process(self.observer):
            current = identity(self.observer["pid"])
            if current and current["group"] == current["session"] == self.observer["pid"]:
                try:
                    os.killpg(self.observer["pid"], signal.SIGKILL)
                except ProcessLookupError:
                    pass
        for record in reversed(self.watched):
            if same_live_process(record):
                try:
                    os.kill(record["pid"], signal.SIGKILL)
                except ProcessLookupError:
                    pass
        if self.watchdog and self.watchdog.poll() is None:
            if getattr(self, "watchdog_identity", None) and same_live_process(self.watchdog_identity):
                try:
                    os.killpg(self.watchdog.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
            try:
                self.watchdog.wait(timeout=5)
            except subprocess.TimeoutExpired:
                pass
        for child, record, _ in self.native_baseline:
            if same_live_process(record):
                try:
                    os.kill(child.pid, signal.SIGCONT)
                    os.kill(child.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
            try:
                child.wait(timeout=3)
            except subprocess.TimeoutExpired:
                pass
        if self.watchdog_log:
            self.watchdog_log.close()
        if self.completed:
            shutil.rmtree(self.root)
        else:
            # Retain failure logs on the host without publishing the private
            # ownership capability or any special FIFO in the evidence subset.
            diagnostic = {"test": self.id(), "fixture_root": str(self.root),
                          "device_accessed": False, "failure_evidence_retained": True}
            (self.root / "failure.json").write_text(json.dumps(diagnostic, indent=2) + "\n")


if __name__ == "__main__":
    unittest.main(verbosity=2)
