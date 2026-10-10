//! The artifact codec: the byte reader/writer and the per-leaf value
//! codecs. Format: docs/notes/artifact-cache.md.

use std::collections::HashMap;
use std::sync::Arc;

use crate::{
    AnyFunctionId, AnyHandle, ArrayItem, LowOperator, LowValue, ModuleKey, Program,
    StaticFunctionId, StaticFunctionRef, StaticHandle, StaticModule, TableItem, ValueExt as _,
};

// --- byte reader / writer -----------------------------------------------------

// The byte reader/writer live in `lichen-registry`, re-exported here so
// `lichen_lowlevel::codec` paths keep resolving.
pub use lichen_registry::codec::{Reader, Writer};

// --- the per-leaf codec traits --------------------------------------------------

/// Encode/decode one **value leaf** enum.
///
/// # Invariant
///
/// The two sides must be exact inverses — every tag the writer emits, the
/// reader decodes to the equal value, and nothing else. A mismatch
/// silently breaks every cache load. Writing is **fallible**: a value
/// can be one this format cannot carry (a compute kernel's registry
/// handle has no on-disk form), and the caller's answer is to *not cache*
/// the module, never to fail the compile.
pub trait ValueCodec: Sized {
    fn write_value<P: Program>(
        w: &mut Writer,
        value: Self,
        modules: &HashMap<ModuleKey, Arc<StaticModule<P>>>,
    ) -> Result<(), String>;
    fn read_value<P: Program>(
        r: &mut Reader<'_>,
        self_key: ModuleKey,
        self_arena: &[u8],
        self_base: *const u8,
        modules: &HashMap<ModuleKey, Arc<StaticModule<P>>>,
    ) -> Result<Self, String>;
}

