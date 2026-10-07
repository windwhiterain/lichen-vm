//! The artifact container: the writer, the reader, and the shape encoders they
//! share.  The byte format itself is documented on the parent module.

use super::*;

use std::collections::HashMap;
use std::sync::Arc;

use lichen_lowlevel::{
    LocalNodeId, LowShape, Program, StaticFunction, StaticFunctionId, StaticFunctionRef,
    StaticModule, StaticNode, StaticOperation,
};

use crate::program::{LangProgram, ProgramCodec};

/// Serialize `module` (and the arenas its refs point into, via `modules`)
/// into the portable artifact format.  `hash` and `export` are the package
/// metadata the store records alongside the module data.
///
/// Fallible only for the vocabulary's own encodability: a leaf name the
/// one-byte discriminator cannot hold is refused (`Writer::leaf`) rather than
/// truncated.
pub fn serialize_artifact(
    module: &StaticModule<LangProgram>,
    modules: &HashMap<ModuleKey, Arc<StaticModule<LangProgram>>>,
    hash: Hash,
    export: LocalNodeId,
) -> Result<Vec<u8>, String> {
    serialize_artifact_with(module, modules, hash, export, ProgramCodec)
}

/// [`Self::serialize_artifact`] with an explicit [`ArtifactCodec`], for
/// downstream vocabularies that need custom value/operator tags.
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
    // The body is assembled first, because its digest goes in the header
    // ahead of it — there is no seeking backwards in a `Writer`.
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
        w.u8(node.undecided as u8);
        write_low_shape_opt(&mut w, &node.low_shape);
    }
    w.u64(module.functions.len() as u64);
    for function in &module.functions {
        w.u64(function.parameter.index as u64);
        w.u64(function.r#return.index as u64);
        w.u64(function.return_type.index as u64);
        // The re-export origin — the real original this function is a copy of,
        // when it is one.  A raw module key + function index, absolute like the
        // static refs the values carry.
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

/// Write an optional [`LowShape`] (the node's stored low type).
fn write_low_shape_opt(w: &mut Writer, shape: &Option<LowShape>) {
    match shape {
        None => w.u8(0),
        Some(shape) => {
            w.u8(1);
            write_low_shape(w, shape);
        }
    }
}

/// The tags are the compatibility contract with already-persisted artifacts:
/// `0`–`4` are the decided shapes and never change, and a new shape takes the
/// next unused tag (`5` is `Unknown`, the lattice's bottom; `6` is `Float`, a
/// decided shape added after the tag space was fixed).
fn write_low_shape(w: &mut Writer, shape: &LowShape) {
    match shape {
        LowShape::USize => w.u8(0),
        LowShape::Tuple(items) => {
            w.u8(1);
            w.u64(items.len() as u64);
            for item in items {
                write_low_shape(w, item);
            }
        }
        LowShape::Array(elem, len) => {
            w.u8(2);
            write_low_shape(w, elem);
            w.u64(*len as u64);
        }
        LowShape::Function(param, result) => {
            w.u8(3);
            write_low_shape(w, param);
            write_low_shape(w, result);
        }
        LowShape::Table(key, value) => {
            w.u8(4);
            write_low_shape(w, key);
            write_low_shape(w, value);
        }
        LowShape::Unknown => w.u8(5),
        LowShape::Float => w.u8(6),
    }
}

fn read_low_shape_opt(r: &mut Reader<'_>) -> Result<Option<LowShape>, String> {
    match r.u8()? {
        0 => Ok(None),
        1 => Ok(Some(read_low_shape(r, 1)?)),
        _ => Err("bad low_shape option tag".into()),
    }
}

/// The deepest [`LowShape`] nesting a node's shape marker may declare.
///
/// The wire form spends one byte per level, so without a cap a megabyte of
/// crafted bytes is a million frames of native stack.  A real shape's depth is
/// bounded by the nesting of the program's own types — the compute layer builds
/// one from a kernel's domain shape, and a tuple shape is its element types
/// recursed — so this cap exists to bound hostile input, not to limit programs.
const MAX_LOW_SHAPE_DEPTH: usize = 256;

/// Read one shape marker, `depth` levels below the node that carries it.
///
/// Every nesting variant passes `depth + 1`, and the cap is checked before the
/// match, so the recursion cannot outgrow the native stack.
fn read_low_shape(r: &mut Reader<'_>, depth: usize) -> Result<LowShape, String> {
    if depth > MAX_LOW_SHAPE_DEPTH {
        return Err(format!(
            "artifact low_shape nests deeper than the {MAX_LOW_SHAPE_DEPTH}-level cap"
        ));
    }
    match r.u8()? {
        0 => Ok(LowShape::USize),
        1 => {
            let len = r.u64()? as usize;
            let mut items = reserve(r, len, "low_shape tuple elements")?;
            for _ in 0..len {
                items.push(read_low_shape(r, depth + 1)?);
            }
            Ok(LowShape::Tuple(items))
        }
        2 => {
            let elem = Box::new(read_low_shape(r, depth + 1)?);
            let len = r.u64()? as usize;
            Ok(LowShape::Array(elem, len))
        }
        3 => Ok(LowShape::Function(
            Box::new(read_low_shape(r, depth + 1)?),
            Box::new(read_low_shape(r, depth + 1)?),
        )),
        4 => Ok(LowShape::Table(
            Box::new(read_low_shape(r, depth + 1)?),
            Box::new(read_low_shape(r, depth + 1)?),
        )),
        5 => Ok(LowShape::Unknown),
        6 => Ok(LowShape::Float),
        _ => Err("bad low_shape tag".into()),
    }
}

/// Reserve a vector for a list whose `count` was read out of the stream.
///
/// **Contract:** every element of a length-prefixed list costs at least one
/// byte on the wire, so a `count` past the reader's remaining byte count is
/// impossible for any artifact a writer produced.  Preallocating from `count`
/// on faith is what lets a 64-byte file request a 2^60-element allocation and
/// abort the process; the preallocation itself is still wanted, because for a
/// real artifact `count` is the list's exact size.
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
/// Every node id in the body is an index into that module's node list, so an
/// index at or past the declared count names a node that does not exist:
/// accepted on faith it loads "successfully" and panics much later, far from
/// the artifact that caused it.
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

/// Deserialize an artifact.  `key` and `hash` are the expected identity of
/// the file (verified against the header); `modules` supplies the arenas of
/// the artifact's dependencies, which must already be registered — foreign
/// refs resolve through their keys, absolute from birth.  The header's body
/// digest is verified before any body field is read.  Returns the module and
/// the exported root's local index.
pub fn deserialize_artifact(
    bytes: &[u8],
    key: ModuleKey,
    hash: Hash,
    modules: &HashMap<ModuleKey, Arc<StaticModule<LangProgram>>>,
) -> Result<(StaticModule<LangProgram>, LocalNodeId), String> {
    deserialize_artifact_with(bytes, key, hash, modules, ProgramCodec)
}

/// [`Self::deserialize_artifact`] with an explicit [`ArtifactCodec`], for
/// downstream vocabularies that need custom value/operator tags.
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
    // The header ends here; the digest covers exactly the bytes after it.  It
    // is checked before the first body field is read, so a corrupted body is
    // rejected here rather than loaded and misinterpreted field by field.
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
        let undecided = r.u8()? != 0;
        let low_shape = read_low_shape_opt(&mut r)?;
        nodes.push(StaticNode {
            value,
            operation,
            low_shape,
            equality: lichen_utils::disjoint::Meta::new(parent, next, tail, size),
            undecided,
        });
    }

    let function_count = r.u64()? as usize;
    let mut functions: Vec<StaticFunction> = reserve(&r, function_count, "functions")?;
    for _ in 0..function_count {
        let parameter = read_node_id(&mut r, node_count, "function parameter")?;
        let r#return = read_node_id(&mut r, node_count, "function return")?;
        let return_type = read_node_id(&mut r, node_count, "function return type")?;
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
            origin,
            asserts,
            nodes: scope,
            // Filled below, from the loaded tables: the open-capture verdict is
            // not serialized — it is a function of the graph, recomputed from
            // the same walk the freeze runs, so an artifact cannot carry a
            // verdict that disagrees with the graph it ships.
            open_captures: false,
        });
    }
    if !r.done() {
        return Err("trailing bytes after the artifact".into());
    }
    let mut module = StaticModule {
        key,
        nodes,
        functions,
        arena,
        // A loaded artifact owns no out-of-arena resource: a resource handle
        // is process-local and cannot be in the bytes.
        releases: Vec::new(),
    };
    // The open-capture verdict, recomputed from the loaded tables — the
    // freeze's walk over the same two tables (see `StaticFunction::open_captures`).
    module.fill_open_captures();
    Ok((module, export))
}
