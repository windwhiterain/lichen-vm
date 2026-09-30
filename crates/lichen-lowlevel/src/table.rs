//! Structural table values: a constant [`LowValue::Table`] — a payload of
//! key/value entries sorted by a deep content hash — and the read lookup's
//! key machinery.
//!
//! A table's keys are **deep content**: two keys compare equal when their
//! evaluated structures are coinductively equal (arrays descend elementwise
//! — the comparison unification makes through arrays — but read-only: a
//! hash table's equality must be pure, so this is the comparison core of
//! `unify` with the mutation stripped out).  [`Module::key_eq`] is that
//! comparison and it is the **authority**: the stored hash only *finds
//! candidates* — a read binary-searches the payload for the equal-hash run
//! and verifies every candidate with `key_eq` — so the one direction the
//! hash owes is *equal keys hash equal*.  A collision is always sound, and
//! every number in a payload is a pre-filter, never a decision.
//!
//! The hash is the key's **canonical content unfolding**: it walks the same
//! structure the comparison walks, mixing in what each position *is* and
//! never an address, a node identity, or a freeze-assigned index.  That is
//! what lets it survive a freeze — a dynamic key and the static key the
//! freeze files it as are the same content, so they unfold to the same
//! number — and it is why both of `key_eq`'s notions of "the same content"
//! have to be honoured here:
//!
//! - **A cycle is cut by the depth bound, not by a path check.**  The walk
//!   unfolds the key to a fixed depth and every position at that frontier
//!   mixes in the same [`FRONTIER_TOKEN`].  A revisited-node token carrying
//!   *its own revisit depth* does not agree with `key_eq`: the comparison
//!   cuts on the *pair* of nodes on its path, which is equality of the
//!   infinite unfolding, and `[1, ↺]` and `[1, [1, ↺]]` are equal under it
//!   while their revisits sit at different depths.  Truncating both at one
//!   depth is invariant under that equality — equal keys unfold equally at
//!   *every* depth — so the bound costs candidate quality and can never hide
//!   a key that is there.
//! - **A table and a function unfold to their content, not their
//!   identity.**  `key_eq` keys a table by identity and a function by its
//!   id, and neither identity crosses a freeze: a dynamic handle's address
//!   and a [`FunctionId`](crate::FunctionId) are process-local.  A table
//!   unfolds to the fold of its entry keys' content hashes — the numbers the
//!   payload is already sorted by, so the fold is the key *set*'s, and the
//!   freeze copies the payload verbatim, so it is the same fold afterwards —
//!   and a function to the shape of its template, whose content is what it
//!   returns and asserts (its parameter is the function's bound variable, so
//!   the body reads it as the unbound cell it is).
//!
//! A function's template is unfolded in a second mode ([`UnfoldMode`]),
//! because a template is *expected* to hold unbound cells and is never
//! compared by `key_eq`: that walk is total, and it reads a node's operation
//! edge in preference to its memoized value, so its answer does not depend on
//! how far the definition pass has run.
//!
//! **What the unfolding does not distinguish — sound, and deliberately
//! stated.**  It is a pre-filter, so a coarser answer costs candidate quality
//! and nothing else; `key_eq` decides every candidate it offers.  What it
//! cannot tell apart: two keys that differ only below [`UNFOLD_DEPTH`]; two
//! function templates that differ only in *which operator* sits at a
//! position, because a program's operator vocabulary is not something the
//! lowlevel can name (they are still two distinct functions, and `key_eq`
//! keys a function by its id, so it never calls them the same key); two
//! tables with the same key set and different values (a table keys by
//! identity, so a comparison would not call those the same key either); and
//! two of the program's own value variants, which are one opaque token each.
//!
//! Keys are force-evaluated when the table is built — hashing needs the
//! decided content — so a stored key is fully concrete and its hash is
//! stable for the table's whole life.  A key that cannot be forced concrete
//! (its subtree holds an unbound cell or a parameterized computation) or
//! whose content is a computed nothing ([`LowValue::Void`], the residue of
//! a failed read) records a [`EvalError::TableKeyUnbound`] and drops the
//! entry.  Values are stored as lazy refs and read on demand, like array
//! items.

use std::collections::HashMap;

