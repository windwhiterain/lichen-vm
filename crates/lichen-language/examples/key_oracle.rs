//! Temporary: does the per-binding key agree with dirty propagation?
//! See docs/notes/heavy-value-cache.md §2, §9.9.
//!
//! # Invariant
//!
//! The comparison is over path sets, never counts, and each edit is applied to the previous
//! version rather than to the original: a window must describe the diff it is handed.

use std::ops::Range;

use lichen_language::dirty::dirty_marked_paths;
use lichen_language::key::binding_keys;
use lichen_language::program::LangProgram;
use lichen_language::session::BufferSession;
use lichen_language_parser::path::Path;

type Edit = fn(&str) -> String;

/// One program and the edits applied to it in sequence.
struct Case {
    name: &'static str,
    source: &'static str,
    steps: Vec<(&'static str, Edit, Range<usize>, Range<usize>)>,
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "chain",
            source: "\
cache a = 1 + 2
cache b = a + 3
cache c = b + 4
cache d = c + 5
cache e = d + 6
e + 0
",
            steps: vec![
                ("change c", |s| s.replace("b + 4", "b + 5"), 2..3, 2..3),
                ("tail only", |s| s.replace("e + 0", "e + 1"), 5..6, 5..6),
                (
                    "append a binding",
                    |s| s.replace("e + 1", "cache f = 1\ne + 1"),
                    5..5,
                    5..6,
                ),
                ("insert at the top", |s| format!("x = 1\n{s}"), 0..0, 0..1),
                (
                    "whitespace",
                    |s| s.replace("cache a", "cache  a"),
                    1..2,
                    1..2,
                ),
                ("change a", |s| s.replace("1 + 2", "1 + 3"), 1..2, 1..2),
                ("duplicate a", |s| format!("a = 100\n{s}"), 0..0, 0..1),
            ],
        },
        Case {
            name: "diamond",
            source: "\
cache a = 1 + 2
cache b = a + 3
cache c = a + 4
cache d = b + c
d
",
            steps: vec![
                ("change a", |s| s.replace("1 + 2", "1 + 3"), 0..1, 0..1),
                (
                    "whitespace on b",
                    |s| s.replace("cache b", "cache  b"),
                    1..2,
                    1..2,
                ),
                ("commute d", |s| s.replace("b + c", "c + b"), 3..4, 3..4),
            ],
        },
        Case {
            name: "nested block",
            source: "\
cache e = { local = 1; local + 2 }
e
",
            steps: vec![
                (
                    "change the nested literal",
                    |s| s.replace("local + 2", "local + 3"),
                    0..1,
                    0..1,
                ),
                (
                    "append a binding",
                    |s| s.replace("}\ne", "}\nx = 1\ne"),
                    1..1,
                    1..2,
                ),
            ],
        },
        Case {
            name: "combined",
            source: "\
cache a = 1 + 2
cache b = a + 3
u = 9
b + u
",
            steps: vec![(
                "whitespace + unmarked change",
                |s| s.replace("cache a", "cache  a").replace("u = 9", "u = 8"),
                0..3,
                0..3,
            )],
        },
    ]
}

fn main() {
    let (mut agreed, mut disagreed) = (0usize, 0usize);
    for case in cases() {
        let mut source = case.source.to_string();
        let mut session = BufferSession::<LangProgram>::new(source.clone());
        let mut previous = session.compile().program;
        println!("== {}", case.name);
        for (label, edit, previous_window, current_window) in case.steps {
            source = edit(&source);
            session.set_source(&source);
            let report = session.compile();
            let current = report.program.clone();

            let external: std::collections::HashMap<usize, u64> = std::collections::HashMap::new();
            let dirty = dirty_marked_paths(&previous, &current, previous_window, current_window);
            let before = binding_keys(&previous, &external);
            let after = binding_keys(&current, &external);
            let mut keyed: Vec<Path> = Vec::new();
            for path in before.keys().chain(after.keys()) {
                let same = matches!(
                    (before.get(path), after.get(path)),
                    (Some(Some(left)), Some(Some(right))) if left == right
                );
                if !same && !keyed.contains(path) {
                    keyed.push(path.clone());
                }
            }

            let mut only_dirty: Vec<&Path> =
                dirty.iter().filter(|path| !keyed.contains(path)).collect();
            let mut only_key: Vec<&Path> =
                keyed.iter().filter(|path| !dirty.contains(path)).collect();
            only_dirty.sort_by_key(|path| format!("{path:?}"));
            only_key.sort_by_key(|path| format!("{path:?}"));
            let agree = only_dirty.is_empty() && only_key.is_empty();
            if agree {
                agreed += 1;
            } else {
                disagreed += 1;
            }
            println!(
                "  {label:<24} dirty {} / keyed {} / agree {agree} / cells {:?}",
                dirty.len(),
                keyed.len(),
                report.cells
            );
            for path in only_dirty {
                println!("    dirty but the key says clean: {path:?}");
            }
            for path in only_key {
                println!("    key says changed but propagation says clean: {path:?}");
            }
            previous = current;
        }
    }
    println!("\n{agreed} edits agree, {disagreed} disagree");
}
