//! The resolved content key of the resolved program.
//! See docs/notes/incremental-update.md §6.1.

use super::*;
/// The content-key format version — written as the key's **first** element.
///
/// # Invariant
///
/// Adding an [`Expr`] variant or changing any tag bumps this constant; there is
/// no compatibility path, so the bump invalidates every cached key. `9` added
/// the value-template variants (`Unify` 39, `TemplateUnify` 40, `Clone` 41,
/// `StructValue` 42, `Array` 43, `ArrayInst` 44) and retired the compound-type
/// ones. See docs/notes/incremental-update.md §6.1.
const KEY_FORMAT_VERSION: u64 = 9;

/// The tag that opens a path-written binder, so an id-written key can never alias one.
const PATH_BINDER: u64 = 0x7061_7468_0000_0001;

/// The **per-binding key** of `value`: its structure, reads named by path, read digests.
///
/// # Invariant
/// The `cache` mark and `let` are **not** written: neither changes the computation, and a key
/// that moved when a mark did would invalidate a value nothing touched
/// (`docs/notes/heavy-value-cache.md` §2).
pub(crate) fn binding_key(
    value: &Expr,
    paths: &HashMap<BinderId, crate::path::Path>,
    read_digests: &[u64],
) -> Vec<u64> {
    let mut k = KeyWriter {
        key: Vec::new(),
        binders: Binders::Path(paths),
    };
    k.u(KEY_FORMAT_VERSION);
    k.expr(value);
    k.u(read_digests.len() as u64);
    for digest in read_digests {
        k.u(*digest);
    }
    k.key
}

/// The **resolved content key** of a resolved program.
///
/// # Invariant
///
/// An exact, injective serialization of what the lowering consumes:
/// names by [`BinderId`], error blocks opaque, spans dropped, literals and
/// field/operator names kept, `pub`/field identity kept for record programs,
/// and the `cache` mark kept. Equal keys mean identical lowering-visible
/// content, which is when the session reuses the build.
/// See docs/notes/incremental-update.md §6.1.
pub fn content_key(program: &Program) -> Vec<u64> {
    let mut k = KeyWriter {
        key: Vec::new(),
        binders: Binders::Id,
    };
    k.u(KEY_FORMAT_VERSION);
    k.program(program);
    k.key
}

/// How a binder is written: its resolution id, or the occurrence path that outlives it.
///
/// # Invariant
/// A per-binding key must use [`Binders::Path`]. Binder ids are numbered in traversal order,
/// so inserting a statement renumbers every later one and a key written by id would change
/// for a binding nothing touched (`docs/notes/heavy-value-cache.md` §2).
enum Binders<'a> {
    Id,
    Path(&'a HashMap<BinderId, crate::path::Path>),
}

struct KeyWriter<'a> {
    key: Vec<u64>,
    binders: Binders<'a>,
}