use crate::{
    AnyFunctionId, AnyHandle, AnyNodeId, AnyNodeId::Dynamic as Dyn, BlockId, EvalError,
    LocalNodeId, LowValue, Module, Program, StaticNodeId, TableItem, ValueExt as _,
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

/// Distinct content tokens for the marker values, an operation edge, a
/// template's undecided position, a position the unfolding cut, and the
/// program's own value variants.
const NONE_TOKEN: u64 = 0x6e6f_6e65_0000_0001; // "none"
const FRONTIER_TOKEN: u64 = 0x6672_6f6e_0000_0005; // "fron"
const FUNCTION_TOKEN: u64 = 0x6675_6e63_0000_0006; // "func"
const OPERATION_TOKEN: u64 = 0x6f70_6572_0000_0007; // "oper"
const UNDECIDED_TOKEN: u64 = 0x756e_6465_0000_0008; // "unde"
const VOID_TOKEN: u64 = 0x766f_6964_0000_0009; // "void"
const EXTENSION_TOKEN: u64 = 0x6578_7465_0000_000a; // "exte"
const NULL_TOKEN: u64 = 0x6e75_6c6c_0000_000b; // "null"
/// The fold seed for an array's positional item hashes.
const ARRAY_SEED: u64 = 0xa22e_b3e1_0000_0004;
/// The fold seed for a table's entry-key hashes.
const TABLE_SEED: u64 = 0x7461_626c_0000_000c; // "tabl"

/// How far a key's canonical unfolding descends before it is cut with
/// [`FRONTIER_TOKEN`].  The bound is a *performance* bound, not a correctness
/// one: equal keys unfold equally at every depth, so a key the bound
/// truncates can only collide with a key it should not have matched, and
/// [`Module::key_eq`] resolves that.  It is also what keeps a cyclic key —
/// the `[Type, ↺]` universe — and a self-referential function template
/// finite.
const UNFOLD_DEPTH: usize = 8;

/// Which of a key's two unfoldings a node is being walked in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum UnfoldMode {
    /// The key's own value graph — the part [`Module::key_eq`] compares, so
    /// the walk reads the fact the comparison reads: the node's decided
    /// value.  A node that has none (a cell that is still unbound, a
    /// computation the deep pass could not resolve) leaves the *whole key*
    /// undecided, which a read answers by staying lazy rather than by missing
    /// a key that may still become concrete.
    Key,
    /// A function template's shape — the part `key_eq` never compares, because
    /// a function keys by identity.  This walk is total (a template is
    /// *expected* to hold unbound cells — its own parameter is one) and it
    /// unfolds a node's operation edge in preference to its memoized value,
    /// so its answer does not depend on how far the definition pass has run.
    Template,
}

/// One key's unfolding, memoized.  The memo is keyed by the node, the mode and
/// the remaining depth — everything the answer depends on and nothing else —
/// so caching it can never change what a key hashes to; it only keeps a
/// shared subgraph from being walked once per path to it.
#[derive(Default)]
struct Unfolding {
    nodes: HashMap<(UnfoldMode, AnyNodeId, usize), KeyState>,
    /// A function template's entry points, or `None` when the function is not
    /// resolvable — a fact about the module graph, not about the mode.
    functions: HashMap<(AnyFunctionId, usize), Option<u64>>,
}

