//! The wasm backend: a kernel fragment set lowered through
//! [`waffle`](https://github.com/bytecodealliance/waffle).
//!
//! **The lowering is `KernelInstr` → `waffle::Operator`, not `KernelInstr` →
//! `wasm_encoder::Instruction`.** That is the whole of the change: `waffle` owns
//! the CFG-to-structured-wasm pipeline (`reducify`, `treeify`, `stackify`,
//! `localify`) and hands back a module whose bodies are valid by construction,
//! where a hand-written emitter had to track the operand stack itself and got it
//! wrong in four separate ways (`docs/notes/wasm-control-flow.md` §2).
//!
//! Three files, one per question:
//!
//! - `lower` — *what does this body compute*: the instruction map, one
//!   `KernelInstr` at a time, onto `waffle`'s SSA values.
//! - `flow` — *where does the stack go*: `If`/`Jump`/`While` onto `waffle`'s
//!   blocks, with a loop's carried tuple as its header's blockparams.
//! - `assemble` — *what does the module look like*: the buffer imports, one
//!   function per fragment, and the export the host resolves `main` by.
//! - `mixed` — *which class is each value*, checked before any of the above so
//!   that a fragment meeting an `Int` and a `Float` in one operation costs no
//!   emitted instruction.
//!
//! **Structured control flow is `flow`'s**, and it is the shape
//! `docs/notes/wasm-backend-handoff.md` §3.2 decides: a labelled block is a
//! loop's header, the fragment's entry block stays the preheader, and an arm of a
//! branch is a block of its own.

mod assemble;
pub(crate) mod lower;
pub(crate) mod mixed;

pub(crate) use assemble::assemble_module;
