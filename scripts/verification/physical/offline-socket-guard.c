/*
 * Socket boundary for a reviewed, offline physical-device fixture.
 *
 * This helper preserves credentials, namespaces, and the inherited
 * no_new_privs value. The caller must already be permitted to install a
 * seccomp filter. Installation, descriptor inspection, or self-check failure
 * prevents exec; there is no unguarded fallback.
 *
 * This is not a sandbox for hostile code. AF_UNIX proxies and SCM_RIGHTS,
 * ptrace/pidfd descriptor acquisition, and io_uring are outside this policy.
 * The reviewed application uses ordinary socket/socketpair system calls.
 */
#define _GNU_SOURCE
#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <linux/audit.h>
#include <linux/filter.h>
#include <linux/seccomp.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/prctl.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <unistd.h>

#if !defined(__linux__) || __BYTE_ORDER__ != __ORDER_LITTLE_ENDIAN__
#error "The socket guard requires a supported little-endian Linux ABI."
#elif defined(__x86_64__) && !defined(__ILP32__)
#define GUARD_AUDIT_ARCH AUDIT_ARCH_X86_64
#define GUARD_X86_64 1
#elif defined(__arm__) && defined(__ARM_EABI__)
#define GUARD_AUDIT_ARCH AUDIT_ARCH_ARM
#define GUARD_ARM_EABI 1
#else
#error "Only x86_64 LP64 and ARM EABI32 are supported."
#endif

#if !defined(__NR_socket) || !defined(__NR_socketpair) || !defined(SO_DOMAIN)
#error "Native socket system calls and SO_DOMAIN must be defined."
#endif

enum {
    EXIT_GUARD_FAILURE = 125,
    STAGE_USAGE = 1,
    STAGE_INITIAL_STATE = 2,
    STAGE_INSTALL = 3,
    STAGE_CHECK = 4,
    STAGE_PROOF = 5,
    STAGE_EXEC = 6,
    STAGE_PROOF_FILE = 7,
    EXPECTED_DENIED_MASK = 63
};

struct descriptor_counts {
    unsigned long non_unix_closed;
    unsigned long unix_preserved;
    unsigned long non_socket_preserved;
};

struct checks {
    unsigned int socket_mask;
    unsigned int socketpair_mask;
    int socketcall_checked;
    int socketcall_denied;
    int x32_checked;
    int x32_denied;
};

/* No diagnostic output is permitted until all inherited FDs are inspected. */
static int inspect_descriptor(int fd, struct descriptor_counts *counts) {
    struct stat value;
    if (fstat(fd, &value) != 0) {
        return errno == EBADF ? 0 : -1;
    }
    if (!S_ISSOCK(value.st_mode)) {
        ++counts->non_socket_preserved;
        return 0;
    }

    int domain = -1;
    socklen_t length = sizeof(domain);
    if (getsockopt(fd, SOL_SOCKET, SO_DOMAIN, &domain, &length) != 0 ||
        length != sizeof(domain)) {
        /* An unidentifiable socket is closed before failing closed. */
        (void) close(fd);
        return -1;
    }
    if (domain == AF_UNIX) {
        ++counts->unix_preserved;
        return 0;
    }
    if (close(fd) != 0) return -1;
    ++counts->non_unix_closed;
    return 0;
}

