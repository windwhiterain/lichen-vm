//! Writing an [`IR`] by hand, with every expression named.
//! See docs/notes/test-harness.md.

use std::collections::HashMap;

use lichen_highlevel::attr::AttrSpec;
use lichen_highlevel::ir::{BinOp, ChildRange, ExprId, ExprKind, IR, PrefixOp};
use lichen_highlevel::program::{
    FloatLit, FloatTypeLit, HighProgramLiteral, IntLit, IntTypeLit, StrLit, StringTypeLit,
    TypeTypeLit,
};

/// The IR under construction, plus the name each expression was written with.
///
/// # Invariant
/// Every allocation goes through a builder that names its result, so a
/// [`Program`] has a name for **every** expression in it — never a node a
/// failure message can only call `ExprId(17)`.
pub struct Writer<A: AttrSpec, L = HighProgramLiteral> {
    ir: IR<A, L>,
    names: HashMap<ExprId, String>,
}

impl<A: AttrSpec, L> Writer<A, L> {
    pub fn new() -> Self {
        Writer {
            ir: IR::new(),
            names: HashMap::new(),
        }
    }

    /// Allocate `kind` under `name`.
    fn named(&mut self, name: &str, kind: ExprKind<L>) -> ExprId {
        let expr = self.ir.alloc(kind);
        self.names.insert(expr, name.to_string());
        expr
    }

    /// `name` — an identifier's use. The name is the expression's own, so a
    /// diagnostic naming it reads as the source did.
    pub fn param(&mut self, name: &str) -> ExprId {
        self.named(name, ExprKind::Parameter)
    }

    /// `name = _` — a block-wide binding's reserved id, filled in by
    /// [`Self::fill`]. A recursion resolves to this id, so it must exist before
    /// the body that refers to it.
    pub fn binding(&mut self, name: &str) -> ExprId {
        let expr = self.named(name, ExprKind::Placeholder);
        self.ir.block_roots.insert(expr);
        expr
    }

    /// Stamp an already-reserved `expr` with its real kind, keeping its name.
    pub fn fill(&mut self, expr: ExprId, kind: ExprKind<L>) {
        self.ir.set_kind(expr, kind);
    }

    /// `name = _` — an inference hole, distinct from [`Self::binding`]: a hole
    /// is never reserved for a forward reference.
    pub fn hole(&mut self, name: &str) -> ExprId {
        self.named(name, ExprKind::Placeholder)
    }

    /// A recovered-error region the checker skips whole.
    pub fn error_block(&mut self, name: &str) -> ExprId {
        self.named(name, ExprKind::ErrorBlock)
    }

    // --- functions and application ----------------------------------------

    /// `name = \p. body`.
    pub fn lam(&mut self, name: &str, parameter: ExprId, body: ExprId) -> ExprId {
        self.lam_typed(name, parameter, None, body, false)
    }

    /// `name = \p @loop. body` — the marker is **permission**, so the build
    /// still decides whether a loop is emitted.
    pub fn lam_looping(&mut self, name: &str, parameter: ExprId, body: ExprId) -> ExprId {
        self.lam_typed(name, parameter, None, body, true)
    }

