// ccommon - a cache common library.
// Copyright (C) 2018 Twitter, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Glue between rust's standard `log` crate and ccommon's cc_log logger.
//!
//! This library allows rust embedded into projects using ccommon to use
//! the same logger provided by `cc_log.h`
//!
//! # Safety
//!
//! This library is AGGRESSIVELY NON-THREADSAFE...for SPEED.
//!
//! If you are using the standard rust macros for logging, you must
//! ensure that you are running your rust code from a single thread or
//! _bad things may happen_.

#![allow(dead_code)]

use bstring::BString;
use bstring::BStringRef;
use cc_binding as bind;
use rslog;
use rslog::{Log, Metadata, Record};
pub use rslog::Level;
use std::result::Result;
use std::sync::atomic::{ATOMIC_USIZE_INIT, AtomicUsize, Ordering};
use super::{CLogger, Logger, LoggerStatus, LoggingError, ModuleState, NopLogger, RawWrapper};

static mut LOGGER: &'static RawWrapper = &NopLogger;

struct ShimLog;

impl Log for ShimLog {
    fn enabled(&self, metadata: &Metadata) -> bool {
        unsafe { LOGGER.enabled(metadata) }
    }

    fn log(&self, record: &Record) {
        unsafe { LOGGER.log(record) }
    }

    fn flush(&self) {
        unsafe { LOGGER.flush() }
    }
}

static STATE: AtomicUsize = ATOMIC_USIZE_INIT;

fn get_state() -> ModuleState {
    let u = STATE.fetch_add(0, Ordering::SeqCst);
    ModuleState::from(u)
}


/// Establishes this module as the rust `log` crate's singleton logger. We first install a
/// no-op logger, and then replace it with an actual logging instance that has an output.
/// Returns a [`ccommon::Result`] that is Ok on success and will be a [`LoggingError`] on failure.
pub(crate) fn try_init_logger() -> Result<(), LoggingError> {
    match get_state() {
        ModuleState::UNINITIALIZED => (),
        ModuleState::INITIALIZED => return Ok(()),
        ModuleState::FAILED => return Err(LoggingError::LoggerRegistrationFailure),
        _ => (),
    };

    if STATE.compare_and_swap(
        ModuleState::UNINITIALIZED as usize,
        ModuleState::INITIALIZING as usize,
        Ordering::SeqCst) != (ModuleState::UNINITIALIZED as usize) {
        return Err(LoggingError::LoggingAlreadySetUp)
    }

    match rslog::set_logger(Box::leak(Box::new(ShimLog{}))) {
        Ok(_) => {
            // set the default max level to 'trace' and provide an API to adjust it
            rslog::set_max_level(rslog::LevelFilter::Trace);
            STATE.store(ModuleState::INITIALIZED as usize, Ordering::SeqCst);
            Ok(())
        }
        Err(err) => {
            eprintln!("Error setting up logger: {}", err);
            STATE.store(ModuleState::FAILED as usize, Ordering::SeqCst);
            Err(err)
        }
    }.map_err(|e| e.into())
}

/// This function will set up our logger as the default
/// one for the `log` crate at the given
/// `level`. This function must be called as early
/// as possible in program setup, followed by
/// a call to [`log_rs_set`]
///
/// [`log_rs_set`]: fn.log_rs_set.html
///
/// # Errors
///
/// If we fail to set up our logger, we will print a
/// message on stderr and return
/// [`LoggerStatus::RegistrationFailure`], which means
/// we could not register ourselves as the provider
/// of the logging backend for the `log` crate.
/// This should be treated as a fatal error because
/// one cannot un-register the existing backend, and
/// this operation will *never* succeed.
///
/// If this method had been called previously,
/// and we are the provider of the logging framework,
/// we return [`Ok`].
///
/// # Safety
///
/// The caller must ensure that the lifetime of `logger`
/// lives until `rust_cc_log_destroy`
/// is called or the program terminates.
#[no_mangle]
pub extern "C" fn log_st_setup_rs() -> LoggerStatus {
    match try_init_logger() {
        Ok(_) => LoggerStatus::OK,
        Err(LoggingError::LoggingAlreadySetUp) => {
            eprintln!("cc_log logging already set up");
            LoggerStatus::OK
        }
        Err(LoggingError::LoggerRegistrationFailure) => {
            eprintln!("Error setting up cc_log! {}", LoggingError::LoggerRegistrationFailure);
            LoggerStatus::RegistrationFailure
        }
    }
}