impl KeyWriter<'_> {
    fn u(&mut self, v: u64) {
        self.key.push(v);
    }
    fn b(&mut self, v: bool) {
        self.key.push(v as u64);
    }
    fn str(&mut self, s: &str) {
        self.u(s.len() as u64);
        self.key.extend(s.bytes().map(u64::from));
    }
    fn opt_binder(&mut self, binder: &Option<BinderId>) {
        if !matches!(self.binders, Binders::Path(_)) {
            self.u(binder.map(|b| b as u64).unwrap_or(UNRESOLVED));
            return;
        }
        let path = match &self.binders {
            Binders::Path(paths) => binder.and_then(|binder| paths.get(&binder).cloned()),
            Binders::Id => None,
        };
        self.u(PATH_BINDER);
        match path {
            Some(path) => self.path(&path),
            None => self.u(UNRESOLVED),
        }
    }

    /// One occurrence path: its steps, each tagged by kind.
    fn path(&mut self, path: &crate::path::Path) {
        self.u(path.0.len() as u64);
        for step in &path.0 {
            match step {
                crate::path::Step::Name(name) => {
                    self.u(0);
                    self.str(name);
                }
                crate::path::Step::Index(index) => {
                    self.u(1);
                    self.u(*index as u64);
                }
                crate::path::Step::Tail => self.u(2),
            }
        }
    }

    /// The program shape and its top level: fields for a record program, plain
    /// statements for a tail program.
    fn program(&mut self, program: &Program) {
        let module = program.expr.is_none();
        self.b(module);
        self.u(program.statements.len() as u64);
        for bs in &program.statements {
            if module {
                self.record_field(bs);
            } else {
                self.stmt(&bs.stmt);
            }
        }
        if let Some(e) = &program.expr {
            self.u(2);
            self.expr(e);
        }
    }

    /// A statement of a tail program — `pub` dropped.
    fn stmt(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::Binding(b) => {
                self.u(0);
                self.b(b.restrictive);
                self.b(b.cached);
                self.opt_binder(&b.binder);
                self.expr(&b.value);
            }
            Stmt::Expr(e) => {
                self.u(1);
                self.expr(e);
            }
        }
    }

    /// A record program's statement — a field, as `compile_record_fields` lowers it.
    fn record_field(&mut self, bs: &crate::ast::BlockStmt) {
        let (name, field) = match &bs.stmt {
            Stmt::Binding(b) => (Some(b.name.as_str()), !b.restrictive),
            Stmt::Expr(_) => (None, true),
        };
        self.u(3);
        self.b(name.is_some());
        self.b(field);
        self.b(bs.public);
        if let Some(name) = name {
            self.str(name);
        }
        match &bs.stmt {
            Stmt::Binding(b) => {
                self.b(b.cached);
                self.opt_binder(&b.binder);
                self.expr(&b.value);
            }
            Stmt::Expr(e) => self.expr(e),
        }
    }

    /// One expression: its own unique tag, then its fields.
    ///
    /// # Invariant
    ///
    /// Tags are unique across [`Expr`] variants and every list writes its
    /// length, so the encoding is injective; adding a variant or changing a tag
    /// bumps [`KEY_FORMAT_VERSION`]. `#[stacksafe]`: one stack frame per nested
    /// expression.
    #[stacksafe]
    fn expr(&mut self, e: &Expr) {
        match e {
            Expr::Int(n, _) => {
                self.u(0);
                self.u(*n as u64);
            }
            // A float enters as its bits, per `LowValue`'s equality: `0.0` and
            // `-0.0` differ, two equal `NaN` patterns do not.
            Expr::Float(n, _) => {
                self.u(33);
                self.u(n.to_bits() as u64);
            }
            Expr::Str(s, _) => {
                self.u(24);
                self.str(s);
            }
            Expr::TypeConst(c, _) => {
                self.u(1);
                self.u(match c {
                    crate::ast::TypeConst::Int => 0,
                    crate::ast::TypeConst::Float => 3,
                    crate::ast::TypeConst::Type => 1,
                    crate::ast::TypeConst::String => 2,
                    crate::ast::TypeConst::Tuple => 4,
                    crate::ast::TypeConst::Array => 5,
                    crate::ast::TypeConst::Struct => 6,
                });
            }
            Expr::Name(_name, _, binder) => {
                self.u(2);
                self.opt_binder(binder);
            }
            Expr::Placeholder(_) => self.u(3),
            Expr::Any(_) => self.u(36),
            // `@gc` is its own form, so it does not collide with a plain one.
            Expr::Gc(inner, _) => {
                self.u(38);
                self.expr(inner);
            }
            // The recovered-error region / an unresolved name: content-free,
            // position-only.
            Expr::Err { .. } => self.u(4),
            Expr::Lambda {
                parameter_binder,
                parameter_type,
                parameter_perspective,
                r#return,
                ..
            } => {
                self.u(5);
                self.opt_binder(parameter_binder);
                self.opt_expr(parameter_type);
                self.u(6);
                self.opt_expr(parameter_perspective);
                self.u(7);
                self.expr(r#return);
            }
            Expr::Apply {
                function, argument, ..
            } => {
                self.u(8);
                self.expr(function);
                self.expr(argument);
            }
            // `a = b`: the actual value and the expected one, in that order.
            Expr::Unify {
                actual, expected, ..
            } => {
                self.u(39);
                self.expr(actual);
                self.expr(expected);
            }
            // `actual <: template`: the actual and the template it is matched against.
            Expr::TemplateUnify {
                actual, template, ..
            } => {
                self.u(40);
                self.expr(actual);
                self.expr(template);
            }
            Expr::BinOp {
                operator,
                left,
                right,
                ..
            } => {
                self.u(9);
                self.u(match operator {
                    crate::ast::BinOp::Add => 0,
                    crate::ast::BinOp::Sub => 1,
                    crate::ast::BinOp::Leq => 2,
                    crate::ast::BinOp::Eq => 3,
                    crate::ast::BinOp::Mul => 4,
                    crate::ast::BinOp::Div => 5,
                    crate::ast::BinOp::Rem => 6,
                    crate::ast::BinOp::Lt => 7,
                    crate::ast::BinOp::Gt => 8,
                    crate::ast::BinOp::Geq => 9,
                    crate::ast::BinOp::Neq => 10,
                    crate::ast::BinOp::BitAnd => 11,
                    crate::ast::BinOp::BitOr => 12,
                    crate::ast::BinOp::BitXor => 13,
                    crate::ast::BinOp::In => 14,
                });
                self.expr(left);
                self.expr(right);
            }
            Expr::If {
                condition,
                then_branch,
                else_branch,
                ..
            } => {
                self.u(10);
                self.expr(condition);
                self.expr(then_branch);
                self.expr(else_branch);
            }
            Expr::Assert { value, .. } => {
                self.u(11);
                self.expr(value);
            }
            // The operator is content: `int2float x` and `float2int x` differ, and so
            // do `max a b` and `min a b`.
            Expr::Prefix {
                operator, operands, ..
            } => {
                self.u(34);
                self.u(match operator {
                    crate::ast::PrefixOp::Int2Float => 0,
                    crate::ast::PrefixOp::Float2Int => 1,
                    crate::ast::PrefixOp::Max => 2,
                    crate::ast::PrefixOp::Min => 3,
                    crate::ast::PrefixOp::Exp => 4,
                    crate::ast::PrefixOp::Sqrt => 5,
                    crate::ast::PrefixOp::Floor => 6,
                    crate::ast::PrefixOp::Sin => 7,
                    crate::ast::PrefixOp::Cos => 8,
                    crate::ast::PrefixOp::Ln => 9,
                });
                for operand in operands {
                    self.expr(operand);
                }
            }
            Expr::NativeCall { op, args, .. } => {
                self.u(12);
                self.str(op);
                self.u(args.len() as u64);
                for a in args {
                    self.expr(a);
                }
            }
            Expr::Index { array, index, .. } => {
                self.u(13);
                self.expr(array);
                self.expr(index);
            }
            Expr::FieldRead { container, key, .. } => {
                self.u(14);
                self.expr(container);
                self.expr(key);
            }
            Expr::NamedFieldRead {
                container, name, ..
            } => {
                self.u(32);
                self.expr(container);
                self.str(name);
            }
            Expr::TableFind { container, key, .. } => {
                self.u(15);
                self.expr(container);
                self.expr(key);
            }
            Expr::Annotation {
                value,
                r#type,
                perspective,
                doc,
                refinement,
                marks,
                ..
            } => {
                self.u(16);
                self.expr(value);
                self.opt_expr(r#type);
                self.opt_expr(perspective);
                self.opt_expr(doc);
                self.opt_expr(refinement);
                // A mark is part of what the source wrote.
                for mark in marks {
                    self.str(mark);
                }
            }
            Expr::Tuple(elems, _) => {
                self.u(29);
                self.u(elems.len() as u64);
                for el in elems {
                    self.expr(el);
                }
            }
            Expr::Set(members, _) => {
                self.u(35);
                self.u(members.len() as u64);
                for member in members {
                    self.expr(member);
                }
            }
            Expr::TypeSet(element, _) => {
                self.u(36);
                self.expr(element);
            }
            Expr::TypeTable { key, value, .. } => {
                self.u(37);
                self.expr(key);
                self.expr(value);
            }
            // `{.a e1, …}`: the struct template's fields, each with its name.
            Expr::StructValue { fields, .. } => {
                self.u(42);
                self.u(fields.len() as u64);
                for f in fields {
                    self.str(&f.name);
                    self.expr(&f.value);
                }
            }
            // `[template, length]`: the array template and its length.
            Expr::Array {
                template, length, ..
            } => {
                self.u(43);
                self.expr(template);
                self.expr(length);
            }
            // `template[e1, …]`: the array instance's template and its elements.
            Expr::ArrayInst {
                template, elements, ..
            } => {
                self.u(44);
                self.expr(template);
                self.u(elements.len() as u64);
                for el in elements {
                    self.expr(el);
                }
            }
            Expr::StructInst { callee, fields, .. } => {
                self.u(19);
                self.expr(callee);
                self.u(fields.len() as u64);
                for f in fields {
                    self.str(&f.name);
                    self.expr(&f.value);
                }
            }
            Expr::Table(entries, _) => {
                self.u(20);
                self.u(entries.len() as u64);
                for (k, v) in entries {
                    self.expr(k);
                    self.expr(v);
                }
            }
            Expr::Shallow(inner, depth, ..) => {
                self.u(21);
                self.u(*depth as u64);
                self.expr(inner);
            }
            Expr::Block {
                statements, expr, ..
            } => {
                self.u(23);
                self.u(statements.len() as u64);
                for s in statements {
                    self.stmt(s);
                }
                self.expr(expr);
            }
            Expr::RecordBlock { fields, .. } => {
                self.u(25);
                self.u(fields.len() as u64);
                for f in fields {
                    self.b(f.name.is_some());
                    self.b(f.field);
                    self.b(f.public);
                    self.b(f.cached);
                    if let Some(name) = &f.name {
                        self.str(name);
                    }
                    self.opt_binder(&f.binder);
                    self.expr(&f.value);
                }
            }
        }
    }

    fn opt_expr(&mut self, e: &Option<Box<Expr>>) {
        match e {
            Some(inner) => {
                self.u(1);
                self.expr(inner);
            }
            None => self.u(0),
        }
    }
}
