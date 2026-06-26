use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::env::current_dir;
use std::hash::RandomState;
use std::io::Write;
use std::iter::once;
use std::ops::Deref;
use std::{fmt::Display, fs::OpenOptions};

use by_address::ByAddress;

use crate::system::sytax::EqualityProjectGeneric;
use crate::system::sytax::GenericsOf;
use crate::system::{
    AST_IMPL_PATH, AST_NAME, AST_TRAIT_PATH, EVALUATION, EXPR_ID, EnumType, Expr, FunctionImpl,
    MANUALLY_DROP, Module, NODE_ID_LOCAL, PHANTOM_DATA, PRINCIPAL_TRAITS, PROJECT_NAME,
    PROJECT_VARIABLE, PROPERTIES_COUNT, Plugin, PluginEnum, UNION, Variant,
};

use crate::system::sytax::{Declare, DisplayMarkers, Generics, Name, WithProject, WrittenPathRaw};
use crate::system::utils::{
    generated_expr, generated_principle_trait, generated_project_trait, generated_struct,
    generated_trait, generated_union,
};
use crate::{OPERATOR_TYPE, PLUGIN, VALUE_TYPE};

const GENERATED_FILE: &str = "// This file is @generated. Do not edit by hand.\n";
const GENERATED_RUST_FILE: &str =
    "#![allow(non_camel_case_types)]#![allow(non_snake_case)]#![allow(non_upper_case_globals)]\n";

impl Plugin {
    pub fn generate(&'static self) -> std::io::Result<()> {
        let project = Project::new(self);

        let current_dir = current_dir()?;
        CTX.set(Some(Context {
            current: GeneratingStage::Lib,
            plugin: self,
        }));
        let lib_path = current_dir
            .join(self.lib_crate_path)
            .join("src")
            .join(self.lib_module.path)
            .with_extension("rs");
        println!("generate lib at: {}", lib_path.to_string_lossy());
        let mut file = OpenOptions::new()
            .write(true)
            .truncate(true)
            .create(true)
            .open(&lib_path)?;

        writeln!(file, "{GENERATED_FILE}{GENERATED_RUST_FILE}",)?;
        writeln!(file, "{}", DisplayProjectLib(&project))?;
        drop(file);
        CTX.set(Some(Context {
            current: GeneratingStage::Bin,
            plugin: self,
        }));
        let bin_path = match &self.bin_module {
            Module::Lib { crate_path, path } => current_dir.join(crate_path).join("src").join(path),
            Module::Bin(path) => current_dir.join(path),
        }
        .with_extension("rs");
        println!("generate bin at: {}", bin_path.to_string_lossy());
        let mut file = OpenOptions::new()
            .write(true)
            .truncate(true)
            .create(true)
            .open(&bin_path)?;
        writeln!(file, "{GENERATED_FILE}{GENERATED_RUST_FILE}",)?;
        writeln!(file, "{}", DisplayProjectBin(&project))?;
        drop(file);

        for path in [&lib_path, &bin_path] {
            if let Err(e) = std::process::Command::new("rustfmt")
                .arg(path)
                .current_dir(&current_dir)
                .status()
            {
                eprintln!(
                    "warning: failed to run rustfmt on {}: {}",
                    path.display(),
                    e
                );
            }
        }

        CTX.set(None);
        Ok(())
    }
}

#[derive(Clone, Copy)]
pub struct Context {
    pub current: GeneratingStage,
    pub plugin: &'static Plugin,
}

thread_local! {
    pub static CTX: std::cell::Cell<Option<Context>> = std::cell::Cell::new(None);
}

pub struct ThisGeneratedLib;
impl Display for ThisGeneratedLib {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let ctx = CTX.get().unwrap();
        write!(f, "{}", ctx.plugin.lib_module)
    }
}

#[derive(Clone, Copy)]
pub enum GeneratingStage {
    Lib,
    Bin,
}

pub struct Project {
    plugin: &'static Plugin,
    ctx: ProjectAnalysis,
}

