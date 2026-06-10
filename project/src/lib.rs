use std::collections::{HashMap, HashSet};
use std::env::current_dir;
use std::future;
use std::io::Write;
use std::iter::once;
use std::marker::PhantomData;
use std::{fmt::Display, fs::OpenOptions};

use by_address::ByAddress;
impl Plugin {
    pub fn generate(&'static self) -> std::io::Result<()> {
        let current_dir = current_dir()?;
        CTX.set(Some(Context {
            current: GenerateStage::Lib,
            plugin: self,
        }));
        let path = current_dir
            .join(self.lib_crate_path)
            .join("src")
            .join(self.lib_module.relative)
            .with_extension("rs");
        println!("generate lib at: {}", path.to_string_lossy());
        let mut file = OpenOptions::new()
            .write(true)
            .truncate(true)
            .create(true)
            .open(path)?;

        writeln!(file, "{}", self)?;
        CTX.set(Some(Context {
            current: GenerateStage::Bin,
            plugin: self,
        }));
        let path = match &self.bin_module {
            Module::Symbol { crate_path, symbol } => current_dir
                .join(crate_path)
                .join("src")
                .join(symbol.relative),
            Module::Path(path) => current_dir.join(path),
        }
        .with_extension("rs");
        println!("generate bin at: {}", path.to_string_lossy());
        let mut file = OpenOptions::new()
            .write(true)
            .truncate(true)
            .create(true)
            .open(path)?;
        writeln!(file, "{}", Project(self))?;
        CTX.set(None);
        Ok(())
    }
}

pub struct ArrayDisplay(pub &'static [&'static (dyn Display + Sync)]);
impl Display for ArrayDisplay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for i in self.0 {
            write!(f, "{i}")?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
pub enum GenerateStage {
    Lib,
    Bin,
}

#[derive(Clone, Copy)]
pub struct Context {
    pub current: GenerateStage,
    pub plugin: &'static Plugin,
}

pub struct Delegate<'a, T>(pub &'a T);
pub struct Impl<'a, T, P: Display> {
    pub this: &'a T,
    pub project: &'a P,
}

thread_local! {
    pub static CTX: std::cell::Cell<Option<Context>> = std::cell::Cell::new(None);
}

pub static PROJECT: &'static str = "Project";
pub static PROJECT_TRAIT: Symbol = Symbol::GeneratedLib {
    relative: "",
    name: PROJECT,
};

pub static PROJECT_GENERIC: Generic = Generic {
    name: PROJECT,
    constraints: &[&Symbol::GeneratedLib {
        relative: "",
        name: PROJECT,
    }],
};

pub struct ProjectVariable;
impl Display for ProjectVariable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let ctx = CTX.get().unwrap();
        match ctx.current {
            GenerateStage::Lib => write!(f, "{}", PROJECT_GENERIC.name),
            GenerateStage::Bin => write!(f, "{PROJECT}"),
        }
    }
}

pub struct AsTrait<This: Display, Trait: Display> {
    pub this: This,
    pub trait_: Trait,
}
impl<This: Display, Trait: Display> Display for AsTrait<This, Trait> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "<{} as {}>", self.this, self.trait_)
    }
}

pub static PARTIAL_EQ: Trait = Trait {
    symbol: &Symbol::Raw("PartialEq"),
    generics: &Generics::none(),
    functions: &[
        Function {
            name: "eq",
            self_: Some(Self_(PassMode::Ref { lifetime: None })),
            params: &Params(&[&Param {
                name: "other",
                pass_mode: PassMode::Ref { lifetime: None },
                symbol: &SELF_SYMBOL,
                mutable: false,
            }]),
            return_: Some(Annotation {
                impl_: false,
                symbol: &BOOL_SYMBOL,
            }),
            body: Some(&PartialEqBody { eq_or_ne: true }),
            default_body: None,
        },
        Function {
            name: "ne",
            self_: Some(Self_(PassMode::Ref { lifetime: None })),
            params: &Params(&[&Param {
                name: "other",
                pass_mode: PassMode::Ref { lifetime: None },
                symbol: &SELF_SYMBOL,
                mutable: false,
            }]),
            return_: Some(Annotation {
                impl_: false,
                symbol: &BOOL_SYMBOL,
            }),
            body: Some(&PartialEqBody { eq_or_ne: false }),
            default_body: None,
        },
    ],
};

