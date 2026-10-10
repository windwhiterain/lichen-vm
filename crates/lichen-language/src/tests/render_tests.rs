use super::*;
use crate::diag::Stage;

use lichen_highlevel::attr::NoAttr;
use lichen_highlevel::checker::Checker;
use lichen_highlevel::diagnostic::DiagKind;
use lichen_highlevel::ir::{ExprKind, IR};
use lichen_highlevel::program::{
    Ctx, HighProgramOperator, IntLit, IntTypeLit, LiteralBuild, LiteralExt, ProgramImpl,
    TypeTypeLit, TypeValue, ValueType,
};
use lichen_lowlevel::{LowValue, Program, ValueExt};

#[test]
fn renders_the_offending_line_with_a_caret() {
    // `y` is at line 1, column 6 — the caret lands under it.
    let diag = crate::diag::Diag::<crate::program::LangProgram>::new(
        Stage::Resolve,
        (1, 6),
        "unresolved name 'y'".to_string(),
    );
    let out = render("x => y", &diag);
    assert_eq!(
        out,
        "error: unresolved name 'y'\n  --> 1:6\n   |\n 1 | x => y\n   |      ^\n"
    );
}

#[test]
fn a_spanless_diagnostic_has_no_caret() {
    let diag = crate::diag::Diag::<crate::program::LangProgram> {
        span: None,
        message: "internal".to_string(),
        stage: Stage::Check,
        file: None,
        related: None,
        check: None,
    };
    assert_eq!(render("x", &diag), "error: internal\n");
}

#[test]
fn a_node_the_module_no_longer_holds_renders_without_panicking() {
    // A released node is a documented state: answer "no answer", don't index it.
    let report = crate::compile("x => x");
    let mut build = report.build.expect("the program checks");
    let stale = build.root_ty;
    build
        .module
        .nodes
        .remove(stale)
        .expect("the root type node is live");
    assert_eq!(crate::render::print_type_lang(&build.module, stale), "?");
}

#[test]
fn a_checker_message_uses_the_cli_type_syntax() {
    // The message uses the CLI's type spellings, not the raw `TypeInt → TypeInt`.
    let report = crate::compile("5 : Int -> Int");
    assert_eq!(
        report.diagnostics[0].message,
        "expected Int -> Int, found Int"
    );
}

#[test]
fn an_array_element_conflict_renders_undecided_arrow_cells() {
    // The found side is the lambda's arrow shape, its two cells sharing one name.
    let report = crate::compile("[1, x => x]");
    assert_eq!(
        report.diagnostics[0].message,
        "expected Int, found ?a -> ?a"
    );
}

#[test]
fn a_struct_conflict_keeps_the_nominal_ids() {
    // Two source occurrences are different nominal types, so the message keeps
    // each side's nominal id.
    let report = crate::compile(
        "s1 = struct<.f Int, .g Int>; s2 = struct<.f Int, .g Int>; [s1(1, 2), s2(1, 2)]",
    );
    let message = &report.diagnostics[0].message;
    assert!(message.contains("struct<.f Int, .g Int>#"), "{}", message);
}

#[test]
fn a_failed_assert_renders_its_message() {
    // The condition resolves to 0: a failed assert, not a unify.
    let report = crate::compile("@assert (1 == 2)");
    assert_eq!(report.diagnostics.len(), 1);
    let d = &report.diagnostics[0];
    assert_eq!(d.stage, Stage::Check);
    let check = d.check.as_ref().expect("a checker diagnostic");
    assert_eq!(check.kind, DiagKind::Assert);
    assert_eq!(d.message, "assertion failed: expected 1, found 0");
    assert_eq!(d.span, Some((1, 1)));
}

// --- the type-chain-driven value rendering ------------------------------

/// Run `source` and return its rendered `value: type` output.
fn output(source: &str) -> String {
    crate::run::evaluate(source).expect("the program runs clean")
}

#[test]
fn a_struct_type_value_renders_in_type_syntax() {
    // The raw shape is `[Int, Type]`; read against its kind it prints as source.
    assert_eq!(
        output("A = struct<.f Int, .t Type>\nA"),
        "struct<.f Int, .t Type>: TypeStruct"
    );
}

#[test]
fn a_struct_instance_renders_its_field_tuple() {
    assert_eq!(
        output("A = struct<.f Int, .t Type>\na = A(1, Int)\n(A, a, a.f, a.t)"),
        "(struct<.f Int, .t Type>, (1, Int), 1, Int): <TypeStruct, struct<.f Int, .t Type>, Int, Type>"
    );
}

