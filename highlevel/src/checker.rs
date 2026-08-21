//! The builder/checker: walks an [`ExprTable`], checks types, and constructs
//! the lowlevel [`Module`] — one pass, interleaved.
//!
//! Typing model: every expression `e` has a type *expression* `ty(e)` (a
//! first-class citizen — `type_of(e)` is exactly the Expr `ty(e)`).  The
//! checker's judgment is `unify(node(ty(e)), node(expected))` — the
//! type-expression nodes unify, never the value nodes.  A binder's type
//! starts as a fresh [`ExprKind::TyVar`] whose node is a separate class from
//! the parameter node (the parameter is a runtime value cell, unified only at
//! apply time); after unification the TyVar's node class carries the type
//! value, so the same expression stays a correct type throughout.

use std::collections::{HashMap, HashSet};

use lichen_vm::lowlevel::{BlockId, Module, NodeId, Operation, Operator, Value};

use crate::expr::{ExprId, ExprKind, ExprTable, Span};
use crate::program::{HighProgram, HighValue};

/// Term position or type position.  The checker threads it because the same
/// expression can be used as a value in one place and a type in another
/// (first-class types).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Term,
    Type,
}

/// A variable in scope.
#[derive(Clone)]
struct Binding {
    /// The runtime node (a parameter).
    term: NodeId,
    /// Monomorphic: the shared type expression.  Polymorphic: the
    /// generalized template.
    ty: ExprId,
    poly: bool,
    /// Free type variables of the template (polymorphic only); instantiation
    /// copies these and references everything else in place.
    free: Vec<ExprId>,
}

pub struct Checker {
    ir: ExprTable,
    module: Module<HighProgram>,
    /// Where the checker's synthesized type nodes live; never released.
    types_block: BlockId,
    /// Where runtime nodes for the current expression go.
    current_block: BlockId,
    scopes: Vec<HashMap<ExprId, Binding>>,
    /// Compiled lowlevel node per expression (the value node for terms, the
    /// type structure for type expressions).
    term: Vec<Option<NodeId>>,
    /// Type expression per expression.
    ty: Vec<Option<ExprId>>,
    int_ty: ExprId,
    type_ty: ExprId,
}

/// The result of checking a program: the built Module plus the checker's
/// records.  `ok` is false when any unification failed (`unify_errors` is
/// non-empty); rendering diagnostics from those is future work.
pub struct Build {
    pub ir: ExprTable,
    pub module: Module<HighProgram>,
    pub term: Vec<Option<NodeId>>,
    pub ty: Vec<Option<ExprId>>,
    pub root_term: NodeId,
    pub root_ty: ExprId,
    pub int_ty: ExprId,
    pub type_ty: ExprId,
    pub ok: bool,
}

impl Checker {
    pub fn build(ir: ExprTable) -> Build {
        let mut module = Module::new();
        let root_block = module.add_block(None);
        let types_block = module.add_block(None);
        let mut checker = Checker {
            ir,
            module,
            types_block,
            current_block: root_block,
            scopes: Vec::new(),
            term: Vec::new(),
            ty: Vec::new(),
            int_ty: ExprId(0),
            type_ty: ExprId(0),
        };
        checker.term = vec![None; checker.ir.expr.len()];
        checker.ty = vec![None; checker.ir.expr.len()];
        checker.install_canonical();
        let root = checker.ir.root;
        let root_term = checker.check_expr(root, None, Role::Term);
        let root_ty = checker.ty[root].expect("the root expression must have a type");
        let ok = checker.module.unify_errors.is_empty();
        Build {
            ir: checker.ir,
            module: checker.module,
            term: checker.term,
            ty: checker.ty,
            root_term,
            root_ty,
            int_ty: checker.int_ty,
            type_ty: checker.type_ty,
            ok,
        }
    }

