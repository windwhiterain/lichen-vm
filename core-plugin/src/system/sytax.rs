use std::fmt::Display;

use crate::system::{
    EnumType, PROJECT_NAME, PROJECT_VARIABLE, Plugin, THIS_PROJECT_GENERIC, generate::CTX,
    generate::GeneratingStage, generate::ThisGeneratedLib, utils::generated_struct,
};

pub struct Declare<'a, T>(pub &'a T);
pub struct WithProject<'a, T, P: Display> {
    pub this: &'a T,
    pub project: &'a P,
}
pub struct GenericsOf<'a, T>(pub &'a T);
struct Raw<'a, T>(pub &'a T);

#[derive(Clone, Copy)]
pub struct Name {
    pub name: &'static str,
    pub generics: &'static Generics,
    pub project_generic: bool,
}

impl Name {
    pub const fn raw(name: &'static str) -> Self {
        Self {
            name,
            generics: &Generics::NONE,
            project_generic: false,
        }
    }
    pub fn non_project_generic(&self) -> Self {
        Self {
            name: self.name,
            generics: self.generics,
            project_generic: false,
        }
    }
}

impl Display for Name {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}<{}", self.name, self.generics)?;
        if self.project_generic {
            write!(f, "{}", THIS_PROJECT_GENERIC)?;
        }
        write!(f, ">",)?;
        Ok(())
    }
}

impl Display for Declare<'_, Name> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}<{}", self.0.name, Declare(self.0.generics))?;
        if self.0.project_generic {
            write!(f, "{}", Declare(&THIS_PROJECT_GENERIC))?;
        }
        write!(f, ">")?;
        Ok(())
    }
}

impl<P: Display> Display for WithProject<'_, Name, P> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}::<{}", self.this.name, self.this.generics)?;
        if self.this.project_generic {
            write!(f, "{}", self.project)?;
        }
        write!(f, ">")?;
        Ok(())
    }
}

impl Display for GenericsOf<'_, Name> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0.generics)?;
        if self.0.project_generic {
            write!(f, "{}", THIS_PROJECT_GENERIC)?;
        }
        Ok(())
    }
}

impl Display for Declare<'_, GenericsOf<'_, Name>> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", Declare(self.0.0.generics))?;
        if self.0.0.project_generic {
            write!(f, "{}", Declare(&THIS_PROJECT_GENERIC))?;
        }
        Ok(())
    }
}

pub struct GeneratedLibPath {
    pub plugin: &'static Plugin,
    pub relative: &'static str,
    pub name: Name,
}

impl Display for Raw<'_, GeneratedLibPath> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0.plugin.lib_module)?;
        if !self.0.relative.is_empty() {
            write!(f, "::{}", self.0.relative)?;
        }
        Ok(())
    }
}

impl Display for GeneratedLibPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}::{}", Raw(self), self.name)
    }
}

impl<P: Display> Display for WithProject<'_, GeneratedLibPath, P> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}::{}",
            Raw(self.this),
            &WithProject {
                this: &self.this.name,
                project: self.project
            }
        )
    }
}

pub struct GeneratedBinPath {
    pub relative: &'static str,
    pub name: Name,
}

impl Display for Raw<'_, GeneratedBinPath> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "self")?;
        if !self.0.relative.is_empty() {
            write!(f, "::{}", self.0.relative)?;
        }
        Ok(())
    }
}

impl Display for GeneratedBinPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}::{}", Raw(self), self.name)
    }
}

impl<P: Display> Display for WithProject<'_, GeneratedBinPath, P> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}::{}",
            Raw(self.this),
            &WithProject {
                this: &self.this.name,
                project: self.project
            }
        )
    }
}

pub struct ThisGeneratedLibPath {
    pub relative: &'static str,
    pub name: Name,
}

impl Display for Raw<'_, ThisGeneratedLibPath> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", ThisGeneratedLib)?;
        if !self.0.relative.is_empty() {
            write!(f, "::{}", self.0.relative)?;
        }
        Ok(())
    }
}

