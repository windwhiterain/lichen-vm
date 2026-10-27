//! Writing an [`IR`] by hand, with every expression named.
//! See docs/notes/test-harness.md.

use std::cell::{RefCell, RefMut};
use std::collections::HashMap;

use lichen_highlevel::attr::{AttrSpec, NoAttr};
use lichen_highlevel::ir::{BinOp, ChildRange, ExprId, ExprKind, IR, PrefixOp};
use lichen_highlevel::program::{
    FloatLit, FloatTypeLit, FunctionTemplateLit, HighProgramLiteral, IntLit, IntTypeLit, StrLit,
    StringTypeLit, TypeTypeLit,
};

/// The IR under construction, plus the name each expression was written with.
///
/// # Invariant
/// Every allocation goes through a builder that names its result, so a
/// [`Program`] has a name for **every** expression in it — never a node a
/// failure message can only call `ExprId(17)`.
///
/// # Invariant
/// The builders take `&self` over a `RefCell`, which is what lets a test nest
/// them the way it reads: `w.array_inst("k", template, &[w.int("a", 1)])`. An
/// `&mut self` builder forbids that and pushes every sub-expression into a
/// `let` first, which is the whole point of a writer being writer-friendly.
pub struct Writer<A = NoAttr, L = HighProgramLiteral> {
    ir: RefCell<IR<A, L>>,
    names: RefCell<HashMap<ExprId, String>>,
}

impl<A: AttrSpec, L> Writer<A, L> {
    pub fn new() -> Self {
        Writer {
            ir: RefCell::new(IR::new()),
            names: RefCell::new(HashMap::new()),
        }
    }

    /// Allocate `kind` under `name`.
    fn named(&self, name: &str, kind: ExprKind<L>) -> ExprId {
        let expr = self.ir.borrow_mut().alloc(kind);
        self.names.borrow_mut().insert(expr, name.to_string());
        expr
    }

    /// `name` — an identifier's use. The name is the expression's own, so a
    /// diagnostic naming it reads as the source did.
    pub fn param(&self, name: &str) -> ExprId {
        self.named(name, ExprKind::Parameter)
    }

    /// `name = _` — a block-wide binding's reserved id, filled in by
    /// [`Self::close_binding`].
    ///
    /// # Invariant
    /// A recursion resolves to this id, so it must exist before the body that
    /// refers to it.
    pub fn binding(&self, name: &str) -> ExprId {
        let expr = self.named(name, ExprKind::Placeholder);
        self.ir.borrow_mut().block_roots.insert(expr);
        expr
    }

    /// Stamp an already-reserved `expr` with its real kind, keeping its name.
    pub fn fill(&self, expr: ExprId, kind: ExprKind<L>) {
        self.ir.borrow_mut().set_kind(expr, kind);
    }

    /// `name = _` — an inference hole, distinct from [`Self::binding`]: a hole
    /// is never reserved for a forward reference.
    pub fn hole(&self, name: &str) -> ExprId {
        self.named(name, ExprKind::Placeholder)
    }

    /// A recovered-error region the checker skips whole.
    pub fn error_block(&self, name: &str) -> ExprId {
        self.named(name, ExprKind::ErrorBlock)
    }

    // --- functions and application ----------------------------------------

    /// `name = \p. body`.
    pub fn lam(&self, name: &str, parameter: ExprId, body: ExprId) -> ExprId {
        self.lam_typed(name, parameter, None, body, false)
    }

    /// `name = \p @loop. body` — the marker is **permission**, so the build
    /// still decides whether a loop is emitted.
    pub fn lam_looping(&self, name: &str, parameter: ExprId, body: ExprId) -> ExprId {
        self.lam_typed(name, parameter, None, body, true)
    }

    /// Stamp a [`Self::binding`]'s reserved id with its lambda, so a recursion
    /// can name its own id before the body exists.
    ///
    /// # Invariant
    /// The returned id is the reserved one, so a call written against it
    /// resolves to the function this stamps rather than to a fresh sibling.
    pub fn close_binding(
        &self,
        reserved: ExprId,
        parameter: ExprId,
        body: ExprId,
        looping: bool,
    ) -> ExprId {
        self.fill(
            reserved,
            ExprKind::Function {
                parameter,
                parameter_type: None,
                parameter_attribute: None,
                r#return: body,
                parent: None,
                looping,
            },
        );
        reserved
    }

