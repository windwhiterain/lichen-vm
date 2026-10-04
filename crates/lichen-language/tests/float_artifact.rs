//! Phase 0, third round trip: an artifact written and read back.
//!
//! A float's value encoding is tag `8` plus its 32 bits
//! (`crates/lichen-lowlevel/src/codec.rs`), and the artifact format version is
//! the current `ARTIFACT_FORMAT_VERSION` (now `9`;
//! `crates/lichen-language/src/persist.rs`).  The round trip must be
//! bit-identical, and it must keep `LowValue`'s hand-written identity: `0.0` and
//! `-0.0` come back as two distinct values while two equal `NaN` bit patterns
//! come back as one — the asymmetry that lets a reuse decision accept a
//! bit-identical artifact and never accept one of two distinct ones
//! (`docs/notes/floating-point.md` §3.1, §3.6).

use std::collections::HashMap;
use std::sync::Arc;

use lichen_language::persist::{deserialize_artifact, serialize_artifact};
use lichen_language::program::{LangProgram as P, LangValue};
use lichen_lowlevel::codec::{ValueCodec, Writer};
use lichen_lowlevel::{BlockId, LowValue, Module, ModuleKey, NodeId, StaticModule};
use lichen_utils::extend::AsEnum;

/// A node holding the float `value`.
fn float_node(module: &mut Module<P>, block: BlockId, value: f32) -> NodeId {
    module.add_node(block, None, Some(LangValue::from(LowValue::Float(value))))
}

#[test]
fn a_float_artifact_round_trips_bit_identically_and_keeps_its_bitwise_identity() {
    let modules: HashMap<ModuleKey, Arc<StaticModule<P>>> = HashMap::new();

    // The value encoding itself: tag 8, then the 32 bits little-endian.
    let mut writer = Writer::new();
    <LowValue as ValueCodec>::write_value::<P>(&mut writer, LowValue::Float(-0.0), &modules)
        .expect("a float is encodable");
    let mut expected = vec![8u8];
    expected.extend_from_slice(&(-0.0f32).to_bits().to_le_bytes());
    assert_eq!(writer.finish().unwrap(), expected, "tag 8 plus the 32 bits");

    // A source module holding the two zeros and two equal NaN bit patterns.
    let mut source = Module::<P>::new();
    let block = source.add_block(None);
    let zero = float_node(&mut source, block, 0.0);
    let negative_zero = float_node(&mut source, block, -0.0);
    let nan = float_node(&mut source, block, f32::NAN);
    let same_nan = float_node(&mut source, block, f32::NAN);
    for node in [zero, negative_zero, nan, same_nan] {
        source.evaluate_node_deep(node, None);
    }

    // Freeze, write and read the whole artifact — not just the value codec.
    let mut host = Module::<P>::new();
    let freeze = host.freeze_mapped(&source, ModuleKey::from_raw(1), [0; 32]);
    let module = host
        .registry
        .read()
        .unwrap()
        .get(freeze.key)
        .expect("the artifact is filed")
        .module
        .clone();
    let bytes = serialize_artifact(&module, &modules, [0; 32], freeze.node_map[&zero])
        .expect("a float-only artifact is encodable");
    let (loaded, _export) = deserialize_artifact(&bytes, freeze.key, [0; 32], &modules)
        .expect("the artifact the writer produced must load");

    let read = |node: NodeId| {
        loaded.nodes[freeze.node_map[&node].index]
            .value
            .expect("the loaded node carries a value")
    };
    let bits = |value: &LangValue| match value.as_enum() {
        Some(LowValue::Float(n)) => n.to_bits(),
        other => panic!("expected a float, got {other:?}"),
    };

    // Bit-identical, for the two zeros and for a NaN.
    assert_eq!(bits(&read(zero)), 0.0f32.to_bits());
    assert_eq!(bits(&read(negative_zero)), (-0.0f32).to_bits());
    assert_eq!(bits(&read(nan)), f32::NAN.to_bits());

    // §3.1's identity: the two zeros are two values, the two equal NaNs are one.
    // The two NaNs are distinct *nodes*, so the equality below is a value
    // equality and not one node compared with itself.
    assert_ne!(freeze.node_map[&nan], freeze.node_map[&same_nan]);
    assert_ne!(
        read(zero),
        read(negative_zero),
        "0.0 and -0.0 are two bit patterns, so two values"
    );
    assert_eq!(
        read(nan),
        read(same_nan),
        "two equal NaN bit patterns are one value"
    );
}
