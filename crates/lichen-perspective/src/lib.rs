//! The `lichen-perspective` plugin: the perspective attribute (`# p`) and its
//! divisibility-lattice semantics.
//!
//! Perspective means *"uniform over `p` aligned threads"* — the plain
//! non-negative integer a `# p` annotation puts on an expression.  Its lattice
//! is divisibility (`a ⊑ b ⟺ a | b`): the **meet** is `gcd`, the **top** is
//! `0` (uniform over all threads, the `∞` fold — a value with no `#` is not
//! expressed per-thread, so it is uniform over everything), and the **bottom**
//! is `1`.
//!
//! # A compiler plugin, not a native plugin
//!
//! This crate is a **compiler plugin** (see `docs/notes/plugin-taxonomy.md`),
//! not a native plugin like `lichen-compute`.  The semantics below are
//! **program-generic** — they never name a concrete host program — but the
//! language layer must **codesign** with the plugin:
//!
//! - the grammar production and AST node for `# p` (the `AnnPiece::Perspective`
//!   annotation),
//! - the `Schema { tail: [.., Perspective, ..] }` IR form the checker reads,
//!   and
//! - the operator **persist discriminator** (`u8(9)` for `GcdOp`) in the
//!   on-disk format.
//!
//! Those live in the host language layer (e.g. `lichen-language`), because
//! they are tied to one language's surface and on-disk contract and so cannot
//! be shared by any host.  What *can* be shared — and is now in this crate —
//! is the **meaning**: the [`Perspective`] attribute marker and its
//! [`AttrExt`] lowering, and the [`GcdOp`] n-ary-gcd operator leaf and its
//! [`OperatorExt`] run.  A host composes both leaves into its own value and
//! operator vocabularies and calls [`persp_attr_ext::<P>()`] to register the
//! attribute's extension with the checker.
//!
//! The codesign boundary is exactly the taxonomy rule: a feature that needs a
//! new grammar production, an IR node, a schema-tail literal, or a persist
//! node is a **compiler plugin**; one that only supplies vocabulary leaves and
//! extension-point values is a **native plugin**.
//!
//! [`AttrExt`]: lichen_highlevel::attr::AttrExt
//! [`OperatorExt`]: lichen_lowlevel::OperatorExt

pub mod perspective;

pub use perspective::{GcdOp, Perspective, divides, gcd, persp_attr_ext};
