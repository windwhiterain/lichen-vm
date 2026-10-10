//! Structural table values, sorted by a deep content hash. See
//! `docs/notes/lowlevel-vm.md` (tables).

use std::collections::HashMap;

use crate::{
    AnyFunctionId, AnyHandle, AnyNodeId, AnyNodeId::Dynamic as Dyn, BlockId, EvalError,
    LocalNodeId, LowValue, Module, Program, StaticNodeId, TableItem, ValueExt as _,
    ancestors::AncestorPairs,
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

/// Content tokens: marker values, an operation edge, an undecided slot, a
/// cut position, and the program's own variants.

const NONE_TOKEN: u64 = 0x6e6f_6e65_0000_0001; // "none"

const FRONTIER_TOKEN: u64 = 0x6672_6f6e_0000_0005; // "fron"

const FUNCTION_TOKEN: u64 = 0x6675_6e63_0000_0006; // "func"

const OPERATION_TOKEN: u64 = 0x6f70_6572_0000_0007; // "oper"

const UNDECIDED_TOKEN: u64 = 0x756e_6465_0000_0008; // "unde"

// Unchanged from the `Void` spelling: the hash is compared across a freeze, so the
// token cannot move.
const ERROR_TOKEN: u64 = 0x766f_6964_0000_0009; // "void"

const EXTENSION_TOKEN: u64 = 0x6578_7465_0000_000a; // "exte"

const NULL_TOKEN: u64 = 0x6e75_6c6c_0000_000b; // "null"
/// The fold seed for an array's positional item hashes.
const ARRAY_SEED: u64 = 0xa22e_b3e1_0000_0004;
/// The fold seed for a table's entry-key hashes.
const TABLE_SEED: u64 = 0x7461_626c_0000_000c; // "tabl"

/// How far a key's canonical unfolding descends before it is cut with
/// [`FRONTIER_TOKEN`].
///
/// # Invariant
///
/// A *performance* bound, not a correctness one: equal keys unfold equally at every
/// depth, so a truncated key can only collide with one it should not have matched, and
/// [`Module::key_eq`] resolves that. It is also what keeps a cyclic key finite.
const UNFOLD_DEPTH: usize = 8;

/// Which of a key's two unfoldings a node is being walked in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum UnfoldMode {
    /// The key's own value graph, the part [`Module::key_eq`] compares.
    ///
    /// # Invariant
    ///
    /// A node with no decided value — an undecided cell, a computation the deep pass
    /// could not resolve — leaves the *whole key* undecided, so a read stays lazy rather
    /// than missing a key that may still become concrete.
    Key,
    /// A function template's shape, which `key_eq` never compares.
    /// A function keys by identity.
    ///
    /// # Invariant
    ///
    /// The walk is total (a template is *expected* to hold undecided cells — its own
    /// parameter is one) and unfolds a node's operation edge in preference to its memoized
    /// value, so the answer does not depend on how far the definition pass has run.
    Template,
}

/// One key's unfolding, memoized by node, mode and remaining depth, so caching
/// cannot change what a key hashes to.
#[derive(Default)]
struct Unfolding {
    nodes: HashMap<(UnfoldMode, AnyNodeId, usize), KeyState>,
    /// A function template's entry points, `None` when the function is not
    /// resolvable — a fact about the graph, not the mode.
    functions: HashMap<(AnyFunctionId, usize), Option<u64>>,
}

/// Why a table key has (or has not) a content hash.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeyState {
    /// The key's content is decided and hashed.
    Hashed(u64),
    /// The key is not decided yet — it may still become a real key, so a read
    /// stays lazy rather than miss.
    Undecided,
    /// The key is decided and will never be key content: a read can miss.
    Unhashable,
}

