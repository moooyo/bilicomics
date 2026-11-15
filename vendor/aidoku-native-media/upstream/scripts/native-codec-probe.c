/* Reviewed codec transaction shared by the trial and isolated JPEG worker.
 * The retired common media probe continues to reject JPEG/PNG execution.
 * Run only through a fresh physical-device lease after worker-side checks.
 * Capture allocation follows the first parsed header, not a guessed layout.
 * USERPTR pages stay allocated until both streams and the context are closed.
 * Driver job_abort is not a bounded cancellation contract.
 */
#define _POSIX_C_SOURCE 200809L
#include <errno.h>
#include <fcntl.h>
#include <linux/videodev2.h>
#include <poll.h>
#include <signal.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/stat.h>
#include <time.h>
#include <unistd.h>

#define MAX_PLANES 3U
#define MAX_PLANE_BYTES (32U * 1024U * 1024U)
#define MAX_CODED_BYTES (2U * 1024U * 1024U)
#define PNG_FOURCC v4l2_fourcc('P', 'N', 'G', ' ')
#define KINDLE_AYUV_FOURCC v4l2_fourcc('A', 'Y', 'U', 'V')

#if defined(AIDOKU_CODEC_BENCH_LIBRARY) || defined(AIDOKU_NATIVE_JPEG_WORKER_LIBRARY)
#include "native-codec-bench.h"
_Static_assert(sizeof(struct aidoku_codec_bench_result) == 112, "Benchmark ABI mismatch");
struct benchmark_call {
    const unsigned char *input;
    size_t input_bytes;
    unsigned char *output;
    size_t output_bytes;
    struct aidoku_codec_bench_result *result;
};
static struct benchmark_call *active_benchmark;
#define BENCHMARK_ACTIVE (active_benchmark != NULL)
#else
#define BENCHMARK_ACTIVE 0
#endif

struct memory_plane {
    void *address;
    unsigned int capacity;
    unsigned int payload;
};

struct queue {
    enum v4l2_buf_type type;
    struct v4l2_format format;
    struct memory_plane planes[MAX_PLANES];
    unsigned int count;
    unsigned int flags;
    int streaming;
};

static volatile sig_atomic_t stop_requested;
static const char *stage = "arguments";

static void on_signal(int signal_number) {
    (void)signal_number;
    stop_requested = 1;
}

static void step(const char *value) {
    stage = value;
    if (!BENCHMARK_ACTIVE) {
        fprintf(stderr, "codec-step %s\n", value);
        fflush(stderr);
    }
}

static double now_ms(void) {
    struct timespec value;
    clock_gettime(CLOCK_MONOTONIC, &value);
    return value.tv_sec * 1000.0 + value.tv_nsec / 1000000.0;
}

/* Match the pinned zune-jpeg 0.5.15 scalar BT.601 conversion before applying
 * the Reader's grayscale weights. Clipping RGB first is significant.
 */
static unsigned int clip_channel(int value) {
    return value < 0 ? 0U : value > 255 ? 255U : (unsigned int)value;
}

static unsigned char reader_jpeg_gray(unsigned int y, unsigned int u, unsigned int v) {
    int base = (int)y * 16384 + 8191;
    int cb = (int)u - 128, cr = (int)v - 128;
    unsigned int r = clip_channel((base + cr * 22970) >> 14);
    unsigned int g = clip_channel((base - cr * 11700 - cb * 5638) >> 14);
    unsigned int b = clip_channel((base + cb * 29032) >> 14);
    return (unsigned char)((54U * r + 183U * g + 19U * b + 128U) >> 8);
}

static unsigned int number(const char *text) {
    char *end = NULL;
    unsigned long value = strtoul(text, &end, 10);
    return text[0] && end && !*end && value <= 4096 ? (unsigned int)value : 0;
}

enum jpeg_coding {
    JPEG_HEADER_INVALID,
    JPEG_HEADER_BASELINE,
    JPEG_HEADER_PROGRESSIVE,
    JPEG_HEADER_UNQUALIFIED
};

struct jpeg_header {
    enum jpeg_coding coding;
    unsigned int width;
    unsigned int height;
    unsigned int components;
    unsigned int horizontal[3];
    unsigned int vertical[3];
};

enum png_admission {
    PNG_HEADER_INVALID,
    PNG_HEADER_ALLOWED,
    PNG_UNSAFE_COLOR_KEY
};

static uint32_t big_endian_u32(const unsigned char *data) {
    return ((uint32_t)data[0] << 24) | ((uint32_t)data[1] << 16)
        | ((uint32_t)data[2] << 8) | data[3];
}

/* The PNG M2M callback dereferences USERPTR for non-palette tRNS without a
 * protected userspace read or a vb2 kernel mapping. Never submit that branch.
 * Inspect chunk boundaries so an embedded marker in IDAT/text is not a key.
 * This structural guard does not replace independent fixture validation.
 */
