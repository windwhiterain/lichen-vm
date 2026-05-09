use arrayvec::ArrayVec;
use std::{
    fmt::Debug,
    mem::{self, MaybeUninit},
};

use crate::{exp2_int, log2_int};

const BIT_WIDTH: usize = mem::size_of::<usize>();
const INIT_BIT_WIDTH: u32 = 3;

pub struct StableVec<V> {
    chunks: ArrayVec<Box<[MaybeUninit<V>]>, { BIT_WIDTH }>,
    next_idx: usize,
}

impl<V> Default for StableVec<V> {
    fn default() -> Self {
        Self {
            chunks: Default::default(),
            next_idx: 0,
        }
    }
}

impl<V: Debug> Debug for StableVec<V> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.iter()).finish()
    }
}

// idx = 2^n - z + x, 0 <= x < 2^n
// => 0 <= idx + 1 - 2^n < 2^n
// let k = idx + z
// => k/2 < 2^n <= k
// => log k - 1 < n <= log k
// => n = log k, x = k - 2^n
impl<V> StableVec<V> {
    const INIT_SIZE: usize = exp2_int(INIT_BIT_WIDTH);
    const fn compute_idx(idx: usize) -> (usize, usize) {
        let k = idx + Self::INIT_SIZE;
        let n = log2_int(k);
        let x = k - exp2_int(n);
        ((n - INIT_BIT_WIDTH) as usize, x)
    }
    pub fn insert(&mut self, value: V) -> (usize, &mut V) {
        let (y, x) = Self::compute_idx(self.next_idx);
        if y >= self.chunks.len() {
            let size = exp2_int(y as u32 + INIT_BIT_WIDTH);
            self.chunks.push(Box::new_uninit_slice(size));
        };
        let chunk = &mut self.chunks[y][x];
        chunk.write(value);
        let ret = self.next_idx;
        self.next_idx += 1;
        (ret, unsafe { chunk.assume_init_mut() })
    }
    pub fn get(&self, idx: usize) -> &V {
        let (y, x) = Self::compute_idx(idx);
        let chunck = &self.chunks[y];
        unsafe { chunck[x].assume_init_ref() }
    }
    pub fn get_maybe_uinit_mut(&mut self, idx: usize) -> &mut MaybeUninit<V> {
        let (y, x) = Self::compute_idx(idx);
        let chunck = &mut self.chunks[y];
        &mut chunck[x]
    }
    pub fn get_mut(&mut self, idx: usize) -> &mut V {
        unsafe { self.get_maybe_uinit_mut(idx).assume_init_mut() }
    }
    pub fn clear(&mut self) {
        for idx in 0..self.next_idx {
            unsafe { self.get_maybe_uinit_mut(idx).assume_init_drop() };
        }
        self.next_idx = 0;
    }
    pub fn len(&self) -> usize {
        self.next_idx
    }
    pub fn iter(&self) -> impl Iterator<Item = &V> {
        (0..self.next_idx).map(|idx| self.get(idx))
    }
    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut V> {
        let ptr = self as *mut Self;
        (0..self.next_idx).map(move |idx| unsafe { &mut *ptr }.get_mut(idx))
    }
}

impl<V> Drop for StableVec<V> {
    fn drop(&mut self) {
        self.clear();
    }
}

#[test]
fn test() {
    use core::cell::Cell;
    thread_local! {
        static DROP_COUNT: Cell<usize> = Cell::new(0);
    }
    struct DropCounter(usize);
    impl Debug for DropCounter {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            self.0.fmt(f)
        }
    }
    impl DropCounter {
        fn new(value: usize) -> Self {
            DROP_COUNT.with(|x| x.set(x.get() + 1));
            Self(value)
        }
    }
    impl Drop for DropCounter {
        fn drop(&mut self) {
            DROP_COUNT.with(|x| x.set(x.get() - 1));
        }
    }
    {
        let mut stable_vec = StableVec::<DropCounter>::default();
        for i in 0..256 {
            stable_vec.insert(DropCounter::new(i));
        }
        stable_vec.clear();
        assert!(DROP_COUNT.get() == 0);
        for i in 0..256 {
            stable_vec.insert(DropCounter::new(i));
        }
    }
    assert!(DROP_COUNT.get() == 0);
}
