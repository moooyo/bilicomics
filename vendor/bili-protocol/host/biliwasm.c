/* Minimal synchronous Go/js host for pinned Bilibili Comics WASM modules.
 * The host implements the Go 1.25 syscall/js ABI, not a JavaScript engine.
 * Each invocation handles one JSON request in an isolated runtime.
 * No request data is passed in argv.
 */
#define _POSIX_C_SOURCE 200809L
#include <errno.h>
#include <inttypes.h>
#include <math.h>
#include <setjmp.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>
#include "wasm3.h"
#include "cJSON.h"
#include "biliwasm.h"

#define BILI_MAX_INPUT (16u * 1024u * 1024u)
#define MAX_VALUES 65536u
#define MAX_PROPERTIES 65536u
#define MAX_DEPTH 64u
#define MAX_ARENA (64u * 1024u * 1024u)
#define MAX_TIMERS 64u
#define MAX_TIMER_WAIT_NS UINT64_C(5000000000)

typedef enum { UNDEFINED, NUMBER, NULL_VALUE, BOOLEAN, STRING, OBJECT, ARRAY,
               BYTES, FUNCTION } Kind;
typedef struct Value Value;
typedef struct Property Property;
struct Property { char *key; Value *value; Property *next; };
struct Value {
    Kind kind;
    uint32_t id;
    double number;
    char *text;
    size_t length;
    Property *properties;
    Value **items;
};
typedef struct { uint32_t id; uint64_t due; } Timer;
typedef struct {
    IM3Environment env;
    IM3Runtime runtime;
    IM3Module module;
    Value *values[MAX_VALUES];
    uint32_t value_count;
    uint32_t property_count;
    Value *undefined;
    Value *global;
    Value *go;
    int exited;
    Timer timers[MAX_TIMERS];
    uint32_t next_timer;
} Host;

typedef struct Allocation Allocation;
struct Allocation { void *pointer; Allocation *next; };
typedef struct {
    Host *host;
    Allocation *allocations;
    size_t allocated;
    cJSON *request;
    cJSON *output;
    char *serialized;
    jmp_buf failure;
    char error[512];
    int module_loaded;
    FILE *open_file;
} Job;
static __thread Job *active_job;

