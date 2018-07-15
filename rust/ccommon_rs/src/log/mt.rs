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

//! Threadsafe glue between the `log` crate and `cc_log`.
//!
//! The C side configures this module with a directory and base filename.
//! When a Rust thread calls one of the logging macros, a new logger is
//! created with a unique filename (based either on the thread's name or
//! its posix unique id) and stored in a thread local variable. At shutdown,
//! the struct that refers to the thread-local loggers is atomically
//! swapped out for a no-op logger, and the thread-local loggers are flushed
//! and shut down cleanly.
//!
//! This configuration is a shared-nothing lockless design...for _SPEED_.

use cc_binding as bind;
use crossbeam::sync::ArcCell;
use failure;
use log::*;
use ptrs;
use rslog;
use std::cell::RefCell;
use std::ffi::CString;
use std::path::PathBuf;
use std::ptr;
use std::sync::Arc;
use std::thread;
use thread_id;
use thread_local::CachedThreadLocal;
use time;

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


impl LogConfig {
    #[doc(hidden)]
    pub fn from_raw(ptr: *mut bind::log_mt_config_rs) -> Result<Self> {
        ptrs::lift_to_option(ptr)
            .ok_or_else(|| ptrs::NullPointerError.into())
            .and_then(|ptr| {
                Ok(LogConfig {
                    path: unsafe { CString::from_raw((*ptr).path).to_str()?.to_owned() },
                    file_basename: unsafe { CString::from_raw((*ptr).file_basename) }.to_str()?.to_owned(),
                    buf_size: unsafe {(*ptr).buf_size},
                    level: Self::from_usize(unsafe { (*ptr).level } as usize).unwrap(),
                })
            })
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
    /// The underlying cc_log logger instance
    clogger: CLogger,
    /// The cached thread name or unique identifier
    thread_name: String,
    /// This buffer is used for preparing the message to be logged
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


impl Log for PerThreadLog {
    fn enabled(&self, _: &Metadata) -> bool {
        true
    }

    fn log(&self, record: &Record) {
        if self.enabled(record.metadata()) {
            let mut buf = self.buf.borrow_mut();
            let sz = format(record, &mut buf).unwrap();
            unsafe { self.clogger.write(&buf[0..sz]); }
        }
    }

    fn flush(&self) {
        unsafe { self.clogger.flush(); }
    }
}

#[repr(C)]
struct Shim {
    tls: CachedThreadLocal<RefCell<Option<PerThreadLog>>>,
    cfg: LogConfig,
}

impl Shim {
    fn get_per_thread(&self) -> super::Result<&RefCell<Option<PerThreadLog>>> {
        self.tls.get_or_try(||
            PerThreadLog::for_current(&self.cfg)
                .map(|ptl| Box::new(RefCell::new(Some(ptl))) )
        )
    }

    fn new(cfg: LogConfig) -> Self {
        Shim { cfg, tls: CachedThreadLocal::new() }
    }

    fn shutdown(&mut self) {
        for cell in self.tls.iter_mut() {
            if let Some(ptl) = cell.replace(None) {
                ptl.flush();
                drop(ptl);
            }
        }
    }

    #[inline]
    fn borrow_and_call<F>(&self, f: F) -> Option<failure::Error>
        where F: FnOnce(&PerThreadLog)
    {
        self.get_per_thread()
            .map(|cell| {
                if let Some(ptl) = &*cell.borrow() {
                    f(ptl);
                }
            })
            .err()
    }
}

impl Log for Shim {
    fn enabled(&self, _: &Metadata) -> bool {
        true
    }

    fn log(&self, record: &Record) {
        if let Some(err) = self.borrow_and_call(|ptl| ptl.log(record)) {
            eprintln!("err in Shim::log {:#?}", err);
        }
    }

    fn flush(&self) {
        if let Some(err) = self.borrow_and_call(|ptl| ptl.flush()) {
            eprintln!("err in Shim::flush {:#?}", err);
        }
    }
}

/// This is the Log instance we give to the log crate. Its job is to
/// hold onto the `Shim` and dispatch calls to it. See `Handle`
/// for a description of the inner structure.
///
#[doc(hidden)]
struct Logger(Arc<ArcCell<Option<Shim>>>);

impl Log for Logger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        if let Some(n) = &*self.0.get() {
            n.enabled(metadata)
        } else {
            false
        }
    }

    fn log(&self, record: &Record) {
        if let Some(log) = &*self.0.get() {
            log.log(record);
        }
    }

    fn flush(&self) {
        if let Some(log) = &*self.0.get() {
            log.flush();
        }
    }
}


