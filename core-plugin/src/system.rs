//! plugin systm via code generation
use std::fmt::Display;

use sytax::WrittenPath;

use crate::CRATE;
use crate::system::enum_impl::{Clone, DebugBody, EnumImpl, Hash, PartialEq};
use crate::system::sytax::{
    Declare, Derives, Generic, Generics, Name, ThisGeneratedLibPath, WrittenPathRaw,
};

pub mod enum_impl;
pub mod generate;
pub mod sytax;
pub mod utils;

pub static PARTIAL_EQ: Trait = Trait {
    symbol: &"PartialEq",
    functions: &[
        Function {
            name: "eq",
            generics: &[],
            self_: Some(Self_(PassMode::Ref { lifetime: None })),
            params: &Params::simple(&[&Param {
                name: "other",
                pass_mode: PassMode::Ref { lifetime: None },
                type_: &"Self",
                mutable: false,
            }]),
            return_: Some(&"bool"),
            enum_impl: Some(&PartialEq { eq_or_ne: true }),
            default_body: None,
            return_impl: false,
        },
        Function {
            name: "ne",
            generics: &[],
            self_: Some(Self_(PassMode::Ref { lifetime: None })),
            params: &Params::simple(&[&Param {
                name: "other",
                pass_mode: PassMode::Ref { lifetime: None },
                type_: &"Self",
                mutable: false,
            }]),
            return_: Some(&"bool"),
            enum_impl: Some(&PartialEq { eq_or_ne: false }),
            default_body: None,
            return_impl: false,
        },
    ],
};

pub static HASH: Trait = Trait {
    symbol: &"std::hash::Hash",
    functions: &[Function {
        name: "hash",
        generics: &[&Generic {
            name: "H",
            constraints: &[&"std::hash::Hasher"],
        }],
        self_: Some(Self_(PassMode::Ref { lifetime: None })),
        params: &Params::simple(&[&Param {
            name: "state",
            pass_mode: PassMode::RefMut { lifetime: None },
            type_: &"H",
            mutable: false,
        }]),
        return_: None,
        enum_impl: Some(&Hash),
        default_body: None,
        return_impl: false,
    }],
};

pub static DEBUG: Trait = Trait {
    symbol: &"std::fmt::Debug",
    functions: &[Function {
        name: "fmt",
        generics: &[],
        self_: Some(Self_(PassMode::Ref { lifetime: None })),
        params: &Params::simple(&[&FORMATTER_PARAM]),
        return_: Some(&FORMATE_RESULT),
        enum_impl: Some(&DebugBody),
        default_body: None,
        return_impl: false,
    }],
};

pub static CLONE: Trait = Trait {
    symbol: &"Clone",
    functions: &[Function {
        name: "clone",
        generics: &[],
        self_: Some(Self_(PassMode::Ref { lifetime: None })),
        params: &Params::simple(&[]),
        return_: Some(&"Self"),
        enum_impl: Some(&Clone),
        default_body: None,
        return_impl: false,
    }],
};

pub static PROJECT_NAME: Name = Name {
    name: "Project",
    generics: &Generics::NONE,
    project_generic: false,
};

pub static AST_NAME: Name = Name {
    name: "Ast",
    generics: &Generics::NONE,
    project_generic: true,
};

pub static AST_TRAIT_PATH: WrittenPath = WrittenPath {
    crate_: CRATE,
    path: "ast::Ast",
    generics: &Generics::NONE,
    project_generic: true,
};

pub static AST_IMPL_PATH: WrittenPath = WrittenPath {
    crate_: CRATE,
    path: "ast::AstImpl",
    generics: &Generics::NONE,
    project_generic: true,
};

pub static PROPERTIES_COUNT: &'static str = "PROPERTIES_COUNT";
pub static THIS_PROJECT_TRAIT: ThisGeneratedLibPath = ThisGeneratedLibPath {
    relative: "",
    name: PROJECT_NAME,
};
pub static PROJECT_VARIABLE: &'static str = "P";
pub static THIS_PROJECT_GENERIC: Generic = Generic {
    name: PROJECT_VARIABLE,
    constraints: &[&THIS_PROJECT_TRAIT],
};
pub static AST_TRAIT: WrittenPath = WrittenPath::raw(CRATE, "ast::Ast");
pub static AST_IMPL: WrittenPath = WrittenPath::raw(CRATE, "ast::AstImpl");
pub static EXPR_ID: WrittenPath = WrittenPath::raw(CRATE, "ast::ExprId");
pub static NODE_ID_LOCAL: WrittenPath = WrittenPath::raw(CRATE, "runtime::NodeIdLocal");
pub static EVALUATION: WrittenPath = WrittenPath::raw(CRATE, "runtime::evaluation::Evaluation");
pub static PHANTOM_DATA: &'static str = "core::marker::PhantomData";

pub static FORMATTER_PARAM: Param = Param {
    name: "f",
    pass_mode: PassMode::RefMut { lifetime: None },
    type_: &"std::fmt::Formatter<'_>",
    mutable: false,
};