static void fail(const char *message) {
    snprintf(active_job->error, sizeof(active_job->error), "%s", message);
    longjmp(active_job->failure, 1);
}
static void *allocate(size_t size) {
    if (size > MAX_ARENA - active_job->allocated) fail("host arena limit exceeded");
    void *p = calloc(1, size ? size : 1);
    if (!p) fail("host allocation failed");
    Allocation *a = malloc(sizeof(*a));
    if (!a) { free(p); fail("host allocation tracking failed"); }
    a->pointer = p;
    a->next = active_job->allocations;
    active_job->allocations = a;
    active_job->allocated += size;
    return p;
}
static uint64_t monotonic_ns(void) {
    struct timespec ts;
    if (clock_gettime(CLOCK_MONOTONIC, &ts)) fail("monotonic clock unavailable");
    return (uint64_t)ts.tv_sec * UINT64_C(1000000000) + (uint64_t)ts.tv_nsec;
}
static char *copy_string(const char *s, size_t n) {
    char *p = allocate(n + 1);
    memcpy(p, s, n);
    return p;
}
static Value *value(Host *h, Kind kind) {
    if (h->value_count >= MAX_VALUES) fail("host value limit exceeded");
    Value *v = allocate(sizeof(*v));
    v->kind = kind;
    v->id = h->value_count;
    h->values[h->value_count++] = v;
    return v;
}
static Value *number(Host *h, double n) {
    if (n == 0 && h->value_count > 1) return h->values[1];
    Value *v = value(h, NUMBER);
    v->number = n;
    return v;
}
static Value *string(Host *h, const char *s, size_t n) {
    for (uint32_t i = 7; i < h->value_count; i++) {
        Value *v = h->values[i];
        if (v->kind == STRING && v->length == n && !memcmp(v->text, s, n)) return v;
    }
    Value *v = value(h, STRING);
    v->text = copy_string(s, n);
    v->length = n;
    return v;
}
static Value *function(Host *h, const char *name, double id) {
    Value *v = value(h, FUNCTION);
    v->text = copy_string(name, strlen(name));
    v->number = id;
    return v;
}
static void set(Host *h, Value *v, const char *key, Value *item) {
    if (v->kind != OBJECT && v->kind != FUNCTION) fail("property target is not an object");
    for (Property *p = v->properties; p; p = p->next) {
        if (!strcmp(p->key, key)) { p->value = item; return; }
    }
    if (++h->property_count > MAX_PROPERTIES) fail("host property limit exceeded");
    Property *p = allocate(sizeof(*p));
    p->key = copy_string(key, strlen(key));
    p->value = item;
    p->next = v->properties;
    v->properties = p;
}
static Value *get(Host *h, Value *v, const char *key) {
    if (!strcmp(key, "length") && (v->kind == ARRAY || v->kind == BYTES || v->kind == STRING))
        return number(h, (double)v->length);
    for (Property *p = v->properties; p; p = p->next)
        if (!strcmp(p->key, key)) return p->value;
    return h->undefined;
}
static uint8_t *memory(Host *h, uint64_t offset, size_t length) {
    size_t size = 0;
    uint8_t *base = m3_GetMemory(h->module, &size, 0);
    if (!base || offset > size || length > size - (size_t)offset)
        fail("Go host memory access out of bounds");
    return base + (size_t)offset;
}
static uint32_t u32(Host *h, uint64_t p) {
    const uint8_t *b = memory(h, p, 4);
    return (uint32_t)b[0] | (uint32_t)b[1]<<8 | (uint32_t)b[2]<<16 | (uint32_t)b[3]<<24;
}
static uint64_t u64(Host *h, uint64_t p) {
    return (uint64_t)u32(h, p) | (uint64_t)u32(h, p + 4)<<32;
}
static void put32(Host *h, uint64_t p, uint32_t n) {
    uint8_t *b = memory(h, p, 4);
    for (unsigned i = 0; i < 4; i++) b[i] = (uint8_t)(n >> (i * 8));
}
static void put64(Host *h, uint64_t p, uint64_t n) {
    put32(h, p, (uint32_t)n);
    put32(h, p + 4, (uint32_t)(n >> 32));
}
static void put8(Host *h, uint64_t p, uint8_t n) { *memory(h, p, 1) = n; }
static char *go_string(Host *h, uint64_t p) {
    uint64_t offset = u64(h, p), length = u64(h, p + 8);
    if (length > BILI_MAX_INPUT) fail("Go string limit exceeded");
    return copy_string((char *)memory(h, offset, (size_t)length), (size_t)length);
}
static Value *load(Host *h, uint64_t p) {
    uint64_t bits = u64(h, p);
    double n;
    memcpy(&n, &bits, 8);
    if (n == 0) return h->undefined;
    if (!isnan(n)) return number(h, n);
    uint32_t id = (uint32_t)bits;
    if (id >= h->value_count) fail("invalid Go reference id");
    return h->values[id];
}
static void store(Host *h, uint64_t p, Value *v) {
    if (v->kind == UNDEFINED) { put64(h, p, 0); return; }
    if (v->kind == NUMBER && v->number != 0 && !isnan(v->number)) {
        uint64_t bits;
        memcpy(&bits, &v->number, 8);
        put64(h, p, bits);
        return;
    }
    uint32_t type = (v->kind == STRING) ? 2 : (v->kind == FUNCTION) ? 4 :
        (v->kind == OBJECT || v->kind == ARRAY || v->kind == BYTES) ? 1 : 0;
    put32(h, p, v->id);
    put32(h, p + 4, 0x7ff80000u | type);
}
static Value *array(Host *h, size_t length) {
    if (length > MAX_VALUES) fail("host array limit exceeded");
    Value *v = value(h, ARRAY);
    v->length = length;
    v->items = allocate(sizeof(Value *) * length);
    for (size_t i = 0; i < length; i++) v->items[i] = h->undefined;
    return v;
}
static Value *load_args(Host *h, uint64_t p) {
    uint64_t offset = u64(h, p), length = u64(h, p + 8);
    if (length > MAX_VALUES) fail("Go arguments limit exceeded");
    Value *v = array(h, (size_t)length);
    for (size_t i = 0; i < v->length; i++) v->items[i] = load(h, offset + i * 8);
    return v;
}
static Value *as_string(Host *h, Value *v) {
    if (v->kind == STRING || v->kind == BYTES) return v;
    const char *text = NULL;
    char formatted[64];
    if (v->kind == UNDEFINED) text = "undefined";
    else if (v->kind == NULL_VALUE) text = "null";
    else if (v->kind == BOOLEAN) text = v->number ? "true" : "false";
    else if (v->kind == NUMBER) { snprintf(formatted, sizeof(formatted), "%.17g", v->number); text = formatted; }
    else fail("unsupported JavaScript string conversion");
    return string(h, text, strlen(text));
}

