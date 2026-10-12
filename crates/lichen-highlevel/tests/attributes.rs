//! An annotated program checked with no attribute extension installed;
//! see docs/notes/attributes.md.

use lichen_highlevel::NoAttr;
use lichen_highlevel::attr::{AttrExt, AttrExtRegistry, AttrSet, AttrSpec};
use lichen_highlevel::checker::Checker;
use lichen_highlevel::diagnostic::DiagKind;
use lichen_highlevel::ir::{ExprKind, Loc, Schema};
use lichen_highlevel::program::{
    Ctx, HighProgram, HighProgramLiteral, HighProgramOperator, HighProgramValue, ProgramImpl,
    ValueType,
};
use lichen_lowlevel::{LowValue, NodeId, Registry};
use lichen_testkit::Writer;
use std::sync::{Arc, RwLock};

/// A one-member attribute set: the canonical order is the single-element list.
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
    fn missing_value(&self) -> Option<LowValue> {
        Some(LowValue::USize(0))
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
fn annotated_int() -> lichen_testkit::Program<Tag, HighProgramLiteral> {
    let w: Writer<Tag> = Writer::new();
    let five = w.int("five", 5);
    let tag_value = w.int("tag_value", 4);
    let annotation = w.annotate_with("annotation", five, None, &[tag_value]);
    w.ir_mut()
        .set_schema(annotation, Schema { tail: vec![Tag] });
    w.finish(annotation)
}

#[test]
fn an_annotated_program_without_an_attribute_extension_reports_a_guard() {
    let program = annotated_int();
    let annotation = program.root;
    let build = Checker::<TaggedProgram>::build(program.ir);
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
    // A refused annotation still compiles to the schema's full width; the
    // attribute slot is a fresh undecided hole.
    let program = annotated_int();
    let annotation = program.root;
    let build = Checker::<TaggedProgram>::build(program.ir);
    let pair = build.state[annotation]
        .term
        .expect("the annotation compiled to a pair");
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
    // One build, one missing extension, one diagnostic: the apply's slot
    // check is skipped rather than reported again.
    let w: Writer<Tag> = Writer::new();
    let f = w.binding("f");
    let x = w.param("x");
    w.ir_mut().set_schema(x, Schema { tail: vec![Tag] });
    let declared = w.int("declared", 4);
    w.fill(
        f,
        ExprKind::Function {
            parameter: x,
            parameter_type: None,
            parameter_attribute: Some(declared),
            r#return: x,
            parent: None,
            looping: false,
        },
    );
    let five = w.int("five", 5);
    let call = w.app("call", f, five);
    let build = Checker::<TaggedProgram>::build(w.finish(call).ir);
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
    let program = annotated_int();
    // Single-threaded: filed values hold raw arena handles, so this `Arc` is
    // not `Send`; the builder takes it by value.
    #[allow(clippy::arc_with_non_send_sync)]
    let registry = Arc::new(RwLock::new(Registry::<TaggedProgram>::new()));
    let attr_ext: AttrExtRegistry<TaggedProgram, Tag> = Box::new(|_marker: &Tag| &Tag);
    let build = Checker::<TaggedProgram>::build_in_attr(program.ir, registry, attr_ext);
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
    let w: Writer<NoAttr> = Writer::new();
    let five = w.int("five", 5);
    let build = Checker::<ProgramImpl>::build(w.finish(five).ir);
    assert!(build.ok, "an unannotated program must check");
    assert!(
        build
            .diagnostics()
            .iter()
            .all(|d| d.kind != DiagKind::NoAttributeExtension)
    );
}