pub static FORMATE_RESULT: &'static str = "std::fmt::Result";
pub static PRINCIPAL_TRAITS: &'static str = "principal_traits";
pub static UNION: &'static str = "union_";
pub static MANUALLY_DROP: &'static str = "std::mem::ManuallyDrop";

pub struct Plugin {
    pub name: &'static str,
    pub lib_crate_path: &'static str,
    pub lib_module: WrittenPathRaw,
    pub bin_module: Module,
    pub dependencies: &'static [&'static Plugin],
    pub enum_types: &'static [&'static EnumType],
    pub plugin_enums: &'static [(&'static EnumType, &'static PluginEnum)],
    pub properties: &'static [&'static str],
    pub exprs: &'static [&'static Expr],
    pub expr_impls: &'static [ExprImpls],
}

pub struct EnumType {
    pub name: &'static sytax::Name,
    pub is_unit: bool,
    pub derives: &'static Derives,
    pub markers: &'static [&'static str],
    pub impls: &'static [&'static Trait],
    pub base_traits: &'static [&'static (dyn Display + Sync)],
    pub functions: &'static [Function],
    pub use_enum_types: &'static [&'static EnumType],
    pub plugin: &'static Plugin,
}

pub struct PluginEnum {
    pub variants: &'static [Variant],
    pub plugin: &'static Plugin,
}

pub struct Variant {
    pub name: &'static str,
    pub path: &'static WrittenPath,
    pub is_unit: bool,
}

pub struct Trait {
    pub symbol: &'static (dyn Display + Sync),
    pub functions: &'static [Function],
}

pub struct Function {
    pub name: &'static str,
    pub generics: &'static [&'static Generic],
    pub self_: Option<Self_>,
    pub params: &'static Params,
    pub return_: Option<&'static (dyn Display + Sync)>,
    pub enum_impl: Option<&'static dyn EnumImpl>,
    pub default_body: Option<&'static (dyn Display + Sync)>,
    pub return_impl: bool,
}

impl Function {
    pub fn generics(&self) -> Generics {
        Generics(self.generics)
    }
}

pub struct FunctionImpl<'a> {
    function: &'a Function,
    body: Option<&'a (dyn Display + Sync)>,
}

impl Display for FunctionImpl<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let function = self.function;
        write!(
            f,
            "fn {}<{}>(",
            function.name,
            Declare(&function.generics())
        )?;
        if let Some(self_) = function.self_.as_ref() {
            write!(f, "{}", self_)?;
        }
        write!(f, "{})", Declare(function.params))?;
        if let Some(return_) = function.return_ {
            write!(
                f,
                "->{} {}",
                if self.function.return_impl {
                    "impl"
                } else {
                    ""
                },
                return_
            )?;
        }
        if let Some(default_body) = self.body {
            writeln!(f, "{{{default_body}}}")?;
        }
        Ok(())
    }
}

impl Display for Function {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(default_body) = self.default_body {
            write!(
                f,
                "{}",
                FunctionImpl {
                    function: self,
                    body: Some(default_body)
                }
            )?;
        } else {
            write!(
                f,
                "{}",
                FunctionImpl {
                    function: self,
                    body: None
                }
            )?;
            writeln!(f, ";")?;
        }
        Ok(())
    }
}

pub struct Params {
    pub this: &'static [&'static Param],
    pub generics: &'static Generics,
}

impl Params {
    pub const fn simple(this: &'static [&'static Param]) -> Self {
        Self {
            this,
            generics: &Generics::NONE,
        }
    }
}

impl Display for Params {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for param in self.this {
            write!(f, "{}", param)?;
        }
        Ok(())
    }
}

impl Display for Declare<'_, Params> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for param in self.0.this {
            write!(f, "{}", Declare(*param))?;
        }
        Ok(())
    }
}

pub enum PassMode {
    Move,
    Ref { lifetime: Option<&'static str> },
    RefMut { lifetime: Option<&'static str> },
}

impl Display for PassMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PassMode::Move => write!(f, ""),
            PassMode::Ref { lifetime } => write!(f, "&{} ", lifetime.unwrap_or("")),
            PassMode::RefMut { lifetime } => write!(f, "&mut{} ", lifetime.unwrap_or("")),
        }
    }
}

pub struct Self_(pub PassMode);

impl Display for Self_ {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}self,", self.0)
    }
}

pub struct Param {
    pub name: &'static str,
    pub pass_mode: PassMode,
    pub type_: &'static (dyn Display + Sync),
    pub mutable: bool,
}

impl Display for Param {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{},", self.name)
    }
}

impl Display for Declare<'_, Param> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}{}: {}{},",
            if self.0.mutable { "mut " } else { "" },
            self.0.name,
            self.0.pass_mode,
            self.0.type_
        )
    }
}

pub struct Expr {
    pub name: &'static str,
    pub params: &'static Params,
}

pub struct ExprImpls {
    pub expr: &'static Expr,
    pub impls: &'static [&'static WrittenPathRaw],
}

pub enum Module {
    Lib {
        crate_path: &'static str,
        path: &'static str,
    },
    Bin(&'static str),
}