impl Project {
    pub fn new(plugin: &'static Plugin) -> Self {
        let mut ctx = ProjectAnalysis::default();
        fn collect(plugin: &'static Plugin, ctx: &mut ProjectAnalysis) {
            if let Entry::Vacant(entry) = ctx.plugins.entry(ByAddress(plugin)) {
                entry.insert((
                    HashSet::from_iter(plugin.enum_types.iter().map(|x| ByAddress(*x))),
                    HashSet::new(),
                ));
                for (enum_type, plugin_enum) in plugin.plugin_enums {
                    debug_assert!(
                        ctx.plugin_enums
                            .entry(ByAddress(enum_type))
                            .or_default()
                            .insert(ByAddress(plugin_enum.plugin), plugin_enum)
                            .is_none()
                    );
                }
                for expr_impls in plugin.expr_impls {
                    ctx.expr_impls
                        .entry(ByAddress(expr_impls.expr))
                        .or_default()
                        .extend(expr_impls.impls.iter().map(|x| (*x, plugin)));
                }
                for dependency in plugin.dependencies.iter().copied() {
                    collect(dependency, ctx);
                    let [
                        Some((this_enum_types, this_ancestors)),
                        Some((dependency_enum_type, dependency_ancestors)),
                    ] = ctx
                        .plugins
                        .get_disjoint_mut([&ByAddress(plugin), &ByAddress(dependency)])
                    else {
                        unreachable!()
                    };
                    this_enum_types.extend(dependency_enum_type.iter());
                    this_ancestors.insert(ByAddress(dependency));
                    this_ancestors.extend(dependency_ancestors.iter());
                }
                for enum_type in &ctx.plugins[&ByAddress(plugin)].0 {
                    ctx.enum_types.entry(*enum_type).or_default().push(plugin);
                }
            }
        }
        collect(&plugin, &mut ctx);
        Self { plugin, ctx }
    }
    pub fn fmt_lib(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        {
            writeln!(f, "pub mod {PRINCIPAL_TRAITS}{{")?;
            for enum_type in self.plugin.enum_types {
                write!(f, "pub trait {}: ", Declare(enum_type.name))?;
                for base in enum_type.base_traits {
                    write!(f, "{}+", base)?;
                }
                writeln!(f, "{{")?;
                for function in enum_type.functions {
                    write!(f, "{}", function)?;
                }
                writeln!(f, "}}")?;
            }

            {
                write!(f, "pub trait {}: ", Declare(&AST_NAME))?;
                writeln!(f, "{{")?;
                writeln!(f, "const {PROPERTIES_COUNT}: usize;")?;
                writeln!(f, "fn impl_(&self)->&{};", AST_IMPL_PATH)?;
                writeln!(f, "fn impl_mut(&mut self)->&mut {};", AST_IMPL_PATH)?;
                writeln!(f, "}}")?;
            }
            writeln!(f, "}}")?;
        }

        {
            write!(
                f,
                "pub trait {}: std::fmt::Debug+Default+Copy+Eq+std::hash::Hash+'static+",
                Declare(&PROJECT_NAME)
            )?;
            for dependency in self.plugin.dependencies {
                write!(f, "{}+", generated_project_trait(dependency))?;
            }
            writeln!(f, "")?;
            writeln!(f, "{{")?;
            for name in self.plugin.enum_types.iter().map(|x| x.name).chain(
                if ByAddress(self.plugin) == ByAddress(&PLUGIN) {
                    Some(&AST_NAME)
                } else {
                    None
                },
            ) {
                writeln!(
                    f,
                    "type {}: {} + {};",
                    Declare(&name.non_project_generic()),
                    &WithProject {
                        this: &generated_principle_trait(self.plugin, *name),
                        project: &"Self"
                    },
                    &WithProject {
                        this: &generated_trait(self.plugin, *name),
                        project: &"Self"
                    }
                )?;
            }
            writeln!(f, "}}")?;
        }

        let plugin_enums = HashMap::<_, _, RandomState>::from_iter(
            self.plugin
                .plugin_enums
                .iter()
                .map(|(enum_type, plugin_enum)| (ByAddress(*enum_type), *plugin_enum)),
        );
        for (enum_type, _) in &self.ctx.enum_types {
            let plugin_enum = plugin_enums.get(enum_type);
            writeln!(f, "pub trait {}: ", Declare(enum_type.name))?;
            for dependency in self.plugin.dependencies {
                write!(f, "{}+", generated_trait(dependency, *enum_type.name))?;
            }
            writeln!(f, "{{")?;
            if let Some(plugin_enum) = &plugin_enum {
                if enum_type.is_unit {
                    for variant in plugin_enum.variants.iter() {
                        writeln!(f, "fn {}()->Self;", variant.name)?
                    }
                } else {
                    for variant in plugin_enum.variants.iter() {
                        if variant.is_unit {
                            writeln!(f, "fn {}(&self)->bool;", variant.name)?;
                            writeln!(f, "fn from_{}()->Self;", variant.name)?;
                        } else {
                            writeln!(f, "fn {}(&self)->Option<&{}>;", variant.name, variant.path)?;
                            writeln!(f, "fn from_{}(data: {})->Self;", variant.name, variant.path)?;
                        }
                    }
                }
            }
            writeln!(f, "}}")?;
        }

        write!(f, "pub trait {}: {AST_TRAIT_PATH}+", Declare(&AST_NAME))?;
        for dependency in self.plugin.dependencies {
            write!(f, "{}+", generated_trait(dependency, AST_NAME))?;
        }
        writeln!(f, "{{")?;
        for property in self.plugin.properties {
            writeln!(f, "fn {property}(&self,expr:&{EXPR_ID})->{NODE_ID_LOCAL};")?;
        }
        writeln!(f, "fn add_literal_{}(&mut self,", self.plugin.name)?;
        for plugin in self.ctx.plugins[&ByAddress(self.plugin)]
            .1
            .iter()
            .map(|x| *x.deref())
            .chain(once(self.plugin))
        {
            for property in plugin.properties {
                write!(
                    f,
                    "{property}: Option<{PROJECT_VARIABLE}::{}>,",
                    VALUE_TYPE.name
                )?;
            }
        }
        writeln!(f, ")->{EXPR_ID};")?;
        for expr in self.plugin.exprs {
            writeln!(
                f,
                "fn add_{}(&mut self,{})->{EXPR_ID};",
                expr.name,
                Declare(expr.params)
            )?;
        }
        writeln!(f, "}}")?;

        writeln!(f, "pub mod expr{{")?;
        for expr in self.plugin.exprs {
            writeln!(
                f,
                "pub trait {}{{",
                Declare(&Name {
                    name: expr.name,
                    generics: &Generics::NONE,
                    project_generic: true
                })
            )?;
            writeln!(
                f,
                "fn build(ast:&mut {PROJECT_VARIABLE}::{},output:&{EXPR_ID},{});",
                AST_NAME.non_project_generic(),
                Declare(expr.params)
            )?;
            writeln!(f, "}}")?;
        }
        writeln!(f, "}}")?;
        Ok(())
    }
    fn fmt_bin(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "#[derive(Debug,Default,Copy,Clone,PartialEq,Eq,Hash)]")?;
        writeln!(f, "pub struct {};", Declare(&PROJECT_NAME))?;
        for (plugin, _) in &self.ctx.plugins {
            writeln!(
                f,
                "impl {} for {}{{",
                generated_project_trait(plugin),
                generated_struct(PROJECT_NAME)
            )?;
            for name in
                plugin
                    .enum_types
                    .iter()
                    .map(|x| x.name)
                    .chain(if plugin == &ByAddress(&PLUGIN) {
                        Some(&AST_NAME)
                    } else {
                        None
                    })
            {
                writeln!(
                    f,
                    "type {} = {};",
                    name.non_project_generic(),
                    &WithProject {
                        this: &generated_struct(*name),
                        project: &"Self"
                    }
                )?;
            }
            writeln!(f, "}}")?;
        }