static int inspect_inherited_descriptors(struct descriptor_counts *counts) {
    /* Include stdio, including a network socket used as an error stream. */
    for (int fd = 0; fd <= STDERR_FILENO; ++fd) {
        if (inspect_descriptor(fd, counts) != 0) return -1;
    }

    DIR *directory = opendir("/proc/self/fd");
    if (!directory) return -1;
    int directory_fd = dirfd(directory);
    if (directory_fd < 0) {
        (void) closedir(directory);
        return -1;
    }
    for (;;) {
        errno = 0;
        struct dirent *entry = readdir(directory);
        if (!entry) {
            int failed = errno != 0;
            if (closedir(directory) != 0) failed = 1;
            return failed ? -1 : 0;
        }
        if (strcmp(entry->d_name, ".") == 0 ||
            strcmp(entry->d_name, "..") == 0) continue;
        char *end = NULL;
        errno = 0;
        long parsed = strtol(entry->d_name, &end, 10);
        if (errno || !end || *end || parsed < 0 || parsed > INT_MAX) {
            (void) closedir(directory);
            return -1;
        }
        int fd = (int) parsed;
        if (fd <= STDERR_FILENO || fd == directory_fd) continue;
        if (inspect_descriptor(fd, counts) != 0) {
            (void) closedir(directory);
            return -1;
        }
    }
}

static int fail(int stage, int error_number, int exit_code) {
    fprintf(stderr,
            "{\"offline_socket_guard\":1,\"schema_version\":1,"
            "\"error_stage\":%d,\"error_errno\":%d}\n",
            stage, error_number);
    return exit_code;
}

static int install_filter(void) {
    struct sock_filter instructions[] = {
        BPF_STMT(BPF_LD | BPF_W | BPF_ABS,
                 (unsigned int) offsetof(struct seccomp_data, arch)),
        BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, GUARD_AUDIT_ARCH, 1, 0),
        /* The legacy KILL action also works on older Kindle kernels. */
        BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_KILL),
        BPF_STMT(BPF_LD | BPF_W | BPF_ABS,
                 (unsigned int) offsetof(struct seccomp_data, nr)),
#if defined(GUARD_X86_64)
        BPF_JUMP(BPF_JMP | BPF_JSET | BPF_K, 0x40000000U, 0, 1),
        BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ERRNO | EPERM),
        /* Reject legacy untagged x32 aliases on kernels older than 5.4. */
        BPF_JUMP(BPF_JMP | BPF_JGE | BPF_K, 512, 0, 2),
        BPF_JUMP(BPF_JMP | BPF_JGT | BPF_K, 547, 1, 0),
        BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ERRNO | EPERM),
#elif defined(GUARD_ARM_EABI)
        /* ARM OABI socketcall is 102; x86_64 syscall 102 is getuid. */
        BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, 102, 0, 1),
        BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ERRNO | EPERM),
        /* Reject unnormalized OABI numbers; retain ARM private EABI calls. */
        BPF_JUMP(BPF_JMP | BPF_JGE | BPF_K, 0x00900000U, 0, 1),
        BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ERRNO | EPERM),
#endif
        BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, __NR_socket, 2, 0),
        BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, __NR_socketpair, 1, 0),
        BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ALLOW),
        /* The low word matches the kernel's int domain on these ABIs. */
        BPF_STMT(BPF_LD | BPF_W | BPF_ABS,
                 (unsigned int) offsetof(struct seccomp_data, args[0])),
        BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, AF_UNIX, 0, 1),
        BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ALLOW),
        BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ERRNO | EPERM)
    };
    struct sock_fprog program = {
        .len = (unsigned short) (sizeof(instructions) / sizeof(instructions[0])),
        .filter = instructions
    };
    /* Do not set no_new_privs or change credentials to make this succeed. */
    return prctl(PR_SET_SECCOMP, SECCOMP_MODE_FILTER,
                 (unsigned long) &program, 0UL, 0UL);
}

static int denied_call(long result, int error_number) {
    if (result >= 0) {
        (void) close((int) result);
        return 0;
    }
    return result == -1 && error_number == EPERM;
}