/// Encode/decode one **operator leaf** enum.
///
/// # Invariant
///
/// Fallible for the same reason [`ValueCodec::write_value`] is: an operator can
/// be one this format cannot carry, and refusing it by name is not the same
/// as panicking.
pub trait OperatorCodec: Sized {
    fn write_operator(w: &mut Writer, op: Self) -> Result<(), String>;
    fn read_operator(r: &mut Reader<'_>) -> Result<Self, String>;
}

/// The payload alignment of a program's frozen arena — the strictest
/// alignment any arena payload kind can need.
///
/// # Invariant
///
/// The freeze layout ([`StaticModule::from_module`]) and this codec both derive
/// the arena base from this one function, so the writer and the reader
/// provably agree; the artifact header records it as a corruption guard.
pub fn arena_align<P: Program>() -> usize {
    std::mem::align_of::<ArrayItem>()
        .max(std::mem::align_of::<TableItem>())
        .max(P::Value::alignment())
}

/// The aligned base of a module's arena — the formula the freeze layout uses,
/// so offsets round-trip exactly.
pub fn arena_base<P: Program>(arena: &[u8]) -> *const u8 {
    let align = arena_align::<P>();
    let ptr = arena.as_ptr() as usize;
    let base = (ptr + align - 1) & !(align - 1);
    base as *const u8
}

/// The base-relative offset of a handle's payload pointer.
pub fn handle_offset<P: Program>(module: &StaticModule<P>, offset: *const u8) -> usize {
    let base = arena_base::<P>(&module.arena) as usize;
    let relative = offset as usize - base;
    assert!(
        relative <= module.arena.len(),
        "a handle offset outside its module's arena — broken frozen module"
    );
    relative
}

// --- the structural leaves ------------------------------------------------------

/// Rebuild a static handle for an arena payload read out of an artifact.
///
/// # Invariant
///
/// `offset` is a whole number of `Item`s from `owner_base`, and the
/// `len * size_of::<Item>()` bytes there are inside `owner_arena` — the
/// freeze layout, so every artifact the writer emits satisfies it. The
/// bound is made in bytes with every step checked, so a crafted
/// `(offset, len)` pair is a clean `Err`, never a wrap, a panic, or a
/// pointer outside the arena.
///
/// # Safety
///
/// The construction turns those checks into the pointer: `gap` places
/// `owner_base` inside `owner_arena`, `available` bounds
/// `offset + byte_len` by the arena's bytes, and `offset` is a whole
/// number of `Item`s, so `owner_base.add(offset)` stays inside the
/// allocation and is aligned for `Item`.
fn relocated_handle<Item>(
    owner: ModuleKey,
    owner_arena: &[u8],
    owner_base: *const u8,
    offset: usize,
    len: usize,
) -> Result<StaticHandle<[Item]>, String> {
    let alignment = std::mem::align_of::<Item>();
    if !offset.is_multiple_of(alignment) {
        return Err("artifact handle is not aligned to its payload type".into());
    }
    let byte_len = len
        .checked_mul(std::mem::size_of::<Item>())
        .ok_or("artifact handle element count overflows its byte length")?;
    let end = offset
        .checked_add(byte_len)
        .ok_or("artifact handle bounds overflow")?;
    let gap = (owner_base as usize)
        .checked_sub(owner_arena.as_ptr() as usize)
        .ok_or("artifact arena base precedes its own buffer")?;
    let available = owner_arena
        .len()
        .checked_sub(gap)
        .ok_or("artifact arena base lies past the end of its buffer")?;
    if end > available {
        return Err("artifact handle out of its arena's bounds".into());
    }
    // SAFETY: `owner_base` is inside `owner_arena`, `end` within its bytes,
    // and `offset` a whole number of `Item`s.
    let payload = unsafe { owner_base.add(offset) as *const Item };
    Ok(StaticHandle {
        module: owner,
        offset: std::ptr::slice_from_raw_parts(payload, len),
    })
}

/// The read half of the relocated layout: the owner's key, the
/// payload's base-relative offset, and its element count.
///
/// # Invariant
///
/// The owner's arena is the reader's own for `self_key`, else the
/// dependency module named by the key — the one lookup the array and
/// the table leaf share, so the two cannot drift.
fn read_relocated_handle<Item, P: Program>(
    r: &mut Reader<'_>,
    self_key: ModuleKey,
    self_arena: &[u8],
    self_base: *const u8,
    modules: &HashMap<ModuleKey, Arc<StaticModule<P>>>,
) -> Result<StaticHandle<[Item]>, String> {
    let owner = ModuleKey::from_raw(r.u64()?);
    let offset = r.u64()? as usize;
    let len = r.u64()? as usize;
    let (owner_arena, owner_base) = if owner == self_key {
        (self_arena, self_base)
    } else {
        let module = modules
            .get(&owner)
            .ok_or_else(|| format!("artifact references unregistered dependency key {owner:?}"))?;
        let arena: &[u8] = &module.arena;
        (arena, arena_base::<P>(arena))
    };
    relocated_handle::<Item>(owner, owner_arena, owner_base, offset, len)
}

/// The write half of [`read_relocated_handle`]: owner key, payload byte
/// offset, element count.
fn write_relocated_handle<Item, P: Program>(
    w: &mut Writer,
    modules: &HashMap<ModuleKey, Arc<StaticModule<P>>>,
    module_key: ModuleKey,
    offset: *const [Item],
) {
    w.u64(module_key.as_raw());
    let module = &modules[&module_key];
    // SAFETY: a handle's payload is inside its home module's arena, and
    // `handle_offset` asserts the address is inside it.
    let slice = unsafe { &*offset };
    w.u64(handle_offset(module, slice.as_ptr() as *const u8) as u64);
    w.u64(slice.len() as u64);
}

impl ValueCodec for LowValue {
    fn write_value<P: Program>(
        w: &mut Writer,
        value: Self,
        modules: &HashMap<ModuleKey, Arc<StaticModule<P>>>,
    ) -> Result<(), String> {
        match value {
            LowValue::USize(n) => {
                w.u8(0);
                w.u64(n as u64);
            }
            // Tag 8 is additive (`Error` took 7). The float travels as its
            // 32-bit pattern, which round-trips every value.
            LowValue::Float(n) => {
                w.u8(8);
                w.u32(n.to_bits());
            }
            LowValue::Array(AnyHandle::Static(handle)) => {
                w.u8(1);
                write_relocated_handle(w, modules, handle.module, handle.offset);
            }
            // Freezing rewrites every dynamic payload to a static ref, so
            // this means the module was not a frozen artifact.
            LowValue::Array(AnyHandle::Dynamic(_)) => {
                return Err("cannot serialize a module carrying a dynamic array payload".into());
            }
            LowValue::Table(AnyHandle::Static(handle)) => {
                w.u8(6);
                write_relocated_handle(w, modules, handle.module, handle.offset);
            }
            LowValue::Table(AnyHandle::Dynamic(_)) => {
                return Err("cannot serialize a module carrying a dynamic table payload".into());
            }
            LowValue::Function(AnyFunctionId::Static(function)) => {
                w.u8(2);
                w.u64(function.module.as_raw());
                w.u64(function.index.0 as u64);
            }
            LowValue::Function(AnyFunctionId::Dynamic(_)) => {
                return Err("cannot serialize a module carrying a dynamic function ref".into());
            }
            LowValue::None => w.u8(3),
            // Tag 7 is additive, and tag 3 keeps meaning the `None` unit.
            // The variant was named `Void` then; the tag is unchanged.
            LowValue::Error => w.u8(7),
            LowValue::Str(s) => {
                w.u8(5);
                w.u32(s.len() as u32);
                w.bytes(s.as_bytes());
            }
        }
        Ok(())
    }

