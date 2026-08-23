//! Rendering: the program's output and the diagnostics share one printer.
//!
//! The CLI output and the diagnostics are rendered as equal as possible:
//! [`print_type`] renders the program's result type, and a checker
//! diagnostic's message ([`checker_message`]) re-renders the highlevel's raw
//! facts — the [`DiagKind`], the conflict classes, the diary — with the
//! *same* [`TypePrinter`], so a failed `5 : Int -> Int` reports
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

use std::collections::{HashMap, HashSet};

use lichen_highlevel::checker::Build;
use lichen_highlevel::diagnostic::{Diag as CheckerDiag, DiagKind, DiaryEntry};
use lichen_highlevel::ir::Span;
use lichen_highlevel::program::{HighProgram, HighProgramValue};
use lichen_lowlevel::{Module, NodeId, is_unbound};
use lichen_utils::disjoint;

use crate::diag::Diag;

// --- the shared pretty printer ----------------------------------------------
// One printer drives the program output (run::evaluate) and every checker
// diagnostic message, so a type looks the same in both.

/// Render a runtime value as the program's output.
///
/// The runtime value is all the printer sees: arrays print with `[ ]`
/// whether the source wrote a tuple or an array (the same value at
/// runtime), functions print as `Function`, and the type constants by their
/// source spellings `Int` / `Type`.  A type pair `[head, [Type, ↺]]` renders
/// as its head (`[TypeInt, K]` → `Int`), and the self-looping universe as
/// `Type` — otherwise the recursive pair encoding would loop forever.
pub fn print_value(module: &Module<HighProgram>, value: HighProgramValue) -> String {
    print_inner(module, value, &mut Vec::new())
}

fn print_inner(
    module: &Module<HighProgram>,
    value: HighProgramValue,
    path: &mut Vec<NodeId>,
) -> String {
    match value {
        HighProgramValue::USize(n) => n.to_string(),
        HighProgramValue::TypeInt => "Int".to_string(),
        HighProgramValue::TypeType => "Type".to_string(),
        HighProgramValue::TypeFunction => "TypeFunction".to_string(),
        HighProgramValue::TypeTuple => "TypeTuple".to_string(),
        HighProgramValue::TypeArray => "TypeArray".to_string(),
        HighProgramValue::TypeStruct => "TypeStruct".to_string(),
        HighProgramValue::TypeId(n) => format!("TypeId({n})"),
        HighProgramValue::Array(ptr) => {
            let elements = unsafe { &*ptr };
            // A type pair `[head, K]`: the kind slot is the self-looping
            // universe, so render just the head (and cut the cycle).
            if elements.len() == 2 && is_universe(module, elements[1]) {
                return print_inner(
                    module,
                    module.nodes[elements[0]]
                        .value
                        .unwrap_or(HighProgramValue::None),
                    path,
                );
            }
            let mut out = Vec::new();
            for &id in elements {
                if path.contains(&id) {
                    out.push("…".to_string());
                } else {
                    path.push(id);
                    let element = module.nodes[id].value.unwrap_or(HighProgramValue::None);
                    let text = print_inner(module, element, path);
                    path.pop();
                    out.push(text);
                }
            }
            format!("[{}]", out.join(", "))
        }
        HighProgramValue::Function(_) => "Function".to_string(),
        HighProgramValue::None => "none".to_string(),
        HighProgramValue::Parameterized => "parameterized".to_string(),
    }
}

/// Render a type expression (the recursive-pair encoding again) in the
/// language's own type syntax: `Int`, `Type`, `T1 -> T2`, `<T1, ..., Tn>`,
/// `T<len>`, `struct<T1, ...>`.  Unbound cells get stable `?a`, `?b`, …
/// names — cells in the same unification class share a name — so the type
/// shows which parts are linked.  Cycles are cut at `…`.
pub fn print_type(module: &Module<HighProgram>, root: NodeId) -> String {
    TypePrinter::new(module).node(root)
}

/// The shared pretty type printer: stateful across calls, so one instance
/// renders a whole diagnostic (or report) with consistent `?a`/`?b` class
/// names.
pub struct TypePrinter<'a> {
    module: &'a Module<HighProgram>,
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
}

impl<'a> TypePrinter<'a> {
    pub fn new(module: &'a Module<HighProgram>) -> Self {
        Self::new_with_arrows(module, None)
    }

    /// A printer that also knows the checker's arrow shapes, so bare
    /// `[in, out]` shapes render as `in -> out`.
    pub fn new_with_arrows(
        module: &'a Module<HighProgram>,
        arrows: Option<&'a HashSet<NodeId>>,
    ) -> Self {
        TypePrinter {
            module,
            arrows,
            names: HashMap::new(),
            next: 0,
            path: Vec::new(),
        }
    }

