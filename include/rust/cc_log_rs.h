/* ccommon - a cache common library.
 * Copyright (C) 2013 Twitter, Inc.
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 * http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

#pragma once

#ifdef __cplusplus
extern "C" {
#endif

#include <cc_log.h>
#include <cc_bstring.h>

/* NOTE: for documentation see ccommon/rust/ccommon_rs/src/log.rs */

typedef enum log_level_rs {
    LOG_LEVEL_ERROR = 1,
    LOG_LEVEL_WARN,
    LOG_LEVEL_INFO,
    LOG_LEVEL_DEBUG,
    LOG_LEVEL_TRACE,
} log_level_rs_e;


typedef enum log_status_rs {
    /* Good work! */
    LOG_STATUS_OK = 0,
    /* An action that requires log_rs_is_setup() to be true, but it isn't
     * i.e. you need to call log_rs_setup() before whatever you just tried to do. */
    LOG_STATUS_NOT_SETUP_ERROR,
    /* We could not register as the backend for the log crate .
     * This state is unrecoverable. */
    LOG_STATUS_REGISTRATION_FAIL,
    /* Returned when there is already a logger set up for rust. */
    LOG_STATUS_ALREADY_SET_ERROR,
    /* Data was expected to be valid UTF8 but was not */
    LOG_STATUS_INVALID_UTF8,
} log_status_rs_e;

log_status_rs_e log_st_setup_rs(void);
/* Set this logger as the one Rust will use for all log output.
 * Note: the rust side will make its own shallow copy of the logger struct
 * (not just the pointer), and will free that when log_st_unset_rs() is called.
 */
log_status_rs_e log_st_set_rs(struct logger *log, log_level_rs_e level);

bool log_st_is_setup_rs(void);
log_status_rs_e log_st_log_rs(struct bstring *msg, log_level_rs_e level);

void log_st_set_max_level_rs(log_level_rs_e level);

/* Tell the rust side to stop logging to its logger and free resources.
 * Returns true if an action was taken.
 */
bool log_st_unset_rs(void);
void log_st_flush_rs(void);

struct log_mt_config_rs {
    char *path;
    char *file_basename;
    uint32_t buf_size;
    log_level_rs_e level;
};

struct Handle;

struct Handle* log_mt_create_handle(struct log_mt_config_rs *cfg);
void log_mt_destroy_handle(struct Handle **h);

#ifdef __cplusplus
}
#endif
