use lichen_project::{
    Annotation, ArrayDisplay, AsTrait, CLONE, DEBUG, DelegateBody, Derives, EnumType,
    FORMATE_RESULT_SYMBOL, FORMATTER_PARAM, Function, Generics, HASH, Module, Name, PARTIAL_EQ,
    PROJECT, PROJECT_GENERIC, PROJECT_TRAIT, Param, Params, PassMode, Plugin, PluginEnum,
    PluginSymbol, ProjectVariable, SELF_SYMBOL, Self_, Symbol, Variant,
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
                    &PluginSymbol {
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
                    &PluginSymbol {
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
                        &PluginSymbol {
                            crate_: CRATE,
                            relative: "runtime::NodeIdLocal",
                        },
                        &",&",
                        &PluginSymbol {
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
                    &PluginSymbol {
                        crate_: CRATE,
                        relative: "runtime::solve::Solver",
                    },
                    &"<",
                    &ProjectVariable,
                    &">",
                ])),
                mutable: false,
            },
            &Param {
                name: "value",
                pass_mode: PassMode::Ref { lifetime: None },
                symbol: &Symbol::Dyn(&ArrayDisplay(&[
                    &AsTrait {
                        this: &ProjectVariable,
                        trait_: &PROJECT_TRAIT,
                    },
                    &"::Value",
                ])),
                mutable: false,
            },
            &Param {
                name: "node",
                pass_mode: PassMode::Ref { lifetime: None },
                symbol: &Symbol::Plugin(&PluginSymbol {
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
                    this: &ProjectVariable,
                    trait_: &PROJECT_TRAIT,
                },
                &"::Value>",
            ]),
        }),
        body: Some(&DelegateBody),
        default_body: None,
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