    /// `name = \p : T. body` — the annotated parameter's type.
    pub fn lam_typed(
        &mut self,
        name: &str,
        parameter: ExprId,
        parameter_type: Option<ExprId>,
        body: ExprId,
        looping: bool,
    ) -> ExprId {
        self.named(
            name,
            ExprKind::Function {
                parameter,
                parameter_type,
                parameter_attribute: None,
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
        &mut self,
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
    pub fn app(&mut self, name: &str, function: ExprId, argument: ExprId) -> ExprId {
        self.named(name, ExprKind::Apply { function, argument })
    }

    // --- reads ------------------------------------------------------------

    /// `name = a[i]` — the container is pinned to an array.
    pub fn index(&mut self, name: &str, array: ExprId, index: ExprId) -> ExprId {
        self.named(name, ExprKind::Index { array, index })
    }

    /// `name = a(k)` — a positional slot read over a tuple element.
    pub fn field(&mut self, name: &str, container: ExprId, key: ExprId) -> ExprId {
        self.named(name, ExprKind::Field { container, key })
    }

    /// `name = a.<field>` — the named form, which also accepts a struct.
    pub fn named_field(&mut self, name: &str, container: ExprId, field: &'static str) -> ExprId {
        self.named(name, ExprKind::NamedField { container, name: field })
    }

    /// `name = a <== k` — a table read.
    pub fn find(&mut self, name: &str, container: ExprId, key: ExprId) -> ExprId {
        self.named(name, ExprKind::Find { container, key })
    }

    // --- annotations and operators ----------------------------------------

    /// `name = e : T`.
    pub fn annotate(&mut self, name: &str, value: ExprId, ty: ExprId) -> ExprId {
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
        &mut self,
        name: &str,
        value: ExprId,
        ty: Option<ExprId>,
        attributes: &[ExprId],
    ) -> ExprId {
        let start = self.ir.children.len() as u32;
        self.ir.children.extend_from_slice(attributes);
        let range = ChildRange {
            start,
            end: self.ir.children.len() as u32,
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
    pub fn binop(&mut self, name: &str, operator: BinOp, left: ExprId, right: ExprId) -> ExprId {
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
    pub fn prefix(&mut self, name: &str, operator: PrefixOp, operands: &[ExprId]) -> ExprId {
        let start = self.ir.children.len() as u32;
        self.ir.children.extend_from_slice(operands);
        let range = ChildRange {
            start,
            end: self.ir.children.len() as u32,
        };
        self.named(name, ExprKind::Prefix { operator, operands: range })
    }

    /// `name = assert(condition)`.
    pub fn assert(&mut self, name: &str, condition: ExprId) -> ExprId {
        self.named(name, ExprKind::Assert { condition })
    }

    // --- compound values and their types ---------------------------------

    /// `name = (a, b, …)`.
    pub fn tuple(&mut self, name: &str, elements: &[ExprId]) -> ExprId {
        self.range(name, elements, ExprKind::Tuple)
    }

    /// `name = [T1, …, Tn]` — a tuple type.
    pub fn type_tuple(&mut self, name: &str, elements: &[ExprId]) -> ExprId {
        let range = self.push_children(elements);
        self.named(name, ExprKind::TypeTuple(range))
    }

    /// `name = [a, b, …]` — an array literal, homogeneous by construction.
    pub fn array(&mut self, name: &str, elements: &[ExprId]) -> ExprId {
        let range = self.push_children(elements);
        self.named(name, ExprKind::Array(range))
    }

    /// `name = Array(T, n)`.
    pub fn type_array(&mut self, name: &str, element_type: ExprId, length: ExprId) -> ExprId {
        self.named(
            name,
            ExprKind::TypeArray {
                element_type,
                length,
            },
        )
    }

    /// `name = set{a, b, …}`.
    pub fn set(&mut self, name: &str, members: &[ExprId]) -> ExprId {
        let range = self.push_children(members);
        self.named(name, ExprKind::Set(range))
    }

    /// `name = set{T}`.
    pub fn type_set(&mut self, name: &str, element: ExprId) -> ExprId {
        self.range(name, &[element], ExprKind::TypeSet)
    }

    /// `name = table{K, V}`.
    pub fn type_table(&mut self, name: &str, key: ExprId, value: ExprId) -> ExprId {
        self.range(name, &[key, value], ExprKind::TypeTable)
    }

    /// `name = table(k ==> v, …)` — keys and values interleaved.
    pub fn table(&mut self, name: &str, entries: &[(ExprId, ExprId)]) -> ExprId {
        let interleaved: Vec<ExprId> = entries
            .iter()
            .flat_map(|&(key, value)| [key, value])
            .collect();
        self.range(name, &interleaved, ExprKind::Table)
    }

    /// `name = struct{.a T, .b U}` — the fresh nominal id lives in the kind,
    /// never in the shape.
    pub fn struct_type(&mut self, name: &str, fields: &[(ExprId, &'static str)]) -> ExprId {
        let elements: Vec<ExprId> = fields.iter().map(|&(element, _)| element).collect();
        let field_range = self.push_children(&elements);
        let start = self.ir.struct_names.len() as u32;
        self.ir
            .struct_names
            .extend(fields.iter().map(|&(_, field)| Some(field)));
        let name_range = ChildRange {
            start,
            end: self.ir.struct_names.len() as u32,
        };
        self.named(
            name,
            ExprKind::TypeStruct {
                fields: field_range,
                names: name_range,
            },
        )
    }

    /// `name = S(a, b, …)` — `names` is index-aligned with `value`'s elements.
    pub fn instantiate(
        &mut self,
        name: &str,
        type_expr: ExprId,
        value: ExprId,
        names: &[Option<&'static str>],
    ) -> ExprId {
        let start = self.ir.struct_names.len() as u32;
        self.ir.struct_names.extend_from_slice(names);
        let name_range = ChildRange {
            start,
            end: self.ir.struct_names.len() as u32,
        };
        self.named(
            name,
            ExprKind::Instantiate {
                type_expr,
                value,
                names: name_range,
            },
        )
    }

    /// `name = { bindings }` — a block whose fields are its bindings.
    pub fn record(&mut self, name: &str, value: ExprId, names: &[Option<&'static str>]) -> ExprId {
        let start = self.ir.struct_names.len() as u32;
        self.ir.struct_names.extend_from_slice(names);
        let name_range = ChildRange {
            start,
            end: self.ir.struct_names.len() as u32,
        };
        self.named(name, ExprKind::Record { value, names: name_range })
    }

    /// `name = T -> U` — a function type.
    pub fn type_function(&mut self, name: &str, parameter: ExprId, return_type: ExprId) -> ExprId {
        self.named(
            name,
            ExprKind::TypeFunction {
                parameter,
                return_type,
            },
        )
    }

    /// `name = @gc e` — a scope of its own, released once `e` is decided.
    pub fn gc(&mut self, name: &str, inner: ExprId) -> ExprId {
        self.named(name, ExprKind::Gc { inner })
    }

    /// `$op(args…)` — a native call the compiling module's plugin resolves.
    pub fn native_call(&mut self, name: &str, op: &'static str, args: &[ExprId]) -> ExprId {
        let range = self.push_children(args);
        self.named(name, ExprKind::NativeCall { op, args: range })
    }

    /// `name = [v1, @shallow v2, @shallow(2) v3]` — each element carries its `~`
    /// depth, and the whole is typed like a tuple rather than an array.
    pub fn shallow_array(&mut self, name: &str, elements: &[(ExprId, usize)]) -> ExprId {
        let element_range = self.push_children(
            &elements
                .iter()
                .map(|&(element, _)| element)
                .collect::<Vec<_>>(),
        );
        let start = self.ir.depths.len() as u32;
        self.ir
            .depths
            .extend(elements.iter().map(|&(_, depth)| depth));
        let depth_range = ChildRange {
            start,
            end: self.ir.depths.len() as u32,
        };
        self.named(
            name,
            ExprKind::ShallowArray {
                range: element_range,
                depths: depth_range,
            },
        )
    }

    // --- finishing --------------------------------------------------------

    /// Make `root` the program's root and hand back the IR with its names.
    pub fn finish(self, root: ExprId) -> Program<A, L> {
        let Writer { mut ir, names } = self;
        ir.set_root(root);
        Program { ir, names, root }
    }

    /// The IR so far, for a test that has to stamp something the builders do
    /// not cover.
    pub fn ir_mut(&mut self) -> &mut IR<A, L> {
        &mut self.ir
    }

    fn push_children(&mut self, elements: &[ExprId]) -> ChildRange {
        let start = self.ir.children.len() as u32;
        self.ir.children.extend_from_slice(elements);
        ChildRange {
            start,
            end: self.ir.children.len() as u32,
        }
    }

    fn range(&mut self, name: &str, elements: &[ExprId], make: fn(ChildRange) -> ExprKind<L>) -> ExprId {
        let range = self.push_children(elements);
        self.named(name, make(range))
    }
}

impl<A: AttrSpec> Writer<A, HighProgramLiteral> {
    /// `name = <n>`.
    pub fn int(&mut self, name: &str, value: usize) -> ExprId {
        self.literal(name, HighProgramLiteral::from(IntLit(value)))
    }

    /// `name = <value>` — a float literal.
    pub fn float(&mut self, name: &str, value: f32) -> ExprId {
        self.literal(name, HighProgramLiteral::from(FloatLit(value)))
    }

    /// `name = "text"`.
    pub fn string(&mut self, name: &str, value: &'static str) -> ExprId {
        self.literal(name, HighProgramLiteral::from(StrLit(value)))
    }

    /// `name = Int`.
    pub fn int_type(&mut self, name: &str) -> ExprId {
        self.literal(name, HighProgramLiteral::from(IntTypeLit))
    }

    /// `name = Float`.
    pub fn float_type(&mut self, name: &str) -> ExprId {
        self.literal(name, HighProgramLiteral::from(FloatTypeLit))
    }

    /// `name = string`.
    pub fn string_type(&mut self, name: &str) -> ExprId {
        self.literal(name, HighProgramLiteral::from(StringTypeLit))
    }

    /// `name = Type` — the universe, which is its own type.
    pub fn universe(&mut self, name: &str) -> ExprId {
        self.literal(name, HighProgramLiteral::from(TypeTypeLit))
    }

    fn literal(&mut self, name: &str, lit: HighProgramLiteral) -> ExprId {
        self.named(name, ExprKind::Literal(lit))
    }
}

impl<A: AttrSpec, L> Default for Writer<A, L> {
    fn default() -> Self {
        Writer::new()
    }
}

/// A finished program: the IR, the root, and every expression's name.
pub struct Program<A: AttrSpec, L = HighProgramLiteral> {
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