    /// [`Self::close_binding`] with the parameter's annotated type.
    pub fn close_binding_typed(
        &self,
        reserved: ExprId,
        parameter: ExprId,
        parameter_type: ExprId,
        body: ExprId,
        looping: bool,
    ) -> ExprId {
        self.fill(
            reserved,
            ExprKind::Function {
                parameter,
                parameter_type: Some(parameter_type),
                parameter_attribute: None,
                r#return: body,
                parent: None,
                looping,
            },
        );
        reserved
    }

    /// `name = \p : T. body` — the annotated parameter's type.
    pub fn lam_typed(
        &self,
        name: &str,
        parameter: ExprId,
        parameter_type: Option<ExprId>,
        body: ExprId,
        looping: bool,
    ) -> ExprId {
        self.function(name, parameter, parameter_type, None, body, looping)
    }

    /// `name = \p # a. body` — the `x # n => e` spelling, which the frontend
    /// keeps rather than desugaring into a block.
    ///
    /// # Invariant
    /// The slot is the optimization for that desugar: an unannotated body
    /// statement would materialize a block, so a test about the node shape must
    /// reach for this rather than for [`Self::lam`].
    pub fn lam_attributed(
        &self,
        name: &str,
        parameter: ExprId,
        parameter_attribute: ExprId,
        body: ExprId,
    ) -> ExprId {
        self.function(
            name,
            parameter,
            None,
            Some(parameter_attribute),
            body,
            false,
        )
    }

    /// The one place a lambda's kind is written, so the three spellings above
    /// cannot disagree about a field.
    #[allow(clippy::too_many_arguments)]
    fn function(
        &self,
        name: &str,
        parameter: ExprId,
        parameter_type: Option<ExprId>,
        parameter_attribute: Option<ExprId>,
        body: ExprId,
        looping: bool,
    ) -> ExprId {
        self.named(
            name,
            ExprKind::Function {
                parameter,
                parameter_type,
                parameter_attribute,
                r#return: body,
                parent: None,
                looping,
            },
        )
    }

    /// `(inner_b, inner_body)` inside `(outer_b)`, the nesting a closure needs
    /// to name its enclosing function.
    ///
    /// # Invariant
    /// The enclosing id is reserved **before** the inner one is written,
    /// because the inner's parent link names the enclosing node.
    pub fn lam_nested(
        &self,
        inner: (&str, ExprId, ExprId),
        outer: (&str, ExprId),
    ) -> (ExprId, ExprId) {
        let (outer_name, outer_b) = outer;
        let reserved = self.binding(outer_name);
        let (inner_name, inner_b, inner_body) = inner;
        let inner_expr = self.named(
            inner_name,
            ExprKind::Function {
                parameter: inner_b,
                parameter_type: None,
                parameter_attribute: None,
                r#return: inner_body,
                parent: Some(reserved),
                looping: false,
            },
        );
        self.fill(
            reserved,
            ExprKind::Function {
                parameter: outer_b,
                parameter_type: None,
                parameter_attribute: None,
                r#return: inner_expr,
                parent: None,
                looping: false,
            },
        );
        (reserved, inner_expr)
    }

    /// `name = f(x)`.
    pub fn app(&self, name: &str, function: ExprId, argument: ExprId) -> ExprId {
        self.named(name, ExprKind::Apply { function, argument })
    }

    // --- reads ------------------------------------------------------------

    /// `name = a[i]` — the container is pinned to an array.
    pub fn index(&self, name: &str, array: ExprId, index: ExprId) -> ExprId {
        self.named(name, ExprKind::Index { array, index })
    }

    /// `name = a(k)` — a positional slot read over a tuple element.
    pub fn field(&self, name: &str, container: ExprId, key: ExprId) -> ExprId {
        self.named(name, ExprKind::Field { container, key })
    }

    /// `name = a.<field>` — the named form, which also accepts a struct.
    pub fn named_field(&self, name: &str, container: ExprId, field: &'static str) -> ExprId {
        self.named(
            name,
            ExprKind::NamedField {
                container,
                name: field,
            },
        )
    }

    /// `name = a <== k` — a table read.
    pub fn find(&self, name: &str, container: ExprId, key: ExprId) -> ExprId {
        self.named(name, ExprKind::Find { container, key })
    }

    // --- annotations and operators ----------------------------------------

