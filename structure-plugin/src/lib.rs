use lichen_core_plugin::{
    ARRAY_EXPR, FIND_EXPR, INDEX_EXPR, SUM_EXPR, expr_id_param,
    system::{
        Expr, ExprImpls, Module, Params, Plugin, PluginEnum, Variant,
        sytax::{Generics, WrittenPath, WrittenPathRaw},
    },
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
        (
            &lichen_core_plugin::DIAGNOSTIC_KIND_TYPE,
            &DIAGNOSTIC_KIND_ENUM,
        ),
    ],
    properties: &["structure"],
    exprs: &[&MEMBER_EXPR, &COMPOSE_EXPR, &CONSTRUCT_EXPR],
    expr_impls: &[
        ExprImpls {
            expr: &MEMBER_EXPR,
            impls: &[&WrittenPathRaw {
                crate_: CRATE,
                path: "expr_impl::Member",
            }],
        },
        ExprImpls {
            expr: &COMPOSE_EXPR,
            impls: &[&WrittenPathRaw {
                crate_: CRATE,
                path: "expr_impl::Compose",
            }],
        },
        ExprImpls {
            expr: &CONSTRUCT_EXPR,
            impls: &[&WrittenPathRaw {
                crate_: CRATE,
                path: "expr_impl::Construct",
            }],
        },
        ExprImpls {
            expr: &ARRAY_EXPR,
            impls: &[&WrittenPathRaw {
                crate_: CRATE,
                path: "expr_impl::Array",
            }],
        },
        ExprImpls {
            expr: &INDEX_EXPR,
            impls: &[&WrittenPathRaw {
                crate_: CRATE,
                path: "expr_impl::Index",
            }],
        },
        ExprImpls {
            expr: &SUM_EXPR,
            impls: &[&WrittenPathRaw {
                crate_: CRATE,
                path: "expr_impl::Sum",
            }],
        },
        ExprImpls {
            expr: &FIND_EXPR,
            impls: &[&WrittenPathRaw {
                crate_: CRATE,
                path: "expr_impl::Find",
            }],
        },
    ],
};

static VALUE_ENUM: PluginEnum = PluginEnum {
    variants: &[
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
            name: "layout",
            path: &WrittenPath {
                crate_: CRATE,
                generics: &Generics::NONE,
                path: "value::Layout",
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
        Variant {
            name: "compose",
            path: &WrittenPath {
                crate_: CRATE,
                path: "operator::Compose",
                generics: &Generics::NONE,
                project_generic: false,
            },
            is_unit: true,
        },
        Variant {
            name: "match",
            path: &WrittenPath {
                crate_: CRATE,
                path: "operator::Match",
                generics: &Generics::NONE,
                project_generic: false,
            },
            is_unit: true,
        },
        Variant {
            name: "transform",
            path: &WrittenPath {
                crate_: CRATE,
                path: "operator::Transform",
                generics: &Generics::NONE,
                project_generic: false,
            },
            is_unit: true,
        },
    ],
    plugin: &PLUGIN,
};

static DIAGNOSTIC_KIND_ENUM: PluginEnum = PluginEnum {
    variants: &[
        Variant {
            name: "member_name_repetition",
            path: &WrittenPath {
                crate_: CRATE,
                generics: &Generics::NONE,
                path: "diagnostic_kind::MemberNameRepetition",
                project_generic: false,
            },
            is_unit: false,
        },
        Variant {
            name: "member_name_missing",
            path: &WrittenPath {
                crate_: CRATE,
                generics: &Generics::NONE,
                path: "diagnostic_kind::MemberNameMissing",
                project_generic: false,
            },
            is_unit: false,
        },
    ],
    plugin: &PLUGIN,
};

static MEMBER_EXPR: Expr = Expr {
    name: "member",
    params: &Params::simple(&[&expr_id_param("instance"), &expr_id_param("name")]),
};

static COMPOSE_EXPR: Expr = Expr {
    name: "compose",
    params: &Params::simple(&[&expr_id_param("name_set"), &expr_id_param("structures")]),
};

static CONSTRUCT_EXPR: Expr = Expr {
    name: "construct",
    params: &Params::simple(&[
        &expr_id_param("structure"),
        &expr_id_param("name_set"),
        &expr_id_param("members"),
    ]),
};

pub const CRATE: &'static str = "lichen_structure";