enum {
    TIMEOUT, CLEAR_TIMEOUT, RESET_MEMORY, WRITE, RANDOM, NANOTIME, EXIT, WALLTIME,
    FINALIZE, STRING_VALUE, GET, SET, INDEX, SET_INDEX, LENGTH, CALL, NEW,
    PREPARE_STRING, LOAD_STRING, COPY_TO_JS, COPY_TO_GO
};
static const char *imports[] = {
    "runtime.scheduleTimeoutEvent", "runtime.clearTimeoutEvent",
    "runtime.resetMemoryDataView", "runtime.wasmWrite", "runtime.getRandomData",
    "runtime.nanotime1", "runtime.wasmExit", "runtime.walltime",
    "syscall/js.finalizeRef", "syscall/js.stringVal", "syscall/js.valueGet",
    "syscall/js.valueSet", "syscall/js.valueIndex", "syscall/js.valueSetIndex",
    "syscall/js.valueLength", "syscall/js.valueCall", "syscall/js.valueNew",
    "syscall/js.valuePrepareString", "syscall/js.valueLoadString",
    "syscall/js.copyBytesToJS", "syscall/js.copyBytesToGo"
};
static const void *host_call(IM3Runtime runtime, IM3ImportContext context,
                             uint64_t *stack, void *unused_memory) {
    (void)unused_memory;
    Host *h = m3_GetUserData(runtime);
    uint64_t sp = (uint32_t)stack[0];
    int operation = (int)(uintptr_t)context->userdata;
    Value *v, *args, *result;
    char *key;
    uint64_t offset, length, index;
    struct timespec ts;
    switch (operation) {
    case TIMEOUT: {
        int64_t delay = (int64_t)u64(h, sp + 8);
        /* setTimeout clamps an out-of-range delay; no browser task runs here. */
        if (delay > INT32_MAX) delay = 1;
        if (delay < 0) delay = 0;
        unsigned slot = 0;
        while (slot < MAX_TIMERS && h->timers[slot].id) slot++;
        if (slot == MAX_TIMERS) return "Go timer limit exceeded";
        if (++h->next_timer == 0) return "Go timer id overflow";
        h->timers[slot].id = h->next_timer;
        h->timers[slot].due = monotonic_ns() + (uint64_t)delay * UINT64_C(1000000);
        put32(h, sp + 16, h->next_timer);
        break;
    }
    case CLEAR_TIMEOUT:
        for (unsigned i = 0; i < MAX_TIMERS; i++)
            if (h->timers[i].id == u32(h, sp + 8)) h->timers[i].id = 0;
        break;
    case RESET_MEMORY: break;
    case EXIT: h->exited = 1; return "Go module exited";
    case WRITE:
        offset = u64(h, sp + 16); length = u32(h, sp + 24);
        if (fwrite(memory(h, offset, (size_t)length), 1, (size_t)length, stderr) != length)
            return "Go stderr write failed";
        break;
    case RANDOM: {
        offset = u64(h, sp + 8); length = u64(h, sp + 16);
        uint8_t *target = memory(h, offset, (size_t)length);
        FILE *random = fopen("/dev/urandom", "rb");
        if (!random) return "system random source unavailable";
        size_t count = fread(target, 1, (size_t)length, random);
        fclose(random);
        if (count != length) return "system random source read failed";
        break;
    }
    case NANOTIME:
        if (clock_gettime(CLOCK_MONOTONIC, &ts)) return "monotonic clock unavailable";
        put64(h, sp + 8, (uint64_t)ts.tv_sec * 1000000000u + (uint64_t)ts.tv_nsec);
        break;
    case WALLTIME:
        if (clock_gettime(CLOCK_REALTIME, &ts)) return "wall clock unavailable";
        put64(h, sp + 8, (uint64_t)ts.tv_sec); put32(h, sp + 16, (uint32_t)ts.tv_nsec);
        break;
    case FINALIZE:
        /* The one-request arena retains references until this call returns. */
        if (u32(h, sp + 8) >= h->value_count) return "invalid finalized reference";
        break;
    case STRING_VALUE:
        length = u64(h, sp + 16); offset = u64(h, sp + 8);
        if (length > BILI_MAX_INPUT) return "Go string limit exceeded";
        store(h, sp + 24, string(h, (char *)memory(h, offset, (size_t)length), (size_t)length));
        break;
    case GET:
        v = load(h, sp + 8); key = go_string(h, sp + 16);
        result = get(h, v, key); store(h, sp + 32, result);
        break;
    case SET:
        v = load(h, sp + 8); key = go_string(h, sp + 16);
        set(h, v, key, load(h, sp + 32));
        break;
    case INDEX:
        v = load(h, sp + 8); index = u64(h, sp + 16);
        if (v->kind == ARRAY) result = index < v->length ? v->items[index] : h->undefined;
        else if (v->kind == BYTES) result = index < v->length ? number(h, (unsigned char)v->text[index]) : h->undefined;
        else return "unsupported JavaScript index target";
        store(h, sp + 24, result);
        break;
    case SET_INDEX:
        v = load(h, sp + 8); index = u64(h, sp + 16);
        if (index >= v->length) return "JavaScript index out of bounds";
        result = load(h, sp + 24);
        if (v->kind == ARRAY) v->items[index] = result;
        else if (v->kind == BYTES && result->kind == NUMBER) v->text[index] = (char)(uint8_t)result->number;
        else return "unsupported JavaScript index assignment";
        break;
    case LENGTH:
        v = load(h, sp + 8);
        if (v->kind != ARRAY && v->kind != BYTES && v->kind != STRING) return "unsupported JavaScript length target";
        put64(h, sp + 16, v->length);
        break;
    case CALL:
        v = load(h, sp + 8); key = go_string(h, sp + 16); args = load_args(h, sp + 32);
        if (v == h->go && !strcmp(key, "_makeFuncWrapper") && args->length == 1 && args->items[0]->kind == NUMBER) {
            result = function(h, "GoCallback", args->items[0]->number);
            store(h, sp + 56, result); put8(h, sp + 64, 1);
        } else return "unsupported JavaScript method call";
        break;
    case NEW:
        v = load(h, sp + 8); args = load_args(h, sp + 16);
        if (v->kind != FUNCTION) return "JavaScript constructor is not a function";
        if (!strcmp(v->text, "Object") && args->length == 0) result = value(h, OBJECT);
        else if (!strcmp(v->text, "Array") && args->length <= 1) {
            double n = args->length ? args->items[0]->number : 0;
            if ((args->length && args->items[0]->kind != NUMBER) || !isfinite(n) || n < 0 || n > MAX_VALUES || floor(n) != n)
                return "array length out of range";
            result = array(h, (size_t)n);
        } else if (!strcmp(v->text, "Uint8Array") && args->length == 1 && args->items[0]->kind == NUMBER) {
            double n = args->items[0]->number;
            if (!isfinite(n) || n < 0 || n > BILI_MAX_INPUT || floor(n) != n) return "byte array length out of range";
            result = value(h, BYTES); result->length = (size_t)n;
            result->text = allocate(result->length + 1);
        } else return "unsupported JavaScript constructor";
        store(h, sp + 40, result); put8(h, sp + 48, 1);
        break;
    case PREPARE_STRING:
        v = as_string(h, load(h, sp + 8));
        store(h, sp + 16, v); put64(h, sp + 24, v->length);
        break;
    case LOAD_STRING:
        v = load(h, sp + 8); offset = u64(h, sp + 16); length = u64(h, sp + 24);
        if ((v->kind != STRING && v->kind != BYTES) || length < v->length) return "invalid Go string destination";
        memcpy(memory(h, offset, v->length), v->text, v->length);
        break;
    case COPY_TO_JS:
        v = load(h, sp + 8); offset = u64(h, sp + 16); length = u64(h, sp + 24);
        if (v->kind != BYTES) { put8(h, sp + 48, 0); break; }
        if (length > v->length) length = v->length;
        memcpy(v->text, memory(h, offset, (size_t)length), (size_t)length);
        put64(h, sp + 40, length); put8(h, sp + 48, 1);
        break;
    case COPY_TO_GO:
        v = load(h, sp + 32); offset = u64(h, sp + 8); length = u64(h, sp + 16);
        if (v->kind != BYTES) { put8(h, sp + 48, 0); break; }
        if (length > v->length) length = v->length;
        memcpy(memory(h, offset, (size_t)length), v->text, (size_t)length);
        put64(h, sp + 40, length); put8(h, sp + 48, 1);
        break;
    default: return "unknown Go host import";
    }
    return m3Err_none;
}

