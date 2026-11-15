/* Reuse the physically qualified queue and buffer-lifetime implementation.
 * This build exports only JPEG worker entry points. PNG trial entry points
 * and the command-line probe are not exported by the application library.
 */
#define AIDOKU_NATIVE_JPEG_WORKER_LIBRARY 1
#include "../../scripts/native-codec-probe.c"
