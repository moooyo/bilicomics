/* Native test launcher only; never deploy this helper to the physical device. */
#define _GNU_SOURCE
#include <errno.h>
#include <linux/filter.h>
#include <linux/seccomp.h>
#include <signal.h>
#include <stddef.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/prctl.h>
#include <sys/resource.h>
#include <sys/socket.h>
#include <sys/syscall.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <unistd.h>

static int deny_install(void) {
    struct sock_filter instructions[] = {
        BPF_STMT(BPF_LD | BPF_W | BPF_ABS,
                 (unsigned int) offsetof(struct seccomp_data, nr)),
#ifdef __NR_seccomp
        BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, __NR_seccomp, 0, 1),
        BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ERRNO | EPERM),
#endif
        BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, __NR_prctl, 0, 3),
        BPF_STMT(BPF_LD | BPF_W | BPF_ABS,
                 (unsigned int) offsetof(struct seccomp_data, args[0])),
        BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, PR_SET_SECCOMP, 0, 1),
        BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ERRNO | EPERM),
        BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ALLOW)
    };
    struct sock_fprog program = {
        .len = (unsigned short) (sizeof(instructions) / sizeof(instructions[0])),
        .filter = instructions
    };
    return prctl(PR_SET_SECCOMP, SECCOMP_MODE_FILTER,
                 (unsigned long) &program, 0UL, 0UL);
}

static int deny_proof_sync(void) {
    struct sock_filter instructions[] = {
        BPF_STMT(BPF_LD | BPF_W | BPF_ABS,
                 (unsigned int) offsetof(struct seccomp_data, nr)),
        BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, __NR_fsync, 0, 1),
        BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ERRNO | EIO),
        BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ALLOW)
    };
    struct sock_fprog program = {
        .len = (unsigned short) (sizeof(instructions) / sizeof(instructions[0])),
        .filter = instructions
    };
    return prctl(PR_SET_SECCOMP, SECCOMP_MODE_FILTER,
                 (unsigned long) &program, 0UL, 0UL);
}

static int check_abi(void) {
#if defined(__x86_64__) && !defined(__ILP32__)
    const long numbers[] = {__NR_socket | 0x40000000L, 512L, 547L};
    for (unsigned int i = 0; i < sizeof(numbers) / sizeof(numbers[0]); ++i) {
        errno = 0;
        long result = syscall(numbers[i], AF_INET, SOCK_STREAM, 0UL,
                              0UL, 0UL, 0UL);
        if (result != -1 || errno != EPERM) return 11;
    }
    if (syscall(__NR_getuid) != (long) getuid()) return 12;
    struct rlimit core_limit = {0, 0};
    if (setrlimit(RLIMIT_CORE, &core_limit) != 0) return 13;
    pid_t child = fork();
    if (child < 0) return 14;
    if (child == 0) {
        long result;
        /* No valid pointer is supplied even if the wrong-ABI filter fails. */
        __asm__ volatile("int $0x80"
                         : "=a"(result)
                         : "0"(102L), "b"(1L), "c"(0L)
                         : "memory");
        (void) result;
        _exit(70);
    }
    int status = 0;
    if (waitpid(child, &status, 0) != child || !WIFSIGNALED(status) ||
        WTERMSIG(status) != SIGSYS) return 15;
    puts("{\"x32_denied\":1,\"compat_arch_killed\":1,\"native_getuid_allowed\":1}");
    return 0;
#else
    return 16;
#endif
}

int main(int argc, char **argv) {
    const char *remote = getenv("BILICOMICS_REMOTE_VERIFICATION");
    if (!remote || strcmp(remote, "1") != 0) return 90;
    if (argc == 2 && strcmp(argv[1], "--check-abi") == 0) return check_abi();
    if (argc < 3 || argv[2][0] != '/' ||
        (strcmp(argv[1], "--allow-install") != 0 &&
         strcmp(argv[1], "--deny-install") != 0 &&
         strcmp(argv[1], "--deny-proof-sync") != 0 &&
         strcmp(argv[1], "--limit-proof-file") != 0)) return 91;
    /* This test-only launcher explicitly supplies the unprivileged test setup. */
    if (prctl(PR_SET_NO_NEW_PRIVS, 1UL, 0UL, 0UL, 0UL) != 0) return 92;
    if (strcmp(argv[1], "--deny-install") == 0 && deny_install() != 0) return 93;
    if (strcmp(argv[1], "--deny-proof-sync") == 0 && deny_proof_sync() != 0) return 95;
    if (strcmp(argv[1], "--limit-proof-file") == 0) {
        struct rlimit file_limit = {0, 0};
        if (setrlimit(RLIMIT_FSIZE, &file_limit) != 0 ||
            signal(SIGXFSZ, SIG_IGN) == SIG_ERR) return 96;
    }
    execv(argv[2], &argv[2]);
    return 94;
}
