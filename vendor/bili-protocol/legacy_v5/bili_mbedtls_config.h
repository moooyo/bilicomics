#ifndef BILI_MBEDTLS_CONFIG_H
#define BILI_MBEDTLS_CONFIG_H

/* Keep the vendored Mbed TLS build limited to the protocol primitives. */
#define MBEDTLS_AES_C
#define MBEDTLS_AES_ROM_TABLES
#define MBEDTLS_AES_FEWER_TABLES
#define MBEDTLS_BLOCK_CIPHER_C
#define MBEDTLS_GCM_C
#define MBEDTLS_MD_C
#define MBEDTLS_PKCS5_C
#define MBEDTLS_SHA512_C

/* The portable backend shares this dependency for P-256 and AES-ECB. */
#define MBEDTLS_BIGNUM_C
#define MBEDTLS_ECP_C
#define MBEDTLS_ECP_DP_SECP256R1_ENABLED
#define MBEDTLS_ECP_NIST_OPTIM

/* Hardware-specific assembly, TLS, X.509, PSA, and filesystem I/O are disabled. */

#endif
