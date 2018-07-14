extern crate cc_binding;
extern crate crossbeam;
extern crate failure;
#[macro_use]
extern crate failure_derive;
extern crate lazy_static;
#[macro_use]
extern crate log as rslog;
extern crate tempfile;
extern crate time;
extern crate thread_local;
extern crate thread_id;

#[cfg(test)]
#[macro_use]
extern crate rusty_fork;

use std::result;

pub mod bstring;
pub mod log;

// like how guava provides enhancements for Int as "Ints"
pub mod ptrs;

pub type Result<T> = result::Result<T, failure::Error>;
