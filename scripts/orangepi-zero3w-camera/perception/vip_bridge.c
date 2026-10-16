// Small checked ABI around the installed VIPLite runtime. SDK headers stay outside Git.
// One context, on one thread: VIPLite init/destroy must be paired on that thread.
#include <vip_lite.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <math.h>

#define MAX_OUTPUTS 8
typedef struct {
    uint32_t dims, sizes[6], format, quant;
    float scale;
    int32_t zero;
    uint32_t bytes;
} md_meta;
typedef struct {
    vip_buffer buffer;
    md_meta meta;
} md_tensor;
typedef struct {
    vip_network network;
    md_tensor input, outputs[MAX_OUTPUTS];
    uint32_t count;
    int initialized, prepared;
} md_context;

static char error_text[256];
static int active;
const char *md_error(void) { return error_text; }
static int failure(const char *where, int status) {
    snprintf(error_text, sizeof(error_text), "%s: status %d", where, status);
    return -1;
}
#define CHECK(call) do { int status_ = (call); if (status_ != VIP_SUCCESS) return failure(#call, status_); } while (0)

static int make_tensor(md_context *c, uint32_t i, int output, md_tensor *t) {
    vip_buffer_create_params_t p = {0};
    vip_status_e (*query)(vip_network, vip_uint32_t, vip_enum, void *) = output ? vip_query_output : vip_query_input;
    CHECK(query(c->network, i, VIP_BUFFER_PROP_NUM_OF_DIMENSION, &p.num_of_dims));
    if (!p.num_of_dims || p.num_of_dims > 6) return failure("invalid dimensions", -1);
    CHECK(query(c->network, i, VIP_BUFFER_PROP_SIZES_OF_DIMENSION, p.sizes));
    CHECK(query(c->network, i, VIP_BUFFER_PROP_DATA_FORMAT, &p.data_format));
    CHECK(query(c->network, i, VIP_BUFFER_PROP_QUANT_FORMAT, &p.quant_format));
    unsigned itemsize;
    switch (p.data_format) {
        case VIP_BUFFER_FORMAT_INT8: case VIP_BUFFER_FORMAT_UINT8: itemsize = 1; break;
        case VIP_BUFFER_FORMAT_FP16: itemsize = 2; break;
        case VIP_BUFFER_FORMAT_FP32: itemsize = 4; break;
        default: return failure("unsupported tensor format", p.data_format);
    }
    if (p.quant_format == VIP_BUFFER_QUANTIZE_TF_ASYMM) {
        CHECK(query(c->network, i, VIP_BUFFER_PROP_TF_SCALE, &p.quant_data.affine.scale));
        CHECK(query(c->network, i, VIP_BUFFER_PROP_TF_ZERO_POINT, &p.quant_data.affine.zeroPoint));
        if (!isfinite(p.quant_data.affine.scale) || p.quant_data.affine.scale <= 0)
            return failure("invalid quantization scale", -1);
        t->meta.scale = p.quant_data.affine.scale;
        t->meta.zero = p.quant_data.affine.zeroPoint;
    } else if (p.quant_format != VIP_BUFFER_QUANTIZE_NONE) {
        return failure("unsupported quantization format", p.quant_format);
    }
    uint64_t bytes = itemsize;
    for (unsigned j = 0; j < p.num_of_dims; ++j) {
        if (!p.sizes[j] || bytes > (64U * 1024U * 1024U) / p.sizes[j])
            return failure("tensor size out of bounds", -1);
        bytes *= p.sizes[j];
    }
    t->meta.dims = p.num_of_dims;
    memcpy(t->meta.sizes, p.sizes, sizeof(p.sizes));
    t->meta.format = p.data_format;
    t->meta.quant = p.quant_format;
    t->meta.bytes = (uint32_t)bytes;
    p.memory_type = VIP_BUFFER_MEMORY_TYPE_DEFAULT;
    CHECK(vip_create_buffer(&p, sizeof(p), &t->buffer));
    if (vip_get_buffer_size(t->buffer) < bytes) return failure("undersized VIP buffer", -1);
    return 0;
}

