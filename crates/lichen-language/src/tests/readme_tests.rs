use super::*;

#[test]
fn replaces_the_region_between_markers() {
    let content = "before\n<!-- begin: examples -->\nstale\n<!-- end: examples -->\nafter";
    let blob = "### `x.lichen`\n\n```text\n1\n```";
    let expected = "before\n<!-- begin: examples -->\n\n### `x.lichen`\n\n```text\n1\n```\n\n\
         <!-- end: examples -->\nafter";
    assert_eq!(replace_examples(content, blob).unwrap(), expected);
}

#[test]
fn missing_markers_are_errors() {
    assert!(replace_examples("no markers", "blob").is_err());
    assert!(replace_examples("<!-- begin: examples -->\nno end", "blob").is_err());
    assert!(
        replace_examples("<!-- end: examples -->\n<!-- begin: examples -->", "blob").is_err()
    );
}

#[test]
fn a_synced_region_is_a_noop() {
    let blob = "### `x.lichen`\n\n```text\n1\n```";
    let content = format!("<!-- begin: examples -->\n\n{blob}\n\n<!-- end: examples -->");
    assert_eq!(replace_examples(&content, blob).unwrap(), content);
}

#[test]
fn renders_the_tree_grouped_and_ordered() {
    let blob = render_examples();
    let headings: Vec<(usize, String)> = blob
        .lines()
        .filter_map(|line| {
            let level = line.chars().take_while(|&c| c == '#').count();
            if level < 3 {
                return None;
            }
            let rest = &line[level..];
            let name = rest.strip_prefix(" `")?.strip_suffix('`')?;
            (rest.len() == name.len() + 3).then(|| (level, name.to_owned()))
        })
        .collect();
    assert_eq!(
        headings,
        [
            "array.lichen",
            "tuple.lichen",
            "index.lichen",
            "closure.lichen",
            "dependent_type.lichen",
            "lazy_infinite.lichen",
            "let_polymorphism.lichen",
            "mutual_recursion.lichen",
            "nested_function.lichen",
            "recursion.lichen",
            "placeholder.lichen",
            "struct.lichen",
            "struct_recursion.lichen",
            "struct_generic.lichen",
            "table.lichen",
        ]
        .into_iter()
        .map(|name| (3, name.to_owned()))
        .chain([
            // The `import` directory is one unit: `_.lichen`'s order (7)
            // places it after every top-level file, the face opens the
            // group, and its files follow by their own orders.
            (3, "import".to_owned()),
            (4, "import/math.lichen".to_owned()),
            (4, "import/geometry.lichen".to_owned()),
            // `perspective.lichen`'s order (6) places it after the
            // `import` directory (order 5).
            (3, "perspective.lichen".to_owned()),
        ])
        .collect::<Vec<_>>(),
        "directories render as units ordered by their `_.lichen`, files by their `-- order:`"
    );
    // The face opens the directory: `_.lichen`'s program sits directly
    // under the directory heading.
    assert!(
        blob.contains("### `import`\n\n```text\n@import \"math.lichen\" as math"),
        "the directory's `_.lichen` is shown first inside the directory"
    );
    // The output is computed by running the program, not read from the
    // file: `array.lichen` runs to `[1, 2, 3]: Int<3>`, and no promise
    // or directive remains.
    assert!(
        blob.contains("output:\n```text\n[1, 2, 3]: Int<3>\n```"),
        "the runner's output is embedded"
    );
    assert!(!blob.contains("-- output:"), "file promises are not shown");
    assert!(!blob.contains("-- order:"), "order directives are not shown");
}

#[test]
fn declared_order_reads_the_comment_from_any_line() {
    assert_eq!(
        declared_order(Path::new("a.lichen"), "-- order: 2\nx"),
        Some(2)
    );
    assert_eq!(
        declared_order(Path::new("a.lichen"), "x\n-- order: 42"),
        Some(42)
    );
    assert_eq!(declared_order(Path::new("a.lichen"), "no order here"), None);
}

#[test]
fn an_output_comment_is_replaced_in_place() {
    let source = "-- order: 2\nrec f = x => x\nf 5\n-- output: stale\n";
    assert_eq!(
        replace_output_comment(source, "-- output: 5: Int"),
        "-- order: 2\nrec f = x => x\nf 5\n-- output: 5: Int\n"
    );
    // A multi-line output becomes one `-- output:` line per line.
    let source = "a\n-- output: x\n-- output: y\nb";
    assert_eq!(
        replace_output_comment(source, "-- output: 1\n-- output: 2"),
        "a\n-- output: 1\n-- output: 2\nb\n"
    );
}

#[test]
fn a_missing_output_comment_is_appended() {
    let source = "rec f = x => x\nf 5\n";
    assert_eq!(
        replace_output_comment(source, "-- output: 5: Int"),
        "rec f = x => x\nf 5\n-- output: 5: Int\n"
    );
    // A file without a trailing newline still ends up clean.
    let source = "rec f = x => x\nf 5";
    assert_eq!(
        replace_output_comment(source, "-- output: 5: Int"),
        "rec f = x => x\nf 5\n-- output: 5: Int\n"
    );
}

#[test]
#[should_panic]
fn a_bad_order_value_panics() {
    declared_order(Path::new("a.lichen"), "-- order: two");
}

#[test]
fn declared_order_breaks_ties_by_name() {
    let files = [
        (
            "c.lichen".to_string(),
            PathBuf::from("c.lichen"),
            "-- order: 2".to_string(),
        ),
        (
            "a.lichen".to_string(),
            PathBuf::from("a.lichen"),
            "-- order: 1".to_string(),
        ),
        (
            "b.lichen".to_string(),
            PathBuf::from("b.lichen"),
            "-- order: 1".to_string(),
        ),
        (
            "d.lichen".to_string(),
            PathBuf::from("d.lichen"),
            "no order".to_string(),
        ),
    ];
    let mut files = files.to_vec();
    files.sort_by(|(name_a, file_a, source_a), (name_b, file_b, source_b)| {
        let order_a = declared_order(file_a, source_a).unwrap_or(DEFAULT_ORDER);
        let order_b = declared_order(file_b, source_b).unwrap_or(DEFAULT_ORDER);
        (order_a, name_a).cmp(&(order_b, name_b))
    });
    let names: Vec<&str> = files.iter().map(|(name, _, _)| name.as_str()).collect();
    assert_eq!(names, ["a.lichen", "b.lichen", "c.lichen", "d.lichen"]);
}
