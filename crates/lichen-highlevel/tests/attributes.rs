//! An annotated program checked through an entry point that installs no
//! attribute extension: the schema's attribute cannot be lowered, so it is a
//! reported diagnostic plus a well-formed hole — never a panic inside the
//! checker.  This is the `Checker::build` path a host reaches when it composes
//! an attribute set but does not pass the matching extension.

use lichen_highlevel::NoAttr;
use lichen_highlevel::attr::{AttrExt, AttrSet, AttrSpec};
use lichen_highlevel::checker::Checker;
use lichen_highlevel::diagnostic::DiagKind;
use lichen_highlevel::ir::{ExprKind, IR, Loc, Schema};
use lichen_highlevel::program::{
    Ctx, HighProgram, HighProgramLiteral, HighProgramOperator, HighProgramValue, IntLit,
    ProgramImpl, ValueType,
};
use lichen_lowlevel::{LowValue, NodeId, Registry};
use std::sync::{Arc, RwLock};

/// A probe attribute: one member, so the composed set's canonical order is the
/// single-element list and an expression's tail can carry it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Tag;

impl AttrSpec for Tag {}

impl AttrSet for Tag {
    const ORDER: &'static [Self] = &[Tag];

    fn order_index(&self) -> usize {
        0
    }
}

impl<P: HighProgram> AttrExt<P> for Tag
where
    P::Value: ValueType,
{
    fn missing_value(&self) -> LowValue {
        LowValue::USize(0)
    }

    fn combine(&self, ctx: &mut dyn Ctx<P>, children: &[NodeId]) -> NodeId {
        children.first().copied().unwrap_or_else(|| ctx.fresh())
    }

    fn unify_slots(&self, ctx: &mut dyn Ctx<P>, a: NodeId, b: NodeId, loc: Loc) {
        ctx.check_unify(a, b, loc, DiagKind::Attribute);
    }
}

/// The probe program: the built-in vocabularies with the probe attribute set.
type TaggedProgram = ProgramImpl<HighProgramValue, HighProgramOperator, Tag, HighProgramLiteral>;

/// `5 # tag` — an annotated int whose schema tail carries `Tag`.
fn annotated_int() -> (lichen_highlevel::ir::ExprId, IR<Tag, HighProgramLiteral>) {
    let mut ir: IR<Tag, HighProgramLiteral> = IR::new();
    let five = ir.alloc(ExprKind::Literal(HighProgramLiteral::from(IntLit(5))));
    let tag_value = ir.alloc(ExprKind::Literal(HighProgramLiteral::from(IntLit(4))));
    let annotation = ir.alloc_annotation(five, None, &[tag_value]);
    ir.set_schema(annotation, Schema { tail: vec![Tag] });
    ir.set_root(annotation);
    (annotation, ir)
}

#[test]
fn an_annotated_program_without_an_attribute_extension_reports_a_guard() {
    let (annotation, ir) = annotated_int();
    let build = Checker::<TaggedProgram>::build(ir);
    assert!(
        !build.ok,
        "a schema carrying an attribute that cannot be lowered must fail the build"
    );
    let guard = build
        .diagnostics()
        .into_iter()
        .find(|d| d.kind == DiagKind::NoAttributeExtension)
        .expect("the missing attribute extension is a diagnostic");
    assert_eq!(
        guard.loc.as_ref().map(|loc| loc.expr),
        Some(annotation),
        "the guard is attributed to the expression whose schema carries the attribute"
    );
}

#[test]
fn the_attribute_slot_of_an_unlowerable_schema_is_a_well_formed_hole() {
    // The refused expression must still compile to a pair of the schema's
    // width, so the rest of the check reads a slot rather than an absent
    // element.  The slot is a fresh unbound pair — the same hole shape the
    // other check-time guards leave.
    let (annotation, ir) = annotated_int();
    let build = Checker::<TaggedProgram>::build(ir);
    let pair = build.term[annotation.0 as usize].expect("the annotation compiled to a pair");
    // SAFETY: `pair` is a live node of the build under test, whose block has
    // not been dropped.
    let items = unsafe { build.module.array_items(pair) }.expect("the pair is an array");
    assert_eq!(
        items.len(),
        3,
        "the pair keeps the schema's `[value, type, attribute]` arity"
    );
}

#[test]
fn an_annotated_parameter_without_an_attribute_extension_is_reported_once() {
    // `f = x # tag => x; f 5` — the parameter annotation is where the
    // attribute is read, and the apply's slot check is skipped rather than
    // reported again: the build is missing one thing, so it reports one
    // diagnostic however many sites read an attribute.
    let mut ir: IR<Tag, HighProgramLiteral> = IR::new();
    let f = ir.alloc(ExprKind::Placeholder);
    let x = ir.alloc(ExprKind::Parameter);
    ir.set_schema(x, Schema { tail: vec![Tag] });
    let declared = ir.alloc(ExprKind::Literal(HighProgramLiteral::from(IntLit(4))));
    ir.set_kind(
        f,
        ExprKind::Function {
            parameter: x,
            parameter_type: None,
            parameter_attribute: Some(declared),
            r#return: x,
            parent: None,
        },
    );
    let five = ir.alloc(ExprKind::Literal(HighProgramLiteral::from(IntLit(5))));
    let call = ir.alloc(ExprKind::Apply {
        function: f,
        argument: five,
    });
    ir.set_root(call);
    let build = Checker::<TaggedProgram>::build(ir);
    assert!(!build.ok, "the annotated parameter cannot be lowered");
    assert_eq!(
        build
            .diagnostics()
            .iter()
            .filter(|d| d.kind == DiagKind::NoAttributeExtension)
            .count(),
        1,
        "one build, one missing attribute extension, one diagnostic"
    );
}

#[test]
fn an_attribute_aware_build_lowers_the_annotation() {
    // The counterpart of the guard: the same annotated program through
    // `build_in_attr`, where the extension is installed.  The annotation
    // lowers, the slot is the annotation value's pair, and nothing is
    // reported.
    let (_annotation, ir) = annotated_int();
    let registry = Arc::new(RwLock::new(Registry::<TaggedProgram>::new()));
    let attr_ext: Box<dyn Fn(&Tag) -> &'static dyn AttrExt<TaggedProgram>> =
        Box::new(|_marker: &Tag| &Tag);
    let build = Checker::<TaggedProgram>::build_in_attr(ir, registry, attr_ext);
    assert!(
        build.ok,
        "an attribute-aware build must check the annotation"
    );
    assert!(
        build.diagnostics().is_empty(),
        "nothing to report: {:?}",
        build.diagnostics()
    );
}

#[test]
fn a_schema_without_an_attribute_is_unaffected_by_the_missing_extension() {
    // The same entry point on a program whose schemas carry nothing: no
    // extension is ever consulted, so the build checks as before.
    let mut ir: IR<NoAttr, HighProgramLiteral> = IR::new();
    let five = ir.alloc(ExprKind::Literal(HighProgramLiteral::from(IntLit(5))));
    ir.set_root(five);
    let build = Checker::<ProgramImpl>::build(ir);
    assert!(build.ok, "an unannotated program must check");
    assert!(
        build
            .diagnostics()
            .iter()
            .all(|d| d.kind != DiagKind::NoAttributeExtension)
    );
}
