//! Per-binding content keys: the judge the key ruling puts beside propagation.
//! See docs/notes/heavy-value-cache.md §2.
//!
//! # Invariant
//!
//! A read is named by the binder's **occurrence path**, never by its `BinderId`: ids are
//! numbered in traversal order, so an insertion before a binding would change the key of a
//! binding nothing touched.

use std::collections::HashMap;

use lichen_language_parser::ast::{BinderId, Program, Stmt};
use lichen_language_parser::path::Path;

use crate::compile::cached_bindings;
use crate::dirty::statement_dependencies;
use crate::resolve::content_key::binding_key;

/// A token for an input the file does not define: the prelude, or an import's binding.
const EXTERNAL: u64 = 0x6578_7465_726e_0001;

/// A token for a statement's read of itself, so a recursive function has a base.
const SELF_READ: u64 = 0x7365_6c66_0000_0001;

/// Each marked binding's key, or `None` where the key cannot be written down.
///
/// # Invariant
/// `None` means the key would be unsound, not that the value is unchanged: a read of a binder
/// declared inside a statement is instance-dependent, and a cycle has no base. A `None` must be
/// read as "changed".
pub fn binding_keys(program: &Program) -> HashMap<Path, Option<Vec<u64>>> {
    let paths = binder_paths(program);
    let mut owner: HashMap<BinderId, usize> = HashMap::new();
    for (index, statement) in program.statements.iter().enumerate() {
        if let Stmt::Binding(binding) = &statement.stmt
            && let Some(binder) = binding.binder
        {
            owner.insert(binder, index);
        }
    }
    let deps = statement_dependencies(program);
    let mut keys = Keys {
        program,
        deps: &deps,
        paths: &paths,
        owner,
        memo: HashMap::new(),
        visiting: Vec::new(),
    };
    let mut out = HashMap::new();
    for (binder, (path, _)) in cached_bindings(program) {
        let owner = keys.owner.get(&binder).copied();
        let key = match owner {
            Some(index) => keys.of(index),
            None => None,
        };
        out.insert(path, key);
    }
    out
}

/// Every binder's occurrence path, at any depth.
fn binder_paths(program: &Program) -> HashMap<BinderId, Path> {
    let mut out = HashMap::new();
    crate::path::for_each(program, &mut |path, node| {
        let binder = match node {
            crate::path::Node::Binding(binding) => binding.binder,
            crate::path::Node::Field(field) => field.binder,
            crate::path::Node::Expr(_) => None,
        };
        if let Some(binder) = binder {
            out.insert(binder, path.clone());
        }
    });
    out
}

struct Keys<'a> {
    program: &'a Program,
    deps: &'a [(Vec<BinderId>, Vec<BinderId>)],
    paths: &'a HashMap<BinderId, Path>,
    owner: HashMap<BinderId, usize>,
    memo: HashMap<usize, Option<Vec<u64>>>,
    visiting: Vec<usize>,
}

impl Keys<'_> {
    fn of(&mut self, index: usize) -> Option<Vec<u64>> {
        if let Some(done) = self.memo.get(&index) {
            return done.clone();
        }
        if self.visiting.contains(&index) {
            return None;
        }
        self.visiting.push(index);
        let reads = self
            .deps
            .get(index)
            .map(|(_, reads)| reads.clone())
            .unwrap_or_default();
        let mut digests = Vec::with_capacity(reads.len());
        let mut sound = true;
        for binder in reads {
            let declared = self.owner.get(&binder).copied();
            if declared.is_none() && self.paths.contains_key(&binder) {
                // Declared inside a statement: its value is an apply's, not this file's.
                sound = false;
                break;
            }
            match declared {
                // A self-reference folds to a constant: refusing is inert for recursion.
                Some(owner) if owner == index => digests.push(SELF_READ),
                Some(owner) => match self.of(owner) {
                    Some(key) => digests.push(digest(&key)),
                    None => {
                        sound = false;
                        break;
                    }
                },
                None => digests.push(EXTERNAL),
            }
        }
        self.visiting.pop();
        let key = if !sound {
            None
        } else if let Stmt::Binding(binding) = &self.program.statements[index].stmt {
            Some(binding_key(&binding.value, self.paths, &digests))
        } else {
            None
        };
        self.memo.insert(index, key.clone());
        key
    }
}

/// A key folded to one word, so a dependent's key stays one word however long the chain is.
fn digest(key: &[u64]) -> u64 {
    key.iter().fold(0xcbf2_9ce4_8422_2325, |hash, word| {
        (hash ^ *word).wrapping_mul(0x100_0000_01b3)
    })
}
