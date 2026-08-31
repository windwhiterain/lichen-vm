//! Rendering: the program's output and the diagnostics share one printer.
//!
//! The CLI output and the diagnostics are rendered as equal as possible:
//! [`print_value`] renders the program's result value *read against its type
//! chain* — the type decides how the value reads, so a struct type value
//! prints `struct<Int, Type>` (not the raw `[TypeId(0), Int]` pair), a tuple
//! value `(1, Int)` (not `[1, Int]`), and an array `[1, 2, 3]` — and
//! [`print_type`] renders the result type in the language's own type syntax.
//! A checker diagnostic's message ([`checker_message`]) re-renders the
//! highlevel's structured facts — the [`DiagKind`], the conflict classes, the
//! diary — with the *same* [`TypePrinter`], so a failed `5 : Int -> Int`
//! reports `expected Int -> Int, found Int` in the language's own type
//! syntax.  The highlevel crate now emits the same pretty view in
//! `Diag::message`; this module re-exports that printer and adds the caret
//! shell ([`render`]) around either message:
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

use lichen_highlevel::diagnostic::Diag as CheckerDiag;
use lichen_highlevel::print::{is_struct_kind, kind_is_struct};
pub use lichen_highlevel::print::TypePrinter;
use lichen_highlevel::program::{HighProgram, HighProgramValue, ValueType};
use lichen_lowlevel::{AnyNodeId, LowValue, Module, NodeId};

use crate::diag::Diag;

// A rendering hook for extension vocabularies: an extension value → its
// spelling, or `None` when the value is not the extension’s to name.  The
// alias exists because the bare closure type trips clippy’s `type_complexity`.

