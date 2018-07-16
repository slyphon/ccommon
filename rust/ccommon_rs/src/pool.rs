use super::Result;
use cc_binding as bind;
use std::collections::VecDeque;
use std::ops::Deref;
use std::os::raw::c_void;
use std::ptr;
use std::rc::Rc;
use ptrs;

struct BufMutator(Rc<Fn(&mut [u8])>);

impl BufMutator {
    pub fn new<F>(f: F) -> BufMutator
    where
        F: Fn(&mut [u8]) + 'static,
    {
        BufMutator(Rc::new(f))
    }
}

impl Deref for BufMutator {
    type Target = Fn(&mut [u8]);

    fn deref(&self) -> &<Self as Deref>::Target {
        self.0.as_ref()
    }
}

impl Clone for BufMutator {
    fn clone(&self) -> Self {
        BufMutator(self.0.clone())
    }
}

/// This is the type bindgen makes the various callbacks
struct CCallback(unsafe extern "C" fn(buf: *mut c_void));

impl From<CCallback> for BufMutator {
    fn from(cc: CCallback) -> Self {
        BufMutator::new(move |buf: &mut [u8]| unsafe { (cc.0)(buf as *mut _ as *mut c_void) })
    }
}

#[doc(hidden)]
#[derive(Clone)]
struct BufCallbacks {
    /// Called when a new buffer is allocated to initialize the contents
    init_cb: Option<BufMutator>,
    /// Called before a buffer is destroyed (freed) to deinitialize the ontents
    destroy_cb: Option<BufMutator>,
    /// Called before a buffer is returned to the free pool to clear any necessary state
    /// before it is borrowed again.
    reset_cb: Option<BufMutator>,
}

impl BufCallbacks {
    fn init(&self, buf: &mut [u8]) {
        self.init_cb.as_ref().map(|f| (f)(&mut buf[..]));
    }

    fn destroy(&self, buf: &mut [u8]) {
        self.destroy_cb.as_ref().map(|f| (f)(&mut buf[..]));
    }

    fn reset(&self, buf: &mut [u8]) {
        self.reset_cb.as_ref().map(|f| (f)(&mut buf[..]));
    }
}

struct BufCallbacksBuilder {
    init_fn: Option<BufMutator>,
    destroy_fn: Option<BufMutator>,
    reset_fn: Option<BufMutator>,
}

impl Default for BufCallbacksBuilder {
    fn default() -> Self {
        BufCallbacksBuilder {
            init_fn: None,
            destroy_fn: None,
            reset_fn: None,
        }
    }
}

#[allow(dead_code)]
impl BufCallbacksBuilder {
    pub fn new() -> Self {
        BufCallbacksBuilder::default()
    }

    pub fn init_fn<F>(&mut self, f: F) -> &mut Self
    where
        F: Fn(&mut [u8]) + 'static,
    {
        let new = self;
        new.init_fn = Some(BufMutator::new(f));
        new
    }

    pub fn raw_init_fn(&mut self, f: CCallback) -> &mut Self {
        let new = self;
        new.init_fn = Some(BufMutator::from(f));
        new
    }

    pub fn destroy_fn<F>(&mut self, f: F) -> &mut Self
    where
        F: Fn(&mut [u8]) + 'static,
    {
        let new = self;
        new.destroy_fn = Some(BufMutator::new(f));
        new
    }

    pub fn raw_destroy_fn(&mut self, f: CCallback) -> &mut Self {
        let new = self;
        new.destroy_fn = Some(BufMutator::from(f));
        new
    }

    pub fn reset_fn<F>(&mut self, f: F) -> &mut Self
    where
        F: Fn(&mut [u8]) + 'static,
    {
        let new = self;
        new.reset_fn = Some(BufMutator::new(f));
        new
    }

    pub fn raw_reset_fn(&mut self, f: CCallback) -> &mut Self {
        let new = self;
        new.reset_fn = Some(BufMutator::from(f));
        new
    }

    pub fn build(&self) -> Result<BufCallbacks> {
        Ok(BufCallbacks {
            init_cb: self.init_fn.as_ref().map(|f| Clone::clone(f)),
            destroy_cb: self.destroy_fn.as_ref().map(|f| Clone::clone(f)),
            reset_cb: self.reset_fn.as_ref().map(|f| Clone::clone(f)),
        })
    }
}

pub struct PoolConfig {
    obj_size: usize,
    nmax: usize,
    callbacks: BufCallbacks,
}

impl PoolConfig {
    unsafe fn from_raw(cfg: *const bind::pool_config_rs) -> Result<PoolConfig> {
        ptrs::null_check(cfg)
            .map_err(|e| e.into())
            .and_then(|cfg| {
                let mut cb = BufCallbacksBuilder::new();

                (*cfg).init_callback.map(|f| cb.raw_init_fn(CCallback(f)));

                (*cfg).destroy_callback.map(|f| cb.raw_destroy_fn(CCallback(f)));

                (*cfg).reset_callback.map(|f| cb.raw_reset_fn(CCallback(f)));

                Ok(PoolConfig {
                    obj_size: (*cfg).obj_size as usize,
                    nmax: (*cfg).nmax as usize,
                    callbacks: cb.build()?,
                })
            })

    }
}

