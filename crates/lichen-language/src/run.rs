//! Running a source program to its output value.
//!
//! [`evaluate`] compiles and checks a program, runs it (the deep evaluation
//! of its root value), and renders the result as text — the program's
//! output, with its type: `5: Int`, `[1, 2, 3]: Int<3>`.  Diagnostics are
//! returned unrendered so the caller (the CLI, the example tests) can render
//! them with carets.

use std::collections::HashMap;

use lichen_highlevel::program::{HighProgram, HighValue};
use lichen_lowlevel::{Module, NodeId, Value};

use crate::compile;
use crate::diag::Diag;

/// Compile, check, and run `source`; the rendered output value and its type.
///
/// On failure the diagnostics (frontend and checker) are returned.  Every
/// v1 program terminates, so the deep evaluation cannot hit the VM's
/// recursion guard.
pub fn evaluate(source: &str) -> Result<String, Vec<Diag>> {
    let report = compile(source);
    if !report.diagnostics.is_empty() {
        return Err(report.diagnostics);
    }
    let build = report.build.unwrap();
    let mut module = build.module;
    let value = module.evaluate_node_deep(build.root_val, None);
    module.evaluate_node_deep(build.root_ty, None);
    Ok(format!(
        "{}: {}",
        print_value(&module, value),
        print_type(&module, build.root_ty)
    ))
}

/// Render a runtime value as the program's output.
///
/// The runtime value is all the printer sees: arrays print with `[ ]`
/// whether the source wrote a tuple or an array (the same value at
/// runtime), functions print as `Function`, and the type constants by their
/// source spellings `Int` / `Type`.  A type pair `[head, [Type, ↺]]` renders
/// as its head (`[TypeInt, K]` → `Int`), and the self-looping universe as
/// `Type` — otherwise the recursive pair encoding would loop forever.
pub fn print_value(module: &Module<HighProgram>, value: Value<HighProgram>) -> String {
    print_inner(module, value, &mut Vec::new())
}

fn print_inner(
    module: &Module<HighProgram>,
    value: Value<HighProgram>,
    path: &mut Vec<NodeId>,
) -> String {
    match value {
        Value::USize(n) => n.to_string(),
        Value::Ext(HighValue::TypeInt) => "Int".to_string(),
        Value::Ext(HighValue::TypeType) => "Type".to_string(),
        Value::Ext(HighValue::TypeFunction) => "TypeFunction".to_string(),
        Value::Ext(HighValue::TypeTuple) => "TypeTuple".to_string(),
        Value::Ext(HighValue::TypeArray) => "TypeArray".to_string(),
        Value::Ext(HighValue::TypeId(n)) => format!("TypeId({n})"),
        Value::Array(ptr) => {
            let elements = unsafe { &*ptr };
            // A type pair `[head, K]`: the kind slot is the self-looping
            // universe, so render just the head (and cut the cycle).
            if elements.len() == 2 && is_universe(module, elements[1]) {
                return print_inner(
                    module,
                    module.nodes[elements[0]].value.unwrap_or(Value::None),
                    path,
                );
            }
            let mut out = Vec::new();
            for &id in elements {
                if path.contains(&id) {
                    out.push("…".to_string());
                } else {
                    path.push(id);
                    let element = module.nodes[id].value.unwrap_or(Value::None);
                    let text = print_inner(module, element, path);
                    path.pop();
                    out.push(text);
                }
            }
            format!("[{}]", out.join(", "))
        }
        Value::Function(_) => "Function".to_string(),
        Value::None => "none".to_string(),
        Value::Parameterized => "parameterized".to_string(),
    }
}

/// Render a type expression (the recursive-pair encoding again) in the
/// language's own type syntax: `Int`, `Type`, `T1 -> T2`, `<T1, ..., Tn>`,
/// `T<len>`, `struct<T1, ...>`.  Unbound cells get stable `?a`, `?b`, …
/// names — cells in the same unification class share a name — so the type
/// shows which parts are linked.  Cycles are cut at `…`.
pub fn print_type(module: &Module<HighProgram>, root: NodeId) -> String {
    TypePrinter {
        module,
        names: HashMap::new(),
        next: 0,
        path: Vec::new(),
    }
    .node(root)
}

struct TypePrinter<'a> {
    module: &'a Module<HighProgram>,
    /// Stable class names: representative → `?a`, `?b`, …, within one type.
    names: HashMap<NodeId, String>,
    next: usize,
    /// Array nodes on the current recursion; a cycle renders as `…`.
    path: Vec<NodeId>,
}

