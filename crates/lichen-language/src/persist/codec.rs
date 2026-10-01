//! The artifact codec: the vocabulary-specific half of value and operator
//! encoding, and the marker for a program that is compiled in memory only.

use super::*;

use std::collections::HashMap;
use std::sync::Arc;

use lichen_highlevel::program::HighProgram;
use lichen_lowlevel::{Program, StaticModule};

/// The vocabulary-specific half of the artifact format.
///
/// The artifact header, node/function frames, arena layout and equality data
/// are generic.  The only vocabulary-dependent parts are the value encoding
/// and the operator encoding; this trait isolates them so a downstream
/// program with extra value/operator variants can reuse the same artifact
/// container by implementing a codec.
///
/// **Codec contract:** `write_value`/`read_value` (and `write_operator`/
/// `read_operator`) must be exact inverses — every tag the writer emits, the
/// reader must decode to the equal value, and nothing else.  Adding a variant
/// to one side and not the other compiles but silently breaks every cache
/// load (a stored artifact fails to deserialize and is recompiled).  This is
/// enforced by the `codec_roundtrip` test below, which round-trips every
/// value and operator variant; keep the two sides in sync with it.
pub trait ArtifactCodec<P: Program> {
    /// Whether this codec actually persists to a device cache directory.  A
    /// codec that is in-memory only (`NoPersist`) sets this to `false` so the
    /// CLI drives the package store without a cache directory (and so never
    /// reaches a serialize/deserialize path that would panic).
    const PERSISTENT: bool = true;

    /// Write one node value.
    ///
    /// Fallible for the same reason [`ArtifactCodec::read_value`] is: the
    /// leaf-name discriminator has a fixed-width length field, and a
    /// vocabulary leaf whose name does not fit it is refused rather than
    /// truncated (`Writer::leaf`).
    fn write_value(
        w: &mut Writer,
        value: P::Value,
        modules: &HashMap<ModuleKey, Arc<StaticModule<P>>>,
    ) -> Result<(), String>;

    /// Read one node value.
    fn read_value(
        r: &mut Reader<'_>,
        self_key: ModuleKey,
        self_arena: &[u8],
        self_base: *const u8,
        modules: &HashMap<ModuleKey, Arc<StaticModule<P>>>,
    ) -> Result<P::Value, String>;

    /// Write one operation's operator tag.  Fallible for the same reason
    /// [`ArtifactCodec::write_value`] is.
    fn write_operator(w: &mut Writer, operator: P::Operator) -> Result<(), String>;

    /// Read one operation's operator tag.
    fn read_operator(r: &mut Reader<'_>) -> Result<P::Operator, String>;
}

/// A compiled program that carries its own artifact codec, so the language
/// tooling is generic over a single program type `P` (the associated-type
/// collector) rather than the value/operator leaves plus a separate `C` codec.
///
/// A codec is not an associated type of the lowlevel [`Program`] trait — it is a
/// serialization concern layered on top by `lichen-language` — so this trait is
/// the seam that folds the codec into the collector.  The
/// [`lang_compose_vocabulary!`](crate::lang_compose_vocabulary) macro implements
/// it for every composed program, binding `Codec` to the
/// [`crate::program::ProgramCodec`] that vocabulary emits; a program that never
/// persists uses [`NoPersist`].
pub trait ProgramCodecOf: HighProgram {
    /// The artifact codec for this program (a composed program's
    /// [`crate::program::ProgramCodec`], or [`NoPersist`] for one that never
    /// serializes).
    type Codec: ArtifactCodec<Self> + Default;
}

/// A marker codec for a program that is compiled in memory only and never
/// serialized to the device cache (the package store's in-memory path, and a
/// plugin program whose artifact codec has not been generated yet).  Every
/// method is unreachable — the codec is only ever selected when the store has
/// no cache directory, so `try_reuse`/`build_package` never reach the
/// serialize/deserialize path.
pub struct NoPersist;

impl<P: Program> ArtifactCodec<P> for NoPersist {
    const PERSISTENT: bool = false;

    fn write_value(
        _w: &mut Writer,
        _value: P::Value,
        _modules: &HashMap<ModuleKey, Arc<StaticModule<P>>>,
    ) -> Result<(), String> {
        unreachable!("NoPersist cannot write values — the store has no device cache")
    }

    fn read_value(
        _r: &mut Reader<'_>,
        _self_key: ModuleKey,
        _self_arena: &[u8],
        _self_base: *const u8,
        _modules: &HashMap<ModuleKey, Arc<StaticModule<P>>>,
    ) -> Result<P::Value, String> {
        unreachable!("NoPersist cannot read values — the store has no device cache")
    }

    fn write_operator(_w: &mut Writer, _operator: P::Operator) -> Result<(), String> {
        unreachable!("NoPersist cannot write operators — the store has no device cache")
    }