static int check_filter(struct checks *checks) {
    /* Bits 0..5: UNSPEC, INET, INET6, PACKET, NETLINK, VSOCK. */
    const int domains[] = {AF_UNSPEC, AF_INET, AF_INET6, AF_PACKET,
                           AF_NETLINK, AF_VSOCK};
    const int types[] = {SOCK_STREAM, SOCK_STREAM, SOCK_STREAM, SOCK_DGRAM,
                         SOCK_RAW, SOCK_STREAM};
    for (unsigned int i = 0; i < sizeof(domains) / sizeof(domains[0]); ++i) {
        errno = 0;
        int fd = socket(domains[i], types[i], 0);
        int error_number = errno;
        if (!denied_call(fd, error_number)) return -1;
        checks->socket_mask |= 1U << i;

        int pair[2] = {-1, -1};
        errno = 0;
        int result = socketpair(domains[i], types[i], 0, pair);
        error_number = errno;
        if (result == 0) {
            (void) close(pair[0]);
            (void) close(pair[1]);
            return -1;
        }
        if (result != -1 || error_number != EPERM) return -1;
        checks->socketpair_mask |= 1U << i;
    }

    int unix_fd = socket(AF_UNIX, SOCK_STREAM, 0);
    if (unix_fd < 0 || close(unix_fd) != 0) return -1;
    int unix_pair[2] = {-1, -1};
    if (socketpair(AF_UNIX, SOCK_STREAM, 0, unix_pair) != 0) return -1;
    int close_first = close(unix_pair[0]);
    int close_second = close(unix_pair[1]);
    if (close_first != 0 || close_second != 0) return -1;

#if defined(GUARD_ARM_EABI)
    unsigned long arguments[] = {AF_INET, SOCK_STREAM, 0};
    checks->socketcall_checked = 1;
    errno = 0;
    long result = syscall(102L, 1UL, (unsigned long) arguments);
    if (!denied_call(result, errno)) return -1;
    checks->socketcall_denied = 1;
#elif defined(GUARD_X86_64)
    checks->x32_checked = 1;
    const long numbers[] = {__NR_socket | 0x40000000L, 512L, 547L};
    for (unsigned int i = 0; i < sizeof(numbers) / sizeof(numbers[0]); ++i) {
        errno = 0;
        long result = syscall(numbers[i], AF_INET, SOCK_STREAM, 0UL,
                              0UL, 0UL, 0UL);
        if (!denied_call(result, errno)) return -1;
    }
    checks->x32_denied = 1;
#endif
    return checks->socket_mask == EXPECTED_DENIED_MASK &&
                   checks->socketpair_mask == EXPECTED_DENIED_MASK ? 0 : -1;
}

static int write_proof(FILE *stream, int mode, int no_new_privs,
                       const struct descriptor_counts *counts,
                       const struct checks *checks) {
    int result = fprintf(stream,
        "{\"offline_socket_guard\":1,\"schema_version\":1,\"mode\":%d,"
        "\"process_id\":%lu,\"uid\":%lu,\"gid\":%lu,"
        "\"effective_uid\":%lu,\"effective_gid\":%lu,"
        "\"no_new_privs\":%d,\"seccomp_mode\":2,\"installed\":1,"
        "\"checks_passed\":1,\"audit_arch\":%lu,"
        "\"inherited_non_unix_closed\":%lu,\"inherited_unix_preserved\":%lu,"
        "\"inherited_non_socket_preserved\":%lu,"
        "\"denied_socket_mask\":%u,\"denied_socketpair_mask\":%u,"
        "\"unix_socket_allowed\":1,\"unix_socketpair_allowed\":1,"
        "\"socketcall_checked\":%d,\"socketcall_denied\":%d,"
        "\"x32_checked\":%d,\"x32_denied\":%d}\n",
        mode, (unsigned long) getpid(), (unsigned long) getuid(),
        (unsigned long) getgid(), (unsigned long) geteuid(),
        (unsigned long) getegid(), no_new_privs, (unsigned long) GUARD_AUDIT_ARCH,
        counts->non_unix_closed, counts->unix_preserved,
        counts->non_socket_preserved, checks->socket_mask, checks->socketpair_mask,
        checks->socketcall_checked, checks->socketcall_denied,
        checks->x32_checked, checks->x32_denied);
    return result >= 0 && fflush(stream) == 0 ? 0 : -1;
}

