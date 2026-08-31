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

use lichen_lowlevel::{
    AnyNodeId, ApplyError, ArrayItem, EvalError, LowOperator, LowValue, NodeId, UnifyError,
    is_unbound,
};
use lichen_utils::disjoint::{self, Node as _};

use crate::{
    checker::Build,
    ir::{ExprId, Span},
    program::{HighProgram, HighProgramOperator, HighProgramValue, TypeOperator, ValueType},
};

/// A diagnostic: the structured facts of a unification failure, plus the
/// rendered message for display.  Tests and tooling match on the structured
/// fields; `message` is derived for display/debug only.
#[derive(Clone, Debug)]
pub struct Diag<V: ValueType = HighProgramValue> {
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
    pub value_a: Option<V>,
    pub value_b: Option<V>,
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
    /// Applying a concretely non-function type — expected = a function.
    Guard,
    /// Indexing a concretely non-indexable type (a function, an atomic
    /// type) — expected = a tuple, array, or struct type.
    IndexTarget,
    /// An array literal's elements must share one type — expected = the
    /// shared element type, found = this element's type.
    ArrayElement,
    /// A table literal's keys must share one type — expected = the shared
    /// key type, found = this key's type.
    TableKey,
    /// A table literal's values must share one type — expected = the shared
    /// value type, found = this value's type.
    TableValue,
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
    /// A table read that missed — no entry for the key (or the key is not
    /// concrete yet, which can match nothing).  A runtime evaluation
    /// failure, not a unify; `a` is the key node.
    TableMiss,
    /// A table build dropped an entry whose key could not be forced
    /// concrete.  A runtime evaluation failure, not a unify; `a` is the key
    /// node.
    TableKeyUnbound,
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

impl<V: ValueType> Build<V> {
    /// Render the unification failures as diagnostics — one per entry in
    /// [`lichen_lowlevel::Module::unify_errors`], in order.
    pub fn diagnostics(&self) -> Vec<Diag<V>> {
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
        // Runtime evaluation failures (an out-of-bounds index, a table
        // read).  The value and type evaluation of the same expression each
        // record one, so identical facts collapse to a single diagnostic.
        let mut seen = HashSet::new();
        for err in &self.module.eval_errors {
            let key = match err {
                EvalError::Index {
                    index,
                    index_value,
                    length,
                } => (Some(*index), Some(*index_value), Some(*length)),
                _ => (None, None, None),
            };
            if seen.insert(key) {
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
        if let Some(items) = self.module.array_items(node) {
            for item in items {
                // A static ref has no importer span: the imported value was
                // not written in this source file.  Only dynamic nodes get
                // spans recorded.
                if let AnyNodeId::Dynamic(node) = item.node {
                    self.record_span(node, span, map, visited);
                }
            }
        }
    }
}

struct Report<'a, V: ValueType> {
    build: &'a Build<V>,
    /// Stable class names: representative → `?a`, `?b`, … within one
    /// diagnostic.
    names: HashMap<NodeId, String>,
    next: usize,
    node_spans: HashMap<NodeId, Span>,
    /// The class representative of the canonical universe `[Type, ↺]`.
    univ: NodeId,
}

impl<'a, V: ValueType> Report<'a, V> {
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