    /// `name = e : T`.
    pub fn annotate(&self, name: &str, value: ExprId, ty: ExprId) -> ExprId {
        self.named(
            name,
            ExprKind::Annotation {
                value,
                r#type: Some(ty),
                attributes: ChildRange::EMPTY,
            },
        )
    }

    /// `name = e : T # attributes…` — the slots come into existence by being
    /// annotated, which is the one asymmetry with `:`.
    pub fn annotate_with(
        &self,
        name: &str,
        value: ExprId,
        ty: Option<ExprId>,
        attributes: &[ExprId],
    ) -> ExprId {
        let start = self.ir.borrow_mut().children.len() as u32;
        self.ir.borrow_mut().children.extend_from_slice(attributes);
        let range = ChildRange {
            start,
            end: self.ir.borrow_mut().children.len() as u32,
        };
        self.named(
            name,
            ExprKind::Annotation {
                value,
                r#type: ty,
                attributes: range,
            },
        )
    }

    /// `name = left op right`.
    pub fn binop(&self, name: &str, operator: BinOp, left: ExprId, right: ExprId) -> ExprId {
        self.named(
            name,
            ExprKind::BinOp {
                operator,
                left,
                right,
            },
        )
    }

    /// `name = <prefix operator> operands…`.
    pub fn prefix(&self, name: &str, operator: PrefixOp, operands: &[ExprId]) -> ExprId {
        let start = self.ir.borrow_mut().children.len() as u32;
        self.ir.borrow_mut().children.extend_from_slice(operands);
        let range = ChildRange {
            start,
            end: self.ir.borrow_mut().children.len() as u32,
        };
        self.named(
            name,
            ExprKind::Prefix {
                operator,
                operands: range,
            },
        )
    }

    /// `name = assert(condition)`.
    pub fn assert(&self, name: &str, condition: ExprId) -> ExprId {
        self.named(name, ExprKind::Assert { condition })
    }

    // --- compound values and their types ---------------------------------

    /// `name = (a, b, …)`.
    pub fn tuple(&self, name: &str, elements: &[ExprId]) -> ExprId {
        self.range(name, elements, ExprKind::Tuple)
    }

    /// `name = set{a, b, …}`.
    pub fn set(&self, name: &str, members: &[ExprId]) -> ExprId {
        let range = self.push_children(members);
        self.named(name, ExprKind::Set(range))
    }

    /// `name = set{T}`.
    pub fn type_set(&self, name: &str, element: ExprId) -> ExprId {
        self.range(name, &[element], ExprKind::TypeSet)
    }

    /// `name = table{K, V}`.
    pub fn type_table(&self, name: &str, key: ExprId, value: ExprId) -> ExprId {
        self.range(name, &[key, value], ExprKind::TypeTable)
    }

    /// `name = table(k ==> v, …)` — keys and values interleaved.
    pub fn table(&self, name: &str, entries: &[(ExprId, ExprId)]) -> ExprId {
        let interleaved: Vec<ExprId> = entries
            .iter()
            .flat_map(|&(key, value)| [key, value])
            .collect();
        self.range(name, &interleaved, ExprKind::Table)
    }

    /// `name = [template, length]` — the array template: the element template
    /// and the array's length.
    ///
    /// # Invariant
    /// The template has exactly two parts: an element template and a length.
    /// An array *value* is an instance of a template, written
    /// `template[…]`.
    pub fn array_template(&self, name: &str, template: ExprId, length: ExprId) -> ExprId {
        self.named(name, ExprKind::Array { template, length })
    }

    /// `name = template[e1, …, en]` — the array instance.
    ///
    /// # Invariant
    /// The instance carries no `~` depth, so its depths range is **empty**:
    /// an unmarked element is the depthless one.
    pub fn array_inst(&self, name: &str, template: ExprId, elements: &[ExprId]) -> ExprId {
        let element_range = self.push_children(elements);
        self.named(
            name,
            ExprKind::ArrayInst {
                template,
                elements: element_range,
                depths: ChildRange::EMPTY,
            },
        )
    }

