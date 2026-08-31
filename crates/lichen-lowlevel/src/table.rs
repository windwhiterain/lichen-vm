//! Structural table values: a constant [`LowValue::Table`] — a payload of
//! key/value entries sorted by a deep content hash — and the read lookup's
//! key machinery.
//!
//! A table's keys are **deep content**: two keys compare equal when their
//! evaluated structures are coinductively equal (arrays descend elementwise
//! — the comparison unification makes through arrays — but read-only: a
//! hash table's equality must be pure, so this is the comparison core of
//! `unify` with the mutation stripped out).  A table value keys by
//! identity, a function by its id.  The hash walks the same discipline, so
//! equal keys always hash equal (the `HashMap` contract's one direction;
//! collisions are fine — a read verifies the equal-hash run with
//! [`Module::key_eq`]).  Cycles (the `[Type, ↺]` universe) are cut by a
//! path check on both sides: a revisited node mixes in a cycle token at its
//! revisit depth, so two equal cyclic structures hash equal and compare
//! equal.
//!
//! Keys are force-evaluated when the table is built — hashing needs the
//! decided content — so a stored key is fully concrete and its hash is
//! stable for the table's whole life.  A key that cannot be forced concrete
//! (its subtree holds an unbound cell or a parameterized computation)
//! records a [`EvalError::TableKeyUnbound`] and drops the entry.  Values
//! are stored as lazy refs and read on demand, like array items.

use crate::{
    AnyFunctionId, AnyHandle, AnyNodeId, AnyNodeId::Dynamic as Dyn, BlockId, EvalError, LowValue,
    Module, Program, TableItem, ValueExt as _,
};
use lichen_utils::extend::AsEnum;

