#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <inttypes.h>
#include <limits.h>
#include <stdio.h>
#include <string.h>
#include <sys/file.h>
#include <sys/stat.h>
#include <unistd.h>

#ifndef ROUTE_MEDIA
#define ROUTE_MEDIA "/mnt/us"
#endif

enum { stage_fd = 8, production_fd = 9 };

static int fail(const char *message) {
    fprintf(stderr, "route-fs: %s\n", message);
    return 1;
}

static int same_directory(int fd, const char *path) {
    struct stat opened, named;
    return fstat(fd, &opened) == 0 && lstat(path, &named) == 0 && S_ISDIR(named.st_mode) &&
           opened.st_dev == named.st_dev && opened.st_ino == named.st_ino;
}

static int open_directory(const char *path, int slot) {
    int fd = open(path, O_RDONLY | O_DIRECTORY | O_NOFOLLOW);
    if (fd < 0 || !same_directory(fd, path)) return 0;
    if (fd != slot) {
        if (dup2(fd, slot) < 0) return 0;
        close(fd);
    }
    return 1;
}

static int held(const char *stage, const char *production) {
    struct stat staging_dir, production_dir;
    return same_directory(stage_fd, stage) && same_directory(production_fd, production) &&
           flock(production_fd, LOCK_EX | LOCK_NB) == 0 && fstat(stage_fd, &staging_dir) == 0 &&
           fstat(production_fd, &production_dir) == 0 && staging_dir.st_dev == production_dir.st_dev;
}

static int move_between(int source_fd, const char *source, int target_fd, const char *target,
                        const char *expected) {
    char identity[160];
    struct stat original, current;
    if (fstatat(source_fd, source, &original, AT_SYMLINK_NOFOLLOW) != 0 ||
        !S_ISREG(original.st_mode) || original.st_nlink != 1)
        return fail("source is not the expected single-link file");
    int written = snprintf(identity, sizeof(identity), "%ju:%ju:%x:%jd:%ju:%jd",
                           (uintmax_t)original.st_dev, (uintmax_t)original.st_ino, (unsigned)original.st_mode,
                           (intmax_t)original.st_size, (uintmax_t)original.st_nlink, (intmax_t)original.st_mtime);
    if (written < 0 || (size_t)written >= sizeof(identity) || strcmp(identity, expected) != 0)
        return fail("source identity changed");
    if (fstatat(target_fd, target, &current, AT_SYMLINK_NOFOLLOW) == 0 || errno != ENOENT)
        return fail("destination is not absent");
    if (renameat(source_fd, source, target_fd, target) != 0) return fail("same-volume rename failed");
    if (fstatat(target_fd, target, &current, AT_SYMLINK_NOFOLLOW) != 0 ||
        original.st_dev != current.st_dev || original.st_ino != current.st_ino ||
        original.st_mode != current.st_mode || original.st_size != current.st_size ||
        original.st_nlink != current.st_nlink || original.st_mtime != current.st_mtime ||
        fstatat(source_fd, source, &current, AT_SYMLINK_NOFOLLOW) == 0 || errno != ENOENT)
        return fail("unexpected identity after rename; recover without overwriting");
    return 0;
}

static int probe(const char *production, const char *expected) {
    int independent = open(production, O_RDONLY | O_DIRECTORY | O_NOFOLLOW);
    if (independent < 0 || !same_directory(independent, production)) return fail("cannot open lock comparison directory");
    int blocked = flock(independent, LOCK_EX | LOCK_NB) < 0 && errno == EWOULDBLOCK;
    close(independent);
    if (!blocked) return fail("volume does not serialize another route operation");
    int parent = openat(stage_fd, "rename-probe", O_RDONLY | O_DIRECTORY | O_NOFOLLOW);
    if (parent < 0) return fail("missing stage-local rename probe directory");
    int left = openat(parent, "left", O_RDONLY | O_DIRECTORY | O_NOFOLLOW);
    int right = openat(parent, "right", O_RDONLY | O_DIRECTORY | O_NOFOLLOW);
    struct stat destination, stage;
    if (left < 0 || right < 0 || fstat(stage_fd, &stage) != 0 ||
        fstat(left, &destination) != 0 || destination.st_dev != stage.st_dev ||
        fstat(right, &destination) != 0 || destination.st_dev != stage.st_dev)
        return fail("probe directories are on an unexpected volume");
    if (move_between(left, "dummy", right, "dummy", expected)) return 1;
    return move_between(right, "dummy", left, "dummy", expected);
}

int main(int argc, char **argv) {
    const char prefix[] = "bili-shelf-";
    const char production[] = ROUTE_MEDIA "/bilicomics";
    char stage[PATH_MAX], script[PATH_MAX];
    if (argc < 3 || strlen(argv[2]) != sizeof(prefix) - 1 + 32 ||
        strncmp(argv[2], prefix, sizeof(prefix) - 1) != 0 ||
        strspn(argv[2] + sizeof(prefix) - 1, "0123456789abcdef") != 32)
        return fail("invalid run");
    int written = snprintf(stage, sizeof(stage), "%s/%s", ROUTE_MEDIA, argv[2]);
    if (written < 0 || (size_t)written >= sizeof(stage)) return fail("invalid stage path");
    written = snprintf(script, sizeof(script), "%s/switch.sh", stage);
    if (written < 0 || (size_t)written >= sizeof(script)) return fail("invalid script path");
    if (argc == 4 && strcmp(argv[1], "lock") == 0 &&
        (strcmp(argv[3], "inspect") == 0 || strcmp(argv[3], "activate") == 0 ||
         strcmp(argv[3], "restore") == 0 || strcmp(argv[3], "cleanup") == 0)) {
        if (!open_directory(production, production_fd) || !open_directory(stage, stage_fd) || !held(stage, production))
            return fail("route operation busy, replaced directory, or different device");
        char *arguments[] = {"sh", script, argv[2], argv[3], "locked", NULL};
        execv("/bin/sh", arguments);
        return fail("cannot execute switch.sh");
    }
    if (!held(stage, production)) return fail("missing route operation lock");
    if (argc == 3 && strcmp(argv[1], "held") == 0) return 0;
    if (argc == 3 && strcmp(argv[1], "persist") == 0)
        return syncfs(production_fd) == 0 ? 0 : fail("cannot sync the route volume");
    if (argc == 4 && strcmp(argv[1], "probe") == 0) return probe(production, argv[3]);
    if (argc == 5 && strcmp(argv[1], "move") == 0) {
        if (strcmp(argv[3], "journal") == 0)
            return move_between(stage_fd, "prepared.pending", stage_fd, "prepared", argv[4]);
        if (strcmp(argv[3], "park") == 0)
            return move_between(production_fd, "launch.sh", stage_fd, "retained-launch.sh", argv[4]);
        if (strcmp(argv[3], "publish") == 0)
            return move_between(stage_fd, "entry-ready.sh", production_fd, "launch.sh", argv[4]);
        if (strcmp(argv[3], "unpublish") == 0)
            return move_between(production_fd, "launch.sh", stage_fd, "entry-ready.sh", argv[4]);
        if (strcmp(argv[3], "return") == 0)
            return move_between(stage_fd, "retained-launch.sh", production_fd, "launch.sh", argv[4]);
    }
    return fail("unsupported route operation");
}
