/* SPDX-License-Identifier: GPL-3.0-or-later
 * Adapted in part from AidokuForKindle's 2026-11-15
 * docs/review/media-qualification-20261115/shutdown-contract-mock.c.
 * Source context: vendor/aidoku-native-media/README.md.
 * This host-only simulation never forwards an open or ioctl to a device.
 */
#define _POSIX_C_SOURCE 200809L
#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <stdarg.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <time.h>
#include <unistd.h>
#include <linux/videodev2.h>

#include "../native-codec-bench.h"

#define MOCK_FD 77
#define OUTPUT_BIT 1U
#define CAPTURE_BIT 2U

struct allocation {
    void *address;
    size_t bytes;
    int freed;
};

static const char *test_case;
static struct allocation allocations[4];
static unsigned int allocated, freed, capture_allocation_attempts;
static unsigned int opens, closes, ioctls, streaming, stop_calls, stop_errors;
static unsigned int source_requested, capture_requested, source_queued, capture_queued;
static unsigned int subscribed, parsed, source_event, capture_configured, capture_readbacks;
static unsigned int capture_plane_count, padding_checked, prefault_checked;
static int device_open, device_closed;
static uint64_t clock_ms;
static struct v4l2_format source_format, capture_format;
static struct v4l2_plane source_plane, capture_planes[3];
static const unsigned char *coded_input;
static size_t coded_length;
static unsigned int input_width = 1920, input_height = 2560;
static unsigned int target_width = 1860, target_height = 2480;

static void require(int condition, const char *reason) {
    if (!condition) {
        fprintf(stderr, "mock-contract-violation case=%s reason=%s\n", test_case, reason);
        fflush(stderr);
        _exit(95);
    }
}

static int scenario(const char *name) { return !strcmp(test_case, name); }

static int fail(const char *name) {
    if (!scenario(name)) return 0;
    errno = EIO;
    return 1;
}

static unsigned int align16(unsigned int value) { return (value + 15U) & ~15U; }
static unsigned int align64(unsigned int value) { return (value + 63U) & ~63U; }

static struct allocation *owned(void *address) {
    for (unsigned int i = 0; i < allocated; ++i)
        if (allocations[i].address == address) return &allocations[i];
    require(0, "every submitted or freed pointer belongs to the codec owner");
    return NULL;
}

static void all_buffers_live(void) {
    for (unsigned int i = 0; i < allocated; ++i)
        require(!allocations[i].freed, "keep all USERPTR storage through stream stop");
}

static int mock_posix_memalign(void **address, size_t alignment, size_t bytes) {
    if (!allocated && scenario("fail_allocate_source")) return ENOMEM;
    if (allocated) {
        ++capture_allocation_attempts;
        require(source_event && capture_configured && capture_readbacks,
                "allocate CAPTURE only after SOURCE_CHANGE and explicit layout readback");
        if (scenario("fail_allocate_capture_first") && capture_allocation_attempts == 1)
            return ENOMEM;
    }
    require(allocated < 4, "bounded plane allocations");
    require(alignment == (size_t)sysconf(_SC_PAGESIZE), "page-aligned USERPTR allocation");
    require(bytes && bytes % alignment == 0, "whole initialized private pages");
    int result = posix_memalign(address, alignment, bytes);
    if (!result) allocations[allocated++] = (struct allocation){*address, bytes, 0};
    return result;
}

static void mock_free(void *address) {
    if (!address) return;
    struct allocation *allocation = owned(address);
    require(!allocation->freed, "free each USERPTR allocation once");
    require(!streaming && (!device_open || device_closed),
            "free USERPTR only after successful stops and context close");
    allocation->freed = 1;
    ++freed;
    free(address);
}

static int mock_open(const char *path, int flags, ...) {
    (void)flags;
    require(!strcmp(path, "/dev/video1"), "only the mocked JDEC path may be opened");
    require(!device_open, "one decoder context per transaction");
    ++opens;
    if (scenario("fail_open")) { errno = ENOENT; return -1; }
    device_open = 1;
    return MOCK_FD;
}

