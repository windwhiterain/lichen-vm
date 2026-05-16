use std::{
    fmt::Debug,
    hash::{DefaultHasher, Hash, Hasher as _},
};

use crate::{
    arena::{Arena, array::ArenaArray},
    mod_exp2,
};

type Hasher = DefaultHasher;

pub struct ArenaHashMap<K, V> {
    table: ArenaArray<Option<usize>>,
    entries: ArenaArray<Entry<K, V>>,
}

impl<K, V> ArenaHashMap<K, V>
where
    K: Hash + Eq,
{
    const TABLE_SCALE: f32 = 2.0;
    pub fn new(arena: &mut Arena, len: usize) -> Self {
        let table_len = ((len as f32 * Self::TABLE_SCALE).ceil() as usize).next_power_of_two();
        Self {
            table: ArenaArray::new_default(arena, table_len),
            entries: ArenaArray::new(arena, len),
        }
    }
    pub fn from_iter(arena: &mut Arena, iter: impl Iterator<Item = (K, V)>) -> Self {
        let len = iter.size_hint().0;
        debug_assert!(iter.size_hint().1 == Some(len));
        let mut ret = Self::new(arena, len);
        for (index, (key, value)) in iter.enumerate() {
            ret.insert(index, key, value);
        }
        ret
    }
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    fn table_len(&self) -> usize {
        self.table.len()
    }
    fn table_location(&self, state: &mut ProbingState) -> usize {
        let ret = mod_exp2(
            state.hash + state.iteration * state.iteration,
            self.table_len(),
        );
        state.iteration += 1;
        ret
    }
    fn probing_state(&self, key: &K) -> ProbingState {
        let mut hasher = Hasher::new();
        key.hash(&mut hasher);
        ProbingState {
            hash: hasher.finish() as usize,
            iteration: 0,
        }
    }
    pub fn insert(&mut self, index: usize, key: K, value: V) -> Option<usize> {
        let mut state = self.probing_state(&key);
        let mut table_location = self.table_location(&mut state);
        let mut existing_entry_index = None;
        loop {
            let entry_index = *self.table.get(table_location);
            if let Some(entry_index) = entry_index {
                let entry = self.entries.get_mut(entry_index);
                if entry.key == key {
                    existing_entry_index = Some(entry_index);
                }
                table_location = self.table_location(&mut state);
                continue;
            }
            self.entries.get_uninit(index).write(Entry { key, value });
            *self.table.get_mut(table_location) = Some(index);
            break;
        }
        existing_entry_index
    }
    pub fn get(&self, key: K) -> Option<&V> {
        let mut state = self.probing_state(&key);
        let mut table_location = self.table_location(&mut state);
        loop {
            let entry_index = *self.table.get(table_location);
            let Some(entry_index) = entry_index else {
                return None;
            };
            let entry = self.entries.get(entry_index);
            if entry.key == key {
                return Some(&entry.value);
            } else {
                table_location = self.table_location(&mut state);
                continue;
            }
        }
    }
    pub fn get_mut(&mut self, key: K) -> Option<&V> {
        let mut state = self.probing_state(&key);
        let mut table_location = self.table_location(&mut state);
        loop {
            let entry_index = *self.table.get(table_location);
            let Some(entry_index) = entry_index else {
                return None;
            };
            let entry = self.entries.get(entry_index);
            if entry.key == key {
                return Some(&mut self.entries.get_mut(entry_index).value);
            } else {
                table_location = self.table_location(&mut state);
                continue;
            }
        }
    }
    pub fn values(&self) -> impl Iterator<Item = &V> {
        self.entries.iter().map(|x| &x.value)
    }
    pub fn values_mut(&mut self) -> impl Iterator<Item = &mut V> {
        self.entries.iter_mut().map(|x| &mut x.value)
    }
    pub fn keys(&mut self) -> impl Iterator<Item = &K> {
        self.entries.iter().map(|x| &x.key)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Entry<K, V> {
    key: K,
    value: V,
}

struct ProbingState {
    pub hash: usize,
    pub iteration: usize,
}

impl<K, V> Clone for ArenaHashMap<K, V> {
    fn clone(&self) -> Self {
        Self {
            table: self.table.clone(),
            entries: self.entries.clone(),
        }
    }
}

impl<K, V> Copy for ArenaHashMap<K, V> {}

impl<K: Debug, V: Debug> Debug for ArenaHashMap<K, V> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_map()
            .entries(self.entries.iter().map(|x| (&x.key, &x.value)))
            .finish()
    }
}

impl<K, V> PartialEq for ArenaHashMap<K, V> {
    fn eq(&self, other: &Self) -> bool {
        self.table == other.table && self.entries == other.entries
    }
}

impl<K, V> Eq for ArenaHashMap<K, V> {}

#[test]
fn test() {
    let mut arena = Arena::new();
    let mut hashmap = ArenaHashMap::<usize, usize>::new(&mut arena, 8);
    for i in 0..8 {
        assert!(hashmap.insert(i, i, i).is_none());
    }
    for i in 0..8 {
        assert_eq!(*hashmap.get(i).unwrap(), i);
    }
}