    /// The canonical `Type` and `int` type constants, appended to the table
    /// so the frontend-built part keeps its ids.
    fn install_canonical(&mut self) {
        let type_ty = self.alloc(ExprKind::Type, None);
        let int_ty = self.alloc(ExprKind::Const(HighValue::Int), None);
        let type_node =
            self.module
                .add_node(self.types_block, None, Some(Value::Ext(HighValue::Type)));
        let int_node =
            self.module
                .add_node(self.types_block, None, Some(Value::Ext(HighValue::Int)));
        self.term[type_ty] = Some(type_node);
        self.ty[type_ty] = Some(type_ty); // Type : Type
        self.term[int_ty] = Some(int_node);
        self.ty[int_ty] = Some(type_ty); // int : Type
        self.type_ty = type_ty;
        self.int_ty = int_ty;
    }

    // --- allocation ------------------------------------------------------

    fn alloc(&mut self, kind: ExprKind, span: Option<Span>) -> ExprId {
        let id = self.ir.alloc(kind, span);
        self.term.push(None);
        self.ty.push(None);
        id
    }

    fn alloc_array(&mut self, elements: &[ExprId], span: Option<Span>) -> ExprId {
        let start = self.ir.children.len() as u32;
        self.ir.children.extend_from_slice(elements);
        let range = crate::expr::ChildRange {
            start,
            end: self.ir.children.len() as u32,
        };
        self.alloc(ExprKind::Array(range), span)
    }

    /// A fresh type variable: a new Expr plus a fresh unbound node in the
    /// types block.
    fn fresh_tyvar(&mut self) -> ExprId {
        let id = self.alloc(ExprKind::TyVar, None);
        let node = self.module.add_node(self.types_block, None, None);
        self.term[id] = Some(node);
        self.ty[id] = Some(self.type_ty);
        id
    }

    /// The arrow type `[dom, codom]` (an array value of the two type nodes).
    fn arrow_type(&mut self, dom: ExprId, cod: ExprId) -> ExprId {
        let id = self.alloc_array(&[dom, cod], None);
        let elements = [self.term[dom].unwrap(), self.term[cod].unwrap()];
        let node = self.array_node(self.types_block, &elements);
        self.term[id] = Some(node);
        self.ty[id] = Some(self.type_ty);
        id
    }

    fn array_node(&mut self, block: BlockId, ids: &[NodeId]) -> NodeId {
        let slice = self.module.blocks[block].arena.alloc_slice_copy(ids);
        self.module.add_node(
            block,
            None,
            Some(Value::Array(std::ptr::slice_from_raw_parts(
                slice.as_ptr(),
                slice.len(),
            ))),
        )
    }

    fn op_node(
        &mut self,
        block: BlockId,
        operator: Operator<HighProgram>,
        operand: Option<NodeId>,
    ) -> NodeId {
        self.module
            .add_node(block, Some(Operation { operator, operand }), None)
    }

    /// Unify the compiled nodes of two type expressions.  Both must already
    /// be compiled (their `term` entries set).
    fn unify_types(&mut self, a: ExprId, b: ExprId) {
        let na = self.term[a].expect("type expression must be compiled");
        let nb = self.term[b].expect("type expression must be compiled");
        self.module.unify(na, nb);
    }

    // --- the check -------------------------------------------------------

    fn check_expr(&mut self, e: ExprId, expected: Option<ExprId>, role: Role) -> NodeId {
        let node = match (role, self.ir[e].kind) {
            (Role::Type, ExprKind::Var(target)) => self.check_var_type(e, target),
            (Role::Type, ExprKind::Array(_)) => self.check_array_type(e),
            // Type-position literals, lambdas, etc. are not syntactic types:
            // check as a term, then kinding records the mismatch.
            (Role::Type, _) => {
                let node = self.check_term(e);
                self.unify_types(self.ty[e].unwrap(), self.type_ty);
                node
            }
            (Role::Term, _) => self.check_term(e),
        };
        if role == Role::Term
            && let Some(exp) = expected
        {
            self.unify_types(self.ty[e].unwrap(), exp);
        }
        node
    }

