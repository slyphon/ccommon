use std;
use std::collections::VecDeque;
use std::rc::Rc;
use std::result;

pub(crate) type ObjectInitFnPtr = unsafe extern "C" fn(*mut [u8]);
pub(crate) type ObjectDestroyFnPtr = unsafe extern "C" fn(*mut *mut [u8]);


struct BufMutator(Rc<Fn(&mut [u8])>);

impl BufMutator {
    pub fn from_raw_init_fn(f: ObjectInitFnPtr) -> BufMutator {
        let myf = Box::new(f);
        let wrapf = move |b: &mut [u8]| { unsafe { (*myf)(b) } };
        BufMutator(Rc::new(wrapf))
    }

    pub fn from_raw_destroy_fn(f: ObjectDestroyFnPtr) -> BufMutator {
        let myf = Box::new(f);
        let wrapf = move |b: &mut [u8]| {
            unsafe { (*myf)(&mut (b as *mut [u8])) }
        };
        BufMutator(Rc::new(wrapf))
    }

    pub fn new<F>(f: F) -> BufMutator
        where F: Fn(&mut [u8]) + 'static
    {
        BufMutator(Rc::new(f))
    }

    fn call(&self, buf: &mut [u8]) {
        (self.0)(buf)
    }
}

impl Clone for BufMutator {
    fn clone(&self) -> Self {
        BufMutator(self.0.clone())
    }
}

/// for testing using rust closures
#[doc(hidden)]
#[derive(Clone)]
struct BufCallbacks {
    init_cb: BufMutator,
    destroy_cb: BufMutator,
}

impl BufCallbacks {
    fn init(&self, buf: &mut [u8]) {
        self.init_cb.call(buf)
    }

    #[allow(dead_code)]
    fn destroy(&self, buf: &mut [u8]) {
        self.destroy_cb.call(buf)
    }
}


struct BufCallbacksBuilder {
    init_fn: Option<BufMutator>,
    destroy_fn: Option<BufMutator>,
}

impl Default for BufCallbacksBuilder {
    fn default() -> Self {
        BufCallbacksBuilder{init_fn: None, destroy_fn: None}
    }
}

#[allow(dead_code)]
impl BufCallbacksBuilder {
    #[cfg(test)]
    pub fn new() -> Self {
        BufCallbacksBuilder::default()
    }

    pub fn init_fn<F>(&mut self, f: F) -> &mut Self
        where F: Fn(&mut [u8]) + 'static
    {
        let new = self;
        new.init_fn = Some(BufMutator::new(f));
        new
    }

    pub fn raw_init_fn(&mut self, f: ObjectInitFnPtr) -> &mut Self {
        let new = self;
        new.init_fn = Some(BufMutator::from_raw_init_fn(f));
        new
    }

    pub fn destroy_fn<F>(&mut self, f: F) -> &mut Self
        where F: Fn(&mut [u8]) + 'static
    {
        let new = self;
        new.destroy_fn = Some(BufMutator::new(f));
        new
    }

    pub fn raw_destroy_fn(&mut self, f: ObjectDestroyFnPtr) -> &mut Self {
        let new = self;
        new.destroy_fn = Some(BufMutator::from_raw_destroy_fn(f));
        new
    }

    pub fn build(&self) -> result::Result<BufCallbacks, String> {
        Ok(BufCallbacks{
            init_cb:
                Clone::clone(
                    self.init_fn.as_ref()
                        .ok_or("init_fn must be initialized")?),
            destroy_cb:
                Clone::clone(
                    self.destroy_fn.as_ref()
                        .ok_or("destroy_fn must be initialized")?),
        })
    }
}


pub struct PoolConfig {
    obj_size: usize,
    nmax: usize,
    callbacks: BufCallbacks,
}


// we can either have a VecDeque of Box<[u8]>, which is like an array
// of (bstring *), or we could contiguously allocate a Vec<u8> and carve
// off owned ranges of it. This implementation follows the existing one, using
// a queue that points to non-contiguous blocks of memory. It's left as an
// enhancement to do the contiguous block implementation.
pub struct Pool {
    freeq: VecDeque<Box<[u8]>>,
    obj_size: usize,
    nused: usize,
    nmax: usize,
    callbacks: BufCallbacks,
}

// |<----------- nmax ---------->|
// | nused | freeq     |  slack  |

impl Pool {
    pub fn new(cfg: &PoolConfig) -> Pool {
        Pool{
            freeq: VecDeque::with_capacity(cfg.nmax),
            nused: 0,
            obj_size: cfg.obj_size,
            nmax:
                match cfg.nmax {
                    0 => std::usize::MAX,
                    _ => cfg.nmax,
                },
            callbacks: cfg.callbacks.clone(),
        }
    }

    pub fn prealloc(&mut self, size: usize) {
        // this doesn't check nmax?
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

    #[inline]
    pub fn take(&mut self) -> Option<Box<[u8]>> {
        let item =
            self.freeq
                .pop_front()
                .or_else(|| {
                    if self.nused < self.nmax {
                        Some(self.allocate_one())
                    } else {
                        None    // we are over capacity
                    }
                });

        if item.is_some() {
            self.nused += 1;
        }
        item
    }

    #[inline]
    pub fn put(&mut self, item: Box<[u8]>) {
        self.freeq.push_back(item);
        self.nused -= 1;
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_prealloc_and_alloc_and_new() {
        let obj_size = 5;
        let nmax = 10;
        let cfg = PoolConfig{
            obj_size,
            nmax,
            callbacks: BufCallbacksBuilder::new()
                .init_fn(|b| b[0] = 1u8)
                .destroy_fn(|b| b[0] = 2u8)
                .build()
                .unwrap()
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
    }

    #[test]
    fn test_borrow_and_unborrow() {
        let obj_size = 5;
        let nmax = 2;
        let cfg = PoolConfig{
            obj_size,
            nmax,
            callbacks: BufCallbacksBuilder::new()
                .init_fn(|b| b[0] = 1u8)
                .destroy_fn(|b| b[0] = 2u8)
                .build()
                .unwrap()
        };
        let mut p = Pool::new(&cfg);

        p.prealloc(1);

        let a = p.take().unwrap();
        let b = p.take().unwrap();    // this should allocate because we're still under nmax
        assert_eq!(p.nused, 2);
        assert!(p.take().is_none());  // sorry we're full

        p.put(a);
        assert_eq!(p.nused, 1);
        p.put(b);
        assert_eq!(p.freeq.len(), 2);
        assert_eq!(p.nused, 0);
    }
}
