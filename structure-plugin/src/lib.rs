use lichen_project::{
    Expr, ExprImpls, ExprParam, ExprParams, Generics, Module, Name, Plugin, PluginEnum, Variant,
    WrittenSymbol, code::WrittenPath,
};

pub static PLUGIN: Plugin = Plugin {
    name: "structure",
    lib_crate_path: "structure",
    lib_module: WrittenSymbol {
        crate_: CRATE,
        relative: "plugin",
    },
    bin_module: Module::Path("structure/tests/project"),
    dependencies: &[&lichen_core_plugin::PLUGIN],
    names: &[],
    enum_types: &[],
    plugin_enums: &[(&lichen_core_plugin::VALUE_TYPE, &VALUE_ENUM)],
    properties: &["structure"],
    exprs: &[&MEMBER_EXPR],
    expr_impls: &[ExprImpls {
        expr: &MEMBER_EXPR,
        impls: &[&WrittenSymbol {
            crate_: CRATE,
            relative: "MemberExprImpl",
        }],
    }],
};

static MEMBER_EXPR: Expr = Expr {
    name: "member",
    params: &ExprParams(&[
        ExprParam {
            name: "structure",
            is_array: false,
        },
        ExprParam {
            name: "name",
            is_array: false,
        },
    ]),
};

static VALUE_ENUM: PluginEnum = PluginEnum {
    variants: &[
        Variant {
            name: "named_array",
            path: &WrittenPath {
                crate_: CRATE,
                generics: &Generics::none(),
                path: "NamedArray",
                project_generic: false,
            },
            is_unit: false,
        },
        Variant {
            name: "name_set",
            path: &WrittenPath {
                crate_: CRATE,
                generics: &Generics::none(),
                path: "NameSet",
                project_generic: false,
            },
            is_unit: false,
        },
        Variant {
            name: "structure",
            path: &WrittenPath {
                crate_: CRATE,
                generics: &Generics::none(),
                path: "Structure",
                project_generic: false,
            },
            is_unit: false,
        },
    ],
    plugin: &PLUGIN,
};

pub const CRATE: &'static str = "lichen_structure";