static void initialize_values(Host *h) {
    number(h, NAN);
    number(h, 0);
    value(h, NULL_VALUE);
    value(h, BOOLEAN)->number = 1;
    value(h, BOOLEAN)->number = 0;
    h->global = value(h, OBJECT);
    h->go = value(h, OBJECT);
    h->undefined = value(h, UNDEFINED);
    set(h, h->global, "Object", function(h, "Object", 0));
    set(h, h->global, "Array", function(h, "Array", 0));
    set(h, h->global, "Uint8Array", function(h, "Uint8Array", 0));
    set(h, h->global, "process", value(h, OBJECT));
    set(h, h->global, "path", value(h, OBJECT));
    Value *fs = value(h, OBJECT), *constants = value(h, OBJECT);
    set(h, h->global, "fs", fs); set(h, fs, "constants", constants);
    const char *names[] = { "O_WRONLY", "O_RDWR", "O_CREAT", "O_TRUNC", "O_APPEND", "O_EXCL", "O_DIRECTORY" };
    for (unsigned i = 0; i < sizeof(names)/sizeof(names[0]); i++) set(h, constants, names[i], number(h, -1));
    set(h, h->go, "_pendingEvent", h->values[2]);
}
static Value *from_json(Host *h, const cJSON *json, unsigned depth) {
    if (depth > MAX_DEPTH) fail("JSON nesting limit exceeded");
    if (cJSON_IsNull(json)) return h->values[2];
    if (cJSON_IsBool(json)) return h->values[cJSON_IsTrue(json) ? 3 : 4];
    if (cJSON_IsNumber(json)) return number(h, json->valuedouble);
    if (cJSON_IsString(json)) return string(h, json->valuestring, strlen(json->valuestring));
    if (cJSON_IsArray(json)) {
        Value *v = array(h, (size_t)cJSON_GetArraySize(json));
        size_t i = 0;
        for (const cJSON *item = json->child; item; item = item->next) v->items[i++] = from_json(h, item, depth + 1);
        return v;
    }
    if (cJSON_IsObject(json)) {
        Value *v = value(h, OBJECT);
        for (const cJSON *item = json->child; item; item = item->next) set(h, v, item->string, from_json(h, item, depth + 1));
        return v;
    }
    fail("unsupported JSON value"); return NULL;
}
static cJSON *to_json(Value *v, unsigned depth) {
    if (depth > MAX_DEPTH) fail("result nesting limit exceeded");
    if (v->kind == UNDEFINED || v->kind == NULL_VALUE) return cJSON_CreateNull();
    if (v->kind == BOOLEAN) return cJSON_CreateBool(v->number != 0);
    if (v->kind == NUMBER) return cJSON_CreateNumber(v->number);
    if (v->kind == STRING) return cJSON_CreateString(v->text);
    if (v->kind == OBJECT) {
        cJSON *result = cJSON_CreateObject();
        if (!result) return NULL;
        for (Property *p = v->properties; p; p = p->next) {
            cJSON *child = to_json(p->value, depth + 1);
            if (!child || !cJSON_AddItemToObject(result, p->key, child)) {
                cJSON_Delete(child); cJSON_Delete(result); return NULL;
            }
        }
        return result;
    }
    if (v->kind == ARRAY || v->kind == BYTES) {
        cJSON *result = cJSON_CreateArray();
        if (!result) return NULL;
        for (size_t i = 0; i < v->length; i++) {
            cJSON *child = v->kind == BYTES ? cJSON_CreateNumber((unsigned char)v->text[i]) : to_json(v->items[i], depth + 1);
            if (!child || !cJSON_AddItemToArray(result, child)) {
                cJSON_Delete(child); cJSON_Delete(result); return NULL;
            }
        }
        return result;
    }
    fail("unsupported result type"); return NULL;
}
static void validate_result(Value *v, unsigned depth) {
    if (depth > MAX_DEPTH) fail("result nesting limit exceeded");
    if (v->kind == FUNCTION) fail("unsupported result type");
    if (v->kind == OBJECT)
        for (Property *p = v->properties; p; p = p->next) validate_result(p->value, depth + 1);
    if (v->kind == ARRAY)
        for (size_t i = 0; i < v->length; i++) validate_result(v->items[i], depth + 1);
}
static uint8_t *read_file(const char *path, uint32_t *size) {
    FILE *file = fopen(path, "rb");
    if (!file) fail("WASM module cannot be opened");
    active_job->open_file = file;
    if (fseek(file, 0, SEEK_END)) fail("WASM module cannot be measured");
    long length = ftell(file);
    if (length <= 0 || (unsigned long)length > BILI_MAX_INPUT) fail("WASM module size is invalid");
    rewind(file);
    uint8_t *bytes = allocate((size_t)length);
    if (fread(bytes, 1, (size_t)length, file) != (size_t)length) fail("WASM module cannot be read");
    fclose(file); active_job->open_file = NULL; *size = (uint32_t)length;
    return bytes;
}
static void checked(M3Result result) { if (result) fail(result); }
static Value *complete_callback(Host *h, IM3Function resume, Value *event) {
    uint64_t deadline = monotonic_ns() + MAX_TIMER_WAIT_NS;
    unsigned resumes = 0;
    for (;;) {
        Value *result = get(h, event, "result");
        if (result->kind != UNDEFINED) return result;
        uint64_t due = UINT64_MAX;
        for (unsigned i = 0; i < MAX_TIMERS; i++)
            if (h->timers[i].id && h->timers[i].due < due) due = h->timers[i].due;
        if (due == UINT64_MAX) fail("WASM callback did not produce a result or schedule a timer");
        uint64_t now = monotonic_ns();
        if (now >= deadline || due > deadline || ++resumes > 10000)
            fail("Go callback timer wait limit exceeded");
        if (due > now) {
            uint64_t wait = due - now;
            struct timespec pause = {(time_t)(wait / UINT64_C(1000000000)),
                                     (long)(wait % UINT64_C(1000000000))};
            while (nanosleep(&pause, &pause) && errno == EINTR) {}
        }
        /* As in wasm_exec.js, retain the timer until Go explicitly clears it. */
        checked(m3_CallV(resume));
    }
}
static void run_request(Job *job, const char *wasm_path, const uint8_t *wasm_bytes,
    size_t wasm_length, const char *input) {
    size_t used = strnlen(input, BILI_MAX_INPUT + 1u);
    if (used > BILI_MAX_INPUT) fail("request input limit exceeded");
    const char *end = NULL;
    cJSON *request = cJSON_ParseWithLengthOpts(input, used + 1, &end, 1);
    job->request = request;
    if (!request || !cJSON_IsObject(request)) fail("request must be one JSON object");
    cJSON *fn = cJSON_GetObjectItemCaseSensitive(request, "function");
    cJSON *args_json = cJSON_GetObjectItemCaseSensitive(request, "args");
    if (!cJSON_IsString(fn) || !cJSON_IsArray(args_json)) fail("request requires function string and args array");
    if (strcmp(fn->valuestring, "y1_z2w2a3") && strcmp(fn->valuestring, "c1_r9k2m7")) fail("function is not permitted");
    Host *h = allocate(sizeof(*h));
    job->host = h;
    initialize_values(h);
    uint32_t wasm_size = (uint32_t)wasm_length;
    const uint8_t *wasm = wasm_bytes;
    if (!wasm) wasm = read_file(wasm_path, &wasm_size);
    h->env = m3_NewEnvironment();
    if (!h->env) fail("WASM environment allocation failed");
    h->runtime = m3_NewRuntime(h->env, 1024u * 1024u, h);
    if (!h->runtime) fail("WASM runtime allocation failed");
    checked(m3_ParseModule(h->env, &h->module, wasm, wasm_size));
    /* This pinned wasm3 revision owns the module even when loading fails. */
    job->module_loaded = 1;
    checked(m3_LoadModule(h->runtime, h->module));
    for (unsigned i = 0; i < sizeof(imports)/sizeof(imports[0]); i++) {
        M3Result result = m3_LinkRawFunctionEx(h->module, "gojs", imports[i], "v(i)", host_call, (void *)(uintptr_t)i);
        if (result && result != m3Err_functionLookupFailed) checked(result);
    }
    IM3Function run, resume;
    checked(m3_FindFunction(&run, h->runtime, "run"));
    checked(m3_FindFunction(&resume, h->runtime, "resume"));
    memcpy(memory(h, 4096, 3), "js", 3);
    put64(h, 4104, 4096); put64(h, 4112, 0); put64(h, 4120, 0);
    checked(m3_CallV(run, (uint32_t)1, (uint32_t)4104));
    Value *callback = get(h, h->global, fn->valuestring);
    if (callback->kind != FUNCTION || strcmp(callback->text, "GoCallback")) fail("WASM callback was not registered");
    Value *event = value(h, OBJECT);
    set(h, event, "id", number(h, callback->number));
    set(h, event, "this", h->global);
    set(h, event, "args", from_json(h, args_json, 0));
    set(h, h->go, "_pendingEvent", event);
    checked(m3_CallV(resume));
    Value *result = complete_callback(h, resume, event);
    validate_result(result, 0);
    cJSON *output = cJSON_CreateObject();
    job->output = output;
    if (!output || !cJSON_AddBoolToObject(output, "ok", 1)) fail("result allocation failed");
    cJSON *result_json = to_json(result, 0);
    if (!result_json || !cJSON_AddItemToObject(output, "result", result_json)) {
        cJSON_Delete(result_json); fail("result allocation failed");
    }
    job->serialized = cJSON_PrintUnformatted(output);
    if (!job->serialized) fail("result serialization failed");
}