    /// `name = template[e1, …, en]` — the instance, each element carrying its
    /// `~` depth.
    ///
    /// # Invariant
    /// The depths range holds one depth per element, index-aligned with the
    /// elements: 0 unmarked, `usize::MAX` the bare `~`, n the value slot at
    /// the first n levels of the element's type spine.
    pub fn array_inst_depths(
        &self,
        name: &str,
        template: ExprId,
        elements: &[(ExprId, usize)],
    ) -> ExprId {
        let element_range = self.push_children(
            &elements
                .iter()
                .map(|&(element, _)| element)
                .collect::<Vec<_>>(),
        );
        let depth_start = self.ir.borrow_mut().depths.len() as u32;
        self.ir
            .borrow_mut()
            .depths
            .extend(elements.iter().map(|&(_, depth)| depth));
        let depth_range = ChildRange {
            start: depth_start,
            end: self.ir.borrow_mut().depths.len() as u32,
        };
        self.named(
            name,
            ExprKind::ArrayInst {
                template,
                elements: element_range,
                depths: depth_range,
            },
        )
    }

    /// `name = {.a e, .b e}` — the struct value: each field carries its name.
    ///
    /// # Invariant
    /// The names range is index-aligned with the fields range, and a `None`
    /// name is a field with no name-table entry, which the checker refuses
    /// rather than reads positionally.
    pub fn struct_value(&self, name: &str, fields: &[(ExprId, Option<&'static str>)]) -> ExprId {
        let elements: Vec<ExprId> = fields.iter().map(|&(element, _)| element).collect();
        let field_range = self.push_children(&elements);
        let name_start = self.ir.borrow_mut().struct_names.len() as u32;
        self.ir
            .borrow_mut()
            .struct_names
            .extend(fields.iter().map(|&(_, field)| field));
        let name_range = ChildRange {
            start: name_start,
            end: self.ir.borrow_mut().struct_names.len() as u32,
        };
        self.named(
            name,
            ExprKind::Struct {
                fields: field_range,
                names: name_range,
            },
        )
    }

    /// `name = actual = expected` — the plain unify.
    ///
    /// # Invariant
    /// The actual value comes first and the expected one second: the order is
    /// the unify's, and a reader may not swap them.
    pub fn unify(&self, name: &str, actual: ExprId, expected: ExprId) -> ExprId {
        self.named(name, ExprKind::Unify { actual, expected })
    }

    /// `name = S{…}` — `names` is index-aligned with `value`'s elements.
    pub fn instantiate(
        &self,
        name: &str,
        template: ExprId,
        value: ExprId,
        names: &[Option<&'static str>],
    ) -> ExprId {
        let start = self.ir.borrow_mut().struct_names.len() as u32;
        self.ir.borrow_mut().struct_names.extend_from_slice(names);
        let name_range = ChildRange {
            start,
            end: self.ir.borrow_mut().struct_names.len() as u32,
        };
        self.named(
            name,
            ExprKind::Instantiate {
                template,
                value,
                names: name_range,
            },
        )
    }

    /// `name = { bindings }` — a block whose fields are its bindings.
    pub fn record(&self, name: &str, value: ExprId, names: &[Option<&'static str>]) -> ExprId {
        let start = self.ir.borrow_mut().struct_names.len() as u32;
        self.ir.borrow_mut().struct_names.extend_from_slice(names);
        let name_range = ChildRange {
            start,
            end: self.ir.borrow_mut().struct_names.len() as u32,
        };
        self.named(
            name,
            ExprKind::Record {
                value,
                names: name_range,
            },
        )
    }

    /// `name = p : T => (_ : U)` — a function template.
    pub fn type_function(&self, name: &str, parameter: ExprId, return_type: ExprId) -> ExprId {
        let parameter_name = self.param(&format!("{name}_parameter"));
        let return_value = self.hole(&format!("{name}_return"));
        let body = self.annotate(&format!("{name}_codomain"), return_value, return_type);
        self.lam_typed(name, parameter_name, Some(parameter), body, false)
    }

    /// `name = @gc e` — a scope of its own, released once `e` is decided.
    pub fn gc(&self, name: &str, inner: ExprId) -> ExprId {
        self.named(name, ExprKind::Gc { inner })
    }

    /// `$op(args…)` — a native call the compiling module's plugin resolves.
    pub fn native_call(&self, name: &str, op: &'static str, args: &[ExprId]) -> ExprId {
        let range = self.push_children(args);
        self.named(name, ExprKind::NativeCall { op, args: range })
    }

    /// `expr` — a module-level statement.
    ///
    /// # Invariant
    /// A statement is a root of the module, so the expression's own name is the name the module
    /// binds and a later statement can read.
    pub fn statement(&self, expr: ExprId) -> ExprId {
        self.ir.borrow_mut().stmt_roots.push(expr);
        expr
    }

    /// `name` — a value imported from a package, named by the export it is.
    ///
    /// # Invariant
    /// The ref is the package's exported expr, which the checker materializes: the
    /// payload itself stays in the package's static arena, so the IR carries the
    /// ref and not the value.
    pub fn static_ref(&self, name: &str, export: lichen_lowlevel::StaticNodeId) -> ExprId {
        self.named(name, ExprKind::Static { export })
    }

    // --- finishing --------------------------------------------------------

    /// Make `root` the program's root and hand back the IR with its names.
    pub fn finish(self, root: ExprId) -> Program<A, L> {
        let Writer { ir, names } = self;
        let mut ir = ir.into_inner();
        ir.set_root(root);
        Program {
            ir,
            names: names.into_inner(),
            root,
        }
    }

    /// The IR so far, for a test that has to stamp something the builders do
    /// not cover.
    pub fn ir_mut(&self) -> RefMut<'_, IR<A, L>> {
        self.ir.borrow_mut()
    }

    fn push_children(&self, elements: &[ExprId]) -> ChildRange {
        let start = self.ir.borrow_mut().children.len() as u32;
        self.ir.borrow_mut().children.extend_from_slice(elements);
        ChildRange {
            start,
            end: self.ir.borrow_mut().children.len() as u32,
        }
    }

    fn range(
        &self,
        name: &str,
        elements: &[ExprId],
        make: fn(ChildRange) -> ExprKind<L>,
    ) -> ExprId {
        let range = self.push_children(elements);
        self.named(name, make(range))
    }
}

impl<A: AttrSpec> Writer<A, HighProgramLiteral> {
    /// `name = <n>`.
    pub fn int(&self, name: &str, value: usize) -> ExprId {
        self.literal(name, HighProgramLiteral::from(IntLit(value)))
    }

    /// `name = <value>` — a float literal.
    pub fn float(&self, name: &str, value: f32) -> ExprId {
        self.literal(name, HighProgramLiteral::from(FloatLit(value)))
    }

    /// `name = "text"`.
    pub fn string(&self, name: &str, value: &'static str) -> ExprId {
        self.literal(name, HighProgramLiteral::from(StrLit(value)))
    }

    /// `name = Int`.
    pub fn int_type(&self, name: &str) -> ExprId {
        self.literal(name, HighProgramLiteral::from(IntTypeLit))
    }

    /// `name = Float`.
    pub fn float_type(&self, name: &str) -> ExprId {
        self.literal(name, HighProgramLiteral::from(FloatTypeLit))
    }

    /// `name = Function` — the shared `Function` type. It names no function.
    pub fn function_type(&self, name: &str) -> ExprId {
        self.literal(name, HighProgramLiteral::from(FunctionTemplateLit))
    }

    /// `name = string`.
    pub fn string_type(&self, name: &str) -> ExprId {
        self.literal(name, HighProgramLiteral::from(StringTypeLit))
    }

    /// `name = Type` — the universe, which is its own type.
    pub fn universe(&self, name: &str) -> ExprId {
        self.literal(name, HighProgramLiteral::from(TypeTypeLit))
    }

    fn literal(&self, name: &str, lit: HighProgramLiteral) -> ExprId {
        self.named(name, ExprKind::Literal(lit))
    }
}

impl<A: AttrSpec, L> Default for Writer<A, L> {
    fn default() -> Self {
        Writer::new()
    }
}

/// A finished program: the IR, the root, and every expression's name.
pub struct Program<A = NoAttr, L = HighProgramLiteral> {
    pub ir: IR<A, L>,
    pub names: HashMap<ExprId, String>,
    pub root: ExprId,
}

impl<A: AttrSpec, L> Program<A, L> {
    /// The name expression `expr` was written under, or its id when it was
    /// written by a builder that named it nothing.
    pub fn name_of(&self, expr: ExprId) -> String {
        match self.names.get(&expr) {
            Some(name) => name.clone(),
            None => format!("#{expr:?}"),
        }
    }

    /// The expression written under `name`, or `None` when no expression is.
    ///
    /// # Invariant
    /// Names are **unique per program**: a builder that reuses a name is a
    /// test bug, and resolving to one of the two would silently pick an
    /// arbitrary node.
    pub fn expr_named(&self, name: &str) -> Option<ExprId> {
        self.names
            .iter()
            .find(|(_, written)| written.as_str() == name)
            .map(|(&expr, _)| expr)
    }
}
