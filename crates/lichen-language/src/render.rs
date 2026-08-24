//! Rendering: the program's output and the diagnostics share one printer.
//!
//! The CLI output and the diagnostics are rendered as equal as possible:
//! [`print_value`] renders the program's result value *read against its type
//! chain* — the type decides how the value reads, so a struct type value
//! prints `struct<Int, Type>` (not the raw `[TypeId(0), Int]` pair), a tuple
//! value `(1, Int)` (not `[1, Int]`), and an array `[1, 2, 3]` — and
//! [`print_type`] renders the result type in the language's own type syntax.
//! A checker diagnostic's message ([`checker_message`]) re-renders the
//! highlevel's raw facts — the [`DiagKind`], the conflict classes, the diary
//! — with the *same* [`TypePrinter`], so a failed `5 : Int -> Int` reports
//! `expected Int -> Int, found Int` in the language's own type syntax, not
//! the raw `TypeInt → TypeInt`.  The highlevel crate stays raw; this module
//! is the pretty view.  The caret shell [`render`] wraps either message:
//!
//! ```text
//! error: unresolved name 'y'
//!   --> 1:6
//!    |
//!  1 | x => y
//!    |      ^
//! ```
//!
//! Both printers are generic over the value vocabulary ([`ValueType`]), so an
//! *extended* union — a crate that re-splices `HighProgramValue` with its own
//! variants via `extend_HighProgramValue!` — renders with the same machinery;
//! the extension's own variants render through the hook the printers carry
//! ([`TypePrinter::new_with_ext`], [`ValuePrinter::new_with_ext`]).

use std::collections::{HashMap, HashSet};

use lichen_highlevel::checker::Build;
use lichen_highlevel::diagnostic::{Diag as CheckerDiag, DiagKind, DiaryEntry};
use lichen_highlevel::ir::Span;
use lichen_highlevel::program::{HighProgram, HighProgramValue, ValueType};
use lichen_lowlevel::{LowValue, Module, NodeId, is_unbound};
use lichen_utils::disjoint;

use crate::diag::Diag;

/// Render a runtime value as the program's output, *read against its type*.
///
/// The type chain decides how the value reads: a value whose type is the
/// universe is an atomic type constant (`Int` / `Type`), a value whose type
/// is a kind is a compound type (`struct<Int, Type>`, `Int -> Int`,
/// `<Int, Type>`, `Int<3>`), a value whose type is a tuple type reads as a
/// tuple `(1, Int)`, an array type as an array `[1, 2, 3]`, and a struct
/// type as its field tuple.  When the type chain is opaque (an unbound cell,
/// an extension type), the value falls back to its raw layout — the old
/// `[head, K]` reading of a recursive pair.
pub fn print_value<V: ValueType>(
    module: &Module<HighProgram<V>>,
    value: V,
    ty: NodeId,
) -> String {
    ValuePrinter::new(module).print(value, ty)
}

/// Render a type expression (the recursive-pair encoding again) in the
/// language's own type syntax: `Int`, `Type`, `T1 -> T2`, `<T1, ..., Tn>`,
/// `T<len>`, `struct<T1, ...>`.  Unbound cells get stable `?a`, `?b`, …
/// names — cells in the same unification class share a name — so the type
/// shows which parts are linked.  Cycles are cut at `…`.
pub fn print_type<V: ValueType>(module: &Module<HighProgram<V>>, root: NodeId) -> String {
    TypePrinter::new(module).node(root)
}

/// The shared pretty type printer: stateful across calls, so one instance
/// renders a whole diagnostic (or report) with consistent `?a`/`?b` class
/// names.  Generic over the value vocabulary: the lowlevel structural values
/// render through [`AsEnum`], the type constants through [`ValueType`], and
/// an extension's own variants through the render hook.
pub struct TypePrinter<'a, V: ValueType = HighProgramValue> {
    module: &'a Module<HighProgram<V>>,
    /// The checker's arrow-shape registry, when rendering diagnostics: a
    /// class whose representative is a bare `[in, out]` shape (no kind
    /// wrapper) renders as an arrow only if a member is registered here.
    /// The CLI output path has no registry and leaves such shapes raw.
    arrows: Option<&'a HashSet<NodeId>>,
    /// Stable class names: representative → `?a`, `?b`, …, within one type
    /// (or one diagnostic report).
    names: HashMap<NodeId, String>,
    next: usize,
    /// Array nodes on the current recursion; a cycle renders as `…`.
    path: Vec<NodeId>,
    /// The extension's own value variants — how a variant the base
    /// vocabulary does not know renders.  `None` (the base vocabulary) or a
    /// hook returning `None` for a value leaves it `?`.
    render_ext: Option<&'a dyn Fn(&V) -> Option<String>>,
}

