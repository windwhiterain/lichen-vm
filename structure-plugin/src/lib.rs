use lichen_core_plugin::system::{
    Expr, ExprImpls, ExprParam, ExprParams, Module, Plugin, PluginEnum, Variant,
    sytax::{Generics, WrittenPath, WrittenPathRaw},
};

pub static PLUGIN: Plugin = Plugin {
    name: "structure",
    lib_crate_path: "structure",
    lib_module: WrittenPathRaw {
        crate_: CRATE,
        path: "plugin",
    },
    bin_module: Module::Bin("structure/tests/project"),
    dependencies: &[&lichen_core_plugin::PLUGIN],
    enum_types: &[],
    plugin_enums: &[
        (&lichen_core_plugin::VALUE_TYPE, &VALUE_ENUM),
        (&lichen_core_plugin::OPERATOR_TYPE, &OPERATOR_ENUM),
    ],
    properties: &["structure"],
    exprs: &[&MEMBER_EXPR],
    expr_impls: &[ExprImpls {
        expr: &MEMBER_EXPR,
        impls: &[&WrittenPathRaw {
            crate_: CRATE,
            path: "expr_impl::Member",
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
                generics: &Generics::NONE,
                path: "value::NamedArray",
                project_generic: false,
            },
            is_unit: false,
        },
        Variant {
            name: "name_set",
            path: &WrittenPath {
                crate_: CRATE,
                generics: &Generics::NONE,
                path: "value::NameSet",
                project_generic: false,
            },
            is_unit: false,
        },
        Variant {
            name: "structure",
            path: &WrittenPath {
                crate_: CRATE,
                generics: &Generics::NONE,
                path: "value::Structure",
                project_generic: false,
            },
            is_unit: false,
        },
    ],
    plugin: &PLUGIN,
};

static OPERATOR_ENUM: PluginEnum = PluginEnum {
    variants: &[
        Variant {
            name: "offset",
            path: &WrittenPath {
                crate_: CRATE,
                path: "operator::Offset",
                generics: &Generics::NONE,
                project_generic: false,
            },
            is_unit: true,
        },
        Variant {
            name: "component",
            path: &WrittenPath {
                crate_: CRATE,
                path: "operator::Component",
                generics: &Generics::NONE,
                project_generic: false,
            },
            is_unit: true,
        },
    ],
    plugin: &PLUGIN,
};

pub const CRATE: &'static str = "lichen_structure";