    /// Render a type node; an unbound cell renders as its class name.
    pub fn node(&mut self, node: NodeId) -> String {
        if self.path.contains(&node) {
            return "…".to_string();
        }
        let value = self.module.nodes[node]
            .value
            .unwrap_or(HighProgramValue::None);
        if matches!(
            value,
            HighProgramValue::None | HighProgramValue::Parameterized
        ) {
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
    fn value(&mut self, node: NodeId, value: HighProgramValue) -> String {
        match value {
            HighProgramValue::USize(n) => n.to_string(),
            HighProgramValue::TypeInt => "Int".to_string(),
            HighProgramValue::TypeType => "Type".to_string(),
            HighProgramValue::TypeFunction => "TypeFunction".to_string(),
            HighProgramValue::TypeTuple => "TypeTuple".to_string(),
            HighProgramValue::TypeArray => "TypeArray".to_string(),
            HighProgramValue::TypeStruct => "TypeStruct".to_string(),
            HighProgramValue::TypeId(n) => format!("TypeId({n})"),
            HighProgramValue::Function(_) => "Function".to_string(),
            HighProgramValue::None | HighProgramValue::Parameterized => {
                unreachable!("handled by node()")
            }
            HighProgramValue::Array(ptr) => self.elements(node, unsafe { &*ptr }),
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
            && let Some(HighProgramValue::Array(kind_ptr)) = self.module.nodes[elements[1]].value
            && let kind = unsafe { &*kind_ptr }
            && kind.len() == 2
            && is_universe(self.module, kind[1])
        {
            match self.module.nodes[kind[0]].value {
                Some(HighProgramValue::TypeFunction) => {
                    // shape = [in, out] — render `in -> out`.
                    if let Some(HighProgramValue::Array(shape_ptr)) =
                        self.module.nodes[elements[0]].value
                        && let s = unsafe { &*shape_ptr }
                        && s.len() == 2
                    {
                        return format!("{} -> {}", self.node(s[0]), self.node(s[1]));
                    }
                }
                Some(HighProgramValue::TypeTuple) => {
                    // shape = the field-type list — render `<T1, ..., Tn>`.
                    let fields = self.fields(elements[0]);
                    return format!("<{}>", fields.join(", "));
                }
                Some(HighProgramValue::TypeArray) => {
                    // shape = [element type, length] — render `T<len>`.
                    if let Some(HighProgramValue::Array(shape_ptr)) =
                        self.module.nodes[elements[0]].value
                        && let s = unsafe { &*shape_ptr }
                        && s.len() == 2
                    {
                        return format!("{}<{}>", self.node(s[0]), self.node(s[1]));
                    }
                }
                Some(HighProgramValue::TypeStruct) => {
                    // A struct type: shape = [TypeId, field-type list] —
                    // render `struct<T1, ..., Tn>` from the list at shape[1].
                    if let Some(HighProgramValue::Array(shape_ptr)) =
                        self.module.nodes[elements[0]].value
                        && let s = unsafe { &*shape_ptr }
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
    fn fields(&mut self, shape: NodeId) -> Vec<String> {
        match self.module.nodes[shape].value {
            Some(HighProgramValue::Array(ptr)) => {
                unsafe { &*ptr }.iter().map(|&f| self.node(f)).collect()
            }
            _ => vec![self.node(shape)],
        }
    }
}

/// The class representative of `node`, via a read-only `parent` walk (the
/// printers never mutate the module).
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
/// that contains a member of its own unification class.  A plain
/// self-referential member (`contains(&node)`) is the canonical node itself;
/// a cell unified into the universe class carries the replicated value, whose
/// self-referential member is the canonical node — the class check covers
/// both.
fn is_universe(module: &Module<HighProgram>, node: NodeId) -> bool {
    let rep = representative(module, node);
    matches!(module.nodes[node].value, Some(HighProgramValue::Array(ptr))
        if unsafe { &*ptr }.iter().any(|&m| representative(module, m) == rep))
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
        DiagKind::Kinding => format!("expected Type, found {}", printer.node(d.a)),
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
    fn a_kinding_error_renders_the_universe_as_type() {
        let report = crate::compile("5 : 5");
        assert_eq!(
            report.diagnostics[0].message,
            "expected Type, found Int\n  ?a is fixed to Int at line 1"
        );
        assert_eq!(
            report.diagnostics[1].message,
            "expected 5, found Int\n  ?c is fixed to Int at line 1\n  ?d is fixed to 5 at line 1"
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
}
