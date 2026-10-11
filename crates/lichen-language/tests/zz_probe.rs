//! TEMPORARY probe; deleted before any commit.

use lichen_language::compile;
use lichen_language::program::LangProgram;
use lichen_language::run::evaluate;
use lichen_lowlevel::{AnyNodeId, ArrayItem, LowValue, Module, NodeId};
use lichen_utils::extend::AsEnum;

fn show(source: &str) {
    let report = compile(source);
    if !report.ok() {
        let messages: Vec<String> = report
            .diagnostics
            .iter()
            .map(|d| format!("{:?}: {}", d.stage, d.message))
            .collect();
        println!("{source:?} => REFUSED {messages:?}");
        return;
    }
    match evaluate(source) {
        Ok(out) => println!("{source:?} => {out}"),
        Err(err) => println!("{source:?} => RUN ERROR {err:?}"),
    }
}

/// A bounded structural dump of a node's value.
fn dump(module: &Module<LangProgram>, node: NodeId, depth: usize) -> String {
    if depth == 0 {
        return "…".to_string();
    }
    let Some(value) = module.node_value(AnyNodeId::Dynamic(node)) else {
        return "?".to_string();
    };
    match value.as_enum() {
        Some(LowValue::Array(array)) => {
            // SAFETY: the node is live in `module`.
            let items: &[ArrayItem] = unsafe { array.items() };
            let inner: Vec<String> = items
                .iter()
                .map(|item| match item.node {
                    AnyNodeId::Dynamic(n) => dump(module, n, depth - 1),
                    AnyNodeId::Static(_) => "static".to_string(),
                })
                .collect();
            format!("[{}]", inner.join(", "))
        }
        other => format!("{other:?}"),
    }
}

fn structure(source: &str) {
    let report = compile(source);
    assert!(report.ok(), "{source:?}: {:?}", report.diagnostics);
    let mut build = report.build.unwrap();
    let before = dump(&build.module, build.root_ty, 6);
    build.module.evaluate_node_deep(build.root_ty, None);
    let after = dump(&build.module, build.root_ty, 6);
    println!("{source:?} type before => {before}");
    println!("{source:?} type after  => {after}");
}