        {
            writeln!(f, "mod code{{")?;
            for (enum_type, plugin_enums) in &self.ctx.plugin_enums {
                let mut code = 0;
                writeln!(f, "pub(super) mod {}{{", enum_type.name.name)?;
                for (plugin, plugin_enum) in plugin_enums {
                    for variant in plugin_enum.variants {
                        writeln!(
                            f,
                            "pub(in super::super) const {}__{}:usize = {code};",
                            plugin.name, variant.name
                        )?;
                        code += 1;
                    }
                }
                writeln!(f, "}}")?;
            }
            writeln!(f, "}}")?;
        }

        writeln!(f, "mod union_{{")?;
        for (enum_type, plugin_enums) in &self.ctx.plugin_enums {
            if !enum_type.is_unit {
                writeln!(f, "{}", enum_type.derives)?;
                writeln!(f, "pub(super) union {}{{", enum_type.name)?;
                for plugin_enum in plugin_enums.values() {
                    for variant in plugin_enum.variants {
                        writeln!(
                            f,
                            "pub(super) {}__{}: {MANUALLY_DROP}<{}>,",
                            plugin_enum.plugin.name, variant.name, variant.path
                        )?;
                    }
                }
                writeln!(
                    f,
                    "_p: core::marker::PhantomData<({})>",
                    &GenericsOf(enum_type.name)
                )?;
                writeln!(f, "}}")?;
            }
        }
        writeln!(f, "}}")?;
        for (enum_type, plugin_enums) in &self.ctx.plugin_enums {
            writeln!(f, "{}", enum_type.derives)?;
            if enum_type.is_unit {
                writeln!(
                    f,
                    "pub struct {}(usize,{PHANTOM_DATA}<{}>);",
                    Declare(enum_type.name),
                    GenericsOf(enum_type.name)
                )?;
            } else {
                writeln!(
                    f,
                    "pub struct {}{{code:usize,data:{}}}",
                    Declare(enum_type.name),
                    generated_union(*enum_type.name)
                )?;
            }

            writeln!(
                f,
                "{}",
                DisplayMarkers {
                    markers: enum_type.markers,
                    enum_type
                }
            )?;

            let principle_trait = generated_principle_trait(enum_type.plugin, *enum_type.name);
            let principle_trait = &principle_trait as &(dyn Display + Sync);
            for (trait_, functions) in enum_type
                .impls
                .iter()
                .map(|x| (x.symbol as &(dyn Display + Sync), x.functions))
                .chain(once((principle_trait, enum_type.functions)))
            {
                writeln!(
                    f,
                    "impl<{}> {} for {}",
                    Declare(&GenericsOf(enum_type.name)),
                    trait_,
                    generated_struct(*enum_type.name),
                )?;
                if !enum_type.use_enum_types.is_empty() {
                    writeln!(f, "where")?;
                    for use_enum_type in enum_type.use_enum_types {
                        writeln!(
                            f,
                            "{PROJECT_VARIABLE}::{}: {},",
                            use_enum_type.name.non_project_generic(),
                            generated_trait(self.plugin, *use_enum_type.name)
                        )?;
                    }
                }
                writeln!(f, "{{")?;
                for function in functions {
                    writeln!(
                        f,
                        "{}{{",
                        FunctionImpl {
                            function,
                            body: None
                        }
                    )?;
                    if let Some(body) = function.enum_impl {
                        body.generate_pre_match(f, enum_type, function)?;
                        if enum_type.is_unit {
                            writeln!(f, "match self.0{{")?;
                        } else {
                            writeln!(f, "match self.code{{")?;
                        }
                        for plugin_enum in plugin_enums.values() {
                            for variant in plugin_enum.variants {
                                writeln!(
                                    f,
                                    "self::code::{}::{}__{}=>{{",
                                    enum_type.name.name, plugin_enum.plugin.name, variant.name
                                )?;
                                body.generate_match_branch(
                                    f,
                                    enum_type,
                                    function,
                                    variant,
                                    plugin_enum.plugin,
                                )?;
                                writeln!(f, "}}")?;
                            }
                        }
                        write!(f, "_=>unreachable!(),")?;
                        writeln!(f, "}}")?;
                    } else {
                        writeln!(f, "unimplemented!();")?;
                    }
                    writeln!(f, "}}")?;
                }
                writeln!(f, "}}")?;
            }

            {
                let mut code2variant = Vec::<(&Variant, &Plugin)>::new();
                for plugin in &self.ctx.enum_types[&ByAddress(enum_type)] {
                    writeln!(
                        f,
                        "impl<{}> {} for {}{{",
                        Declare(&GenericsOf(enum_type.name)),
                        generated_trait(plugin, *enum_type.name),
                        generated_struct(*enum_type.name)
                    )?;
                    if let Some(plugin_enum) =
                        self.ctx.plugin_enums[&ByAddress(enum_type)].get(&ByAddress(plugin))
                    {
                        for variant in plugin_enum.variants {
                            code2variant.push((variant, plugin_enum.plugin));
                            if enum_type.is_unit {
                                writeln!(
                                    f,
                                    "fn {}()->Self{{Self(self::code::{}::{}__{},{PHANTOM_DATA})}}",
                                    variant.name,
                                    enum_type.name.name,
                                    plugin_enum.plugin.name,
                                    variant.name
                                )?
                            } else {
                                if variant.is_unit {
                                    writeln!(
                                        f,
                                        "fn {}(&self)->bool{{self.code==self::code::{}::{}__{}}}",
                                        variant.name,
                                        enum_type.name.name,
                                        plugin_enum.plugin.name,
                                        variant.name
                                    )?;
                                    writeln!(
                                        f,
                                        "fn from_{0}()->Self{{Self{{code:self::code::{1}::{3}__{0},data:self::union_::{1}{2}{{{3}__{0}: {MANUALLY_DROP}::new({4})}} }} }}",
                                        variant.name,
                                        enum_type.name.name,
                                        enum_type.name.generics,
                                        plugin_enum.plugin.name,
                                        variant.path
                                    )?;
                                } else {
                                    writeln!(
                                        f,
                                        "fn {0}(&self)->Option<&{1}>{{if self.code==self::code::{2}::{3}__{4}{{Some(unsafe{{&self.data.{3}__{4}}})}}else{{None}} }}",
                                        variant.name,
                                        variant.path,
                                        enum_type.name.name,
                                        plugin_enum.plugin.name,
                                        variant.name
                                    )?;
                                    writeln!(
                                        f,
                                        "fn from_{0}(data: {1})->Self{{Self{{code:self::code::{2}::{3}__{4},data:self::{UNION}::{2}{{{3}__{4}:{MANUALLY_DROP}::new(data)}} }} }}",
                                        variant.name,
                                        variant.path,
                                        enum_type.name.name,
                                        plugin_enum.plugin.name,
                                        variant.name
                                    )?;
                                }
                            }
                        }
                    }
                    writeln!(f, "}}")?;
                }
            }
        }
        let mut properties_count = 0;
        {
            for (plugin, _) in &self.ctx.plugins {
                writeln!(
                    f,
                    "impl<{}> {} for {}",
                    EqualityProjectGeneric {
                        associated: &AST_NAME.non_project_generic(),
                        target: &generated_struct(AST_NAME),
                    },
                    generated_trait(plugin, AST_NAME),
                    generated_struct(AST_NAME)
                )?;
                writeln!(
                    f,
                    "where {PROJECT_VARIABLE}::{}: {},",
                    OPERATOR_TYPE.name.non_project_generic(),
                    generated_trait(self.plugin, *OPERATOR_TYPE.name)
                )?;
                writeln!(f, "{{")?;
                for property in plugin.properties {
                    writeln!(
                        f,
                        "fn {property}(&self,expr:&{EXPR_ID})->{NODE_ID_LOCAL}{{self.impl_.property(expr,{properties_count})}}"
                    )?;
                }
                properties_count += 1;
                writeln!(f, "fn add_literal_{}(&mut self,", plugin.name)?;
                for plugin in self.ctx.plugins[plugin].1.iter().chain(once(plugin)) {
                    for property in plugin.properties {
                        write!(
                            f,
                            "{property}: Option<{PROJECT_VARIABLE}::{}>,",
                            VALUE_TYPE.name
                        )?;
                    }
                }
                writeln!(f, ")->{EXPR_ID}{{")?;
                writeln!(f, "let expr = <Self as {AST_TRAIT_PATH}>::add_auto(self);")?;
                for plugin in self.ctx.plugins[plugin].1.iter().chain(once(plugin)) {
                    for property in plugin.properties {
                        writeln!(f, "if let Some({property}) = {property}{{")?;
                        writeln!(
                            f,
                            "let node = <Self as {}>::{property}(self,&expr);",
                            generated_trait(plugin, AST_NAME)
                        )?;
                        write!(
                            f,
                            "*self.impl_.module.evaluation_mut(&node)={EVALUATION}::Value({property})"
                        )?;
                        writeln!(f, "}}")?;
                    }
                }
                writeln!(f, "expr")?;
                writeln!(f, "}}")?;
                for expr in plugin.exprs {
                    writeln!(
                        f,
                        "fn add_{}(&mut self,{})->{EXPR_ID}{{",
                        expr.name,
                        Declare(expr.params)
                    )?;
                    writeln!(
                        f,
                        "let output = <Self as {AST_TRAIT_PATH}>::add_auto(self);"
                    )?;
                    for (expr_impl, _) in
                        self.ctx.expr_impls.get(&ByAddress(expr)).unwrap_or(&vec![])
                    {
                        writeln!(
                            f,
                            "<{expr_impl} as {}>::build(self,&output,{});",
                            generated_expr(plugin, expr),
                            expr.params
                        )?;
                    }
                    writeln!(f, "output")?;
                    writeln!(f, "}}")?;
                }
                writeln!(f, "}}")?;
            }

            {
                writeln!(f, "pub struct {}{{", Declare(&AST_NAME))?;
                writeln!(f, "pub impl_: {}", AST_IMPL_PATH)?;
                writeln!(f, "}}")?;
            }
            {
                writeln!(
                    f,
                    "impl<{}> {} for {}{{",
                    Declare(&GenericsOf(&AST_NAME)),
                    generated_principle_trait(&PLUGIN, AST_NAME),
                    generated_struct(AST_NAME),
                )?;
                writeln!(f, "const {PROPERTIES_COUNT}: usize = {properties_count};")?;
                writeln!(f, "fn impl_(&self)->&{AST_IMPL_PATH}{{&self.impl_}}",)?;
                writeln!(
                    f,
                    "fn impl_mut(&mut self)->&mut {AST_IMPL_PATH}{{&mut self.impl_}}",
                )?;
                writeln!(f, "}}")?;
            }
        }
        Ok(())
    }
}

struct DisplayProjectLib<'a>(&'a Project);

impl Display for DisplayProjectLib<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt_lib(f)
    }
}

struct DisplayProjectBin<'a>(&'a Project);

impl Display for DisplayProjectBin<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt_bin(f)
    }
}

#[derive(Default)]
pub struct ProjectAnalysis {
    plugins: HashMap<
        ByAddress<&'static Plugin>,
        (
            HashSet<ByAddress<&'static EnumType>>,
            HashSet<ByAddress<&'static Plugin>>,
        ),
    >,
    enum_types: HashMap<ByAddress<&'static EnumType>, Vec<&'static Plugin>>,
    plugin_enums: HashMap<
        ByAddress<&'static EnumType>,
        HashMap<ByAddress<&'static Plugin>, &'static PluginEnum>,
    >,
    expr_impls: HashMap<ByAddress<&'static Expr>, Vec<(&'static WrittenPathRaw, &'static Plugin)>>,
}

pub struct VariantData {
    pub variant_name: &'static str,
}

impl Display for VariantData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "self.data.{}", self.variant_name)
    }
}