static enum png_admission inspect_png(const unsigned char *data, size_t size,
                                     unsigned int width, unsigned int height) {
    if (size < 33 || memcmp(data, "\x89PNG\r\n\x1a\n", 8))
        return PNG_HEADER_INVALID;
    size_t position = 8;
    unsigned int color = 255, palette_entries = 0;
    int header_seen = 0, data_seen = 0;
    while (size - position >= 12) {
        uint32_t length = big_endian_u32(data + position);
        if (length > size - position - 12) return PNG_HEADER_INVALID;
        const unsigned char *type = data + position + 4;
        const unsigned char *payload = data + position + 8;
        if (!memcmp(type, "IHDR", 4)) {
            if (header_seen || position != 8 || length != 13
                || big_endian_u32(payload) != width
                || big_endian_u32(payload + 4) != height)
                return PNG_HEADER_INVALID;
            unsigned int depth = payload[8];
            color = payload[9];
            int standard_depth = color == 0
                ? depth == 1 || depth == 2 || depth == 4 || depth == 8 || depth == 16
                : color == 3 ? depth == 1 || depth == 2 || depth == 4 || depth == 8
                : (color == 2 || color == 4 || color == 6) && (depth == 8 || depth == 16);
            if (!standard_depth || payload[10] || payload[11] || payload[12] > 1)
                return PNG_HEADER_INVALID;
            header_seen = 1;
        } else if (!header_seen) {
            return PNG_HEADER_INVALID;
        } else if (!memcmp(type, "PLTE", 4)) {
            if (data_seen || !length || length > 768 || length % 3)
                return PNG_HEADER_INVALID;
            palette_entries = length / 3;
        } else if (!memcmp(type, "tRNS", 4)) {
            if (color != 3) return PNG_UNSAFE_COLOR_KEY;
            if (data_seen || !length || length > palette_entries)
                return PNG_HEADER_INVALID;
        } else if (!memcmp(type, "IDAT", 4)) {
            if (color == 3 && !palette_entries) return PNG_HEADER_INVALID;
            data_seen = 1;
        } else if (!memcmp(type, "IEND", 4)) {
            return data_seen && !length ? PNG_HEADER_ALLOWED : PNG_HEADER_INVALID;
        }
        position += (size_t)length + 12;
    }
    return PNG_HEADER_INVALID;
}

/* Inspect marker boundaries, not arbitrary FF C2 bytes in metadata or entropy.
 * This admission check stops at the first scan; it is not a JPEG decoder.
 */
static struct jpeg_header inspect_jpeg_header(const unsigned char *data, size_t size) {
    struct jpeg_header result = {0};
    if (size < 4 || data[0] != 0xff || data[1] != 0xd8) return result;
    size_t position = 2;
    int frame_seen = 0;
    while (position < size) {
        if (data[position++] != 0xff) break;
        while (position < size && data[position] == 0xff) ++position;
        if (position == size) break;
        unsigned int marker = data[position++];
        if (marker == 0x01) continue;
        if (marker == 0 || marker == 0xd8 || marker == 0xd9
            || (marker >= 0xd0 && marker <= 0xd7) || size - position < 2) break;
        size_t length = ((size_t)data[position] << 8) | data[position + 1];
        if (length < 2 || length > size - position) break;
        if (marker == 0xda) {
            if (frame_seen) return result;
            break;
        }
        if (marker >= 0xc0 && marker <= 0xcf
            && marker != 0xc4 && marker != 0xc8 && marker != 0xcc) {
            if (frame_seen || length < 8) break;
            unsigned int components = data[position + 7];
            if (!components || length != 8U + 3U * components) break;
            frame_seen = 1;
            result.height = ((unsigned int)data[position + 3] << 8) | data[position + 4];
            result.width = ((unsigned int)data[position + 5] << 8) | data[position + 6];
            if (!result.width || !result.height) break;
            result.components = components;
            for (unsigned int i = 0; i < components && i < 3; ++i) {
                unsigned int sampling = data[position + 9 + 3 * i];
                result.horizontal[i] = sampling >> 4;
                result.vertical[i] = sampling & 15;
            }
            if (marker == 0xc2 || marker == 0xc6 || marker == 0xca || marker == 0xce)
                result.coding = JPEG_HEADER_PROGRESSIVE;
            else if (marker == 0xc0 && data[position + 2] == 8
                && (components == 1 || components == 3))
                result.coding = JPEG_HEADER_BASELINE;
            else result.coding = JPEG_HEADER_UNQUALIFIED;
        }
        position += length;
    }
    result.coding = JPEG_HEADER_INVALID;
    return result;
}

static int allocate_plane(struct memory_plane *plane, unsigned int required,
                          unsigned char sentinel) {
    long page = sysconf(_SC_PAGESIZE);
    if (page < 4096 || page > 65536 || (page & (page - 1)) || !required
        || required > MAX_PLANE_BYTES - 2U * (unsigned int)page) {
        errno = EOVERFLOW; return -1;
    }
    unsigned int bytes = (required + (unsigned int)page - 1U)
        & ~((unsigned int)page - 1U);
    bytes += (unsigned int)page;
    int result = posix_memalign(&plane->address, (size_t)page, bytes);
    if (result) { errno = result; return -1; }
    plane->capacity = bytes;
    /* Prefault private pages before the driver pins and maps them for DMA. */
    memset(plane->address, sentinel, bytes);
    return 0;
}

static int read_exact(int fd, void *memory, size_t size) {
    unsigned char *position = memory;
    while (size) {
        ssize_t count = read(fd, position, size);
        if (count < 0 && errno == EINTR && !stop_requested) continue;
        if (count <= 0) { if (!count) errno = EIO; return -1; }
        position += count; size -= (size_t)count;
    }
    return 0;
}

static int identify(int fd, int jpeg) {
    struct v4l2_capability value;
    memset(&value, 0, sizeof(value));
    if (ioctl(fd, VIDIOC_QUERYCAP, &value) < 0) return -1;
    /* The JPEG driver name is truncated in the 16-byte driver field. */
    if (!memchr(value.card, '\0', sizeof(value.card))
        || strcmp((const char *)value.card, jpeg ? "mtk-jdec decoder" : "mtk-png decoder")) {
        errno = ENODEV; return -1;
    }
    unsigned int caps = value.capabilities & V4L2_CAP_DEVICE_CAPS
        ? value.device_caps : value.capabilities;
    unsigned int expected = V4L2_CAP_STREAMING | V4L2_CAP_VIDEO_M2M_MPLANE;
    if ((caps & expected) != expected) { errno = ENODEV; return -1; }
    return 0;
}

