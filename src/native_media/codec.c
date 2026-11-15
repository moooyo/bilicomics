/* SPDX-License-Identifier: GPL-3.0-or-later
 * Adapted from the AidokuForKindle native-media candidate dated 2026-11-15.
 * See vendor/aidoku-native-media/README.md for source identity and provenance.
 */
/* Reuse the physically qualified queue and buffer-lifetime implementation.
 * This build exports only JPEG worker entry points. PNG trial entry points
 * and the command-line probe are not exported by the application library.
 */
#define AIDOKU_NATIVE_JPEG_WORKER_LIBRARY 1
#include "../../scripts/native-codec-probe.c"
