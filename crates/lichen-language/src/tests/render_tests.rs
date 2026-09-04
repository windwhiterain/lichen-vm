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
    let diag = Diag::new(Stage::Resolve, (1, 6), "unresolved name 'y'".to_string());
    let out = render("x => y", &diag);
    assert_eq!(
        out,
        "error: unresolved name 'y'\n  --> 1:6\n   |\n 1 | x => y\n   |      ^\n"
    );
}

#[test]
fn a_spanless_diagnostic_has_no_caret() {
    let diag = Diag {
        span: None,
        message: "internal".to_string(),
        stage: Stage::Check,
        check: None,
    };
    assert_eq!(render("x", &diag), "error: internal\n");
}

#[test]
fn a_checker_message_uses_the_cli_type_syntax() {
    // 5 : Int -> Int — the found type is Int, the expected the arrow
    // type: the same spellings the CLI prints for a program's output,
    // not the raw `TypeInt → TypeInt`.  No `?a` journey line — the user
    // inspects the expression's type directly.
    let report = crate::compile("5 : Int -> Int");
    assert_eq!(
        report.diagnostics[0].message,
        "expected Int -> Int, found Int"
    );
}

#[test]
fn an_array_element_conflict_renders_unbound_arrow_cells() {
    // [1, x => x] — the found side is the lambda's arrow shape with its
    // two unbound cells sharing one name.  No `?a` journey line.
    let report = crate::compile("[1, x => x]");
    assert_eq!(
        report.diagnostics[0].message,
        "expected Int, found ?a -> ?a"
    );
}

#[test]
fn a_struct_conflict_keeps_the_nominal_ids() {
    // Two source occurrences are different nominal types.  The message
    // renders each side's full struct type *with its nominal id*
    // (`struct<Int, Int>#0` vs `#1`), so the two structs stay
    // distinguishable even though their field shapes match.
    let report =
        crate::compile("s1 = struct<Int, Int>; s2 = struct<Int, Int>; [s1(1, 2), s2(1, 2)]");
    let message = &report.diagnostics[0].message;
    assert!(message.contains("struct<Int, Int>#"), "{}", message);
}

#[test]
fn a_failed_assert_renders_its_message() {
    // `!(1 == 2)` — the condition resolves to 0, a failed assert (a runtime
    // evaluation failure, not a unify): the message and the caret at the `!`.
    let report = crate::compile("!(1 == 2)");
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
    // A struct type's value is the raw shape `[TypeId(0), [Int, Type]]` —
    // the lowlevel data layout.  Read against its kind, it prints as the
    // code that produced it.
    assert_eq!(
        output("A = struct<Int, Type>\nA"),
        "struct<Int, Type>: TypeStruct"
    );
}

#[test]
fn a_struct_instance_renders_its_field_tuple() {
    assert_eq!(
        output("A = struct<Int, Type>\na = A(1, Int)\n(A, a, a(0), a(1))"),
        "(struct<Int, Type>, (1, Int), 1, Int): <TypeStruct, struct<Int, Type>, Int, Type>"
    );
}

#[test]
fn a_single_field_struct_instance_keeps_the_tuple_comma() {
    // A single field needs no extra comma in the source (`B(1)`); the
    // rendered value still shows the one-element tuple's comma `(1,)`.
    assert_eq!(
        output("B = struct<Int>\nb = B(1,)\n(B, b)"),
        "(struct<Int>, (1,)): <TypeStruct, struct<Int>>"
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
    assert_eq!(output("[1, 2, 3]"), "[1, 2, 3]: Int<3>");
}

#[test]
fn a_compound_type_value_renders_in_type_syntax() {
    // `Int -> Int` as a value is the shape `[Int, Int]`; read against its
    // kind it prints as the arrow, not the raw pair.
    assert_eq!(output("Int -> Int"), "Int -> Int: TypeFunction");
    assert_eq!(output("<Int, Type>"), "<Int, Type>: TypeTuple");
    assert_eq!(output("Int<3>"), "Int<3>: TypeArray");
}

#[test]
fn a_type_second_slot_does_not_collapse_an_array() {
    // The raw layout's `[head, K]` heuristic reads a two-element array
    // whose second element is the universe as an atomic type pair and
    // drops the head.  The type chain says what the value really is: a
    // tuple keeps both elements.
    assert_eq!(output("(1, Type)"), "(1, Type): <Int, Type>");
}

#[test]
fn a_function_value_prints_function() {
    assert_eq!(output("x => x"), "Function: ?a -> ?a");
    assert_eq!(output("f = x => x\nf"), "Function: ?a -> ?a");
}

// --- the extended vocabulary --------------------------------------------

// A probe extension: a type constant beyond the highlevel's vocabulary,
// composed in one `enum_ext!` invocation that lists every layer's enum
// directly — the path a language crate takes to add its own value
// variants.  The renderers are generic over the vocabulary; the
// extension's own variant renders through the hook both printers carry.
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

// The probe literal vocabulary, mirroring the value-vocabulary extension: the
// highlevel's built-in literal structs compose with a downstream's own.
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
    // `FloatType : Type` — the extension's type constant, paired with the
    // universe like `Int` is.  The value printer (generic over the
    // vocabulary) reads it as an atomic type constant; the extension's
    // own spelling comes from the render hook.
    let mut ir: IR<NoAttr, ProbeLiteral> = IR::new();
    let float_ty = ir.alloc(ExprKind::Literal(ProbeLiteral::Float(FloatLit)), None);
    ir.set_root(float_ty);
    let build = Checker::<ProbeProgram>::build(ir);
    assert!(build.ok);
    let mut module = build.module;
    let value = module.evaluate_node_deep(build.root_val, None);
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
    let float_ty = ir.alloc(ExprKind::Literal(ProbeLiteral::Float(FloatLit)), None);
    ir.set_root(float_ty);
    let build = Checker::<ProbeProgram>::build(ir);
    let mut module = build.module;
    let value = module.evaluate_node_deep(build.root_val, None);
    module.evaluate_node_deep(build.root_ty, None);
    assert_eq!(ValuePrinter::new(&module).print(value, build.root_ty), "?");
}