impl<'a, V: ValueType> TypePrinter<'a, V> {
    pub fn new(module: &'a Module<HighProgram<V>>) -> Self {
        Self::new_with(module, None, None)
    }

    /// A printer that also knows the checker's arrow shapes, so bare
    /// `[in, out]` shapes render as `in -> out`.
    pub fn new_with_arrows(
        module: &'a Module<HighProgram<V>>,
        arrows: Option<&'a HashSet<NodeId>>,
    ) -> Self {
        Self::new_with(module, arrows, None)
    }

    /// A printer that renders an extension vocabulary's own variants through
    /// `render_ext` — a hook the base renderer cannot know, returning the
    /// variant's spelling (or `None` for a value it does not recognize).
    pub fn new_with_ext(
        module: &'a Module<HighProgram<V>>,
        render_ext: Option<&'a dyn Fn(&V) -> Option<String>>,
    ) -> Self {
        Self::new_with(module, None, render_ext)
    }

    fn new_with(
        module: &'a Module<HighProgram<V>>,
        arrows: Option<&'a HashSet<NodeId>>,
        render_ext: Option<&'a dyn Fn(&V) -> Option<String>>,
    ) -> Self {
        TypePrinter {
            module,
            arrows,
            names: HashMap::new(),
            next: 0,
            path: Vec::new(),
            render_ext,
        }
    }

    /// Render a type node; an unbound cell renders as its class name.
    pub fn node(&mut self, node: NodeId) -> String {
        if self.path.contains(&node) {
            return "…".to_string();
        }
        let value = self.module.nodes[node]
            .value
            .unwrap_or_else(|| V::from(LowValue::None));
        if matches!(value.as_enum(), Some(LowValue::None | LowValue::Parameterized)) {
            return self.class_name(node);
        }
        self.path.push(node);
        let out = self.value(node, value);
        self.path.pop();
        out
    }

    /// The stable name of an unbound cell's class: `?a`, `?b`, … — cells in
    /// the same class share a name.
    pub fn class_name(&mut self, node: NodeId) -> String {
        let rep = representative(self.module, node);
        if let Some(name) = self.names.get(&rep) {
            return name.clone();
        }
        let name = letter_name(self.next);
        self.next += 1;
        self.names.insert(rep, name.clone());
        name
    }

    /// Render a type value, descending into arrays.
    fn value(&mut self, node: NodeId, value: V) -> String {
        if let Some(structural) = value.as_enum() {
            return match structural {
                LowValue::USize(n) => n.to_string(),
                LowValue::Array(array) => self.elements(node, array.ids()),
                LowValue::Function(_) => "Function".to_string(),
                LowValue::None | LowValue::Parameterized => {
                    unreachable!("handled by node()")
                }
            };
        }
        self.type_constant(&value).unwrap_or_else(|| "?".to_string())
    }

    /// The spelling of a type constant — or of an extension's own variant:
    /// the lowlevel structural values return `None`, they are rendered by
    /// [`Self::value`]'s structural branch.
    pub(crate) fn type_constant(&self, value: &V) -> Option<String> {
        if value == &V::int_marker() {
            Some("Int".to_string())
        } else if value == &V::type_marker() {
            Some("Type".to_string())
        } else if value == &V::function_type_marker() {
            Some("TypeFunction".to_string())
        } else if value == &V::tuple_type_marker() {
            Some("TypeTuple".to_string())
        } else if value == &V::array_type_marker() {
            Some("TypeArray".to_string())
        } else if value == &V::type_struct_marker() {
            Some("TypeStruct".to_string())
        } else if let Some(n) = value.type_id() {
            Some(format!("TypeId({n})"))
        } else if let Some(render_ext) = self.render_ext {
            render_ext(value)
        } else {
            None
        }
    }

