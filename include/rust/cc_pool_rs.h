#pragma once

#include <stdint.h>
#include <cc_debug.h>

struct pool_handle_rs;

typedef void (*pool_init_callback_t)(void *buf);
typedef void (*pool_destroy_callback_t)(void **buf);

struct pool_handle_rs *
pool_create_handle_rs(
    size_t obj_size,
    uint32_t nmax,
    pool_init_callback_t,
    pool_destroy_callback_t
);

void
pool_destroy_handle_rs(struct pool_handle_rs *handle_p);

void *
pool_take_rs(struct pool_handle_rs *handle_p);

void
pool_put_rs(struct pool_handle_rs *handle_p, void *buf);