/// This is essentially `Arc->ArcCell->Arc->Option->Shim`. The outermost `Arc` is shared
/// between the log crate and this `Handle` that
/// we return to the user to allow them to shut down.
///
/// ```
///
///       +-------------------------------+
///       |                               |
///       |                               |
/// +----------+                          |
/// |   Arc    |                          v
/// |          |            +--------------------------+
/// |  Logger  |            |          ArcCell         |
/// |          |            | +----------------------+ |
/// |          |            | |         Arc          | |
/// +----------+            | |    +------------+    | |
///                         | |    |   Option   |    | |
/// +----------+            | |    |  +------+  |    | |
/// |          |            | |    |  | Shim |  |    | |
/// |          |            | |    |  +------+  |    | |
/// |  Handle  |            | |    +------------+    | |
/// |          |            | +----------------------+ |
/// |   Arc    |            |                          |
/// +----------+            +--------------------------+
///       |                               ^
///       |                               |
///       +-------------------------------+
/// ```
///
/// We perform the shutdown
/// by first swapping out the innermost `Arc` for a no-op (None) version, then unboxing and
/// shutting down the per-thread loggers in the `Shim`.
#[repr(C)]
pub struct Handle {
    shim: Arc<ArcCell<Option<Shim>>>
}

impl Handle {
    fn shutdown(&mut self, timeout: time::Duration) {
        let mut active: Arc<Option<Shim>> = self.shim.set(Arc::new(None));

        let stop_at = time::SteadyTime::now() + timeout;

        loop {
            if let Some(opt_shim) = Arc::get_mut(&mut active) {
                if let Some(shim) = opt_shim {
                    shim.shutdown();
                    break
                }
            } else {
                eprintln!("failed to get_mut on the active logger");
                thread::yield_now();
            }

            if time::SteadyTime::now() < stop_at {
                break
            }
        }
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        self.shutdown(time::Duration::zero());
    }
}

fn log_mt_setup_safe(config: LogConfig) -> Result<Handle> {
    rslog::set_max_level(config.level.to_level_filter());
    let shim = Shim::new(config);
    let logger = Logger(Arc::new(ArcCell::new(Arc::new(Some(shim)))));

    let handle = Handle{shim: logger.0.clone()};

    rslog::set_boxed_logger(Box::new(logger))
        .map(|()| handle)
        .map_err(|e| e.into())
}

#[no_mangle]
pub unsafe extern "C" fn log_mt_create_handle(cfgp: *mut bind::log_mt_config_rs) -> *mut Handle {
    ptrs::null_check(cfgp)                              // make sure our input is good
        .map_err(|e| e.into())                          // error type bookkeeping
        .and_then(LogConfig::from_raw)                  // convert the *mut into a rust struct
        .and_then(log_mt_setup_safe)                    // register our logger
        .map(|handle| Box::into_raw(Box::new(handle)))  // convert our handle into a raw pointer
        .unwrap_or_else(|err| {                         // hand it back to C
            eprintln!("failure in log_mt_create_handle: {:#?}", err);
            ptr::null_mut()                             // unless there was an error, then return NULL
        })
}

#[no_mangle]
pub unsafe extern "C" fn log_mt_shutdown_rs(ph: *mut Handle) -> LoggerStatus {
    let handle =
        match ptrs::lift_to_option(ph) {
            Some(ph) => Box::from_raw(ph),
            None => return LoggerStatus::NullPointerError,
        };

    drop(handle);
    LoggerStatus::OK
}

#[no_mangle]
pub unsafe extern "C" fn log_mt_destroy_handle_rs(pph: *mut *mut Handle) {
    assert!(!pph.is_null());
    let ph = *pph;
    drop(Box::from_raw(ph));
    *pph = ptr::null_mut();
}

// for integration testing with C
#[doc(hidden)]
#[no_mangle]
pub unsafe extern "C" fn log_mt_test_threaded_writes_rs() -> bool {
    let t1 = thread::spawn(move || {
        for x in 0..10 {
            error!("thread 1: {}", x);
        }
    });

    let t2 = thread::spawn(move || {
        for x in 0..10 {
            warn!("thread 2: {}", x);
        }
    });

    t1.join().unwrap();
    t2.join().unwrap();

    true
}

#[cfg(test)]
mod test {
    use std::fs;
    use std::sync::mpsc;
    use super::*;
    use tempfile;
    use time;


    // this is necessary until https://github.com/rust-lang/rust/issues/48854
    // lands in stable
    fn assert_result<F, E>(f: F)
        where F: FnOnce() -> Result<E>
    {
        match f() {
            Ok(_) => (),
            Err(e) => panic!(e)
        }
    }

