//! P0-c probe: a type-parameter-first native generic's surface.

use lichen_highlevel::ir::{ExprId, Loc};
use lichen_highlevel::native::{NativeApply, NativeArg, NativeOp};
use lichen_highlevel::program::Ctx;
use lichen_language::package::PackageStore;
use lichen_language::persist::{NoPersist, ProgramCodecOf};

/// A host over the bare language plus the std-native plugin, as `std_native.rs`.
mod host {
    #![allow(dead_code)]
    lichen_language::lang_compose_vocabulary! {
        attrs = [
            lichen_perspective::Perspective as Perspective;
            lichen_doc::Doc as Doc;
        ]
        [ P::Operator: From<lichen_perspective::GcdOp> ];
        values = [
            lichen_lowlevel::LowValue as LowValue;
            lichen_highlevel::program::TypeValue as TypeValue;
        ];
        operators = [
            lichen_lowlevel::LowOperator as LowOperator;
            lichen_highlevel::program::TypeOperator as TypeOperator;
            lichen_perspective::GcdOp as GcdOp;
        ];
        plugins = [ lichen_std_native as lichen_std_native_leaves; ];
    }
}

use host::{LangOperator, LangValue};

#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq)]
struct HostProgram(
    ::lichen_highlevel::program::ProgramImpl<
        LangValue,
        LangOperator,
        lichen_language::program::LangAttr,
    >,
);

impl lichen_lowlevel::Program for HostProgram {
    type Value = LangValue;
    type Operator = LangOperator;
    type GlobalExt = lichen_highlevel::program::HighGlobalExt;
    type PackageMeta = lichen_highlevel::program::HighPackageMeta;
}

impl lichen_highlevel::program::HighProgram for HostProgram {
    type Attr = lichen_language::program::LangAttr;
    type Literal = lichen_highlevel::program::HighProgramLiteral;
}

impl lichen_lowlevel::OperatorExt<HostProgram> for LangOperator {
    fn run(
        &self,
        operand: <HostProgram as lichen_lowlevel::Program>::Value,
        block: lichen_lowlevel::BlockId,
        module: &mut lichen_lowlevel::Module<HostProgram>,
    ) -> Option<<HostProgram as lichen_lowlevel::Program>::Value> {
        match self {
            LangOperator::LowOperator(op) => op.run(operand, block, module),
            LangOperator::TypeOperator(op) => op.run(operand, block, module),
            LangOperator::GcdOp(op) => op.run(operand, block, module),
            LangOperator::SortOp(op) => op.run(operand, block, module),
        }
    }
}

impl ProgramCodecOf for HostProgram {
    type Codec = NoPersist;
}

impl lichen_language::LangProgramShape for HostProgram {}

/// Hands the argument back and prints what `build` sees.
struct ProbeOp;

impl NativeOp<HostProgram> for ProbeOp {
    fn build(
        &self,
        ctx: &mut dyn Ctx<HostProgram>,
        _e: ExprId,
        args: &[NativeArg],
        _loc: Loc,
    ) -> NativeApply {
        let seen = ctx.node_value(args[0].value);
        println!("[probe] build: the type argument's node value = {seen:?}");
        NativeApply {
            value: args[1].value,
            decided: false,
        }
    }
}

static PROBE: ProbeOp = ProbeOp;
static PROBE_OPS: [(&str, &dyn NativeOp<HostProgram>); 1] = [("probe_sample", &PROBE)];

/// The wrapper is generic over a type argument refined to the declared instances.
const WRAPPER: &str = "{\n\
    scalars := set(Float, Int)\n\
    probe_kind := t => t @in scalars\n\
    sample := (t ! probe_kind) => (x : t) => { r := $probe_sample(t, x); r : t; r }\n\
}\n";

fn new_store() -> PackageStore<HostProgram> {
    let mut store = PackageStore::<HostProgram>::new();
    store
        .register_native("probe.lichen", WRAPPER, &PROBE_OPS)
        .expect("the wrapper must compile and register");
    store
}

/// Run `source`; print its rendered type, or the diagnostics.
fn show(source: &str) -> String {
    let mut store = new_store();
    match lichen_language::run::evaluate_raw(source, None, &mut store) {
        Ok(output) => match output.rsplit_once(": ") {
            Some((_, ty)) => format!("type: {ty}"),
            None => output,
        },
        Err(diags) => format!(
            "<refused> {}",
            diags
                .iter()
                .map(|diag| diag.message.as_str())
                .collect::<Vec<_>>()
                .join(" | ")
        ),
    }
}

#[test]
fn link_probe() {
    let cases = [
        ("a type constant", "p.sample Float 1.5"),
        ("a wrong argument", "p.sample Float 3"),
        ("a kind outside the set", "p.sample string \"a\""),
        ("two instantiations", "(p.sample Float 1.5, p.sample Int 3)"),
        (
            "the same instantiation twice",
            "(p.sample Float 1.5, p.sample Float 2.5)",
        ),
        ("the bare wrapper", "p.sample"),
        ("the inferred wrapper", "(t => x => x) Float 1.5"),
    ];
    for (what, body) in cases {
        let source = format!("--- p = import \"probe.lichen\" ---\n{body}");
        println!("\n=== {what}: {body}\n  -> {}", show(&source));
    }
}
