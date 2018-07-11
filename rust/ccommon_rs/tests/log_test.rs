extern crate cc_binding as bind;
extern crate ccommon_rs as ccommon;
#[macro_use]
extern crate log as rs_log;
#[macro_use]
extern crate rusty_fork;
extern crate tempfile;

use ccommon::log::st as log_st;
use ccommon::log::{Level, LoggerStatus};
use ccommon::Result;
use std::ffi::CString;
use std::fs::File;
use std::io::Read;
use std::str;
use rs_log::LevelFilter;

// this is necessary until https://github.com/rust-lang/rust/issues/48854
// lands in stable
fn assert_result<F, E>(f: F)
where F: FnOnce() -> Result<E> {
    match f() {
        Ok(_) => (),
        Err(e) => panic!(e)
    }
}

fn basic_roundtrip() {
    assert_result(|| {
        let mut stats: *mut bind::log_metrics_st = unsafe { bind::log_metrics_create() };
        assert!(!stats.is_null());

        unsafe { bind::log_setup(stats) };

        let tf = tempfile::NamedTempFile::new()?;
        let pb = tf.path().to_path_buf();
        let path = pb.to_str().unwrap();

        let mut logger: *mut bind::logger = unsafe {
            bind::log_create(CString::new(path)?.into_raw(), 0)
        };
        assert!(!logger.is_null());

        assert_eq!(log_st::log_st_setup_rs(), LoggerStatus::OK);
        assert_eq!(unsafe{log_st::log_st_set_rs(logger, Level::Debug)}, LoggerStatus::OK);
        rs_log::set_max_level(LevelFilter::Trace);

        let logged_msg = "this message should be sent to the cc logger";

        error!("msg: {}", logged_msg);

        unsafe { log_st::log_st_flush_rs() };

        let mut buf = Vec::new();
        {
            let mut fp = File::open(path)?;
            let sz = fp.read_to_end(&mut buf)?;
            assert!(sz > logged_msg.len());
        }
        let s = str::from_utf8(&buf[..])?;
        assert!(s.rfind(logged_msg).is_some());

        let b = unsafe { log_st::log_st_unset_rs() };
        assert!(b);

        unsafe { bind::log_destroy(&mut logger) };
        unsafe { bind::log_metrics_destroy(&mut stats) }

        Ok(())
    })
}

// runs this test with process isolation
rusty_fork_test! {
    #[test]
    fn test_basic_roundtrip() { basic_roundtrip() }
}