static int mock_close(int fd) {
    require(fd == MOCK_FD && device_open && !device_closed, "close the owned mock context once");
    require(!streaming, "stop every active queue before context close");
    all_buffers_live();
    device_closed = 1;
    ++closes;
    if (scenario("fail_close")) { errno = EIO; return -1; }
    return 0;
}

static ssize_t mock_read(int fd, void *memory, size_t bytes) {
    (void)fd; (void)memory; (void)bytes;
    require(0, "resident FFI input must not perform file or device reads");
    errno = EBADF;
    return -1;
}

static int mock_fstat(int fd, struct stat *info) {
    (void)fd; (void)info;
    require(0, "resident FFI input must not inspect an external descriptor");
    errno = EBADF;
    return -1;
}

static int mock_clock_gettime(clockid_t kind, struct timespec *value) {
    require(kind == CLOCK_MONOTONIC, "only the monotonic deadline clock is used");
    ++clock_ms;
    value->tv_sec = (time_t)(clock_ms / 1000U);
    value->tv_nsec = (long)(clock_ms % 1000U) * 1000000L;
    return 0;
}

static int mock_poll(struct pollfd *fds, nfds_t count, int timeout) {
    require(count == 1 && fds[0].fd == MOCK_FD && device_open && !device_closed,
            "poll only the owned mock context");
    all_buffers_live();
    clock_ms += (unsigned int)timeout;
    return 0;
}

static int mock_nanosleep(const struct timespec *duration, struct timespec *remaining) {
    (void)remaining;
    require(device_open && !device_closed && streaming, "retry stops while retaining context");
    all_buffers_live();
    clock_ms += (uint64_t)duration->tv_sec * 1000U + (uint64_t)duration->tv_nsec / 1000000U;
    return 0;
}

static void initial_capture_header(void) {
    memset(&capture_format, 0, sizeof(capture_format));
    capture_format.type = V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE;
    struct v4l2_pix_format_mplane *p = &capture_format.fmt.pix_mp;
    p->width = source_format.fmt.pix_mp.width;
    p->height = source_format.fmt.pix_mp.height;
    p->pixelformat = V4L2_PIX_FMT_NV12;
    p->num_planes = 2;
    for (unsigned int i = 0; i < 2; ++i) {
        /* Reproduce the qualified driver's half-width default UV metadata.
         * The production transaction must renegotiate this before allocation.
         */
        p->plane_fmt[i].bytesperline = align16(i ? (p->width + 1U) / 2U : p->width);
        p->plane_fmt[i].sizeimage = align64(p->plane_fmt[i].bytesperline * p->height);
    }
}

