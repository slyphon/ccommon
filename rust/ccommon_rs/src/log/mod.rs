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

#![allow(dead_code)]

use cc_binding as bind;
use rslog::{Log, Metadata, Record, SetLoggerError};
pub use rslog::Level;
use rslog::LevelFilter;
use std::cell::RefCell;
use std::ptr;
use time;

pub mod st;

// TODO(simms): add C-side setup code here.

/*
binding:

pub struct logger {
    pub name: *mut ::std::os::raw::c_char,
    pub fd: ::std::os::raw::c_int,
    pub buf: *mut rbuf,
}
*/

#[derive(Fail, Debug)]
pub enum LoggingError {
    #[fail(display = "logging already set up")]
    LoggingAlreadySetUp,

    #[fail(display = "Other logger has already been set up with log crate")]
    LoggerRegistrationFailure,
}

impl From<SetLoggerError> for LoggingError {
    fn from(_: SetLoggerError) -> Self {
        LoggingError::LoggerRegistrationFailure
    }
}

struct CLogger(RefCell<bind::logger>);

impl CLogger {
    /// Takes a raw pointer to a bind::logger struct and internalizes a copy of that
    /// struct for the lifetime of this object.
    pub unsafe fn from_raw(p: *mut bind::logger) -> CLogger {
        CLogger(RefCell::new(ptr::read(p)))
    }

    pub unsafe fn write(&self, message: &str) -> bool {
        let msg = message.as_bytes();
        let b = bind::log_write(self.0.as_ptr(), msg.as_ptr() as *mut i8, msg.len() as u32);
        if !b {
            eprintln!("failed to write to log: {}", message);
        }
        b
    }

    pub unsafe fn flush(&self) { bind::log_flush(self.0.as_ptr()); }
}


/// The API around writing to the underlying logger
#[doc(hidden)]
trait RawWrapper: Log {
    fn clogger(&self) -> Option<&CLogger>;
    fn level_filter(&self) -> LevelFilter;
    fn is_some(&self) -> bool;
    fn is_none(&self) -> bool;
}

struct Logger {
    inner: CLogger,
    filter: LevelFilter,
}

impl Logger {
    fn new(inner: CLogger, filter: LevelFilter) -> Self { Logger{inner, filter} }
}

// This is a VICIOUS LIE. We're not safe for mt use,
// but Log insists on it, so we lie to the compiler.
unsafe impl Send for Logger {}
unsafe impl Sync for Logger {}

impl RawWrapper for Logger {
    fn clogger(&self) -> Option<&CLogger> {
        Some(&self.inner)
    }

    fn level_filter(&self) -> LevelFilter {
        self.filter
    }

    fn is_some(&self) -> bool { true }
    fn is_none(&self) -> bool { false }
}

impl Log for Logger {
    #[inline]
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= self.level_filter()
    }

    fn log(&self, record: &Record) {
        // taken from borntyping/rust-simple_logger
        if self.enabled(record.metadata()) {
            if let Some(clog) = self.clogger() {
                let msg = format!(
                    "{} {:<5} [{}] {}\n",
                    time::strftime("%Y-%m-%d %H:%M:%S", &time::now()).unwrap(),
                    record.level().to_string(),
                    record.module_path().unwrap_or_default(),
                    record.args());

                unsafe { clog.write(msg.as_ref()); }
            }
        }
    }

    #[inline]
    fn flush(&self) {
        if let Some(clog) = self.clogger().as_mut() {
            unsafe { clog.flush() }
        }
    }
}

struct NopLogger;

impl RawWrapper for NopLogger {
    fn clogger(&self) -> Option<&CLogger> { None }
    fn level_filter(&self) -> LevelFilter { LevelFilter::Off }
    fn is_some(&self) -> bool { false }
    fn is_none(&self) -> bool { true }
}

impl Log for NopLogger {
    fn enabled(&self, _: &Metadata) -> bool { false }
    fn log(&self, _: &Record) {}
    fn flush(&self) {}
}

#[repr(u32)]
#[derive(Debug, PartialEq, PartialOrd, Eq)]
pub enum LoggerStatus {
    OK = 0,
    LoggerNotSetupError = 1,
    RegistrationFailure = 2,
    LoggerAlreadySetError = 3,
    InvalidUTF8 = 4,
}

impl From<LoggingError> for LoggerStatus {
    fn from(e: LoggingError) -> Self {
        match e {
            LoggingError::LoggerRegistrationFailure => LoggerStatus::RegistrationFailure,
            LoggingError::LoggingAlreadySetUp => LoggerStatus::LoggerAlreadySetError,
        }
    }
}


#[repr(usize)]
#[derive(Debug, Eq, PartialEq)]
enum ModuleState {
    UNINITIALIZED = 0,
    INITIALIZING,
    INITIALIZED,
    FAILED,
}

impl From<usize> for ModuleState {
    fn from(u: usize) -> Self {
        match u {
            0 => ModuleState::UNINITIALIZED,
            1 => ModuleState::INITIALIZING,
            2 => ModuleState::INITIALIZED,
            3 => ModuleState::FAILED,
            _ => unreachable!()
        }
    }
}