pub static DEBUG: Trait = Trait {
    symbol: &Symbol::Raw("std::fmt::Debug"),
    generics: &Generics::none(),
    functions: &[Function {
        name: "fmt",
        self_: Some(Self_(PassMode::Ref { lifetime: None })),
        params: &Params(&[&FORMATTER_PARAM]),
        return_: Some(Annotation {
            impl_: false,
            symbol: &FORMATE_RESULT_SYMBOL,
        }),
        body: Some(&DebugBody),
        default_body: None,
    }],
};

pub static FORMATTER_PARAM: Param = Param {
    name: "f",
    pass_mode: PassMode::RefMut { lifetime: None },
    symbol: &Symbol::Raw("std::fmt::Formatter<'_>"),
    mutable: false,
};

pub static FORMATE_RESULT_SYMBOL: Symbol = Symbol::Raw("std::fmt::Result");

pub static SELF_SYMBOL: Symbol = Symbol::Self_;
pub static BOOL_SYMBOL: Symbol = Symbol::Raw("bool");

pub static PRINCIPAL_TRAITS: &'static str = "principal_traits";
pub static MANUALLY_DROP: &'static str = "std::mem::ManuallyDrop";

pub struct EnumType {
    pub name: &'static Name<&'static str>,
    pub is_unit: bool,
    pub derives: &'static Derives,
    pub markers: &'static [&'static str],
    pub impls: &'static [&'static Trait],
    pub base_traits: &'static [Symbol],
    pub functions: &'static [Function],
}
pub struct Lib;
impl Display for Lib {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let ctx = CTX.get().unwrap();
        write!(f, "{}", ctx.plugin.lib_module)
    }
}

#[derive(Clone, Copy)]
pub enum Symbol {
    Plugin(&'static PluginSymbol),
    Raw(&'static str),
    Dyn(&'static (dyn Display + Sync)),
    GeneratedLib {
        relative: &'static str,
        name: &'static str,
    },
    GeneratedBin {
        relative: &'static str,
        name: &'static str,
    },
    Self_,
}

impl Display for Symbol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Symbol::Plugin(symbol) => write!(f, "{}", symbol),
            Symbol::Raw(symbol) => write!(f, "{}", symbol),
            Symbol::Dyn(symbol) => write!(f, "{}", symbol),
            Symbol::GeneratedLib { relative, name } => {
                write!(f, "{Lib}")?;
                if !relative.is_empty() {
                    write!(f, "::{relative}")?;
                }
                writeln!(f, "::{}", name)?;
                Ok(())
            }
            Symbol::GeneratedBin { relative, name } => {
                write!(f, "crate")?;
                if !relative.is_empty() {
                    write!(f, "::{relative}")?;
                }
                writeln!(f, "::{}", name)?;
                Ok(())
            }
            Symbol::Self_ => write!(f, "Self"),
        }
    }
}

#[derive(Clone, Copy)]
pub struct Annotation {
    pub impl_: bool,
    pub symbol: &'static (dyn Display + Sync),
}

impl Display for Annotation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}{}",
            if self.impl_ { "impl " } else { "" },
            self.symbol
        )
    }
}

pub struct Name<T: Display> {
    pub name: T,
    pub generics: &'static Generics,
    pub project_generic: bool,
}

impl<T: Display> Display for Name<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}<", self.name)?;
        if self.project_generic {
            write!(f, "{}", PROJECT_GENERIC)?;
        }
        write!(f, "{}>", self.generics)?;
        Ok(())
    }
}

impl<T: Display> Display for Delegate<'_, Name<T>> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}::<", self.0.name)?;
        if self.0.project_generic {
            write!(f, "{}", PROJECT_GENERIC.name)?;
        }
        write!(f, "{}>", Delegate(self.0.generics))?;
        Ok(())
    }
}

impl<T: Display, P: Display> Display for Impl<'_, Name<T>, P> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}<", self.this.name)?;
        write!(f, "{}>", self.this.generics)?;
        Ok(())
    }
}

impl<T: Display, P: Display> Display for Delegate<'_, Impl<'_, Name<T>, P>> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}::<", self.0.this.name)?;
        if self.0.this.project_generic {
            write!(f, "{}", self.0.project)?;
        }
        write!(f, "{}>", Delegate(self.0.this.generics))?;
        Ok(())
    }
}

pub struct Generic {
    pub name: &'static str,
    pub constraints: &'static [&'static Symbol],
}

impl Display for Generic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: ", self.name)?;
        for constraint in self.constraints {
            write!(f, "{}+", constraint)?;
        }
        Ok(())
    }
}

