use core::slice;
use std::mem::{self, MaybeUninit};

pub mod array;
pub mod hashmap;

#[derive(Debug)]
pub struct Arena {
    chunks: Vec<Box<[MaybeUninit<u8>]>>,
    next: usize,
    avaliable_size: usize,
    next_chunk_size: usize,
}

impl Arena {
    pub fn new() -> Self {
        Self {
            chunks: Default::default(),
            next: 0,
            avaliable_size: 0,
            next_chunk_size: 1024,
        }
    }
    fn add_raw<T>(&mut self, len: usize) -> *mut MaybeUninit<T> {
        let size = mem::size_of::<T>() * len;
        if size == 0 || size > self.avaliable_size {
            let new_chunk_size = (self.avaliable_size * 2).max(size);
            self.chunks.push(Box::new_uninit_slice(new_chunk_size));
            self.avaliable_size = new_chunk_size;
            self.next_chunk_size = new_chunk_size * 2;
            self.next = 0;
        }
        let ptr = unsafe { self.chunks.last_mut().unwrap().as_mut_ptr().add(self.next) };
        let ptr = unsafe { ptr.add(ptr.align_offset(mem::align_of::<T>())) } as *mut MaybeUninit<T>;
        self.avaliable_size -= size;
        self.next += size;
        ptr
    }
    pub fn add_slice_uninit<T>(&mut self, len: usize) -> &mut [MaybeUninit<T>] {
        let ptr = self.add_raw::<T>(len);
        unsafe { slice::from_raw_parts_mut(ptr, len) }
    }
    pub fn add_slice_default<T: Default>(&mut self, len: usize) -> &mut [T] {
        let slice = self.add_slice_uninit::<T>(len);
        for i in slice.iter_mut() {
            i.write(T::default());
        }
        unsafe { slice::from_raw_parts_mut(slice.as_mut_ptr() as *mut T, len) }
    }
    pub fn add_uninit<T>(&mut self) -> &mut MaybeUninit<T> {
        let ptr = self.add_raw::<T>(1);
        unsafe { &mut *ptr }
    }
    pub fn add<T>(&mut self, value: T) -> &mut T {
        let ptr = self.add_raw::<T>(1);
        let r#ref = unsafe { &mut *ptr };
        r#ref.write(value)
    }
}