// we can either have a VecDeque of Box<[u8]>, which is like an array
// of (bstring *), or we could contiguously allocate a Vec<u8> and carve
// off owned ranges of it. This implementation follows the existing one, using
// a queue that points to non-contiguous blocks of memory. It's left as an
// enhancement to do the contiguous block implementation.
#[allow(non_camel_case_types)]
pub struct pool_handle_rs {
    freeq: VecDeque<Box<[u8]>>,
    obj_size: usize,
    nused: usize,
    nmax: usize,
    callbacks: BufCallbacks,
}

type Pool = pool_handle_rs;

// |<----------- nmax ---------->|
// | nused | freeq     |  slack  |

impl Pool {
    pub fn new(cfg: &PoolConfig) -> Pool {
        Pool {
            freeq: VecDeque::with_capacity(cfg.nmax),
            nused: 0,
            obj_size: cfg.obj_size,
            nmax: cfg.nmax,
            callbacks: cfg.callbacks.clone(),
        }
    }

    /// The count of "used" objects, i.e. currently allocated and taken.
    pub fn nused(&self) -> usize {
        self.nused
    }

    /// The count of unused objects.
    pub fn nfree(&self) -> usize {
        self.freeq.len()
    }

    /// The maximum number of objects this pool will allocate.
    /// If 0 the pool is unlimited.
    pub fn nmax(&self) -> usize {
        self.nmax
    }

    pub fn prealloc(&mut self, size: usize) {
        // this doesn't check nmax?
        // this is the behavior of cc_pool.h, not sure if it's correct.
        while self.freeq.len() < size {
            let v = self.allocate_one();
            self.freeq.push_back(v);
        }
    }

    fn allocate_one(&mut self) -> Box<[u8]> {
        let mut bs = vec![0u8; self.obj_size].into_boxed_slice();
        self.callbacks.init(&mut bs[..]);
        bs
    }

    /// Get an object from the pool. If `self.nused < self.nmax` and
    /// `self.nfree == 0` we will allocate a new object, initialize and
    /// return it. If `self.nused == self.nmax` then None is returned because
    /// the pool is at capacity.
    #[inline]
    pub fn take(&mut self) -> Option<Box<[u8]>> {
        let item = self.freeq.pop_front().or_else(|| {
            if self.nmax == 0 || self.nused < self.nmax {
                Some(self.allocate_one())
            } else {
                None // we are over capacity
            }
        });

        if item.is_some() {
            self.nused += 1;
        }
        item
    }

    #[inline]
    pub fn put(&mut self, mut item: Box<[u8]>) {
        self.callbacks.reset(&mut item[..]);
        self.freeq.push_back(item);
        self.nused -= 1;
    }

    /// Drops unused buffers, calling the destructor on each before freeing them.
    pub fn shrink_to_fit(&mut self) {
        while !self.freeq.is_empty() {
            if let Some(mut buf) = self.freeq.pop_front() {
                self.callbacks.destroy(&mut buf[..]);
                drop(buf);
            }
        }
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        if self.nused > 0 {
            // not sure what to do here? I guess it leaks if hasn't been returned.
            eprintln!("WARNING: leaking {} pool items", self.nused)
        }

        self.shrink_to_fit();
    }
}

pub unsafe extern "C" fn pool_create_handle_rs(
    cfg: *const bind::pool_config_rs,
) -> *mut pool_handle_rs {
    ptrs::null_check(cfg)
        .map_err(|e| e.into())
        .and_then(|cfg| PoolConfig::from_raw(cfg))
        .map(|pc| Pool::new(&pc))
        .map(|pool| Box::into_raw(Box::new(pool)))
        .unwrap_or_else(|err| {
            eprintln!("ERROR: pool_create_handle_rs {:#?}", err);
            ptr::null_mut()
        })
}


#[cfg(test)]
mod test {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn test_prealloc_and_alloc_and_new() {
        let obj_size = 5;
        let nmax = 10;
        let counter = Rc::new(Cell::new(0));

        let c2 = counter.clone();

        let cfg = PoolConfig {
            obj_size,
            nmax,
            callbacks: BufCallbacksBuilder::new()
                .init_fn(|b| b[0] = 1u8)
                .destroy_fn(move |_| c2.set(c2.get() + 1))
                .build()
                .unwrap(),
        };
        let mut p = Pool::new(&cfg);

        assert_eq!(p.nused, 0);
        assert_eq!(p.nmax, 10);
        assert_eq!(p.freeq.len(), 0);
        assert!(p.freeq.capacity() >= 10);

        p.prealloc(3);
        assert_eq!(p.freeq.len(), 3);

        // make sure the callback was called
        for b in p.freeq.iter() {
            assert_eq!(b.len(), obj_size);
            assert_eq!(b[0], 1u8)
        }

        drop(p);
        assert_eq!(counter.get(), 3);
    }

    #[test]
    fn test_borrow_and_unborrow() {
        let obj_size = 5;
        let nmax = 2;
        let cfg = PoolConfig {
            obj_size,
            nmax,
            callbacks: BufCallbacksBuilder::new()
                .init_fn(|b| b[0] = 1u8)
                .destroy_fn(|b| b[0] = 2u8)
                .build()
                .unwrap(),
        };
        let mut p = Pool::new(&cfg);

        p.prealloc(1);

        let a = p.take().unwrap();
        let b = p.take().unwrap(); // this should allocate because we're still under nmax
        assert_eq!(p.nused, 2);
        assert!(p.take().is_none()); // sorry we're full

        p.put(a);
        assert_eq!(p.nused, 1);
        p.put(b);
        assert_eq!(p.freeq.len(), 2);
        assert_eq!(p.nused, 0);
    }
}
