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

use cc_binding as bind;
use crossbeam::sync::ArcCell;
use log::*;
use rslog;
use std::cell::RefCell;
use std::error;
use std::io;
use std::ffi::CString;
use std::path::PathBuf;
use std::ptr;
use std::sync::Arc;
use std::thread;
use thread_id;
use thread_local::CachedThreadLocal;

#[repr(C)]
pub struct LogConfig {
    /// Path to the directory where we will write log files
    path: String,

    /// The basis for log filenames. If `foobar` is given,
    /// logs will be named `foobar.${thread_id}.log`. There will be one
    /// log created per thread. If the thread is named, that will be used
    /// as `thread_id` otherwise a unique identifier will be chosen.
    file_basename: String,

    /// What size buffer should the cc_log side use?
    buf_size: u32,

    level: Level,
}

/*
pub struct log_mt_config_rs {
    pub path: *mut ::std::os::raw::c_char,
    pub file_basename: *mut ::std::os::raw::c_char,
    pub buf_size: u32,
}
*/

impl LogConfig {
    pub unsafe fn from_raw(ptr: *mut bind::log_mt_config_rs) -> Result<Self> {
        let cfg = LogConfig {
            path: CString::from_raw((*ptr).path).to_str()?.to_owned(),
            file_basename: CString::from_raw((*ptr).file_basename).to_str()?.to_owned(),
            buf_size: (*ptr).buf_size,
            level: Self::from_usize((*ptr).level as usize).unwrap(),
        };

        Ok(cfg)
    }

    fn to_path_buf(&self, thread_id: &str) -> PathBuf {
        let mut pb = PathBuf::new();
        pb.push(&self.path);
        pb.push(format!("{}.{}.log", self.file_basename, thread_id));
        pb
    }

    fn from_usize(u: usize) -> Option<Level> {
        match u {
            1 => Some(Level::Error),
            2 => Some(Level::Warn),
            3 => Some(Level::Info),
            4 => Some(Level::Debug),
            5 => Some(Level::Trace),
            _ => None,
        }
    }
}

struct PerThreadLog {
    clogger: CLogger,
    thread_name: String,
    buf: RefCell<Vec<u8>>,
}

impl PerThreadLog {
    fn for_current(cfg: &LogConfig) -> super::Result<Self> {
        let tc = thread::current();
        let thread_name =
            tc.name()
                .map(|s| s.to_owned())
                .unwrap_or_else(|| { format!("{}", thread_id::get()) });

        let clogger = unsafe {
            CLogger::open(cfg.to_path_buf(&thread_name[..]).to_str().unwrap(), cfg.buf_size)?
        };

        let buf = RefCell::new(Vec::with_capacity(PER_THREAD_BUF_SIZE));

        Ok(PerThreadLog{thread_name, clogger, buf})
    }
}

unsafe impl Sync for PerThreadLog {}
unsafe impl Send for PerThreadLog {}

fn handle_error<E: error::Error + ?Sized>(e: &E) {
    let _ = writeln!(io::stderr(), "log_mt_rs: {}", e);
}

impl Log for PerThreadLog {
    fn enabled(&self, _: &Metadata) -> bool {
        true
    }

    fn log(&self, record: &Record) {
        let mut buf = self.buf.borrow_mut();
        let sz = format(record, &mut buf).unwrap();
        unsafe { self.clogger.write(&buf[0..sz]); }
    }

    fn flush(&self) {
        unsafe { self.clogger.flush(); }
    }
}

struct Shim {
    tls: CachedThreadLocal<PerThreadLog>,
    cfg: LogConfig,
}

impl Shim {
    fn get_per_thread(&self) -> super::Result<&PerThreadLog> {
        self.tls.get_or_try(|| PerThreadLog::for_current(&self.cfg).map(Box::new) )
    }

    fn new(cfg: LogConfig) -> Self {
        Shim { cfg, tls: CachedThreadLocal::new() }
    }
}

#[allow(unknown_lints)]
impl Log for Shim {
    fn enabled(&self, _: &Metadata) -> bool {
        true
    }

    #[allow(single_match)]
    fn log(&self, record: &Record) {
        match self.get_per_thread() {
            Ok(log) => log.log(record),
            Err(_) => () /* what to do here */
        }
    }

    #[allow(single_match)]
    fn flush(&self) {
        match self.get_per_thread() {
            Ok(log) => log.flush(),
            Err(_) => () /* what do? */
        }
    }
}


struct Logger(Arc<ArcCell<Shim>>);

impl Log for Logger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        self.0.get().enabled(metadata)
    }

    fn log(&self, record: &Record) {
        self.0.get().log(record)
    }

    fn flush(&self) {
        self.0.get().flush()
    }
}


pub struct Handle {
    shim: Arc<ArcCell<Shim>>
}


#[no_mangle]
pub unsafe extern "C" fn log_mt_setup(cfg: *mut bind::log_mt_config_rs) -> *mut Handle {
    let config = match LogConfig::from_raw(cfg) {
        Ok(c) => c,
        Err(err) => {
            eprintln!("log_mt_setup error {:?}", err);
            return ptr::null_mut();
        }
    };

    rslog::set_max_level(config.level.to_level_filter());
    let shim = Shim::new(config);
    let logger = Logger(Arc::new(ArcCell::new(Arc::new(shim))));
    let handle = Box::new(Handle{shim: logger.0.clone()});

    match rslog::set_boxed_logger(Box::new(logger)) {
        Ok(()) => Box::into_raw(handle),
        Err(err) => {
            eprintln!("log_mt_setup error {:?}", err);
            return ptr::null_mut();
        }
    }
}
