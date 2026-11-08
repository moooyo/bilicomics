#ifndef BILICRYPTO_H
#define BILICRYPTO_H

#if defined(__GNUC__) || defined(__clang__)
#define BILI_API __attribute__((visibility("default")))
#else
#define BILI_API
#endif

#ifdef __cplusplus
extern "C" {
#endif

/* All functions return 1 on success and 0 on failure. Callers own all buffers.
 * Scalars and coordinates are big endian. Public keys are 0x04 || X || Y.
 * Key and peer input buffers must not overlap output buffers.
 * AES input and output may be the same buffer; partial overlap is unsupported.
 */
BILI_API int bili_p256_new(unsigned char private32[32], unsigned char public65[65]);
BILI_API int bili_p256_public(const unsigned char private32[32], unsigned char public65[65]);
BILI_API int bili_p256_derive(const unsigned char private32[32],
    const unsigned char public65[65], unsigned char secret32[32]);
BILI_API int bili_aes256_ecb_encrypt(const unsigned char key32[32],
    const unsigned char *input, unsigned long length, unsigned char *output);
BILI_API int bili_aes256_ecb_decrypt(const unsigned char key32[32],
    const unsigned char *input, unsigned long length, unsigned char *output);
BILI_API int bili_aes192_ecb_decrypt(const unsigned char key24[24],
    const unsigned char *input, unsigned long length, unsigned char *output);

#ifdef __cplusplus
}
#endif
#endif