pub struct Generics(pub &'static [&'static Generic]);

impl Generics {
    pub const fn none() -> Self {
        Self(&[])
    }
}

impl Display for Generics {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for generic in self.0 {
            write!(f, "{}", generic)?;
        }
        Ok(())
    }
}

impl Display for Delegate<'_, Generics> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for generic in self.0.0 {
            write!(f, "{},", generic.name)?;
        }
        Ok(())
    }
}

pub struct Plugin {
    pub name: &'static str,
    pub lib_crate_path: &'static str,
    pub lib_module: PluginSymbol,
    pub bin_module: Module,
    pub dependencies: &'static [&'static Plugin],
    pub enum_types: &'static [&'static EnumType],
    pub plugin_enums: &'static [(&'static EnumType, &'static PluginEnum)],
}

pub enum Module {
    Symbol {
        crate_path: &'static str,
        symbol: PluginSymbol,
    },
    Path(&'static str),
}

impl Display for Plugin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "pub mod {PRINCIPAL_TRAITS}{{")?;
        for enum_type in self.enum_types {
            write!(f, "pub trait {}:", enum_type.name)?;
            for base in enum_type.base_traits {
                write!(f, "{}+", base)?;
            }
            writeln!(f, "{{")?;
            for function in enum_type.functions {
                write!(f, "{}", function)?;
            }
            writeln!(f, "}}")?;
        }
        writeln!(f, "}}")?;
        write!(
            f,
            "pub trait {PROJECT}: std::fmt::Debug + Default + Copy + Eq + 'static"
        )?;
        for dependency in self.dependencies {
            write!(f, "{}::{}+", dependency.lib_module, PROJECT)?;
        }
        writeln!(f, "{{")?;
        for enum_type in self.enum_types {
            writeln!(
                f,
                "type {}:self::{}::{};",
                Impl {
                    this: enum_type.name,
                    project: &SELF_SYMBOL
                },
                PRINCIPAL_TRAITS,
                Delegate(&Impl {
                    this: enum_type.name,
                    project: &SELF_SYMBOL
                })
            )?;
        }
        writeln!(f, "}}")?;

        for (enum_type, plugin_enum) in self.plugin_enums {
            writeln!(
                f,
                "pub trait {}:self::{}::{}{{",
                enum_type.name,
                PRINCIPAL_TRAITS,
                Delegate(enum_type.name)
            )?;
            if enum_type.is_unit {
                for variant in plugin_enum.variants.iter() {
                    writeln!(f, "fn {}()->Self;", variant.name)?
                }
            } else {
                for variant in plugin_enum.variants.iter() {
                    if variant.is_unit {
                        writeln!(f, "fn {}(self)->bool;", variant.name)?;
                        writeln!(f, "fn from_{}()->Self;", variant.name)?;
                    } else {
                        writeln!(
                            f,
                            "fn {}(self)->Option<{}>;",
                            variant.name,
                            Delegate(variant.symbol)
                        )?;
                        writeln!(
                            f,
                            "fn from_{}(data: {})->Self;",
                            variant.name,
                            Delegate(variant.symbol)
                        )?;
                    }
                }
            }
            writeln!(f, "}}")?;
        }
        Ok(())
    }
}