    fn check_term(&mut self, e: ExprId) -> NodeId {
        match self.ir[e].kind {
            ExprKind::Int(n) => {
                let node =
                    self.module
                        .add_node(self.current_block, None, Some(Value::USize(n as usize)));
                self.term[e] = Some(node);
                self.ty[e] = Some(self.int_ty);
                node
            }
            ExprKind::Type => {
                let node = self.term[self.type_ty].unwrap();
                self.term[e] = Some(node);
                self.ty[e] = Some(self.type_ty);
                node
            }
            ExprKind::Const(HighValue::Int) => {
                let node = self.term[self.int_ty].unwrap();
                self.term[e] = Some(node);
                self.ty[e] = Some(self.type_ty);
                node
            }
            ExprKind::Const(HighValue::Type) => {
                let node = self.term[self.type_ty].unwrap();
                self.term[e] = Some(node);
                self.ty[e] = Some(self.type_ty);
                node
            }
            ExprKind::TyVar => {
                let node = self.module.add_node(self.types_block, None, None);
                self.term[e] = Some(node);
                self.ty[e] = Some(self.type_ty);
                node
            }
            ExprKind::Binder => {
                debug_assert!(false, "a binder used as a value");
                let node =
                    self.module
                        .add_node(self.current_block, None, Some(Value::Parameterized));
                self.term[e] = Some(node);
                self.ty[e] = Some(self.fresh_tyvar());
                node
            }
            ExprKind::Var(target) => self.check_var(e, target),
            ExprKind::Lam(binder, body) => self.check_lam(e, binder, body),
            ExprKind::Let(binder, value, body) => self.check_let(e, binder, value, body),
            ExprKind::App(f, x) => self.check_app(e, f, x),
            ExprKind::Ann(inner, t) => self.check_ann(e, inner, t),
            ExprKind::Array(_) => self.check_array_term(e),
        }
    }

    fn check_var(&mut self, e: ExprId, target: ExprId) -> NodeId {
        match self.lookup(target) {
            Some(binding) if binding.poly => {
                let instance = self.instantiate(binding.ty, &binding.free);
                self.term[e] = Some(binding.term);
                self.ty[e] = Some(instance);
            }
            Some(binding) => {
                self.term[e] = Some(binding.term);
                self.ty[e] = Some(binding.ty);
            }
            None => {
                debug_assert!(false, "unresolved variable reference");
                self.term[e] = Some(self.term[self.type_ty].unwrap());
                self.ty[e] = Some(self.type_ty);
            }
        }
        self.term[e].unwrap()
    }

    /// A variable used in type position.  Its value *is* the type: for a
    /// monomorphic binding the parameter node (which holds the type at
    /// runtime), for a polymorphic binding the instantiated type node.  The
    /// referenced value's own type must be `Type` (kinding).
    fn check_var_type(&mut self, e: ExprId, target: ExprId) -> NodeId {
        match self.lookup(target) {
            Some(binding) if binding.poly => {
                let instance = self.instantiate(binding.ty, &binding.free);
                self.term[e] = Some(self.term[instance].unwrap());
                self.ty[e] = Some(self.type_ty);
            }
            Some(binding) => {
                self.term[e] = Some(binding.term);
                self.ty[e] = Some(binding.ty);
                let node = self.term[binding.ty].unwrap();
                self.module.unify(node, self.term[self.type_ty].unwrap());
            }
            None => {
                debug_assert!(false, "unresolved variable reference");
                self.term[e] = Some(self.term[self.type_ty].unwrap());
                self.ty[e] = Some(self.type_ty);
            }
        }
        self.term[e].unwrap()
    }

