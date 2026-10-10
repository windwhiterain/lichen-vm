//! Deferred unification of a struct type built from a partially applied type
//! function. See eval-before-unify.md §3.

use lichen_highlevel::program::TypeValue;
use lichen_lowlevel::{LowValue, Module, NodeId};

use lichen_language::compile;
use lichen_language::program::{LangProgram, LangValue};

/// `type_of` from the standard library, bound ahead of every probed program.
const TYPE_OF: &str = "type_of = x => {t = _; x: t; t}\n";

/// Compile and run a program, asserting it checks; returns the module and the
/// root value node.
fn run(source: &str) -> (Module<LangProgram>, NodeId) {
    let report = compile(source);
    assert!(
        report.ok(),
        "expected {source:?} to check, got: {:?}",
        report.diagnostics
    );
    let build = report.build.unwrap();
    (build.module, build.root_val)
}

fn evaluate(source: &str) -> LangValue {
    let (mut module, root) = run(source);
    module
        .evaluate_node_deep(root, None)
        .expect("the program's root value is undecided")
}

fn usize_of(value: &LangValue) -> usize {
    let LangValue::LowValue(LowValue::USize(n)) = value else {
        panic!("expected a usize value, got {value:?}");
    };
    *n
}

/// The silent half: the field type used to merge against the argument and stay
/// undecided, reading back as a raw layout.
#[test]
fn a_deferred_field_read_binds_the_type_value() {
    assert_eq!(
        usize_of(&evaluate(&format!(
            "{TYPE_OF}{}",
            r#"
P = ins => struct<.I ins.x>
y = (P _)(.I Int)
T = type_of y
T::I == Type
"#
        ))),
        1
    );
}

/// The error half: both reads of one undecided placeholder used to fail the
/// construction.
#[test]
fn two_deferred_field_reads_both_bind() {
    let source = |field: &str| {
        format!(
            "{TYPE_OF}
P = ins => struct<.I ins.x, .O ins.y>
y = (P _)(.I Int, .O Int)
T = type_of y
T::{field} == Type
"
        )
    };
    assert_eq!(usize_of(&evaluate(&source("I"))), 1);
    assert_eq!(usize_of(&evaluate(&source("O"))), 1);
}

/// A struct type value in the slot: its kind carries a names table, never a
/// skeleton.
#[test]
fn a_deferred_field_read_binds_a_struct_type_value() {
    assert_eq!(
        usize_of(&evaluate(&format!(
            "{TYPE_OF}{}",
            r#"
S1 = struct<.a Int>
P = ins => struct<.I ins.x>
y = (P _)(.I S1)
T = type_of y
T::I == type_of S1
"#
        ))),
        1
    );
}

/// `type_of` is a lambda, so the stall's pending side is a lazy Apply, not an
/// Index read: the gate must cover calls too.
#[test]
fn a_deferred_type_of_call_binds_the_type_value() {
    assert_eq!(
        usize_of(&evaluate(&format!(
            "{TYPE_OF}{}",
            r#"
S1 = struct<.a Int>
P = ins => struct<.I type_of ins.x>
y = (P _)(.I S1)
T = type_of y
T::I == type_of S1
"#
        ))),
        1
    );
}

/// `Type` is the universe marker, the vocabulary every comparison above
/// assumes.
#[test]
fn the_type_constant_is_the_universe_marker() {
    assert_eq!(evaluate("Type"), LangValue::TypeValue(TypeValue::TypeType));
}