static int get_format(int fd, enum v4l2_buf_type type, struct v4l2_format *value) {
    memset(value, 0, sizeof(*value));
    value->type = type;
    return ioctl(fd, VIDIOC_G_FMT, value);
}

static int same_layout(const struct v4l2_format *first, const struct v4l2_format *second) {
    const struct v4l2_pix_format_mplane *a = &first->fmt.pix_mp;
    const struct v4l2_pix_format_mplane *b = &second->fmt.pix_mp;
    if (a->width != b->width || a->height != b->height
        || a->pixelformat != b->pixelformat || a->num_planes != b->num_planes
        || !a->num_planes || a->num_planes > MAX_PLANES) return 0;
    for (unsigned int i = 0; i < a->num_planes; ++i)
        if (a->plane_fmt[i].sizeimage != b->plane_fmt[i].sizeimage
            || a->plane_fmt[i].bytesperline != b->plane_fmt[i].bytesperline) return 0;
    return 1;
}

static int capture_layout(const struct v4l2_format *value, unsigned int width,
                          unsigned int height, int jpeg, int gray) {
    const struct v4l2_pix_format_mplane *p = &value->fmt.pix_mp;
    unsigned int count = jpeg && !gray ? 2U : 1U;
    unsigned int pixel = jpeg ? (gray ? V4L2_PIX_FMT_GREY : V4L2_PIX_FMT_NV12)
        : V4L2_PIX_FMT_ARGB32;
    if (p->width != width || p->height != height || p->pixelformat != pixel
        || p->num_planes != count) { errno = EPROTO; return -1; }
    for (unsigned int i = 0; i < count; ++i) {
        const struct v4l2_plane_pix_format *plane = &p->plane_fmt[i];
        unsigned int minimum_pitch = jpeg ? (i ? (width + 1U) / 2U : width) : width * 4U;
        if (plane->bytesperline < minimum_pitch || plane->bytesperline % 16
            || !plane->sizeimage || plane->sizeimage > MAX_PLANE_BYTES - 131072U
            || (uint64_t)plane->bytesperline * height > plane->sizeimage) {
            errno = EOVERFLOW; return -1;
        }
    }
    return 0;
}

/* Explicit YUV negotiation uses byte pitches for interleaved chroma rows. */
static int explicit_yuv_layout(const struct v4l2_format *value, unsigned int width,
                               unsigned int height, unsigned int pixel) {
    const struct v4l2_pix_format_mplane *p = &value->fmt.pix_mp;
    unsigned int count = pixel == KINDLE_AYUV_FOURCC ? 1U : 2U;
    if (p->width != width || p->height != height || p->pixelformat != pixel
        || p->num_planes != count) { errno = EPROTO; return -1; }
    for (unsigned int i = 0; i < count; ++i) {
        unsigned int rows = pixel == V4L2_PIX_FMT_NV12 && i == 1 ? (height + 1U) / 2U : height;
        unsigned int minimum = pixel == KINDLE_AYUV_FOURCC ? width * 4U
            : i ? ((width + 1U) / 2U) * 2U : width;
        const struct v4l2_plane_pix_format *plane = &p->plane_fmt[i];
        if (plane->bytesperline < minimum || plane->bytesperline % 16
            || !plane->sizeimage || plane->sizeimage > MAX_PLANE_BYTES - 131072U
            || (uint64_t)plane->bytesperline * rows > plane->sizeimage) {
            errno = EOVERFLOW; return -1;
        }
    }
    return 0;
}

static int request_queue(int fd, struct queue *queue) {
    struct v4l2_requestbuffers request = {.type = queue->type,
        .memory = V4L2_MEMORY_USERPTR, .count = 1};
    if (ioctl(fd, VIDIOC_REQBUFS, &request) < 0) return -1;
    if (!request.count) { errno = ENOMEM; return -1; }
    struct v4l2_plane planes[MAX_PLANES];
    memset(planes, 0, sizeof(planes));
    struct v4l2_buffer query = {.type = queue->type, .memory = V4L2_MEMORY_USERPTR,
        .index = 0, .length = MAX_PLANES, .m.planes = planes};
    if (ioctl(fd, VIDIOC_QUERYBUF, &query) < 0) return -1;
    if (query.length != queue->count) { errno = EPROTO; return -1; }
    for (unsigned int i = 0; i < queue->count; ++i)
        if (planes[i].length > queue->planes[i].capacity) { errno = EOVERFLOW; return -1; }
    return 0;
}

static int queue_buffer(int fd, struct queue *queue, unsigned int coded_bytes) {
    struct v4l2_plane planes[MAX_PLANES];
    memset(planes, 0, sizeof(planes));
    for (unsigned int i = 0; i < queue->count; ++i) {
        if (coded_bytes > queue->planes[i].capacity) { errno = EOVERFLOW; return -1; }
        planes[i].m.userptr = (unsigned long)queue->planes[i].address;
        planes[i].length = queue->planes[i].capacity;
        planes[i].bytesused = coded_bytes;
    }
    struct v4l2_buffer value = {.type = queue->type, .memory = V4L2_MEMORY_USERPTR,
        .index = 0, .length = queue->count, .m.planes = planes};
    return ioctl(fd, VIDIOC_QBUF, &value);
}