    fn read_value<P: Program>(
        r: &mut Reader<'_>,
        self_key: ModuleKey,
        self_arena: &[u8],
        self_base: *const u8,
        modules: &HashMap<ModuleKey, Arc<StaticModule<P>>>,
    ) -> Result<Self, String> {
        Ok(match r.u8()? {
            0 => LowValue::USize(r.u64()? as usize),
            8 => LowValue::Float(f32::from_bits(r.u32()?)),
            1 => LowValue::Array(AnyHandle::Static(read_relocated_handle::<ArrayItem, P>(
                r, self_key, self_arena, self_base, modules,
            )?)),
            2 => {
                let module = ModuleKey::from_raw(r.u64()?);
                let index = r.u64()? as usize;
                // A function ref indexes the module it names, checked when the
                // map holds it. Only direct dependencies are mapped.
                if let Some(owner) = modules.get(&module)
                    && index >= owner.functions.len()
                {
                    return Err(format!(
                        "artifact function ref names function {index}, which is not among module {module:?}'s {} functions",
                        owner.functions.len()
                    ));
                }
                LowValue::Function(AnyFunctionId::Static(StaticFunctionRef {
                    module,
                    index: StaticFunctionId(index),
                }))
            }
            3 => LowValue::None,
            7 => LowValue::Error,
            // Tag 4 is **reserved**: it named the deleted `Parameterized`
            // marker. Such an artifact is refused by name.
            4 => {
                return Err(
                    "artifact carries lowlevel value tag 4, the deleted `Parameterized` marker: \
                     it was written by a version that still had that marker"
                        .into(),
                );
            }
            5 => {
                let len = r.u32()? as usize;
                let bytes = r.take(len)?;
                // Leaked to `&'static str` by decision `D14`; see
                // docs/notes/code-audit.md.
                let s = std::str::from_utf8(bytes).map_err(|_| "string literal is not UTF-8")?;
                LowValue::Str(Box::leak(s.to_string().into_boxed_str()))
            }
            6 => LowValue::Table(AnyHandle::Static(read_relocated_handle::<TableItem, P>(
                r, self_key, self_arena, self_base, modules,
            )?)),
            tag => return Err(format!("unknown lowlevel value tag {tag}")),
        })
    }
}

impl OperatorCodec for LowOperator {
    fn write_operator(w: &mut Writer, op: LowOperator) -> Result<(), String> {
        match op {
            LowOperator::Index => w.u8(0),
            LowOperator::Apply => w.u8(1),
            LowOperator::TableGet => w.u8(2),
        }
        Ok(())
    }

    fn read_operator(r: &mut Reader<'_>) -> Result<LowOperator, String> {
        Ok(match r.u8()? {
            0 => LowOperator::Index,
            1 => LowOperator::Apply,
            2 => LowOperator::TableGet,
            tag => return Err(format!("unknown lowlevel operator tag {tag}")),
        })
    }
}