/// Why a table key has (or has not) a content hash.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeyState {
    /// The key's content is decided and hashed.
    Hashed(u64),
    /// The key is not decided yet — an unbound cell, a lazy computation whose
    /// operands are not bound.  It may still become a real key, so a read
    /// must stay lazy rather than miss.
    Undecided,
    /// The key is decided and will never be key content (it holds a
    /// [`LowValue::Void`](crate::LowValue::Void), the residue of a failed
    /// read).  A read can miss.
    Unhashable,
}

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
    /// be hashed or matched (a build drops the entry, a read misses) — or
    /// when the content holds a [`LowValue::Void`], the residue of an
    /// already-recorded failed read: not hashable, same class as an unbound
    /// subtree.
    pub(crate) fn key_hash(&mut self, key: AnyNodeId) -> Option<u64> {
        match self.key_state(key) {
            KeyState::Hashed(hash) => Some(hash),
            // Not decidable yet, or not key content at all — both mean "this
            // key cannot match", and the caller decides what that costs.
            KeyState::Undecided | KeyState::Unhashable => None,
        }
    }

    /// [`Self::key_hash`] with the two reasons for "no hash" kept apart.
    ///
    /// They are different facts and the callers pay differently for them: a
    /// build drops the entry either way, but a **read** must only record a
    /// miss when the key is decided and simply absent.  A key that is still
    /// undecided (a lambda parameter mid-apply, a lazy computation whose
    /// operands are not bound yet) may become a real key later, so the read
    /// stays lazy instead of reporting a miss for a lookup that has not
    /// happened yet.
    pub(crate) fn key_state(&mut self, key: AnyNodeId) -> KeyState {
        match key {
            Dyn(node) => {
                self.evaluate_node_forced(node, None);
                if self.nodes[node]
                    .evaluated_deep
                    .is_some_and(|e| e.parameterized)
                {
                    return KeyState::Undecided;
                }
            }
            AnyNodeId::Static(sref) => {
                if self.static_module(sref.module).nodes[sref.index.index].parameterized {
                    return KeyState::Undecided;
                }
            }
        }
        let mut unfolding = Unfolding::default();
        self.hash_node(key, &mut unfolding, UNFOLD_DEPTH, UnfoldMode::Key)
    }

    /// The canonical content unfolding's recursion: cut at the depth bound,
    /// then read the node's content as `mode` decides it.  The answer is one
    /// of the three [`KeyState`] outcomes, because *where* in the key the
    /// undecided content sits must not change the verdict.
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

    /// One level of [`Self::hash_node`], after the depth cut and the memo: the
    /// node's content, hashed as `mode` reads it.
    fn hash_step(
        &self,
        id: AnyNodeId,
        unfolding: &mut Unfolding,
        depth: usize,
        mode: UnfoldMode,
    ) -> KeyState {
        // A template is walked by its *shape*: a node carrying an operation
        // unfolds that edge rather than the result the definition pass has
        // since memoized, so the template's answer cannot depend on how far
        // the pass has run.  (The operator itself is not part of the shape the
        // lowlevel can name — see the residual in the module docs.)
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
            return match mode {
                UnfoldMode::Key => KeyState::Undecided,
                UnfoldMode::Template => KeyState::Hashed(UNDECIDED_TOKEN),
            };
        };
        match value.as_enum() {
            Some(LowValue::USize(n)) => KeyState::Hashed(mix(n as u64)),
            // A string key hashes by its byte content.
            Some(LowValue::Str(s)) => KeyState::Hashed(mix(s
                .as_bytes()
                .iter()
                .fold(0u64, |hash, &byte| mix(hash ^ byte as u64)))),
            Some(LowValue::None) => KeyState::Hashed(NONE_TOKEN),
            // A computed nothing is never key content: it is a failed read's
            // residue, so the key is not hashable — the same class as an
            // unbound subtree (a build drops the entry, a read misses).  A
            // template is not key content at all, so there it is only a shape.
            Some(LowValue::Void) => match mode {
                UnfoldMode::Key => KeyState::Unhashable,
                UnfoldMode::Template => KeyState::Hashed(VOID_TOKEN),
            },
            // Undecided content, wherever it sits in the key: the key cannot be
            // hashed yet, and must not collapse onto one constant (two
            // different undecided keys would then collide).  A template's hole
            // is not undecided — it is what the shape says.
            Some(LowValue::Parameterized) => match mode {
                UnfoldMode::Key => KeyState::Undecided,
                UnfoldMode::Template => KeyState::Hashed(UNDECIDED_TOKEN),
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
                // SAFETY: `array` is the payload of a live node of this module
                // and the unfolding only reads, so its home block stays alive
                // across the recursion.
                for item in unsafe { array.items() } {
                    match self.hash_node(item.node, unfolding, depth - 1, mode) {
                        KeyState::Hashed(item_hash) => hash = mix(hash ^ item_hash),
                        // An undecided or unhashable position makes the whole
                        // key that — never a partial hash a different key
                        // could match.
                        verdict => return verdict,
                    }
                }
                KeyState::Hashed(hash)
            }
            // A table value keys by identity for [`Self::key_eq`], and that
            // identity cannot cross a freeze — a dynamic handle's address is
            // process-local.  The payload's content can: the fold walks the
            // entry keys' content hashes, the numbers the payload is already
            // sorted by.  A freeze copies the payload verbatim, so the fold is
            // the same number before and after a reload; folding the sorted
            // sequence makes it a function of the key *set*, not of the order
            // the source wrote the entries in.
            Some(LowValue::Table(table)) => {
                let mut hash = TABLE_SEED;
                // SAFETY: as the array arm above — `table` is the payload of a
                // live node of this module and the fold only reads.
                for item in unsafe { table.items() } {
                    hash = mix(hash ^ item.hash);
                }
                KeyState::Hashed(hash)
            }
            // The program's own value variants are opaque to the lowlevel: it
            // has no vocabulary to unfold them with, so every one contributes
            // a single token.  They are ordinary key content — a vocabulary
            // with type values reaches this from source — and `key_eq` still
            // tells two of them apart (by payload bytes where the value
            // carries a handle, by structural equality otherwise), so the
            // collision is resolved, never a wrong hit.
            None => KeyState::Hashed(EXTENSION_TOKEN),
        }
    }

    /// [`UnfoldMode::Template`]'s answer for a node, as a plain number: that
    /// walk decides every node it reaches (see [`UnfoldMode`]), so the other
    /// two outcomes are unreachable here.
    fn hash_template(&self, id: AnyNodeId, unfolding: &mut Unfolding, depth: usize) -> u64 {
        match self.hash_node(id, unfolding, depth, UnfoldMode::Template) {
            KeyState::Hashed(hash) => hash,
            KeyState::Undecided | KeyState::Unhashable => {
                unreachable!("template mode decides every node it reaches")
            }
        }
    }

    /// The content hash of a function used as key content — `None` when its
    /// template is not resolvable (the function was released, or a static ref
    /// names past its module's function list).
    ///
    /// The template's content is its *entry points*: what it returns and what
    /// it asserts, in order.  Its parameter is the function's bound variable,
    /// not content, so it is not folded in — the body reads it as the unbound
    /// cell it is.
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

    /// A node's operation *edge*: `None` when the node carries no operation at
    /// all (or names nothing live), `Some(None)` for a nullary operator, and
    /// `Some(Some(..))` for one with an operand.  This is the structural edge a
    /// template unfolds — fixed when the node is built, so it says the same
    /// thing however much of the definition pass has run since.
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
                    // SAFETY: `pa`/`pb` are payloads of live nodes of this
                    // module and `key_eq` only reads, so their home blocks
                    // stay alive across the recursion.
                    let (left, right) = (unsafe { pa.items() }, unsafe { pb.items() });
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
