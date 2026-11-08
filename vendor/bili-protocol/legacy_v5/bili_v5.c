#include "bili_v5.h"

#include "mbedtls/gcm.h"
#include "mbedtls/pkcs5.h"
#include "mbedtls/platform_util.h"

#include <stdint.h>
#include <stdlib.h>
#include <string.h>

#define BILI_PBKDF2_MAX_INPUT 1024u
#define BILI_PBKDF2_MAX_OUTPUT 1024u
#define BILI_PBKDF2_MAX_WORK 1000000u
#define BILI_GCM_MAX_IV 1024u
#define BILI_GCM_MAX_AAD (1024u * 1024u)
#define BILI_GCM_MAX_CIPHER (64u * 1024u * 1024u)

int bili_pbkdf2_sha512(
    const unsigned char *password, size_t passlen,
    const unsigned char *salt, size_t saltlen,
    unsigned int iterations, unsigned char *out, size_t outlen)
{
    static const unsigned char empty = 0;
    unsigned char *derived;
    size_t blocks;
    int result;

    if ((password == NULL && passlen != 0) ||
        (salt == NULL && saltlen != 0) || out == NULL ||
        passlen > BILI_PBKDF2_MAX_INPUT || saltlen > BILI_PBKDF2_MAX_INPUT ||
        outlen == 0 || outlen > BILI_PBKDF2_MAX_OUTPUT || iterations == 0 ||
        iterations > BILI_PBKDF2_MAX_WORK) {
        return 0;
    }
    blocks = (outlen + 63u) / 64u;
    if (iterations > BILI_PBKDF2_MAX_WORK / blocks) {
        return 0;
    }
    derived = (unsigned char *) malloc(outlen);
    if (derived == NULL) {
        return 0;
    }
    result = mbedtls_pkcs5_pbkdf2_hmac_ext(
        MBEDTLS_MD_SHA512, password != NULL ? password : &empty, passlen,
        salt != NULL ? salt : &empty, saltlen, iterations,
        (uint32_t) outlen, derived) == 0;
    if (result) {
        memcpy(out, derived, outlen);
    }
    mbedtls_platform_zeroize(derived, outlen);
    free(derived);
    return result;
}

int bili_aes_gcm_decrypt(
    const unsigned char *key, size_t keylen,
    const unsigned char *iv, size_t ivlen,
    const unsigned char *aad, size_t aadlen,
    const unsigned char *cipher, size_t cipherlen,
    const unsigned char *tag, size_t taglen,
    unsigned char *out)
{
    static const unsigned char empty = 0;
    mbedtls_gcm_context context;
    unsigned char *plain;
    size_t allocation;
    int result = 0;

    if (key == NULL || (keylen != 16 && keylen != 24 && keylen != 32) ||
        iv == NULL || ivlen == 0 || ivlen > BILI_GCM_MAX_IV ||
        (aad == NULL && aadlen != 0) || aadlen > BILI_GCM_MAX_AAD ||
        (cipher == NULL && cipherlen != 0) ||
        (out == NULL && cipherlen != 0) || cipherlen > BILI_GCM_MAX_CIPHER ||
        tag == NULL || taglen != 16) {
        return 0;
    }
    allocation = cipherlen != 0 ? cipherlen : 1;
    plain = (unsigned char *) malloc(allocation);
    if (plain == NULL) {
        return 0;
    }
    mbedtls_gcm_init(&context);
    if (mbedtls_gcm_setkey(&context, MBEDTLS_CIPHER_ID_AES, key,
                           (unsigned int) keylen * 8u) == 0 &&
        mbedtls_gcm_auth_decrypt(
            &context, cipherlen, iv, ivlen,
            aad != NULL ? aad : &empty, aadlen, tag, taglen,
            cipher != NULL ? cipher : &empty, plain) == 0) {
        if (cipherlen != 0) {
            memcpy(out, plain, cipherlen);
        }
        result = 1;
    }
    mbedtls_gcm_free(&context);
    mbedtls_platform_zeroize(plain, allocation);
    free(plain);
    return result;
}