impl<P: Program> Module<P> {
    /// Build a constant table value from raw `(key, value)` node pairs.
    ///
    /// # Invariant
    ///
    /// Every key is deep-evaluated first ([`Self::evaluate_node_deep`]; a static key
    /// reads its solved value). An undecided key records an [`EvalError::TableKeyUndecided`]
    /// and drops the entry; the survivors are hashed and stored sorted by hash for the
    /// binary-search lookup.
    pub fn build_table(
        &mut self,
        entries: &[(AnyNodeId, AnyNodeId)],
        block: BlockId,
    ) -> AnyHandle<[TableItem]> {
        let mut items = Vec::with_capacity(entries.len());
        for &(key, value) in entries {
            let Some(hash) = self.key_hash(key) else {
                self.eval_errors.push(EvalError::TableKeyUndecided { key });
                continue;
            };
            items.push(TableItem { key, value, hash });
        }
        // Stable: equal-hash entries keep their source order, so the same source builds
        // the same payload.
        items.sort_by_key(|item| item.hash);
        self.alloc_table(&items, block)
    }

    /// The deep content hash of `key`, or `None` when its content is undecided or
    /// holds a [`LowValue::Error`].
    ///
    /// # Invariant
    ///
    /// The hash is a *pre-filter*, never a decision: [`Self::key_eq`] is the authority
    /// and verifies every candidate, so the one direction the hash owes is *equal keys
    /// hash equal*. A collision is therefore always sound.
    pub(crate) fn key_hash(&mut self, key: AnyNodeId) -> Option<u64> {
        match self.key_state(key) {
            KeyState::Hashed(hash) => Some(hash),
            // Not decidable, or not key content at all: both mean "cannot match",
            // and the caller decides what that costs.
            KeyState::Undecided | KeyState::Unhashable => None,
        }
    }

    /// [`Self::key_hash`] with the two reasons for "no hash" kept apart.
    ///
    /// # Invariant
    ///
    /// The callers pay differently: a build drops the entry either way, but a **read**
    /// must only record a miss when the key is decided and simply absent — a key that
    /// is still undecided may become a real key later, so the read stays lazy instead
    /// of reporting a miss for a lookup that has not happened yet.
    pub(crate) fn key_state(&mut self, key: AnyNodeId) -> KeyState {
        match key {
            Dyn(node) => {
                // The **lazy** deep pass: nothing needs a walk past the shallow mask.
                self.evaluate_node_deep(node, None);
                // Only `Some(undecided)`: the unfolding is total, so a never-walked
                // key is still hashable.
                if self.nodes[node].evaluated_deep.is_some_and(|e| e.undecided) {
                    return KeyState::Undecided;
                }
            }
            AnyNodeId::Static(sref) => {
                if self.static_module(sref.module).nodes[sref.index.index].undecided() {
                    return KeyState::Undecided;
                }
            }
        }
        let mut unfolding = Unfolding::default();
        self.hash_node(key, &mut unfolding, UNFOLD_DEPTH, UnfoldMode::Key)
    }

    /// The canonical content unfolding's recursion, cut at the depth bound.
    ///
    /// # Invariant
    ///
    /// One of the three [`KeyState`] outcomes, because *where* in the key the undecided
    /// content sits must not change the verdict.
    fn hash_node(
        &self,
        id: AnyNodeId,
        unfolding: &mut Unfolding,
        depth: usize,
        mode: UnfoldMode,
    ) -> KeyState {
        if depth == 0 {
            return KeyState::Hashed(FRONTIER_TOKEN);
        }
        if let Some(&memo) = unfolding.nodes.get(&(mode, id, depth)) {
            return memo;
        }
        let verdict = self.hash_step(id, unfolding, depth, mode);
        unfolding.nodes.insert((mode, id, depth), verdict);
        verdict
    }