    fn error(&mut self, i: usize, err: &UnifyError<HighProgram<V>>) -> Diag<V> {
        // An apply-time parameter-check failure: the raw unify error dropped
        // the two top-level sides, but the apply context kept them — attribute
        // this error to the call site with the declared parameter type and the
        // argument's type instead of the deep conflict leaves.
        if let Some(apply) = self
            .build
            .module
            .apply_errors
            .iter()
            .find(|a| a.error_index == i)
        {
            return self.apply_error(i, apply, err);
        }
        // The owning diary entry: the last one whose error_index <= i (one
        // unify may own a whole run of errors, e.g. elementwise).
        let entry = self.build.diary.iter().rev().find(|e| e.error_index <= i);
        let span = entry.and_then(|e| e.span).or_else(|| self.best_span(err));
        // `a`/`b` are the *source-meaningful* sides — a diary-attributed
        // check's top-level operands (an expression's type, an annotation's
        // type), so the message reads through the source expr's type chain
        // (the full `struct<…>` shape) rather than from the deep conflict
        // leaves the unify recorded.  A nominal-id mismatch (`TypeId(0)` vs
        // `TypeId(1)`) is a lowlevel symptom; it surfaces here as the two
        // `struct<…>` types whose chains led to it.  `value_a`/`value_b` stay
        // the raw conflict-class snapshots (`err.*`), for tooling that wants
        // the actual mismatching leaves.  For an unattributed runtime error
        // (no diary entry) both fall back to `err.*`.
        let (a, b) = match entry {
            Some(entry) => (entry.a, entry.b),
            None => (err.a, err.b),
        };
        let message = match entry {
            Some(entry) => self.checker_error(entry, a, b),
            None => self.runtime_error(err),
        };
        Diag {
            span,
            kind: entry.map(|e| e.kind).unwrap_or(DiagKind::Runtime),
            a,
            b,
            value_a: err.value_a,
            value_b: err.value_b,
            error_index: Some(i),
            message,
        }
    }

    /// An apply-time parameter-check failure, attributed to the offending
    /// argument: the declared parameter type is the expected side, the
    /// argument's type the found side.  The message (and the structured `a`/
    /// `b`) are the two top-level nodes.  The `Span` comes from the argument
    /// *edge* (`Build::apply_edges`), not from a node lookup — so it points at
    /// the argument being rejected even when the argument node is shared (e.g.
    /// a type constant whose term is reused).  Falls back to the argument
    /// node's span for a hand-built or cloned apply with no recorded edge.
    /// The highlevel message is debug-only (the language crate re-renders
    /// from these fields).
    fn apply_error(
        &mut self,
        i: usize,
        apply: &ApplyError,
        err: &UnifyError<HighProgram<V>>,
    ) -> Diag<V> {
        let span = self
            .build
            .apply_edges
            .get(&apply.apply_node)
            .and_then(|edge| edge.argument_span)
            .or_else(|| self.node_spans.get(&apply.argument).copied());
        let kind = DiagKind::Runtime;
        let message = format!(
            "expected {}, found {}",
            self.print_type(apply.parameter_type),
            self.print_type(apply.argument_type)
        );
        Diag {
            span,
            kind,
            a: apply.parameter_type,
            b: apply.argument_type,
            value_a: err.value_a,
            value_b: err.value_b,
            error_index: Some(i),
            message,
        }
    }

    fn checker_error(&mut self, entry: &DiaryEntry, a: NodeId, b: NodeId) -> String {
        // The message renders the diary's top-level sides — `a` is the found
        // side, `b` the expected — so it reads through the source expr's type
        // chain (the full `struct<…>` shape, an arrow's `_ -> _`), not the raw
        // deep conflict leaves.  The wording and direction come from the kind.
        match entry.kind {
            DiagKind::Annotation => format!(
                "expected {}, found {}",
                self.print_type(b),
                self.print_type(a)
            ),
            DiagKind::Guard => format!(
                "expected {}, found {}",
                self.print_type(b),
                self.print_type(a)
            ),
            DiagKind::IndexTarget => format!(
                "expected a tuple, array, or struct type, found {}",
                self.print_type(a)
            ),
            DiagKind::ArrayElement => format!(
                "expected {}, found {}",
                self.print_type(b),
                self.print_type(a)
            ),
            DiagKind::TableKey => format!(
                "expected key {}, found {}",
                self.print_type(b),
                self.print_type(a)
            ),
            DiagKind::TableValue => format!(
                "expected value {}, found {}",
                self.print_type(b),
                self.print_type(a)
            ),
            DiagKind::BinOp => format!("expected Int, found {}", self.print_type(a)),
            // Diary entries are checker-issued unifies only — runtime
            // failures are rendered elsewhere.
            DiagKind::Runtime
            | DiagKind::IndexOutOfBounds
            | DiagKind::TableMiss
            | DiagKind::TableKeyUnbound => {
                unreachable!("the diary never attributes runtime diagnostics")
            }
        }
    }