    fn elements(&mut self, node: NodeId, elements: &[NodeId]) -> String {
        // `[head, K]` — an atomic type: the kind slot is the self-looping
        // universe, so render the head (`int`, `Type`, …).
        if elements.len() == 2 && is_universe(self.module, elements[1]) {
            return self.node(elements[0]);
        }
        // `[shape, [marker, K]]` — a compound type: the kind's marker decides
        // how the shape reads.
        if elements.len() == 2
            && let Some(kind) = self.module.nodes[elements[1]].value
            && let Some(LowValue::Array(kind)) = kind.as_enum()
            && let kind = kind.ids()
            && kind.len() == 2
            && is_universe(self.module, kind[1])
        {
            match self.module.nodes[kind[0]].value {
                Some(m) if m == V::function_type_marker() => {
                    // shape = [in, out] — render `in -> out`.
                    if let Some(shape) = self.module.nodes[elements[0]].value
                        && let Some(LowValue::Array(shape)) = shape.as_enum()
                        && let s = shape.ids()
                        && s.len() == 2
                    {
                        return format!("{} -> {}", self.node(s[0]), self.node(s[1]));
                    }
                }
                Some(m) if m == V::tuple_type_marker() => {
                    // shape = the field-type list — render `<T1, ..., Tn>`.
                    let fields = self.fields(elements[0]);
                    return format!("<{}>", fields.join(", "));
                }
                Some(m) if m == V::array_type_marker() => {
                    // shape = [element type, length] — render `T<len>`.
                    if let Some(shape) = self.module.nodes[elements[0]].value
                        && let Some(LowValue::Array(shape)) = shape.as_enum()
                        && let s = shape.ids()
                        && s.len() == 2
                    {
                        return format!("{}<{}>", self.node(s[0]), self.node(s[1]));
                    }
                }
                Some(m) if m == V::type_struct_marker() => {
                    // A struct type: shape = [TypeId, field-type list] —
                    // render `struct<T1, ..., Tn>` from the list at shape[1].
                    if let Some(shape) = self.module.nodes[elements[0]].value
                        && let Some(LowValue::Array(shape)) = shape.as_enum()
                        && let s = shape.ids()
                        && s.len() == 2
                    {
                        let fields = self.fields(s[1]);
                        return format!("struct<{}>", fields.join(", "));
                    }
                }
                _ => {}
            }
        }
        // A bare `[in, out]` shape with no kind wrapper: an arrow only when
        // the checker registered the shape (the diagnostic path); otherwise
        // it falls through to the raw pair.
        if elements.len() == 2 && self.is_arrow(node) {
            return format!("{} -> {}", self.node(elements[0]), self.node(elements[1]));
        }
        // Fallback: render the raw elements.
        let parts: Vec<String> = elements.iter().map(|&e| self.node(e)).collect();
        format!("[{}]", parts.join(", "))
    }

    /// Is `node`'s class a checker-registered arrow shape?
    fn is_arrow(&self, node: NodeId) -> bool {
        self.arrows.is_some_and(|arrows| {
            disjoint::members(&self.module.nodes, representative(self.module, node))
                .any(|m| arrows.contains(&m))
        })
    }

    /// The field-type list of a compound type: the shape is the list itself,
    /// or a single field for a non-array shape.
    pub(crate) fn fields(&mut self, shape: NodeId) -> Vec<String> {
        if let Some(LowValue::Array(array)) = self
            .module
            .nodes[shape]
            .value
            .and_then(|v| v.as_enum())
        {
            array.ids().iter().map(|&f| self.node(f)).collect()
        } else {
            vec![self.node(shape)]
        }
    }
}

/// The pretty value printer: renders a runtime value against its type chain,
/// so a value reads like the code that produced it.  Generic over the value
/// vocabulary, like [`TypePrinter`]; an extension's own variants render
/// through the same render hook.
pub struct ValuePrinter<'a, V: ValueType = HighProgramValue> {
    module: &'a Module<HighProgram<V>>,
    printer: TypePrinter<'a, V>,
    /// Value nodes on the current recursion; a cycle renders as `…`.
    path: Vec<NodeId>,
    /// Type nodes on the current recursion; a cycle renders as `…`.
    tpath: Vec<NodeId>,
}

impl<'a, V: ValueType> ValuePrinter<'a, V> {
    pub fn new(module: &'a Module<HighProgram<V>>) -> Self {
        Self::new_with_ext(module, None)
    }

    /// A printer that renders an extension vocabulary's own variants through
    /// `render_ext` — see [`TypePrinter::new_with_ext`].
    pub fn new_with_ext(
        module: &'a Module<HighProgram<V>>,
        render_ext: Option<&'a dyn Fn(&V) -> Option<String>>,
    ) -> Self {
        ValuePrinter {
            module,
            printer: TypePrinter::new_with_ext(module, render_ext),
            path: Vec::new(),
            tpath: Vec::new(),
        }
    }

    /// Render the runtime value `value`, whose type is `ty`.
    pub fn print(&mut self, value: V, ty: NodeId) -> String {
        self.value(value, ty)
    }