/// This function sets the cc_log logger instance to be the
/// sink for messages logged from the `log` crate. The user
/// must call [`log_rs_setup`] _before_ calling this function
/// to register us as the backend for the `log` crate.
///
/// # Panics
///
/// This function will panic if the `logger` pointer is NULL.
///
/// # Errors
///
/// Returns [`LoggerNotSetupError`] if [`log_rs_setup`] was NOT
/// called prior to this function being called.
///
/// If there's already been a `logger` instance set up, then we will return
/// [`LoggerAlreadySetError`]. This error need not be fatal.
///
/// [`log_rs_setup`]: fn.log_rs_setup.html
/// [`LoggerNotSetupError`]: enum.LoggerStatus.html
/// [`LoggerAlreadySetError`]: enums.LoggerStatus.html
///
/// # Undefined Behavior
///
/// If the `logger` pointer becomes invalid before [`log_rs_unset`] is called, the
/// behavior is undefined.
///
/// [`log_rs_unset`]: fn.log_rs_unset.html
#[no_mangle]
pub unsafe extern "C" fn log_st_set_rs(logger: *mut bind::logger, level: Level) -> LoggerStatus {
    assert!(!logger.is_null());

    let cur_state = get_state();
    if cur_state != ModuleState::INITIALIZED {
        eprintln!("log_rs_set: error state was: {:?}", cur_state);
        return LoggerStatus::LoggerNotSetupError
    }

    if LOGGER.is_none() {
        let clogger = CLogger::from_raw(logger);
        LOGGER = Box::leak(Box::new(Logger::new(clogger, level.to_level_filter())));

        LoggerStatus::OK
    } else {
        LoggerStatus::LoggerAlreadySetError
    }
}

/// Returns true if [`log_setup_rs`] has been called previously and
/// it is safe to set the logger instance.
#[no_mangle]
pub unsafe extern "C" fn log_st_is_setup_rs() -> bool {
    if get_state() != ModuleState::INITIALIZED {
        return false;
    }

    LOGGER.clogger().is_some()
}


/// Log a message through the rust path at the given level.
/// Useful for testing from the C side that the rust side is properly set up.
///
/// # Errors
///
/// [`LoggerStatus::InvalidUTF8`] will be returned if the
/// bstring's contents are not valid UTF8.
///
/// # Panics
///
/// This function panics if the `msg` pointer is NULL.
#[no_mangle]
pub unsafe extern "C" fn log_st_log_rs(msg: *const BString, level: Level) -> LoggerStatus {
    assert!(!msg.is_null());
    let bsr = BStringRef::from_raw(msg);

    match bsr.to_str() {
        Ok(s) => {
            log!(level, "{}", s);
            LOGGER.flush();
        },
        Err(err) => {
            eprintln!("error in log_rs_log: {:?}", err);
            return LoggerStatus::InvalidUTF8;
        }
    }

    LoggerStatus::OK
}


/// Set the level at which the rust logging macros should be active.
/// Default is 'Trace' which allows messages at all levels.
pub extern "C" fn log_st_set_max_level_rs(level: Level) {
    rslog::set_max_level(level.to_level_filter())
}

/// Replace the existing `logger` instance with a no-op logger and returns
/// the instance. If there is no current logger instance, returns NULL.
#[no_mangle]
pub unsafe extern "C" fn log_st_unset_rs() -> bool {
    if LOGGER.is_none() {
        return false
    }

    LOGGER = &NopLogger;
    true
}


/// Flushes the current logger instance by calling the
/// underlying `log_flush` function in cc_log.
///
/// # Undefined Behavior
///
/// If the underlying `logger` pointer has become
/// invalid the behavior is undefined.
#[no_mangle]
pub unsafe extern "C" fn log_st_flush_rs() {
    LOGGER.flush()
}