#[test]
fn a_single_field_struct_instance_keeps_the_tuple_comma() {
    // A one-element tuple's comma `(1,)` shows even though the source `B(1)` has none.
    assert_eq!(
        output("B = struct<.f Int>\nb = B(1,)\n(B, b)"),
        "(struct<.f Int>, (1,)): <TypeStruct, struct<.f Int>>"
    );
}

#[test]
fn a_tuple_value_renders_with_parens() {
    // The type says tuple, so the value reads as the source tuple, not
    // the raw `[1, Int]` array layout.
    assert_eq!(output("(1, Int)"), "(1, Int): <Int, Type>");
}

#[test]
fn an_array_value_keeps_brackets() {
    assert_eq!(output("[1, 2, 3]"), "[1, 2, 3]: array<Int, 3>");
}

#[test]
fn a_compound_type_value_renders_in_type_syntax() {
    // A type expression's value is the type, so it reads in type syntax.
    assert_eq!(output("<Int, Type>"), "<Int, Type>: TypeTuple");
    assert_eq!(output("array<Int, 3>"), "array<Int, 3>: TypeArray");
}

#[test]
fn a_written_arrow_is_a_function() {
    // A type-position arrow lowers to a real function whose type is its signature.
    assert_eq!(output("Int -> Int"), "Function: Int -> Int");
}

#[test]
fn a_raw_index_reads_a_type_component() {
    // `X<e>` reads component `e` of a tuple type value, refused for another kind.
    assert_eq!(output("<Int, string><0>"), "Int: Type");
    assert_eq!(output("<Int, string><1>"), "string: Type");
    assert_eq!(output("<Int, string, Type><0>"), "Int: Type");
}

#[test]
fn a_raw_named_read_yields_the_field_type() {
    // `X::a` reads the named field's type; `.a` reads the instance's field value.
    assert_eq!(output("S = struct<.a Int, .b string>\nS::a"), "Int: Type");
    assert_eq!(
        output("S = struct<.a Int, .b string>\nS::b"),
        "string: Type"
    );
    assert_eq!(
        output("S = struct<.a Int, .b string>\ns = S(.a 1, .b \"h\")\ns.a"),
        "1: Int"
    );
}

#[test]
fn a_type_second_slot_does_not_collapse_an_array() {
    // The raw `[head, K]` heuristic would drop the head; the type chain keeps both.
    assert_eq!(output("(1, Type)"), "(1, Type): <Int, Type>");
}

#[test]
fn a_function_value_prints_function() {
    assert_eq!(output("x => x"), "Function: ?a -> ?a");
    assert_eq!(output("f = x => x\nf"), "Function: ?a -> ?a");
}

// --- `type_of` as an ordinary lichen function ----------------------------

// `type_of` is the standard library's lambda, spelled locally because `output`
// has no package store to import from.

/// `source` with the standard library's `type_of` bound ahead of it.
fn with_type_of(source: &str) -> String {
    output(&format!("type_of = x => {{t = _; x: t; t}}\n{source}"))
}

#[test]
fn type_of_reads_the_operands_type() {
    // The spaced and juxtaposed spellings are ordinary application.
    assert_eq!(with_type_of("type_of (1)"), "Int: Type");
    assert_eq!(with_type_of("type_of 1"), "Int: Type");
    assert_eq!(
        with_type_of("(type_of (1), type_of Int, type_of Type)"),
        "(Int, Type, Type): <Type, Type, Type>"
    );
}

#[test]
fn type_of_is_first_class() {
    // Bindable, passable — application is the whole story.
    assert_eq!(with_type_of("f = type_of\nf 1"), "Int: Type");
    assert_eq!(with_type_of("g = x => type_of x\ng 2"), "Int: Type");
    assert_eq!(with_type_of("type_of (x => x)"), "Function: ?a -> ?a");
}

#[test]
fn type_of_reads_compound_types() {
    assert_eq!(with_type_of("type_of [1, 2]"), "array<Int, 2>: TypeArray");
    assert_eq!(with_type_of("type_of (1, Int)"), "<Int, Type>: TypeTuple");
    assert_eq!(with_type_of("type_of (type_of (1))"), "Type: Type");
}

#[test]
fn a_value_annotates_against_its_own_type_of() {
    // `type_of e` in a type position IS the operand's type expression.
    assert_eq!(with_type_of("5 : type_of (5)"), "5: Int");
    assert_eq!(with_type_of("type_of (1) : Type"), "Int: Type");
}

// --- the extended vocabulary --------------------------------------------

// A probe extension: a type constant beyond the highlevel's vocabulary, added
// through the `enum_ext!` path.
lichen_utils::enum_ext! {
    #[derive(Debug, Clone, Copy, PartialEq)]
    pub enum ProbeValue {
        FloatType,
    }
    + LowValue as LowValue;
    + TypeValue as TypeValue;
}