    /// Render a value against its type: the type chain decides how the value
    /// reads.  When the type chain is opaque, fall back to the raw layout.
    fn value(&mut self, value: V, ty: NodeId) -> String {
        // The universe as the type: the value is an atomic type constant —
        // `Int`, `Type`, or an extension's own type constant.
        if is_universe(self.module, ty) {
            return self.atomic(value);
        }
        let Some(LowValue::Array(ty_array)) = self.module.nodes[ty].value.and_then(|v| v.as_enum())
        else {
            return self.raw(value);
        };
        let tys = ty_array.ids();
        // A kind `[marker, K]`: the value is a compound type — render its
        // shape in type syntax.
        if tys.len() == 2 && is_universe(self.module, tys[1])
            && let Some(out) = self.compound_type(value, tys[0])
        {
            return out;
        }
        // A term of a tuple/array/struct type `[shape, [marker, K]]`: the
        // value's elements read against the shape.
        if tys.len() == 2
            && let Some(kind) = self.module.nodes[tys[1]].value
            && let Some(LowValue::Array(kind)) = kind.as_enum()
            && let kind = kind.ids()
            && kind.len() == 2
            && is_universe(self.module, kind[1])
            && let Some(marker) = self.module.nodes[kind[0]].value
            && let Some(out) = self.instance(value, tys[0], marker)
        {
            return out;
        }
        self.raw(value)
    }

    /// An atomic type constant: the value of a type expression whose type is
    /// the universe.  A structural value typed by the universe (the universe
    /// node itself) falls back to the raw layout.
    fn atomic(&mut self, value: V) -> String {
        if let Some(spelling) = self.printer.type_constant(&value) {
            spelling
        } else {
            self.raw(value)
        }
    }

    /// A compound type value: the value is the shape `[in, out]` /
    /// element list / `[element, length]` / `[TypeId, fields]`, and the
    /// marker decides how the shape reads.  `None` when the value or the
    /// marker does not fit a compound type.
    fn compound_type(&mut self, value: V, marker_node: NodeId) -> Option<String> {
        let marker = self.module.nodes[marker_node].value?;
        let Some(LowValue::Array(shape)) = value.as_enum() else {
            return None;
        };
        let shape = shape.ids();
        if marker == V::function_type_marker() {
            if shape.len() == 2 {
                return Some(format!(
                    "{} -> {}",
                    self.printer.node(shape[0]),
                    self.printer.node(shape[1])
                ));
            }
        } else if marker == V::tuple_type_marker() {
            let fields: Vec<String> = shape.iter().map(|&s| self.printer.node(s)).collect();
            return Some(format!("<{}>", fields.join(", ")));
        } else if marker == V::array_type_marker() {
            if shape.len() == 2 {
                return Some(format!(
                    "{}<{}>",
                    self.printer.node(shape[0]),
                    self.printer.node(shape[1])
                ));
            }
        } else if marker == V::type_struct_marker() {
            if shape.len() == 2 {
                let fields = self.printer.fields(shape[1]);
                return Some(format!("struct<{}>", fields.join(", ")));
            }
        }
        None
    }

    /// A term of a tuple, array, or struct type: the value's elements read
    /// against the shape — a tuple reads `(v1, ..., vn)` (a single element
    /// `(v1,)`), an array `[v1, ..., vn]`, a struct instance its field tuple
    /// `(v1, ..., vn)`.  `None` when the value or the shape does not fit.
    fn instance(&mut self, value: V, shape_node: NodeId, marker: V) -> Option<String> {
        let Some(LowValue::Array(values)) = value.as_enum() else {
            return None;
        };
        let values = values.ids();
        let shape = self.module.nodes[shape_node]
            .value
            .and_then(|v| v.as_enum());
        let Some(LowValue::Array(shape)) = shape else {
            return None;
        };
        let shape = shape.ids();
        if marker == V::tuple_type_marker() {
            if shape.len() != values.len() {
                return None;
            }
            let mut out = Vec::with_capacity(values.len());
            for (i, &v) in values.iter().enumerate() {
                out.push(self.element(v, shape[i]));
            }
            return Some(self.parens(&out));
        }
        if marker == V::array_type_marker() {
            if shape.len() != 2 {
                return None;
            }
            let mut out = Vec::with_capacity(values.len());
            for &v in values {
                out.push(self.element(v, shape[0]));
            }
            return Some(format!("[{}]", out.join(", ")));
        }
        if marker == V::type_struct_marker() {
            // The shape is [TypeId, field-type list] — the element types are
            // the fields.
            if shape.len() != 2 {
                return None;
            }
            let Some(LowValue::Array(fields)) = self.module.nodes[shape[1]]
                .value
                .and_then(|v| v.as_enum())
            else {
                return None;
            };
            let fields = fields.ids();
            if fields.len() != values.len() {
                return None;
            }
            let mut out = Vec::with_capacity(values.len());
            for (i, &v) in values.iter().enumerate() {
                out.push(self.element(v, fields[i]));
            }
            return Some(self.parens(&out));
        }
        None
    }

