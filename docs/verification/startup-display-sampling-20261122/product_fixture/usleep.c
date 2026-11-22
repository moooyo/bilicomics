#include <errno.h>
#include <string.h>
#include <time.h>

int main(int argc, char **argv) {
    if (argc != 2 || (strcmp(argv[1], "10000") && strcmp(argv[1], "20000"))) return 2;
    struct timespec remaining = {0, !strcmp(argv[1], "10000") ? 10000000L : 20000000L};
    while (nanosleep(&remaining, &remaining)) if (errno != EINTR) return 1;
    return 0;
}