    fn read_operator(_r: &mut Reader<'_>) -> Result<P::Operator, String> {
        unreachable!("NoPersist cannot read operators — the store has no device cache")
    }
}

impl Default for NoPersist {
    fn default() -> Self {
        NoPersist
    }
}

// ---------------------------------------------------------------------------
// Codec-round-trip tests: enforce the write/read bijection contract.
//
// `write_value`/`read_value` and `write_operator`/`read_operator` are two
// independent exhaustive `match`es — one over the value/operator *type*, one
// over the *tag byte*.  The compiler can check each side is total, but it
// cannot check that they name the same tag.  A variant added to the write
// side but not the read side (exactly the `TypeString` asymmetry) compiles
// and silently makes every stored artifact underivable.  These tests drive
// every (arena-free) value and every operator through the codec and assert
// the round trip is the identity, so an asymmetry fails the build.
// ---------------------------------------------------------------------------
#[cfg(test)]
mod codec_roundtrip {
    use super::*;
    use crate::program::{LangOperator, LangProgram, LangValue, ProgramCodec};
    use lichen_highlevel::program::{TypeOperator, TypeValue};
    use lichen_lowlevel::{LowOperator, LowValue};

    /// Encode `v`, then decode it back and return the deserialized value.
    /// Arena-free variants never touch the module map or the (dummy) self
    /// arena/base, so the map is empty and the base is null.
    fn roundtrip_value(v: LangValue) -> LangValue {
        let modules: HashMap<ModuleKey, Arc<StaticModule<LangProgram>>> = HashMap::new();
        let mut w = Writer::new();
        ProgramCodec::write_value(&mut w, v, &modules).expect("a leaf name fits the discriminator");
        let bytes = w.finish().expect("a complete leaf name");
        let mut r = Reader::new(&bytes);
        let out = ProgramCodec::read_value(
            &mut r,
            ModuleKey::from_raw(0),
            &[],
            std::ptr::null(),
            &modules,
        )
        .expect("deserializing a value the codec itself wrote");
        assert!(r.done(), "the value codec left trailing bytes");
        out
    }

    /// Every `LangValue` variant that round-trips without a module arena.  The
    /// handle/function-ref variants (array/table/function tags) need a real
    /// frozen module and are exercised at the artifact level by the `persist`
    /// integration tests; this covers every scalar/type/string variant.  The
    /// `TypeValue`/`TypeOperator` coverage iterates the leaf enums' own
    /// registry-derived variant lists ([`TypeValue::KIND_MARKERS`],
    /// [`TypeOperator::ALL`]), so a variant added to the single-source list
    /// is covered here automatically — the hand-written part of this list
    /// only spells the leaves that have no such registry.
    #[test]
    fn every_arena_free_value_round_trips() {
        let values: &[LangValue] = &[
            LangValue::LowValue(LowValue::USize(41)),
            LangValue::LowValue(LowValue::None),
            LangValue::LowValue(LowValue::Void),
            LangValue::LowValue(LowValue::Parameterized),
            LangValue::LowValue(LowValue::Str("hello")),
            LangValue::TypeValue(TypeValue::TypeId(7)),
            LangValue::ComputeValue(::lichen_compute::ComputeValue::TypeBuffer),
            LangValue::ComputeValue(::lichen_compute::ComputeValue::TypeWrite),
        ];
        for &v in values {
            assert_eq!(roundtrip_value(v), v, "value did not round-trip");
        }
        for &marker in TypeValue::KIND_MARKERS {
            let v = LangValue::TypeValue(marker);
            assert_eq!(roundtrip_value(v), v, "kind marker did not round-trip");
        }
    }

    fn roundtrip_op(op: LangOperator) -> LangOperator {
        let mut w = Writer::new();
        ProgramCodec::write_operator(&mut w, op).expect("a leaf name fits the discriminator");
        let bytes = w.finish().expect("a complete leaf name");
        let mut r = Reader::new(&bytes);
        let out = ProgramCodec::read_operator(&mut r)
            .expect("deserializing an operator the codec itself wrote");
        assert!(r.done(), "the operator codec left trailing bytes");
        out
    }

    #[test]
    fn every_operator_round_trips() {
        let ops: &[LangOperator] = &[
            LangOperator::LowOperator(LowOperator::Index),
            LangOperator::LowOperator(LowOperator::Apply),
            LangOperator::LowOperator(LowOperator::TableGet),
            LangOperator::GcdOp(crate::program::GcdOp::Gcd),
        ];
        for &op in ops {
            assert_eq!(roundtrip_op(op), op, "operator did not round-trip");
        }
        for &ty_op in TypeOperator::ALL {
            let op = LangOperator::TypeOperator(ty_op);
            assert_eq!(roundtrip_op(op), op, "type operator did not round-trip");
        }
    }
}
