//! The artifact codec: value/operator encoding, and the in-memory-only marker.

use super::*;

use std::collections::HashMap;
use std::sync::Arc;

use lichen_highlevel::program::HighProgram;
use lichen_lowlevel::{Program, StaticModule};

/// The vocabulary-specific half of the artifact format.
///
/// # Invariant
///
/// `write_value`/`read_value` (and `write_operator`/`read_operator`) must be
/// exact inverses; an asymmetry loads nothing and makes every stored artifact
/// underivable. See docs/notes/artifact-cache.md.
pub trait ArtifactCodec<P: Program> {
    /// Whether this codec persists to a device cache directory.
    ///
    /// # Invariant
    ///
    /// `false` (`NoPersist`) means the store has no cache directory and never
    /// reaches the serialize/deserialize paths, which would panic.
    const PERSISTENT: bool = true;

    /// Write one node value.
    ///
    /// # Invariant
    ///
    /// Fallible: a leaf name that does not fit the fixed-width discriminator is
    /// refused rather than truncated.
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

/// A compiled program that carries its own artifact codec.
///
/// # Invariant
///
/// The codec seam folds the codec into the single `P` associated-type
/// collector, which the lowlevel [`Program`] trait does not name.
pub trait ProgramCodecOf: HighProgram {
    /// The artifact codec for this program.
    type Codec: ArtifactCodec<Self> + Default;
}

/// A marker codec for a program that is never serialized to the device cache.
///
/// # Invariant
///
/// Every method is `unreachable!`: the codec is selected only when the store
/// has no cache directory.
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

// --- Codec-round-trip tests: enforce the write/read bijection contract -------

// The compiler checks each side is total, not that they name the same tag, so
// every value and operator round-trips here.
#[cfg(test)]
mod codec_roundtrip {
    use super::*;
    use crate::program::{LangOperator, LangProgram, LangValue, ProgramCodec};
    use lichen_highlevel::program::{TypeOperator, TypeValue};
    use lichen_lowlevel::{LowOperator, LowValue};

    /// Encode `v`, decode it back, and return the deserialized value.
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

    /// Every arena-free `LangValue` variant round-trips.
    ///
    /// # Invariant
    ///
    /// The `TypeValue`/`TypeOperator` coverage iterates the leaves' own
    /// registry-derived variant lists, so a variant added there is covered here
    /// automatically; the handle/function-ref variants need a real frozen module
    /// and are exercised by the persist integration tests.
    #[test]
    fn every_arena_free_value_round_trips() {
        let values: &[LangValue] = &[
            LangValue::LowValue(LowValue::USize(41)),
            LangValue::LowValue(LowValue::None),
            LangValue::LowValue(LowValue::Error),
            LangValue::LowValue(LowValue::Str("hello")),
            LangValue::TypeValue(TypeValue::TypeId(7)),
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
