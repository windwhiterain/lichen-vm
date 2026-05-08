use std::{fmt::Debug, mem::MaybeUninit, ptr::NonNull};

use crate::arena::Arena;

#[derive(Clone)]
pub struct ArenaArray<T>(NonNull<[MaybeUninit<T>]>);

impl<T> ArenaArray<T> {
    pub fn new(arena: &mut Arena, len: usize) -> Self {
        Self(NonNull::from_mut(arena.add_slice_uninit(len)))
    }
    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn inner(&self) -> &[MaybeUninit<T>] {
        unsafe { self.0.as_ref() }
    }
    pub fn inner_mut(&mut self) -> &mut [MaybeUninit<T>] {
        unsafe { self.0.as_mut() }
    }
    pub fn get_uninit(&mut self, index: usize) -> &mut MaybeUninit<T> {
        self.inner_mut().get_mut(index).unwrap()
    }
    pub fn get_mut(&mut self, index: usize) -> &mut T {
        unsafe { self.get_uninit(index).assume_init_mut() }
    }
    pub fn get(&self, index: usize) -> &T {
        unsafe { self.inner().get(index).unwrap().assume_init_ref() }
    }
    pub fn iter(&self) -> impl Iterator<Item = &T> {
        unsafe { self.inner().iter().map(|x| x.assume_init_ref()) }
    }
    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut T> {
        unsafe { self.inner_mut().iter_mut().map(|x| x.assume_init_mut()) }
    }
}

impl<T: Clone> ArenaArray<T> {
    pub fn new_uniform(arena: &mut Arena, len: usize, val: T) -> Self {
        let mut ret = Self::new(arena, len);
        for i in ret.inner_mut() {
            i.write(val.clone());
        }
        ret
    }
}

impl<T: Default> ArenaArray<T> {
    pub fn new_default(arena: &mut Arena, len: usize) -> Self {
        let mut ret = Self::new(arena, len);
        for i in ret.inner_mut() {
            i.write(Default::default());
        }
        ret
    }
}

impl<T: Debug> Debug for ArenaArray<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.iter()).finish()
    }
}
