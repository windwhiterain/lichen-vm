use lichen_project::{
    Annotation, ArrayDisplay, AsTrait, CLONE, DEBUG, DelegateBody, Derives, EnumType, Expr,
    ExprImpls, ExprParam, ExprParams, FORMATE_RESULT_SYMBOL, FORMATTER_PARAM, Function, Generic,
    Generics, HASH, Module, Name, PARTIAL_EQ, PROJECT, PROJECT_GENERIC, PROJECT_TRAIT,
    PROJECT_VARIABLE, Param, Params, PassMode, Plugin, PluginEnum, SELF_SYMBOL, Self_, Symbol,
    Variant, WrittenSymbol,
    code::{self, WrittenPath},
    plugin::ProjectTrait,
};

pub static PLUGIN: Plugin = Plugin {
    name: "core",
    lib_crate_path: "core",
    lib_module: WrittenSymbol {
        crate_: CRATE,
        relative: "plugin",
    },
    bin_module: Module::Path("core/tests/project"),
    dependencies: &[],
    names: &[&code::Name {
        name: "Ast",
        generics: &Generics::none(),
        project_generic: true,
    }],
    enum_types: &[&VALUE_TYPE, &OPERATOR_TYPE, &DIAGNOSTIC_KIND_TYPE],
    plugin_enums: &[
        (&VALUE_TYPE, &VALUE_ENUM),
        (&OPERATOR_TYPE, &OPERATOR_ENUM),
        (&DIAGNOSTIC_KIND_TYPE, &DIAGNOSTIC_KIND_ENUM),
    ],
    properties: &["value"],
    exprs: &[&SUM_EXPR, &INDEX_EXPR, &FIND_EXPR],
    expr_impls: &[
        ExprImpls {
            expr: &SUM_EXPR,
            impls: &[&WrittenSymbol {
                crate_: CRATE,
                relative: "Sum",
            }],
        },
        ExprImpls {
            expr: &INDEX_EXPR,
            impls: &[&WrittenSymbol {
                crate_: CRATE,
                relative: "Index",
            }],
        },
        ExprImpls {
            expr: &FIND_EXPR,
            impls: &[&WrittenSymbol {
                crate_: CRATE,
                relative: "Find",
            }],
        },
    ],
};

pub static VALUE_TYPE: EnumType = EnumType {
    name: &code::Name {
        name: "Value",
        project_generic: false,
        generics: &Generics(&[]),
    },
    is_unit: false,
    derives: &Derives(&["Clone", "Copy"]),
    markers: &["Eq"],
    impls: &[&PARTIAL_EQ, &DEBUG],
    base_traits: &[
        Symbol::Raw("std::fmt::Debug"),
        Symbol::Raw("Copy"),
        Symbol::Raw("Eq"),
    ],
    functions: &[
        Function {
            name: "fields",
            generics: &Generics::none(),
            self_: Some(Self_(PassMode::Ref { lifetime: None })),
            params: &Params(&[]),
            return_: Some(Annotation {
                impl_: true,
                symbol: &ArrayDisplay(&[
                    &"Iterator<Item=&",
                    &WrittenSymbol {
                        crate_: CRATE,
                        relative: "runtime::NodeIdLocal",
                    },
                    &">",
                ]),
            }),
            body: Some(&DelegateBody),
            default_body: Some(&"std::iter::empty()"),
        },
        Function {
            name: "for_fields",
            generics: &Generics::none(),
            self_: Some(Self_(PassMode::Ref { lifetime: None })),
            params: &Params(&[&Param {
                name: "action",
                pass_mode: PassMode::Move,
                symbol: &Symbol::Dyn(&ArrayDisplay(&[
                    &"impl FnMut(&",
                    &WrittenSymbol {
                        crate_: CRATE,
                        relative: "runtime::NodeIdLocal",
                    },
                    &")",
                ])),
                mutable: true,
            }]),
            return_: None,
            body: Some(&DelegateBody),
            default_body: Some(&ArrayDisplay(&[&"for i in self.fields(){{action(i);}}"])),
        },
        Function {
            name: "for_field_pairs",
            generics: &Generics::none(),
            self_: Some(Self_(PassMode::Ref { lifetime: None })),
            params: &Params(&[
                &Param {
                    name: "other",
                    pass_mode: PassMode::Ref { lifetime: None },
                    symbol: &SELF_SYMBOL,
                    mutable: false,
                },
                &Param {
                    name: "action",
                    pass_mode: PassMode::Move,
                    symbol: &Symbol::Dyn(&ArrayDisplay(&[
                        &"impl FnMut(&",
                        &WrittenSymbol {
                            crate_: CRATE,
                            relative: "runtime::NodeIdLocal",
                        },
                        &",&",
                        &WrittenSymbol {
                            crate_: CRATE,
                            relative: "runtime::NodeIdLocal",
                        },
                        &")",
                    ])),
                    mutable: true,
                },
            ]),
            return_: None,
            body: Some(&DelegateBody),
            default_body: Some(&ArrayDisplay(&[
                &"for (i,j) in self.fields().zip(other.fields()){{action(i,j);}}",
            ])),
        },
    ],
    plugin: &PLUGIN,
};