static int write_proof_file(const char *path, int no_new_privs,
                            const struct descriptor_counts *counts,
                            const struct checks *checks) {
    /* The harness controls the parent directory. Never replace an old leaf. */
    int fd = open(path, O_WRONLY | O_CREAT | O_EXCL | O_NOFOLLOW | O_CLOEXEC, 0600);
    if (fd < 0) return -1;
    struct stat value;
    int failure = 0;
    if (fstat(fd, &value) != 0) failure = errno;
    else if (!S_ISREG(value.st_mode) || value.st_nlink != 1) failure = EPROTO;
    if (failure) {
        (void) close(fd);
        errno = failure;
        return -1;
    }
    FILE *stream = fdopen(fd, "w");
    if (!stream) {
        failure = errno;
        (void) close(fd);
        errno = failure;
        return -1;
    }
    if (write_proof(stream, 2, no_new_privs, counts, checks) != 0)
        failure = errno ? errno : EIO;
    if (!failure && fsync(fd) != 0) failure = errno ? errno : EIO;
    if (fclose(stream) != 0 && !failure) failure = errno ? errno : EIO;
    /* Keep partial or complete failure evidence; never unlink this path. */
    if (failure) {
        errno = failure;
        return -1;
    }
    return 0;
}

int main(int argc, char **argv) {
    struct descriptor_counts counts = {0};
    if (inspect_inherited_descriptors(&counts) != 0) {
        /* Even stderr may still be a foreign socket after an inspection error. */
        _exit(EXIT_GUARD_FAILURE);
    }
    int mode = 0;
    int target_index = 2;
    const char *proof_path = NULL;
    if (argc == 2 && strcmp(argv[1], "--probe") == 0) mode = 1;
    else if (argc >= 3 && strcmp(argv[1], "--") == 0 && argv[2][0] == '/') mode = 2;
    else if (argc >= 5 && strcmp(argv[1], "--proof-file") == 0 &&
             argv[2][0] == '/' && strcmp(argv[3], "--") == 0 && argv[4][0] == '/') {
        mode = 2;
        target_index = 4;
        proof_path = argv[2];
    }
    if (!mode) return fail(STAGE_USAGE, EINVAL, 2);

    int no_new_privs = prctl(PR_GET_NO_NEW_PRIVS, 0UL, 0UL, 0UL, 0UL);
    if (no_new_privs < 0) return fail(STAGE_INITIAL_STATE, errno, EXIT_GUARD_FAILURE);
    if (install_filter() != 0) return fail(STAGE_INSTALL, errno, EXIT_GUARD_FAILURE);
    if (prctl(PR_GET_SECCOMP, 0UL, 0UL, 0UL, 0UL) != SECCOMP_MODE_FILTER ||
        prctl(PR_GET_NO_NEW_PRIVS, 0UL, 0UL, 0UL, 0UL) != no_new_privs) {
        return fail(STAGE_CHECK, EPROTO, EXIT_GUARD_FAILURE);
    }
    struct checks checks = {0};
    if (check_filter(&checks) != 0) return fail(STAGE_CHECK, EPROTO, EXIT_GUARD_FAILURE);
    if (proof_path && write_proof_file(proof_path, no_new_privs, &counts, &checks) != 0)
        return fail(STAGE_PROOF_FILE, errno, EXIT_GUARD_FAILURE);
    if (write_proof(mode == 1 ? stdout : stderr, mode, no_new_privs,
                    &counts, &checks) != 0) {
        return fail(STAGE_PROOF, EIO, EXIT_GUARD_FAILURE);
    }
    if (mode == 1) return 0;
    execv(argv[target_index], &argv[target_index]);
    return fail(STAGE_EXEC, errno, 127);
}
