//! Probe: at what arity a linked family's entry runs. Prints, asserts nothing.

use lichen_highlevel::ir::{ExprId, Loc};
use lichen_highlevel::native::{NativeApply, NativeArg, NativeOp};
use lichen_highlevel::program::Ctx;
use lichen_language::package::PackageStore;
use lichen_language::persist::{NoPersist, ProgramCodecOf};
use lichen_utils::extend::AsEnum;

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

/// The library id the probe's one family is installed under.
const LIBRARY: u32 = 901;
const INSTANCE: u32 = 0;

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

/// Reads the operand array the way a generated entry does, and sums the integer
/// items it finds.
///
/// # Safety
/// [`lichen_lowlevel::LowOperator::External`]'s entry contract: the module,
/// operand and out pointers name this build's `HostProgram` types.
unsafe extern "C" fn probe_entry(
    module: *mut std::ffi::c_void,
    operand: *const std::ffi::c_void,
    out: *mut std::ffi::c_void,
) {
    let module = unsafe { &mut *(module as *mut lichen_lowlevel::Module<HostProgram>) };
    let operand = unsafe { &*(operand as *const LangValue) };
    let mut sum = 0usize;
    match AsEnum::<lichen_lowlevel::LowValue>::as_enum(operand) {
        Some(lichen_lowlevel::LowValue::Array(array)) => {
            // SAFETY: the operand array is the live value the evaluator handed over.
            let items = unsafe { array.items() };
            println!("    entry: operand is an Array of {} items", items.len());
            for (k, item) in items.iter().enumerate() {
                let read = module.node_value(item.node);
                let low = read
                    .as_ref()
                    .and_then(|value| AsEnum::<lichen_lowlevel::LowValue>::as_enum(value));
                // The expr the item reads: an `Index` read names it as its operand.
                let expr = match item.node {
                    lichen_lowlevel::AnyNodeId::Dynamic(node) => module
                        .node_operation(node)
                        .and_then(|operation| operation.operand),
                    _ => None,
                };
                println!(
                    "    entry: item {k}: shallow={} node={:?} reads_expr={expr:?} owner={:?} read={:?}",
                    item.shallow,
                    item.node,
                    expr.and_then(|expr| module.node_function(expr)),
                    read
                );
                if let Some(lichen_lowlevel::LowValue::USize(n)) = low {
                    sum += n;
                }
            }
        }
        other => println!("    entry: operand is not an Array: {other:?}"),
    }
    unsafe { (out as *mut LangValue).write(lichen_lowlevel::LowValue::USize(sum).into()) };
}

/// The `LinkOp`-shaped operator: `External` over the call's argument array.
struct ArityOp;

impl NativeOp<HostProgram> for ArityOp {
    fn build(
        &self,
        ctx: &mut dyn Ctx<HostProgram>,
        _e: ExprId,
        args: &[NativeArg],
        _loc: Loc,
    ) -> NativeApply {
        let values: Vec<lichen_lowlevel::NodeId> = args.iter().map(|arg| arg.value).collect();
        println!(
            "    build: {} arguments, value nodes {values:?}",
            values.len()
        );
        let operand = ctx.array_node(&values);
        NativeApply {
            value: ctx.op_node(
                LangOperator::from(lichen_lowlevel::LowOperator::External {
                    library: LIBRARY,
                    instance: INSTANCE,
                }),
                Some(operand),
            ),
            decided: false,
        }
    }
}

static ARITY: ArityOp = ArityOp;
static ARITY_OPS: [(&str, &dyn NativeOp<HostProgram>); 1] = [("probe_family", &ARITY)];

/// The wrapper's own bindings, once per form.
///
/// # Invariant
/// `sphere` and `voxels` are the documented surface — one parameter holding a
/// tuple, a type argument beside it (`docs/language-spec.md` §442–448).
/// `one`..`four` are regression readings for an operand of N items.
const FAMILIES: &str = "scalars := set(Float, Int)\n\
    grid_kind := t => t @in scalars\n\
    sphere := args => { r := $probe_family(args); r : Int; r }\n\
    voxels := (t ! grid_kind) => args => { r := $probe_family(t, args); r : Int; r }\n\
    one := (a : Int) => { r := $probe_family(a); r : Int; r }\n\
    two := (a : Int) => (b : Int) => { r := $probe_family(a, b); r : Int; r }\n\
    three := (a : Int) => (b : Int) => (c : Int) => { r := $probe_family(a, b, c); r : Int; r }\n\
    four := (a : Int) => (b : Int) => (c : Int) => (d : Int) => { r := $probe_family(a, b, c, d); r : Int; r }\n";

/// Three struct types, as the linked workbench's wrapper carries them.
const STRUCTS: &str = "GridName := _x => {.path @any : string, .name @any : string}\n\
    WriteSphere := _x => {.radius @any : Float, .voxel @any : Float, .path @any : string, .name @any : string}\n\
    Voxels := _x => {.kind @any : Type, .path @any : string, .name @any : string}\n";

