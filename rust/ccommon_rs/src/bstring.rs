use cc_binding as bind;
use std::boxed::Box;
use std::convert::AsMut;
use std::ffi::CString;
use std::io;
use std::ops::{Deref, DerefMut};
use std::os::raw::c_char;
use std::slice;
use std::result::Result;
use std::str::Utf8Error;
use std::str;
use std::mem;

pub type RawBString = bind::bstring;


// TODO: Differentiate between native (rust-allocated) bstrings and foreign (c-allocated) ones

/// A wrapper around a string reference that returns a properly initialized `*mut bstring` pointer.
/// Useful for testing and possibly in other circumstances.
///
pub struct BStringStr<'a>(pub &'a str);

impl<'a> BStringStr<'a> {
    #[allow(dead_code)]
    pub fn into_raw(self) -> *mut RawBString {
        let bs = RawBString{
            len: self.0.len() as u32,
            data: CString::new(self.0).unwrap().into_raw(),
        };

        Box::into_raw(Box::new(bs))
    }

    /// Frees a BStringStr that was previously converted into a `*mut bstring` via the
    /// into_raw method. Passing this method a pointer created through other means
    /// may lead to undefined behavior.
    #[allow(dead_code)]
    pub unsafe fn free(ptr: *mut RawBString) {
        let b: Box<RawBString> = Box::from_raw(ptr);
        // reclaim pointer from the bstring, allowing it to be freed
        drop(CString::from_raw(b.data));
        drop(b);
    }
}


/// BStringRef provides a wrapper around a raw pointer to a cc_bstring. It's important to note that
/// this struct does not take ownership of the underlying pointer, nor does it free it when it's
/// dropped.
///
/// # Examples
/// ```rust
/// use ccommon_rs::bstring::{BStringStr, BStringRef};
///
/// let s = "sea change";
/// let bsp = BStringStr(s).into_raw();
/// let bsr = unsafe { BStringRef::from_raw(bsp) };
///
/// assert_eq!(&bsr[0..4], b"sea ");
/// assert_eq!(&bsr[0..10], b"sea change");
/// assert_eq!(&bsr[..], b"sea change");
///
/// unsafe { BStringStr::free(bsp) };
/// ```
// see go/rust-newtype-pattern
pub struct BStringRef<'a> {
    inner: &'a RawBString,
}

impl<'a> BStringRef<'a> {
    pub unsafe fn from_raw(ptr: *const RawBString) -> Self {
        assert!(!ptr.is_null());
        let inner = &*ptr;
        BStringRef{inner}
    }

    #[allow(dead_code)]
    pub fn into_raw(self) -> *const RawBString {
        self.inner as *const RawBString
    }

    pub fn to_str(&self) -> Result<&str, Utf8Error> {
        str::from_utf8(&*self)
    }

    pub fn new(b: &'a RawBString) -> BStringRef<'a> {
        BStringRef{inner: b}
    }
}

impl<'a> Deref for BStringRef<'a> {
    type Target = [u8];

    fn deref(&self) -> &<Self as Deref>::Target {
        unsafe {
            slice::from_raw_parts(
                self.inner.data as *const c_char as *const u8,  // cast *const i8 -> *const u8
                self.inner.len as usize
            )
        }
    }
}

/// BStringRef provides a wrapper around a raw pointer to a cc_bstring. It's important to note that
/// this struct does not take ownership of the underlying pointer, nor does it free it when it's
/// dropped.
///
/// # Examples
/// ```rust
/// # use ccommon_rs::bstring::{BStringStr, BStringRefMut};
///
/// use std::io::Write;
///
/// let s = "sea change";
/// let bsp = BStringStr(s).into_raw();
/// let mut bsr = unsafe { BStringRefMut::from_raw(bsp) };
///
/// let d = vec![0u8, 1u8, 2u8];
/// assert_eq!(d.len(), 3);
///
/// {
///     let mut buf: &mut [u8] = &mut bsr;
///     let n = buf.write(&d).unwrap();
///     assert_eq!(n, 3);
/// }
///
/// assert_eq!(&bsr[0..3], &d[0..3]);
///
/// unsafe { BStringStr::free(bsp) };
/// ```
#[derive(Debug)]
pub struct BStringRefMut<'a> {
    inner: &'a mut RawBString,
}

impl<'a> BStringRefMut<'a> {
    pub unsafe fn from_raw(ptr: *mut RawBString) -> Self {
        assert!(!ptr.is_null());
        BStringRefMut{inner: &mut *ptr}
    }

    pub fn into_raw(self) -> *mut RawBString {
        self.inner as *mut RawBString
    }
}

impl<'a> Deref for BStringRefMut<'a> {
    type Target = [u8];

    fn deref(&self) -> &<Self as Deref>::Target {
        unsafe {
            slice::from_raw_parts(
                self.inner.data as *const c_char as *const u8,
                self.inner.len as usize
            )
        }
    }
}

impl<'a> DerefMut for BStringRefMut<'a> {
    fn deref_mut(&mut self) -> &mut <Self as Deref>::Target {
        unsafe {
            slice::from_raw_parts_mut(
                self.inner.data as *mut u8,
                self.inner.len as usize
            )
        }
    }
}

impl<'a> io::Write for BStringRefMut<'a> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        DerefMut::deref_mut(self).write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        DerefMut::deref_mut(self).flush()
    }
}

impl<'a> AsMut<RawBString> for BStringRefMut<'a> {
    fn as_mut(&mut self) -> &mut RawBString {
        self.inner
    }
}


/// An owned BString
pub struct BString(RawBString);

impl BString {
    pub fn new(size: u32) -> Self {
        BString::from(vec![0u8; size as usize].into_boxed_slice())
    }

    pub fn into_raw(self) -> *mut RawBString {
        let mut uniq = self.0;
        mem::forget(self);

        (&mut uniq) as *mut RawBString
    }

    pub unsafe fn from_raw(ptr: *mut RawBString) -> BString {
        assert!(!ptr.is_null());
        BString(*ptr)
    }

    fn as_slice(&self) -> &[u8] {
        unsafe {
            slice::from_raw_parts(
                self.0.data as *const _ as *const u8,
                self.0.len as usize
            )
        }
    }

    fn as_slice_mut(&mut self) -> &mut [u8] {
        unsafe {
            slice::from_raw_parts_mut(
                self.0.data as *mut _ as *mut u8,
                self.0.len as usize
            )
        }
    }
}

impl Deref for BString {
    type Target = [u8];

    fn deref(&self) -> &<Self as Deref>::Target {
        self.as_slice()
    }
}

impl DerefMut for BString {
    fn deref_mut(&mut self) -> &mut <Self as Deref>::Target {
        self.as_slice_mut()
    }
}

impl From<Box<[u8]>> for BString {
    fn from(b: Box<[u8]>) -> Self {
        BString(
            RawBString {
                len: b.len() as u32,
                data: Box::into_raw(b) as *mut _ as *mut c_char
            }
        )
    }
}

impl From<Vec<u8>> for BString {
    fn from(v: Vec<u8>) -> Self {
        BString::from(v.into_boxed_slice())
    }
}

impl AsRef<RawBString> for BString {
    fn as_ref(&self) -> &RawBString { &self.0 }
}

