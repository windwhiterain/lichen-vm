use super::*;

#[test]
fn replaces_the_region_between_markers() {
    let content = "before\n<!-- begin: examples -->\nstale\n<!-- end: examples -->\nafter";
    let blob = "### `x.lichen`\n\n```text\n1\n```";
    let expected = "before\n<!-- begin: examples -->\n\n### `x.lichen`\n\n```text\n1\n```\n\n<!-- end: examples -->\nafter";
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
    // Render the controlled fixture tree, not the live example set: the real
    // `examples/programs/` is a moving spec, so asserting its names here
    // would force a test edit for every example added/renamed/reordered.
    // The fixture exercises the same behaviours the live tree does: files at
    // several `order =` values, a tie broken by name, an undeclared entry
    // sorting last, a directory opened by its `_.lichen`, and a nested
    // directory rendered a level deeper.
    let fixture = crate_dir().join("tests").join("fixtures").join("readme");
    let blob = render_examples_in(&fixture);
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
            (3, "b.lichen"),
            (3, "d.lichen"),
            (3, "a.lichen"),
            (3, "pkg"),
            (4, "pkg/sub"),
            (5, "pkg/sub/z.lichen"),
            (4, "pkg/x.lichen"),
            (4, "pkg/y.lichen"),
            (3, "c.lichen"),
        ]
        .into_iter()
        .map(|(level, name)| (level as usize, name.to_owned()))
        .collect::<Vec<_>>(),
        "directories render as units ordered by their `_.lichen`, files by their `order =`"
    );
    // The face opens the directory: `_.lichen`'s whole file sits directly
    // under the directory heading, `@{...@}` block included.
    assert!(
        blob.contains("### `pkg`\n\n```text\n@{"),
        "the directory's `_.lichen` is shown first inside the directory"
    );
    // The whole file is embedded, so its output metadata is what shows.
    assert!(
        blob.contains("output = \"1: Int\""),
        "each file's output metadata is embedded with the whole file"
    );
    assert!(
        blob.contains("order = \"0\""),
        "the order metadata is shown with the whole file"
    );
    assert!(!blob.contains("output:\n```text"), "no separate output block");
}

#[test]
fn declared_order_reads_the_block_anywhere() {
    assert_eq!(
        declared_order(Path::new("a.lichen"), "@{order = \"2\"@}\nx"),
        Some(2)
    );
    assert_eq!(
        declared_order(Path::new("a.lichen"), "x\n@{order = \"42\"@}"),
        Some(42)
    );
    assert_eq!(declared_order(Path::new("a.lichen"), "no order here"), None);
}

#[test]
fn an_output_comment_is_replaced_in_place() {
    let source = "@{order = \"2\"\noutput = \"stale\"@}\nrec f = x => x\nf 5\n";
    assert_eq!(
        replace_output_comment(source, "5: Int"),
        "@{\n  order = \"2\"\n  output = \"5: Int\"\n@}\nrec f = x => x\nf 5\n"
    );
    // A multi-line output becomes a multi-line string.
    let source = "@{output = \"x\ny\"@}\nb";
    assert_eq!(
        replace_output_comment(source, "1\n2"),
        "@{\n  output = \"1\n2\"\n@}\nb\n"
    );
}

#[test]
fn a_missing_output_comment_is_appended() {
    let source = "rec f = x => x\nf 5\n";
    assert_eq!(
        replace_output_comment(source, "5: Int"),
        "@{\n  output = \"5: Int\"\n@}\nrec f = x => x\nf 5\n"
    );
    // A file without a trailing newline still ends up clean.
    let source = "rec f = x => x\nf 5";
    assert_eq!(
        replace_output_comment(source, "5: Int"),
        "@{\n  output = \"5: Int\"\n@}\nrec f = x => x\nf 5\n"
    );
}

#[test]
#[should_panic]
fn a_bad_order_value_panics() {
    declared_order(Path::new("a.lichen"), "@{order = \"two\"@}");
}

#[test]
fn declared_order_breaks_ties_by_name() {
    let files = [
        (
            "c.lichen".to_string(),
            PathBuf::from("c.lichen"),
            "@{order = \"2\"@}".to_string(),
        ),
        (
            "a.lichen".to_string(),
            PathBuf::from("a.lichen"),
            "@{order = \"1\"@}".to_string(),
        ),
        (
            "b.lichen".to_string(),
            PathBuf::from("b.lichen"),
            "@{order = \"1\"@}".to_string(),
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