pub static OPERATOR_TYPE: EnumType = EnumType {
    name: &code::Name {
        name: "Operator",
        project_generic: true,
        generics: &Generics(&[]),
    },
    is_unit: true,
    derives: &Derives(&["Clone", "Copy", "PartialEq", "Eq"]),
    markers: &[],
    impls: &[&DEBUG],
    base_traits: &[
        Symbol::Raw("std::fmt::Debug"),
        Symbol::Raw("Copy"),
        Symbol::Raw("Eq"),
    ],
    functions: &[Function {
        name: "run",
        generics: &Generics::none(),
        self_: Some(Self_(PassMode::Ref { lifetime: None })),
        params: &Params(&[
            &Param {
                name: "solver",
                pass_mode: PassMode::RefMut { lifetime: None },
                symbol: &Symbol::Dyn(&ArrayDisplay(&[
                    &WrittenSymbol {
                        crate_: CRATE,
                        relative: "runtime::solve::Solver",
                    },
                    &"<",
                    &PROJECT_VARIABLE,
                    &">",
                ])),
                mutable: false,
            },
            &Param {
                name: "value",
                pass_mode: PassMode::Ref { lifetime: None },
                symbol: &Symbol::Dyn(&ArrayDisplay(&[
                    &AsTrait {
                        this: &PROJECT_VARIABLE,
                        trait_: &ProjectTrait { plugin: &PLUGIN },
                    },
                    &"::Value",
                ])),
                mutable: false,
            },
            &Param {
                name: "node",
                pass_mode: PassMode::Ref { lifetime: None },
                symbol: &Symbol::Written(&WrittenSymbol {
                    crate_: CRATE,
                    relative: "runtime::solve::LocalNodeId",
                }),
                mutable: false,
            },
        ]),
        return_: Some(Annotation {
            impl_: false,
            symbol: &ArrayDisplay(&[
                &"Option<",
                &AsTrait {
                    this: &PROJECT_VARIABLE,
                    trait_: &ProjectTrait { plugin: &PLUGIN },
                },
                &"::Value>",
            ]),
        }),
        body: Some(&DelegateBody),
        default_body: None,
    }],
    plugin: &PLUGIN,
};

pub static DIAGNOSTIC_KIND_TYPE: EnumType = EnumType {
    name: &code::Name {
        name: "DiagnosticKind",
        project_generic: true,
        generics: &Generics(&[]),
    },
    is_unit: false,
    derives: &Derives(&[]),
    markers: &["Eq"],
    impls: &[&DEBUG, &PARTIAL_EQ, &HASH, &CLONE],
    base_traits: &[
        Symbol::Raw("std::fmt::Debug"),
        Symbol::Raw("Eq"),
        *HASH.symbol,
        *CLONE.symbol,
    ],
    functions: &[Function {
        name: "message",
        generics: &Generics::none(),
        self_: Some(Self_(PassMode::Ref { lifetime: None })),
        params: &Params(&[&FORMATTER_PARAM]),
        return_: Some(Annotation {
            impl_: false,
            symbol: &FORMATE_RESULT_SYMBOL,
        }),
        body: Some(&DelegateBody),
        default_body: None,
    }],
    plugin: &PLUGIN,
};

pub static VALUE_ENUM: PluginEnum = PluginEnum {
    variants: &[
        Variant {
            name: "int",
            path: &WrittenPath {
                crate_: CRATE,
                generics: &Generics::none(),
                path: "runtime::value::Int",
                project_generic: false,
            },
            is_unit: false,
        },
        Variant {
            name: "string",
            path: &WrittenPath {
                crate_: CRATE,
                generics: &Generics::none(),
                path: "runtime::StringId",
                project_generic: false,
            },
            is_unit: false,
        },
        Variant {
            name: "array",
            path: &WrittenPath {
                crate_: CRATE,
                generics: &Generics::none(),
                path: "runtime::value::Array",
                project_generic: false,
            },
            is_unit: false,
        },
        Variant {
            name: "table",
            path: &WrittenPath {
                crate_: CRATE,
                generics: &Generics::none(),
                path: "runtime::value::Table",
                project_generic: false,
            },
            is_unit: false,
        },
        Variant {
            name: "unit",
            path: &WrittenPath {
                crate_: CRATE,
                generics: &Generics::none(),
                path: "runtime::value::Unit",
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
                generics: &Generics::none(),
                path: "runtime::operation::Sum",
                project_generic: false,
            },
            is_unit: true,
        },
        Variant {
            name: "index",
            path: &WrittenPath {
                crate_: CRATE,
                generics: &Generics::none(),
                path: "runtime::operation::Index",
                project_generic: false,
            },
            is_unit: true,
        },
        Variant {
            name: "find",
            path: &WrittenPath {
                crate_: CRATE,
                generics: &Generics::none(),
                path: "runtime::operation::Find",
                project_generic: false,
            },
            is_unit: true,
        },
    ],
    plugin: &PLUGIN,
};
pub static DIAGNOSTIC_KIND_ENUM: PluginEnum = PluginEnum {
    variants: &[Variant {
        name: "equality_error",
        path: &WrittenPath {
            crate_: CRATE,
            generics: &Generics::none(),
            path: "runtime::diagnostic::EqualityError",
            project_generic: false,
        },
        is_unit: false,
    }],
    plugin: &PLUGIN,
};
pub static SUM_EXPR: Expr = Expr {
    name: "sum",
    params: &ExprParams(&[ExprParam {
        name: "addends",
        is_array: false,
    }]),
};
pub static INDEX_EXPR: Expr = Expr {
    name: "index",
    params: &ExprParams(&[
        ExprParam {
            name: "array",
            is_array: false,
        },
        ExprParam {
            name: "index",
            is_array: false,
        },
    ]),
};
pub static FIND_EXPR: Expr = Expr {
    name: "find",
    params: &ExprParams(&[
        ExprParam {
            name: "table",
            is_array: false,
        },
        ExprParam {
            name: "name",
            is_array: false,
        },
    ]),
};
pub const CRATE: &'static str = "lichen_core";