    /// A runtime apply-time failure: the parameter is the expected side, the
    /// argument the found side.
    fn runtime_error(&mut self, err: &UnifyError<HighProgram<V>>) -> String {
        format!(
            "expected {}, found {}",
            self.print_type(err.a),
            self.print_type(err.b)
        )
    }

    /// Attribution for runtime failures: walk both conflict classes for
    /// checker-built nodes; the first known span wins.
    fn best_span(&self, err: &UnifyError<HighProgram<V>>) -> Option<Span> {
        for rep in [err.a, err.b] {
            for member in disjoint::members(&self.build.module.nodes, rep) {
                if let Some(span) = self.node_spans.get(&member) {
                    return Some(*span);
                }
            }
        }
        None
    }

    /// A runtime evaluation failure — an out-of-bounds index or a table
    /// read.  The offending literal's span attributes it; there are no
    /// conflict classes to walk.
    fn eval_error(&mut self, err: &EvalError) -> Diag<V> {
        match err {
            // A static index (a solved constant of a plugged dependency) has
            // no importer node or span — report the facts without
            // attribution.
            EvalError::Index {
                index,
                index_value,
                length,
            } => self.index_error(*index, *index_value, *length),
            // A table read that missed (or whose key is still unbound).
            // The key literal's span attributes it.
            EvalError::TableMiss { table: _, key } => {
                let AnyNodeId::Dynamic(key) = *key else {
                    return Diag {
                        span: None,
                        kind: DiagKind::TableMiss,
                        a: NodeId::default(),
                        b: NodeId::default(),
                        value_a: None,
                        value_b: None,
                        message: "table lookup missed — no entry for this key".into(),
                        error_index: None,
                    };
                };
                Diag {
                    span: self.node_spans.get(&key).copied(),
                    kind: DiagKind::TableMiss,
                    a: key,
                    b: key,
                    value_a: None,
                    value_b: None,
                    message: "table lookup missed — no entry for this key".into(),
                    error_index: None,
                }
            }
            EvalError::TableKeyUnbound { key } => {
                let AnyNodeId::Dynamic(key) = *key else {
                    return Diag {
                        span: None,
                        kind: DiagKind::TableKeyUnbound,
                        a: NodeId::default(),
                        b: NodeId::default(),
                        value_a: None,
                        value_b: None,
                        message: "table key is not concrete (it depends on an unbound value) — the entry is dropped".into(),
                        error_index: None,
                    };
                };
                Diag {
                    span: self.node_spans.get(&key).copied(),
                    kind: DiagKind::TableKeyUnbound,
                    a: key,
                    b: key,
                    value_a: None,
                    value_b: None,
                    message: "table key is not concrete (it depends on an unbound value) — the entry is dropped".into(),
                    error_index: None,
                }
            }
        }
    }

    fn index_error(&mut self, index: AnyNodeId, index_value: usize, length: usize) -> Diag<V> {
        let AnyNodeId::Dynamic(index) = index else {
            return Diag {
                span: None,
                kind: DiagKind::IndexOutOfBounds,
                a: NodeId::default(),
                b: NodeId::default(),
                value_a: Some(V::from(LowValue::USize(index_value))),
                value_b: Some(V::from(LowValue::USize(length))),
                message: format!("index {index_value} out of bounds (array length {length})"),
                error_index: None,
            };
        };
        Diag {
            span: self.node_spans.get(&index).copied(),
            kind: DiagKind::IndexOutOfBounds,
            a: index,
            b: index,
            value_a: Some(V::from(LowValue::USize(index_value))),
            value_b: Some(V::from(LowValue::USize(length))),
            message: format!("index {index_value} out of bounds (array length {length})"),
            error_index: None,
        }
    }

    // --- the type printer -------------------------------------------------

    /// Render a node's class as a type.  Unbound cells get stable `?a`
    /// names; pending computations (dependent codomains) render
    /// symbolically; cycles are cut at the class name.
    fn print_type(&mut self, node: NodeId) -> String {
        self.print_inner(node, &mut HashSet::new())
    }