/// The deterministic mixer for the deep content hash (a splitmix64
/// finalizer — one input, one output, no state).
fn mix(h: u64) -> u64 {
    let mut z = h.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// The identity hash of an id — the id's own [`Hash`] implementation
/// (slotmap keys hash their raw index; a static ref its key and index),
/// deterministically (a fixed-seed `DefaultHasher`).
fn id_hash(id: impl std::hash::Hash) -> u64 {
    use std::hash::Hasher as _;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    id.hash(&mut hasher);
    hasher.finish()
}

/// Distinct content tokens for the marker values and a cycle revisit.
const NONE_TOKEN: u64 = 0x6e6f_6e65_0000_0001; // "none"
const PARAM_TOKEN: u64 = 0x7061_7261_0000_0002; // "para"
const CYCLE_TOKEN: u64 = 0x6379_636c_0000_0003; // "cycl"
/// The fold seed for an array's positional item hashes.
const ARRAY_SEED: u64 = 0xa22e_b3e1_0000_0004;

impl<P: Program> Module<P> {
    /// Build a constant table value from raw `(key, value)` node pairs (see
    /// the module docs).  Every key is force-evaluated first
    /// ([`Self::evaluate_node_forced`]; a static key reads its solved
    /// value), an unforceable key records a
    /// [`EvalError::TableKeyUnbound`] and drops the entry, and the
    /// survivors are deep-content-hashed and stored sorted by hash for the
    /// binary-search lookup.  The payload is a plain arena slice like an
    /// array's.
    pub fn build_table(
        &mut self,
        entries: &[(AnyNodeId, AnyNodeId)],
        block: BlockId,
    ) -> AnyHandle<[TableItem]> {
        let mut items = Vec::with_capacity(entries.len());
        for &(key, value) in entries {
            let Some(hash) = self.key_hash(key) else {
                self.eval_errors.push(EvalError::TableKeyUnbound { key });
                continue;
            };
            items.push(TableItem { key, value, hash });
        }
        // Stable: equal-hash entries keep their source order, so the same
        // source builds the same payload (content-addressed artifacts stay
        // deterministic).
        items.sort_by_key(|item| item.hash);
        self.alloc_table(&items, block)
    }

    /// The deep content hash of `key`, or `None` when the key's subtree is
    /// not fully concrete — its content is not yet decided, so nothing can
    /// be hashed or matched (a build drops the entry, a read misses).
    pub(crate) fn key_hash(&mut self, key: AnyNodeId) -> Option<u64> {
        match key {
            Dyn(node) => {
                self.evaluate_node_forced(node, None);
                if self.nodes[node].evaluated_deep.is_some_and(|e| e.parameterized) {
                    return None;
                }
            }
            AnyNodeId::Static(sref) => {
                if self.static_module(sref.module).nodes[sref.index.index].parameterized {
                    return None;
                }
            }
        }
        let mut path = Vec::new();
        Some(self.hash_inner(key, &mut path, 0))
    }

    /// The recursive content hash — [`ValueExt::value_eq`]'s comparison
    /// discipline run through a mixer: scalar variants hash their payload,
    /// a function hashes its identity, an array descends its item nodes
    /// positionally, and a table value (as key content) hashes by identity.
    /// A cycle is cut by the path check: a revisited node mixes in the
    /// cycle token at its revisit depth, so two equal cyclic structures
    /// hash equal.
    fn hash_inner(&self, id: AnyNodeId, path: &mut Vec<AnyNodeId>, depth: usize) -> u64 {
        if path.contains(&id) {
            return mix(CYCLE_TOKEN ^ depth as u64);
        }
        path.push(id);
        let value = self.node_value(id).unwrap_or_else(|| {
            panic!("hashing a key whose subtree holds a node without a value — the deep pass must have resolved it")
        });
        let h = match value.as_enum() {
            Some(LowValue::USize(n)) => mix(n as u64),
            Some(LowValue::None) => NONE_TOKEN,
            Some(LowValue::Parameterized) => PARAM_TOKEN,
            Some(LowValue::Function(AnyFunctionId::Dynamic(function))) => mix(id_hash(function)),
            Some(LowValue::Function(AnyFunctionId::Static(sref))) => mix(id_hash(sref)),
            Some(LowValue::Array(array)) => array
                .items()
                .iter()
                .fold(ARRAY_SEED, |h, item| mix(h ^ self.hash_inner(item.node, path, depth + 1))),
            // A table keyed by identity (user directive) — the payload's
            // identity, matching [`AnyHandle`]'s `PartialEq`.
            Some(LowValue::Table(table)) => mix(match table {
                AnyHandle::Dynamic(handle) => handle.0 as *const TableItem as usize as u64,
                AnyHandle::Static(handle) => {
                    handle.module.as_raw() ^ (handle.offset as *const TableItem as usize as u64)
                }
            }),
            None => unreachable!("a structural value is one of the variants above"),
        };
        path.pop();
        h
    }

    /// Pure, coinductive structural equality of two key nodes — the
    /// read-only counterpart of the unification comparison (same elementwise
    /// descent, same path guard; no binding, no error recording — a hash
    /// table's equality must be pure).  Stored keys are concrete by
    /// construction, so the comparison never meets an unbound cell.  A
    /// table value keys by identity, a function by its id.
    pub(crate) fn key_eq(
        &self,
        a: AnyNodeId,
        b: AnyNodeId,
        path: &mut Vec<(AnyNodeId, AnyNodeId)>,
    ) -> bool {
        if a == b {
            return true;
        }
        if path.contains(&(a, b)) || path.contains(&(b, a)) {
            return true;
        }
        path.push((a, b));
        let ok = match (self.node_value(a), self.node_value(b)) {
            (Some(va), Some(vb)) => match (va.as_enum(), vb.as_enum()) {
                (Some(LowValue::Array(pa)), Some(LowValue::Array(pb))) => {
                    let (left, right) = (pa.items(), pb.items());
                    left.len() == right.len()
                        && left
                            .iter()
                            .zip(right.iter())
                            .all(|(ia, ib)| self.key_eq(ia.node, ib.node, path))
                }
                // A table value keys by identity — its payload's `PartialEq`
                // (see [`hash_inner`]'s table arm for the matching hash).
                (Some(LowValue::Table(ta)), Some(LowValue::Table(tb))) => ta == tb,
                _ => va.value_eq(&vb),
            },
            _ => false,
        };
        path.pop();
        ok
    }
}
