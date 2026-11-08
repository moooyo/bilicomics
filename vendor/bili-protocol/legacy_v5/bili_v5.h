#ifndef BILI_V5_H
#define BILI_V5_H

#include <stddef.h>

#if defined(_WIN32)
#define BILI_V5_API __declspec(dllexport)
#elif defined(__GNUC__)
#define BILI_V5_API __attribute__((visibility("default")))
#else
#define BILI_V5_API
#endif

#ifdef __cplusplus
extern "C" {
#endif

/*
 * All inputs are byte strings. A NULL pointer is allowed only for a zero length.
 * The caller must provide readable input and writable output buffers of the
 * stated sizes. Success returns 1. Every failure returns 0 without modifying
 * output. Input and output may overlap because results are staged privately.
 * Calls use independent contexts and never retain caller-owned pointers.
 *
 * Password and salt are limited to 1024 bytes each, output to 1024 bytes,
 * iterations to 1..1000000, and iterations * ceil(outlen / 64) to 1000000.
 * A zero output length is rejected. Protocol v5 uses 32 output bytes and
 * exactly 100000 iterations.
 */
BILI_V5_API int bili_pbkdf2_sha512(
    const unsigned char *password, size_t passlen,
    const unsigned char *salt, size_t saltlen,
    unsigned int iterations, unsigned char *out, size_t outlen);

/*
 * AES-GCM decryption accepts AES-128/192/256, a 1..1024-byte IV, up to
 * 1 MiB of AAD, up to 64 MiB of ciphertext, and an exact 16-byte tag.
 * A zero ciphertext length is supported. The tag is separate from ciphertext.
 * No plaintext is copied to output before authentication succeeds. Internal
 * key contexts and temporary plaintext are cleared on every cleanup path.
 */
BILI_V5_API int bili_aes_gcm_decrypt(
    const unsigned char *key, size_t keylen,
    const unsigned char *iv, size_t ivlen,
    const unsigned char *aad, size_t aadlen,
    const unsigned char *cipher, size_t cipherlen,
    const unsigned char *tag, size_t taglen,
    unsigned char *out);

#ifdef __cplusplus
}
#endif

#endif