    /// `(v1, ..., vn)` — a single element keeps its trailing comma, the
    /// source spelling of a one-tuple.
    fn parens(&self, elements: &[String]) -> String {
        let body = elements.join(", ");
        let body = if elements.len() == 1 {
            format!("{body},")
        } else {
            body
        };
        format!("({body})")
    }

    /// Render a value element against its type, cutting cycles on both the
    /// value and the type side.
    fn element(&mut self, id: NodeId, ty: NodeId) -> String {
        if self.path.contains(&id) || self.tpath.contains(&ty) {
            return "…".to_string();
        }
        self.path.push(id);
        self.tpath.push(ty);
        let value = self.module.nodes[id]
            .value
            .unwrap_or_else(|| V::from(LowValue::None));
        let out = self.value(value, ty);
        self.tpath.pop();
        self.path.pop();
        out
    }

    /// The raw value layout — the fallback when the type chain cannot guide
    /// the reading: a type pair `[head, [Type, ↺]]` renders as its head
    /// (`[TypeInt, K]` → `Int`), arrays `[ ]`, functions `Function`, and the
    /// type constants by their spellings `Int` / `Type`.
    fn raw(&mut self, value: V) -> String {
        if let Some(structural) = value.as_enum() {
            return match structural {
                LowValue::USize(n) => n.to_string(),
                LowValue::Function(_) => "Function".to_string(),
                LowValue::None => "none".to_string(),
                LowValue::Parameterized => "parameterized".to_string(),
                LowValue::Array(array) => {
                    let elements = array.ids();
                    // A type pair `[head, K]`: the kind slot is the
                    // self-looping universe, so render just the head (and cut
                    // the cycle).
                    if elements.len() == 2 && is_universe(self.module, elements[1]) {
                        let head = self.module.nodes[elements[0]]
                            .value
                            .unwrap_or_else(|| V::from(LowValue::None));
                        return self.raw(head);
                    }
                    let mut out = Vec::new();
                    for &id in elements {
                        if self.path.contains(&id) {
                            out.push("…".to_string());
                        } else {
                            self.path.push(id);
                            let element = self.module.nodes[id]
                                .value
                                .unwrap_or_else(|| V::from(LowValue::None));
                            let text = self.raw(element);
                            self.path.pop();
                            out.push(text);
                        }
                    }
                    format!("[{}]", out.join(", "))
                }
            };
        }
        self.printer
            .type_constant(&value)
            .unwrap_or_else(|| "?".to_string())
    }
}