    /// Read the array items behind either a dynamic node or a static ref.
    fn any_items(&self, id: AnyNodeId) -> Option<&'static [ArrayItem]> {
        let value = self.build.module.node_value(id)?;
        let LowValue::Array(array) = value.as_enum()? else {
            return None;
        };
        Some(array.items())
    }

    /// Universe test that accepts static refs.
    fn is_universe_any(&mut self, id: AnyNodeId) -> bool {
        match id {
            AnyNodeId::Dynamic(node) => self.rep(node) == self.univ,
            AnyNodeId::Static(sref) => {
                let Some(items) = self.any_items(id) else {
                    return false;
                };
                items.len() == 2
                    && matches!(items[1].node, AnyNodeId::Static(tail) if tail.module == sref.module && tail.index == sref.index)
            }
        }
    }

    /// Render a dynamic node or a static ref.  Static nodes have no
    /// importer spans or class names, so they render through a small
    /// structural printer that cuts static cycles.
    fn any_node(&mut self, id: AnyNodeId, visiting: &mut HashSet<NodeId>) -> String {
        match id {
            AnyNodeId::Dynamic(node) => self.print_inner(node, visiting),
            AnyNodeId::Static(sref) => self.print_static(sref),
        }
    }

    fn print_static(&mut self, sref: lichen_lowlevel::StaticNodeId) -> String {
        let mut visiting = HashSet::new();
        self.print_static_inner(sref, &mut visiting)
    }

