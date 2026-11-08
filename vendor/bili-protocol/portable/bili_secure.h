#ifndef BILI_SECURE_H
#define BILI_SECURE_H

#include <stddef.h>
#include <stdint.h>

/* Volatile stores keep these explicit wipes in optimized GCC/Clang builds. */
static void bili_secure_zero(void *value, size_t size) {
    volatile unsigned char *bytes = (volatile unsigned char *)value;
    while (size-- > 0) {
        *bytes++ = 0;
    }
}

typedef struct {
    void *value;
    size_t size;
} bili_wipe_guard;

static void bili_wipe_cleanup(bili_wipe_guard *guard) {
    bili_secure_zero(guard->value, guard->size);
}

#if !defined(__GNUC__) && !defined(__clang__)
#error The portable crypto build requires GCC or Clang cleanup attributes.
#endif

/* Clear wrapper-owned automatic arrays, including early returns. */
#define BILI_WIPE_ON_EXIT(name) \
    bili_wipe_guard bili_wipe_guard_##name \
        __attribute__((cleanup(bili_wipe_cleanup))) = { (name), sizeof(name) }

int bili_os_random(uint8_t *destination, unsigned size);

#endif