    /// One level of [`Self::hash_node`], after the depth cut and the memo.
    fn hash_step(
        &self,
        id: AnyNodeId,
        unfolding: &mut Unfolding,
        depth: usize,
        mode: UnfoldMode,
    ) -> KeyState {
        // A template is walked by its *shape*: an operation-bearing node unfolds
        // its edge, not the memoized result.
        if mode == UnfoldMode::Template
            && let Some(operand) = self.operation_operand(id)
        {
            let nested = match operand {
                Some(operand) => self.hash_template(operand, unfolding, depth - 1),
                None => NULL_TOKEN,
            };
            return KeyState::Hashed(mix(OPERATION_TOKEN ^ nested));
        }
        let Some(value) = self.node_value(id) else {
            // An empty slot is undecided wherever it sits; it must not collapse onto
            // one constant.
            return match mode {
                UnfoldMode::Key => KeyState::Undecided,
                UnfoldMode::Template => KeyState::Hashed(UNDECIDED_TOKEN),
            };
        };
        match value.as_enum() {
            Some(LowValue::USize(n)) => KeyState::Hashed(mix(n as u64)),
            // A float hashes by the bits `key_eq` compares, so equal keys hash equal.
            Some(LowValue::Float(n)) => KeyState::Hashed(mix(n.to_bits() as u64)),
            // A string key hashes by its byte content.
            Some(LowValue::Str(s)) => KeyState::Hashed(mix(s
                .as_bytes()
                .iter()
                .fold(0u64, |hash, &byte| mix(hash ^ byte as u64)))),
            Some(LowValue::None) => KeyState::Hashed(NONE_TOKEN),
            // An empty value is never key content. A template is not key content at
            // all — there it is only a shape.
            Some(LowValue::Error) => match mode {
                UnfoldMode::Key => KeyState::Unhashable,
                UnfoldMode::Template => KeyState::Hashed(ERROR_TOKEN),
            },
            Some(LowValue::Function(function)) => {
                match self.hash_function(function, unfolding, depth - 1) {
                    Some(hash) => KeyState::Hashed(hash),
                    None => match mode {
                        UnfoldMode::Key => KeyState::Undecided,
                        UnfoldMode::Template => KeyState::Hashed(UNDECIDED_TOKEN),
                    },
                }
            }
            Some(LowValue::Array(array)) => {
                let mut hash = ARRAY_SEED;
                // SAFETY: `array` is the payload of a live node of this module, and the
                // unfolding only reads.
                for item in unsafe { array.items() } {
                    match self.hash_node(item.node, unfolding, depth - 1, mode) {
                        KeyState::Hashed(item_hash) => hash = mix(hash ^ item_hash),
                        // An undecided position makes the whole key that.
                        verdict => return verdict,
                    }
                }
                KeyState::Hashed(hash)
            }
            // A table keys by identity, which cannot cross a freeze; the fold
            // depends on the key *set*.
            Some(LowValue::Table(table)) => {
                let mut hash = TABLE_SEED;
                // SAFETY: as the array arm — `table` is a live payload and the fold
                // only reads.
                for item in unsafe { table.items() } {
                    hash = mix(hash ^ item.hash);
                }
                KeyState::Hashed(hash)
            }
            // The program's value variants are opaque: one token each. `key_eq`
            // still tells two apart.
            None => KeyState::Hashed(EXTENSION_TOKEN),
        }
    }

    /// [`UnfoldMode::Template`]'s answer: that walk decides every node it reaches.
    fn hash_template(&self, id: AnyNodeId, unfolding: &mut Unfolding, depth: usize) -> u64 {
        match self.hash_node(id, unfolding, depth, UnfoldMode::Template) {
            KeyState::Hashed(hash) => hash,
            KeyState::Undecided | KeyState::Unhashable => {
                unreachable!("template mode decides every node it reaches")
            }
        }
    }