/// Three more families, as the linked workbench's wrapper carries them.
const MORE_FAMILIES: &str = "grid_count := path: string => { r := $probe_family(path); r : Int; r }\n\
    write_sphere := args => { r := $probe_family(args); r : Int; r }\n\
    vdb_stats := args => { r := $probe_family(args); r : Int; r }\n";

/// The wrapper contents the type-first shape is measured against.
///
/// # Invariant
/// Each entry keeps [`FAMILIES`] whole and only adds content the linked
/// workbench's wrapper carries, so the one difference between two entries is
/// the additions named.
const VARIANTS: &[(&str, &[&str])] = &[
    ("base", &[]),
    ("structs", &[STRUCTS]),
    ("families", &[MORE_FAMILIES]),
    ("both", &[STRUCTS, MORE_FAMILIES]),
];

/// The wrapper a variant runs: the base bindings plus what the variant adds.
fn variant_wrapper(additions: &[&str]) -> String {
    format!("{FAMILIES}{}", additions.concat())
}

fn new_store(families: &str) -> PackageStore<HostProgram> {
    let mut store = PackageStore::<HostProgram>::new();
    store
        .register_native("arity.lichen", &format!("{{\n{families}}}\n"), &ARITY_OPS)
        .expect("the wrapper must compile and register");
    store
}

/// The **frozen** path: the wrapper is a native package the program imports.
fn show_frozen(families: &str, body: &str) -> String {
    match build_frozen(families, body) {
        Ok((mut build, _store)) => {
            let value = build.module.evaluate_node_deep(build.root_val, None);
            match value {
                None => "none".to_string(),
                Some(value) => {
                    let (value_text, type_text) = lichen_language::render::print_answer_lang(
                        &build.module,
                        value,
                        build.root_ty,
                    );
                    format!("{value_text}: {type_text}")
                }
            }
        }
        Err(message) => format!("<refused> {message}"),
    }
}

/// The frozen build, kept whole: the module carries the probe's readings.
fn build_frozen(
    families: &str,
    body: &str,
) -> Result<
    (
        lichen_highlevel::checker::Build<HostProgram>,
        PackageStore<HostProgram>,
    ),
    String,
> {
    let mut store = new_store(families);
    let source = format!("--- p = import \"arity.lichen\" ---\n({body})");
    let (preprocessed, diags) = lichen_language::preprocess::preprocess(&source, None, &mut store);
    if !diags.is_empty() {
        return Err(diags
            .iter()
            .map(|diag| diag.message.as_str())
            .collect::<Vec<_>>()
            .join(" | "));
    }
    let ops = if preprocessed.links.is_empty() {
        lichen_highlevel::no_native_ops()
    } else {
        store.linked_ops()
    };
    let line_starts = lichen_language::lex::line_starts(&source);
    let report = lichen_language::compile_with_imports_at::<HostProgram>(
        preprocessed.code,
        &preprocessed.imports,
        Some(store.registry()),
        preprocessed.code_base,
        &line_starts,
        ops,
    );
    if !report.ok() {
        return Err(report
            .diagnostics
            .iter()
            .map(|diag| diag.message.as_str())
            .collect::<Vec<_>>()
            .join(" | "));
    }
    Ok((report.build.unwrap(), store))
}

/// The **spliced** path: the wrapper's source joined in front of the program.
fn show_spliced(families: &str, body: &str) -> String {
    let source = format!("{families}\n({body})\n");
    let line_starts = lichen_language::lex::line_starts(&source);
    let report = lichen_language::compile_with_imports_at::<HostProgram>(
        &source,
        &[],
        None,
        0,
        &line_starts,
        &ARITY_OPS,
    );
    if !report.ok() {
        return format!(
            "<refused> {}",
            report
                .diagnostics
                .iter()
                .map(|diag| diag.message.as_str())
                .collect::<Vec<_>>()
                .join(" | ")
        );
    }
    let mut build = report.build.unwrap();
    let value = build.module.evaluate_node_deep(build.root_val, None);
    match value {
        None => "none".to_string(),
        Some(value) => {
            let (value_text, type_text) =
                lichen_language::render::print_answer_lang(&build.module, value, build.root_ty);
            format!("{value_text}: {type_text}")
        }
    }
}

#[test]
fn link_arity() {
    lichen_lowlevel::install_process_external(LIBRARY, INSTANCE, probe_entry);
    let cases = [
        (
            "one tuple parameter",
            "p.sphere (6.0, 0.25, \"out/sphere.vdb\", \"ls_sphere\")",
        ),
        (
            "a type argument and a tuple",
            "p.voxels Float (\"assets/cube.vdb\", \"ls_cube\")",
        ),
        ("a type argument and a tuple, Int", "p.voxels Int (1, 2)"),
        ("one int", "p.one 1"),
        ("two ints", "p.two 1 2"),
        ("three ints", "p.three 1 2 3"),
        ("four ints", "p.four 1 2 3 4"),
    ];
    for (what, body) in cases {
        let spliced_body = body.strip_prefix("p.").unwrap_or(body);
        println!("\n=== {what}: {body}");
        println!("  -- frozen --");
        println!("  -> {}", show_frozen(FAMILIES, body));
        println!("  -- spliced --");
        println!("  -> {}", show_spliced(FAMILIES, spliced_body));
    }
}