static int mock_ioctl(int fd, unsigned long request, ...) {
    va_list arguments;
    va_start(arguments, request);
    void *argument = va_arg(arguments, void *);
    va_end(arguments);
    require(fd == MOCK_FD && device_open && !device_closed, "ioctl only the mock context");
    ++ioctls;
    if (request == VIDIOC_QUERYCAP) {
        if (fail("fail_querycap")) return -1;
        struct v4l2_capability *value = argument;
        memset(value, 0, sizeof(*value));
        strcpy((char *)value->card, scenario("query_bad_card") ? "unqualified decoder" : "mtk-jdec decoder");
        value->capabilities = V4L2_CAP_DEVICE_CAPS;
        value->device_caps = V4L2_CAP_STREAMING | V4L2_CAP_VIDEO_M2M_MPLANE;
        return 0;
    }
    if (request == VIDIOC_SUBSCRIBE_EVENT) {
        require(!parsed, "subscribe before source parsing");
        if (fail("fail_subscribe")) return -1;
        subscribed = 1;
        return 0;
    }
    if (request == VIDIOC_S_FMT) {
        struct v4l2_format *value = argument;
        if (V4L2_TYPE_IS_OUTPUT(value->type)) {
            require(!source_requested, "configure OUTPUT before requesting buffers");
            if (fail("fail_output_format")) return -1;
            source_format = *value;
        } else {
            require(source_event && !capture_requested, "configure CAPTURE after the source event");
            if (fail("fail_capture_format")) return -1;
            capture_format = *value;
            struct v4l2_pix_format_mplane *p = &capture_format.fmt.pix_mp;
            require(p->width == target_width && p->height == target_height, "preserve requested target dimensions");
            require(p->pixelformat == V4L2_PIX_FMT_GREY && p->num_planes == 1,
                    "the qualified production output is single-plane GREY");
            for (unsigned int i = 0; i < p->num_planes; ++i)
                require(p->plane_fmt[i].bytesperline >= target_width
                        && p->plane_fmt[i].sizeimage >= p->plane_fmt[i].bytesperline * target_height,
                        "the GREY plane covers full-width rows and the full height");
            if (scenario("invalid_capture_planes")) p->num_planes = 3;
            capture_configured = 1;
            capture_plane_count = p->num_planes;
            *value = capture_format;
        }
        return 0;
    }
    if (request == VIDIOC_G_FMT) {
        struct v4l2_format *value = argument;
        if (V4L2_TYPE_IS_OUTPUT(value->type)) {
            if (fail("fail_output_readback")) return -1;
            *value = source_format;
        } else {
            require(source_event, "observe CAPTURE only after SOURCE_CHANGE");
            if (!capture_configured && fail("fail_header_readback")) return -1;
            if (capture_configured) {
                ++capture_readbacks;
                if (capture_readbacks == 1 && fail("fail_capture_readback")) return -1;
                if (capture_readbacks == 3 && fail("fail_capture_queued_readback")) return -1;
            }
            *value = capture_format;
            if (scenario("changed_planes") && capture_requested) value->fmt.pix_mp.num_planes = 3;
        }
        return 0;
    }
    if (request == VIDIOC_REQBUFS) {
        struct v4l2_requestbuffers *value = argument;
        require(value->memory == V4L2_MEMORY_USERPTR, "use USERPTR on both queues");
        if (V4L2_TYPE_IS_OUTPUT(value->type)) {
            if (fail("fail_output_request")) return -1;
            source_requested = 1;
        } else {
            require(source_event && capture_configured && source_queued,
                    "request CAPTURE buffers after parsed source and explicit format");
            if (fail("fail_capture_request")) return -1;
            capture_requested = 1;
        }
        value->count = 1;
        return 0;
    }
    if (request == VIDIOC_QUERYBUF) {
        struct v4l2_buffer *value = argument;
        int output = V4L2_TYPE_IS_OUTPUT(value->type);
        if (fail(output ? "fail_output_query" : "fail_capture_query")) return -1;
        unsigned int count = output ? 1 : capture_format.fmt.pix_mp.num_planes;
        require(value->length >= count, "QUERYBUF plane array capacity");
        value->length = count;
        for (unsigned int i = 0; i < count; ++i)
            value->m.planes[i].length = output ? 16 : capture_format.fmt.pix_mp.plane_fmt[i].sizeimage;
        return 0;
    }
    if (request == VIDIOC_QBUF) {
        struct v4l2_buffer *value = argument;
        if (V4L2_TYPE_IS_OUTPUT(value->type)) {
            require(source_requested && value->length == 1 && !capture_requested,
                    "queue OUTPUT before any CAPTURE allocation");
            source_plane = value->m.planes[0];
            unsigned char *bytes = (void *)source_plane.m.userptr;
            struct allocation *allocation = owned(bytes);
            require(!allocation->freed && source_plane.length == allocation->bytes,
                    "advertise the owned OUTPUT capacity");
            require(source_plane.bytesused == align64((unsigned int)coded_length + 64U),
                    "include at least 64 zero bytes in the advertised aligned payload");
            require(!memcmp(bytes, coded_input, coded_length), "retain the exact encoded input");
            for (size_t i = coded_length; i < allocation->bytes; ++i)
                require(bytes[i] == 0, "initialize and prefault the entire OUTPUT allocation");
            padding_checked = 1;
            if (fail("fail_output_qbuf")) return -1;
            source_queued = 1;
        } else {
            require(source_event && capture_requested && value->length == capture_format.fmt.pix_mp.num_planes,
                    "queue the current CAPTURE plane count");
            for (unsigned int i = 0; i < value->length; ++i) {
                capture_planes[i] = value->m.planes[i];
                struct allocation *allocation = owned((void *)capture_planes[i].m.userptr);
                require(!allocation->freed && capture_planes[i].length == allocation->bytes,
                        "each CAPTURE plane has its own live allocation");
                for (size_t j = 0; j < allocation->bytes; ++j)
                    require(((unsigned char *)allocation->address)[j] == 0xa5,
                            "initialize and prefault every CAPTURE page before DMA");
            }
            prefault_checked = 1;
            if (fail("fail_capture_qbuf")) return -1;
            capture_queued = 1;
        }
        return 0;
    }
    if (request == VIDIOC_STREAMON) {
        enum v4l2_buf_type type = *(enum v4l2_buf_type *)argument;
        if (V4L2_TYPE_IS_OUTPUT(type)) {
            require(source_queued && subscribed, "start the subscribed OUTPUT header transaction");
            if (fail("fail_output_streamon")) return -1;
            streaming |= OUTPUT_BIT;
            parsed = 1;
            initial_capture_header();
        } else {
            require(parsed && source_event && capture_queued, "start CAPTURE only with parsed-header storage");
            if (fail("fail_capture_streamon")) return -1;
            streaming |= CAPTURE_BIT;
        }
        return 0;
    }
    if (request == VIDIOC_DQEVENT) {
        require(parsed && (streaming & OUTPUT_BIT), "source event follows OUTPUT parsing");
        if (fail("fail_source_event")) return -1;
        if (scenario("fail_source_timeout")) { errno = EAGAIN; return -1; }
        struct v4l2_event *value = argument;
        memset(value, 0, sizeof(*value));
        value->type = V4L2_EVENT_SOURCE_CHANGE;
        value->u.src_change.changes = V4L2_EVENT_SRC_CH_RESOLUTION;
        source_event = 1;
        return 0;
    }
    if (request == VIDIOC_DQBUF) {
        struct v4l2_buffer *value = argument;
        int output = V4L2_TYPE_IS_OUTPUT(value->type);
        require((streaming & 3U) == 3U, "complete buffers only after both streams started");
        if (fail(output ? "fail_output_dequeue" : "fail_capture_dequeue")) return -1;
        if (scenario(output ? "fail_output_timeout" : "fail_capture_timeout")) {
            errno = EAGAIN; return -1;
        }
        value->index = 0;
        value->flags = V4L2_BUF_FLAG_DONE;
        if (output) {
            value->length = 1;
            value->m.planes[0] = source_plane;
        } else {
            value->length = capture_format.fmt.pix_mp.num_planes;
            for (unsigned int i = 0; i < value->length; ++i) {
                unsigned int bytes = capture_format.fmt.pix_mp.plane_fmt[i].sizeimage;
                memset((void *)capture_planes[i].m.userptr, 128, bytes);
                value->m.planes[i] = capture_planes[i];
                value->m.planes[i].bytesused = bytes;
                if (scenario("oversized_payload")) value->m.planes[i].bytesused = capture_planes[i].length + 1;
                if (scenario("short_payload")) value->m.planes[i].bytesused = bytes - 1;
            }
            if (scenario("capture_error_flag")) value->flags |= V4L2_BUF_FLAG_ERROR;
        }
        return 0;
    }
    if (request == VIDIOC_STREAMOFF) {
        unsigned int bit = V4L2_TYPE_IS_OUTPUT(*(enum v4l2_buf_type *)argument) ? OUTPUT_BIT : CAPTURE_BIT;
        require(streaming & bit, "stop only an active stream");
        all_buffers_live();
        ++stop_calls;
        if (scenario("stop_eintr") && stop_calls == 1) { errno = EINTR; return -1; }
        if ((scenario("stop_io_once") && stop_calls == 1)
            || (scenario("stop_io_repeated") && stop_calls <= 3)) {
            ++stop_errors;
            errno = EIO;
            return -1;
        }
        streaming &= ~bit;
        return 0;
    }
    require(0, "every codec ioctl is handled by the host mock");
    errno = ENOTTY;
    return -1;
}

