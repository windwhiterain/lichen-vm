//! Probe: the callee-inferred instantiation.

use lichen_language::compile;
use lichen_language::run::evaluate;

/// Print the outcome of a program: its answer or its refusals.
fn probe(label: &str, source: &str) {
    match evaluate(source) {
        Ok(text) => println!("[{label}] answered {text}"),
        Err(diagnostics) => println!(
            "[{label}] refused: {:?}",
            diagnostics
                .iter()
                .map(|diagnostic| format!("{:?}: {}", diagnostic.span, diagnostic.message))
                .collect::<Vec<_>>()
        ),
    }
}

const FULL: &str = "Param = _x => struct{.type _, .length _}\nBufT  = x : (Param _) => struct{.type x.type, .length x.length}\nbuf   = (x : (Param _)) => (BufT x)(.type x.type, .length x.length)\ne     = buf _(.type Float, .length 192)\ne\n";

const NAMED_CALLEE: &str = "Param = _x => struct{.type _, .length _}\nBufT  = x : (Param _) => struct{.type x.type, .length x.length}\nbuf   = (x : (Param _)) => (BufT x)(.type x.type, .length x.length)\ne     = buf ((Param _))(.type Float, .length 192)\ne\n";

const PLAIN_NAMED: &str = "Param = _x => struct{.type _, .length _}\nBufT  = x : (Param _) => struct{.type x.type, .length x.length}\nbuf   = (x : (Param _)) => (BufT x)(.type x.type, .length x.length)\ne     = buf Param(.type Float, .length 192)\ne\n";

const ONLY_FIELD_READS: &str = "Param = _x => struct{.type _, .length _}\nbuf = (x : (Param _)) => (x.type, x.length)\nbuf\n";

const NO_CONTEXT: &str = "Param = _x => struct{.type _, .length _}\ne = _(.type Float, .length 192)\ne\n";

/// A concrete expected type: the struct type is named.
const CONCRETE: &str = "S = struct{.a Type, .b Int}\nf = (x : S) => x\ne = f _(.a Float, .b 192)\ne\n";

/// A concrete expected type, the callee named.
const CONCRETE_NAMED: &str = "S = struct{.a Type, .b Int}\nf = (x : S) => x\ne = f (S(.a Float, .b 192))\ne\n";

/// The type-function expected type, the callee named.
const TYPE_FUNCTION_NAMED: &str = "Param = _x => struct{.type _, .length _}\nf = (x : (Param _)) => x\ne = f ((Param _))(.type Float, .length 192)\ne\n";

/// The type-function expected type, the placeholder callee.
const TYPE_FUNCTION_PLACEHOLDER: &str = "Param = _x => struct{.type _, .length _}\nf = (x : (Param _)) => x\ne = f _(.type Float, .length 192)\ne\n";

/// The type function alone: the builder that reads a value's fields.
const TYPE_FUNCTION_BUILDER: &str = "Param = _x => struct{.type _, .length _}\nBufT = x : (Param _) => struct{.type x.type, .length x.length}\nBufT\n";

/// The type function and the call, without the instantiation.
const TYPE_FUNCTION_CALL: &str = "Param = _x => struct{.type _, .length _}\nBufT = x : (Param _) => struct{.type x.type, .length x.length}\nbuf = (x : (Param _)) => (BufT x)(.type x.type, .length x.length)\nbuf\n";

/// The type function, the call, and the placeholder instantiation.
const TYPE_FUNCTION_FULL: &str = "Param = _x => struct{.type _, .length _}\nBufT = x : (Param _) => struct{.type x.type, .length x.length}\nbuf = (x : (Param _)) => (BufT x)(.type x.type, .length x.length)\ne = buf _(.type Float, .length 192)\ne\n";

/// The same shape without a type function: a field's type is a read of a field.
const SELF_REFERENCE: &str = "S = struct{.a Type}\nT = x : S => struct{.f x.a}\nf = (x : S) => (T x)(.f x.a)\ne = f (S(.a Float))\ne\n";

/// The same shape without the instantiation in the body.
const SELF_REFERENCE_PLAIN: &str = "S = struct{.a Type}\nT = x : S => struct{.f x.a}\nf = (x : S) => x.a\ne = f (S(.a Float))\ne\n";

/// A hole-typed field, a body that reads it, and a call that binds it.
const HOLE_READ_AND_BIND: &str = "S = struct{.a _}\nT = x : S => struct{.f x.a}\nf = (x : S) => (T x)(.f x.a)\ne = f (S(.a Float))\ne\n";

/// The body reads the hole; the call binds it; no instantiation in the body.
const HOLE_READ_AND_BIND_PLAIN: &str = "S = struct{.a _}\nf = (x : S) => x.a\ne = f (S(.a Float))\ne\n";