pub struct Project(&'static Plugin);

impl Display for Project {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "use {PROJECT_TRAIT} as _;")?;
        #[derive(Default)]
        struct Ctx {
            plugins: HashSet<ByAddress<&'static Plugin>>,
            plugin_enums:
                HashMap<ByAddress<&'static EnumType>, Vec<(&'static PluginEnum, &'static Plugin)>>,
        }
        let mut ctx = Ctx::default();
        fn collect(plugin: &'static Plugin, ctx: &mut Ctx) {
            if ctx.plugins.insert(ByAddress(plugin)) {
                for (enum_type, plugin_enum) in plugin.plugin_enums {
                    ctx.plugin_enums
                        .entry(ByAddress(enum_type))
                        .or_default()
                        .push((plugin_enum, plugin));
                }
                for dependency in plugin.dependencies.iter().copied() {
                    collect(dependency, ctx);
                }
            }
        }
        collect(&self.0, &mut ctx);

        writeln!(f, "#[derive(Debug,Default,Copy,Clone,PartialEq,Eq,Hash)]")?;
        writeln!(f, "pub struct Project;")?;
        for plugin in ctx.plugins {
            writeln!(f, "impl {PROJECT_TRAIT} for Project{{")?;
            for (enum_type, _) in plugin.plugin_enums {
                write!(
                    f,
                    "type {} = {}<{}>;",
                    Impl {
                        this: enum_type.name,
                        project: &SELF_SYMBOL
                    },
                    enum_type.name.name,
                    Delegate(enum_type.name.generics),
                )?;
            }
            writeln!(f, "}}")?;
        }

        writeln!(f, "mod unions{{")?;
        writeln!(f, "use super::{PROJECT};")?;
        for (enum_type, plugin_enums) in &ctx.plugin_enums {
            if !enum_type.is_unit {
                writeln!(f, "{}", enum_type.derives)?;
                writeln!(
                    f,
                    "pub(super) union {}<{}>{{",
                    enum_type.name.name, enum_type.name.generics
                )?;
                for (plugin_enum, plugin) in plugin_enums {
                    for variant in plugin_enum.variants {
                        writeln!(
                            f,
                            "pub(super) {}__{}: {MANUALLY_DROP}<{}>,",
                            plugin.name,
                            variant.name,
                            Delegate(&Impl {
                                this: variant.symbol,
                                project: &PROJECT
                            })
                        )?;
                    }
                }
                writeln!(f, "}}")?;
            }
        }
        writeln!(f, "}}")?;
        for (enum_type, plugin_enums) in &ctx.plugin_enums {
            writeln!(f, "{}", enum_type.derives)?;
            if enum_type.is_unit {
                writeln!(
                    f,
                    "pub struct {}<{}>(usize);",
                    enum_type.name.name, enum_type.name.generics
                )?;
            } else {
                writeln!(
                    f,
                    "pub struct {0}<{1}>{{code:usize,data:self::unions::{0}<{1}>}}",
                    enum_type.name.name, enum_type.name.generics
                )?;
            }

            writeln!(
                f,
                "{}",
                MarkersDisplay {
                    markers: enum_type.markers,
                    enum_type
                }
            )?;

            for (name, functions) in enum_type
                .impls
                .iter()
                .map(|x| {
                    (
                        Name {
                            name: *x.symbol,
                            generics: x.generics,
                            project_generic: false,
                        },
                        x.functions,
                    )
                })
                .chain(once((
                    Name {
                        name: Symbol::GeneratedLib {
                            relative: PRINCIPAL_TRAITS,
                            name: enum_type.name.name,
                        },
                        generics: enum_type.name.generics,
                        project_generic: enum_type.name.project_generic,
                    },
                    enum_type.functions,
                )))
            {
                writeln!(
                    f,
                    "impl{} {} for {}{}{{",
                    name.generics,
                    Delegate(&Impl {
                        this: &name,
                        project: &PROJECT
                    }),
                    enum_type.name.name,
                    enum_type.name.generics,
                )?;
                for function in functions {
                    let mut code = 0;
                    writeln!(
                        f,
                        "{}{{",
                        FunctionImpl {
                            function,
                            body: None
                        }
                    )?;
                    if let Some(body) = function.body {
                        body.generate_pre_match(f, enum_type, function)?;
                        if enum_type.is_unit {
                            writeln!(f, "match self.0{{")?;
                        } else {
                            writeln!(f, "match self.code{{")?;
                        }
                        for (plugin_enum, plugin) in plugin_enums {
                            for variant in plugin_enum.variants {
                                write!(f, "{code}=>{{")?;
                                body.generate_match_branch(
                                    f, enum_type, function, variant, plugin,
                                )?;
                                write!(f, "}}")?;
                                code += 1;
                            }
                            write!(f, "_=>unreachable!(),")?;
                        }
                        writeln!(f, "}}")?;
                    } else {
                        writeln!(f, "unimplemented!();")?;
                    }
                    writeln!(f, "}}")?;
                }
                writeln!(f, "}}")?;
            }

            {
                let mut code = 0;
                let mut code2variant = Vec::<(&Variant, &Plugin)>::new();
                for (plugin_enum, plugin) in plugin_enums {
                    writeln!(
                        f,
                        "impl<{}> {Lib}::{} for {}<{}>{{",
                        enum_type.name.generics,
                        Delegate(&Impl {
                            this: enum_type.name,
                            project: &PROJECT
                        }),
                        enum_type.name.name,
                        Delegate(enum_type.name.generics),
                    )?;
                    for variant in plugin_enum.variants {
                        code2variant.push((variant, plugin));
                        if enum_type.is_unit {
                            writeln!(f, "fn {}()->Self{{Self({})}}", variant.name, code)?
                        } else {
                            if variant.is_unit {
                                writeln!(
                                    f,
                                    "fn {}(self)->bool{{self.code=={}}}",
                                    variant.name, code
                                )?;
                                writeln!(
                                    f,
                                    "fn from_{0}()->Self{{Self{{code:{1},data:self::unions::{2}{3}{{{4}__{0}: {MANUALLY_DROP}::new({5})}} }} }}",
                                    variant.name,
                                    code,
                                    enum_type.name.name,
                                    Delegate(enum_type.name.generics),
                                    plugin.name,
                                    Delegate(&Impl {
                                        this: variant.symbol,
                                        project: &PROJECT
                                    })
                                )?;
                            } else {
                                writeln!(
                                    f,
                                    "fn {}(self)->Option<{}>{{if self.code=={}{{Some(unsafe{{*self.data.{}__{}}})}}else{{None}} }}",
                                    variant.name,
                                    Delegate(&Impl {
                                        this: variant.symbol,
                                        project: &PROJECT
                                    }),
                                    code,
                                    plugin.name,
                                    variant.name
                                )?;
                                writeln!(
                                    f,
                                    "fn from_{}(data: {})->Self{{Self{{code:{},data:self::unions::{}{{{}__{}:{MANUALLY_DROP}::new(data)}} }} }}",
                                    variant.name,
                                    Delegate(&Impl {
                                        this: variant.symbol,
                                        project: &PROJECT
                                    }),
                                    code,
                                    enum_type.name.name,
                                    plugin.name,
                                    variant.name
                                )?;
                            }
                        }
                        code += 1;
                    }
                    writeln!(f, "}}")?;
                }
            }
        }
        Ok(())
    }
}