/* Interpose within this translation unit, including the actual production
 * worker entry points. No dynamic loader or real device fallback is present.
 */
#define open mock_open
#define close mock_close
#define read mock_read
#define fstat mock_fstat
#define ioctl mock_ioctl
#define poll mock_poll
#define posix_memalign mock_posix_memalign
#define free mock_free
#define clock_gettime mock_clock_gettime
#define nanosleep mock_nanosleep
#include "../../src/native_media/codec.c"
#undef open
#undef close
#undef read
#undef fstat
#undef ioctl
#undef poll
#undef posix_memalign
#undef free
#undef clock_gettime
#undef nanosleep

static size_t jpeg_markers(unsigned char *bytes, unsigned int width, unsigned int height,
                           unsigned int components, unsigned int sampling, int progressive) {
    /* These marker fixtures exercise the C header/transaction layer only.
     * Rust's complete entropy admission has separate tests; no decoder sees
     * these deliberately minimal host-only marker streams.
     */
    size_t n = 0;
#define BYTE(value) bytes[n++] = (unsigned char)(value)
    BYTE(0xff); BYTE(0xd8); BYTE(0xff); BYTE(progressive ? 0xc2 : 0xc0);
    BYTE(0); BYTE(8 + 3 * components); BYTE(8);
    BYTE(height >> 8); BYTE(height); BYTE(width >> 8); BYTE(width); BYTE(components);
    for (unsigned int i = 0; i < components; ++i) {
        BYTE(i + 1); BYTE(i ? 0x11 : sampling); BYTE(0);
    }
    BYTE(0xff); BYTE(0xda); BYTE(0); BYTE(6 + 2 * components); BYTE(components);
    for (unsigned int i = 0; i < components; ++i) { BYTE(i + 1); BYTE(0); }
    BYTE(0); BYTE(63); BYTE(0); BYTE(0); BYTE(0xff); BYTE(0xd9);
#undef BYTE
    return n;
}