#[test]
fn probe() {
    println!("--- acceptance ---");
    show("[(1, 2), (3, 4)]");
    show("((1, 2), (3, 4))");
    show("[Int, Float]");
    show("f = x => (x, 1)\n[f 1, f 2]");
    show("(1, 2) : <Int, Int>");
    show("(1, 2) : <Int, string>");
    show("l = [(1, 2), (3, 4)]\nl[0](1)");
    println!("--- element attributes today ---");
    show("(1 # 4, 2) : <Int # 2, Int>");
    show("(1 # 2, 2) : <Int # 2, Int>");
    show("(1 # 4, 2) : <Int, Int>");
    show("(Int # 2, Int)");
    println!("--- element requirement enforcement ---");
    show("(1 # 2, 2) : <Int # 4, Int>");
    show("(1 # 4, 2) : <Int # 4, Int>");
    show("((1 # 2, 2), 3) : <<Int # 2, Int>, Int>");
    show("((1 # 2, 2), 3) : <<Int # 4, Int>, Int>");
    println!("--- a whole-annotation attribute ---");
    show("(1 # 4) : Int");
    show("(1 # 2) : (Int # 4)");
    show("(1 # 4) : (Int # 2)");
    println!("--- the spelled-out slot form ---");
    show("tuple{Int, Int}");
    show("(1, 2) : tuple{Int, Int}");
    show("tuple{Int # 2, Int} : <Int, Int>");
    show("(1 # 4, 2) : tuple{Int # 2, Int}");
    show("(1 # 2, 2) : tuple{Int # 4, Int}");
    show("(5, 2) : tuple{5 : Int, Int}");
    show("(6, 2) : tuple{5 : Int, Int}");
    show("(5, 2) : tuple{@any : Int, Int}");
    show("(5, 2) : <5 : Int, Int>");
    show("tuple{@any : Int # 2, @any : Int}");
    show("t = tuple{5 : Int, Int}\n(5, 2) : t");
    show("t = tuple{5 : Int, Int}\n(6, 2) : t");
    show("tuple{}");
    show("t = tuple{5 : Int, Int}\nt");
    show("t = tuple{5 : Int, @any : Int}\nt");
    show("t = tuple{Int # 2, Int}\nt");
    println!("--- @any, the wildcard literal ---");
    show("@any : Int");
    show("(@any : Int, 1)");
    show("x = @any : Int\nx");
    show("(@any, 1)");
    show("@any");
    println!("--- a value-less annotation ---");
    show("x = : Int\nx");
    show("(1, 2) : tuple{: Int, Int}");
    show("(1 # 4, 2) : tuple{: Int # 2, Int}");
    show("(5, 2) : tuple{5 : Int, : Int}");
    show("S = struct<.a :Int # 2>\nS");
    show(": Int # 2");
    show("(1, 2) : tuple{: Int # 2, Int}");
    println!("--- structs ---");
    show("S = struct<.a Int, .b string>\ns = S(.a 1, .b \"h\")\n(s.a, s.b)");
    show("S = struct<.a Int>\nS(.a \"h\")");
    show("S = struct<.a :Int # 2>\nS");
    show("f = s => s(1, 2)\nf (Int)");
    show("f = s => s(1, 2)\nf (<Int, Int>)");
    show("S = struct<.a Int, .b Int>\nf = s => s(1, 2)\nf S");
    show("S = struct<.a Int, .b Int>\nf = s => s(1, 2)\nf (struct<.a Int, .b Int>)");
    println!("--- raw reads ---");
    show("X = <Int, string>\nX<0>");
    show("X = <Int, string>\nX<1>");
    show("S = struct<.a Int, .b string>\nS::a");
    println!("--- structure ---");
    structure("(1, 2)");
    structure("<Int, string>");
    structure("S = struct<.a Int>\nS");
    structure("array<Int, 3>");
    println!("--- the gcd shape ---");
    show("f = (p => p(0)) : <Int, Int> -> Int\nf (1, 2)");
    show("f = (p => (p(0), p(1))) : <Int, Int> -> <Int, Int>\nf (1, 2)");
    show("f = p => (p(0), p(1))\nf (1, 2)");
    show("f = (p => p(0)) : <Int, Int> -> Int");
    println!("--- bisecting gcd ---");
    show("f = (p => if p(1) == 0 then p(0) else p(0)) : <Int, Int> -> Int\nf (1, 2)");
    show("f = (p => if p(1) == 0 then p(0) else f (p(1), p(0))) : <Int, Int> -> Int\nf (1, 2)");
    show(
        "f = (p => if p(1) == 0 then p(0) else f (p(1), p(0) % p(1))) : <Int, Int> -> Int\nf (48, 18)",
    );
    show("f = (p => p(0) % p(1)) : <Int, Int> -> Int\nf (1, 2)");
    println!("--- the gcd example, diagnosed ---");
    gcd_diagnostic();
    println!("--- the deferred named read ---");
    show("f = x => x.a\nf (1, 2)");
    show("f = x => x.a\nf 5");
    show("S = struct<.a Int, .b string>\nf = x => x.a\nf (S(.a 1, .b \"h\"))");
}

fn gcd_diagnostic() {
    let source = "gcd = (p => if p(1) == 0 then p(0) else gcd (p(1), p(0) % p(1))) : <Int, Int> -> Int\ngcd (48, 18)";
    let report = compile(&source);
    println!("ok = {}", report.ok());
    for d in &report.diagnostics {
        println!("{:?}: {}", d.stage, d.message);
        if let Some(check) = &d.check {
            println!(
                "  value_a = {}",
                check.value_a.map_or("none".into(), |v| format!("{v:?}"))
            );
            println!(
                "  value_b = {}",
                check.value_b.map_or("none".into(), |v| format!("{v:?}"))
            );
        }
    }
}