static int stream_on(int fd, struct queue *queue) {
    if (ioctl(fd, VIDIOC_STREAMON, &queue->type) < 0) return -1;
    queue->streaming = 1;
    return 0;
}

/* An unsuccessful STREAMOFF is not permission to close a running GREY
 * context: its release routine frees temporary chroma before waiting for work.
 * Keep every buffer and the context alive until the stop actually completes.
 * The external controller may stop waiting, but must not kill this owner.
 */
static int stop_queue(int fd, struct queue *queue) {
    int first_error = 0;
    if (!queue->streaming) return 0;
    for (;;) {
        if (ioctl(fd, VIDIOC_STREAMOFF, &queue->type) == 0) {
            queue->streaming = 0;
            if (first_error) { errno = first_error; return -1; }
            return 0;
        }
        if (errno == EINTR) continue;
        if (!first_error) {
            first_error = errno ? errno : EIO;
            fprintf(stderr, "codec-streamoff-pending errno=%d; retaining context and buffers\n",
                    first_error);
            fflush(stderr);
        }
        struct timespec pause = {.tv_sec = 0, .tv_nsec = 100000000};
        nanosleep(&pause, NULL);
    }
}

static int source_change(int fd) {
    double deadline = now_ms() + 4000.0;
    while (!stop_requested && now_ms() < deadline) {
        struct v4l2_event event;
        memset(&event, 0, sizeof(event));
        if (ioctl(fd, VIDIOC_DQEVENT, &event) == 0) {
            if (event.type == V4L2_EVENT_SOURCE_CHANGE
                && event.u.src_change.changes & V4L2_EVENT_SRC_CH_RESOLUTION) return 0;
        } else if (errno != EAGAIN && errno != EINTR) return -1;
        struct pollfd wait = {.fd = fd, .events = POLLPRI | POLLERR};
        if (poll(&wait, 1, 50) < 0 && errno != EINTR) return -1;
    }
    errno = stop_requested ? EINTR : ETIMEDOUT; return -1;
}

static int dequeue(int fd, struct queue *queue) {
    double deadline = now_ms() + 5000.0;
    while (!stop_requested && now_ms() < deadline) {
        struct v4l2_plane planes[MAX_PLANES];
        memset(planes, 0, sizeof(planes));
        struct v4l2_buffer value = {.type = queue->type, .memory = V4L2_MEMORY_USERPTR,
            .length = queue->count, .m.planes = planes};
        if (ioctl(fd, VIDIOC_DQBUF, &value) == 0) {
            if (value.index != 0 || value.length != queue->count) { errno = EPROTO; return -1; }
            queue->flags = value.flags;
            for (unsigned int i = 0; i < queue->count; ++i) {
                if (planes[i].bytesused > queue->planes[i].capacity) { errno = EOVERFLOW; return -1; }
                queue->planes[i].payload = planes[i].bytesused;
            }
            if (value.flags & V4L2_BUF_FLAG_ERROR) { errno = EIO; return -1; }
            return 0;
        }
        if (errno != EAGAIN && errno != EINTR) return -1;
        struct pollfd wait = {.fd = fd, .events = POLLIN | POLLERR};
        if (poll(&wait, 1, 50) < 0 && errno != EINTR) return -1;
    }
    errno = stop_requested ? EINTR : ETIMEDOUT; return -1;
}

static int save_plane(const char *prefix, unsigned int index, const struct memory_plane *plane) {
    char path[512];
    int length = snprintf(path, sizeof(path), "%s.plane%u.bin", prefix, index);
    if (length <= 0 || (size_t)length >= sizeof(path)) { errno = ENAMETOOLONG; return -1; }
    int fd = open(path, O_WRONLY | O_CREAT | O_EXCL | O_CLOEXEC | O_NOFOLLOW, 0600);
    if (fd < 0) return -1;
    const unsigned char *position = plane->address;
    size_t remaining = plane->payload;
    while (remaining) {
        ssize_t size = write(fd, position, remaining);
        if (size < 0 && errno == EINTR) continue;
        if (size <= 0) { int error = errno ? errno : EIO; close(fd); errno = error; return -1; }
        position += size; remaining -= (size_t)size;
    }
    return close(fd);
}

