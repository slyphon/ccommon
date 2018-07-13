extern crate cc_binding as bind;
extern crate ccommon_rs as ccommon;
#[macro_use]
extern crate log as rs_log;
#[macro_use]
extern crate rusty_fork;
extern crate tempfile;

use ccommon::log::{Level, LoggerStatus};
use ccommon::log::CLogger;
use ccommon::log::LogMetrics;
use ccommon::log::st as log_st;
use ccommon::Result;
use rs_log::LevelFilter;
use std::fs::File;
use std::io::Read;
use std::str;

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

fn basic_st_roundtrip() {
    assert_result(|| {
        let mut stats = LogMetrics::new();
        unsafe { bind::log_setup(stats.as_mut_ptr()) };

        let tf = tempfile::NamedTempFile::new()?;
        let pb = tf.path().to_path_buf();
        let path = pb.to_str().unwrap();

        let mut logger = unsafe { CLogger::open(path, 0)? };

        assert_eq!(log_st::log_st_setup_rs(), LoggerStatus::OK);
        assert_eq!(unsafe{log_st::log_st_set_rs(logger.as_mut_ptr(), Level::Debug)}, LoggerStatus::OK);
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

        drop(logger);
        drop(stats);

        Ok(())
    })
}


// runs this test with process isolation
rusty_fork_test! {
    #[test]
    fn test_basic_st_roundtrip() { basic_st_roundtrip() }
}