    fn check_lam(&mut self, e: ExprId, binder: ExprId, body: ExprId) -> NodeId {
        let body_block = self.module.add_block(None);
        let saved = self.current_block;
        self.current_block = body_block;
        let param = self
            .module
            .add_node(body_block, None, Some(Value::Parameterized));
        let tyvar = self.fresh_tyvar();
        self.term[binder] = Some(param);
        self.ty[binder] = Some(tyvar);
        self.scopes.push(HashMap::from([(
            binder,
            Binding {
                term: param,
                ty: tyvar,
                poly: false,
                free: Vec::new(),
            },
        )]));
        let ret = self.check_expr(body, None, Role::Term);
        self.scopes.pop();
        self.current_block = saved;
        let nodes = self.module.blocks[body_block].nodes.clone();
        let func_node = self.module.add_function(body_block, ret, param, &nodes);
        let arrow = self.arrow_type(tyvar, self.ty[body].unwrap());
        self.term[e] = Some(func_node);
        self.ty[e] = Some(arrow);
        func_node
    }

    fn check_let(&mut self, e: ExprId, binder: ExprId, value: ExprId, body: ExprId) -> NodeId {
        let vt = self.check_expr(value, None, Role::Term);
        let vty = self.ty[value].unwrap();
        let free = self.generalize(vty);
        let body_block = self.module.add_block(None);
        let saved = self.current_block;
        self.current_block = body_block;
        let param = self
            .module
            .add_node(body_block, None, Some(Value::Parameterized));
        self.term[binder] = Some(param);
        self.ty[binder] = Some(vty);
        self.scopes.push(HashMap::from([(
            binder,
            Binding {
                term: param,
                ty: vty,
                poly: true,
                free,
            },
        )]));
        let ret = self.check_expr(body, None, Role::Term);
        self.scopes.pop();
        self.current_block = saved;
        let nodes = self.module.blocks[body_block].nodes.clone();
        let func_node = self.module.add_function(body_block, ret, param, &nodes);
        // Runtime encoding of `let x = e in b`: apply a function over `e`.
        let operands = self.array_node(saved, &[func_node, vt]);
        let node = self.op_node(saved, Operator::Apply, Some(operands));
        self.term[e] = Some(node);
        self.ty[e] = Some(self.ty[body].unwrap());
        node
    }

    fn check_app(&mut self, e: ExprId, f: ExprId, x: ExprId) -> NodeId {
        let ft = self.check_expr(f, None, Role::Term);
        let d = self.fresh_tyvar();
        let c = self.fresh_tyvar();
        let arrow = self.arrow_type(d, c);
        // The guard: the function's type must unify with an arrow.
        self.unify_types(self.ty[f].unwrap(), arrow);
        let xt = self.check_expr(x, Some(d), Role::Term);
        let operands = self.array_node(self.current_block, &[ft, xt]);
        let node = self.op_node(self.current_block, Operator::Apply, Some(operands));
        self.term[e] = Some(node);
        self.ty[e] = Some(c);
        node
    }

    fn check_ann(&mut self, e: ExprId, inner: ExprId, t: ExprId) -> NodeId {
        self.check_expr(t, None, Role::Type);
        let it = self.check_expr(inner, Some(t), Role::Term);
        self.term[e] = Some(it);
        self.ty[e] = Some(t);
        it
    }

    fn check_array_term(&mut self, e: ExprId) -> NodeId {
        let range = match self.ir[e].kind {
            ExprKind::Array(range) => range,
            _ => unreachable!(),
        };
        let elements: Vec<ExprId> =
            self.ir.children[range.start as usize..range.end as usize].to_vec();
        let mut terms = Vec::new();
        let mut tys = Vec::new();
        for &el in &elements {
            let t = self.check_expr(el, None, Role::Term);
            terms.push(t);
            tys.push(self.ty[el].unwrap());
        }
        let node = self.array_node(self.current_block, &terms);
        // The array type is an array of the element type expressions.
        let ty_id = self.alloc_array(&tys, None);
        let nodes: Vec<NodeId> = tys.iter().map(|&ty| self.term[ty].unwrap()).collect();
        let ty_node = self.array_node(self.types_block, &nodes);
        self.term[ty_id] = Some(ty_node);
        self.ty[ty_id] = Some(self.type_ty);
        self.term[e] = Some(node);
        self.ty[e] = Some(ty_id);
        node
    }