impl TypePrinter<'_> {
    /// Render a type node; an unbound cell renders as its class name.
    fn node(&mut self, node: NodeId) -> String {
        if self.path.contains(&node) {
            return "…".to_string();
        }
        let value = self.module.nodes[node].value.unwrap_or(Value::None);
        if matches!(value, Value::None | Value::Parameterized) {
            return self.class_name(node);
        }
        self.path.push(node);
        let out = self.value(value);
        self.path.pop();
        out
    }

    /// Render a type value, descending into arrays.
    fn value(&mut self, value: Value<HighProgram>) -> String {
        match value {
            Value::USize(n) => n.to_string(),
            Value::Ext(HighValue::TypeInt) => "Int".to_string(),
            Value::Ext(HighValue::TypeType) => "Type".to_string(),
            Value::Ext(HighValue::TypeFunction) => "TypeFunction".to_string(),
            Value::Ext(HighValue::TypeTuple) => "TypeTuple".to_string(),
            Value::Ext(HighValue::TypeArray) => "TypeArray".to_string(),
            Value::Ext(HighValue::TypeId(n)) => format!("TypeId({n})"),
            Value::Function(_) => "Function".to_string(),
            Value::None | Value::Parameterized => unreachable!("handled by node()"),
            Value::Array(ptr) => self.elements(unsafe { &*ptr }),
        }
    }

    fn elements(&mut self, elements: &[NodeId]) -> String {
        // `[head, K]` — an atomic type: the kind slot is the self-looping
        // universe, so render the head (`int`, `Type`, …).
        if elements.len() == 2 && is_universe(self.module, elements[1]) {
            return self.node(elements[0]);
        }
        // `[shape, [marker, K]]` — a compound type: the kind's marker decides
        // how the shape reads.
        if elements.len() == 2
            && let Some(Value::Array(kind_ptr)) = self.module.nodes[elements[1]].value
            && let kind = unsafe { &*kind_ptr }
            && kind.len() == 2
            && is_universe(self.module, kind[1])
        {
            match self.module.nodes[kind[0]].value {
                Some(Value::Ext(HighValue::TypeFunction)) => {
                    // shape = [in, out] — render `in -> out`.
                    if let Some(Value::Array(shape_ptr)) = self.module.nodes[elements[0]].value
                        && let s = unsafe { &*shape_ptr }
                        && s.len() == 2
                    {
                        return format!("{} -> {}", self.node(s[0]), self.node(s[1]));
                    }
                }
                Some(Value::Ext(HighValue::TypeTuple)) => {
                    // shape = the field-type list — render `<T1, ..., Tn>`.
                    let fields = self.fields(elements[0]);
                    return format!("<{}>", fields.join(", "));
                }
                Some(Value::Ext(HighValue::TypeArray)) => {
                    // shape = [element type, length] — render `T<len>`.
                    if let Some(Value::Array(shape_ptr)) = self.module.nodes[elements[0]].value
                        && let s = unsafe { &*shape_ptr }
                        && s.len() == 2
                    {
                        return format!("{}<{}>", self.node(s[0]), self.node(s[1]));
                    }
                }
                Some(Value::Ext(HighValue::TypeId(_))) => {
                    // A struct type: shape = the field-type list — render
                    // `struct<T1, ..., Tn>`.
                    let fields = self.fields(elements[0]);
                    return format!("struct<{}>", fields.join(", "));
                }
                _ => {}
            }
        }
        // Fallback: render the raw elements.
        let parts: Vec<String> = elements.iter().map(|&e| self.node(e)).collect();
        format!("[{}]", parts.join(", "))
    }

    /// The field-type list of a compound type: the shape is the list itself,
    /// or a single field for a non-array shape.
    fn fields(&mut self, shape: NodeId) -> Vec<String> {
        match self.module.nodes[shape].value {
            Some(Value::Array(ptr)) => unsafe { &*ptr }.iter().map(|&f| self.node(f)).collect(),
            _ => vec![self.node(shape)],
        }
    }

    /// The stable name of an unbound cell's class: `?a`, `?b`, … — cells in
    /// the same class share a name.
    fn class_name(&mut self, node: NodeId) -> String {
        let rep = representative(self.module, node);
        if let Some(name) = self.names.get(&rep) {
            return name.clone();
        }
        let name = letter_name(self.next);
        self.next += 1;
        self.names.insert(rep, name.clone());
        name
    }
}

/// The class representative of `node`, via a read-only `parent` walk (the
/// printer never mutates the module).
fn representative(module: &Module<HighProgram>, node: NodeId) -> NodeId {
    let mut n = node;
    while let Some(parent) = module.nodes[n].equality.parent {
        n = parent;
    }
    n
}

/// `0 → "?a"`, `1 → "?b"`, …, `26 → "?a1"`, `27 → "?b1"`, …
fn letter_name(i: usize) -> String {
    let letter = (b'a' + (i % 26) as u8) as char;
    let round = i / 26;
    if round == 0 {
        format!("?{letter}")
    } else {
        format!("?{letter}{round}")
    }
}

/// The canonical universe `K = [Type, ↺]` — a node whose value is an array
/// that contains the node itself.
fn is_universe(module: &Module<HighProgram>, node: NodeId) -> bool {
    matches!(module.nodes[node].value, Some(Value::Array(ptr)) if unsafe { &*ptr }.contains(&node))
}
