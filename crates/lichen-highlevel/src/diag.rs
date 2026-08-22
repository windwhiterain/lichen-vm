//! Diagnostics for the highlevel checker.
//!
//! The lowlevel records unification failures as facts
//! ([`lichen_lowlevel::Module::unify_errors`]); this module turns them into rendered
//! diagnostics.  Each checker-issued unify is attributed in the **diary**
//! ([`DiaryEntry`]): which error it produced, the source span, and what kind
//! of check it implements — the kind drives the expected/found wording.
//! Runtime apply-time failures (the parameter type check, executed by the
//! VM) have no diary entry; they are attributed by walking the two conflict
//! classes for checker-built nodes, via a node → span side table derived
//! from the checker's records.
//!
//! The type printer renders values with stable `?a` names per class (one
//! class, one name within a diagnostic — confluence), renders pending
//! computations (dependent type codomains) symbolically, and cuts cycles at
//! the class name.

use std::collections::{HashMap, HashSet};

use lichen_lowlevel::{NodeId, Operator, UnifyError, Value};
use lichen_utils::disjoint::{self, Node as _};

use crate::{
    checker::Build,
    expr::{ExprId, Span},
    program::{HighProgram, HighValue},
};

/// A rendered diagnostic: a message plus the source span it is grounded in.
#[derive(Clone, Debug)]
pub struct Diag {
    pub span: Option<Span>,
    pub message: String,
}

/// What kind of check a checker-issued unification implements — drives the
/// wording and the expected/found direction.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DiagKind {
    /// `inner : T` — expected = the annotation's type value.
    Annotation,
    /// Type-position kinding `ty[e] : Type` — expected = `Type`.
    Kinding,
    /// Applying a concretely non-function type — expected = a function.
    Guard,
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
    /// Render the unification failures (plus top-level ambiguity) as
    /// diagnostics — one per entry in
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
        if self.ok && is_unbound(self.module.nodes[report.rep(self.root_ty)].value) {
            out.push(report.ambiguity());
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
    fn node_spans(&self) -> HashMap<NodeId, Span> {
        let mut map = HashMap::new();
        let mut visited = HashSet::new();
        for (i, expr) in self.ir.expr.iter().enumerate() {
            if let Some(span) = expr.span {
                let e = ExprId(i as u32);
                for node in [self.term[e], self.val[e], self.ty[e]].into_iter().flatten() {
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
        if let Some(Value::Array(ptr)) = self.module.nodes[node].value {
            for &id in unsafe { &*ptr } {
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
        let span = entry
            .and_then(|e| e.span)
            .or_else(|| self.best_span(err));
        let mut message = match entry {
            Some(entry) => self.checker_error(entry, err),
            None => self.runtime_error(err),
        };
        let flow = self.flow(entry, err);
        if !flow.is_empty() {
            message.push_str("\n  ");
            message.push_str(&flow.join("\n  "));
        }
        Diag { span, message }
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
            DiagKind::Kinding => format!("expected Type, found {}", self.print_type(err.a)),
            DiagKind::Guard => format!("expected a function, found {}", self.print_type(err.a)),
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
                    self.flow_value(member, value),
                    span.0
                ));
            }
        }
    }

    /// A fixed member's value: type expressions (arrays) render as types
    /// (`[int, Type]` as `int`, the universe as `Type`), leaves by value.
    fn flow_value(&mut self, member: NodeId, value: Value<HighProgram>) -> String {
        match value {
            Value::Array(_) => self.print_type(member),
            _ => value_str(value),
        }
    }

    /// Residual unbound placeholders at the top level render as ambiguity.
    fn ambiguity(&mut self) -> Diag {
        let name = self.name_of(self.rep(self.build.root_ty));
        Diag {
            span: self.build.ir[self.build.ir.root].span,
            message: format!("cannot determine the type of the program: {name} is ambiguous"),
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
            None | Some(Value::Parameterized) => {
                if self.class_has_pending_op(rep) {
                    format!("⟨{}⟩", self.op_name(rep))
                } else {
                    self.name_of(rep)
                }
            }
            Some(Value::Ext(HighValue::Int)) => "int".to_string(),
            Some(Value::Ext(HighValue::Type)) => "Type".to_string(),
            Some(Value::Ext(HighValue::FunctionType)) => "FunctionType".to_string(),
            Some(Value::Ext(HighValue::ArrayType)) => "ArrayType".to_string(),
            Some(Value::USize(n)) => n.to_string(),
            Some(Value::None) => "unit".to_string(),
            Some(Value::Function(_)) => "a function".to_string(),
            Some(Value::Array(ptr)) => {
                let ids = unsafe { &*ptr };
                if ids.len() == 2 {
                    let kind = ids[1];
                    // `[shape, K]`: an atomic type expression — render the
                    // shape's marker (`int`, `Type`, …).
                    if self.rep(kind) == self.univ {
                        return self.print_inner(ids[0], visiting);
                    }
                    // `[shape, [Kind, K]]`: a compound type expression —
                    // the kind decides the shape's rendering.
                    if let Some(Value::Array(kptr)) = self.build.module.nodes[self.rep(kind)].value {
                        let k = unsafe { &*kptr };
                        if k.len() == 2 && self.rep(k[1]) == self.univ {
                            match self.build.module.nodes[self.rep(k[0])].value {
                                Some(Value::Ext(HighValue::FunctionType)) => {
                                    return format!(
                                        "{} → {}",
                                        self.print_inner(ids[0], visiting),
                                        self.print_inner(ids[1], visiting)
                                    );
                                }
                                Some(Value::Ext(HighValue::ArrayType)) => {
                                    let elements: Vec<String> = ids
                                        .iter()
                                        .map(|&id| self.print_inner(id, visiting))
                                        .collect();
                                    return format!("[{}]", elements.join(", "));
                                }
                                _ => {}
                            }
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
                let elements: Vec<String> =
                    ids.iter().map(|&id| self.print_inner(id, visiting)).collect();
                format!("[{}]", elements.join(", "))
            }
        }
    }

    fn class_is_arrow(&self, rep: NodeId) -> bool {
        disjoint::members(&self.build.module.nodes, rep)
            .any(|m| self.build.arrows.contains(&m))
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
            Some(Operator::Index) => "Index".to_string(),
            Some(Operator::Apply) => "Apply".to_string(),
            Some(Operator::Ext(_)) | None => "op".to_string(),
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

/// A class is unbound while it carries no value or only the lazy marker.
fn is_unbound(value: Option<Value<HighProgram>>) -> bool {
    matches!(value, None | Some(Value::Parameterized))
}

/// Render a concrete value for a flow line (not class-aware).
fn value_str(value: Value<HighProgram>) -> String {
    match value {
        Value::Ext(HighValue::Int) => "int".to_string(),
        Value::Ext(HighValue::Type) => "Type".to_string(),
        Value::Ext(HighValue::FunctionType) => "FunctionType".to_string(),
        Value::Ext(HighValue::ArrayType) => "ArrayType".to_string(),
        Value::USize(n) => n.to_string(),
        Value::Array(_) => "an array".to_string(),
        Value::Function(_) => "a function".to_string(),
        Value::None => "unit".to_string(),
        Value::Parameterized => "unbound".to_string(),
    }
}
