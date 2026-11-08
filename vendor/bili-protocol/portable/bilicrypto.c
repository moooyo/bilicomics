#include "bilicrypto.h"
#include "bili_secure.h"
#include "mbedtls/aes.h"
#include "mbedtls/ecp.h"

#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <string.h>
#include <unistd.h>
#if defined(__linux__)
#include <sys/syscall.h>
#endif

#ifndef O_CLOEXEC
#define O_CLOEXEC 0
#endif

/* Linux getrandom blocks until the kernel CSPRNG is initialized. Using the
 * syscall preserves the glibc 2.17 and Android API 21 link baselines. Old kernels
 * without this syscall use the OS random device, never a userspace PRNG.
 */
int bili_os_random(uint8_t *destination, unsigned size) {
    unsigned complete = 0;
#if defined(__linux__) && defined(SYS_getrandom)
    while (complete < size) {
        long count = syscall(SYS_getrandom, destination + complete, size - complete, 0);
        if (count > 0) {
            complete += (unsigned)count;
        } else if (count < 0 && errno == EINTR) {
            continue;
        } else if (count < 0 && errno == ENOSYS) {
            break;
        } else {
            bili_secure_zero(destination, size);
            return 0;
        }
    }
    if (complete == size) {
        return 1;
    }
#endif
    int descriptor;
    do {
        descriptor = open("/dev/urandom", O_RDONLY | O_CLOEXEC);
    } while (descriptor < 0 && errno == EINTR);
    if (descriptor < 0) {
        bili_secure_zero(destination, size);
        return 0;
    }
    while (complete < size) {
        ssize_t count = read(descriptor, destination + complete, size - complete);
        if (count > 0) {
            complete += (unsigned)count;
        } else if (count < 0 && errno == EINTR) {
            continue;
        } else {
            close(descriptor);
            bili_secure_zero(destination, size);
            return 0;
        }
    }
    close(descriptor);
    return 1;
}

/* Compare the complete scalar without an early exit on its leading bytes. */
static int scalar_valid(const unsigned char scalar[32]) {
    static const unsigned char order[32] = {
        0xff,0xff,0xff,0xff,0x00,0x00,0x00,0x00,
        0xff,0xff,0xff,0xff,0xff,0xff,0xff,0xff,
        0xbc,0xe6,0xfa,0xad,0xa7,0x17,0x9e,0x84,
        0xf3,0xb9,0xca,0xc2,0xfc,0x63,0x25,0x51
    };
    unsigned borrow = 0, nonzero = 0;
    unsigned index = 32;
    while (index > 0) {
        --index;
        nonzero |= scalar[index];
        unsigned difference = (unsigned)scalar[index] - order[index] - borrow;
        borrow = difference >> (sizeof(unsigned) * CHAR_BIT - 1);
    }
    return nonzero != 0 && borrow == 1;
}

static int ecp_random(void *context, unsigned char *destination, size_t size) {
    (void)context;
    if (size > UINT_MAX) return MBEDTLS_ERR_ECP_RANDOM_FAILED;
    return bili_os_random(destination, (unsigned)size) ? 0 : MBEDTLS_ERR_ECP_RANDOM_FAILED;
}

int bili_p256_new(unsigned char private32[32], unsigned char public65[65]) {
    unsigned char scalar[32];
    BILI_WIPE_ON_EXIT(scalar);
    if (private32 != NULL) bili_secure_zero(private32, 32);
    if (public65 != NULL) bili_secure_zero(public65, 65);
    if (private32 == NULL || public65 == NULL) return 0;
    for (unsigned attempt = 0; attempt < 64; ++attempt) {
        if (!bili_os_random(scalar, sizeof(scalar))) return 0;
        if (!scalar_valid(scalar)) continue;
        if (!bili_p256_public(scalar, public65)) return 0;
        memcpy(private32, scalar, 32);
        return 1;
    }
    return 0;
}