    fn basic_mt_roundtrip() {
        assert_result(|| {
            let mut stats = LogMetrics::new();
            unsafe { bind::log_setup(stats.as_mut_ptr()) };
            let tmpdir = tempfile::tempdir()?;

            let cfg = LogConfig {
                path: tmpdir.path().to_path_buf().to_str().unwrap().to_owned(),
                file_basename: String::from("testmt"),
                buf_size: 0,
                level: Level::Trace,
            };

            let handle = log_mt_setup_safe(cfg).unwrap();

            let t1 = thread::spawn(move || {
                error!("thread 1 error");
            });

            let t2 = thread::spawn(move || {
                warn!("thread 2 error");
            });

            t1.join().unwrap();
            t2.join().unwrap();

            drop(handle);

            Ok(())
        })
    }


    fn build(name: &str) -> thread::Builder {
        thread::Builder::new().name(name.to_owned())
    }

    fn named_threads_test() {
        assert_result(||{
            let mut stats = LogMetrics::new();
            unsafe { bind::log_setup(stats.as_mut_ptr()) };
            let tmpdir = tempfile::tempdir()?;

            let cfg = LogConfig {
                path: tmpdir.path().to_path_buf().to_str().unwrap().to_owned(),
                file_basename: String::from("testmt"),
                buf_size: 0,
                level: Level::Trace,
            };

            let handle = log_mt_setup_safe(cfg).unwrap();

            let t1 = build("d_level").spawn(move || {
                debug!("debug message");
            }).unwrap();

            let t2 = build("w_level").spawn(move || {
                warn!("warn message");
            }).unwrap();

            t1.join().unwrap();
            t2.join().unwrap();

            drop(handle);

            {
                let mut dlevelp = tmpdir.path().to_owned();
                dlevelp.push("testmt.d_level.log");
                let md = fs::metadata(dlevelp)?;
                assert!(md.len() > 0);
            }

            {
                let mut wlevelp = tmpdir.path().to_owned();
                wlevelp.push("testmt.w_level.log");
                let md = fs::metadata(wlevelp)?;
                assert!(md.len() > 0);
            }

            Ok(())
        })
    }

    fn mt_shutdown_resilience_test() {
        assert_result(||{
            // make sure a thread logging doesn't crash if we shutdown simultaneously
            let mut stats = LogMetrics::new();
            unsafe { bind::log_setup(stats.as_mut_ptr()) };
            let tmpdir = tempfile::tempdir()?;

            let cfg = LogConfig {
                path: tmpdir.path().to_path_buf().to_str().unwrap().to_owned(),
                file_basename: String::from("testmt"),
                buf_size: 0,
                level: Level::Trace,
            };

            let handle = log_mt_setup_safe(cfg).unwrap();

            let (start_tx, start_rx) = mpsc::sync_channel::<String>(0);
            let (stop_tx, stop_rx) = mpsc::sync_channel::<bool>(0);
            let (loop_tx, loop_rx) = mpsc::sync_channel::<u64>(300);

            eprintln!("start thread");
            let th = build("worker").spawn(move||{
                eprintln!("thread started, waiting for message");
                let msg = start_rx.try_recv().unwrap();
                eprintln!("got start msg: {}", msg);

                let mut count: u64 = 0;
                loop {
                    let tm = time::now_utc();
                    trace!("{:#?}", tm.to_timespec());
                    count += 1;
                    loop_tx.send(count).unwrap();

                    match stop_rx.try_recv() {
                        Ok(_) => {
                            eprintln!("received stop signal");
                            break;
                        },
                        Err(mpsc::TryRecvError::Disconnected) => {
                            eprintln!("gah! disconnected!");
                            panic!("bad things!");
                        },
                        Err(mpsc::TryRecvError::Empty) => ()
                    };
                }

                eprintln!("while loop exited");
                count
            }).unwrap();

            start_tx.send("GO!".to_owned())?;

            let delay = ::std::time::Duration::from_millis(100);

            assert_eq!(loop_rx.recv_timeout(delay)?, 1);

            eprintln!("dropping handle");
            drop(handle);

            // make sure the thread writes another message or two
            assert_eq!(loop_rx.recv_timeout(delay)?, 2);
            assert_eq!(loop_rx.recv_timeout(delay)?, 3);

            // signal to the thread that it should exit
            stop_tx.send(true)?;

            eprintln!("joining");
            let count = th.join().unwrap();
            eprintln!("thread joined, wrote {} messages", count);

            Ok(())
        })
    }

    // runs this test with process isolation
    rusty_fork_test! {
        #[test]
        fn test_basic_mt_roundtrip() { basic_mt_roundtrip(); }
    }

    rusty_fork_test! {
        #[test]
        fn test_named_threads() { named_threads_test(); }
    }

    rusty_fork_test! {
        #[test]
        fn test_shutdown_resilience() { mt_shutdown_resilience_test(); }
    }
}

