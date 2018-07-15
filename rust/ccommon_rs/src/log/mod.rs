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
use ptrs;
pub use rslog::{Level, Log, Metadata, Record, SetLoggerError};
use rslog::LevelFilter;
use std::cell::RefCell;
use std::ffi::CString;
use std::io::{Cursor, Write};
pub use super::Result;
use time;

pub mod st;
pub mod mt;

// TODO(simms): add C-side setup code here.

const PER_THREAD_BUF_SIZE: usize = 4096;

#[derive(Fail, Debug)]
pub enum LoggingError {
    #[fail(display = "logging already set up")]
    LoggingAlreadySetUp,

    #[fail(display = "Other logger has already been set up with log crate")]
    LoggerRegistrationFailure,

    #[fail(
        display = "cc_log_create failed. see stderr for message. path: {}, buf_size: {}",
        path, buf_size
    )]
    CreationError { path: String, buf_size: u32 },

}

impl From<SetLoggerError> for LoggingError {
    fn from(_: SetLoggerError) -> Self {
        LoggingError::LoggerRegistrationFailure
    }
}


#[doc(hidden)]
pub struct CLogger(*mut bind::logger);

impl CLogger {
    pub unsafe fn from_raw(p: *mut bind::logger) -> super::Result<CLogger> {
        ptrs::null_check(p).map(CLogger).map_err(|e| e.into())
    }

    pub unsafe fn write(&self, msg: &[u8]) -> bool {
        let b = bind::log_write(self.0, msg.as_ptr() as *mut i8, msg.len() as u32);
        if !b {
            eprintln!("failed to write to log: {:#?}", &msg);
        }
        b
    }

    pub unsafe fn flush(&self) { bind::log_flush(self.0); }

    pub unsafe fn open(path: &str, buf_size: u32) -> super::Result<CLogger> {
        let p = bind::log_create(CString::new(path)?.into_raw(), buf_size);

        ptrs::lift_to_option(p)
            .ok_or_else(|| LoggingError::CreationError {path: path.to_owned(), buf_size}.into())
            .map(CLogger)
    }

    pub fn as_mut_ptr(&mut self) -> *mut bind::logger { self.0 }
}

impl Drop for CLogger {
    fn drop(&mut self) {
        unsafe { bind::log_destroy(&mut self.0) }
    }
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
    buf: RefCell<Vec<u8>>,
}

impl Logger {
    fn new(inner: CLogger, filter: LevelFilter) -> Self {
        let buf = RefCell::new(Vec::with_capacity(PER_THREAD_BUF_SIZE));
        Logger{inner, filter, buf}
    }
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

fn format(record: &Record, buf: &mut Vec<u8>) -> Result<usize> {
    let tm = time::now_utc();

    let mut curs = Cursor::new(buf);

    let ts = time::strftime("%Y-%m-%d %H:%M:%S", &tm).unwrap();

    writeln!(
        curs,
        "{}.{:06} {:<5} [{}] {}",
        ts,
        tm.tm_nsec,
        record.level().to_string(),
        record.module_path().unwrap_or_default(),
        record.args()
    )?;

    Ok(curs.position() as usize)
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
                let mut buf = self.buf.borrow_mut();
                let sz = format(record, &mut buf).unwrap();
                unsafe { clog.write(&buf[0..sz]); }
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
    CreationError = 5,
    OtherFailure = 6,
    NullPointerError = 7,
}

impl From<LoggingError> for LoggerStatus {
    fn from(e: LoggingError) -> Self {
        match e {
            LoggingError::LoggerRegistrationFailure => LoggerStatus::RegistrationFailure,
            LoggingError::LoggingAlreadySetUp => LoggerStatus::LoggerAlreadySetError,
            LoggingError::CreationError{..} => LoggerStatus::CreationError,
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

#[cfg(test)]
pub(in log) struct LogMetrics(*mut bind::log_metrics_st);

#[cfg(test)]
impl LogMetrics {
    pub fn new() -> Self {
        let ptr = unsafe { bind::log_metrics_create() };
        assert!(!ptr.is_null());
        LogMetrics(ptr)
    }

    pub fn as_mut_ptr(&mut self) -> *mut bind::log_metrics_st { self.0 }
}

#[cfg(test)]
impl Drop for LogMetrics {
    fn drop(&mut self) {
        unsafe { bind::log_metrics_destroy(&mut self.0) }
    }
}