int bili_p256_public(const unsigned char private32[32], unsigned char public65[65]) {
    if (public65 != NULL) bili_secure_zero(public65, 65);
    if (private32 == NULL || public65 == NULL || !scalar_valid(private32)) return 0;
    mbedtls_ecp_group group;
    mbedtls_ecp_point public_point;
    mbedtls_mpi private_scalar;
    size_t written = 0;
    int success = 0;
    mbedtls_ecp_group_init(&group);
    mbedtls_ecp_point_init(&public_point);
    mbedtls_mpi_init(&private_scalar);
    if (mbedtls_ecp_group_load(&group, MBEDTLS_ECP_DP_SECP256R1) != 0
        || mbedtls_mpi_read_binary(&private_scalar, private32, 32) != 0
        || mbedtls_ecp_check_privkey(&group, &private_scalar) != 0
        || mbedtls_ecp_mul(&group, &public_point, &private_scalar, &group.G, ecp_random, NULL) != 0
        || mbedtls_ecp_check_pubkey(&group, &public_point) != 0
        || mbedtls_ecp_point_write_binary(&group, &public_point, MBEDTLS_ECP_PF_UNCOMPRESSED,
            &written, public65, 65) != 0 || written != 65 || public65[0] != 4) goto cleanup;
    success = 1;
cleanup:
    mbedtls_mpi_free(&private_scalar);
    mbedtls_ecp_point_free(&public_point);
    mbedtls_ecp_group_free(&group);
    if (!success) bili_secure_zero(public65, 65);
    return success;
}

int bili_p256_derive(const unsigned char private32[32],
    const unsigned char public65[65], unsigned char secret32[32]) {
    if (secret32 != NULL) bili_secure_zero(secret32, 32);
    if (private32 == NULL || public65 == NULL || secret32 == NULL
        || !scalar_valid(private32) || public65[0] != 4) return 0;
    mbedtls_ecp_group group;
    mbedtls_ecp_point peer, shared;
    mbedtls_mpi private_scalar;
    int success = 0;
    mbedtls_ecp_group_init(&group);
    mbedtls_ecp_point_init(&peer);
    mbedtls_ecp_point_init(&shared);
    mbedtls_mpi_init(&private_scalar);
    if (mbedtls_ecp_group_load(&group, MBEDTLS_ECP_DP_SECP256R1) != 0
        || mbedtls_mpi_read_binary(&private_scalar, private32, 32) != 0
        || mbedtls_ecp_check_privkey(&group, &private_scalar) != 0
        || mbedtls_ecp_point_read_binary(&group, &peer, public65, 65) != 0
        || mbedtls_ecp_check_pubkey(&group, &peer) != 0
        || mbedtls_ecp_mul(&group, &shared, &private_scalar, &peer, ecp_random, NULL) != 0
        || mbedtls_ecp_is_zero(&shared)
        || mbedtls_mpi_write_binary(&shared.MBEDTLS_PRIVATE(X), secret32, 32) != 0) goto cleanup;
    success = 1;
cleanup:
    mbedtls_mpi_free(&private_scalar);
    mbedtls_ecp_point_free(&shared);
    mbedtls_ecp_point_free(&peer);
    mbedtls_ecp_group_free(&group);
    if (!success) bili_secure_zero(secret32, 32);
    return success;
}

static int aes_ecb(const unsigned char *key, unsigned key_bits, const unsigned char *input,
    unsigned long length, unsigned char *output, int decrypt) {
    mbedtls_aes_context context;
    if (key == NULL || (key_bits != 192 && key_bits != 256) || length % 16 != 0
        || (length != 0 && (input == NULL || output == NULL))) return 0;
    if (length == 0) return 1;
    mbedtls_aes_init(&context);
    int success = 0;
    int status = decrypt ? mbedtls_aes_setkey_dec(&context, key, key_bits)
                         : mbedtls_aes_setkey_enc(&context, key, key_bits);
    if (status != 0) goto cleanup;
    for (unsigned long offset = 0; offset < length; offset += 16) {
        if (mbedtls_aes_crypt_ecb(&context, decrypt ? MBEDTLS_AES_DECRYPT : MBEDTLS_AES_ENCRYPT,
            input + offset, output + offset) != 0) goto cleanup;
    }
    success = 1;
cleanup:
    mbedtls_aes_free(&context);
    if (!success) bili_secure_zero(output, length);
    return success;
}

int bili_aes256_ecb_encrypt(const unsigned char key32[32],
    const unsigned char *input, unsigned long length, unsigned char *output) {
    return aes_ecb(key32, 256, input, length, output, 0);
}

int bili_aes256_ecb_decrypt(const unsigned char key32[32],
    const unsigned char *input, unsigned long length, unsigned char *output) {
    return aes_ecb(key32, 256, input, length, output, 1);
}

/* Response decoding uses the same block primitive with a 192-bit key. */
int bili_aes192_ecb_decrypt(const unsigned char key24[24],
    const unsigned char *input, unsigned long length, unsigned char *output) {
    return aes_ecb(key24, 192, input, length, output, 1);
}