static int run_probe(int argc, char **argv) {
    const char *opt_in = getenv("AIDOKU_REVIEWED_CODEC_TRIAL");
    int no_output = argc > 1 && !strcmp(argv[argc - 1], "--no-output");
    int arguments = argc - no_output;
    if ((arguments != 8 && arguments != 10) || strcmp(argv[1], "--reviewed-single-frame")
        || (!BENCHMARK_ACTIVE && (!opt_in || strcmp(opt_in, "1")))) {
        fprintf(stderr, "Use a prepared leased single-frame trial with explicit opt-in.\n");
        return 2;
    }
    int jpeg = !strcmp(argv[2], "jpeg"), png = !strcmp(argv[2], "png");
    int gray = !strcmp(argv[7], "gray");
    unsigned int yuv_pixel = !strcmp(argv[7], "nv12") ? V4L2_PIX_FMT_NV12
        : !strcmp(argv[7], "nv16") ? V4L2_PIX_FMT_NV16
        : !strcmp(argv[7], "ayuv") ? KINDLE_AYUV_FOURCC : 0;
    unsigned int width = number(argv[5]), height = number(argv[6]);
    unsigned int target_width = arguments == 10 ? number(argv[8]) : width;
    unsigned int target_height = arguments == 10 ? number(argv[9]) : height;
    if ((!jpeg && !png) || (!gray && !yuv_pixel && strcmp(argv[7], "auto")) || (png && (gray || yuv_pixel))
        || width < 16 || width > 2048 || height < 16 || height > 4096
        || target_width < 16 || target_width > 2048 || target_height < 16 || target_height > 4096
        || (uint64_t)width * height > 8U * 1024U * 1024U
        || ((width != target_width || height != target_height)
            && (!jpeg || (!gray && yuv_pixel != V4L2_PIX_FMT_NV12 && yuv_pixel != V4L2_PIX_FMT_NV16)))
        || (yuv_pixel && (target_width & 1U))
        || (no_output && jpeg && !gray && yuv_pixel != V4L2_PIX_FMT_NV16
            && yuv_pixel != KINDLE_AYUV_FOURCC)) return 2;
    if (!BENCHMARK_ACTIVE) {
        signal(SIGTERM, on_signal);
        signal(SIGINT, on_signal);
        signal(SIGHUP, on_signal);
    }
    int fd = -1, input = -1, error = 0, cleanup_error = 0, header_seen = 0;
    unsigned int source_events = 0, coded_bytes = 0, submitted_bytes = 0;
    struct queue output = {.type = V4L2_BUF_TYPE_VIDEO_OUTPUT_MPLANE, .count = 1};
    struct queue capture = {.type = V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE};
    struct v4l2_format header_format = {0}, recheck = {0};
    double began = now_ms(), decode_started = 0.0, decode_finished = 0.0;
    double input_ready = 0.0, normalized_ready = 0.0, cleanup_ms = 0.0;
    unsigned char *normalized = NULL;
    uint32_t normalized_checksum = UINT32_C(2166136261);
    struct stat info;
    step("read_input");
#if defined(AIDOKU_CODEC_BENCH_LIBRARY) || defined(AIDOKU_NATIVE_JPEG_WORKER_LIBRARY)
    if (BENCHMARK_ACTIVE) {
        coded_bytes = (unsigned int)active_benchmark->input_bytes;
    } else
#endif
    {
    input = open(argv[3], O_RDONLY | O_CLOEXEC | O_NOFOLLOW);
    if (input < 0 || fstat(input, &info) < 0 || !S_ISREG(info.st_mode)
        || info.st_size <= 0 || info.st_size > MAX_CODED_BYTES) goto failed;
    coded_bytes = (unsigned int)info.st_size;
    }
    /* The exact JDEC driver programs FIFO end in 64-byte units and its write
     * pointer in 16-byte units. Controlled Scribe trials need a zero tail
     * included in bytesused, not only extra unadvertised allocation capacity.
     * The original JPEG file and its decoded image remain unchanged.
     */
    submitted_bytes = jpeg ? (coded_bytes + 64U + 63U) & ~63U : coded_bytes;
    if (allocate_plane(&output.planes[0], submitted_bytes, 0) < 0) goto failed;
#if defined(AIDOKU_CODEC_BENCH_LIBRARY) || defined(AIDOKU_NATIVE_JPEG_WORKER_LIBRARY)
    if (BENCHMARK_ACTIVE) {
        memcpy(output.planes[0].address, active_benchmark->input, coded_bytes);
    } else
#endif
    {
        if (read_exact(input, output.planes[0].address, coded_bytes) < 0) goto failed;
        close(input); input = -1;
    }
    if (jpeg) {
        step("check_jpeg_header");
        struct jpeg_header header = inspect_jpeg_header(output.planes[0].address, coded_bytes);
        if (header.coding == JPEG_HEADER_PROGRESSIVE) {
            step("unsupported_progressive_jpeg");
            errno = ENOTSUP; goto failed;
        }
        if (header.coding != JPEG_HEADER_BASELINE) {
            step(header.coding == JPEG_HEADER_INVALID
                ? "invalid_jpeg_header" : "unsupported_jpeg_coding");
            errno = header.coding == JPEG_HEADER_INVALID ? EINVAL : ENOTSUP;
            goto failed;
        }
        if (header.width != width || header.height != height) {
            step("jpeg_dimensions_mismatch");
            errno = EINVAL; goto failed;
        }
        /* Page-sized 4:2:0 trials produced IOMMU read faults despite correct
         * visible pixels. Small vertically subsampled trials do not qualify
         * those inputs for further hardware use. Retain software fallback.
         */
        int sampling_allowed = header.components == 1
            ? header.horizontal[0] == 1 && header.vertical[0] == 1
            : header.components == 3
                && (header.horizontal[0] == 1 || header.horizontal[0] == 2)
                && header.horizontal[1] == 1 && header.horizontal[2] == 1
                && header.vertical[0] == 1 && header.vertical[1] == 1
                && header.vertical[2] == 1;
        if (!sampling_allowed) {
            step("unsupported_jpeg_sampling");
            errno = ENOTSUP; goto failed;
        }
        if (header.components == 3 && (width & 1U)) {
            step("unsupported_jpeg_odd_color_width");
            errno = ENOTSUP; goto failed;
        }
    } else {
        step("check_png_header");
        enum png_admission admission = inspect_png(output.planes[0].address,
                                                   coded_bytes, width, height);
        if (admission != PNG_HEADER_ALLOWED) {
            step(admission == PNG_UNSAFE_COLOR_KEY
                 ? "unsupported_png_color_key_transparency" : "invalid_png_header");
            errno = admission == PNG_UNSAFE_COLOR_KEY ? ENOTSUP : EINVAL;
            goto failed;
        }
    }
    input_ready = now_ms();
    step("identify_decoder");
    fd = open(jpeg ? "/dev/video1" : "/dev/video6", O_RDWR | O_NONBLOCK | O_CLOEXEC);
    if (fd < 0 || identify(fd, jpeg) < 0) goto failed;
    if (jpeg) {
        step("subscribe_source_change");
        struct v4l2_event_subscription event = {.type = V4L2_EVENT_SOURCE_CHANGE};
        if (ioctl(fd, VIDIOC_SUBSCRIBE_EVENT, &event) < 0) goto failed;
    }
    step("set_output_only");
    output.format.type = output.type;
    struct v4l2_pix_format_mplane *p = &output.format.fmt.pix_mp;
    p->width = width; p->height = height;
    p->pixelformat = jpeg ? V4L2_PIX_FMT_JPEG : PNG_FOURCC;
    p->field = V4L2_FIELD_NONE; p->num_planes = 1;
    p->colorspace = V4L2_COLORSPACE_JPEG;
    p->quantization = V4L2_QUANTIZATION_FULL_RANGE;
    p->plane_fmt[0].sizeimage = output.planes[0].capacity;
    if (ioctl(fd, VIDIOC_S_FMT, &output.format) < 0
        || get_format(fd, output.type, &recheck) < 0) goto failed;
    if (!same_layout(&output.format, &recheck)) { errno = EPROTO; goto failed; }
    step("allocate_output_queue");
    if (request_queue(fd, &output) < 0) goto failed;
    step("queue_first_header");
    if (queue_buffer(fd, &output, submitted_bytes) < 0) goto failed;
    step("stream_output_parse_header");
    if (stream_on(fd, &output) < 0) goto failed;
    if (jpeg) {
        step("receive_source_change");
        if (source_change(fd) < 0) goto failed;
        source_events = 1;
    }
    step("read_parsed_capture_layout");
    if (get_format(fd, capture.type, &header_format) < 0
        || capture_layout(&header_format, width, height, jpeg, 0) < 0) goto failed;
    header_seen = 1;
    capture.format = header_format;
    if (gray) {
        step("select_gray_after_header");
        p = &capture.format.fmt.pix_mp;
        memset(p->plane_fmt, 0, sizeof(p->plane_fmt));
        p->pixelformat = V4L2_PIX_FMT_GREY; p->num_planes = 1;
        p->width = target_width; p->height = target_height;
        p->plane_fmt[0].bytesperline = (target_width + 15U) & ~15U;
        p->plane_fmt[0].sizeimage = (p->plane_fmt[0].bytesperline * target_height + 63U) & ~63U;
        if (ioctl(fd, VIDIOC_S_FMT, &capture.format) < 0
            || get_format(fd, capture.type, &recheck) < 0) goto failed;
        if (!same_layout(&capture.format, &recheck)) { errno = EPROTO; goto failed; }
        capture.format = recheck;
    }
    if (yuv_pixel) {
        step("select_yuv_after_header");
        p = &capture.format.fmt.pix_mp;
        memset(p->plane_fmt, 0, sizeof(p->plane_fmt));
        p->pixelformat = yuv_pixel;
        p->num_planes = yuv_pixel == KINDLE_AYUV_FOURCC ? 1 : 2;
        p->width = target_width; p->height = target_height;
        for (unsigned int i = 0; i < p->num_planes; ++i) {
            unsigned int row_bytes = yuv_pixel == KINDLE_AYUV_FOURCC ? target_width * 4U : target_width;
            unsigned int rows = yuv_pixel == V4L2_PIX_FMT_NV12 && i == 1 ? (target_height + 1U) / 2U : target_height;
            p->plane_fmt[i].bytesperline = (row_bytes + 15U) & ~15U;
            p->plane_fmt[i].sizeimage = (p->plane_fmt[i].bytesperline * rows + 63U) & ~63U;
        }
        if (ioctl(fd, VIDIOC_S_FMT, &capture.format) < 0
            || get_format(fd, capture.type, &recheck) < 0) goto failed;
        if (!same_layout(&capture.format, &recheck)) { errno = EPROTO; goto failed; }
        capture.format = recheck;
        if (explicit_yuv_layout(&capture.format, target_width, target_height, yuv_pixel) < 0) goto failed;
    } else if (capture_layout(&capture.format, target_width, target_height, jpeg, gray) < 0) goto failed;
    capture.count = capture.format.fmt.pix_mp.num_planes;
    step("allocate_reported_capture_planes");
    for (unsigned int i = 0; i < capture.count; ++i)
        if (allocate_plane(&capture.planes[i], capture.format.fmt.pix_mp.plane_fmt[i].sizeimage, 0xa5) < 0) goto failed;
    if (request_queue(fd, &capture) < 0) goto failed;
    if (get_format(fd, capture.type, &recheck) < 0) goto failed;
    if (!same_layout(&capture.format, &recheck)) { errno = EPROTO; goto failed; }
    step("queue_capture_with_matching_planes");
    if (queue_buffer(fd, &capture, 0) < 0) goto failed;
    if (get_format(fd, capture.type, &recheck) < 0) goto failed;
    if (!same_layout(&capture.format, &recheck)) { errno = EPROTO; goto failed; }
    step("stream_capture_decode");
    decode_started = now_ms();
    if (stream_on(fd, &capture) < 0) goto failed;
    step("dequeue_capture");
    if (dequeue(fd, &capture) < 0) goto failed;
    step("dequeue_output");
    if (dequeue(fd, &output) < 0) goto failed;
    decode_finished = now_ms();
    if (no_output) {
        step("normalize_owned_gray_result");
#if defined(AIDOKU_CODEC_BENCH_LIBRARY) || defined(AIDOKU_NATIVE_JPEG_WORKER_LIBRARY)
        if (BENCHMARK_ACTIVE) {
            if ((uint64_t)target_width * target_height > active_benchmark->output_bytes) {
                errno = ENOBUFS; goto failed;
            }
            normalized = active_benchmark->output;
        } else
#endif
        normalized = malloc((size_t)target_width * target_height);
        if (!normalized) goto failed;
        unsigned int stride = capture.format.fmt.pix_mp.plane_fmt[0].bytesperline;
        for (unsigned int y = 0; y < target_height; ++y) {
            const unsigned char *row = (const unsigned char *)capture.planes[0].address + (size_t)y * stride;
            unsigned char *destination = normalized + (size_t)y * target_width;
            if (jpeg && gray) memcpy(destination, row, target_width);
            else if (jpeg && yuv_pixel == V4L2_PIX_FMT_NV16) {
                unsigned int chroma_stride = capture.format.fmt.pix_mp.plane_fmt[1].bytesperline;
                const unsigned char *uv = (const unsigned char *)capture.planes[1].address
                    + (size_t)y * chroma_stride;
                unsigned int samples = target_width / 2;
                for (unsigned int x = 0; x < target_width; ++x) {
                    unsigned int current = x / 2;
                    unsigned int neighbor = x & 1U ? (current + 1 < samples ? current + 1 : current)
                        : (current ? current - 1 : 0);
                    unsigned int u = (3U * uv[current * 2] + uv[neighbor * 2] + 2U) >> 2;
                    unsigned int v = (3U * uv[current * 2 + 1] + uv[neighbor * 2 + 1] + 2U) >> 2;
                    destination[x] = reader_jpeg_gray(row[x], u, v);
                }
            } else if (jpeg && yuv_pixel == KINDLE_AYUV_FOURCC) {
                /* Qualified Scribe 479432 split-luma quirk, not standard AYUV.
                 * Only even-width, unscaled 4:4:4 is selected by the worker.
                 */
                for (unsigned int x = 0; x < target_width; ++x)
                    destination[x] = reader_jpeg_gray(row[x * 2U], row[x * 4U + 1U], row[x * 4U + 3U]);
            }
            else for (unsigned int x = 0; x < target_width; ++x) {
                /* Production Reader weights and white-paper alpha compositing. */
                const unsigned char *pixel = row + x * 4U;
                unsigned int luma = (54U * pixel[2] + 183U * pixel[1] + 19U * pixel[0] + 128U) / 256U;
                destination[x] = (unsigned char)((luma * pixel[3] + 255U * (255U - pixel[3]) + 127U) / 255U);
            }
        }
    }
    atomic_signal_fence(memory_order_seq_cst);
    normalized_ready = now_ms();
    step("save_complete_planes");
    for (unsigned int i = 0; i < capture.count; ++i) {
        if (capture.planes[i].payload < capture.format.fmt.pix_mp.plane_fmt[i].sizeimage) {
            errno = EPROTO; goto failed;
        }
        if (!no_output && save_plane(argv[4], i, &capture.planes[i]) < 0) goto failed;
    }
    stage = "complete";
    goto cleanup;
failed:
    error = errno ? errno : EIO;
    fprintf(stderr, "codec-failure stage=%s errno=%d\n", stage, error);
    fflush(stderr);
cleanup:
    /* STREAMOFF/close may wait in the driver; a timeout is not a DMA fence. */
    cleanup_ms = now_ms();
    if (stop_queue(fd, &capture) < 0) cleanup_error = errno;
    if (stop_queue(fd, &output) < 0 && !cleanup_error) cleanup_error = errno;
    if (fd >= 0 && close(fd) < 0 && !cleanup_error) cleanup_error = errno;
    if (input >= 0) close(input);
    for (unsigned int i = 0; i < MAX_PLANES; ++i) {
        free(capture.planes[i].address);
        capture.planes[i].address = NULL;
    }
    atomic_signal_fence(memory_order_seq_cst);
    cleanup_ms = now_ms() - cleanup_ms;
    if (!BENCHMARK_ACTIVE && normalized) for (size_t i = 0; i < (size_t)target_width * target_height; ++i)
        normalized_checksum = (normalized_checksum ^ normalized[i]) * UINT32_C(16777619);
#if defined(AIDOKU_CODEC_BENCH_LIBRARY) || defined(AIDOKU_NATIVE_JPEG_WORKER_LIBRARY)
    if (BENCHMARK_ACTIVE) {
        struct aidoku_codec_bench_result *result = active_benchmark->result;
        result->width = target_width;
        result->height = target_height;
        result->output_bytes = error || cleanup_error ? 0 : target_width * target_height;
        result->source_events = source_events;
        result->error = error;
        result->cleanup_error = cleanup_error;
        result->driver_ms = decode_finished > decode_started ? decode_finished - decode_started : 0.0;
        result->preparation_and_driver_ms = normalized_ready > input_ready
            ? normalized_ready - input_ready + cleanup_ms : 0.0;
        snprintf(result->stage, sizeof(result->stage), "%s", stage);
    } else
#endif
    {
    printf("{\"schema\":1,\"codec\":\"%s\",\"ioctl_completed\":%s,\"pixel_validated\":false,"
        "\"stage\":\"%s\",\"errno\":%d,\"cleanup_errno\":%d,\"header_before_capture_allocation\":%s,"
        "\"source_change_events\":%u,\"coded_file_bytes\":%u,\"submitted_bytes\":%u,"
        "\"header_capture_fourcc\":%u,\"header_capture_planes\":%u,"
        "\"capture_fourcc\":%u,\"capture_planes\":%u,\"width\":%u,\"height\":%u,"
        "\"source_width\":%u,\"source_height\":%u,\"hardware_resize_requested\":%s,"
        "\"capture_flags\":%u,\"source_flags\":%u,\"allocator\":\"page_aligned_userptr\","
        "\"decode_ms\":%.3f,\"compute_ms\":%.3f,\"cleanup_ms\":%.3f,\"normalized_checksum\":%u,"
        "\"benchmark_no_output\":%s,\"total_ms\":%.3f,\"planes\":[",
        jpeg ? "jpeg" : "png", error || cleanup_error ? "false" : "true", stage, error, cleanup_error,
        header_seen ? "true" : "false", source_events, coded_bytes, submitted_bytes,
        header_format.fmt.pix_mp.pixelformat,
        header_format.fmt.pix_mp.num_planes, capture.format.fmt.pix_mp.pixelformat, capture.count,
        target_width, target_height, width, height,
        width != target_width || height != target_height ? "true" : "false", capture.flags, output.flags,
        decode_finished > decode_started ? decode_finished - decode_started : 0.0,
        normalized_ready > input_ready ? normalized_ready - input_ready + cleanup_ms : 0.0,
        cleanup_ms, normalized_checksum, no_output ? "true" : "false", now_ms() - began);
    for (unsigned int i = 0; i < capture.count; ++i)
        printf("%s{\"index\":%u,\"stride\":%u,\"sizeimage\":%u,\"capacity\":%u,\"payload\":%u}",
            i ? "," : "", i, capture.format.fmt.pix_mp.plane_fmt[i].bytesperline,
            capture.format.fmt.pix_mp.plane_fmt[i].sizeimage, capture.planes[i].capacity, capture.planes[i].payload);
    printf("]}\n");
    }
    for (unsigned int i = 0; i < MAX_PLANES; ++i) {
        free(output.planes[i].address);
    }
    if (!BENCHMARK_ACTIVE) free(normalized);
    return error || cleanup_error ? 1 : 0;
}