pub struct Derives(pub &'static [&'static str]);
impl Display for Derives {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if !self.0.is_empty() {
            write!(f, "#[derive(")?;
            for derive in self.0 {
                write!(f, "{},", derive)?;
            }
            write!(f, ")]")?;
        }
        Ok(())
    }
}
pub struct MarkersDisplay {
    pub markers: &'static [&'static str],
    pub enum_type: &'static EnumType,
}
impl Display for MarkersDisplay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for marker in self.markers {
            writeln!(f, "impl {} for {}{{}}", marker, self.enum_type.name.name)?;
        }
        Ok(())
    }
}

pub struct PluginEnum {
    pub variants: &'static [Variant],
}

pub struct Variant {
    pub name: &'static str,
    pub symbol: &'static Name<&'static PluginSymbol>,
    pub is_unit: bool,
}

pub struct PluginSymbol {
    pub crate_: &'static str,
    pub relative: &'static str,
}

impl Display for PluginSymbol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}::{}", Crate(self.crate_), self.relative)
    }
}

pub struct Crate(&'static str);

impl Display for Crate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let ctx = CTX.get().unwrap();
        if matches!(ctx.current, GenerateStage::Lib) && ctx.plugin.lib_module.crate_ == self.0 {
            write!(f, "crate")
        } else {
            write!(f, "::{}", self.0)
        }
    }
}

pub struct Trait {
    pub symbol: &'static Symbol,
    pub generics: &'static Generics,
    pub functions: &'static [Function],
}

pub struct Function {
    pub name: &'static str,
    pub self_: Option<Self_>,
    pub params: &'static Params,
    pub return_: Option<Annotation>,
    pub body: Option<&'static dyn Body>,
    pub default_body: Option<&'static (dyn Display + Sync)>,
}

pub struct FunctionImpl<'a> {
    function: &'a Function,
    body: Option<&'a (dyn Display + Sync)>,
}

impl Display for FunctionImpl<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let function = self.function;
        write!(f, "fn {}(", function.name)?;
        if let Some(self_) = function.self_.as_ref() {
            write!(f, "{}", self_)?;
        }
        write!(f, "{})", function.params)?;
        if let Some(return_) = function.return_ {
            write!(f, "->{}", return_)?;
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

pub struct Params(pub &'static [&'static Param]);

impl Display for Params {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for param in self.0 {
            write!(f, "{}", param)?;
        }
        Ok(())
    }
}

impl Display for Delegate<'_, Params> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for param in self.0.0 {
            write!(f, "{},", param.name)?;
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
    pub symbol: &'static Symbol,
    pub mutable: bool,
}

impl Display for Param {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}{}: {}{},",
            if self.mutable { "mut " } else { "" },
            self.name,
            self.pass_mode,
            self.symbol
        )
    }
}