/// The class representative of `node`, via a read-only `parent` walk (the
/// printers never mutate the module).
fn representative<V: ValueType>(module: &Module<HighProgram<V>>, node: NodeId) -> NodeId {
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
/// that contains a member of its own unification class.  A plain
/// self-referential member (`contains(&node)`) is the canonical node itself;
/// a cell unified into the universe class carries the replicated value, whose
/// self-referential member is the canonical node — the class check covers
/// both.
fn is_universe<V: ValueType>(module: &Module<HighProgram<V>>, node: NodeId) -> bool {
    let rep = representative(module, node);
    matches!(module.nodes[node].value, Some(value)
        if matches!(value.as_enum(), Some(LowValue::Array(array))
            if array.ids().iter().any(|&m| representative(module, m) == rep)))
}

// --- the caret shell ---------------------------------------------------------

/// Render a diagnostic with its source line and a caret.
///
/// ```text
/// error: unresolved name 'y'
///   --> 1:6
///    |
///  1 | x => y
///    |      ^
/// ```
pub fn render(source: &str, diag: &Diag) -> String {
    let mut out = format!("error: {}\n", diag.message);
    if let Some((line, col)) = diag.span {
        out.push_str(&format!("  --> {line}:{col}\n"));
        out.push_str("   |\n");
        if let Some(text) = source.lines().nth((line as usize).saturating_sub(1)) {
            let caret = format!("{}^", " ".repeat((col as usize).saturating_sub(1)));
            out.push_str(&format!(" {line} | {text}\n"));
            out.push_str(&format!("   | {caret}\n"));
        }
    }
    out
}

/// Render a whole diagnostic list back to back, exactly as the CLI prints
/// them: one caret block per diagnostic, no separator.
pub fn render_all(source: &str, diags: &[Diag]) -> String {
    diags.iter().map(|d| render(source, d)).collect()
}

// --- the pretty checker message ----------------------------------------------
// Re-renders the highlevel's raw facts in the CLI's vocabulary: the wording
// per kind, then the `?a` flow lines.  One TypePrinter drives the whole
// message, so a class keeps a single `?a` name across the main line and the
// flow.

/// Re-render a checker diagnostic's message with the shared pretty printer,
/// mirroring the highlevel's raw rendering ([`liche_highlevel::diagnostic`])
/// line for line but in the language's own type syntax.  `printer` is shared
/// across a whole report, so a class keeps a single `?a` name across
/// diagnostics; it must carry the checker's arrow registry.  `node_spans` is
/// the raw node → span table (`Build::node_spans`), needed for the flow
/// lines' line numbers.
pub fn checker_message(
    build: &Build,
    node_spans: &HashMap<NodeId, Span>,
    printer: &mut TypePrinter,
    d: &CheckerDiag,
) -> String {
    // The owning diary entry: the last one whose error_index <= ours (one
    // unify may own a whole run of errors, e.g. elementwise).
    let entry = d
        .error_index
        .and_then(|i| build.diary.iter().rev().find(|e| e.error_index <= i));
    let mut message = match d.kind {
        DiagKind::Annotation | DiagKind::ArrayElement => format!(
            "expected {}, found {}",
            printer.node(d.b),
            printer.node(d.a)
        ),
        DiagKind::Guard => format!("expected a function, found {}", printer.node(d.a)),
        DiagKind::IndexTarget => {
            format!(
                "expected a tuple, array, or struct type, found {}",
                printer.node(d.a)
            )
        }
        DiagKind::BinOp => format!("expected Int, found {}", printer.node(d.a)),
        // A runtime apply-time failure: the parameter is the expected side,
        // the argument the found side.
        DiagKind::Runtime => format!(
            "expected {}, found {}",
            printer.node(d.a),
            printer.node(d.b)
        ),
        DiagKind::IndexOutOfBounds => {
            let (Some(HighProgramValue::USize(index)), Some(HighProgramValue::USize(length))) =
                (d.value_a, d.value_b)
            else {
                return "index out of bounds".to_string();
            };
            return format!("index {index} out of bounds (array length {length})");
        }
    };
    let flow = flow(build, node_spans, printer, entry, d);
    if !flow.is_empty() {
        message.push_str("\n  ");
        message.push_str(&flow.join("\n  "));
    }
    message
}

/// The `?a`-journey: which members fixed either side of the conflict, and to
/// what.  The conflicting classes (the markers where the merge failed) are
/// walked first; the diary-attributed top-level unified nodes are hunted too
/// — e.g. the expected side of `5 : Type` is the universe `K` itself.
fn flow(
    build: &Build,
    node_spans: &HashMap<NodeId, Span>,
    printer: &mut TypePrinter,
    entry: Option<&DiaryEntry>,
    d: &CheckerDiag,
) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let name_a = printer.class_name(d.a);
    let name_b = printer.class_name(d.b);
    flow_side(
        build, node_spans, printer, d.a, &name_a, &mut seen, &mut out,
    );
    flow_side(
        build, node_spans, printer, d.b, &name_b, &mut seen, &mut out,
    );
    if let Some(entry) = entry {
        flow_side(
            build, node_spans, printer, entry.a, &name_a, &mut seen, &mut out,
        );
        flow_side(
            build, node_spans, printer, entry.b, &name_b, &mut seen, &mut out,
        );
    }
    out
}

