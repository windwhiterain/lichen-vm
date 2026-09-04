fn main() {
    let src_dir = std::path::Path::new("src");

    let mut config = cc::Build::new();
    config.include(src_dir);
    config
        .flag_if_supported("-Wno-unused-parameter")
        .flag_if_supported("-Wno-unused-but-set-variable")
        .flag_if_supported("-Wno-unused-variable");
    config.file(src_dir.join("parser.c"));
    config.compile("tree-sitter-lichen");
}
