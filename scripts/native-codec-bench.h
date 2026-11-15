/* SPDX-License-Identifier: GPL-3.0-or-later
 * Adapted from the AidokuForKindle native-media candidate dated 2026-11-15.
 * See vendor/aidoku-native-media/README.md for source identity and provenance.
 */
#ifndef AIDOKU_NATIVE_CODEC_BENCH_H
#define AIDOKU_NATIVE_CODEC_BENCH_H

#include <stddef.h>
#include <stdint.h>

/* Fixed-size resident-call ABI. The caller owns input and Gray8 output.
 * The application only calls the JPEG entry from its isolated one-shot worker.
 */
struct aidoku_codec_bench_result {
    uint32_t abi_version;
    uint32_t structure_bytes;
    uint32_t width;
    uint32_t height;
    uint32_t output_bytes;
    uint32_t source_events;
    int32_t error;
    int32_t cleanup_error;
    double driver_ms;
    double preparation_and_driver_ms;
    char stage[64];
};

int aidoku_png_bench_decode(const unsigned char *input, size_t input_bytes,
    uint32_t width, uint32_t height, unsigned char *output, size_t output_bytes,
    struct aidoku_codec_bench_result *result, int explicit_native_opt_in);
void aidoku_png_bench_cancel(void);

int aidoku_jpeg_worker_query(void);
int aidoku_jpeg_worker_decode(const unsigned char *input, size_t input_bytes,
    uint32_t width, uint32_t height, uint32_t target_width, uint32_t target_height,
    unsigned char *output, size_t output_bytes, struct aidoku_codec_bench_result *result);

#endif
