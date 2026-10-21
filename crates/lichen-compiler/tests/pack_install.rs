//! A pack a command line names is read, installed, and refused by name.
//! See docs/notes/graph-fusion.md §5.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use lichen_compute::compute::clear_graph_pass;
use lichen_compute::floor::install_floor_for_run;
use lichen_compute::language::{LangProgram, LangValue, new_store};
use lichen_compute::packs::{PackLoadRefusal, load_packs};
use lichen_compute::placement_pack::{PackRefusal, PlacementPack};
use lichen_highlevel::no_native_ops;
use lichen_language::lex;
use lichen_language::preprocess::preprocess;
use lichen_language::{compile_with_imports_at, run};
use lichen_lowlevel::{LowValue, Module};

/// The lane count one workgroup runs, which is the width the pack states.
const LANES: u32 = 64;

/// A launch the tile divides: three workgroups cover `[0, 192)`.
const COUNT: usize = 3 * LANES as usize;

/// The `f32` at element zero with no pass installed: the lane folds its own one element.
const PER_LANE: f32 = 1.0;

/// The name the pack the CLI installs goes by, which is the name its line prints.
const PACK: &str = "sum-in-shared";

/// The staging the pack's plan publishes, which is the mark the landed fragment carries.
const STAGING: &str = "tile";

/// A hash no node spells, so a pack carrying it is consulted and matches nothing.
const NO_SUCH_SCHEMA: &str =
    "sha256:0000000000000000000000000000000000000000000000000000000000000000";

/// The shipping compiler's own binary, so a refusal is read as a user reads it.
const BINARY: &str = env!("CARGO_BIN_EXE_lichen-compiler");

/// The answer a sum over `1 ..= COUNT` has.
///
/// # Invariant
/// `COUNT` and `COUNT + 1` are exact in `f32` and their product is below `2^53`, so the closed
/// form is one exact number however it is spelled.
fn closed_form() -> f32 {
    (COUNT * (COUNT + 1) / 2) as f32
}

/// The declared program with the dispatch's own answer as its root: the run's value is the sum.
fn declared() -> String {
    rooted("out")
}

/// A program whose one kernel declares a reduction over a **runtime** extent, answering `root`.
///
/// # Invariant
/// This is the program `crates/lichen-language/tests/pack_placement.rs` writes and its pack
/// steers: the body folds each lane's own elements, so the per-lane answer at element zero is
/// `1.0` and the tiled answer is the whole sum. `root` is the whole of what differs between two
/// roots: the graph is dispatched on the way to either, so the floor is consulted either way.
fn rooted(root: &str) -> String {
    format!(
        r#"
---
  compute = import "compute.lichen"
---
In  = struct{{.b (compute.Buf Float)}}
Out = struct{{.z (compute.Buf Float)}}
Par = compute.P (compute.KT Float)(.I In, .O Out)
reduction = (k : Par) => {{
  i = compute.range k.n
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((s(1) + (compute.read ((compute.Read _)(.from k.in.b, .at s(0) - 1)) : Float)) ~ "combine" ~ "accumulator"))
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value fold (i + 1, 0.0 ~ "identity")))
}}
kernel = compute.parallel (reduction ~ "claim:Reduction(combine,identity)") "gpu" Out
step = (s : Par) => {{
  (compute.plrun kernel ((compute.A In)(.n s.n, .I In(.b s.in.b))) : Out)
}}
built = compute.graph step
data = (compute.buffer ([{elements}] : array{{Float, LENGTH}}) : (compute.Buf _))
out = compute.collect (compute.graphrun built ({count}, data))
{root}
"#,
        elements = data(),
        count = COUNT,
    )
    .replace("array{Float, LENGTH}", &format!("array{{Float, {COUNT}}}"))
}

/// The elements a launch folds: `1 ..= count`, which is what makes the closed form the answer.
fn data() -> String {
    (1..=COUNT)
        .map(|position| format!("{position}.0"))
        .collect::<Vec<String>>()
        .join(", ")
}

