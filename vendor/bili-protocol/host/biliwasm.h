#ifndef BILIWASM_H
#define BILIWASM_H

#include <stddef.h>
#include <stdint.h>

#if defined(__GNUC__) || defined(__clang__)
#define BILIWASM_API __attribute__((visibility("default")))
#else
#define BILIWASM_API
#endif

#ifdef __cplusplus
extern "C" {
#endif

/* Returns 0 for a completed WASM callback, even when its result.error is set.
 * Returns 1 for a host failure. output_json may be NULL on an allocation or
 * argument failure; otherwise release it with biliwasm_free.
 * Each invocation creates and frees an isolated runtime. The library does
 * not use a browser, Node.js, network, cookies, shell, or temporary files.
 * Calls must be serialized: the cJSON dependency has process-global state.
 */
BILIWASM_API int biliwasm_run(const char *wasm_path, const char *request_json, char **output_json);
/* The caller owns immutable module bytes until this synchronous call returns,
 * including cleanup. Each call still owns a fresh environment/runtime/module.
 * Borrowed bytes are reserved against the existing host arena budget but are
 * never freed by the host. Calls have the same serialization requirement. */
BILIWASM_API int biliwasm_run_bytes(const uint8_t *wasm_bytes, size_t wasm_length,
    const char *request_json, char **output_json);
BILIWASM_API void biliwasm_free(char *output_json);

#ifdef __cplusplus
}
#endif
#endif
