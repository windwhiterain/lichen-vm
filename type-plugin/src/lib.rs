use lichen_core_plugin::{
    ARRAY_EXPR,
    system::{
        ExprImpls, Module, Plugin, PluginEnum, Variant,
        sytax::{Generics, WrittenPath, WrittenPathRaw},
    },
};

pub static PLUGIN: Plugin = Plugin {
    name: "type",
    lib_crate_path: "type",
    lib_module: WrittenPathRaw {
        crate_: CRATE,
        path: "plugin",
    },
    bin_module: Module::Bin("type/tests/project"),
    dependencies: &[&lichen_core_plugin::PLUGIN],
    enum_types: &[],
    plugin_enums: &[(&lichen_core_plugin::VALUE_TYPE, &VALUE_ENUM)],
    properties: &["r#type"],
    exprs: &[],
    expr_impls: &[ExprImpls {
        expr: &ARRAY_EXPR,
        impls: &[&WrittenPathRaw {
            crate_: CRATE,
            path: "expr_impl::Array",
        }],
    }],
};

static VALUE_ENUM: PluginEnum = PluginEnum {
    variants: &[
        Variant {
            name: "int_type",
            path: &WrittenPath {
                crate_: CRATE,
                generics: &Generics::NONE,
                path: "value::IntType",
                project_generic: false,
            },
            is_unit: true,
        },
        Variant {
            name: "string_type",
            path: &WrittenPath {
                crate_: CRATE,
                generics: &Generics::NONE,
                path: "value::StringType",
                project_generic: false,
            },
            is_unit: true,
        },
    ],
    plugin: &PLUGIN,
};

pub const CRATE: &'static str = "lichen_type";