void md_close(md_context *c) {
    if (!c) return;
    if (c->prepared) vip_finish_network(c->network);
    if (c->network) vip_destroy_network(c->network);
    if (c->input.buffer) vip_destroy_buffer(c->input.buffer);
    for (unsigned i = 0; i < MAX_OUTPUTS; ++i)
        if (c->outputs[i].buffer) vip_destroy_buffer(c->outputs[i].buffer);
    if (c->initialized) { vip_destroy(); active = 0; }
    free(c);
}

md_context *md_open(const char *path) {
    if (active) { failure("only one VIPLite context per process is supported", -1); return NULL; }
    md_context *c = calloc(1, sizeof(*c));
    if (!c) { failure("allocate context", -1); return NULL; }
    int status = vip_init();
    if (status != VIP_SUCCESS) { failure("vip_init", status); goto fail; }
    c->initialized = active = 1;
    status = vip_create_network(path, 0, VIP_CREATE_NETWORK_FROM_FILE, &c->network);
    if (status != VIP_SUCCESS) { failure("vip_create_network", status); goto fail; }
    uint32_t inputs = 0;
    status = vip_query_network(c->network, VIP_NETWORK_PROP_INPUT_COUNT, &inputs);
    if (status != VIP_SUCCESS || inputs != 1) { failure("expected one network input", status); goto fail; }
    status = vip_query_network(c->network, VIP_NETWORK_PROP_OUTPUT_COUNT, &c->count);
    if (status != VIP_SUCCESS || !c->count || c->count > MAX_OUTPUTS) {
        failure("invalid output count", status); goto fail;
    }
    if (make_tensor(c, 0, 0, &c->input)) goto fail;
    for (unsigned i = 0; i < c->count; ++i)
        if (make_tensor(c, i, 1, &c->outputs[i])) goto fail;
    status = vip_prepare_network(c->network);
    if (status != VIP_SUCCESS) { failure("vip_prepare_network", status); goto fail; }
    c->prepared = 1;
    status = vip_set_input(c->network, 0, c->input.buffer);
    if (status != VIP_SUCCESS) { failure("vip_set_input", status); goto fail; }
    for (unsigned i = 0; i < c->count; ++i) {
        status = vip_set_output(c->network, i, c->outputs[i].buffer);
        if (status != VIP_SUCCESS) { failure("vip_set_output", status); goto fail; }
    }
    return c;
fail:
    md_close(c);
    return NULL;
}

int md_info(md_context *c, int index, md_meta *meta) {
    if (!c || !meta || index < -1 || index >= (int)c->count) return failure("invalid tensor index", -1);
    *meta = index == -1 ? c->input.meta : c->outputs[index].meta;
    return 0;
}

int md_output_count(md_context *c) { return c ? (int)c->count : -1; }

int md_run(md_context *c, const void *input, uint32_t bytes, void **outputs, const uint32_t *lengths) {
    if (!c || !input || !outputs || !lengths || bytes != c->input.meta.bytes)
        return failure("incorrect input length", -1);
    for (unsigned i = 0; i < c->count; ++i)
        if (!outputs[i] || lengths[i] != c->outputs[i].meta.bytes)
            return failure("incorrect output length", -1);
    void *mapped = vip_map_buffer(c->input.buffer);
    if (!mapped) return failure("map input", -1);
    memcpy(mapped, input, bytes);
    CHECK(vip_unmap_buffer(c->input.buffer));
    CHECK(vip_flush_buffer(c->input.buffer, VIP_BUFFER_OPER_TYPE_FLUSH));
    CHECK(vip_run_network(c->network));
    for (unsigned i = 0; i < c->count; ++i) {
        CHECK(vip_flush_buffer(c->outputs[i].buffer, VIP_BUFFER_OPER_TYPE_INVALIDATE));
        mapped = vip_map_buffer(c->outputs[i].buffer);
        if (!mapped) return failure("map output", -1);
        memcpy(outputs[i], mapped, lengths[i]);
        CHECK(vip_unmap_buffer(c->outputs[i].buffer));
    }
    return 0;
}