pub trait Body: Send + Sync {
    fn generate_pre_match(
        &self,
        f: &mut std::fmt::Formatter<'_>,
        enum_type: &EnumType,
        function: &Function,
    ) -> std::fmt::Result;
    fn generate_match_branch(
        &self,
        f: &mut std::fmt::Formatter<'_>,
        enum_type: &EnumType,
        function: &Function,
        variant: &Variant,
        plugin: &Plugin,
    ) -> std::fmt::Result;
}

pub struct DelegateBody;

impl Body for DelegateBody {
    fn generate_pre_match(
        &self,
        f: &mut std::fmt::Formatter<'_>,
        enum_type: &EnumType,
        function: &Function,
    ) -> std::fmt::Result {
        Ok(())
    }

    fn generate_match_branch(
        &self,
        f: &mut std::fmt::Formatter<'_>,
        enum_type: &EnumType,
        function: &Function,
        variant: &Variant,
        plugin: &Plugin,
    ) -> std::fmt::Result {
        if let Some(return_) = function.return_
            && return_.impl_
        {
            write!(f, "std::boxed::Box::new(")?;
        }
        write!(
            f,
            "<{} as {Lib}::{PRINCIPAL_TRAITS}::{}>::{}(",
            Delegate(&Impl {
                this: variant.symbol,
                project: &PROJECT
            }),
            Delegate(&Impl {
                this: enum_type.name,
                project: &PROJECT
            }),
            function.name
        )?;
        if let Some(self_) = &function.self_ {
            if enum_type.is_unit {
                write!(
                    f,
                    "unsafe{{{}{}}}",
                    self_.0,
                    Delegate(&Impl {
                        this: variant.symbol,
                        project: &PROJECT
                    })
                )?;
            } else {
                write!(
                    f,
                    "unsafe{{{}self.data.{}__{}}}",
                    self_.0, plugin.name, variant.name
                )?;
            }
            write!(f, ",")?;
        }
        for param in function.params.0 {
            if matches!(param.symbol, Symbol::Self_) {
                write!(
                    f,
                    "unsafe{{{}{}.data.{}__{}}},",
                    param.pass_mode, param.name, plugin.name, variant.name
                )?;
            } else {
                write!(f, "{},", param.name)?;
            }
        }
        writeln!(f, ")")?;
        if let Some(return_) = function.return_
            && return_.impl_
        {
            write!(f, ") as std::boxed::Box<dyn {}>", return_.symbol)?;
        }
        Ok(())
    }
}

pub struct PartialEqBody {
    pub eq_or_ne: bool,
}

impl Body for PartialEqBody {
    fn generate_pre_match(
        &self,
        f: &mut std::fmt::Formatter<'_>,
        enum_type: &EnumType,
        function: &Function,
    ) -> std::fmt::Result {
        assert!(!enum_type.is_unit);
        writeln!(f, "if self.code!=other.code{{return {}}}", !self.eq_or_ne)
    }

    fn generate_match_branch(
        &self,
        f: &mut std::fmt::Formatter<'_>,
        enum_type: &EnumType,
        function: &Function,
        variant: &Variant,
        plugin: &Plugin,
    ) -> std::fmt::Result {
        assert!(!enum_type.is_unit);
        writeln!(
            f,
            "unsafe{{self.data.{0}__{1}{2}other.data.{0}__{1}}}",
            plugin.name,
            variant.name,
            if self.eq_or_ne { "==" } else { "!=" }
        )
    }
}

pub struct DebugBody;

impl Body for DebugBody {
    fn generate_pre_match(
        &self,
        f: &mut std::fmt::Formatter<'_>,
        enum_type: &EnumType,
        function: &Function,
    ) -> std::fmt::Result {
        Ok(())
    }

    fn generate_match_branch(
        &self,
        f: &mut std::fmt::Formatter<'_>,
        enum_type: &EnumType,
        function: &Function,
        variant: &Variant,
        plugin: &Plugin,
    ) -> std::fmt::Result {
        write!(f, "write!(f,\"{}::{}", plugin.name, variant.name)?;
        if enum_type.is_unit || variant.is_unit {
            write!(f, "\")")?;
        } else {
            write!(
                f,
                "({{:?}})\",unsafe{{*self.data.{}__{}}})",
                plugin.name, variant.name
            )?;
        }
        Ok(())
    }
}

pub struct VariantData {
    pub variant_name: &'static str,
}

impl Display for VariantData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "self.data.{}", self.variant_name)
    }
}