#ifdef AIDOKU_CODEC_BENCH_LIBRARY
int aidoku_png_bench_decode(const unsigned char *input, size_t input_bytes,
    uint32_t width, uint32_t height, unsigned char *output, size_t output_bytes,
    struct aidoku_codec_bench_result *result, int explicit_native_opt_in) {
    if (!result) return 2;
    memset(result, 0, sizeof(*result));
    result->abi_version = 1;
    result->structure_bytes = sizeof(*result);
    if (!explicit_native_opt_in || !input || !output || !input_bytes
        || input_bytes > MAX_CODED_BYTES || width < 16 || width > 2048
        || height < 16 || height > 4096 || (uint64_t)width * height > output_bytes
        || active_benchmark) {
        result->error = explicit_native_opt_in ? EINVAL : EPERM;
        snprintf(result->stage, sizeof(result->stage), "invalid_benchmark_admission");
        return 2;
    }
    char width_text[16], height_text[16];
    snprintf(width_text, sizeof(width_text), "%u", width);
    snprintf(height_text, sizeof(height_text), "%u", height);
    char *arguments[] = {"codec-bench", "--reviewed-single-frame", "png",
        "resident-input", "unused-output", width_text, height_text, "auto", "--no-output"};
    struct benchmark_call call = {input, input_bytes, output, output_bytes, result};
    active_benchmark = &call;
    stop_requested = 0;
    int status = run_probe(9, arguments);
    active_benchmark = NULL;
    return status;
}