/// The pack a human writes for `schema`, as the JSON artifact's fields spell it.
fn pack_text(schema: &str) -> String {
    format!(
        r#"{{
  "schema": "{schema}",
  "name": "{PACK}",
  "requires": {{ "class": "float" }},
  "plan": {{ "staging": [ {{ "name": "{STAGING}", "space": "shared", "group_width": {LANES} }} ] }}
}}"#
    )
}

/// The device, held for the whole of a test that runs on one.
///
/// # Invariant
/// `install_default` replaces the process's backend, so a second test installing one while the
/// first runs leaves the first's buffers belonging to a device that is no longer installed: the
/// tests here run one at a time.


/// What the CLI itself does with a program's packs: install the floor, compile, and run it.
///
/// # Invariant
/// The floor is installed **before** the compile, which is the only order that steers a run: a
/// graph is consulted at the dispatch its own compile reaches, so a pass installed after that
/// compile answers no run at all. [`None`] is an invocation that named no pack, which installs
/// nothing at all.
fn reported(source: &str, packs: Option<&[PlacementPack]>) -> (f32, Vec<String>) {
    clear_graph_pass();
    if let Some(packs) = packs {
        install_floor_for_run(packs);
    }
    let mut store = new_store(None).expect("the embedded compute package registers");
    let (preprocessed, diags) = preprocess(source, None, &mut store);
    assert!(diags.is_empty(), "the imports resolved: {diags:?}");
    let line_starts = lex::line_starts(source);
    let report = compile_with_imports_at::<LangProgram>(
        preprocessed.code,
        &preprocessed.imports,
        Some(store.registry()),
        preprocessed.code_base,
        &line_starts,
        no_native_ops(),
    );
    assert!(report.ok(), "the program checked: {:?}", report.diagnostics);
    let build = report.build.expect("a checked build is a build");
    let mut module = build.module;
    let value = module.evaluate_node_deep(build.root_val, None);
    let value = value.expect("the program's root value is decided");
    let report = module
        .extension_diagnostics
        .iter()
        .filter(|entry| entry.category.contains("pass"))
        .map(|entry| entry.message.clone())
        .collect();
    (first_float(&module, &value), report)
}

/// The `f32` at element zero of an array value.
///
/// # Invariant
/// The array is the payload of the value `module` just produced, so its items are live.
fn first_float(module: &Module<LangProgram>, value: &LangValue) -> f32 {
    let LangValue::LowValue(LowValue::Array(array)) = value else {
        panic!("the program answers an array, got {value:?}");
    };
    // SAFETY: `array` is the payload of a live value this module produced; this reader drops
    // no block.
    let items = unsafe { array.items() };
    let element = items.first().expect("the answer has an element");
    let LangValue::LowValue(LowValue::Float(value)) = module
        .node_value(element.node)
        .expect("an evaluated element has a value")
    else {
        panic!("element zero is not a float");
    };
    value
}

/// The `sha256:<hex>` a report line prints, which is what a pack author copies.
fn reported_schema(report: &[String]) -> String {
    let line = report
        .iter()
        .find(|line| line.contains("sha256:"))
        .unwrap_or_else(|| panic!("a node's line prints its schema: {report:?}"));
    let start = line.find("sha256:").expect("the line carries the prefix");
    line[start..start + "sha256:".len() + 64].to_string()
}

/// Write `text` under the target directory and answer its path, as a command line would name it.
fn written(name: &str, text: &str) -> PathBuf {
    let path = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    std::fs::write(&path, text).expect("the fixture is written under the target directory");
    path
}

/// The compiler's own run of `program`, naming each pack the paths hold.
fn invoke(program: &Path, packs: &[PathBuf]) -> Output {
    let mut command = Command::new(BINARY);
    for pack in packs {
        command.arg("--pack").arg(pack);
    }
    command
        .arg(program)
        .output()
        .expect("the compiler binary ran")
}

