//! Diagnostics for the highlevel checker.
//!
//! The lowlevel records unification failures as facts
//! ([`lichen_lowlevel::Module::unify_errors`]) and runtime evaluation
//! failures (an out-of-bounds index) as facts
//! ([`lichen_lowlevel::Module::eval_errors`]); this module turns them into
//! rendered diagnostics.  Each checker-issued unify is attributed in the
//! **diary** ([`DiaryEntry`]): which error it produced, the source span, and
//! what kind of check it implements — the kind drives the expected/found
//! wording.  Runtime apply-time failures (the parameter type check, executed
//! by the VM) have no diary entry; they are attributed by walking the two
//! conflict classes for checker-built nodes, via a node → span side table
//! derived from the checker's records.
//!
//! The type printer renders values with stable `?a` names per class (one
//! class, one name within a diagnostic — confluence), renders pending
//! computations (dependent type codomains) symbolically, and cuts cycles at
//! the class name.

use std::collections::{HashMap, HashSet};

use lichen_lowlevel::{EvalError, NodeId, UnifyError, is_unbound};
use lichen_utils::disjoint::{self, Node as _};

use crate::{
    checker::Build,
    ir::{ExprId, Span},
    program::{HighProgram, HighProgramOperator, HighProgramValue},
};

/// A diagnostic: the structured facts of a unification failure, plus the
/// rendered message for display.  Tests and tooling match on the structured
/// fields; `message` is derived for display/debug only.
#[derive(Clone, Debug)]
pub struct Diag {
    pub span: Option<Span>,
    /// What kind of check failed — see [`DiagKind`] for the expected/found
    /// direction of `a`/`b`.
    pub kind: DiagKind,
    /// The conflicting classes, as the lowlevel recorded them ([`UnifyError`]
    /// snapshots `a`/`b` as class representatives).
    pub a: NodeId,
    pub b: NodeId,
    /// The conflicting classes' values at error time — snapshots, not
    /// re-reads (a failed unify never merges the classes, so they are stable).
    pub value_a: Option<HighProgramValue>,
    pub value_b: Option<HighProgramValue>,
    /// Which `Module::unify_errors` entry this diagnostic came from — the
    /// key back to its diary entry, for callers (the language crate) that
    /// re-render the message.  `None` for runtime evaluation failures.
    pub error_index: Option<usize>,
    /// Rendered for display/debug — not the test contract.
    pub message: String,
}

/// What kind of check a checker-issued unification implements — drives the
/// wording and the expected/found direction of a [`Diag`]'s `a`/`b` (for
/// checker kinds `a` is the found side and `b` the expected; runtime
/// failures reverse them — `a` is the parameter's expected type).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DiagKind {
    /// `inner : T` — expected = the annotation's type value.
    Annotation,
    /// Type-position kinding `ty[e] : Type` — expected = `Type`.
    Kinding,
    /// Applying a concretely non-function type — expected = a function.
    Guard,
    /// Indexing a concretely non-indexable type (a function, an atomic
    /// type) — expected = a tuple, array, or struct type.
    IndexTarget,
    /// An array literal's elements must share one type — expected = the
    /// shared element type, found = this element's type.
    ArrayElement,
    /// A binary operator's operand must be an `Int` (it is unified against
    /// the int type expression, pinning an unbound operand) — expected =
    /// `Int`, found = the operand's type.
    BinOp,
    /// A runtime apply-time failure (the parameter type check, executed by
    /// the VM) — no diary entry.  Reversed direction: `a` = the parameter's
    /// expected type, `b` = the argument's found type.
    Runtime,
    /// An out-of-bounds index — a runtime evaluation failure, not a unify.
    /// `a` is the index node; `value_a` the index value, `value_b` the
    /// container's length.
    IndexOutOfBounds,
}

/// One checker-issued unification, attributed with where it came from.
#[derive(Clone, Copy, Debug)]
pub struct DiaryEntry {
    /// Index into [`lichen_lowlevel::Module::unify_errors`] of the first
    /// error this unify
    /// produced (one unify may record several, e.g. elementwise).
    pub error_index: usize,
    pub a: NodeId,
    pub b: NodeId,
    pub span: Option<Span>,
    pub kind: DiagKind,
}

