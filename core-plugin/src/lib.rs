pub mod system;
use system::{
    CLONE, DEBUG, EnumType, Expr, ExprImpls, FORMATE_RESULT, FORMATTER_PARAM, Function, HASH,
    Module, PARTIAL_EQ, PROJECT_VARIABLE, Param, Params, PassMode, Plugin, PluginEnum, Self_,
    Variant, enum_impl::Delegate, sytax::Derives, sytax::DisplayArray, sytax::Generics,
    sytax::WrittenPath,
};

use crate::system::{
    EXPR_ID,
    sytax::{AsTrait, Generic, Name, WrittenPathRaw},
    utils::generated_project_trait,
};

pub static PLUGIN: Plugin = Plugin {
    name: "core",
    lib_crate_path: "core",
    lib_module: WrittenPathRaw {
        crate_: CRATE,
        path: "plugin",
    },
    bin_module: Module::Bin("core/tests/project"),
    dependencies: &[],
    enum_types: &[&VALUE_TYPE, &OPERATOR_TYPE, &DIAGNOSTIC_KIND_TYPE],
    plugin_enums: &[
        (&VALUE_TYPE, &VALUE_ENUM),
        (&OPERATOR_TYPE, &OPERATOR_ENUM),
        (&DIAGNOSTIC_KIND_TYPE, &DIAGNOSTIC_KIND_ENUM),
    ],
    properties: &["value"],
    exprs: &[&SUM_EXPR, &INDEX_EXPR, &FIND_EXPR, &ARRAY_EXPR],
    expr_impls: &[
        ExprImpls {
            expr: &SUM_EXPR,
            impls: &[&WrittenPathRaw {
                crate_: CRATE,
                path: "expr_impl::Sum",
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
            expr: &FIND_EXPR,
            impls: &[&WrittenPathRaw {
                crate_: CRATE,
                path: "expr_impl::Find",
            }],
        },
        ExprImpls {
            expr: &ARRAY_EXPR,
            impls: &[&WrittenPathRaw {
                crate_: CRATE,
                path: "expr_impl::Array",
            }],
        },
    ],
};

pub static VALUE_TYPE: EnumType = EnumType {
    name: &Name {
        name: "Value",
        project_generic: false,
        generics: &Generics(&[]),
    },
    is_unit: false,
    derives: &Derives(&["Clone", "Copy"]),
    markers: &["Eq"],
    impls: &[&PARTIAL_EQ, &DEBUG],
    base_traits: &[&"std::fmt::Debug", &"Copy", &"Eq"],
    functions: &[
        Function {
            name: "fields",
            generics: &[],
            self_: Some(Self_(PassMode::Ref { lifetime: None })),
            params: &Params::simple(&[]),
            return_: Some(&DisplayArray(&[
                &"Iterator<Item=&",
                &WrittenPathRaw {
                    crate_: CRATE,
                    path: "runtime::NodeIdLocal",
                },
                &">",
            ])),
            enum_impl: Some(&Delegate),
            default_body: Some(&"std::iter::empty()"),
            return_impl: true,
        },
        Function {
            name: "for_fields",
            generics: &[],
            self_: Some(Self_(PassMode::Ref { lifetime: None })),
            params: &Params::simple(&[&Param {
                name: "action",
                pass_mode: PassMode::Move,
                type_: &DisplayArray(&[
                    &"impl FnMut(&",
                    &WrittenPathRaw {
                        crate_: CRATE,
                        path: "runtime::NodeIdLocal",
                    },
                    &")",
                ]),
                mutable: true,
            }]),
            return_: None,
            enum_impl: Some(&Delegate),
            default_body: Some(&DisplayArray(&[&"for i in self.fields(){{action(i);}}"])),
            return_impl: false,
        },
        Function {
            name: "for_field_pairs",
            generics: &[],
            self_: Some(Self_(PassMode::Ref { lifetime: None })),
            params: &Params::simple(&[
                &Param {
                    name: "other",
                    pass_mode: PassMode::Ref { lifetime: None },
                    type_: &"Self",
                    mutable: false,
                },
                &Param {
                    name: "action",
                    pass_mode: PassMode::Move,
                    type_: &DisplayArray(&[
                        &"impl FnMut(&",
                        &WrittenPathRaw {
                            crate_: CRATE,
                            path: "runtime::NodeIdLocal",
                        },
                        &",&",
                        &WrittenPathRaw {
                            crate_: CRATE,
                            path: "runtime::NodeIdLocal",
                        },
                        &")",
                    ]),
                    mutable: true,
                },
            ]),
            return_: None,
            enum_impl: Some(&Delegate),
            default_body: Some(&DisplayArray(&[
                &"for (i,j) in self.fields().zip(other.fields()){{action(i,j);}}",
            ])),
            return_impl: false,
        },
    ],
    use_enum_types: &[],
    plugin: &PLUGIN,
};

pub static OPERATOR_TYPE: EnumType = EnumType {
    name: &Name {
        name: "Operator",
        project_generic: true,
        generics: &Generics(&[]),
    },
    is_unit: true,
    derives: &Derives(&["Clone", "Copy", "PartialEq", "Eq"]),
    markers: &[],
    impls: &[&DEBUG],
    base_traits: &[&"std::fmt::Debug", &"Copy", &"Eq"],
    functions: &[Function {
        name: "run",
        generics: &[],
        self_: Some(Self_(PassMode::Ref { lifetime: None })),
        params: &Params::simple(&[
            &Param {
                name: "solver",
                pass_mode: PassMode::RefMut { lifetime: None },
                type_: &DisplayArray(&[
                    &WrittenPathRaw {
                        crate_: CRATE,
                        path: "runtime::solve::Solver",
                    },
                    &"<",
                    &PROJECT_VARIABLE,
                    &">",
                ]),
                mutable: false,
            },
            &Param {
                name: "operand",
                pass_mode: PassMode::Ref { lifetime: None },
                type_: &DisplayArray(&[
                    &AsTrait {
                        this: &PROJECT_VARIABLE,
                        trait_: &generated_project_trait(&PLUGIN),
                    },
                    &"::Value",
                ]),
                mutable: false,
            },
            &Param {
                name: "node",
                pass_mode: PassMode::Ref { lifetime: None },
                type_: &WrittenPathRaw {
                    crate_: CRATE,
                    path: "runtime::solve::LocalNodeId",
                },
                mutable: false,
            },
        ]),
        return_: Some(&WrittenPath {
            crate_: CRATE,
            path: "runtime::operation::Option",
            generics: &Generics::NONE,
            project_generic: true,
        }),
        enum_impl: Some(&Delegate),
        default_body: None,
        return_impl: false,
    }],
    use_enum_types: &[&VALUE_TYPE, &DIAGNOSTIC_KIND_TYPE],
    plugin: &PLUGIN,
};

pub static DIAGNOSTIC_KIND_TYPE: EnumType = EnumType {
    name: &Name {
        name: "DiagnosticKind",
        project_generic: true,
        generics: &Generics(&[]),
    },
    is_unit: false,
    derives: &Derives(&[]),
    markers: &["Eq"],
    impls: &[&DEBUG, &PARTIAL_EQ, &HASH, &CLONE],
    base_traits: &[&"std::fmt::Debug", &"Eq", HASH.symbol, CLONE.symbol],
    functions: &[Function {
        name: "message",
        generics: &[],
        self_: Some(Self_(PassMode::Ref { lifetime: None })),
        params: &Params::simple(&[&FORMATTER_PARAM]),
        return_: Some(&FORMATE_RESULT),
        enum_impl: Some(&Delegate),
        default_body: None,
        return_impl: false,
    }],
    use_enum_types: &[],
    plugin: &PLUGIN,
};

pub static VALUE_ENUM: PluginEnum = PluginEnum {
    variants: &[
        Variant {
            name: "int",
            path: &WrittenPath {
                crate_: CRATE,
                generics: &Generics::NONE,
                path: "value::Int",
                project_generic: false,
            },
            is_unit: false,
        },
        Variant {
            name: "string",
            path: &WrittenPath {
                crate_: CRATE,
                generics: &Generics::NONE,
                path: "value::StringId",
                project_generic: false,
            },
            is_unit: false,
        },
        Variant {
            name: "array",
            path: &WrittenPath {
                crate_: CRATE,
                generics: &Generics::NONE,
                path: "value::Array",
                project_generic: false,
            },
            is_unit: false,
        },
        Variant {
            name: "table",
            path: &WrittenPath {
                crate_: CRATE,
                generics: &Generics::NONE,
                path: "value::Table",
                project_generic: false,
            },
            is_unit: false,
        },
        Variant {
            name: "unit",
            path: &WrittenPath {
                crate_: CRATE,
                generics: &Generics::NONE,
                path: "value::Unit",
                project_generic: false,
            },
            is_unit: true,
        },
    ],
    plugin: &PLUGIN,
};

pub static OPERATOR_ENUM: PluginEnum = PluginEnum {
    variants: &[
        Variant {
            name: "sum",
            path: &WrittenPath {
                crate_: CRATE,
                generics: &Generics::NONE,
                path: "operator::Sum",
                project_generic: false,
            },
            is_unit: true,
        },
        Variant {
            name: "index",
            path: &WrittenPath {
                crate_: CRATE,
                generics: &Generics::NONE,
                path: "operator::Index",
                project_generic: false,
            },
            is_unit: true,
        },
        Variant {
            name: "find",
            path: &WrittenPath {
                crate_: CRATE,
                generics: &Generics::NONE,
                path: "operator::Find",
                project_generic: false,
            },
            is_unit: true,
        },
    ],
    plugin: &PLUGIN,
};
pub static DIAGNOSTIC_KIND_ENUM: PluginEnum = PluginEnum {
    variants: &[
        Variant {
            name: "equality_error",
            path: &WrittenPath {
                crate_: CRATE,
                generics: &Generics::NONE,
                path: "diagnostic_kind::EqualityError",
                project_generic: false,
            },
            is_unit: false,
        },
        Variant {
            name: "index_out_of_bounds",
            path: &WrittenPath {
                crate_: CRATE,
                generics: &Generics::NONE,
                path: "diagnostic_kind::IndexOutOfBounds",
                project_generic: false,
            },
            is_unit: false,
        },
    ],
    plugin: &PLUGIN,
};
pub static SUM_EXPR: Expr = Expr {
    name: "sum",
    params: &Params::simple(&[&expr_id_param("addends")]),
};
pub static INDEX_EXPR: Expr = Expr {
    name: "index",
    params: &Params::simple(&[&expr_id_param("array"), &expr_id_param("index")]),
};
pub static FIND_EXPR: Expr = Expr {
    name: "find",
    params: &Params::simple(&[&expr_id_param("table"), &expr_id_param("name")]),
};
pub static ARRAY_EXPR: Expr = Expr {
    name: "array",
    params: &Params {
        this: &[&expr_ids_param("items")],
        generics: &Generics(&[&Generic {
            name: "'a",
            constraints: &[],
        }]),
    },
};
pub const CRATE: &'static str = "lichen_core";
pub const EXPR_IDS: DisplayArray =
    DisplayArray(&[&"impl IntoIterator<Item = &'a ", &EXPR_ID, &"> + Copy"]);
pub const fn expr_id_param(name: &'static str) -> Param {
    Param {
        name,
        pass_mode: PassMode::Ref { lifetime: None },
        type_: &EXPR_ID,
        mutable: false,
    }
}
pub const fn expr_ids_param(name: &'static str) -> Param {
    Param {
        name,
        pass_mode: PassMode::Move,
        type_: &EXPR_IDS,
        mutable: false,
    }
}

fn a(_a: impl IntoIterator) {}
