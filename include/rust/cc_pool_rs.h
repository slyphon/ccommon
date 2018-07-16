#pragma once

#include <stdint.h>
#include <cc_debug.h>
#include <cc_bstring.h>

struct pool_handle_rs;

typedef void (*pool_init_callback_t)(struct bstring *buf);
typedef void (*pool_reset_callback_t)(struct bstring *buf);
typedef void (*pool_destroy_callback_t)(struct bstring *buf);

#define POOL_CALLBACK_RS(fname) ((void (*)(struct bstring *))&(fname))
#define POOL_BORROW_RS(sname, handle) ((struct sname *)pool_take_rs(handle))
#define POOL_RETURN_RS(handle, instance) (pool_put_rs(handle, (struct bstring *)&(instance)))

struct pool_config_rs {
    size_t obj_size;
    uint32_t nmax;
    pool_init_callback_t init_callback;
    pool_destroy_callback_t destroy_callback;
    pool_reset_callback_t reset_callback;
};

struct pool_handle_rs *
pool_create_handle_rs(struct pool_config_rs const *cfg);

void
pool_destroy_handle_rs(struct pool_handle_rs **handle_p);

struct bstring *
pool_take_rs(struct pool_handle_rs *handle_p);

void
pool_put_rs(struct pool_handle_rs *handle_p, struct bstring **buf);