impl Build {
    /// Render the unification failures as diagnostics — one per entry in
    /// [`lichen_lowlevel::Module::unify_errors`], in order.
    pub fn diagnostics(&self) -> Vec<Diag> {
        let mut report = Report {
            build: self,
            names: HashMap::new(),
            next: 0,
            node_spans: self.node_spans(),
            univ: NodeId::default(),
        };
        report.univ = report.rep(self.type_expr);
        let mut out = Vec::new();
        for (i, err) in self.module.unify_errors.iter().enumerate() {
            out.push(report.error(i, err));
        }
        // Runtime evaluation failures (an out-of-bounds index).  The value
        // and type evaluation of the same expression each record one, so
        // identical facts collapse to a single diagnostic.
        let mut seen = HashSet::new();
        for err in &self.module.eval_errors {
            if seen.insert((err.index, err.index_value, err.length)) {
                out.push(report.eval_error(err));
            }
        }
        out
    }

    /// The node → span side table: every checker-built node (the compiled
    /// pair, value, and type of each expression) *and the elements of its
    /// array values* map to its source span, so a type marker buried inside
    /// a type expression is attributed to the expression that built it.  The
    /// shared universe `[Type, ↺]` is recorded but not descended into — its
    /// elements (the `Type` marker) only carry spans through direct
    /// references, so a parameter's template type never inherits the span of
    /// an unrelated expression that happens to share the universe.
    pub fn node_spans(&self) -> HashMap<NodeId, Span> {
        let mut map = HashMap::new();
        let mut visited = HashSet::new();
        for (i, expr) in self.ir.expr.iter().enumerate() {
            if let Some(span) = expr.span {
                let e = ExprId(i as u32);
                for node in [self.term[e], self.val[e], self.ty[e]]
                    .into_iter()
                    .flatten()
                {
                    self.record_span(node, span, &mut map, &mut visited);
                }
            }
        }
        map
    }

    fn record_span(
        &self,
        node: NodeId,
        span: Span,
        map: &mut HashMap<NodeId, Span>,
        visited: &mut HashSet<NodeId>,
    ) {
        if !visited.insert(node) {
            return;
        }
        map.insert(node, span);
        if node == self.type_expr {
            return; // the shared universe — do not leak its span to its elements
        }
        if let Some(ids) = self.module.array_ids(node) {
            for &id in ids {
                self.record_span(id, span, map, visited);
            }
        }
    }
}

struct Report<'a> {
    build: &'a Build,
    /// Stable class names: representative → `?a`, `?b`, … within one
    /// diagnostic.
    names: HashMap<NodeId, String>,
    next: usize,
    node_spans: HashMap<NodeId, Span>,
    /// The class representative of the canonical universe `[Type, ↺]`.
    univ: NodeId,
}