int main(int argc, char **argv) {
    if (argc != 2) return 2;
    test_case = argv[1];
    int query = !strncmp(test_case, "query", 5);
    int status;
    struct aidoku_codec_bench_result report = {0};
    if (query) {
        status = aidoku_jpeg_worker_query();
        require((status == 0) == scenario("query"), "QUERYCAP admission result");
    } else {
        unsigned char input[128];
        /* Reserve enough owned caller output for the full qualified target
         * and the nearby rejected identity/upscale requests. This buffer is
         * outside the interposed USERPTR allocations and never owns DMA.
         */
        size_t output_capacity = (size_t)1920 * 2560;
        unsigned char *output = malloc(output_capacity);
        require(output != NULL, "allocate the bounded caller-owned Gray8 output");
        unsigned int width = 1920, height = 2560;
        /* Every lifecycle/error scenario must enter the currently qualified
         * grayscale transaction. Color cases exercise pre-device rejection.
         */
        unsigned int components = 1, sampling = 0x11;
        if (scenario("reject_422") || scenario("reject_444") || scenario("reject_420")
            || scenario("reject_odd_color") || scenario("reject_odd_color_target")) {
            components = 3;
            sampling = scenario("reject_444") ? 0x11 : 0x21;
        }
        if (scenario("gray_wide")) { height = 1280; target_height = 1240; }
        if (scenario("reject_420")) sampling = 0x22;
        /* Odd color dimensions are also outside the qualified tuple set;
         * these cases assert zero device access, not an isolated parity gate.
         */
        if (scenario("reject_odd_color")) width = 1921;
        if (scenario("reject_unchanged")) { target_width = width; target_height = height; }
        if (scenario("reject_upscale_width")) target_width = width + 1;
        if (scenario("reject_upscale_height")) target_height = height + 1;
        if (scenario("reject_source_pixels")) { width = 2048; height = 4096; }
        if (scenario("reject_odd_color_target") || scenario("reject_gray_odd_target")) target_width = 1859;
        if (scenario("reject_faulting_tall")) { width = 1280; target_width = 1240; }
        if (scenario("reject_unqualified_source")) {
            width = 1600; height = 2400; target_width = 1550; target_height = 2325;
        }
        if (scenario("reject_unqualified_target")) { target_width = 1800; target_height = 2400; }
        if (scenario("reject_reference_target")) { target_width = 930; target_height = 1240; }
        input_width = width;
        input_height = height;
        coded_length = jpeg_markers(input, width, height, components, sampling, scenario("reject_progressive"));
        if (scenario("reject_png")) {
            memcpy(input, "\x89PNG\r\n\x1a\n", 8);
            coded_length = 8;
        }
        coded_input = input;
        memset(output, 0xcc, output_capacity);
        status = aidoku_jpeg_worker_decode(input, coded_length, width, height,
                                           target_width, target_height, output, output_capacity, &report);
        int success = scenario("gray") || scenario("gray_wide") || scenario("stop_eintr");
        require((status == 0) == success, "codec result matches the injected transaction");
        require(report.abi_version == 1 && report.structure_bytes == 112, "preserve the C FFI report ABI");
        require(report.output_bytes == (success ? target_width * target_height : 0), "publish only successful output");
        if (success) {
            require(report.width == target_width && report.height == target_height,
                    "preserve the exact qualified target in the completed report");
            require(padding_checked && prefault_checked, "verify OUTPUT padding and every prefaulted CAPTURE plane");
            require(capture_plane_count == 1, "publish only the qualified single-plane GREY output");
            for (unsigned int i = 0; i < report.output_bytes; ++i)
                require(output[i] == 128, "normalize the owned GREY result");
        }
        if (!strncmp(test_case, "reject_", 7) || scenario("fail_allocate_source"))
            require(!opens && !ioctls, "reject unsupported inputs before opening a video context");
        if (scenario("stop_eintr"))
            require(stop_calls == 3 && !report.cleanup_error, "retry interrupted STREAMOFF without a false failure");
        if (scenario("stop_io_once") || scenario("stop_io_repeated"))
            require(stop_errors == (scenario("stop_io_once") ? 1U : 3U)
                    && report.cleanup_error == EIO && stop_calls == stop_errors + 2,
                    "retain buffers through failed stops and latch the cleanup error after safe completion");
        free(output);
    }
    require(allocated == freed, "release every allocation after a safe fence");
    require(!streaming, "leave no active mock stream");
    require(!device_open || (device_closed && closes == 1), "close every opened mock context");
    printf("{\"case\":\"%s\",\"status\":%d,\"video_open_attempts\":%u,\"mock_ioctls\":%u,"
           "\"allocations\":%u,\"frees\":%u,\"stop_calls\":%u,\"capture_planes\":%u,"
           "\"source_width\":%u,\"source_height\":%u,\"target_width\":%u,\"target_height\":%u,"
           "\"physical_device_opened\":false,\"native_ioctl_executed\":false}\n",
           test_case, status, opens, ioctls, allocated, freed, stop_calls, capture_plane_count,
           input_width, input_height, target_width, target_height);
    return 0;
}