    fn print_static_inner(
        &mut self,
        sref: lichen_lowlevel::StaticNodeId,
        visiting: &mut HashSet<lichen_lowlevel::StaticNodeId>,
    ) -> String {
        if !visiting.insert(sref) {
            return "…".to_string();
        }
        let value = self.build.module.node_value(AnyNodeId::Static(sref));
        let out = match value.as_ref().and_then(|v| v.as_enum()) {
            Some(LowValue::USize(n)) => n.to_string(),
            Some(LowValue::None) => "none".to_string(),
            Some(LowValue::Function(_)) => "Function".to_string(),
            Some(LowValue::Parameterized) | None => "?".to_string(),
            Some(LowValue::Table(_)) => "Table".to_string(),
            Some(LowValue::Array(array)) => {
                let items = array.items();
                if items.len() == 2 && self.is_universe_any(AnyNodeId::Static(sref)) {
                    let mut dummy = HashSet::new();
                    self.any_node(items[0].node, &mut dummy)
                } else {
                    let parts: Vec<String> = items
                        .iter()
                        .map(|item| match item.node {
                            AnyNodeId::Dynamic(node) => {
                                let mut dummy = HashSet::new();
                                self.print_inner(node, &mut dummy)
                            }
                            AnyNodeId::Static(ref next) => self.print_static_inner(*next, visiting),
                        })
                        .collect();
                    format!("[{}]", parts.join(", "))
                }
            }
        };
        visiting.remove(&sref);
        out
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
        // An unbound cell (no value, or the lazy marker) renders as a stable
        // class name — or symbolically when a pending computation (a
        // dependent codomain) is behind it.
        let pending = value.is_none()
            || value.is_some_and(|v| matches!(v.as_enum(), Some(LowValue::Parameterized)));
        if pending {
            if self.class_has_pending_op(rep) {
                format!("⟨{}⟩", self.op_name(rep))
            } else {
                self.name_of(rep)
            }
        } else {
            let value = value.expect("a non-pending cell carries a concrete value");
            match value.as_enum() {
                Some(LowValue::USize(n)) => n.to_string(),
                Some(LowValue::None) => "none".to_string(),
                Some(LowValue::Function(_)) => "Function".to_string(),
                Some(LowValue::Parameterized) => unreachable!("handled above"),
                Some(LowValue::Table(_)) => "Table".to_string(),
                Some(LowValue::Array(_)) => {
                    let items = self
                        .build
                        .module
                        .array_items(rep)
                        .expect("the value just matched as an array");
                    if items.len() == 2 {
                        // `[shape, K]`: an atomic type expression — render the
                        // shape's marker (`int`, `Type`, …).
                        if self.is_universe_any(items[1].node) {
                            return self.any_node(items[0].node, visiting);
                        }
                        // `[shape, [Kind, K]]`: a compound type expression —
                        // the kind decides the shape's rendering.
                        if let Some(k) = self.any_items(items[1].node)
                            && k.len() == 2
                            && self.is_universe_any(k[1].node)
                        {
                            let kind_value = self.build.module.node_value(k[0].node);
                            if kind_value == Some(V::function_type_marker()) {
                                // The pair is `[shape, [FunctionType, K]]`
                                // where shape = [in, out] — render the
                                // arrow `in → out`, not `shape → kind`.
                                if let Some(s) = self.any_items(items[0].node)
                                    && s.len() == 2
                                {
                                    return format!(
                                        "{} → {}",
                                        self.any_node(s[0].node, visiting),
                                        self.any_node(s[1].node, visiting)
                                    );
                                }
                            } else if kind_value == Some(V::tuple_type_marker()) {
                                let elements: Vec<String> = items
                                    .iter()
                                    .map(|item| self.any_node(item.node, visiting))
                                    .collect();
                                return format!("[{}]", elements.join(", "));
                            } else if kind_value == Some(V::array_type_marker()) {
                                // The array type's pair is [shape, kind]
                                // where shape = [type, length] — render
                                // `int[3]`, not `[int, 3]`.
                                if let Some(s) = self.any_items(items[0].node)
                                    && s.len() == 2
                                {
                                    return format!(
                                        "{}[{}]",
                                        self.any_node(s[0].node, visiting),
                                        self.any_node(s[1].node, visiting)
                                    );
                                }
                            } else if kind_value == Some(V::type_struct_marker()) {
                                // A struct type: `[[TypeId(n), fields],
                                // [TypeStruct, Type]]` — render
                                // `struct#n { f1, f2 }`, the id from shape[0]
                                // and the field list from shape[1].
                                let mut n = 0;
                                let mut list = items[0].node;
                                if let Some(s) = self.any_items(items[0].node)
                                    && s.len() == 2
                                {
                                    n = self
                                        .build
                                        .module
                                        .node_value(s[0].node)
                                        .and_then(|v| v.type_id())
                                        .unwrap_or(0);
                                    list = s[1].node;
                                }
                                let fields: Vec<String> = match self.any_items(list) {
                                    Some(fs) => fs
                                        .iter()
                                        .map(|item| self.any_node(item.node, visiting))
                                        .collect(),
                                    None => vec![self.any_node(list, visiting)],
                                };
                                return format!("struct#{n} {{ {} }}", fields.join(", "));
                            }
                        }
                        if self.class_is_arrow(rep) {
                            return format!(
                                "{} → {}",
                                self.any_node(items[0].node, visiting),
                                self.any_node(items[1].node, visiting)
                            );
                        }
                    }
                    let elements: Vec<String> = items
                        .iter()
                        .map(|item| self.any_node(item.node, visiting))
                        .collect();
                    format!("[{}]", elements.join(", "))
                }
                // Extension values (the type constants, a nominal id) render
                // via Debug — the raw `TypeInt` / `TypeId(3)` names.
                None => format!("{value:?}"),
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
            Some(HighProgramOperator::LowOperator(LowOperator::Index)) => "Index".to_string(),
            Some(HighProgramOperator::LowOperator(LowOperator::Apply)) => "Apply".to_string(),
            Some(HighProgramOperator::LowOperator(LowOperator::TableGet)) => "TableGet".to_string(),
            Some(HighProgramOperator::TypeOperator(TypeOperator::Fresh)) => "Fresh".to_string(),
            Some(
                HighProgramOperator::TypeOperator(TypeOperator::Add)
                | HighProgramOperator::TypeOperator(TypeOperator::Sub)
                | HighProgramOperator::TypeOperator(TypeOperator::Leq)
                | HighProgramOperator::TypeOperator(TypeOperator::Eq),
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
