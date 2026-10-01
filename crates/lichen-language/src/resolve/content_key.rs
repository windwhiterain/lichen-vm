//! The resolved content key: an exact, digest-free serialization of the
//! name-resolved program — see [`content_key`].  It lives apart from the scope
//! machine it reads the annotations of.

use super::*;
/// The content-key format version — written as the key's **first** element.
///
/// Adding an [`Expr`] variant, or changing any tag or field encoding in
/// [`KeyWriter`], **bumps** this constant.  There is no compatibility path: the
/// bump invalidates every cached key, so every [`crate::session::BufferSession`]
/// rebuilds on its next compile.  That is the intended answer — a stale key
/// must never be silently reusable.
const KEY_FORMAT_VERSION: u64 = 2;

/// The **resolved content key** of a resolved program: an exact, digest-free
/// serialization of the name-resolved, beyond-error structure that the lowering
/// consumes — names encoded by their [`BinderId`] (never their spelling), error
/// blocks opaque, spans dropped, literals and field/operator names kept (they
/// become IR nodes), `pub`/field identity kept for record programs, and the
/// `cache` mark kept (the lowering consumes it too: a marked binding is the one
/// the cell store may lower to a static read instead of its body).  Two
/// programs with equal keys have literally identical lowering-visible content:
/// every [`Expr`] variant writes its own tag and every list writes its length,
/// so the serialization is injective, not merely self-delimiting.  The
/// incremental session therefore reuses the established IR+Build exactly when
/// the key is unchanged (a rename, an error-block growth, an unresolved-name
/// extension all leave it unchanged).  Built purely from the resolver's
/// annotations, so it needs no scope walk of its own.
///
/// The key begins with [`KEY_FORMAT_VERSION`]; see that constant for the bump
/// rule that keeps a changed encoding from reusing a stale build.
pub fn content_key(program: &Program) -> Vec<u64> {
    let mut k = KeyWriter { key: Vec::new() };
    k.u(KEY_FORMAT_VERSION);
    k.program(program);
    k.key
}

struct KeyWriter {
    key: Vec<u64>,
}

impl KeyWriter {
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
        self.u(binder.map(|b| b as u64).unwrap_or(UNRESOLVED));
    }

    /// The program shape and its top level.  A record program (a module) is
    /// signed as fields (`pub`/field-identity significant), a tail program as
    /// plain statements (the compiler folds `pub` away).  The statement count
    /// makes the top-level list self-delimiting.
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

    /// A statement (a tail program's — `pub` dropped).
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

    /// A record program's statement — a field (named or positional, `pub`
    /// significant), exactly how `compile_record_fields` lowers it.
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

    /// One expression: its own tag, then its fields.  Tags are unique across
    /// [`Expr`] variants and every list writes its length, so the encoding is
    /// injective; adding a variant or changing a tag bumps
    /// [`KEY_FORMAT_VERSION`].
    ///
    /// `#[stacksafe]`: this walk recurses one frame per nested expression
    /// (through [`Self::opt_expr`] and [`Self::stmt`] and back here) on the
    /// caller's thread, so it grows the stack instead of overflowing it.
    #[stacksafe]
    fn expr(&mut self, e: &Expr) {
        match e {
            Expr::Int(n, _) => {
                self.u(0);
                self.u(*n as u64);
            }
            Expr::Str(s, _) => {
                self.u(24);
                self.str(s);
            }
            Expr::TypeConst(c, _) => {
                self.u(1);
                self.u(match c {
                    crate::ast::TypeConst::Int => 0,
                    crate::ast::TypeConst::Type => 1,
                    crate::ast::TypeConst::String => 2,
                });
            }
            Expr::Name(_name, _, binder) => {
                self.u(2);
                self.opt_binder(binder);
            }
            Expr::Placeholder(_) => self.u(3),
            // The recovered-error region / an unresolved name: content-free,
            // position-only.
            Expr::Err { .. } => self.u(4),
            Expr::TypeOf(_) => self.u(26),
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
            Expr::RawIndex {
                container, index, ..
            } => {
                self.u(27);
                self.expr(container);
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
            Expr::RawNamedField {
                container, name, ..
            } => {
                self.u(28);
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
                ..
            } => {
                self.u(16);
                self.expr(value);
                self.opt_expr(r#type);
                self.opt_expr(perspective);
                self.opt_expr(doc);
            }
            Expr::Arrow {
                parameter,
                r#return,
                ..
            } => {
                self.u(17);
                self.expr(parameter);
                self.expr(r#return);
            }
            Expr::Tuple(elems, _) => {
                self.u(29);
                self.u(elems.len() as u64);
                for el in elems {
                    self.expr(el);
                }
            }
            Expr::TypeTuple(elems, _) => {
                self.u(30);
                self.u(elems.len() as u64);
                for el in elems {
                    self.expr(el);
                }
            }
            Expr::Array(elems, _) => {
                self.u(31);
                self.u(elems.len() as u64);
                for el in elems {
                    self.expr(el);
                }
            }
            Expr::StructType(fields, _) => {
                self.u(18);
                self.u(fields.len() as u64);
                for field in fields {
                    self.b(field.name.is_some());
                    if let Some(name) = &field.name {
                        self.str(name);
                    }
                    self.expr(&field.ty);
                }
            }
            Expr::StructInst { callee, fields, .. } => {
                self.u(19);
                self.expr(callee);
                self.u(fields.len() as u64);
                for f in fields {
                    self.b(f.name.is_some());
                    if let Some(name) = &f.name {
                        self.str(name);
                    }
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
            Expr::TypeArray {
                element_type,
                length,
                ..
            } => {
                self.u(22);
                self.expr(element_type);
                self.expr(length);
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