    /// The content hash of a function used as key content — `None` when its
    /// template is not resolvable.
    ///
    /// # Invariant
    ///
    /// The template's content is its *entry points*: what it returns and asserts. Its
    /// parameter is the bound variable, not content, so it is not folded in.
    fn hash_function(
        &self,
        function: AnyFunctionId,
        unfolding: &mut Unfolding,
        depth: usize,
    ) -> Option<u64> {
        if depth == 0 {
            return Some(FRONTIER_TOKEN);
        }
        if let Some(&memo) = unfolding.functions.get(&(function, depth)) {
            return memo;
        }
        let hashed = match function {
            AnyFunctionId::Dynamic(function) => {
                let template = self.functions.get(function)?;
                let r#return = Dyn(template.r#return);
                Some(self.hash_entry_points(
                    r#return,
                    template.asserts.iter().copied().map(Dyn),
                    unfolding,
                    depth,
                ))
            }
            AnyFunctionId::Static(function) => {
                let module = self.static_module(function.module);
                let template = module.functions.get(function.index.0)?;
                let local = |index: LocalNodeId| {
                    AnyNodeId::Static(StaticNodeId {
                        module: function.module,
                        index,
                    })
                };
                let r#return = local(template.r#return);
                Some(self.hash_entry_points(
                    r#return,
                    template.asserts.iter().copied().map(local),
                    unfolding,
                    depth,
                ))
            }
        };
        unfolding.functions.insert((function, depth), hashed);
        hashed
    }

    /// [`Self::hash_function`]'s fold over a template's entry points.
    fn hash_entry_points(
        &self,
        r#return: AnyNodeId,
        asserts: impl Iterator<Item = AnyNodeId>,
        unfolding: &mut Unfolding,
        depth: usize,
    ) -> u64 {
        let mut hash = FUNCTION_TOKEN;
        hash = mix(hash ^ self.hash_template(r#return, unfolding, depth));
        for assert in asserts {
            hash = mix(hash ^ self.hash_template(assert, unfolding, depth));
        }
        hash
    }

    /// A node's operation *edge*: `None` for none, `Some(None)` for nullary,
    /// `Some(Some(..))` for one with an operand.
    ///
    /// # Invariant
    ///
    /// This is the structural edge a template unfolds — fixed when the node is built, so
    /// it says the same thing however much of the definition pass has run since.
    fn operation_operand(&self, id: AnyNodeId) -> Option<Option<AnyNodeId>> {
        match id {
            Dyn(node) => {
                let operation = self.nodes.get(node)?.operation?;
                Some(operation.operand.map(Dyn))
            }
            AnyNodeId::Static(sref) => {
                let module = self.static_module(sref.module);
                let operation = module.nodes.get(sref.index.index)?.operation?;
                Some(operation.operand.map(|index| {
                    AnyNodeId::Static(StaticNodeId {
                        module: sref.module,
                        index,
                    })
                }))
            }
        }
    }

    /// Pure, coinductive equality of two key nodes — the read-only counterpart
    /// of the unification comparison.
    ///
    /// # Invariant
    ///
    /// Same elementwise descent and path guard, with no binding and no error recording: a
    /// hash table's equality must be pure. Stored keys are concrete by construction, so
    /// the comparison never meets an undecided cell. A table keys by identity, a
    /// function by its id.
    pub(crate) fn key_eq(
        &self,
        a: AnyNodeId,
        b: AnyNodeId,
        path: &mut AncestorPairs<AnyNodeId>,
    ) -> bool {
        if a == b {
            return true;
        }
        if path.contains(a, b) {
            return true;
        }
        path.insert(a, b);
        let ok = match (self.node_value(a), self.node_value(b)) {
            (Some(va), Some(vb)) => match (va.as_enum(), vb.as_enum()) {
                (Some(LowValue::Array(pa)), Some(LowValue::Array(pb))) => {
                    // SAFETY: `pa`/`pb` are payloads of live nodes of this module, and
                    // `key_eq` only reads.
                    let (left, right) = (unsafe { pa.items() }, unsafe { pb.items() });
                    left.len() == right.len()
                        && left
                            .iter()
                            .zip(right.iter())
                            .all(|(ia, ib)| self.key_eq(ia.node, ib.node, path))
                }
                // A table value keys by identity — its payload's `PartialEq`; see
                // [`Self::hash_step`]'s table arm for the matching hash.
                (Some(LowValue::Table(ta)), Some(LowValue::Table(tb))) => ta == tb,
                _ => va.value_eq(&vb),
            },
            _ => false,
        };
        path.remove(a, b);
        ok
    }
}