/// The user's program without the builder: the body reads the holes.
const TYPE_FUNCTION_BODY_READS: &str = "Param = _x => struct{.type _, .length _}\nbuf = (x : (Param _)) => (x.type, x.length)\ne = buf _(.type Float, .length 192)\ne\n";

/// The user's body, with a plain struct type instead of the type function.
const PLAIN_STRUCT_BODY_NAMED: &str = "S = struct{.type _, .length _}\nBufT = x : S => struct{.type x.type, .length x.length}\nbuf = (x : S) => (BufT x)(.type x.type, .length x.length)\ne = buf (S(.type Float, .length 192))\ne\n";

/// The same, with the placeholder callee.
const PLAIN_STRUCT_BODY_PLACEHOLDER: &str = "S = struct{.type _, .length _}\nBufT = x : S => struct{.type x.type, .length x.length}\nbuf = (x : S) => (BufT x)(.type x.type, .length x.length)\ne = buf _(.type Float, .length 192)\ne\n";

/// Two hole-typed fields, a body that reads both, and a call that binds both.
const TWO_FIELDS: &str = "S = struct{.a _, .b _}\nT = x : S => struct{.f x.a, .g x.b}\nf = (x : S) => (T x)(.f x.a, .g x.b)\ne = f (S(.a Float, .b 192))\ne\n";

/// One hole-typed field, read twice.
const ONE_FIELD_TWICE: &str = "S = struct{.a _}\nT = x : S => struct{.f x.a, .g x.a}\nf = (x : S) => (T x)(.f x.a, .g x.a)\ne = f (S(.a Float))\ne\n";

/// Two fields, one of them read.
const TWO_FIELDS_ONE_READ: &str = "S = struct{.a _, .b _}\nT = x : S => struct{.f x.a}\nf = (x : S) => (T x)(.f x.a)\ne = f (S(.a Float, .b 192))\ne\n";

/// A value in a struct field's type position.
const VALUE_IN_TYPE_POSITION: &str = "S = struct{.f 5}\ne = S(.f 5)\ne\n";

/// The user's program with the builder kept and the body call dropped.
const USER_PROGRAM_WITHOUT_BODY_CALL: &str = "Param = _x => struct{.type _, .length _}\nBufT  = x : (Param _) => struct{.type x.type, .length x.length}\nbuf   = (x : (Param _)) => x\ne     = buf _(.type Float, .length 192)\ne\n";

/// The user's reads, applied to a named instance.
const READS_APPLIED: &str = "Param = _x => struct{.type _, .length _}\nbuf = (x : (Param _)) => (x.type, x.length)\nbuf ((Param _)(.type Float, .length 192))\n";

#[test]
fn probe_the_spellings() {
    probe("full", FULL);
    probe("named-callee", NAMED_CALLEE);
    probe("plain-named", PLAIN_NAMED);
    probe("only-field-reads", ONLY_FIELD_READS);
    probe("no-context", NO_CONTEXT);
    probe("concrete", CONCRETE);
    probe("concrete-named", CONCRETE_NAMED);
    probe("type-function-named", TYPE_FUNCTION_NAMED);
    probe("type-function-placeholder", TYPE_FUNCTION_PLACEHOLDER);
    probe("type-function-builder", TYPE_FUNCTION_BUILDER);
    probe("type-function-call", TYPE_FUNCTION_CALL);
    probe("type-function-full", TYPE_FUNCTION_FULL);
    probe("self-reference", SELF_REFERENCE);
    probe("self-reference-plain", SELF_REFERENCE_PLAIN);
    probe("hole-read-and-bind", HOLE_READ_AND_BIND);
    probe("hole-read-and-bind-plain", HOLE_READ_AND_BIND_PLAIN);
    probe("type-function-body-reads", TYPE_FUNCTION_BODY_READS);
    probe("plain-struct-body-named", PLAIN_STRUCT_BODY_NAMED);
    probe("plain-struct-body-placeholder", PLAIN_STRUCT_BODY_PLACEHOLDER);
    probe("two-fields", TWO_FIELDS);
    probe("one-field-twice", ONE_FIELD_TWICE);
    probe("two-fields-one-read", TWO_FIELDS_ONE_READ);
    probe("value-in-type-position", VALUE_IN_TYPE_POSITION);
    probe("user-program-without-body-call", USER_PROGRAM_WITHOUT_BODY_CALL);
    probe("reads-applied", READS_APPLIED);
    let report = compile(FULL);
    println!("[full] diagnostics {:?}", report.diagnostics.len());
    for diagnostic in &report.diagnostics {
        println!(
            "  {:?} stage={:?} check={:?}",
            diagnostic.span,
            diagnostic.stage,
            diagnostic
                .check
                .as_ref()
                .map(|check| check.kind)
        );
    }
}