fn flow_side(
    build: &Build,
    node_spans: &HashMap<NodeId, Span>,
    printer: &mut TypePrinter,
    root: NodeId,
    name: &str,
    seen: &mut HashSet<Span>,
    out: &mut Vec<String>,
) {
    for member in disjoint::members(&build.module.nodes, representative(&build.module, root)) {
        if let Some(span) = node_spans.get(&member).copied()
            && let Some(value) = build.module.nodes[member].value
            && !is_unbound(Some(value))
            && seen.insert(span)
        {
            out.push(format!(
                "{name} is fixed to {} at line {}",
                printer.node(member),
                span.0
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diag::Stage;

    use lichen_highlevel::checker::Checker;
    use lichen_highlevel::ir::{ExprKind, IR};
    use lichen_lowlevel::{ArrayRef, FunctionId, ValueExt};
    use lichen_utils::extend::AsEnum;

    #[test]
    fn renders_the_offending_line_with_a_caret() {
        // `y` is at line 1, column 6 — the caret lands under it.
        let diag = Diag::new(Stage::Resolve, (1, 6), "unresolved name 'y'".to_string());
        let out = render("x => y", &diag);
        assert_eq!(
            out,
            "error: unresolved name 'y'\n  --> 1:6\n   |\n 1 | x => y\n   |      ^\n"
        );
    }

    #[test]
    fn a_spanless_diagnostic_has_no_caret() {
        let diag = Diag {
            span: None,
            message: "internal".to_string(),
            stage: Stage::Check,
            check: None,
        };
        assert_eq!(render("x", &diag), "error: internal\n");
    }

    #[test]
    fn a_checker_message_uses_the_cli_type_syntax() {
        // 5 : Int -> Int — the found type is Int, the expected the arrow
        // type: the same spellings the CLI prints for a program's output,
        // not the raw `TypeInt → TypeInt`.
        let report = crate::compile("5 : Int -> Int");
        assert_eq!(
            report.diagnostics[0].message,
            "expected Int -> Int, found Int\n  ?a is fixed to Int at line 1\n  ?b is fixed to Int -> Int at line 1"
        );
    }

    #[test]
    fn an_array_element_conflict_renders_unbound_arrow_cells() {
        // [1, x => x] — the found side is the lambda's arrow shape with its
        // two unbound cells sharing one name.  (The `?c` line appears twice,
        // exactly as in the raw highlevel rendering: two members of the Int
        // class at different columns both render the same line.)
        let report = crate::compile("[1, x => x]");
        assert_eq!(
            report.diagnostics[0].message,
            "expected Int, found ?a -> ?a\n  ?b is fixed to ?a -> ?a at line 1\n  ?c is fixed to Int at line 1\n  ?c is fixed to Int at line 1"
        );
    }

    #[test]
    fn a_struct_conflict_keeps_the_nominal_ids() {
        // Two source occurrences are different nominal types, and the ids
        // stay visible even in the pretty rendering.
        let report =
            crate::compile("s1 = struct<Int, Int>; s2 = struct<Int, Int>; [s1(1, 2), s2(1, 2)]");
        let message = &report.diagnostics[0].message;
        assert!(message.contains("TypeId("), "{}", message);
        assert!(message.contains("struct<Int, Int>"), "{}", message);
    }

    // --- the type-chain-driven value rendering ------------------------------

    /// Run `source` and return its rendered `value: type` output.
    fn output(source: &str) -> String {
        crate::run::evaluate(source).expect("the program runs clean")
    }

    #[test]
    fn a_struct_type_value_renders_in_type_syntax() {
        // A struct type's value is the raw shape `[TypeId(0), [Int, Type]]` —
        // the lowlevel data layout.  Read against its kind, it prints as the
        // code that produced it.
        assert_eq!(output("A = struct<Int, Type>\nA"), "struct<Int, Type>: TypeStruct");
    }

    #[test]
    fn a_struct_instance_renders_its_field_tuple() {
        assert_eq!(
            output("A = struct<Int, Type>\na = A(1, Int)\n(A, a, a[0], a[1])"),
            "(struct<Int, Type>, (1, Int), 1, Int): <TypeStruct, struct<Int, Type>, Int, Type>"
        );
    }

    #[test]
    fn a_single_field_struct_instance_keeps_the_tuple_comma() {
        assert_eq!(
            output("B = struct<Int>\nb = B((1,))\n(B, b)"),
            "(struct<Int>, (1,)): <TypeStruct, struct<Int>>"
        );
    }

    #[test]
    fn a_tuple_value_renders_with_parens() {
        // The type says tuple, so the value reads as the source tuple, not
        // the raw `[1, Int]` array layout.
        assert_eq!(output("(1, Int)"), "(1, Int): <Int, Type>");
    }

    #[test]
    fn an_array_value_keeps_brackets() {
        assert_eq!(output("[1, 2, 3]"), "[1, 2, 3]: Int<3>");
    }

    #[test]
    fn a_compound_type_value_renders_in_type_syntax() {
        // `Int -> Int` as a value is the shape `[Int, Int]`; read against its
        // kind it prints as the arrow, not the raw pair.
        assert_eq!(output("Int -> Int"), "Int -> Int: TypeFunction");
        assert_eq!(output("<Int, Type>"), "<Int, Type>: TypeTuple");
        assert_eq!(output("Int<3>"), "Int<3>: TypeArray");
    }

    #[test]
    fn a_type_second_slot_does_not_collapse_an_array() {
        // The raw layout's `[head, K]` heuristic reads a two-element array
        // whose second element is the universe as an atomic type pair and
        // drops the head.  The type chain says what the value really is: a
        // tuple keeps both elements.
        assert_eq!(output("(1, Type)"), "(1, Type): <Int, Type>");
    }

    #[test]
    fn a_function_value_prints_function() {
        assert_eq!(output("x => x"), "Function: ?a -> ?a");
        assert_eq!(output("f = x => x\nf"), "Function: ?a -> ?a");
    }

    // --- the extended vocabulary --------------------------------------------

    // A probe extension: a type constant beyond the highlevel's vocabulary,
    // spliced into a flat union with `extend_HighProgramValue!` — the path a
    // language crate takes to add its own value variants.  The renderers are
    // generic over the vocabulary; the extension's own variant renders
    // through the hook both printers carry.
    lichen_highlevel::extend_HighProgramValue! {
        #[derive(Debug, Clone, Copy, PartialEq)]
        pub enum ProbeValue {
            FloatType,
        }
    }

    impl From<LowValue> for ProbeValue {
        fn from(value: LowValue) -> Self {
            HighProgramValue::from(value).into()
        }
    }

    impl AsEnum<LowValue> for ProbeValue {
        fn as_enum(&self) -> Option<LowValue> {
            AsEnum::<HighProgramValue>::as_enum(self).and_then(|value| value.as_enum())
        }
    }

    impl ValueExt for ProbeValue {
        fn is_handle(&self) -> bool {
            false
        }
    }

    impl ValueType for ProbeValue {
        fn int_marker() -> Self {
            Self::TypeInt
        }
        fn type_marker() -> Self {
            Self::TypeType
        }
        fn function_type_marker() -> Self {
            Self::TypeFunction
        }
        fn tuple_type_marker() -> Self {
            Self::TypeTuple
        }
        fn array_type_marker() -> Self {
            Self::TypeArray
        }
        fn type_struct_marker() -> Self {
            Self::TypeStruct
        }
        fn type_of(&self) -> Self {
            match self {
                ProbeValue::USize(_) => Self::TypeInt,
                ProbeValue::FloatType
                | ProbeValue::TypeInt
                | ProbeValue::TypeType
                | ProbeValue::TypeFunction
                | ProbeValue::TypeTuple
                | ProbeValue::TypeArray
                | ProbeValue::TypeStruct
                | ProbeValue::TypeId(_) => Self::TypeType,
                _ => unreachable!("a structural non-USize value is not a constant"),
            }
        }
        fn type_id(&self) -> Option<usize> {
            match self {
                Self::TypeId(n) => Some(*n),
                _ => None,
            }
        }
        fn type_id_value(n: usize) -> Self {
            Self::TypeId(n)
        }
    }

    #[test]
    fn an_extended_value_renders_through_the_hook() {
        // `FloatType : Type` — the extension's type constant, paired with the
        // universe like `Int` is.  The value printer (generic over the
        // vocabulary) reads it as an atomic type constant; the extension's
        // own spelling comes from the render hook.
        let mut ir: IR<ProbeValue> = IR::new();
        let float_ty = ir.alloc(ExprKind::Constant(ProbeValue::FloatType), None);
        ir.set_root(float_ty);
        let build = Checker::build(ir);
        assert!(build.ok);
        let mut module = build.module;
        let value = module.evaluate_node_deep(build.root_val, None);
        module.evaluate_node_deep(build.root_ty, None);
        let render_ext = |value: &ProbeValue| match value {
            ProbeValue::FloatType => Some("FloatType".to_string()),
            _ => None,
        };
        let mut printer = ValuePrinter::new_with_ext(&module, Some(&render_ext));
        assert_eq!(printer.print(value, build.root_ty), "FloatType");
        assert_eq!(print_type(&module, build.root_ty), "Type");
    }

    #[test]
    fn an_extended_value_without_a_hook_prints_a_placeholder() {
        // Without a hook the base renderer cannot know the extension's own
        // variant — it degrades to `?` rather than panicking.
        let mut ir: IR<ProbeValue> = IR::new();
        let float_ty = ir.alloc(ExprKind::Constant(ProbeValue::FloatType), None);
        ir.set_root(float_ty);
        let build = Checker::build(ir);
        let mut module = build.module;
        let value = module.evaluate_node_deep(build.root_val, None);
        module.evaluate_node_deep(build.root_ty, None);
        assert_eq!(
            ValuePrinter::new(&module).print(value, build.root_ty),
            "?"
        );
    }
}