    /// An array used as a type: its value *is* the type structure (an array
    /// of the element type nodes), and its own type is `Type`.
    fn check_array_type(&mut self, e: ExprId) -> NodeId {
        let range = match self.ir[e].kind {
            ExprKind::Array(range) => range,
            _ => unreachable!(),
        };
        let elements: Vec<ExprId> =
            self.ir.children[range.start as usize..range.end as usize].to_vec();
        for &el in &elements {
            self.check_expr(el, None, Role::Type);
        }
        let nodes: Vec<NodeId> = elements.iter().map(|&el| self.term[el].unwrap()).collect();
        let node = self.array_node(self.types_block, &nodes);
        self.term[e] = Some(node);
        self.ty[e] = Some(self.type_ty);
        node
    }

    // --- polymorphism ----------------------------------------------------

    fn lookup(&self, target: ExprId) -> Option<Binding> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(&target).cloned())
    }

    /// Generalize the free type variables of `ty`: the type variables
    /// reachable from it that are not merged with any context binding's
    /// type.  (A merged variable is bound by the context and must not be
    /// re-instantiated at uses.)
    fn generalize(&mut self, ty: ExprId) -> Vec<ExprId> {
        let mut free = Vec::new();
        let mut seen = HashSet::new();
        self.collect_tyvars(ty, &mut seen, &mut free);
        free.retain(|&tv| {
            let tv_rep = self.module.equality_representative(self.term[tv].unwrap());
            !self.scopes.iter().any(|scope| {
                scope.values().any(|b| {
                    let b_rep = self
                        .module
                        .equality_representative(self.term[b.ty].unwrap());
                    tv_rep == b_rep
                })
            })
        });
        free
    }

    fn collect_tyvars(&self, e: ExprId, seen: &mut HashSet<ExprId>, out: &mut Vec<ExprId>) {
        if !seen.insert(e) {
            return;
        }
        match self.ir[e].kind {
            ExprKind::TyVar => out.push(e),
            ExprKind::Array(range) => {
                for &c in &self.ir.children[range.start as usize..range.end as usize] {
                    self.collect_tyvars(c, seen, out);
                }
            }
            _ => {}
        }
    }

    /// Instantiate a generalized template at a use: copy the type
    /// expression, giving the free variables fresh nodes (they *are* the
    /// fresh instance variables); everything else is referenced in place.
    /// The copy preserves the template's sharing — a free variable that
    /// appears twice (e.g. the domain and codomain of `\x. x`) becomes the
    /// *same* fresh variable in the instance.
    fn instantiate(&mut self, template: ExprId, free: &[ExprId]) -> ExprId {
        let mut memo = HashMap::new();
        self.copy_type(template, free, &mut memo)
    }

    fn copy_type(
        &mut self,
        e: ExprId,
        free: &[ExprId],
        memo: &mut HashMap<ExprId, ExprId>,
    ) -> ExprId {
        match self.ir[e].kind {
            ExprKind::TyVar => {
                if free.contains(&e) {
                    *memo.entry(e).or_insert_with(|| self.fresh_tyvar())
                } else {
                    e
                }
            }
            ExprKind::Array(range) => {
                let children: Vec<ExprId> =
                    self.ir.children[range.start as usize..range.end as usize].to_vec();
                let children: Vec<ExprId> = children
                    .iter()
                    .map(|&c| self.copy_type(c, free, memo))
                    .collect();
                let id = self.alloc_array(&children, None);
                let nodes: Vec<NodeId> = children.iter().map(|&c| self.term[c].unwrap()).collect();
                let node = self.array_node(self.types_block, &nodes);
                self.term[id] = Some(node);
                self.ty[id] = Some(self.type_ty);
                id
            }
            ExprKind::Type | ExprKind::Const(_) => e,
            other => {
                debug_assert!(
                    false,
                    "type expression contains a non-type construct: {other:?}"
                );
                e
            }
        }
    }
}
