use lichen_project::{
    ArrayDisplay, AsTrait, DEBUG, DelegateBody, Derives, EnumType, FORMATE_RESULT_SYMBOL,
    FORMATTER_PARAM, Function, Generics, Module, Name, PARTIAL_EQ, PROJECT, PROJECT_GENERIC,
    PROJECT_TRAIT, Param, Params, PassMode, Plugin, PluginEnum, PluginSymbol, ProjectVariable,
    Self_, Variant, WrittenSymbol,
};

pub static PLUGIN: Plugin = Plugin {
    name: "core",
    lib_crate_path: "core",
    lib_module: PluginSymbol {
        crate_: CRATE,
        relative: "plugin",
    },
    bin_module: Module::Path("core/tests/project"),
    dependencies: &[],
    enum_types: &[&VALUE_TYPE, &OPERATOR_TYPE, &DIAGNOSTIC_KIND_TYPE],
    plugin_enums: &[
        (&VALUE_TYPE, &VALUE_ENUM),
        (&OPERATOR_TYPE, &OPERATOR_ENUM),
        (&DIAGNOSTIC_KIND_TYPE, &DIAGNOSTIC_KIND_ENUM),
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
    base_traits: &[
        WrittenSymbol::Raw("std::fmt::Debug"),
        WrittenSymbol::Raw("Copy"),
        WrittenSymbol::Raw("Eq"),
    ],
    functions: &[],
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
    base_traits: &[
        WrittenSymbol::Raw("std::fmt::Debug"),
        WrittenSymbol::Raw("Copy"),
        WrittenSymbol::Raw("Eq"),
    ],
    functions: &[Function {
        name: "run",
        self_: Some(Self_(PassMode::Ref { lifetime: None })),
        params: &Params(&[
            &Param {
                name: "solver",
                pass_mode: PassMode::RefMut { lifetime: None },
                symbol: &ArrayDisplay(&[
                    &PluginSymbol {
                        crate_: CRATE,
                        relative: "runtime::solve::Solver",
                    },
                    &"<",
                    &ProjectVariable,
                    &">",
                ]),
            },
            &Param {
                name: "value",
                pass_mode: PassMode::Ref { lifetime: None },
                symbol: &ArrayDisplay(&[
                    &AsTrait {
                        this: &ProjectVariable,
                        trait_: &PROJECT_TRAIT,
                    },
                    &"::Value",
                ]),
            },
            &Param {
                name: "node",
                pass_mode: PassMode::Ref { lifetime: None },
                symbol: &WrittenSymbol::Plugin(&PluginSymbol {
                    crate_: CRATE,
                    relative: "runtime::solve::LocalNodeId",
                }),
            },
        ]),
        return_: Some(&ArrayDisplay(&[
            &"Option<",
            &AsTrait {
                this: &ProjectVariable,
                trait_: &PROJECT_TRAIT,
            },
            &"::Value>",
        ])),
        body: &DelegateBody,
    }],
};

pub static DIAGNOSTIC_KIND_TYPE: EnumType = EnumType {
    name: &Name {
        name: "DiagnosticKind",
        project_generic: true,
        generics: &Generics(&[]),
    },
    is_unit: false,
    derives: &Derives(&[]),
    markers: &[],
    impls: &[&DEBUG],
    base_traits: &[WrittenSymbol::Raw("std::fmt::Debug")],
    functions: &[Function {
        name: "message",
        self_: Some(Self_(PassMode::Ref { lifetime: None })),
        params: &Params(&[&FORMATTER_PARAM]),
        return_: Some(&FORMATE_RESULT_SYMBOL),
        body: &DelegateBody,
    }],
};

pub static VALUE_ENUM: PluginEnum = PluginEnum {
    variants: &[
        Variant {
            name: "int",
            symbol: &Name {
                name: &PluginSymbol {
                    crate_: CRATE,
                    relative: "runtime::value::Int",
                },
                generics: &Generics::none(),
                project_generic: false,
            },
            is_unit: false,
        },
        Variant {
            name: "string",
            symbol: &Name {
                name: &PluginSymbol {
                    crate_: CRATE,
                    relative: "runtime::StringId",
                },
                generics: &Generics::none(),
                project_generic: false,
            },
            is_unit: false,
        },
        Variant {
            name: "array",
            symbol: &Name {
                name: &PluginSymbol {
                    crate_: CRATE,
                    relative: "runtime::value::Array",
                },
                generics: &Generics::none(),
                project_generic: false,
            },
            is_unit: false,
        },
        Variant {
            name: "table",
            symbol: &Name {
                name: &PluginSymbol {
                    crate_: CRATE,
                    relative: "runtime::value::Table",
                },
                generics: &Generics::none(),
                project_generic: false,
            },
            is_unit: false,
        },
        Variant {
            name: "unit",
            symbol: &Name {
                name: &PluginSymbol {
                    crate_: CRATE,
                    relative: "runtime::value::Unit",
                },
                generics: &Generics::none(),
                project_generic: false,
            },
            is_unit: true,
        },
    ],
};

pub static OPERATOR_ENUM: PluginEnum = PluginEnum {
    variants: &[
        Variant {
            name: "sum",
            symbol: &Name {
                name: &PluginSymbol {
                    crate_: CRATE,
                    relative: "runtime::operation::Sum",
                },
                generics: &Generics::none(),
                project_generic: false,
            },
            is_unit: true,
        },
        Variant {
            name: "index",
            symbol: &Name {
                name: &PluginSymbol {
                    crate_: CRATE,
                    relative: "runtime::operation::Index",
                },
                generics: &Generics::none(),
                project_generic: false,
            },
            is_unit: true,
        },
        Variant {
            name: "find",
            symbol: &Name {
                name: &PluginSymbol {
                    crate_: CRATE,
                    relative: "runtime::operation::Find",
                },
                generics: &Generics::none(),
                project_generic: false,
            },
            is_unit: true,
        },
    ],
};
pub static DIAGNOSTIC_KIND_ENUM: PluginEnum = PluginEnum {
    variants: &[Variant {
        name: "equality_error",
        symbol: &Name {
            name: &PluginSymbol {
                crate_: CRATE,
                relative: "runtime::diagnostic::EqualityError",
            },
            generics: &Generics::none(),
            project_generic: false,
        },
        is_unit: false,
    }],
};
pub const CRATE: &'static str = "lichen_core";
