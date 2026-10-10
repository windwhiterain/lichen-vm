//! Reading a claim's payload: the record of fields a `?` label holds.

use std::collections::BTreeMap;

use lichen_compute::ComputeValue;
use lichen_lowlevel::{AnyNodeId, LowValue, Module, NodeId};
use lichen_render::struct_type_named_fields;
use lichen_utils::extend::AsEnum;

use crate::fusion_claim::ClaimRefusal;
use crate::program::LangProgram;

/// The payload a label slot holds, as its field names to their value nodes.
///
/// # Invariant
/// The slot is the annotation value's `[value, type]` pair — the one shape every attribute
/// slot has — so the payload is that pair's value and the names come from its type chain
/// rather than from a shape a reader hardcodes.
pub fn record(
    module: &Module<LangProgram>,
    slot: NodeId,
) -> Result<BTreeMap<&'static str, NodeId>, ClaimRefusal> {
    // SAFETY: `slot` is a live node of `module`; nothing here releases a block.
    let pair = unsafe { module.array_items(slot) }.ok_or(ClaimRefusal::PayloadNotARecord)?;
    let value = pair.first().and_then(|item| dynamic(item.node));
    let ty = pair.get(1).and_then(|item| dynamic(item.node));
    // SAFETY: `value` is a field of the slot pair, a live node of `module`.
    let fields = value
        .and_then(|value| unsafe { module.array_items(value) })
        .ok_or(ClaimRefusal::PayloadNotARecord)?;
    let names = ty
        .and_then(|ty| struct_type_named_fields(module, ty))
        .unwrap_or_default();
    let mut record = BTreeMap::new();
    for (at, item) in fields.iter().enumerate() {
        let Some(Some(name)) = names.get(at) else {
            continue;
        };
        if let Some(node) = dynamic(item.node) {
            record.insert(*name, node);
        }
    }
    Ok(record)
}

/// The text a payload field holds.
pub fn text(
    module: &Module<LangProgram>,
    record: &BTreeMap<&'static str, NodeId>,
    name: &'static str,
) -> Result<String, ClaimRefusal> {
    let node = *record
        .get(name)
        .ok_or(ClaimRefusal::MissingField { name })?;
    match held(module, node) {
        Some(LowValue::Str(value)) => Ok(value.to_string()),
        other => Err(ClaimRefusal::FieldNotText {
            name,
            found: describe(other),
        }),
    }
}

/// The texts a payload field holds: a tuple of strings, or one string of its own.
pub fn texts(
    module: &Module<LangProgram>,
    record: &BTreeMap<&'static str, NodeId>,
    name: &'static str,
) -> Result<Vec<String>, ClaimRefusal> {
    let node = *record
        .get(name)
        .ok_or(ClaimRefusal::MissingField { name })?;
    let refused = |found: String| ClaimRefusal::FieldNotText { name, found };
    match held(module, node) {
        Some(LowValue::Str(value)) => Ok(vec![value.to_string()]),
        Some(LowValue::Array(items)) => {
            // SAFETY: `items` is the payload of the value read from the live node `node`.
            let items = unsafe { items.items() };
            let mut texts = Vec::with_capacity(items.len());
            for item in items {
                let text = dynamic(item.node)
                    .and_then(|node| held(module, node))
                    .and_then(|value| match value {
                        LowValue::Str(value) => Some(value.to_string()),
                        _ => None,
                    });
                texts.push(text.ok_or_else(|| refused("a record of no text".to_string()))?);
            }
            Ok(texts)
        }
        other => Err(refused(describe(other))),
    }
}

/// What a compute value is, for a refusal that names it.
pub fn describe_compute(value: Option<ComputeValue>) -> String {
    match value {
        Some(ComputeValue::Kernel(_)) | Some(ComputeValue::ParKernel(..)) => "a kernel".to_string(),
        Some(ComputeValue::Buffer(..)) | Some(ComputeValue::DeviceBuffer(_)) => {
            "a buffer".to_string()
        }
        Some(_) => "a compute value".to_string(),
        None => "an undecided value".to_string(),
    }
}

/// What a value is, for a refusal that names it.
pub fn describe(value: Option<LowValue>) -> String {
    match value {
        Some(other) => format!("a {}", spelling(&other)),
        None => "an undecided value".to_string(),
    }
}

/// The value a dynamic node holds, or `None` when nothing decided it.
fn held(module: &Module<LangProgram>, node: NodeId) -> Option<LowValue> {
    module
        .node_value(AnyNodeId::Dynamic(node))
        .and_then(|value| AsEnum::<LowValue>::as_enum(&value))
}

/// The id of a dynamic node; a static reference is no node of this module.
fn dynamic(id: AnyNodeId) -> Option<NodeId> {
    match id {
        AnyNodeId::Dynamic(node) => Some(node),
        AnyNodeId::Static(_) => None,
    }
}

/// The name a lowlevel value goes by in a refusal.
fn spelling(value: &LowValue) -> &'static str {
    match value {
        LowValue::USize(_) => "runtime scalar",
        LowValue::Float(_) => "runtime float",
        LowValue::Str(_) => "string",
        LowValue::Array(_) => "array",
        LowValue::Table(_) => "table",
        LowValue::Function(_) => "function",
        LowValue::None => "unit",
        LowValue::Error => "refused value",
    }
}