impl ValueExt for ProbeValue {
    fn is_handle(&self) -> bool {
        false
    }
}

impl ValueType for ProbeValue {
    fn int_marker() -> Self {
        Self::TypeValue(TypeValue::TypeInt)
    }
    fn string_marker() -> Self {
        Self::TypeValue(TypeValue::TypeString)
    }
    fn type_marker() -> Self {
        Self::TypeValue(TypeValue::TypeType)
    }
    fn function_type_marker() -> Self {
        Self::TypeValue(TypeValue::TypeFunction)
    }
    fn tuple_type_marker() -> Self {
        Self::TypeValue(TypeValue::TypeTuple)
    }
    fn array_type_marker() -> Self {
        Self::TypeValue(TypeValue::TypeArray)
    }
    fn type_struct_marker() -> Self {
        Self::TypeValue(TypeValue::TypeStruct)
    }
    fn table_type_marker() -> Self {
        Self::TypeValue(TypeValue::TypeTable)
    }
    fn type_id(&self) -> Option<usize> {
        match self {
            Self::TypeValue(TypeValue::TypeId(n)) => Some(*n),
            _ => None,
        }
    }
    fn type_id_value(n: usize) -> Self {
        Self::TypeValue(TypeValue::TypeId(n))
    }
}

// The probe's literal vocabulary, mirroring the value-vocabulary extension.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FloatLit;

impl<P> LiteralExt<P> for FloatLit
where
    P: Program,
    P::Value: From<ProbeValue>,
{
    fn build(&self, ctx: &mut dyn Ctx<P>) -> LiteralBuild {
        let value_node = ctx.value_node(P::Value::from(ProbeValue::FloatType));
        let ty = ctx.universe();
        let pair = ctx.pair(value_node, ty);
        LiteralBuild {
            pair,
            value: value_node,
            ty,
        }
    }
}

lichen_utils::enum_ext! {
    #[derive(Debug, Clone, Copy, PartialEq)]
    pub enum ProbeLiteral {
    }
    + IntLit as Int;
    + IntTypeLit as IntType;
    + TypeTypeLit as TypeType;
    + FloatLit as Float;
}

pub type ProbeProgram = ProgramImpl<ProbeValue, HighProgramOperator, NoAttr, ProbeLiteral>;

impl LiteralExt<ProbeProgram> for ProbeLiteral {
    fn build(&self, ctx: &mut dyn Ctx<ProbeProgram>) -> LiteralBuild {
        match self {
            ProbeLiteral::Int(lit) => lit.build(ctx),
            ProbeLiteral::IntType(lit) => lit.build(ctx),
            ProbeLiteral::TypeType(lit) => lit.build(ctx),
            ProbeLiteral::Float(lit) => lit.build(ctx),
        }
    }
}

#[test]
fn an_extended_value_renders_through_the_hook() {
    // The extension's type constant, paired with the universe like `Int`; the hook
    // spells it.
    let mut ir: IR<NoAttr, ProbeLiteral> = IR::new();
    let float_ty = ir.alloc(ExprKind::Literal(ProbeLiteral::Float(FloatLit)));
    ir.set_root(float_ty);
    let build = Checker::<ProbeProgram>::build(ir);
    assert!(build.ok);
    let mut module = build.module;
    let value = module
        .evaluate_node_deep(build.root_val, None)
        .expect("the probe root is decided");
    module.evaluate_node_deep(build.root_ty, None);
    let render_ext = |value: &ProbeValue| match value {
        ProbeValue::FloatType => Some("FloatType".to_string()),
        _ => None,
    };
    let mut printer = ValuePrinter::new_with_ext(&module, Some(&render_ext));
    assert_eq!(printer.print(value, build.root_ty), "FloatType");
    assert_eq!(print_type(&module, build.root_ty), "Type");
}

#[test]
fn an_extended_value_without_a_hook_prints_a_placeholder() {
    // Without a hook the base renderer cannot know the extension's own
    // variant — it degrades to `?` rather than panicking.
    let mut ir: IR<NoAttr, ProbeLiteral> = IR::new();
    let float_ty = ir.alloc(ExprKind::Literal(ProbeLiteral::Float(FloatLit)));
    ir.set_root(float_ty);
    let build = Checker::<ProbeProgram>::build(ir);
    let mut module = build.module;
    let value = module
        .evaluate_node_deep(build.root_val, None)
        .expect("the probe root is decided");
    module.evaluate_node_deep(build.root_ty, None);
    assert_eq!(ValuePrinter::new(&module).print(value, build.root_ty), "?");
}