type RenderExt<'a, V> = &'a dyn Fn(&V) -> Option<String>;

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
pub fn print_value<V: ValueType>(module: &Module<HighProgram<V>>, value: V, ty: NodeId) -> String {
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
        render_ext: Option<RenderExt<'a, V>>,
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
        if self.printer.is_universe_any(AnyNodeId::Dynamic(ty)) {
            return self.atomic(value);
        }
        let Some(LowValue::Array(ty_array)) = self
            .module
            .node_value(AnyNodeId::Dynamic(ty))
            .and_then(|v| v.as_enum())
        else {
            return self.raw(value);
        };
        let tys = ty_array.items();
        // A struct type itself: the value's type is the struct kind
        // `[id, [TypeStruct, K]]` (not a `[shape, [marker, K]]` pair), and the
        // value is the field-type list — render `struct<T1, ..., Tn>`.
        if is_struct_kind(self.module, ty)
            && let Some(LowValue::Array(shape)) = value.as_enum()
        {
            let fields: Vec<String> = shape
                .items()
                .iter()
                .map(|item| self.printer.any_node(item.node))
                .collect();
            return format!("struct<{}>", fields.join(", "));
        }
        // A kind `[marker, K]`: the value is a compound type — render its
        // shape in type syntax.
        if tys.len() == 2
            && self.printer.is_universe_any(tys[1].node)
            && let Some(out) = self.compound_type(value, tys[0].node)
        {
            return out;
        }
        // A struct instance: the value reads against the struct's
        // field-type list (the shape); its kind is `[id, [TypeStruct, K]]`,
        // so it is detected beside the `[marker, K]` kinds.
        if tys.len() == 2
            && self.is_struct_kind_any(tys[1].node)
            && let Some(out) = self.instance(value, tys[0].node, V::type_struct_marker())
        {
            return out;
        }
        // A term of a tuple/array/struct type `[shape, [marker, K]]`: the
        // value's elements read against the shape.
        if tys.len() == 2
            && let Some(kind) = self.module.node_value(tys[1].node)
            && let Some(LowValue::Array(kind)) = kind.as_enum()
            && let kind = kind.items()
            && kind.len() == 2
            && self.printer.is_universe_any(kind[1].node)
            && let Some(marker) = self.module.node_value(kind[0].node)
            && let Some(out) = self.instance(value, tys[0].node, marker)
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
    fn compound_type(&mut self, value: V, marker_node: AnyNodeId) -> Option<String> {
        let marker = self.module.node_value(marker_node)?;
        let Some(LowValue::Array(shape)) = value.as_enum() else {
            return None;
        };
        let shape = shape.items();
        if marker == V::function_type_marker() {
            if shape.len() == 2 {
                return Some(format!(
                    "{} -> {}",
                    self.printer.any_node(shape[0].node),
                    self.printer.any_node(shape[1].node)
                ));
            }
        } else if marker == V::tuple_type_marker() {
            let fields: Vec<String> = shape
                .iter()
                .map(|item| self.printer.any_node(item.node))
                .collect();
            return Some(format!("<{}>", fields.join(", ")));
        } else if marker == V::array_type_marker() && shape.len() == 2 {
            return Some(format!(
                "{}<{}>",
                self.printer.any_node(shape[0].node),
                self.printer.any_node(shape[1].node)
            ));
        }
        // A struct type's kind is `[shape, [id, [TypeStruct, K]]]`, not a
        // `[marker, K]` pair, so it never reaches `compound_type` — it is
        // handled by the struct branch in `value` / `elements`.
        None
    }

    /// A term of a tuple, array, or struct type: the value's elements read
    /// against the shape — a tuple reads `(v1, ..., vn)` (a single element
    /// `(v1,)`), an array `[v1, ..., vn]`, a struct instance its field tuple
    /// `(v1, ..., vn)`.  `None` when the value or the shape does not fit.
    fn instance(&mut self, value: V, shape_node: AnyNodeId, marker: V) -> Option<String> {
        let Some(LowValue::Array(values)) = value.as_enum() else {
            return None;
        };
        let values = values.items();
        let shape = self.module.node_value(shape_node).and_then(|v| v.as_enum());
        let Some(LowValue::Array(shape)) = shape else {
            return None;
        };
        let shape = shape.items();
        if marker == V::tuple_type_marker() {
            if shape.len() != values.len() {
                return None;
            }
            let mut out = Vec::with_capacity(values.len());
            for (i, v) in values.iter().enumerate() {
                out.push(self.element_any(v.node, shape[i].node));
            }
            return Some(self.parens(&out));
        }
        if marker == V::array_type_marker() {
            if shape.len() != 2 {
                return None;
            }
            let mut out = Vec::with_capacity(values.len());
            for v in values {
                out.push(self.element_any(v.node, shape[0].node));
            }
            return Some(format!("[{}]", out.join(", ")));
        }
        if marker == V::type_struct_marker() {
            // The shape is the positional field-type list (the nominal id
            // lives in the kind), so the element types are the fields.
            let fields = shape;
            if fields.len() != values.len() {
                return None;
            }
            let mut out = Vec::with_capacity(values.len());
            for (i, v) in values.iter().enumerate() {
                out.push(self.element_any(v.node, fields[i].node));
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

    /// [`Self::element`] for static or dynamic value/type refs.
    fn element_any(&mut self, id: AnyNodeId, ty: AnyNodeId) -> String {
        match (id, ty) {
            (AnyNodeId::Dynamic(id), AnyNodeId::Dynamic(ty)) => {
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
            _ => {
                let value = self
                    .module
                    .node_value(id)
                    .unwrap_or_else(|| V::from(LowValue::None));
                self.raw_any(value)
            }
        }
    }

    /// Whether an `AnyNodeId` names a struct kind `[id, [TypeStruct, K]]`.
    fn is_struct_kind_any(&self, id: AnyNodeId) -> bool {
        self.module
            .node_value(id)
            .and_then(|v| v.as_enum())
            .is_some_and(|v| match v {
                LowValue::Array(kind) => kind_is_struct(self.module, kind.items()),
                _ => false,
            })
    }

    /// The raw value layout — the fallback when the type chain cannot guide
    /// the reading: a type pair `[head, [Type, ↺]]` renders as its head
    /// (`[TypeInt, K]` → `Int`), arrays `[ ]`, functions `Function`, and the
    /// type constants by their spellings `Int` / `Type`.
    fn raw(&mut self, value: V) -> String {
        self.raw_any(value)
    }

    /// [`Self::raw`] for a value whose array items may be static refs.
    fn raw_any(&mut self, value: V) -> String {
        if let Some(structural) = value.as_enum() {
            return match structural {
                LowValue::USize(n) => n.to_string(),
                LowValue::Function(_) => "Function".to_string(),
                LowValue::None => "none".to_string(),
                LowValue::Parameterized => "parameterized".to_string(),
                LowValue::Array(array) => {
                    let elements = array.items();
                    // A type pair `[head, K]`: the kind slot is the
                    // self-looping universe, so render just the head (and cut
                    // the cycle).
                    if elements.len() == 2 && self.printer.is_universe_any(elements[1].node) {
                        let head = self
                            .module
                            .node_value(elements[0].node)
                            .unwrap_or_else(|| V::from(LowValue::None));
                        return self.raw_any(head);
                    }
                    let mut out = Vec::new();
                    for item in elements {
                        let value = self
                            .module
                            .node_value(item.node)
                            .unwrap_or_else(|| V::from(LowValue::None));
                        let text = self.raw_any(value);
                        out.push(text);
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
// per kind.  One TypePrinter drives the whole message (and a whole report),
// so a class keeps a single `?a` name; it must carry the checker's arrow
// registry.  The `?a` journey is gone — every expression's type is queryable,
// so the user inspects an expr's type instead of reading a source trace.

/// Re-render a checker diagnostic's message with the shared pretty printer,
/// mirroring the highlevel's raw rendering ([`liche_highlevel::diagnostic`])
/// but in the language's own type syntax.  `printer` is shared across a whole
/// report, so a class keeps a single `?a` name across diagnostics.
/// The checker diagnostic's message, already rendered by the highlevel's
/// shared [`TypePrinter`].
///
/// The language crate used to re-render highlevel raw facts into CLI syntax.
/// The highlevel now produces the same pretty message, so this is a thin
/// compatibility wrapper (and the natural place for consumers that still want
/// to override it).
pub fn checker_message(_printer: &mut TypePrinter, d: &CheckerDiag) -> String {
    d.message.clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diag::Stage;

    use lichen_highlevel::checker::Checker;
    use lichen_highlevel::ir::{ExprKind, IR};
    use lichen_highlevel::program::TypeValue;
    use lichen_lowlevel::ValueExt;

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
        // not the raw `TypeInt → TypeInt`.  No `?a` journey line — the user
        // inspects the expression's type directly.
        let report = crate::compile("5 : Int -> Int");
        assert_eq!(
            report.diagnostics[0].message,
            "expected Int -> Int, found Int"
        );
    }

    #[test]
    fn an_array_element_conflict_renders_unbound_arrow_cells() {
        // [1, x => x] — the found side is the lambda's arrow shape with its
        // two unbound cells sharing one name.  No `?a` journey line.
        let report = crate::compile("[1, x => x]");
        assert_eq!(
            report.diagnostics[0].message,
            "expected Int, found ?a -> ?a"
        );
    }

    #[test]
    fn a_struct_conflict_keeps_the_nominal_ids() {
        // Two source occurrences are different nominal types.  The message
        // renders each side's full struct type *with its nominal id*
        // (`struct<Int, Int>#0` vs `#1`), so the two structs stay
        // distinguishable even though their field shapes match.
        let report =
            crate::compile("s1 = struct<Int, Int>; s2 = struct<Int, Int>; [s1(1, 2), s2(1, 2)]");
        let message = &report.diagnostics[0].message;
        assert!(message.contains("struct<Int, Int>#"), "{}", message);
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
        assert_eq!(
            output("A = struct<Int, Type>\nA"),
            "struct<Int, Type>: TypeStruct"
        );
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
        // A single field needs no extra comma in the source (`B(1)`); the
        // rendered value still shows the one-element tuple's comma `(1,)`.
        assert_eq!(
            output("B = struct<Int>\nb = B(1)\n(B, b)"),
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
    // composed in one `enum_ext!` invocation that lists every layer's enum
    // directly — the path a language crate takes to add its own value
    // variants.  The renderers are generic over the vocabulary; the
    // extension's own variant renders through the hook both printers carry.
    lichen_utils::enum_ext! {
        #[derive(Debug, Clone, Copy, PartialEq)]
        pub enum ProbeValue {
            FloatType,
        }
        + LowValue as LowValue;
        + TypeValue as TypeValue;
    }

    impl ValueExt for ProbeValue {
        fn is_handle(&self) -> bool {
            false
        }
    }

    impl ValueType for ProbeValue {
        fn int_marker() -> Self {
            Self::TypeValue(TypeValue::TypeInt)
        }
        fn type_marker() -> Self {
            Self::TypeValue(TypeValue::TypeType)
        }
        fn function_type_marker() -> Self {
            Self::TypeValue(TypeValue::TypeFunction)
        }
        fn tuple_type_marker() -> Self {
            Self::TypeValue(TypeValue::TypeTuple)
        }
        fn array_type_marker() -> Self {
            Self::TypeValue(TypeValue::TypeArray)
        }
        fn type_struct_marker() -> Self {
            Self::TypeValue(TypeValue::TypeStruct)
        }
        fn type_of(&self) -> Self {
            match self {
                ProbeValue::FloatType | ProbeValue::TypeValue(_) => Self::type_marker(),
                ProbeValue::LowValue(LowValue::USize(_)) => Self::int_marker(),
                ProbeValue::LowValue(_) => {
                    unreachable!("a structural non-USize value is not a constant")
                }
            }
        }
        fn type_id(&self) -> Option<usize> {
            match self {
                Self::TypeValue(TypeValue::TypeId(n)) => Some(*n),
                _ => None,
            }
        }
        fn type_id_value(n: usize) -> Self {
            Self::TypeValue(TypeValue::TypeId(n))
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
        assert_eq!(ValuePrinter::new(&module).print(value, build.root_ty), "?");
    }
}
