//! The artifact container: the writer, the reader, and the shape encoders.
//! See docs/notes/artifact-cache.md.

use super::*;

use std::collections::HashMap;
use std::sync::Arc;

use lichen_lowlevel::codec::{read_relocated_handle, write_relocated_handle};
use lichen_lowlevel::{
    ArrayItem, EvaluatedDeep, LocalNodeId, LowShape, Program, StaticFunction, StaticFunctionId,
    StaticFunctionRef, StaticModule, StaticNode, StaticOperation,
};

use crate::program::{LangProgram, ProgramCodec};

/// Serialize `module`, and the arenas its refs point into, into the artifact.
///
/// # Invariant
///
/// Fallible only for the vocabulary's encodability: a leaf name the one-byte
/// discriminator cannot hold is refused, not truncated.
pub fn serialize_artifact(
    module: &StaticModule<LangProgram>,
    modules: &HashMap<ModuleKey, Arc<StaticModule<LangProgram>>>,
    hash: Hash,
    export: LocalNodeId,
) -> Result<Vec<u8>, String> {
    serialize_artifact_with(module, modules, hash, export, ProgramCodec)
}

/// [`serialize_artifact`] with an explicit [`ArtifactCodec`].
pub fn serialize_artifact_with<P, C>(
    module: &StaticModule<P>,
    modules: &HashMap<ModuleKey, Arc<StaticModule<P>>>,
    hash: Hash,
    export: LocalNodeId,
    _codec: C,
) -> Result<Vec<u8>, String>
where
    P: Program,
    C: ArtifactCodec<P>,
{
    // The body first: its digest goes in the header ahead of it.
    let mut w = Writer::new();
    w.u64(export.index as u64);
    w.u64(module.arena.len() as u64);
    w.bytes(&module.arena);
    w.u64(module.nodes.len() as u64);
    for node in &module.nodes {
        match node.value {
            None => w.u8(0),
            Some(value) => {
                w.u8(1);
                C::write_value(&mut w, value, modules)?;
            }
        }
        match node.operation {
            None => w.u8(0),
            Some(operation) => {
                w.u8(1);
                C::write_operator(&mut w, operation.operator)?;
                match operation.operand {
                    None => w.u8(0),
                    Some(operand) => {
                        w.u8(1);
                        w.u64(operand.index as u64);
                    }
                }
            }
        }
        match node.equality.parent() {
            None => w.u8(0),
            Some(parent) => {
                w.u8(1);
                w.u64(parent.index as u64);
            }
        }
        match node.equality.next() {
            None => w.u8(0),
            Some(next) => {
                w.u8(1);
                w.u64(next.index as u64);
            }
        }
        match node.equality.tail() {
            None => w.u8(0),
            Some(tail) => {
                w.u8(1);
                w.u64(tail.index as u64);
            }
        }
        w.u32(node.equality.size());
        // Store the two axes rather than the collapsed `undecided` read: the
        // materialize pass's carry rule needs them apart.
        w.u8(node.runned as u8);
        match node.evaluated_deep {
            None => w.u8(0),
            Some(deep) => {
                w.u8(1);
                w.u8(deep.undecided as u8);
            }
        }
        write_low_shape_opt(&mut w, &node.low_shape);
    }
    w.u64(module.functions.len() as u64);
    for function in &module.functions {
        w.u64(function.parameter.index as u64);
        w.u64(function.r#return.index as u64);
        w.u64(function.return_type.index as u64);
        match function.return_marker {
            None => w.u8(0),
            Some(index) => {
                w.u8(1);
                w.u32(index);
            }
        }
        match function.return_field {
            None => w.u8(0),
            Some(index) => {
                w.u8(1);
                w.u32(index);
            }
        }

        // The re-export origin: a raw module key + function index.
        match function.origin {
            None => w.u8(0),
            Some(origin) => {
                w.u8(1);
                w.u64(origin.module.as_raw());
                w.u64(origin.index.0 as u64);
            }
        }
        w.u64(function.asserts.len() as u64);
        for &assert in &function.asserts {
            w.u64(assert.index as u64);
        }
        w.u64(function.nodes.len() as u64);
        for &node in &function.nodes {
            w.u64(node.index as u64);
        }
    }
    // The registry, in id order: a layout id indexes it, so a reordering
    // would re-read every frozen array's attributes.
    w.u64(module.attribute_layouts.len() as u64);
    for layout in &module.attribute_layouts {
        w.u64(layout.len() as u64);
        for &canonical in layout {
            w.u32(canonical);
        }
    }
    // The module's default attribute array, when it declares one.
    match &module.default_attributes {
        None => w.u8(0),
        Some(handle) => {
            w.u8(1);
            write_relocated_handle(&mut w, modules, handle.module, handle.items_ptr());
        }
    }
    let body = w.finish()?;

    let mut w = Writer::new();
    w.bytes(b"LCHN");
    w.u32(ARTIFACT_FORMAT_VERSION);
    w.u64(module.key.as_raw());
    w.bytes(&hash);
    w.u64(arena_align::<P>() as u64);
    w.bytes(&sha256(&body));
    w.bytes(&body);
    Ok(w.into_bytes())
}

/// The compatibility contract: a shape's tag never changes, and an unstated
/// position is tag `0`.
fn write_low_shape(w: &mut Writer, shape: &Option<LowShape>) {
    match shape {
        None => w.u8(0),
        Some(LowShape::USize) => w.u8(1),
        Some(LowShape::Tuple(items)) => {
            w.u8(2);
            w.u64(items.len() as u64);
            for item in items {
                write_low_shape(w, item);
            }
        }
        Some(LowShape::Array(elem, len)) => {
            w.u8(3);
            write_low_shape(w, elem);
            w.u64(*len as u64);
        }
        Some(LowShape::Function(param, result)) => {
            w.u8(4);
            write_low_shape(w, param);
            write_low_shape(w, result);
        }
        Some(LowShape::Table(key, value)) => {
            w.u8(5);
            write_low_shape(w, key);
            write_low_shape(w, value);
        }
        Some(LowShape::Float) => w.u8(6),
    }
}

/// Write a node's stored low type: exactly the shape form above.
fn write_low_shape_opt(w: &mut Writer, shape: &Option<LowShape>) {
    write_low_shape(w, shape);
}

fn read_low_shape_opt(r: &mut Reader<'_>) -> Result<Option<LowShape>, String> {
    read_low_shape(r, 1)
}

/// The deepest [`LowShape`] nesting a node's shape marker may declare.
///
/// # Invariant
///
/// The wire form spends one byte per level, so the cap bounds hostile input;
/// a real shape's depth is bounded by the program's own type nesting.
const MAX_LOW_SHAPE_DEPTH: usize = 256;

/// Read one shape marker, `depth` levels below the node that carries it.
///
/// # Invariant
///
/// The cap is checked before the match, so the recursion cannot outgrow the
/// native stack.
fn read_low_shape(r: &mut Reader<'_>, depth: usize) -> Result<Option<LowShape>, String> {
    if depth > MAX_LOW_SHAPE_DEPTH {
        return Err(format!(
            "artifact low_shape nests deeper than the {MAX_LOW_SHAPE_DEPTH}-level cap"
        ));
    }
    match r.u8()? {
        0 => Ok(None),
        1 => Ok(Some(LowShape::USize)),
        2 => {
            let len = r.u64()? as usize;
            let mut items = reserve(r, len, "low_shape tuple elements")?;
            for _ in 0..len {
                items.push(read_low_shape(r, depth + 1)?);
            }
            Ok(Some(LowShape::Tuple(items)))
        }
        3 => {
            let elem = Box::new(read_low_shape(r, depth + 1)?);
            let len = r.u64()? as usize;
            Ok(Some(LowShape::Array(elem, len)))
        }
        4 => Ok(Some(LowShape::Function(
            Box::new(read_low_shape(r, depth + 1)?),
            Box::new(read_low_shape(r, depth + 1)?),
        ))),
        5 => Ok(Some(LowShape::Table(
            Box::new(read_low_shape(r, depth + 1)?),
            Box::new(read_low_shape(r, depth + 1)?),
        ))),
        6 => Ok(Some(LowShape::Float)),
        _ => Err("bad low_shape tag".into()),
    }
}

/// Reserve a vector for a list whose `count` was read out of the stream.
///
/// # Invariant
///
/// Every element of a length-prefixed list costs at least one byte, so a
/// `count` past the reader's remaining bytes is refused rather than trusted —
/// a 64-byte file must not request a 2^60-element allocation.
fn reserve<T>(r: &Reader<'_>, count: usize, what: &str) -> Result<Vec<T>, String> {
    let remaining = r.remaining();
    if count > remaining {
        return Err(format!(
            "artifact declares {count} {what}, more than the {remaining} bytes it could encode"
        ));
    }
    Ok(Vec::with_capacity(count))
}

/// Reject a node index the module being loaded does not have.
///
/// # Invariant
///
/// Every node id is an index into the module's node list; accepted on faith, an
/// out-of-range one panics much later, far from the artifact that caused it.
fn check_node_index(index: usize, node_count: usize, what: &str) -> Result<(), String> {
    if index >= node_count {
        return Err(format!(
            "artifact {what} names node index {index}, which is not among the module's {node_count} nodes"
        ));
    }
    Ok(())
}

/// Read one node index and validate it against the module's node count.
fn read_node_id(r: &mut Reader<'_>, node_count: usize, what: &str) -> Result<LocalNodeId, String> {
    let index = r.u64()? as usize;
    check_node_index(index, node_count, what)?;
    Ok(LocalNodeId { index })
}

/// Deserialize an artifact, verifying `key` and `hash` against the header.
///
/// # Invariant
///
/// `modules` supplies the already-registered dependency arenas, and the header's
/// body digest is verified before any body field is read.
pub fn deserialize_artifact(
    bytes: &[u8],
    key: ModuleKey,
    hash: Hash,
    modules: &HashMap<ModuleKey, Arc<StaticModule<LangProgram>>>,
) -> Result<(StaticModule<LangProgram>, LocalNodeId), String> {
    deserialize_artifact_with(bytes, key, hash, modules, ProgramCodec)
}

/// [`deserialize_artifact`] with an explicit [`ArtifactCodec`].
pub fn deserialize_artifact_with<P, C>(
    bytes: &[u8],
    key: ModuleKey,
    hash: Hash,
    modules: &HashMap<ModuleKey, Arc<StaticModule<P>>>,
    _codec: C,
) -> Result<(StaticModule<P>, LocalNodeId), String>
where
    P: Program,
    C: ArtifactCodec<P>,
{
    let mut r = Reader::new(bytes);
    if r.take(4)? != b"LCHN" {
        return Err("bad artifact magic".into());
    }
    if r.u32()? != ARTIFACT_FORMAT_VERSION {
        return Err("unknown artifact format version".into());
    }
    if ModuleKey::from_raw(r.u64()?) != key {
        return Err("artifact key does not match its file".into());
    }
    if r.take(32)? != hash {
        return Err("artifact hash does not match its file".into());
    }
    let max_align = r.u64()? as usize;
    if max_align != arena_align::<P>() {
        return Err("artifact payload alignment mismatch".into());
    }
    let expected_digest: Hash = r.take(32)?.try_into().expect("32 bytes");
    // The digest covers exactly the bytes after the header and is checked
    // before the first body field is read.
    let body_start = r.position();
    if sha256(&bytes[body_start..]) != expected_digest {
        return Err("artifact body digest does not match the file".into());
    }
    let export = LocalNodeId {
        index: r.u64()? as usize,
    };
    let arena_len = r.u64()? as usize;
    let arena = r.take(arena_len)?.to_vec();
    let base = arena_base::<P>(&arena);

    let node_count = r.u64()? as usize;
    // The export index is read before the count it names, so it is checked here.
    check_node_index(export.index, node_count, "export")?;
    let mut nodes: Vec<StaticNode<P>> = reserve(&r, node_count, "nodes")?;
    for _ in 0..node_count {
        let value = if r.u8()? != 0 {
            Some(C::read_value(&mut r, key, &arena, base, modules)?)
        } else {
            None
        };
        let operation = if r.u8()? != 0 {
            let operator = C::read_operator(&mut r)?;
            let operand = if r.u8()? != 0 {
                Some(read_node_id(&mut r, node_count, "node operation operand")?)
            } else {
                None
            };
            Some(StaticOperation { operator, operand })
        } else {
            None
        };
        let parent = if r.u8()? != 0 {
            Some(read_node_id(&mut r, node_count, "node equality parent")?)
        } else {
            None
        };
        let next = if r.u8()? != 0 {
            Some(read_node_id(&mut r, node_count, "node equality next")?)
        } else {
            None
        };
        let tail = if r.u8()? != 0 {
            Some(read_node_id(&mut r, node_count, "node equality tail")?)
        } else {
            None
        };
        let size = r.u32()?;
        let runned = r.u8()? != 0;
        let evaluated_deep = match r.u8()? {
            0 => None,
            1 => Some(EvaluatedDeep {
                undecided: r.u8()? != 0,
            }),
            _ => return Err("bad evaluated_deep option tag".into()),
        };
        let low_shape = read_low_shape_opt(&mut r)?;
        nodes.push(StaticNode {
            value,
            operation,
            low_shape,
            equality: lichen_utils::disjoint::Meta::new(parent, next, tail, size),
            runned,
            evaluated_deep,
        });
    }

    let function_count = r.u64()? as usize;
    let mut functions: Vec<StaticFunction> = reserve(&r, function_count, "functions")?;
    for _ in 0..function_count {
        let parameter = read_node_id(&mut r, node_count, "function parameter")?;
        let r#return = read_node_id(&mut r, node_count, "function return")?;
        let return_type = read_node_id(&mut r, node_count, "function return type")?;
        let return_marker = match r.u8()? {
            0 => None,
            1 => Some(r.u32()?),
            _ => return Err("bad function return marker tag".into()),
        };
        let return_field = match r.u8()? {
            0 => None,
            1 => Some(r.u32()?),
            _ => return Err("bad function return field tag".into()),
        };
        let origin = match r.u8()? {
            0 => None,
            1 => Some(StaticFunctionRef {
                module: ModuleKey::from_raw(r.u64()?),
                index: StaticFunctionId(r.u64()? as usize),
            }),
            _ => return Err("bad function origin tag".into()),
        };
        let assert_count = r.u64()? as usize;
        let mut asserts = reserve(&r, assert_count, "function assert entries")?;
        for _ in 0..assert_count {
            asserts.push(read_node_id(
                &mut r,
                node_count,
                "function assert condition",
            )?);
        }
        let scope_count = r.u64()? as usize;
        let mut scope = reserve(&r, scope_count, "function template nodes")?;
        for _ in 0..scope_count {
            scope.push(read_node_id(&mut r, node_count, "function template node")?);
        }
        functions.push(StaticFunction {
            parameter,
            r#return,
            return_type,
            return_marker,
            return_field,
            origin,
            asserts,
            nodes: scope,
            // Not serialized: recomputed from the loaded tables, so an artifact
            // cannot disagree with its graph.
            open_captures: false,
        });
    }

    let layout_count = r.u64()? as usize;
    let mut attribute_layouts = reserve(&r, layout_count, "attribute layouts")?;
    for _ in 0..layout_count {
        let entry_len = r.u64()? as usize;
        let mut entry = reserve(&r, entry_len, "attribute layout indices")?;
        for _ in 0..entry_len {
            entry.push(r.u32()?);
        }
        attribute_layouts.push(entry.into());
    }
    let default_attributes = match r.u8()? {
        0 => None,
        1 => Some(read_relocated_handle::<ArrayItem, P>(
            &mut r, key, &arena, base, modules,
        )?),
        _ => return Err("bad default attribute array tag".into()),
    };
    if !r.done() {
        return Err("trailing bytes after the artifact".into());
    }
    let mut module = StaticModule {
        key,
        nodes,
        functions,
        arena,
        attribute_layouts,
        default_attributes,
        // No mode on the wire yet: a loaded artifact is content-portable.
        read_only: false,
        // A loaded artifact owns no out-of-arena resource: a resource handle
        // is process-local and cannot be in the bytes.
        releases: Vec::new(),
    };
    // Recompute the verdict from the loaded tables, as the freeze's walk does.
    module.fill_open_captures();
    Ok((module, export))
}