void aidoku_png_bench_cancel(void) { on_signal(SIGTERM); }
#elif defined(AIDOKU_NATIVE_JPEG_WORKER_LIBRARY)
int aidoku_jpeg_worker_query(void) {
    int fd = open("/dev/video1", O_RDWR | O_NONBLOCK | O_CLOEXEC);
    if (fd < 0) return errno;
    int error = identify(fd, 1) < 0 ? errno : 0;
    if (close(fd) < 0 && !error) error = errno;
    return error;
}

int aidoku_jpeg_worker_decode(const unsigned char *input, size_t input_bytes,
    uint32_t width, uint32_t height, uint32_t target_width, uint32_t target_height,
    unsigned char *output, size_t output_bytes, struct aidoku_codec_bench_result *result) {
    if (!result) return 2;
    memset(result, 0, sizeof(*result));
    result->abi_version = 1;
    result->structure_bytes = sizeof(*result);
    if (!input || !output || !input_bytes || input_bytes > MAX_CODED_BYTES
        || (uint64_t)target_width * target_height > output_bytes || active_benchmark) {
        result->error = EINVAL; return 2;
    }
    struct jpeg_header header = inspect_jpeg_header(input, input_bytes);
    char *format = header.components == 1 ? "gray"
        : header.horizontal[0] == 1 && width == target_width && height == target_height
            ? "ayuv" : "nv16";
    char w[16], h[16], tw[16], th[16];
    snprintf(w, sizeof(w), "%u", width); snprintf(h, sizeof(h), "%u", height);
    snprintf(tw, sizeof(tw), "%u", target_width); snprintf(th, sizeof(th), "%u", target_height);
    char *arguments[] = {"jpeg-worker", "--reviewed-single-frame", "jpeg", "resident-input",
        "unused-output", w, h, format, tw, th, "--no-output"};
    struct benchmark_call call = {input, input_bytes, output, output_bytes, result};
    active_benchmark = &call;
    stop_requested = 0;
    int status = run_probe(11, arguments);
    active_benchmark = NULL;
    return status;
}
#else
int main(int argc, char **argv) { return run_probe(argc, argv); }
#endif