/// The same two-item type-first shape, once per wrapper content.
#[test]
fn wrapper_content() {
    lichen_lowlevel::install_process_external(LIBRARY, INSTANCE, probe_entry);
    let cases = [
        ("float", "p.voxels Float (\"assets/cube.vdb\", \"ls_cube\")"),
        ("int", "p.voxels Int (1, 2)"),
    ];
    for (what, body) in cases {
        let spliced_body = body.strip_prefix("p.").unwrap_or(body);
        for (name, additions) in VARIANTS {
            let wrapper = variant_wrapper(additions);
            println!(
                "\n### {what} wrapper={name}\n  frozen -> {}\n  spliced -> {}",
                show_frozen(&wrapper, body),
                show_spliced(&wrapper, spliced_body)
            );
        }
    }
}

/// The same case repeated in one process: a per-module flip varies across repeats.
#[test]
fn same_process_repeats() {
    lichen_lowlevel::install_process_external(LIBRARY, INSTANCE, probe_entry);
    const REPEATS: usize = 40;
    for path in ["frozen", "spliced"] {
        let mut ran = 0;
        let mut failed: Vec<usize> = Vec::new();
        for index in 0..REPEATS {
            let answer = if path == "frozen" {
                show_frozen(
                    FAMILIES,
                    "p.voxels Float (\"assets/cube.vdb\", \"ls_cube\")",
                )
            } else {
                show_spliced(FAMILIES, "voxels Float (\"assets/cube.vdb\", \"ls_cube\")")
            };
            if !answer.starts_with("none") {
                ran += 1;
            } else {
                failed.push(index);
            }
        }
        println!("### {path}: {ran} of {REPEATS} answered; failed at {failed:?}");
    }
}

/// The frozen two-item type-first case, with the node ids its answer turns on.
///
/// # Invariant
///
/// Every reading is a node id or an owner tag the module itself holds: the expr an
/// operand item reads, the parameter the apply's check writes, the closure the apply
/// read, and the owner tag on the return expr. Two applies that read the same closure
/// read the same shell, so the owner tag names both.
#[test]
fn frozen_readings() {
    lichen_lowlevel::install_process_external(LIBRARY, INSTANCE, probe_entry);
    let body = "p.voxels Float (\"assets/cube.vdb\", \"ls_cube\")";
    let Ok((mut build, _store)) = build_frozen(FAMILIES, body) else {
        println!("### the frozen build was refused");
        return;
    };
    let value = build.module.evaluate_node_deep(build.root_val, None);
    let answer = match value {
        None => "none".to_string(),
        Some(value) => {
            let (value_text, type_text) =
                lichen_language::render::print_answer_lang(&build.module, value, build.root_ty);
            format!("{value_text}: {type_text}")
        }
    };
    println!("### frozen answer: {answer}");
    for rehome in build.module.rehomed_functions() {
        println!(
            "  rehome {:?} of {:?}: parent={:?} return_expr={:?} owner={:?} parameter={:?} owner={:?}",
            rehome.function,
            rehome.origin,
            rehome.parent,
            rehome.r#return,
            rehome.return_owner,
            rehome.parameter,
            rehome.parameter_owner,
        );
    }
    let mut applies: Vec<(lichen_lowlevel::NodeId, lichen_lowlevel::NodeId)> = build
        .module
        .applied_parameters
        .iter()
        .map(|(&apply, &parameter)| (apply, parameter))
        .collect();
    applies.sort_by_key(|&(apply, _)| apply);
    for (apply, parameter) in applies {
        let expr = build.module.applied_exprs.get(&apply).copied();
        // The closure the apply read: the callee is the operand array's first item.
        let callee = build
            .module
            .node_operation(apply)
            .and_then(|operation| operation.operand)
            .and_then(|operand| {
                build
                    .module
                    .node_value(lichen_lowlevel::AnyNodeId::Dynamic(operand))
            })
            .and_then(|value| {
                AsEnum::<lichen_lowlevel::LowValue>::as_enum(&value).and_then(|low| match low {
                    lichen_lowlevel::LowValue::Array(array) => {
                        // SAFETY: the payload is the operand array's own, live in the module.
                        let items = unsafe { array.items() };
                        items.first().map(|item| item.node)
                    }
                    _ => None,
                })
            })
            .and_then(|node| build.module.node_value(node))
            .and_then(|value| {
                AsEnum::<lichen_lowlevel::LowValue>::as_enum(&value).map(|low| format!("{low:?}"))
            });
        println!(
            "  apply {apply:?}: closure={callee:?} parameter={parameter:?} owner={:?} return_expr={expr:?} owner={:?}",
            build.module.node_function(parameter),
            expr.and_then(|expr| build.module.node_function(expr)),
        );
    }
}