impl Report<'_> {
    /// The class representative of `node`, via a read-only `parent` walk (the
    /// printer never mutates the module).
    fn rep(&self, node: NodeId) -> NodeId {
        let mut n = node;
        while let Some(parent) = self.build.module.nodes[n].meta().parent {
            n = parent;
        }
        n
    }

    fn name_of(&mut self, rep: NodeId) -> String {
        if let Some(name) = self.names.get(&rep) {
            return name.clone();
        }
        let name = letter_name(self.next);
        self.next += 1;
        self.names.insert(rep, name.clone());
        name
    }

    fn error(&mut self, i: usize, err: &UnifyError<HighProgram>) -> Diag {
        // The owning diary entry: the last one whose error_index <= i (one
        // unify may own a whole run of errors, e.g. elementwise).
        let entry = self.build.diary.iter().rev().find(|e| e.error_index <= i);
        let span = entry.and_then(|e| e.span).or_else(|| self.best_span(err));
        let mut message = match entry {
            Some(entry) => self.checker_error(entry, err),
            None => self.runtime_error(err),
        };
        let flow = self.flow(entry, err);
        if !flow.is_empty() {
            message.push_str("\n  ");
            message.push_str(&flow.join("\n  "));
        }
        Diag {
            span,
            kind: entry.map(|e| e.kind).unwrap_or(DiagKind::Runtime),
            a: err.a,
            b: err.b,
            value_a: err.value_a,
            value_b: err.value_b,
            error_index: Some(i),
            message,
        }
    }

    fn checker_error(&mut self, entry: &DiaryEntry, err: &UnifyError<HighProgram>) -> String {
        // The message renders the *conflicting* classes (err.a/err.b — for an
        // elementwise failure these are the elements that clashed, not the
        // top-level unified nodes); the diary entry supplies the wording and
        // direction.  a is always the found side, b the expected side.
        match entry.kind {
            DiagKind::Annotation => format!(
                "expected {}, found {}",
                self.print_type(err.b),
                self.print_type(err.a)
            ),
            DiagKind::Kinding => format!("expected TypeType, found {}", self.print_type(err.a)),
            DiagKind::Guard => format!("expected a function, found {}", self.print_type(err.a)),
            DiagKind::IndexTarget => format!(
                "expected a tuple, array, or struct type, found {}",
                self.print_type(err.a)
            ),
            DiagKind::ArrayElement => format!(
                "expected {}, found {}",
                self.print_type(err.b),
                self.print_type(err.a)
            ),
            DiagKind::BinOp => format!("expected Int, found {}", self.print_type(err.a)),
            // Diary entries are checker-issued unifies only — runtime
            // failures are rendered elsewhere.
            DiagKind::Runtime | DiagKind::IndexOutOfBounds => {
                unreachable!("the diary never attributes runtime diagnostics")
            }
        }
    }

    /// A runtime apply-time failure: the parameter is the expected side, the
    /// argument the found side.
    fn runtime_error(&mut self, err: &UnifyError<HighProgram>) -> String {
        format!(
            "expected {}, found {}",
            self.print_type(err.a),
            self.print_type(err.b)
        )
    }

    /// Attribution for runtime failures: walk both conflict classes for
    /// checker-built nodes; the first known span wins.
    fn best_span(&self, err: &UnifyError<HighProgram>) -> Option<Span> {
        for rep in [err.a, err.b] {
            for member in disjoint::members(&self.build.module.nodes, rep) {
                if let Some(span) = self.node_spans.get(&member) {
                    return Some(*span);
                }
            }
        }
        None
    }

    /// A runtime evaluation failure — an out-of-bounds index.  The index
    /// literal's span attributes it; there are no conflict classes to walk.
    fn eval_error(&mut self, err: &EvalError) -> Diag {
        Diag {
            span: self.node_spans.get(&err.index).copied(),
            kind: DiagKind::IndexOutOfBounds,
            a: err.index,
            b: err.index,
            value_a: Some(HighProgramValue::USize(err.index_value)),
            value_b: Some(HighProgramValue::USize(err.length)),
            message: format!(
                "index {} out of bounds (array length {})",
                err.index_value, err.length
            ),
            error_index: None,
        }
    }

    /// The HM-loc-style journey: which checker-known members fixed either
    /// side of the conflict, and to what.  The conflicting classes (the
    /// markers where the merge failed) are walked first; for a
    /// diary-attributed check the top-level unified nodes are hunted too —
    /// e.g. the expected side of `5 : Type` is the universe `K` itself.
    fn flow(&mut self, entry: Option<&DiaryEntry>, err: &UnifyError<HighProgram>) -> Vec<String> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        let name_a = self.name_of(self.rep(err.a));
        let name_b = self.name_of(self.rep(err.b));
        self.flow_side(err.a, &name_a, &mut seen, &mut out);
        self.flow_side(err.b, &name_b, &mut seen, &mut out);
        if let Some(entry) = entry {
            self.flow_side(entry.a, &name_a, &mut seen, &mut out);
            self.flow_side(entry.b, &name_b, &mut seen, &mut out);
        }
        out
    }

    fn flow_side(
        &mut self,
        root: NodeId,
        name: &str,
        seen: &mut HashSet<Span>,
        out: &mut Vec<String>,
    ) {
        for member in disjoint::members(&self.build.module.nodes, self.rep(root)) {
            if let Some(span) = self.node_spans.get(&member).copied()
                && let Some(value) = self.build.module.nodes[member].value
                && !is_unbound(Some(value))
                && seen.insert(span)
            {
                out.push(format!(
                    "{name} is fixed to {} at line {}",
                    self.print_type(member),
                    span.0
                ));
            }
        }
    }

    // --- the type printer -------------------------------------------------

    /// Render a node's class as a type.  Unbound cells get stable `?a`
    /// names; pending computations (dependent codomains) render
    /// symbolically; cycles are cut at the class name.
    fn print_type(&mut self, node: NodeId) -> String {
        self.print_inner(node, &mut HashSet::new())
    }

    fn print_inner(&mut self, node: NodeId, visiting: &mut HashSet<NodeId>) -> String {
        let rep = self.rep(node);
        if !visiting.insert(rep) {
            return self.name_of(rep);
        }
        let out = self.render(rep, visiting);
        visiting.remove(&rep);
        out
    }

    fn render(&mut self, rep: NodeId, visiting: &mut HashSet<NodeId>) -> String {
        let value = self.build.module.nodes[rep].value;
        match value {
            None | Some(HighProgramValue::Parameterized) => {
                if self.class_has_pending_op(rep) {
                    format!("⟨{}⟩", self.op_name(rep))
                } else {
                    self.name_of(rep)
                }
            }
            Some(HighProgramValue::TypeInt) => "TypeInt".to_string(),
            Some(HighProgramValue::TypeType) => "TypeType".to_string(),
            Some(HighProgramValue::TypeFunction) => "TypeFunction".to_string(),
            Some(HighProgramValue::TypeTuple) => "TypeTuple".to_string(),
            Some(HighProgramValue::TypeArray) => "TypeArray".to_string(),
            Some(HighProgramValue::TypeId(n)) => format!("TypeId({n})"),
            Some(HighProgramValue::USize(n)) => n.to_string(),
            Some(HighProgramValue::None) => "none".to_string(),
            Some(HighProgramValue::Function(_)) => "Function".to_string(),
            Some(HighProgramValue::Array(_)) => {
                let ids = self
                    .build
                    .module
                    .array_ids(rep)
                    .expect("the value just matched as an array");
                if ids.len() == 2 {
                    let kind = ids[1];
                    // `[shape, K]`: an atomic type expression — render the
                    // shape's marker (`int`, `Type`, …).
                    if self.rep(kind) == self.univ {
                        return self.print_inner(ids[0], visiting);
                    }
                    // `[shape, [Kind, K]]`: a compound type expression —
                    // the kind decides the shape's rendering.
                    if let Some(k) = self.build.module.array_ids(self.rep(kind))
                        && k.len() == 2
                        && self.rep(k[1]) == self.univ
                    {
                        match self.build.module.nodes[self.rep(k[0])].value {
                            Some(HighProgramValue::TypeFunction) => {
                                // The pair is `[shape, [FunctionType, K]]`
                                // where shape = [in, out] — render the
                                // arrow `in → out`, not `shape → kind`.
                                if let Some(s) = self.build.module.array_ids(self.rep(ids[0]))
                                    && s.len() == 2
                                {
                                    return format!(
                                        "{} → {}",
                                        self.print_inner(s[0], visiting),
                                        self.print_inner(s[1], visiting)
                                    );
                                }
                            }
                            Some(HighProgramValue::TypeTuple) => {
                                let elements: Vec<String> = ids
                                    .iter()
                                    .map(|&id| self.print_inner(id, visiting))
                                    .collect();
                                return format!("[{}]", elements.join(", "));
                            }
                            Some(HighProgramValue::TypeArray) => {
                                // The array type's pair is [shape, kind]
                                // where shape = [type, length] — render
                                // `int[3]`, not `[int, 3]`.
                                if let Some(s) = self.build.module.array_ids(self.rep(ids[0]))
                                    && s.len() == 2
                                {
                                    return format!(
                                        "{}[{}]",
                                        self.print_inner(s[0], visiting),
                                        self.print_inner(s[1], visiting)
                                    );
                                }
                            }
                            Some(HighProgramValue::TypeId(n)) => {
                                // A struct type: `[fields, [TypeId(n), Type]]`
                                // — render `struct#n { f1, f2 }`, the shape
                                // being the field-type list.
                                let fields: Vec<String> = if let Some(s) =
                                    self.build.module.array_ids(self.rep(ids[0]))
                                {
                                    s.iter().map(|&id| self.print_inner(id, visiting)).collect()
                                } else {
                                    vec![self.print_inner(ids[0], visiting)]
                                };
                                return format!("struct#{n} {{ {} }}", fields.join(", "));
                            }
                            _ => {}
                        }
                    }
                    if self.class_is_arrow(rep) {
                        return format!(
                            "{} → {}",
                            self.print_inner(ids[0], visiting),
                            self.print_inner(ids[1], visiting)
                        );
                    }
                }
                let elements: Vec<String> = ids
                    .iter()
                    .map(|&id| self.print_inner(id, visiting))
                    .collect();
                format!("[{}]", elements.join(", "))
            }
        }
    }

    fn class_is_arrow(&self, rep: NodeId) -> bool {
        disjoint::members(&self.build.module.nodes, rep).any(|m| self.build.arrows.contains(&m))
    }

    fn class_has_pending_op(&self, rep: NodeId) -> bool {
        disjoint::members(&self.build.module.nodes, rep).any(|m| {
            self.build.module.nodes[m].operation.is_some()
                && is_unbound(self.build.module.nodes[m].value)
        })
    }

    fn op_name(&self, rep: NodeId) -> String {
        match disjoint::members(&self.build.module.nodes, rep)
            .find_map(|m| self.build.module.nodes[m].operation)
            .map(|op| op.operator)
        {
            Some(HighProgramOperator::Index | HighProgramOperator::IndexType) => {
                "Index".to_string()
            }
            Some(HighProgramOperator::Apply) => "Apply".to_string(),
            Some(HighProgramOperator::Fresh) => "Fresh".to_string(),
            Some(
                HighProgramOperator::Add
                | HighProgramOperator::Sub
                | HighProgramOperator::Leq
                | HighProgramOperator::Eq,
            ) => "op".to_string(),
            None => "op".to_string(),
        }
    }
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