impl Display for ThisGeneratedLibPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}::{}", Raw(self), self.name)
    }
}

impl<P: Display> Display for WithProject<'_, ThisGeneratedLibPath, P> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}::{}",
            Raw(self.this),
            &WithProject {
                this: &self.this.name,
                project: self.project
            }
        )
    }
}

pub struct WrittenPath {
    pub crate_: &'static str,
    pub path: &'static str,
    pub generics: &'static Generics,
    pub project_generic: bool,
}

impl WrittenPath {
    pub const fn raw(crate_: &'static str, path: &'static str) -> Self {
        Self {
            crate_,
            path,
            generics: &Generics::NONE,
            project_generic: false,
        }
    }
}

impl Display for Raw<'_, WrittenPath> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}::{}", Crate(self.0.crate_), self.0.path)
    }
}

impl Display for WrittenPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}::<{}", Raw(self), self.generics)?;
        if self.project_generic {
            write!(f, "{}", THIS_PROJECT_GENERIC)?;
        }
        write!(f, ">")?;
        Ok(())
    }
}

pub struct Path {
    pub path: &'static str,
    pub generics: &'static Generics,
    pub project_generic: bool,
}

impl Path {
    pub const fn raw(path: &'static str) -> Self {
        Self {
            path,
            generics: &Generics::NONE,
            project_generic: false,
        }
    }
}

impl Display for Raw<'_, Path> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0.path)
    }
}

impl Display for Path {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}::<{}", Raw(self), self.generics)?;
        if self.project_generic {
            write!(f, "{}", THIS_PROJECT_GENERIC)?;
        }
        write!(f, ">")?;
        Ok(())
    }
}

pub struct WrittenPathRaw {
    pub crate_: &'static str,
    pub path: &'static str,
}

impl Display for WrittenPathRaw {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}::{}", Crate(self.crate_), self.path)
    }
}

pub struct EqualityProjectGeneric<'a> {
    pub associated: &'a (dyn Display + Sync),
    pub target: &'a (dyn Display + Sync),
}

impl Display for EqualityProjectGeneric<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{PROJECT_VARIABLE}: {ThisGeneratedLib}::{}<{} = {}>",
            PROJECT_NAME.name, self.associated, self.target
        )
    }
}

pub struct DisplayArray<'a>(pub &'a [&'static (dyn Display + Sync)]);
impl Display for DisplayArray<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for i in self.0 {
            write!(f, "{i}")?;
        }
        Ok(())
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

pub struct Generic {
    pub name: &'static str,
    pub constraints: &'static [&'static (dyn Display + Sync)],
}

impl Display for Generic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{},", self.name)?;
        Ok(())
    }
}

impl Display for Declare<'_, Generic> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: ", self.0.name)?;
        for constraint in self.0.constraints {
            write!(f, "{}+", constraint)?;
        }
        Ok(())
    }
}

pub struct Generics(pub &'static [&'static Generic]);

impl Generics {
    pub const NONE: Self = Self(&[]);
}

impl Display for Generics {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for generic in self.0 {
            write!(f, "{}", generic)?;
        }
        Ok(())
    }
}

impl Display for Declare<'_, Generics> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for generic in self.0.0 {
            write!(f, "{}", Declare(*generic))?;
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
pub struct DisplayMarkers {
    pub markers: &'static [&'static str],
    pub enum_type: &'static EnumType,
}
impl Display for DisplayMarkers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for marker in self.markers {
            writeln!(
                f,
                "impl<{}> {} for {}{{}}",
                Declare(&GenericsOf(self.enum_type.name)),
                marker,
                generated_struct(*self.enum_type.name)
            )?;
        }
        Ok(())
    }
}

pub struct Crate(&'static str);

impl Display for Crate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let ctx = CTX.get().unwrap();
        if matches!(ctx.current, GeneratingStage::Lib) && ctx.plugin.lib_module.crate_ == self.0 {
            write!(f, "crate")
        } else {
            write!(f, "::{}", self.0)
        }
    }
}