/// The schema the node of `program` spells, read off a run of it that named `probe`.
///
/// # Invariant
/// The floor is installed **only for an invocation that names a pack**, so the line that prints the
/// schema cannot be read off a run that named none. What it prints is a property of the node, not
/// of which packs were consulted, so the same `sha256:…` is on the line that says a pack fired.
fn probe_schema(program: &Path, probe: &Path) -> String {
    let output = invoke(program, &[probe.to_path_buf()]);
    let lines = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::to_string)
        .collect::<Vec<String>>();
    reported_schema(&lines)
}

/// The one stage line that names `needle`, as stderr carries it.
fn stage_line<'a>(stderr: &'a str, needle: &str) -> &'a str {
    stderr
        .lines()
        .find(|line| line.contains(needle))
        .unwrap_or_else(|| panic!("a stage line names {needle:?}: {stderr}"))
}




/// A pack whose text is not a pack is a named refusal, and the run does not happen.
///
/// # Invariant
/// The refusal's own text is what reaches the caller — the loader carries [`PackRefusal`] whole
/// and the CLI prints it unchanged — and the path beside it names the file to fix.
#[test]
fn a_pack_that_is_not_one_is_refused_and_nothing_runs() {
    let program = written("refused.lichen", "1 + 1\n");
    let malformed = written("malformed.pack", "{ \"name\": \"x\", }");
    let bad_hash = written(
        "bad-hash.pack",
        r#"{
  "schema": "sha256:9f2c",
  "name": "x",
  "requires": { "class": "float" },
  "plan": { "staging": [ { "name": "tile", "space": "shared", "group_width": 64 } ] }
}"#,
    );
    let missing = Path::new(env!("CARGO_TARGET_TMPDIR")).join("no-such.pack");

    let refusal = load_packs(&[malformed.clone()]).expect_err("a trailing comma is not a pack");
    let PackLoadRefusal::Refused {
        path,
        refusal: text,
    } = &refusal
    else {
        panic!("a pack that does not read is refused by its reader: {refusal}");
    };
    assert_eq!(
        *path, malformed,
        "the refusal names the file the command line named"
    );
    assert!(
        matches!(text, PackRefusal::Malformed { .. }),
        "and it is the reader's own refusal: {text}"
    );
    assert!(
        refusal.to_string().contains(&text.message()),
        "the CLI prints the reader's own text: {refusal}"
    );

    let hash_refusal = load_packs(&[bad_hash.clone()]).expect_err("a truncated hash is not one");
    assert!(
        matches!(
            hash_refusal,
            PackLoadRefusal::Refused {
                refusal: PackRefusal::BadSchemaHash { .. },
                ..
            }
        ),
        "the refusal is the reader's own: {hash_refusal}"
    );

    let unreadable = load_packs(&[missing.clone()]).expect_err("no such file is not a pack");
    assert!(
        matches!(unreadable, PackLoadRefusal::Unreadable { .. }),
        "a path that cannot be read is refused the same way: {unreadable}"
    );
    assert!(
        unreadable
            .to_string()
            .contains(&missing.display().to_string()),
        "and it names the path: {unreadable}"
    );

    for pack in [&malformed, &bad_hash, &missing] {
        let stderr = String::from_utf8_lossy(&invoke(&program, std::slice::from_ref(pack)).stderr)
            .into_owned();
        let output = invoke(&program, std::slice::from_ref(pack));
        assert_eq!(
            output.status.code(),
            Some(1),
            "a refused pack fails the run"
        );
        assert!(
            output.stdout.is_empty(),
            "and the program never ran: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        assert!(
            stderr.contains("placement pack"),
            "the refusal is what is reported: {stderr}"
        );
        assert!(
            stderr.contains(&pack.display().to_string()),
            "and it names the file: {stderr}"
        );
        assert_eq!(
            stderr.lines().count(),
            1,
            "one refusal is one line, and no report is printed beside it: {stderr}"
        );
    }
}