static void cleanup(Job *job) {
    if (job->open_file) fclose(job->open_file);
    Host *h = job->host;
    if (h) {
        if (h->module && !job->module_loaded) m3_FreeModule(h->module);
        if (h->runtime) m3_FreeRuntime(h->runtime);
        if (h->env) m3_FreeEnvironment(h->env);
    }
    cJSON_Delete(job->request);
    cJSON_Delete(job->output);
    Allocation *a = job->allocations;
    while (a) { Allocation *next = a->next; free(a->pointer); free(a); a = next; }
}

static int run_input(const char *wasm_path, const uint8_t *wasm_bytes, size_t wasm_length,
    const char *request_json, char **output_json) {
    if (!output_json) return 1;
    *output_json = NULL;
    if ((!wasm_path && !wasm_bytes) || !request_json || active_job) return 1;
    if (wasm_bytes && (wasm_length == 0 || wasm_length > BILI_MAX_INPUT)) return 1;
    Job *job = calloc(1, sizeof(*job));
    if (!job) return 1;
    /* Keep the same arena admission as read_file without owning its buffer. */
    if (wasm_bytes) job->allocated = wasm_length;
    active_job = job;
    int status = setjmp(job->failure);
    if (status == 0) run_request(job, wasm_path, wasm_bytes, wasm_length, request_json);
    else {
        cJSON *failure = cJSON_CreateObject();
        if (failure && cJSON_AddBoolToObject(failure, "ok", 0) &&
            cJSON_AddStringToObject(failure, "error", job->error))
            job->serialized = cJSON_PrintUnformatted(failure);
        cJSON_Delete(failure);
    }
    *output_json = job->serialized;
    cleanup(job);
    active_job = NULL;
    free(job);
    return status ? 1 : 0;
}
int biliwasm_run(const char *wasm_path, const char *request_json, char **output_json) {
    return run_input(wasm_path, NULL, 0, request_json, output_json);
}
int biliwasm_run_bytes(const uint8_t *wasm_bytes, size_t wasm_length,
    const char *request_json, char **output_json) {
    return run_input(NULL, wasm_bytes, wasm_length, request_json, output_json);
}
void biliwasm_free(char *output_json) { free(output_json); }

#ifndef BILIWASM_NO_MAIN
int main(int argc, char **argv) {
    if (argc != 2) {
        puts("{\"ok\":false,\"error\":\"usage: biliwasm /path/to/pinned.wasm < request.json\"}");
        return 1;
    }
    char *input = calloc(BILI_MAX_INPUT + 1u, 1);
    if (!input) return 1;
    size_t used = fread(input, 1, BILI_MAX_INPUT, stdin);
    if (ferror(stdin) || (used == BILI_MAX_INPUT && fgetc(stdin) != EOF)) {
        puts("{\"ok\":false,\"error\":\"request input could not be read within limit\"}");
        free(input); return 1;
    }
    char *output = NULL;
    int status = biliwasm_run(argv[1], input, &output);
    free(input);
    if (output) { puts(output); biliwasm_free(output); }
    else puts("{\"ok\":false,\"error\":\"native host initialization failed\"}");
    return status;
}
#endif
